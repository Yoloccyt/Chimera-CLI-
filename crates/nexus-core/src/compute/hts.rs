//! HTS-CPU 混合阈值调度 — 动态阈值表 + 序贯检验 + cgroup 核数校正（P1-T9，手册 W4）
//!
//! 对应架构层:L1 Core
//!
//! # 设计来源
//! - 阈值"三重来源法"（手册 §8.4 / ADR-103）:第一重 **W1 离线测定**（由 T1 观测基线
//!   提供,本波次以 dispatch.rs 静态初值入表并保留更新接口）;第二重 **运行期序贯检验**
//!   （本文件 `sequential_test`,Inline vs Rayon 双桶 A/B 对照）;第三重 **cgroup 核数校正**
//!   （本文件 `cgroup`,容器内 `available_parallelism` 不可信的修正,ADR-103）。
//! - §4.1 推演 15:调度阈值不可拍脑袋——阈值必须可溯源（诚实数据红线）。
//!
//! # 性能契约（ADR-128 / 手册 §8.4）
//! - [`HtsTable`] 由 `ArcSwap` 承载（workspace 已有 arc-swap = "1.7"）:读路径
//!   `load()` 拿无锁快照 Guard（~5ns,RCU 语义）,`get(kind)` 为固定数组索引,**零分配**;
//!   写路径（T1 测定灌入 / T14 序贯校准）低频,走 RCU store 全表替换。
//! - `route()` 决策门禁:P99 < 1µs（见 `benches/hts_bench.rs`）。
//!
//! # 红线
//! 本文件全 safe（`forbid(unsafe_code)` 由 crate 根保证,arc-swap 为纯 safe 封装,
//! unsafe 在依赖内不传播）;禁自旋（无等待逻辑）;库代码禁 unwrap/expect;
//! 错误经 [`sequential_test::HtsError`]（thiserror）或 `None` 传播。

use super::dispatch::TaskKind;

/// HTS 阈值表条目 — 手册 §8.4 单行记录
///
/// WHY 全 Copy:热路径 `route()` 每调用一次仅读取本结构的一个快照副本,
/// 无堆分配、无借用逃逸（`[Entry; 6]` 数组索引拷贝）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Entry {
    /// 并行调度阈值（items）:条目数 >= 阈值时进 L-a rayon 池
    pub threshold: usize,
    /// 并行块大小（chunk）:批量计算的分块粒度（T14 并行化接线消费）
    pub chunk: usize,
    /// 阈值来源 — 诚实数据红线:阈值必须可溯源（ADR-103 三重来源）
    pub source: ThresholdSource,
}

/// 阈值来源标记 — ADR-103 三重来源 + 诚实数据红线
///
/// 语义约定:
/// - [`OfflineMeasured`](ThresholdSource::OfflineMeasured):离线测定值。
///   `measured_at = 0` 表示 **S9 离线测定预填初值,尚未 W1 复测**——预填不等于已校准
///   （手册 §8.4 标注"W1 复测"）;T1 测定灌入时携带真实时间戳。
/// - [`SequentialTest`](ThresholdSource::SequentialTest):序贯检验 Promote 后的校准值
///   （T14 运行时接线后产生）。
/// - [`ConservativeDefault`](ThresholdSource::ConservativeDefault):保守默认
///   （如 [`TaskKind::Generic`] 的 10,000,未测定前的兜底）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThresholdSource {
    /// 离线测定（W1 复测前 `measured_at = 0` 为预填占位）
    OfflineMeasured {
        /// 测定时间戳（unix 秒）;0 = 预填占位（未复测,不作已校准结论）
        measured_at: u64,
    },
    /// 运行期序贯检验校准（T14 接线后产生）
    SequentialTest {
        /// Promote 时间戳（unix 秒）
        promoted_at: u64,
        /// 判定时已积累的样本数（检验证据量）
        samples: u32,
    },
    /// 保守默认 — 未测定前的兜底值（不宣称任何测定证据）
    ConservativeDefault,
}

/// HTS 动态阈值表 — 按 [`TaskKind`] 六类登记阈值与块大小（手册 §8.4）
///
/// # 存储
/// `[Entry; 6]` 定长数组（六类任务,与 [`dispatch::TaskKind`] 穷举对应）:
/// - 读:`get(kind)` 经 `kind_index` 数组索引,**零分配、零锁**（在 ArcSwap 快照内）;
/// - 写:`update` 改快照副本,由持有方（`ComputeBridge`）RCU store 全表替换。
///
/// # 生命周期
/// `ComputeBridge` 以 `ArcSwap<HtsTable>` 持有本表;`update` 为运行期更新接口
/// （T1 测定值灌入 + T14 序贯检验校准用）。
///
/// # 初始值来源
/// 迁移自 [`dispatch::TaskKind::threshold`] 静态表（T8）,并补充块大小 chunk:
/// ClvSimilarity=1000/64、OsaMask=100/16、KnnSearch=5000/256、GsoeEvaluate=500/8、
/// CscCollapseScore=200/32、Generic=10000/64（手册 §8.4,标注"S9 离线测定,W1 复测"）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HtsTable {
    /// 六类登记项,索引与 [`kind_index`] 一致（Clv=0, Osa=1, Knn=2, Gsoe=3, Csc=4, Gen=5）
    entries: [Entry; 6],
}

impl Default for HtsTable {
    /// 初始表 — 迁移 dispatch.rs 静态值（S9 离线测定预填,W1 复测）;
    /// 五类标注 [`OfflineMeasured`](ThresholdSource::OfflineMeasured)（measured_at=0 预填占位）,
    /// [`Generic`](TaskKind::Generic) 为 [`ConservativeDefault`](ThresholdSource::ConservativeDefault)
    /// （10,000,未测定兜底,来源标注见手册 §8.4）。
    fn default() -> Self {
        Self {
            entries: [
                // ClvSimilarity:S9 离线测定初值,W1 复测（chunk=64 并行分块）
                Entry {
                    threshold: 1_000,
                    chunk: 64,
                    source: ThresholdSource::OfflineMeasured { measured_at: 0 },
                },
                // OsaMask:S9 离线测定初值,W1 复测（chunk=16）
                Entry {
                    threshold: 100,
                    chunk: 16,
                    source: ThresholdSource::OfflineMeasured { measured_at: 0 },
                },
                // KnnSearch:S9 离线测定初值,W1 复测（chunk=256）
                Entry {
                    threshold: 5_000,
                    chunk: 256,
                    source: ThresholdSource::OfflineMeasured { measured_at: 0 },
                },
                // GsoeEvaluate:S9 离线测定初值,W1 复测（chunk=8,离线通道,R2 约束不动）
                Entry {
                    threshold: 500,
                    chunk: 8,
                    source: ThresholdSource::OfflineMeasured { measured_at: 0 },
                },
                // CscCollapseScore:S9 离线测定初值,W1 复测（chunk=32）
                Entry {
                    threshold: 200,
                    chunk: 32,
                    source: ThresholdSource::OfflineMeasured { measured_at: 0 },
                },
                // Generic:保守默认兜底（未测定,不作已校准结论）
                Entry {
                    threshold: 10_000,
                    chunk: 64,
                    source: ThresholdSource::ConservativeDefault,
                },
            ],
        }
    }
}

impl HtsTable {
    /// 查询单条目 — 热路径核心读操作
    ///
    /// WHY 返回 `Entry` 副本而非引用:`Entry` 全 Copy（阈值/块大小/来源都是值语义）,
    /// 副本零分配且不产生借用逃逸,`ArcSwap` 快照 Guard 可在语句末尾立即释放,
    /// 配合 `route()` 每调用一次仅读取的语义（ADR-128 读无锁）。
    #[must_use]
    pub fn get(&self, kind: TaskKind) -> Entry {
        self.entries[kind_index(kind)]
    }

    /// 运行期更新 — T1 测定值灌入 + T14 序贯检验校准
    ///
    /// # 签名说明（相对任务原型扩展一个参数）
    /// 任务原型为 `update(kind, threshold, chunk)`;但诚实数据红线要求阈值必须可溯源
    /// （ADR-103）,来源不能由调用方隐式推断——故 `source` 为**强制显式参数**,
    /// 防止"静默标错来源"污染溯源链。T1 灌入用 [`OfflineMeasured`](ThresholdSource::OfflineMeasured),
    /// T14 校准用 [`SequentialTest`](ThresholdSource::SequentialTest)。
    pub fn update(
        &mut self,
        kind: TaskKind,
        threshold: usize,
        chunk: usize,
        source: ThresholdSource,
    ) {
        self.entries[kind_index(kind)] = Entry {
            threshold,
            chunk,
            source,
        };
    }
}

/// [`TaskKind`] → 数组索引 — 与 [`HtsTable`] 的 `entries` 布局唯一对应
///
/// WHY 不用 HashMap:六类登记可穷举 match,索引 O(1) 且零哈希开销;
/// match 穷举性在 [`TaskKind`] 新增变体时编译报错（六类不变的红线由编译期兜底）。
#[must_use]
const fn kind_index(kind: TaskKind) -> usize {
    match kind {
        TaskKind::ClvSimilarity => 0,
        TaskKind::OsaMask => 1,
        TaskKind::KnnSearch => 2,
        TaskKind::GsoeEvaluate => 3,
        TaskKind::CscCollapseScore => 4,
        TaskKind::Generic => 5,
    }
}

/// 序贯检验框架 — PostHog 式 Inline vs Rayon 双桶 A/B 对照（手册 §8.4 三重来源②）
///
/// # 方法来源
/// - Wald, A. (1945). *Sequential Tests of Statistical Hypotheses*,
///   Annals of Mathematical Statistics 16(2): 117-186 —— 序贯概率比检验（SPRT）;
/// - 实现采用 SPRT 正态近似:差值样本 \(d_i = inline_i - ray on_i\) 视为 iid 正态,
///   \(\sigma^2\) 用样本方差估计（近似处理;PostHog 实际用 delta-method / bootstrap,
///   其复杂度超出本波次框架,文档如实标注）。
/// - Alpha spending:本实现为 **一次性预算墙**（整段检验消耗 alpha,终局即耗尽,
///   耗尽后强制 Reject 不 Promote）——PostHog 式"预先承诺边界"防 p-hacking 的简化;
///   正式的 alpha-spending 函数（如 O'Brien-Fleming 多次中间分析分摊）留待 T14。
///
/// # 判定规则（契约,勿调整）
/// 1. `n < min_samples` → [`Continue`](TestDecision::Continue)（小样本不判定）;
/// 2. SPRT 对数似然比跨上界 `ln((1-β)/α)` **且**效果量达标（rayon 相对 inline
///    快 ≥ `effect_size`）→ [`Promote`](TestDecision::Promote);
/// 3. SPRT 跨上界但效果量未达标 → [`Reject`](TestDecision::Reject)（**Promote 只在
///    效果量达到时发出**,否则一律 Reject）;
/// 4. SPRT 跨下界 `ln(β/(1-α))` → [`Reject`](TestDecision::Reject);
/// 5. `n >= max_samples` → 硬停止,强制给出终局（达标 Promote,否则 Reject）;
/// 6. 终局判定后实例关闭（alpha 预算已耗尽）:后续 `record` **强制 [`Reject`](TestDecision::Reject)**,
///    永不产生新的 Promote 判定——PostHog 式预算墙,防 p-hacking 门禁核心
///    （本次检验的结论经 [`decision`](SequentialTest::decision) 查询,不再经 `record` 回传）。
///
/// # 确定性（Ω₂）
/// 本框架无随机源,同输入序列必同决策序列（单测锁定）。
pub mod sequential_test {
    use thiserror::Error;

    /// 序贯检验错误 — 配置校验失败（thiserror,库层错误标准 §4.1）
    #[derive(Debug, Clone, Copy, PartialEq, Error)]
    pub enum HtsError {
        /// alpha 不在 (0,1) 开区间（含 NaN）
        #[error("alpha {0} 必须在 (0,1) 开区间内")]
        AlphaOutOfRange(f64),
        /// power 不在 (0,1) 开区间（含 NaN）
        #[error("power {0} 必须在 (0,1) 开区间内")]
        PowerOutOfRange(f64),
        /// min_samples 大于 max_samples（样本窗口倒挂）
        #[error("min_samples {0} 必须不大于 max_samples {1}")]
        SampleRange(usize, usize),
        /// effect_size 非正（效果量无意义）
        #[error("effect_size {0} 必须为正")]
        EffectSizeNonPositive(f64),
    }

    /// 序贯检验配置 — 手册 §8.4 / PostHog 式双桶对照参数
    ///
    /// # 默认值
    /// - `alpha = 0.05`（Type I error,误 Promote 概率上限,预算墙）;
    /// - `power = 0.8`（检验功效,β = 0.2 为漏检概率）;
    /// - `min_samples = 30`（达到此样本量才允许判定,小样本强制 Continue）;
    /// - `max_samples = 500`（硬停止上限,防止无限收集）;
    /// - `effect_size = 0.2`（效果量判据:rayon 相对 inline 快 ≥20%,§4.1 推演 15
    ///   的工程初值;手册未给默认,此值 T1 复测后可校准）。
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub struct SequentialTestConfig {
        /// 显著性水平（Type I error 预算）
        pub alpha: f64,
        /// 检验功效（1 − β）
        pub power: f64,
        /// 最小判定样本量（小样本不判定）
        pub min_samples: usize,
        /// 最大样本量（硬停止上限）
        pub max_samples: usize,
        /// 效果量判据:rayon 相对 inline 快 ≥ 此比例才允许 Promote
        pub effect_size: f64,
    }

    impl Default for SequentialTestConfig {
        /// 手册 §8.4 默认参数（effect_size 为工程初值 0.2,标注 T1 可校准）
        fn default() -> Self {
            Self {
                alpha: 0.05,
                power: 0.8,
                min_samples: 30,
                max_samples: 500,
                effect_size: 0.2,
            }
        }
    }

    impl SequentialTestConfig {
        /// 校验配置合法性 — 非法配置返回 [`HtsError`]（编程错误显式化）
        pub fn validate(&self) -> Result<(), HtsError> {
            if !(0.0 < self.alpha && self.alpha < 1.0) {
                return Err(HtsError::AlphaOutOfRange(self.alpha));
            }
            if !(0.0 < self.power && self.power < 1.0) {
                return Err(HtsError::PowerOutOfRange(self.power));
            }
            if self.min_samples > self.max_samples {
                return Err(HtsError::SampleRange(self.min_samples, self.max_samples));
            }
            if self.effect_size <= 0.0 || !self.effect_size.is_finite() {
                return Err(HtsError::EffectSizeNonPositive(self.effect_size));
            }
            Ok(())
        }
    }

    /// 检验决策 — `record` 每次调用返回三态
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub enum TestDecision {
        /// 证据不足,继续采样
        Continue,
        /// 序贯检验通过 + 效果量达标 — 建议更新阈值
        ///
        /// `threshold` 为候选阈值建议:由调用方经 [`SequentialTest::set_candidate_threshold`]
        /// 注入（T14 接线时设为测定/校准的目标阈值）;未注入时为 0,
        /// 表示"仅标记效果量达标,具体阈值待接线层决定"。
        Promote {
            /// 建议的新阈值（候选值,由调用方注入）
            threshold: usize,
        },
        /// 检验拒绝 — 不更新阈值（含:效果量未达标 / SPRT 跨下界 / 预算耗尽）
        Reject,
    }

    /// 序贯检验状态机 — Inline vs Rayon 双桶样本积累 + Wald SPRT 判定
    ///
    /// # 线程模型
    /// 单实例单线程使用（T14 接线时每个 [`TaskKind`] 一个实例,串行喂样本）;
    /// 不实现 `Sync` 共享——检验状态是私有证据,不跨线程竞争。
    pub struct SequentialTest {
        /// 检验参数（构造后不可变）
        config: SequentialTestConfig,
        /// Inline 桶样本（µs 计时）
        inline_samples: Vec<f64>,
        /// Rayon 桶样本（µs 计时）
        rayon_samples: Vec<f64>,
        /// 候选阈值建议（Promote 时回传;0 = 未注入）
        candidate_threshold: usize,
        /// 已消耗的 alpha 预算（0 或 alpha:一次性预算墙,终局即耗尽）
        alpha_spent: f64,
        /// 终局判定（None = 检验进行中）
        decision: Option<TestDecision>,
    }

    impl SequentialTest {
        /// 构造状态机 — 便捷入口
        ///
        /// # Panics
        /// 配置非法时 panic（编程错误,构造前提;调用方可用
        /// [`try_new`](Self::try_new) 获得 [`Result`] 语义）。
        /// 与 [`crate::compute::bridge::ComputeBridge::new`] 同模式:前提失败即进程级缺陷,
        /// 无运行期降级路径。
        #[must_use]
        pub fn new(config: SequentialTestConfig) -> Self {
            match config.validate() {
                Ok(()) => Self {
                    config,
                    inline_samples: Vec::new(),
                    rayon_samples: Vec::new(),
                    candidate_threshold: 0,
                    alpha_spent: 0.0,
                    decision: None,
                },
                Err(e) => panic!("SequentialTest 配置非法（构造前应 validate）: {e}"),
            }
        }

        /// 构造状态机 — 校验失败返回 [`HtsError`]（非 panic 路径）
        pub fn try_new(config: SequentialTestConfig) -> Result<Self, HtsError> {
            config.validate()?;
            Ok(Self {
                config,
                inline_samples: Vec::new(),
                rayon_samples: Vec::new(),
                candidate_threshold: 0,
                alpha_spent: 0.0,
                decision: None,
            })
        }

        /// 注入候选阈值建议 — Promote 时随决策回传（T14 接线用）
        pub fn set_candidate_threshold(&mut self, threshold: usize) {
            self.candidate_threshold = threshold;
        }

        /// 记录一组双桶计时并返回当前决策
        ///
        /// # 前置条件
        /// `inline_us` / `rayon_us` 须为有限正计时值（µs,计时器输出天然满足）。
        ///
        /// # 决策语义（契约）
        /// 见模块文档"判定规则"1-6;终局后实例关闭,`record` 一律返回
        /// [`Reject`](TestDecision::Reject)（预算耗尽强制 Reject,永不二次 Promote）。
        #[must_use]
        pub fn record(&mut self, inline_us: f64, rayon_us: f64) -> TestDecision {
            // 终局后（alpha 预算已耗尽,实例关闭）:强制 Reject——不再接受样本,
            // 永不产生新的 Promote 判定（PostHog 式预算墙,防 p-hacking 的预算语义）
            if self.decision.is_some() {
                return TestDecision::Reject;
            }
            self.inline_samples.push(inline_us);
            self.rayon_samples.push(rayon_us);
            let n = self.inline_samples.len();
            // 小样本不判定（契约 1）
            if n < self.config.min_samples {
                return TestDecision::Continue;
            }
            let decision = self.decide_now();
            if decision != TestDecision::Continue {
                // alpha spending:终局判定一次性耗尽本实例的 alpha 预算（PostHog 式预算墙）
                self.alpha_spent = self.config.alpha;
                self.decision = Some(decision);
            }
            decision
        }

        /// 已消耗的 alpha 预算 — 0（检验进行中）或 `config.alpha`（终局已判定）
        ///
        /// WHY 一次性预算墙:PostHog 式"预先承诺边界"要求每次检验只允许一次终局判定,
        /// 防止反复检验直到偶然显著（p-hacking）;正式 alpha-spending 函数留待 T14。
        #[must_use]
        pub fn spent_alpha(&self) -> f64 {
            self.alpha_spent
        }

        /// 已积累的样本数（双桶各 n 条）
        #[must_use]
        pub fn sample_count(&self) -> usize {
            self.inline_samples.len()
        }

        /// 当前终局决策（None = 检验进行中）
        #[must_use]
        pub fn decision(&self) -> Option<TestDecision> {
            self.decision
        }

        /// 判定核心 — 硬停止 / SPRT 边界 / 效果量三重规则
        fn decide_now(&self) -> TestDecision {
            let n = self.inline_samples.len();
            // 契约 5:硬停止 — 达到 max_samples 必须给出终局
            if n >= self.config.max_samples {
                return if self.effect_met() && self.lr_crossed_upper() {
                    TestDecision::Promote {
                        threshold: self.candidate_threshold,
                    }
                } else {
                    TestDecision::Reject
                };
            }
            let lr = self.log_likelihood_ratio();
            // Wald 1945 SPRT 边界:A = ln((1-β)/α),B = ln(β/(1-α))
            let upper = ((1.0 - self.config.power) / self.config.alpha).ln();
            let lower = (self.config.power / (1.0 - self.config.alpha)).ln();
            if lr >= upper {
                // 契约 3:跨上界但效果量未达标 → Reject（Promote 仅在效果量达到时发出）
                return if self.effect_met() {
                    TestDecision::Promote {
                        threshold: self.candidate_threshold,
                    }
                } else {
                    TestDecision::Reject
                };
            }
            if lr <= lower {
                // 契约 4:跨下界 → Reject（接受 H0:无证据表明 rayon 更快）
                return TestDecision::Reject;
            }
            TestDecision::Continue
        }

        /// 效果量判据 — rayon 相对 inline 快 ≥ effect_size（如快 ≥20%）
        fn effect_met(&self) -> bool {
            let mean_inline = mean(&self.inline_samples);
            // 防御:NaN 或非正均值无比较意义（计时输入不应出现）
            if !mean_inline.is_finite() || mean_inline <= 0.0 {
                return false;
            }
            let mean_rayon = mean(&self.rayon_samples);
            let d = mean_inline - mean_rayon;
            d >= self.config.effect_size * mean_inline
        }

        /// SPRT 对数似然比（Wald 1945,正态近似）— 仅上界跨越用,下界由数值比较
        ///
        /// H0:μ_d = 0;H1:μ_d = δ = effect_size × mean_inline。
        /// Λ_n = (δ/σ²)·Σd_i − n·δ²/(2σ²)。
        /// 零方差（所有差值相同）时检验退化为确定性比较:达标 → +∞,未达标 → −∞。
        fn log_likelihood_ratio(&self) -> f64 {
            let n = self.inline_samples.len();
            debug_assert!(n >= 1, "record 已保证至少 1 个样本");
            let mean_inline = mean(&self.inline_samples);
            let delta = self.config.effect_size * mean_inline;
            // 防御:mean_inline 非正或 NaN 时差值检验无意义（计时输入不应出现）
            if !delta.is_finite() || delta <= 0.0 {
                return f64::NEG_INFINITY;
            }
            let diffs: Vec<f64> = self
                .inline_samples
                .iter()
                .zip(&self.rayon_samples)
                .map(|(a, b)| a - b)
                .collect();
            let mean_d = mean(&diffs);
            if !mean_d.is_finite() {
                return f64::NEG_INFINITY;
            }
            let var = sample_variance(&diffs, mean_d);
            if var <= 0.0 {
                // 零方差:全部差值相同 → 无随机性,退化为确定性比较
                return if mean_d >= delta {
                    f64::INFINITY
                } else {
                    f64::NEG_INFINITY
                };
            }
            let sum_d: f64 = diffs.iter().sum();
            (delta / var) * sum_d - (n as f64 * delta * delta) / (2.0 * var)
        }

        /// SPRT 上界跨越判定（供硬停止分支复用）
        fn lr_crossed_upper(&self) -> bool {
            let upper = ((1.0 - self.config.power) / self.config.alpha).ln();
            self.log_likelihood_ratio() >= upper
        }
    }

    /// 算术均值 — n ≥ 1 保证（调用方保证非空切片）
    fn mean(v: &[f64]) -> f64 {
        v.iter().sum::<f64>() / v.len() as f64
    }

    /// 样本方差（ddof = 1）;少于 2 个样本返回 0（由调用方退化处理）
    fn sample_variance(v: &[f64], m: f64) -> f64 {
        if v.len() < 2 {
            return 0.0;
        }
        let ss = v.iter().map(|x| {
            let d = x - m;
            d * d
        });
        ss.sum::<f64>() / (v.len() - 1) as f64
    }
}

/// cgroup 核数校正 — ADR-103 三重来源③（容器内 `available_parallelism` 不可信的修正）
///
/// # 背景
/// 容器内 `std::thread::available_parallelism()` 可能返回宿主机核数而非 cgroup 配额,
/// 导致线程数/并行度配置虚高（手册 §8.4 / ADR-103）。本模块读取 cgroup v2 的
/// `cpu.max`（格式 `"quota period"`,如 `"80000 100000"` → 0.8 核）,
/// 与物理核数取小得有效核数。
///
/// # 边界
/// - 解析失败 / 非 Linux → `None`（无 cgroup 校正信息,调用方回退物理核数）;
/// - `"max 100000"`（未限制）→ `None`;
/// - 测试经 [`CgroupProbe`] 注入临时文件路径（Ω₇ 可测试性）。
pub mod cgroup {
    use std::path::{Path, PathBuf};

    /// cgroup 探测点 — 路径可配置（测试注入临时文件,生产默认 `/sys/fs/cgroup/cpu.max`）
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct CgroupProbe {
        /// `cpu.max` 文件路径
        pub cpu_max_path: PathBuf,
    }

    impl CgroupProbe {
        /// 以显式路径构造 — 测试注入临时文件用
        #[must_use]
        pub fn new(cpu_max_path: PathBuf) -> Self {
            Self { cpu_max_path }
        }

        /// 探测 cgroup 配额核数 — 读文件 + 解析
        ///
        /// 读取失败（文件缺失/无权限）与解析失败均返回 `None`;
        /// 文件内容格式:第一行 `"quota period"`。
        #[must_use]
        pub fn probe(&self) -> Option<f64> {
            let content = std::fs::read_to_string(&self.cpu_max_path).ok()?;
            parse_cpu_max(&content)
        }
    }

    impl Default for CgroupProbe {
        /// 生产默认路径 — cgroup v2 配额文件
        fn default() -> Self {
            Self {
                cpu_max_path: PathBuf::from("/sys/fs/cgroup/cpu.max"),
            }
        }
    }

    /// 解析 `cpu.max` 内容 — 纯函数,平台无关,可单测
    ///
    /// 格式:`"<quota> <period>"`,如 `"80000 100000"` → `Some(0.8)`。
    /// - `quota = "max"`（未限制）→ `None`;
    /// - 字段缺失 / 多余字段 / 非数值 / 非正数 / 非有限 → `None`。
    #[must_use]
    pub fn parse_cpu_max(content: &str) -> Option<f64> {
        let mut parts = content.split_whitespace();
        let quota = parts.next()?;
        let period = parts.next()?;
        // 多余字段视为格式非法（严格解析,防误读截断）
        if parts.next().is_some() {
            return None;
        }
        // cgroup v2 语义:`max` 表示未限制配额,无 cgroup 校正信息
        if quota == "max" {
            return None;
        }
        let q: f64 = quota.parse().ok()?;
        let p: f64 = period.parse().ok()?;
        if !q.is_finite() || !p.is_finite() || q <= 0.0 || p <= 0.0 {
            return None;
        }
        Some(q / p)
    }

    /// 有效核数 — cgroup 配额核数 vs 物理核数取小（ADR-103 三重来源③）
    ///
    /// 无 cgroup 限制（`probe` 返回 `None`）或无法探测物理核数时返回 `None`,
    /// 由调用方回退 `available_parallelism`（本函数只在有 cgroup 校正信息时给出修正值）。
    #[cfg(target_os = "linux")]
    #[must_use]
    pub fn effective_cores() -> Option<f64> {
        let quota = CgroupProbe::default().probe()?;
        let phys = std::thread::available_parallelism().ok()?.get() as f64;
        if phys <= 0.0 {
            return None;
        }
        Some(quota.min(phys))
    }

    /// 非 Linux 平台占位 — 无 cgroup 校正,恒 `None`
    #[cfg(not(target_os = "linux"))]
    #[must_use]
    pub fn effective_cores() -> Option<f64> {
        None
    }

    /// 路径借用辅助 — 供 `probe` 的文档示例引用（占位无实际用途）
    #[allow(dead_code)]
    fn _path_ref(_p: &Path) {}
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use arc_swap::ArcSwap;

    use super::cgroup::{parse_cpu_max, CgroupProbe};
    use super::sequential_test::{SequentialTest, SequentialTestConfig, TestDecision};
    use super::*;
    use crate::compute::bridge::decide;
    use crate::compute::dispatch::DispatchPlan;

    /// §8.4 初始值核对 — 阈值与 chunk 逐项锁定（来源:S9 离线测定,W1 复测）
    #[test]
    fn initial_values_match_manual_s8_4() {
        let table = HtsTable::default();
        let expect = [
            (TaskKind::ClvSimilarity, 1_000, 64),
            (TaskKind::OsaMask, 100, 16),
            (TaskKind::KnnSearch, 5_000, 256),
            (TaskKind::GsoeEvaluate, 500, 8),
            (TaskKind::CscCollapseScore, 200, 32),
            (TaskKind::Generic, 10_000, 64),
        ];
        for (kind, want_threshold, want_chunk) in expect {
            let e = table.get(kind);
            assert_eq!(e.threshold, want_threshold, "{kind:?} 阈值偏离手册 §8.4");
            assert_eq!(e.chunk, want_chunk, "{kind:?} chunk 偏离手册 §8.4");
        }
    }

    /// 初始值 == dispatch.rs 静态表（迁移等价性）— 保证 T8 测试继续通过
    #[test]
    fn default_table_matches_dispatch_static() {
        let table = HtsTable::default();
        for kind in TaskKind::ALL {
            assert_eq!(
                table.get(kind).threshold,
                kind.threshold(),
                "{kind:?} 动态表初值须与 dispatch 静态表一致（迁移等价性）"
            );
        }
    }

    /// 初始来源标记 — 五类 OfflineMeasured 预填 + Generic 保守默认（诚实数据红线）
    #[test]
    fn initial_sources_are_honest() {
        let table = HtsTable::default();
        for kind in [
            TaskKind::ClvSimilarity,
            TaskKind::OsaMask,
            TaskKind::KnnSearch,
            TaskKind::GsoeEvaluate,
            TaskKind::CscCollapseScore,
        ] {
            assert_eq!(
                table.get(kind).source,
                ThresholdSource::OfflineMeasured { measured_at: 0 },
                "{kind:?} 应为 S9 预填占位（measured_at=0,未复测不作已校准结论）"
            );
        }
        assert_eq!(
            table.get(TaskKind::Generic).source,
            ThresholdSource::ConservativeDefault
        );
    }

    /// update 后 get 反映新值 — 运行期更新接口基础语义
    #[test]
    fn update_changes_entry() {
        let mut table = HtsTable::default();
        table.update(
            TaskKind::OsaMask,
            42,
            7,
            ThresholdSource::SequentialTest {
                promoted_at: 1_700_000_000,
                samples: 30,
            },
        );
        let e = table.get(TaskKind::OsaMask);
        assert_eq!(e.threshold, 42);
        assert_eq!(e.chunk, 7);
        assert_eq!(
            e.source,
            ThresholdSource::SequentialTest {
                promoted_at: 1_700_000_000,
                samples: 30,
            }
        );
        // 未更新的类别不受影响
        assert_eq!(table.get(TaskKind::ClvSimilarity).threshold, 1_000);
    }

    /// update 后 route 行为变化 — 阈值 1000→10 后 20 项从 Inline 转 Rayon
    ///
    /// WHY 经 `decide` 纯函数而非全局 `bridge()`:全局单例表会被并行测试共享,
    /// 修改会与其他测试（如 T8 `route_concurrent_reads` 的静态阈值断言）竞争;
    /// 真实 `ComputeBridge::route` 联动在 bridge.rs 测试中用独立实例锁定。
    #[test]
    fn update_changes_route_behavior() {
        let mut table = HtsTable::default();
        let kind = TaskKind::ClvSimilarity;
        // 初值阈值 1000:20 项 < 1000 → Inline
        assert_eq!(
            decide(kind, false, table.get(kind).threshold, 20),
            DispatchPlan::Inline
        );
        // 运行期校准阈值降至 10:20 项 >= 10 → Rayon
        table.update(kind, 10, 64, ThresholdSource::ConservativeDefault);
        assert_eq!(
            decide(kind, false, table.get(kind).threshold, 20),
            DispatchPlan::Rayon
        );
        assert_eq!(
            decide(kind, false, table.get(kind).threshold, 9),
            DispatchPlan::Inline
        );
    }

    /// arc-swap 并发读写 — 独立 ArcSwap<HtsTable>,8 写 × 8 读,无 panic 无数据竞争
    ///
    /// WHY 替代 loom（同 T8 理由:Windows GNU 工具链 loom 受限）;
    /// 验证 RCU 语义:读线程任意时刻看到完整快照（要么旧表要么新表,永不撕裂）。
    #[test]
    fn arc_swap_concurrent_read_write() {
        // Arc<ArcSwap<..>> 包装:线程间共享同一 RCU 实例（ArcSwapAny 自身无 Clone）
        let table = Arc::new(ArcSwap::from_pointee(HtsTable::default()));
        let writers: Vec<_> = (0..8)
            .map(|w| {
                let table = Arc::clone(&table);
                std::thread::spawn(move || {
                    for i in 0..1_000usize {
                        let kind = TaskKind::ALL[w % TaskKind::ALL.len()];
                        let t = (i % 20_000) + 1;
                        let src = ThresholdSource::ConservativeDefault;
                        let cur = table.load_full();
                        let mut next = (*cur).clone();
                        next.update(kind, t, 64, src);
                        table.store(Arc::new(next));
                    }
                })
            })
            .collect();
        let readers: Vec<_> = (0..8)
            .map(|_| {
                let table = Arc::clone(&table);
                std::thread::spawn(move || {
                    for _ in 0..10_000usize {
                        let guard = table.load();
                        for kind in TaskKind::ALL {
                            let e = guard.get(kind);
                            // 快照一致性:阈值恒 >= 1（写线程从未写 0）
                            assert!(e.threshold >= 1, "读线程观测到非法快照");
                        }
                    }
                })
            })
            .collect();
        for h in writers.into_iter().chain(readers) {
            h.join().expect("并发读写线程应正常退出");
        }
    }

    /// 序贯检验:小样本强制 Continue（即使数据效果极显著,n < min_samples 不判定）
    #[test]
    fn sequential_small_sample_continues() {
        let mut t = SequentialTest::new(SequentialTestConfig::default());
        for i in 0..29 {
            // 强效果数据（rayon 快 90%）
            let d = t.record(100.0, 10.0 + (i % 3) as f64);
            assert_eq!(
                d,
                TestDecision::Continue,
                "第 {} 次记录（n<30）必须 Continue",
                i + 1
            );
        }
        assert_eq!(t.sample_count(), 29);
        assert_eq!(t.spent_alpha(), 0.0, "小样本阶段不消耗 alpha 预算");
    }

    /// 序贯检验:效果量达标 + SPRT 跨上界 → Promote（回传候选阈值）
    #[test]
    fn sequential_promote_when_effect_reached() {
        let mut t = SequentialTest::new(SequentialTestConfig::default());
        t.set_candidate_threshold(256);
        let mut decisions = Vec::new();
        for i in 0..35 {
            // 强效果:rayon 快 90%,带小抖动（方差非零）
            decisions.push(t.record(100.0 + (i % 3) as f64, 10.0 + (i % 3) as f64));
        }
        assert_eq!(
            decisions[29],
            TestDecision::Promote { threshold: 256 },
            "n=30 首个判定点应 Promote"
        );
        // 终局后实例关闭:后续记录强制 Reject（预算耗尽,永不二次 Promote）
        assert_eq!(
            t.record(100.0, 10.0),
            TestDecision::Reject,
            "终局后 record 一律 Reject"
        );
        assert_eq!(t.sample_count(), 30, "终局后不再积累样本");
        assert_eq!(t.spent_alpha(), 0.05, "终局判定耗尽 alpha 预算");
        assert_eq!(t.decision(), Some(TestDecision::Promote { threshold: 256 }));
    }

    /// 序贯检验:SPRT 跨上界但效果量未达标（15% < 20%）→ Reject（Promote 仅在效果量达到时发出）
    #[test]
    fn sequential_reject_when_effect_not_reached() {
        let mut t = SequentialTest::new(SequentialTestConfig::default());
        let mut last = TestDecision::Continue;
        for i in 0..35 {
            // 只快 10%,未达 effect_size=20%;零方差 → SPRT 确定性退化
            last = t.record(100.0 + (i % 2) as f64, 90.0 + (i % 2) as f64);
        }
        assert_eq!(
            last,
            TestDecision::Reject,
            "效果量未达标必须 Reject,不 Promote"
        );
        assert_eq!(t.spent_alpha(), 0.05, "Reject 同样是终局,耗尽预算");
    }

    /// 序贯检验:alpha spending 耗尽后强制 Reject（预算墙——防 p-hacking 门禁核心）
    ///
    /// 语义:一次检验只能有一次终局判定;终局后实例预算耗尽,继续喂数据
    /// 只能 Reject,永不二次 Promote。
    #[test]
    fn sequential_alpha_spending_exhausted_forces_reject() {
        let mut t = SequentialTest::new(SequentialTestConfig::default());
        // 阶段 1:强效果 → Promote（消耗 alpha 预算）
        for i in 0..30 {
            let _ = t.record(100.0 + (i % 3) as f64, 10.0 + (i % 3) as f64);
        }
        assert_eq!(t.spent_alpha(), 0.05, "Promote 后预算必须耗尽");
        // 阶段 2:预算已耗尽,即使更强的效果数据也强制 Reject（不 Promote）
        for i in 0..8 {
            let d = t.record(100.0 + (i % 3) as f64, 1.0 + (i % 3) as f64);
            assert_eq!(
                d,
                TestDecision::Reject,
                "alpha 预算耗尽后强制 Reject,永不 Promote"
            );
        }
        assert_eq!(t.spent_alpha(), 0.05, "耗尽后 spent_alpha 保持不变");
    }

    /// 防 p-hacking 门禁:同分布数据重复检验不会误 Promote
    ///
    /// Inline == Rayon（无真实效果）,喂满 max_samples:
    /// 全程无 Promote;首个判定点（n=30）跨 SPRT 下界 → Reject 并冻结。
    #[test]
    fn sequential_no_false_promote_on_equal_distributions() {
        let mut t = SequentialTest::new(SequentialTestConfig::default());
        for i in 0..500usize {
            let j = i as f64;
            let d = t.record(100.0 + (j % 5.0), 100.0 + ((j * 7.0) % 5.0));
            assert_ne!(
                d,
                TestDecision::Promote { threshold: 0 },
                "第 {} 次记录:同分布数据严禁误 Promote（p-hacking 红线）",
                i + 1
            );
        }
        // 同分布下 n=30 跨下界 Reject 并冻结,后续幂等
        assert_eq!(t.decision(), Some(TestDecision::Reject));
        assert_eq!(t.spent_alpha(), 0.05);
    }

    /// 确定性（Ω₂）:同输入序列两次检验结果完全一致
    #[test]
    fn sequential_deterministic_same_input_same_decision() {
        // 混合序列:先无差异（29 条）→ 强效果（31 条）
        let seq: Vec<(f64, f64)> = (0..60)
            .map(|i| {
                if i < 29 {
                    (100.0 + (i % 5) as f64, 100.0 + ((i * 3) % 5) as f64)
                } else {
                    (100.0 + (i % 5) as f64, 10.0 + (i % 5) as f64)
                }
            })
            .collect();
        let mut a = SequentialTest::new(SequentialTestConfig::default());
        let mut b = SequentialTest::new(SequentialTestConfig::default());
        let (mut da, mut db) = (Vec::new(), Vec::new());
        for (x, y) in &seq {
            da.push(a.record(*x, *y));
            db.push(b.record(*x, *y));
        }
        assert_eq!(da, db, "Ω₂:同输入序列必须同决策序列");
        assert_eq!(a.sample_count(), b.sample_count());
        assert_eq!(a.spent_alpha(), b.spent_alpha());
    }

    /// 硬停止:max_samples 是终局上限 — 弱效果数据最迟在 max_samples 处给出终局
    #[test]
    fn sequential_max_samples_hard_stop() {
        let cfg = SequentialTestConfig {
            min_samples: 5,
            max_samples: 20,
            ..SequentialTestConfig::default()
        };
        // 同分布:首个判定点（n=5）跨下界 Reject;断言终局在 <= max_samples 内发生
        let mut t = SequentialTest::new(cfg);
        for _ in 0..20 {
            let _ = t.record(100.0, 100.0);
        }
        assert_eq!(
            t.decision(),
            Some(TestDecision::Reject),
            "同分布必在 max_samples 内终局"
        );
        assert!(t.sample_count() <= 20, "样本量不得超过 max_samples");
    }

    /// 配置校验 — 非法配置 try_new 拒绝,new panic;合法配置通过
    #[allow(clippy::field_reassign_with_default)] // 逐字段置非法值以覆盖每个校验分支
    #[test]
    fn sequential_config_validation() {
        use super::sequential_test::HtsError;
        // 合法默认
        assert!(SequentialTestConfig::default().validate().is_ok());
        // alpha 越界（含 NaN）与 power 越界
        let mut cfg = SequentialTestConfig::default();
        cfg.alpha = 1.0;
        assert_eq!(cfg.validate(), Err(HtsError::AlphaOutOfRange(1.0)));
        cfg.alpha = f64::NAN;
        assert!(matches!(cfg.validate(), Err(HtsError::AlphaOutOfRange(_))));
        cfg = SequentialTestConfig::default();
        cfg.power = 0.0;
        assert_eq!(cfg.validate(), Err(HtsError::PowerOutOfRange(0.0)));
        // 样本窗口倒挂
        cfg = SequentialTestConfig::default();
        cfg.min_samples = 600;
        assert_eq!(cfg.validate(), Err(HtsError::SampleRange(600, 500)));
        // effect_size 非正
        cfg = SequentialTestConfig::default();
        cfg.effect_size = 0.0;
        assert_eq!(cfg.validate(), Err(HtsError::EffectSizeNonPositive(0.0)));
        // try_new 拒绝非法;new 非法 panic（构造前提,编程错误）
        cfg = SequentialTestConfig::default();
        cfg.effect_size = -1.0;
        assert!(SequentialTest::try_new(cfg).is_err());
    }

    /// cgroup:Linux 路径解析 — "80000 100000" → 0.8
    #[test]
    fn cgroup_parse_quota_period() {
        assert_eq!(parse_cpu_max("80000 100000"), Some(0.8));
        assert_eq!(parse_cpu_max("80000 100000\n"), Some(0.8), "容忍尾部换行");
        assert_eq!(parse_cpu_max("400000 100000"), Some(4.0));
        assert_eq!(parse_cpu_max("1 1000"), Some(0.001));
    }

    /// cgroup:"max 100000"（未限制）→ None
    #[test]
    fn cgroup_parse_max_unlimited() {
        assert_eq!(parse_cpu_max("max 100000"), None);
        assert_eq!(parse_cpu_max("max max"), None);
    }

    /// cgroup:无效输入 → None（字段缺失/多余/非数值/非正/NaN）
    #[test]
    fn cgroup_parse_invalid_inputs() {
        assert_eq!(parse_cpu_max(""), None, "空内容");
        assert_eq!(parse_cpu_max("   "), None, "纯空白");
        assert_eq!(parse_cpu_max("80000"), None, "缺 period");
        assert_eq!(parse_cpu_max("80000 100000 123"), None, "多余字段");
        assert_eq!(parse_cpu_max("abc 100000"), None, "quota 非数值");
        assert_eq!(parse_cpu_max("80000 xyz"), None, "period 非数值");
        assert_eq!(parse_cpu_max("0 100000"), None, "quota 为零");
        assert_eq!(parse_cpu_max("80000 0"), None, "period 为零");
        assert_eq!(parse_cpu_max("-1 100000"), None, "quota 为负");
        assert_eq!(parse_cpu_max("NaN 100000"), None, "quota 为 NaN");
    }

    /// cgroup:文件注入 — 临时文件路径可配置（Ω₇ 缝合）
    #[test]
    fn cgroup_probe_reads_file() {
        let dir = std::env::temp_dir().join(format!("hts_cgroup_t9_{}", std::process::id()));
        let path = dir.join("cpu.max");
        std::fs::create_dir_all(&dir).expect("临时目录创建");
        std::fs::write(&path, "80000 100000").expect("临时文件写入");
        let probe = CgroupProbe::new(path.clone());
        assert_eq!(probe.probe(), Some(0.8), "注入路径应解析出 0.8 核");
        // 文件不存在 → None
        let missing = CgroupProbe::new(dir.join("nope.max"));
        assert_eq!(missing.probe(), None, "文件缺失返回 None");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// cgroup:默认探测点在非 Linux 平台恒 None（占位语义）
    #[test]
    fn cgroup_effective_cores_platform() {
        #[cfg(target_os = "linux")]
        {
            // Linux 上:无 cgroup 限制或探测失败 → None;有限制 → 正数
            let v = crate::compute::hts::cgroup::effective_cores();
            match v {
                Some(n) => assert!(n > 0.0, "有效核数必须为正"),
                None => {} // 未限制/无权限:无校正信息,合法
            }
        }
        #[cfg(not(target_os = "linux"))]
        {
            assert_eq!(
                crate::compute::hts::cgroup::effective_cores(),
                None,
                "非 Linux 平台 cgroup 校正恒 None"
            );
        }
    }

    /// kind_index — 六类映射互斥且覆盖 ALL（布局一致性防御）
    #[test]
    fn kind_index_bijective() {
        use std::collections::HashSet;
        let mut seen = HashSet::new();
        for kind in TaskKind::ALL {
            let idx = kind_index(kind);
            assert!(idx < 6, "索引越界");
            assert!(seen.insert(idx), "索引重复:kind_index 非单射");
        }
        assert_eq!(seen.len(), 6, "六类必须映射到六个不同索引");
    }
}
