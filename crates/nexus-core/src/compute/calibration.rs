//! 阈值校准闭环 — SPRT 序贯检验接入 `update_threshold`（ADR-198 Phase B1）
//!
//! 对应架构层:L1 Core
//!
//! # 为什么需要本模块
//! `HtsTable` 的三重来源里 SPRT 实物早已存在（[`sequential_test`]）,但唯一写入口
//! [`update_threshold`](super::bridge::ComputeBridge::update_threshold) 的生产调用者为 0
//! ⇒ 运行期阈值表永远等于 S9 预填初值,"动态阈值"名不副实。本模块补的就是这条边。
//!
//! # 成对样本从哪来（关键约束,勿再走回头路）
//! SPRT 要求同一 kind 的 `(inline_us, rayon_us)` **成对**样本,而卸载入口收到的是
//! `FnOnce` 闭包 —— **同一个闭包不能跑两遍**,所以"同一调用同时测两条路径"不可能。
//! 唯一诚实的来源是**探测式分流**:对被采样的少数调用,按计数器奇偶决定这次走池还是
//! 走 Inline,两条路径各自攒样本,凑够 `min_samples` 后按序号配对喂给 SPRT。
//! 结果值两条路径逐元素相同（各点位的既有断测保证,ADR-196 D3 的同式契约）,
//! 但**执行路径不同 = 延迟特征不同 = 行为变更**,因此:
//!
//! | `CHIMERA_COMPUTE_CALIBRATION` | 效果 |
//! |---|---|
//! | `off`（**默认**） | 不探测、不采样、不写表 —— 零额外开销、零行为变更 |
//! | `observe` | 探测 + 采样 + 判定,**不写表**（只出证据） |
//! | `apply` | 探测 + 采样 + 判定,且 Promote 时写表（带 `promoted_at`/`samples`） |
//!
//! 非法值回落 `off`（最保守档),与 ADR-197 D2 同一 fail-safe 取向。
//!
//! # 判定与写表规则（不改统计量,只接线）
//! - 统计判定**全部委托** [`SequentialTest`]（Wald SPRT + `min_samples` 小样本墙 +
//!   `max_samples` 硬停 + `effect_size` 效果量门 + 终局一次性 alpha 预算墙）;
//!   本模块不新造统计量、不改 [`SequentialTestConfig`] 常数。
//! - 样本量纲统一为 **µs/项**（吞吐）,否则不同 `n_items` 的两桶直接配对会把
//!   "任务本身更大"误判成"池更慢"。
//! - 候选阈值 = **当前阈值折半**,下限 [`MIN_PROMOTED_THRESHOLD`]:一次只走一小步,
//!   每一步都要重新攒够自己的样本与 alpha 预算;`Reject` **不写表也不反向调整**
//!   （上调需要成本模型证据,本批不伪造）。

use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use super::bridge::ComputeBridge;
use super::dispatch::TaskKind;
use super::hts::sequential_test::{SequentialTest, SequentialTestConfig, TestDecision};
use super::hts::{kind_index, ThresholdSource};

/// 采样周期：每 64 次卸载取 1 次做校准（把 `Instant` 与探测成本摊薄到 <2%）
pub const SAMPLE_EVERY: u64 = 64;

/// 折半下调的下限 —— 低于此值并行固定成本必吃掉收益,不许再降
const MIN_PROMOTED_THRESHOLD: usize = 16;

/// 校准模式（与 `CHIMERA_COMPUTE_CALIBRATION` 字符串一一对应）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalibrationMode {
    /// 关闭:不探测、不采样、不写表（默认）
    Off,
    /// 观察:探测 + 采样 + 判定,不写表
    Observe,
    /// 应用:在观察基础上,Promote 时写表
    Apply,
}

/// 实际执行路径 —— 样本按此分桶
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecPath {
    /// 走 L-a rayon 计算池
    Pool,
    /// 在调用线程 Inline 执行（含饱和降级与校准探测两种来源）
    Inline,
}

/// 解析 env —— 纯函数,便于表驱动单测
///
/// 非法/空值回落 [`CalibrationMode::Off`]:探测会改执行路径,未显式授权即不做。
#[must_use]
pub fn parse_calibration_env(raw: Option<&str>) -> CalibrationMode {
    let Some(text) = raw
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_ascii_lowercase())
    else {
        return CalibrationMode::Off;
    };
    match text.as_str() {
        "off" | "0" | "" => CalibrationMode::Off,
        "observe" | "watch" => CalibrationMode::Observe,
        "apply" | "on" | "1" => CalibrationMode::Apply,
        // 拼错/未知值不开探测:fail-safe 优先于"看起来在工作"
        _ => CalibrationMode::Off,
    }
}

/// 生效模式（进程生命周期内读 env 一次）
#[must_use]
pub fn mode() -> CalibrationMode {
    static CACHED: OnceLock<CalibrationMode> = OnceLock::new();
    *CACHED.get_or_init(|| {
        parse_calibration_env(std::env::var("CHIMERA_COMPUTE_CALIBRATION").ok().as_deref())
    })
}

/// 采样判定 —— 每 [`SAMPLE_EVERY`] 次取一次
///
/// `counter` 从 0 起,故第 0、64、128… 次被采样（起点确定,便于测试）。
#[must_use]
pub fn should_sample(counter: u64) -> bool {
    counter.is_multiple_of(SAMPLE_EVERY)
}

/// 探测分流:被采样的调用里,奇偶交替决定本次走池还是走 Inline
///
/// WHY 交替而非随机:两桶样本量必须大致相等才能配对;随机源会引入偏斜,
/// 且破坏可复现性（确定性奇偶便于单测直接推导期望）。
///
/// 本函数只对 [`should_sample`] 为真的 counter 有语义；其余 counter 的返回值
/// 无意义，调用方不得在采样窗口外使用。
#[must_use]
pub fn explores_inline(counter: u64) -> bool {
    // 第一个被采样的调用（counter=0）走 Inline 基线,随后交替
    (counter / SAMPLE_EVERY).is_multiple_of(2)
}

/// 单个 kind 的双桶吞吐样本（µs/项）
#[derive(Debug, Default)]
struct Buckets {
    inline: Vec<f64>,
    pool: Vec<f64>,
}

impl Buckets {
    /// 记录一条吞吐样本（µs/项）
    ///
    /// `n_items == 0` 无意义,直接丢弃（调用方已保证 n>0,此处只防未来误用）。
    fn push(&mut self, path: ExecPath, n_items: usize, elapsed: Duration) {
        if n_items == 0 {
            return;
        }
        let per_item_us = elapsed.as_nanos() as f64 / 1_000.0 / n_items as f64;
        // 上限先算再可变借 self（否则与下面的 &mut 分桶冲突）
        let cap = self.cap();
        let bucket = match path {
            ExecPath::Inline => &mut self.inline,
            ExecPath::Pool => &mut self.pool,
        };
        // 硬停上限：到量即停采,防止长生命周期进程无限吃内存与样本
        if bucket.len() < cap {
            bucket.push(per_item_us);
        }
    }

    fn cap(&self) -> usize {
        SequentialTestConfig::default().max_samples
    }
}

/// 每 kind 的校准槽状态（编码进 u8 以便原子读取快照）
#[derive(Debug, Default)]
struct Slot {
    buckets: Mutex<Buckets>,
    /// 终局判定:`0` = 未判定,`1` = Promote,`2` = Reject
    decision: AtomicU8,
    /// Promote 候选阈值（decision=1 时有效）
    candidate: AtomicU64,
    /// 判定时双桶各自的样本数（写表时作为 `samples` 证据）
    samples: AtomicU64,
    /// 是否已把结论写表（防同一结论重复落表）
    applied: AtomicU8,
}

/// 校准器 —— 定长槽数组,索引复用 [`kind_index`]（TaskKind 扩容时自动跟随）
#[derive(Debug, Default)]
pub struct Calibrator {
    slots: Vec<Slot>,
}

impl Calibrator {
    #[must_use]
    fn new_with_kinds() -> Self {
        Self {
            slots: (0..TaskKind::ALL.len()).map(|_| Slot::default()).collect(),
        }
    }

    /// 记录一条路径样本,并在两桶各够 `min_samples` 时做一次终局判定
    pub fn record(&self, kind: TaskKind, path: ExecPath, n_items: usize, elapsed: Duration) {
        let idx = kind_index(kind);
        let Some(slot) = self.slots.get(idx) else {
            return;
        };
        {
            // 仓库惯例:任务内全部 catch_unwind,锁不会被 poison;即 poison 也取回内部值
            // （而非静默返回 —— 那会把真 bug 变成“样本莫名消失”）
            let mut buckets = slot
                .buckets
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            buckets.push(path, n_items, elapsed);
        }
        // 已有终局判定则不再复检（尊重 SPRT 的一次性 alpha 预算墙）
        if slot.decision.load(Ordering::Relaxed) != 0 {
            return;
        }
        let (inline_n, pool_n, inline, pool) = {
            let buckets = slot
                .buckets
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            (
                buckets.inline.len(),
                buckets.pool.len(),
                buckets.inline.clone(),
                buckets.pool.clone(),
            )
        };
        let config = SequentialTestConfig::default();
        if inline_n < config.min_samples || pool_n < config.min_samples {
            return;
        }
        let current = {
            let b = super::bridge::bridge();
            b.threshold_of(kind)
        };
        // 候选:当前阈值折半,不低于下限 —— 一次只走一小步
        let candidate = (current / 2).max(MIN_PROMOTED_THRESHOLD);
        let Ok(mut test) = SequentialTest::try_new(config) else {
            // 默认配置本应合法;不可达 defensively 保守返回（宁不判定也不误写表）
            return;
        };
        test.set_candidate_threshold(candidate);
        // 按序号配对喂入（两桶等长部分才可比）
        let pairs = inline_n.min(pool_n);
        let mut decision = TestDecision::Continue;
        for i in 0..pairs {
            decision = test.record(inline[i], pool[i]);
            if decision != TestDecision::Continue {
                break;
            }
        }
        match decision {
            TestDecision::Promote { threshold } => {
                slot.candidate
                    .store(threshold.max(candidate) as u64, Ordering::Relaxed);
                slot.samples.store(pairs as u64, Ordering::Relaxed);
                slot.decision.store(1, Ordering::Relaxed);
            }
            TestDecision::Reject => {
                slot.samples.store(pairs as u64, Ordering::Relaxed);
                slot.decision.store(2, Ordering::Relaxed);
            }
            TestDecision::Continue => {}
        }
    }

    /// 把已判定为 Promote 的结论写入阈值表（仅 `apply` 模式）
    ///
    /// 返回实际写表条数。`observe` 模式调用本函数**不写表**（只由调用方读快照),
    /// 这是"默认零行为变更"承诺的落地点。
    pub fn apply_pending(&self, bridge: &ComputeBridge, mode: CalibrationMode) -> usize {
        if mode != CalibrationMode::Apply {
            return 0;
        }
        let mut applied = 0;
        for (idx, slot) in self.slots.iter().enumerate() {
            if slot.decision.load(Ordering::Relaxed) != 1 {
                continue;
            }
            if slot.applied.swap(1, Ordering::Relaxed) == 1 {
                continue; // 同一结论只落一次表
            }
            let Some(&kind) = TaskKind::ALL.get(idx) else {
                continue;
            };
            let threshold = slot.candidate.load(Ordering::Relaxed) as usize;
            let samples = slot.samples.load(Ordering::Relaxed) as u32;
            // chunk 沿用表内现值:本批不主张 chunk 语义（该字段当前无消费者,ADR-196 D2-ter）
            let chunk = bridge.chunk_of(kind);
            bridge.update_threshold(
                kind,
                threshold,
                chunk,
                ThresholdSource::SequentialTest {
                    promoted_at: unix_now(),
                    samples,
                },
            );
            applied += 1;
        }
        applied
    }

    /// 导出校准快照（供周期上报与测试断言）
    #[must_use]
    pub fn snapshot(&self) -> Vec<CalibrationStat> {
        TaskKind::ALL
            .iter()
            .enumerate()
            .map(|(idx, kind)| {
                let slot = &self.slots[idx];
                let (inline_n, pool_n) = match slot.buckets.lock() {
                    Ok(b) => (b.inline.len(), b.pool.len()),
                    Err(poisoned) => {
                        let b = poisoned.into_inner();
                        (b.inline.len(), b.pool.len())
                    }
                };
                CalibrationStat {
                    kind: *kind,
                    inline_samples: inline_n as u64,
                    pool_samples: pool_n as u64,
                    decided: match slot.decision.load(Ordering::Relaxed) {
                        1 => Decided::Promote,
                        2 => Decided::Reject,
                        _ => Decided::Pending,
                    },
                    candidate_threshold: slot.candidate.load(Ordering::Relaxed) as usize,
                    applied: slot.applied.load(Ordering::Relaxed) == 1,
                }
            })
            .collect()
    }
}

/// 判定结果三态（快照导出用）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decided {
    /// 样本不足或检验进行中
    Pending,
    /// SPRT 通过 + 效果量达标
    Promote,
    /// SPRT 拒绝（含硬停止与终局后复检）
    Reject,
}

/// 单 kind 的校准状态快照
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CalibrationStat {
    /// 任务类型
    pub kind: TaskKind,
    /// Inline 桶样本数
    pub inline_samples: u64,
    /// 池桶样本数
    pub pool_samples: u64,
    /// 判定状态
    pub decided: Decided,
    /// Promote 候选阈值
    pub candidate_threshold: usize,
    /// 是否已写表
    pub applied: bool,
}

/// 进程级校准器（`off` 模式下不会被写入）
static CALIBRATOR: OnceLock<Calibrator> = OnceLock::new();

/// 取全局校准器
#[must_use]
pub fn calibrator() -> &'static Calibrator {
    CALIBRATOR.get_or_init(Calibrator::new_with_kinds)
}

/// 记录一条样本（便捷入口,带模式门禁）
pub fn observe(kind: TaskKind, path: ExecPath, n_items: usize, elapsed: Duration) {
    if mode() == CalibrationMode::Off {
        return;
    }
    calibrator().record(kind, path, n_items, elapsed);
}

/// 把 Promote 结论写表（仅 `apply` 模式生效）;返回写表条数
pub fn flush(bridge: &ComputeBridge) -> usize {
    calibrator().apply_pending(bridge, mode())
}

/// 快照（模式 + 每 kind 状态）,供上报器与测试
#[must_use]
pub fn snapshot() -> Vec<CalibrationStat> {
    calibrator().snapshot()
}

/// `SystemTime` → unix 秒（`ThresholdSource::SequentialTest.promoted_at` 用）
fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|e| e.duration())
        .as_secs()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // test-module unwrap is the Rust idiom; E-5 targets production code
mod tests {
    use super::*;

    #[test]
    fn parse_env_table() {
        assert_eq!(
            parse_calibration_env(None),
            CalibrationMode::Off,
            "默认必须关闭"
        );
        assert_eq!(parse_calibration_env(Some("")), CalibrationMode::Off);
        assert_eq!(parse_calibration_env(Some("  ")), CalibrationMode::Off);
        assert_eq!(parse_calibration_env(Some("OFF")), CalibrationMode::Off);
        assert_eq!(
            parse_calibration_env(Some("observe")),
            CalibrationMode::Observe
        );
        assert_eq!(
            parse_calibration_env(Some(" OBSERVE ")),
            CalibrationMode::Observe
        );
        assert_eq!(parse_calibration_env(Some("apply")), CalibrationMode::Apply);
        assert_eq!(parse_calibration_env(Some("on")), CalibrationMode::Apply);
        // 拼错不得悄悄开探测
        assert_eq!(parse_calibration_env(Some("aplyy")), CalibrationMode::Off);
        assert_eq!(parse_calibration_env(Some("true")), CalibrationMode::Off);
    }

    #[test]
    fn sampling_is_periodic_and_alternating() {
        assert!(should_sample(0), "起点必须可预测");
        assert!(!should_sample(1));
        assert!(!should_sample(SAMPLE_EVERY - 1));
        assert!(should_sample(SAMPLE_EVERY));
        assert!(should_sample(SAMPLE_EVERY * 7));
        // 采样点奇偶交替：0→Inline 基线, 64→Pool, 128→Inline …
        assert!(explores_inline(0));
        assert!(!explores_inline(SAMPLE_EVERY));
        assert!(explores_inline(SAMPLE_EVERY * 2));
        assert!(!explores_inline(SAMPLE_EVERY * 3));
    }

    #[test]
    fn zero_items_sample_is_dropped() {
        let c = Calibrator::new_with_kinds();
        c.record(
            TaskKind::Generic,
            ExecPath::Inline,
            0,
            Duration::from_micros(10),
        );
        let snap = &c.snapshot()[kind_index(TaskKind::Generic)];
        assert_eq!(snap.inline_samples, 0, "n=0 无吞吐定义,必须丢弃");
        assert_eq!(snap.decided, Decided::Pending);
    }

    #[test]
    fn pool_faster_than_inline_promotes_with_half_threshold_and_no_write_in_observe() {
        let c = Calibrator::new_with_kinds();
        let min = SequentialTestConfig::default().min_samples;
        // 喂「池吞吐 2µs/项 vs Inline 100µs/项」的稳定优势样本
        for _ in 0..min {
            c.record(
                TaskKind::KnnSearch,
                ExecPath::Inline,
                10,
                Duration::from_micros(1000),
            );
            c.record(
                TaskKind::KnnSearch,
                ExecPath::Pool,
                10,
                Duration::from_micros(20),
            );
        }
        let snap = &c.snapshot()[kind_index(TaskKind::KnnSearch)];
        assert!(
            snap.inline_samples >= min as u64 && snap.pool_samples >= min as u64,
            "两桶都要攒够 min_samples"
        );
        assert_eq!(snap.decided, Decided::Promote, "显著优势必须判定 Promote");
        let current = super::super::bridge::bridge().threshold_of(TaskKind::KnnSearch);
        assert!(
            snap.candidate_threshold > 0 && snap.candidate_threshold < current,
            "候选阈值必须是当前值折半(实际 {}/{current})",
            snap.candidate_threshold
        );
        assert!(!snap.applied, "刚判定完不应自动写表");
        // observe 模式下 flush 不得写表（零行为变更承诺的落地点）
        let written = c.apply_pending(super::super::bridge::bridge(), CalibrationMode::Observe);
        assert_eq!(written, 0, "observe 模式绝不写表");
        let written_off = c.apply_pending(super::super::bridge::bridge(), CalibrationMode::Off);
        assert_eq!(written_off, 0, "off 模式绝不写表");
    }

    #[test]
    fn inline_faster_than_pool_rejects_and_never_writes() {
        let c = Calibrator::new_with_kinds();
        let min = SequentialTestConfig::default().min_samples;
        for _ in 0..min {
            // Inline 2µs/项 快于 池 500µs/项 ⇒ 不该下调阈值
            c.record(
                TaskKind::OsaMask,
                ExecPath::Inline,
                8,
                Duration::from_micros(16),
            );
            c.record(
                TaskKind::OsaMask,
                ExecPath::Pool,
                8,
                Duration::from_micros(4000),
            );
        }
        let snap = &c.snapshot()[kind_index(TaskKind::OsaMask)];
        assert_eq!(snap.decided, Decided::Reject, "池不占优必须 Reject");
        assert_eq!(
            c.apply_pending(super::super::bridge::bridge(), CalibrationMode::Apply),
            0,
            "Reject 不写表,也不做反向调整"
        );
    }

    #[test]
    fn decision_is_one_shot_alpha_budget_wall() {
        let c = Calibrator::new_with_kinds();
        let min = SequentialTestConfig::default().min_samples;
        for _ in 0..min {
            c.record(
                TaskKind::Generic,
                ExecPath::Inline,
                4,
                Duration::from_micros(400),
            );
            c.record(
                TaskKind::Generic,
                ExecPath::Pool,
                4,
                Duration::from_micros(8),
            );
        }
        let first = c.snapshot()[kind_index(TaskKind::Generic)].decided;
        assert_eq!(first, Decided::Promote);
        // 继续喂"池变慢"的样本,不得翻案（终局后一次性预算墙）
        for _ in 0..min {
            c.record(
                TaskKind::Generic,
                ExecPath::Inline,
                4,
                Duration::from_micros(4),
            );
            c.record(
                TaskKind::Generic,
                ExecPath::Pool,
                4,
                Duration::from_micros(9000),
            );
        }
        let after = c.snapshot()[kind_index(TaskKind::Generic)].decided;
        assert_eq!(after, first, "同一 kind 只允许一次终局判定(防 p-hacking)");
    }

    #[test]
    fn apply_writes_table_once_and_is_idempotent() {
        let c = Calibrator::new_with_kinds();
        let min = SequentialTestConfig::default().min_samples;
        for _ in 0..min {
            c.record(
                TaskKind::ClvSimilarity,
                ExecPath::Inline,
                10,
                Duration::from_micros(2000),
            );
            c.record(
                TaskKind::ClvSimilarity,
                ExecPath::Pool,
                10,
                Duration::from_micros(40),
            );
        }
        assert_eq!(
            c.snapshot()[kind_index(TaskKind::ClvSimilarity)].decided,
            Decided::Promote
        );
        let b = ComputeBridge::test_bridge(2);
        let before = b.threshold_of(TaskKind::ClvSimilarity);
        let written = c.apply_pending(&b, CalibrationMode::Apply);
        assert_eq!(written, 1, "apply 模式应写一条");
        let after = b.threshold_of(TaskKind::ClvSimilarity);
        assert!(after < before, "写表后阈值应下调({before} → {after})");
        let src = b.threshold_source_of(TaskKind::ClvSimilarity);
        assert!(
            src.starts_with("SequentialTest"),
            "来源必须可审计(实际 {src})"
        );
        // 证据量必须与真实喂入数一致（防“已校准”话术无数据支撑）
        assert!(
            src.contains(&format!("samples:{min}")),
            "来源必须携带真实样本数证据(实际 {src},期望含 samples:{min})"
        );
        // 再 flush 一次不得重复写表
        assert_eq!(
            c.apply_pending(&b, CalibrationMode::Apply),
            0,
            "同一结论只落一次表"
        );
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// env 解析只可能返回三态之一,且非法输入必落 Off
        #[test]
        fn env_parse_is_safe(raw in proptest::option::of("[a-zA-Z0-9_ ]{0,10}")) {
            let m = parse_calibration_env(raw.as_deref());
            prop_assert!(matches!(m, CalibrationMode::Off | CalibrationMode::Observe | CalibrationMode::Apply));
            if let Some(text) = raw.as_deref() {
                let known = matches!(
                    text.trim().to_ascii_lowercase().as_str(),
                    "off" | "0" | "observe" | "watch" | "apply" | "on" | "1"
                );
                if !known {
                    prop_assert_eq!(m, CalibrationMode::Off, "未知值不得开启探测");
                }
            }
        }

        /// 采样与探测分流对任意 counter 都成立:每 64 次恰一次,且采样点严格交替
        #[test]
        fn sampling_invariants(counter in 0u64..1_000_000) {
            let sampled = should_sample(counter);
            prop_assert_eq!(sampled, counter.is_multiple_of(SAMPLE_EVERY));
            if sampled {
                let round = counter / SAMPLE_EVERY;
                prop_assert_eq!(explores_inline(counter), round.is_multiple_of(2));
            }
        }

        /// n_items=0 或耗时 0 的样本都不得污染桶计数(除零/负吞吐防线)
        #[test]
        fn degenerate_samples_are_harmless(n in 0usize..3, us in 0u64..1_000) {
            let c = Calibrator::new_with_kinds();
            c.record(TaskKind::Generic, ExecPath::Pool, n, Duration::from_micros(us));
            let snap = &c.snapshot()[kind_index(TaskKind::Generic)];
            if n == 0 {
                prop_assert_eq!(snap.pool_samples, 0);
            } else {
                prop_assert_eq!(snap.pool_samples, 1);
            }
            prop_assert_eq!(snap.decided, Decided::Pending);
        }
    }
}
