//! MoE(Mixture of Experts)稀疏门控 — 大规模模型注册表的粗筛层
//!
//! 对应架构层:L1 Core,作为 `route_auto` 策略的前置优化层
//! 对应定律:Ω-Sparse(全维稀疏 — 仅激活 Top-K 专家,而非全量评估)
//!
//! # 设计动机
//! 50 以上模型规模下,`route_auto` 的全量评估(两遍遍历求 max_cost/max_latency、
//! 逐模型归一化评分、生成 n-1 个 candidates 的 String clone)成为
//! O(n) 高常数因子瓶颈。MoE 门控先用**轻量级评分函数**粗筛 Top-K(K ≤ 5)
//! 候选,再仅对 Top-K 做完整评估,将高常数因子操作从 O(n) 降至 O(k)。
//!
//! # 门控评分(轻量级)
//! - **无需全局 max 归一化**(避免两遍遍历),支持单遍 O(n) 评分
//! - 用倒数形式 `1/(1+x)`,值域 (0,1],方向与完整评分一致(越小越好 → 分越高)
//! - 纯算术,无字符串 `format!`,常数因子远低于完整评估
//!
//! ## v1.2.0 三维评分(cost / latency / quality)
//! 权重 0.4/0.4/0.2,与 `route_auto` 完整评分一致,保证粗筛排序近似。
//!
//! ## v1.3.0 五维评分扩展(加入运行时统计维度)
//! 历史数据充足时(≥ 100 条记录),扩展为五维评分:
//! - `cost`(0.3):成本倒数(与 v1.2.0 一致,权重从 0.4 降至 0.3)
//! - `latency`(0.3):延迟倒数(与 v1.2.0 一致,权重从 0.4 降至 0.3)
//! - `quality`(0.2):质量评分(与 v1.2.0 一致)
//! - `success_rate`(0.1):历史成功率,值域 [0,1],直接作为分数
//! - `latency_variance`(0.1):延迟稳定性倒数 `1/(1+variance)`,惩罚抖动模型
//!
//! WHY 五维权重 0.3/0.3/0.2/0.1/0.1:cost/latency 仍是主导因素(各 0.3,
//! 合计 0.6),quality 补充(0.2),历史维度仅占 0.2(success_rate 0.1 +
//! variance 0.1)作为排名微调,避免历史噪声主导决策。前三维权重合计 0.8
//! (v1.2.0 为 1.0),历史维度占 0.2,总权重 1.0。
//!
//! # 降级路径(向后兼容)
//! - **模型数 < threshold(默认 50)**:门控返回全部模型,退化为全量评估
//! - **history=None 或历史数据不足(< 100 条)**:降级三维评分,权重重新
//!   归一化为 0.375/0.375/0.25(保持 3:3:2 比例,等比放大 1.25x,总权重 1.0)
//!
//! WHY 降级归一化:历史数据不足时 success_rate/variance 估计不稳定(统计
//! 显著性不足),降级三维避免噪声主导排名。归一化保持 3:3:2 比例不变,
//! Top-K 选择结果与 v1.2.0 一致(仅绝对分数值缩放,排名不变)。
//!
//! WHY 降级阈值 100:统计显著性最小样本数。success_rate 在 < 100 样本时
//! 置信区间过宽(如 50 样本 → 95% CI ±0.14),variance 估计同样不稳定。
//! 100 样本下 95% CI 收窄至 ±0.10,可接受作为排名微调输入。

#![forbid(unsafe_code)]

use std::cmp::Ordering;
use std::collections::VecDeque;
use std::sync::RwLock;

// v1.4.0 P1:HistoryStore trait + InMemoryHistoryStore + 常量已迁移至 `history` 模块
// WHY `pub use` 而非 `use`:strategies.rs 等同 crate 模块通过 `crate::moe::HistoryStore`
// 路径引用 trait,需要 `pub use` 重导出保持路径可见性(向后兼容)。外部用户应优先
// 从 `model_router::history` 或 `model_router::` 顶层导入(lib.rs 已重导出)。
pub use crate::history::{
    HistoryStore, InMemoryHistoryStore, HISTORY_SUFFICIENT_THRESHOLD, LATENCY_WINDOW_CAPACITY,
};

use crate::types::ModelInfo;

/// 默认稀疏化触发阈值 — 模型数达到此值时启用 Top-K 门控
///
/// WHY 50:任务规格定义的验收门槛(50+ 模型规模)。低于此规模时
/// O(n) 与 O(k) 差异可忽略,退化路径避免引入 partition 开销与候选列表
/// 语义变化(退化路径候选含全部模型,稀疏路径仅含 Top-K)。
pub const DEFAULT_MOE_THRESHOLD: usize = 50;

/// 默认 Top-K 激活数 — 稀疏路径下完整评估的候选数量
///
/// WHY 5:覆盖典型路由的候选广度(主选 + 4 个降级候选),与 CACR
/// Downgrade 的"次优候选"语义衔接,同时将完整评估开销固定为常数。
pub const DEFAULT_MOE_TOP_K: usize = 5;

// `HISTORY_SUFFICIENT_THRESHOLD` / `LATENCY_WINDOW_CAPACITY` 已迁移至
// `crate::history` 模块(权威源),本文件通过 `use` 引用(见文件顶部)。

/// 单个模型的历史路由记录(运行时统计)
///
/// 用于五维门控评分的后两维:`success_rate` 与 `latency_variance`。
/// 由 `HistoryStore` 实现(ex: `InMemoryHistoryStore`)按 model_id 维护。
///
/// # 字段
/// - `success_count` / `total_count`:累计成功/总次数,success_rate = success/total
/// - `latency_samples`:最近 `LATENCY_WINDOW_CAPACITY`(100)次延迟样本(ms),
///   最新在尾部;VecDeque 滑动窗口自动淘汰最旧样本
///
/// WHY Clone:get() 返回 owned HistoryRecord(避免 DashMap Ref guard 跨 await/
/// 作用域问题)。克隆成本 ~400B/次,在路由热路径(单次决策)上可忽略。
///
/// WHY 手动 Clone 而非 derive(Clone):`RwLock` 不实现 `Clone`(锁是同步原语,
/// 非数据容器),需手动 clone 内部缓存值 `RwLock::new(cached.read().unwrap().clone())`。
/// 每个 clone 获得独立的 RwLock + 相同的缓存值,行为正确:clone 的 `record()`
/// 仅失效 clone 自身缓存,不影响 stored record 的缓存。
#[derive(Debug)]
pub struct HistoryRecord {
    /// 成功路由次数(累计,不随窗口滑动)
    pub success_count: u64,
    /// 总路由次数(累计,不随窗口滑动)
    pub total_count: u64,
    /// 最近 `LATENCY_WINDOW_CAPACITY` 次延迟样本(ms),尾部为最新
    pub latency_samples: VecDeque<f32>,
    /// `latency_variance` 缓存:None = 需重算,Some(v) = 缓存命中
    ///
    /// WHY RwLock 而非 AtomicF32:stable Rust 无 AtomicF32(§4.1 规则),
    /// 并发浮点缓存需 RwLock<Option<f32>> 提供读写隔离。
    /// WHY Option<f32>:None 表示缓存失效(记录已变更),Some(v) 表示缓存有效。
    /// WHY 字段在 HistoryRecord 而非 InMemoryHistoryStore:每模型独立缓存,
    /// DashMap 内的 stored record 持有缓存,跨 `get()` clone 持久化。
    /// WHY pub(crate):sqlite.rs 需在反序列化时构造 HistoryRecord,
    /// 外部 crate 不应直接访问缓存(通过 latency_variance() 方法读写)。
    pub(crate) cached_variance: RwLock<Option<f32>>,
}

impl HistoryRecord {
    /// 创建空记录(零样本)
    pub fn new() -> Self {
        Self {
            success_count: 0,
            total_count: 0,
            latency_samples: VecDeque::with_capacity(LATENCY_WINDOW_CAPACITY),
            cached_variance: RwLock::new(None),
        }
    }

    /// 记录一次路由结果(latency_ms + success)
    ///
    /// - total_count 始终递增(累计统计,用于 success_rate)
    /// - latency_samples 维持滑动窗口:满则淘汰最旧(popleft),再 push 新样本
    /// - success_count 仅在 success=true 时递增
    /// - cached_variance 失效:样本变更后缓存值不再有效,设为 None 懒重算
    pub fn record(&mut self, latency_ms: f32, success: bool) {
        self.total_count += 1;
        if success {
            self.success_count += 1;
        }
        // 滑动窗口:超容量时淘汰最旧样本,保持窗口大小恒定
        if self.latency_samples.len() >= LATENCY_WINDOW_CAPACITY {
            self.latency_samples.pop_front();
        }
        self.latency_samples.push_back(latency_ms);
        // WHY get_mut() 而非 write():record() 取 &mut self,已有独占访问,
        // 无需 RwLock 写锁(get_mut 利用 &mut self 保证无竞争,零开销)
        *self.cached_variance.get_mut().unwrap() = None;
    }

    /// 历史成功率 ∈ [0,1],无样本时返回 0.0
    pub fn success_rate(&self) -> f32 {
        if self.total_count == 0 {
            0.0
        } else {
            self.success_count as f32 / self.total_count as f32
        }
    }

    /// 延迟方差(样本方差,无偏估计)
    ///
    /// 公式:`sum((x - mean)^2) / (n - 1)`(n >= 2),n < 2 时返回 0.0
    /// WHY 无偏估计(n-1 而非 n):样本方差修正 Bessel 校正,避免小样本低估。
    /// 返回值单位为 ms²,值域 [0, +∞),方差越大 → 延迟越不稳定 → 惩罚越重。
    ///
    /// # 缓存(v1.5.0-omega 优化)
    /// 首次调用计算方差并写入 `cached_variance`,后续调用直接读缓存(O(1))。
    /// `record()` 调用时缓存失效(设为 None),下次调用懒重算。
    ///
    /// WHY 双重检查锁定(double-checked locking):
    /// 1. 先 read lock 查缓存(fast path,不阻塞并发读)
    /// 2. miss 时释放 read lock → write lock → 再次检查(防竞争)→ 计算 → 写入
    ///
    /// 读锁在写锁前释放,避免死锁(RwLock 不支持 read→write 升级)。
    pub fn latency_variance(&self) -> f32 {
        // Fast path:读缓存(read lock,不阻塞并发 reader)
        {
            let cache = self.cached_variance.read().unwrap();
            if let Some(v) = *cache {
                return v;
            }
        } // read lock 在此释放,避免 read→write 死锁

        // Slow path:cache miss → write lock → double-check → 计算 → 写入
        let mut cache = self.cached_variance.write().unwrap();
        // Double-check:等待 write lock 期间可能已有其他线程完成计算
        if let Some(v) = *cache {
            return v;
        }

        let n = self.latency_samples.len();
        let variance = if n < 2 {
            0.0
        } else {
            let mean: f32 = self.latency_samples.iter().sum::<f32>() / n as f32;
            let sum_sq: f32 = self
                .latency_samples
                .iter()
                .map(|x| {
                    let diff = x - mean;
                    diff * diff
                })
                .sum();
            sum_sq / (n - 1) as f32
        };

        *cache = Some(variance);
        variance
    }

    /// 历史数据是否充分(>= HISTORY_SUFFICIENT_THRESHOLD 100 条)
    ///
    /// 充分时启用五维评分,不足时降级三维(向后兼容)。
    /// WHY 用 total_count 而非 latency_samples.len():total_count 是累计
    /// 统计(不随窗口滑动),反映"该模型是否被路由过足够多次"。
    /// latency_samples 受窗口限制(max 100),无法区分"刚好 100"与"1000+次"。
    pub fn is_sufficient(&self) -> bool {
        self.total_count >= HISTORY_SUFFICIENT_THRESHOLD
    }
}

impl Clone for HistoryRecord {
    /// 手动 Clone:`RwLock` 不实现 `Clone`,需读取内部值并构造新 `RwLock`。
    /// clone 继承 stored record 的缓存值(若有),避免 clone 首次 `latency_variance()`
    /// 时重算。`get()` 返回的 clone 可直接命中 inherited 缓存。
    fn clone(&self) -> Self {
        Self {
            success_count: self.success_count,
            total_count: self.total_count,
            latency_samples: self.latency_samples.clone(),
            cached_variance: RwLock::new(*self.cached_variance.read().unwrap()),
        }
    }
}

impl Default for HistoryRecord {
    fn default() -> Self {
        Self::new()
    }
}

// `HistoryStore` trait 与 `InMemoryHistoryStore` 已迁移至 `crate::history` 模块:
// - `crate::history::HistoryStore`(trait 定义,权威源)
// - `crate::history::InMemoryHistoryStore`(DashMap 实现,v1.3.0 行为不变)
// - `crate::history::SqliteHistoryStore`(v1.4.0 P1 新增,SQLite 持久化)
// 本文件通过文件顶部的 `use crate::history::{HistoryStore, InMemoryHistoryStore, ...}`
// 引用保持内部兼容(MoeGate::gate 签名不变)。

/// MoE 稀疏门控 — 控制 `route_auto` 的大规模稀疏化行为
///
/// 不可变值类型,`Copy` 语义便于在路由热路径上零开销传递。
/// 既是配置载体(threshold/top_k),也是门控执行者(`gate()` 方法)。
///
/// `RouterConfig` 用独立标量字段(`moe_threshold`/`moe_top_k`)而非内嵌
/// `MoeGate` 序列化,保持与 `cacr` 字段一致的渐进式 serde default 设计;
/// 构造 `MoeGate` 时从 `RouterConfig` 字段取值传入 `MoeGate::new`。
///
/// # 使用示例
/// ```
/// use model_router::{MoeGate, ModelRegistry, RouterConfig, RoutingRequest, RoutingStrategy};
/// use nexus_core::{UserIntent, MultimodalInput};
///
/// let registry = ModelRegistry::from_config(&RouterConfig::default());
/// let gate = MoeGate::default();
/// let req = RoutingRequest {
///     quest_id: "q".into(),
///     intent: UserIntent {
///         intent_id: "i".into(),
///         raw_text: "hi".into(),
///         multimodal_inputs: vec![MultimodalInput::Text("hi".into())],
///         risk_level: 10,
///     },
///     estimated_tokens: 1000,
///     strategy: RoutingStrategy::Auto,
/// };
/// let models = registry.list();
/// // history=None:退化三维评分(向后兼容 v1.2.0)
/// let candidates = gate.gate(&models, None);
/// // 3 模型 < 50 阈值,退化为全量(返回全部引用)
/// assert_eq!(candidates.len(), models.len());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoeGate {
    /// 稀疏化触发阈值:模型数 >= threshold 时启用 Top-K 稀疏门控
    pub threshold: usize,
    /// Top-K 激活数:稀疏路径下仅完整评估 top_k 个候选
    pub top_k: usize,
}

impl Default for MoeGate {
    fn default() -> Self {
        Self {
            threshold: DEFAULT_MOE_THRESHOLD,
            top_k: DEFAULT_MOE_TOP_K,
        }
    }
}

impl MoeGate {
    /// 创建门控,指定阈值与 Top-K
    ///
    /// WHY top_k 至少为 1:Top-0 无意义,且会导致候选列表为空。
    /// threshold 不做强约束(should_sparsify 用 >= 判定,由调用方负责语义)。
    pub fn new(threshold: usize, top_k: usize) -> Self {
        Self {
            threshold,
            top_k: top_k.max(1),
        }
    }

    /// 模型数 >= threshold 时启用稀疏门控
    pub fn should_sparsify(&self, n: usize) -> bool {
        n >= self.threshold
    }

    /// 有效 Top-K — 不超过模型数,避免 select_nth 越界
    pub fn effective_k(&self, n: usize) -> usize {
        self.top_k.min(n)
    }

    /// 门控评分(轻量级)— 无需全局 max 归一化,每模型独立计算
    ///
    /// WHY 倒数形式 `1/(1+x)`:
    /// - 值域 (0,1],方向与完整评分 `1 - x/max` 一致(越小越好 → 分越高)
    /// - 无需预计算全局 max,支持单遍 O(n) 评分 + Top-K 选取
    /// - 纯算术,无字符串操作,常数因子远低于完整评估(含归一化遍历 + format!)
    ///
    /// # 五维评分(v1.3.0)— 历史数据充足时(≥ 100 条)
    /// `0.3*cost + 0.3*latency + 0.2*quality + 0.1*success_rate + 0.1*variance_gate`
    /// - cost/latency/quality 权重从 v1.2.0 的 0.4/0.4/0.2 降至 0.3/0.3/0.2,
    ///   腾出 0.2 给历史维度(success_rate 0.1 + variance 0.1)
    /// - success_rate ∈ [0,1] 直接作为分数(无需归一化)
    /// - variance_gate = `1/(1+variance)`:方差越大 → gate 越小 → 惩罚越重
    ///
    /// # 三维降级 — 历史数据不足时(history=None 或 < 100 条)
    /// `0.375*cost + 0.375*latency + 0.25*quality`
    /// - 权重从 0.3/0.3/0.2 等比放大 1.25x → 0.375/0.375/0.25(总权重 1.0)
    /// - 保持 3:3:2 比例不变,Top-K 排名与 v1.2.0 一致(仅绝对值缩放)
    ///
    /// `quality_score` 为 f32,显式 `as f64` 转换参与 f64 运算(此处为算术
    /// 运算而非比较,不触发 §4.4 #6 的 f32→f64 比较精度问题)。
    ///
    /// # v1.5.0-omega 缓存优化
    /// `variance` 由调用方(`gate()`)通过 `HistoryStore::latency_variance()`
    /// 预计算并传入,而非在 `gate_score` 内部调 `h.latency_variance()`。
    /// 这允许 `InMemoryHistoryStore` 在 stored record 上计算(缓存持久),
    /// 而非在 `get()` 返回的 clone 上计算(缓存随 drop 丢失)。
    fn gate_score(m: &ModelInfo, history: Option<(&HistoryRecord, f32)>) -> f64 {
        // 成本倒数:cost_per_1k_tokens 单位为美元,量级 0.0001~0.05,
        // 乘以 1000 放大到 0.1~50 区间,使倒数有合理区分度
        let cost_gate = 1.0 / (1.0 + m.cost_per_1k_tokens * 1000.0);
        // 延迟倒数:avg_latency_ms 单位为毫秒,量级 50~1000,
        // 除以 100 归一到 0.5~10 区间,使倒数有合理区分度
        let latency_gate = 1.0 / (1.0 + m.avg_latency_ms as f64 / 100.0);
        let quality = m.quality_score as f64;

        match history {
            // 五维:历史数据充足(调用方已确保 is_sufficient),启用完整五维评分
            // variance 由调用方预计算(来自 HistoryStore::latency_variance 缓存)
            Some((h, variance)) => {
                let success_rate = h.success_rate() as f64;
                let variance_gate = 1.0 / (1.0 + variance as f64);
                0.3 * cost_gate
                    + 0.3 * latency_gate
                    + 0.2 * quality
                    + 0.1 * success_rate
                    + 0.1 * variance_gate
            }
            // 三维降级:历史不足或 None,权重重新归一化(0.3/0.3/0.2 → 0.375/0.375/0.25)
            // WHY 0.375 = 0.3/0.8、0.25 = 0.2/0.8:前三维权重合计 0.8,等比放大至 1.0
            None => 0.375 * cost_gate + 0.375 * latency_gate + 0.25 * quality,
        }
    }

    /// 门控评分降序比较器:评分高者优先,相同则 model_id 升序(保证确定性)
    ///
    /// WHY 命名为独立 fn(item 天生 Copy):可同时传递给
    /// `select_nth_unstable_by` 和 `sort_by`,避免闭包 move 后无法复用。
    fn cmp_gate_score_desc(a: &(f64, &ModelInfo), b: &(f64, &ModelInfo)) -> Ordering {
        b.0.partial_cmp(&a.0)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.1.model_id.cmp(&b.1.model_id))
    }

    /// 执行门控:返回 Top-K 候选引用(评分降序),或退化时返回全部
    ///
    /// # 复杂度
    /// - 退化(模型数 < threshold):O(n) 收集引用,不评分
    /// - 门控:O(n) 评分(含历史查询)+ O(n) `select_nth_unstable_by` + O(k log k) 排序 = O(n)
    ///
    /// # 返回
    /// - 门控模式:Top-K 候选引用(按门控评分降序)
    /// - 退化模式:全部模型引用(原顺序,交给 `route_auto` 全量评估排序)
    ///
    /// # 历史维度(history 参数)
    /// - `Some(store)`:每模型查询历史,充分(≥ 100 条)则五维评分,不足则三维降级
    /// - `None`:全部模型三维降级(向后兼容 v1.2.0)
    /// - 混合模式:同一 gate() 调用中,有历史的模型用五维,无历史的用三维
    ///
    /// WHY 退化模式不评分不排序:保持与历史全量评估行为完全一致(由调用方排序),
    /// 避免引入额外的排序顺序差异;退化时历史也无需查询(全量评估用 `route_auto`
    /// 完整评分,不经过 gate_score)。
    pub fn gate<'a>(
        &self,
        models: &'a [ModelInfo],
        history: Option<&dyn HistoryStore>,
    ) -> Vec<&'a ModelInfo> {
        if !self.should_sparsify(models.len()) {
            // WHY 退化:模型数低于阈值,全量评估的绝对耗时在微秒级,
            // 门控评分开销不划算;返回全部引用交给 route_auto 全量评估。
            return models.iter().collect();
        }

        // 防御 top_k > models.len() 的边界(effective_k clamp)
        let k = self.effective_k(models.len());

        // 轻量级门控评分:O(n),每模型查询历史(若有)决定五维 vs 三维
        // WHY 逐模型查询:不同模型历史充足性可能不同(新模型无历史→三维,
        // 老模型有历史→五维),混合模式正确反映"已知模型用历史,未知用静态"。
        //
        // WHY 通过 store 级 latency_variance() 而非 record.latency_variance():
        // store 级方法操作 DashMap 内的 stored record,缓存跨 get() clone 持久化,
        // 显著降低 gate() 热路径中重复 variance 计算的延迟(v1.5.0-omega 优化)。
        let mut scored: Vec<(f64, &ModelInfo)> = models
            .iter()
            .map(|m| {
                let hist = history.and_then(|h| {
                    let record = h.get(&m.model_id)?;
                    if !record.is_sufficient() {
                        return None;
                    }
                    // 在 stored record 上计算/读缓存(缓存持久,跨 gate() 调用)
                    let variance = h.latency_variance(&m.model_id)?;
                    Some((record, variance))
                });
                (Self::gate_score(m, hist.as_ref().map(|(r, v)| (r, *v))), m)
            })
            .collect();

        // Top-K 选取:O(n) select_nth_unstable_by
        // WHY select_nth_unstable_by:O(n) 部分排序,符合 §4.1 Engineering Convention,
        // 禁止 sort_by(O(n log n))做 Top-K。partition 后 [0..k] 为 Top-K(无序)。
        // 传入 k-1 使 [0..k] 恰好为 Top-K(含 k 个元素)。
        if k < scored.len() {
            scored.select_nth_unstable_by(k - 1, Self::cmp_gate_score_desc);
        }

        // 截取 Top-K 并按评分降序排序(便于完整评估优先处理高分候选)
        scored.truncate(k);
        scored.sort_by(Self::cmp_gate_score_desc);

        scored.into_iter().map(|(_, m)| m).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_model(id: &str, cost: f64, latency: u64, quality: f32) -> ModelInfo {
        ModelInfo {
            model_id: id.into(),
            provider: "test".into(),
            cost_per_1k_tokens: cost,
            avg_latency_ms: latency,
            max_context: 8192,
            quality_score: quality,
        }
    }

    #[test]
    fn test_default_constants() {
        let gate = MoeGate::default();
        assert_eq!(gate.threshold, DEFAULT_MOE_THRESHOLD);
        assert_eq!(gate.top_k, DEFAULT_MOE_TOP_K);
        assert_eq!(gate.threshold, 50);
        assert_eq!(gate.top_k, 5);
    }

    #[test]
    fn test_new_clamps_top_k_to_one() {
        // WHY top_k=0 无意义,强制为 1 保护候选列表非空语义
        let gate = MoeGate::new(50, 0);
        assert_eq!(gate.top_k, 1);
    }

    #[test]
    fn test_new_preserves_threshold() {
        let gate = MoeGate::new(100, 3);
        assert_eq!(gate.threshold, 100);
        assert_eq!(gate.top_k, 3);
    }

    #[test]
    fn test_should_sparsify_boundary() {
        let gate = MoeGate::new(50, 5);
        assert!(!gate.should_sparsify(49), "49 < 50 不应稀疏化");
        assert!(gate.should_sparsify(50), "50 >= 50 应稀疏化");
        assert!(gate.should_sparsify(200));
    }

    #[test]
    fn test_effective_k_capped_by_n() {
        let gate = MoeGate::new(50, 5);
        assert_eq!(gate.effective_k(200), 5);
        assert_eq!(gate.effective_k(3), 3, "n < top_k 时取 n");
        assert_eq!(gate.effective_k(5), 5);
    }

    #[test]
    fn test_gate_score_low_cost_high_score() {
        // 低成本应得高分(倒数形式)— history=None 退化三维
        let cheap = make_model("cheap", 0.0001, 100, 0.6);
        let expensive = make_model("expensive", 0.05, 100, 0.6);
        assert!(MoeGate::gate_score(&cheap, None) > MoeGate::gate_score(&expensive, None));
    }

    #[test]
    fn test_gate_score_low_latency_high_score() {
        let fast = make_model("fast", 0.001, 50, 0.6);
        let slow = make_model("slow", 0.001, 1000, 0.6);
        assert!(MoeGate::gate_score(&fast, None) > MoeGate::gate_score(&slow, None));
    }

    #[test]
    fn test_gate_score_high_quality_high_score() {
        let good = make_model("good", 0.001, 100, 0.95);
        let bad = make_model("bad", 0.001, 100, 0.5);
        assert!(MoeGate::gate_score(&good, None) > MoeGate::gate_score(&bad, None));
    }

    #[test]
    fn test_gate_degrades_below_threshold() {
        // 3 模型 < 50 阈值,退化为全量
        let models = vec![
            make_model("m1", 0.001, 100, 0.8),
            make_model("m2", 0.002, 200, 0.7),
            make_model("m3", 0.003, 300, 0.6),
        ];
        let gate = MoeGate::default();
        let result = gate.gate(&models, None);
        assert_eq!(result.len(), 3, "退化模式应返回全部模型");
    }

    #[test]
    fn test_gate_activates_top_k_above_threshold() {
        // 50 模型 >= 50 阈值,激活 Top-5
        let models: Vec<ModelInfo> = (0..50)
            .map(|i| {
                make_model(
                    &format!("m{i}"),
                    0.001 + i as f64 * 0.0001,
                    100 + i * 10,
                    (0.5 + i as f32 * 0.01).min(1.0),
                )
            })
            .collect();
        let gate = MoeGate::default();
        let result = gate.gate(&models, None);
        assert_eq!(result.len(), 5, "门控模式应返回 Top-5");
    }

    #[test]
    fn test_gate_custom_top_k() {
        // 自定义 top_k=3
        let models: Vec<ModelInfo> = (0..60)
            .map(|i| {
                make_model(
                    &format!("m{i}"),
                    0.001 + i as f64 * 0.0001,
                    100 + i * 10,
                    0.5,
                )
            })
            .collect();
        let gate = MoeGate::new(50, 3);
        let result = gate.gate(&models, None);
        assert_eq!(result.len(), 3, "自定义 top_k=3 应返回 3 个候选");
    }

    #[test]
    fn test_gate_returns_sorted_descending() {
        // 门控结果应按评分降序
        let models: Vec<ModelInfo> = (0..55)
            .map(|i| {
                make_model(
                    &format!("m{i:02}"),
                    0.001 + i as f64 * 0.0001,
                    100 + i * 10,
                    0.5,
                )
            })
            .collect();
        let gate = MoeGate::default();
        let result = gate.gate(&models, None);
        // 验证降序:每个候选的 gate_score 应 >= 下一个
        for i in 0..result.len() - 1 {
            let score_curr = MoeGate::gate_score(result[i], None);
            let score_next = MoeGate::gate_score(result[i + 1], None);
            assert!(
                score_curr >= score_next,
                "候选应降序排列: [{}] score {:.6} < [{}] score {:.6}",
                i,
                score_curr,
                i + 1,
                score_next
            );
        }
    }

    #[test]
    fn test_gate_top_k_clamped_when_models_fewer_than_k() {
        // threshold=1(极低阈值)但只有 3 模型,top_k=5 应 clamp 到 3
        let models = vec![
            make_model("m1", 0.001, 100, 0.8),
            make_model("m2", 0.002, 200, 0.7),
            make_model("m3", 0.003, 300, 0.6),
        ];
        let gate = MoeGate::new(1, 5);
        let result = gate.gate(&models, None);
        assert_eq!(
            result.len(),
            3,
            "top_k > models.len() 时应 clamp 到 models.len()"
        );
    }

    #[test]
    fn test_gate_includes_best_model_in_top_k() {
        // 门控 Top-K 应包含真正评分最高的模型(召回验证)
        let best = make_model("best", 0.0001, 50, 0.99);
        let mut models: Vec<ModelInfo> = (0..60)
            .map(|i| {
                make_model(
                    &format!("m{i}"),
                    0.01 + i as f64 * 0.0001,
                    200 + i * 10,
                    0.5,
                )
            })
            .collect();
        models.push(best);
        let gate = MoeGate::default();
        let result = gate.gate(&models, None);
        let ids: Vec<&str> = result.iter().map(|m| m.model_id.as_str()).collect();
        assert!(ids.contains(&"best"), "Top-K 应包含最优模型, got {:?}", ids);
    }
}
