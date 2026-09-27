//! 任务优先级与动态调整机制 (§8 CHIMERA-MAS-Q / ADR-027 决策 4)
//!
//! 以 `event_bus::TaskPriority`(Low/Medium/High/Critical)为调度一等公民,
//! 叠加加权最短作业优先(WSJF)评分,支持动态重排、Critical 抢占 Low、
//! 以及 Low 任务饥饿保护(线性提权)。
//!
//! ## WSJF 评分公式 (§8.2)
//!
//! ```text
//! Priority Score = (W1·业务价值 + W2·时间敏感度 + W3·风险消减 + W4·依赖解锁度) / 任务规模
//! ```
//!
//! 各输入项 1~10 归一,`score_to_priority` 按阈值 T1/T2/T3 映射回 `TaskPriority`。
//!
//! ## 设计:纯函数 + 调度器分离
//!
//! WHY 拆分: `priority_rank` / `wsjf_score` / `score_to_priority` / `aged_priority_rank`
//! 均为**纯函数**(无副作用、确定性),可独立单元测试;`PriorityScheduler` 只负责
//! 存储与选择,复用纯函数, 避免时间/随机性渗入核心逻辑(回应可测性诉求)。

use crate::delegation::AgentTask;
use event_bus::TaskPriority;
use std::cmp::Reverse;
use std::collections::{BTreeSet, BinaryHeap, HashMap};
use std::time::{Duration, Instant};

/// WSJF 的全序包装 — `f64` 无 `Ord`，而桶内索引必须是 `BTreeSet` 的可比较键。
///
/// WHY `total_cmp` 而不手写位模式展开：`f64::total_cmp` 就是 IEEE754 totalOrder 的
/// 稳定实现，非 NaN 输入下与 `partial_cmp` 同序；NaN 排在 +∞ 之上是它的规范定义，
/// 而入口已把 NaN 归一（见 `enqueue`），故该位置在库内不可达。
#[derive(Debug, Clone, Copy, PartialEq)]
struct WsjfOrd(f64);

impl Eq for WsjfOrd {}

impl Ord for WsjfOrd {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.0.total_cmp(&other.0)
    }
}

impl PartialOrd for WsjfOrd {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// 桶内排序键：WSJF 降序 → `enqueued_at` 升序 → `seq` 升序。
///
/// 与 `best_index` 三维尾键**逐位同序**（后者保留为 debug 参考实现），故“取桶内最大
/// 键”就是“取同秩下最优条目”；`seq` 唯一，使 `BTreeSet` 不会因时间戳同刻而压键。
///
/// ## 方向表（首版两处都写反过，被 debug 对拍当场抓出，故列表存证）
///
/// `BTreeSet` 里「最优」= **最大键**，而派生 `Ord` 逐字段取大，于是：
///
/// | 字段 | 胜者 | 写法 |
/// |---|---|---|
/// | WSJF | 高者 | **原值**（`WsjfOrd` 取大就是分高）|
/// | `enqueued_at` | 早者 | `Reverse`（反转后“大”= 时间早）|
/// | `seq` | 小者 | `Reverse`（同上，同刻入队时定序）|
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct OrderKey {
    /// WSJF：最大键 = 最高分（直接放原值，不能套 `Reverse`）。
    wsjf: WsjfOrd,
    /// 入队时刻：`Reverse` 使最大键落在**最早**入队者。
    enqueued_asc: Reverse<Instant>,
    /// 入队序号：`Reverse` 使最大键落在 `seq` **最小**（先入队）者。
    seq_asc: Reverse<u64>,
}

/// 下一个秩提升到点：`enqueue` 起第 k 个饥饿间隔（k = 当前秩 − 基础秩 + 1）。
///
/// 返回 `None` 有三义，且三者都对应「无需再排事件」：秩已封顶 3、老化关闭（零阈值，
/// `aged_priority_rank` 同语义）、或时间运算溢出（阈值量级 ~10^11 年，此时
/// `waited / threshold` 在任何真实时钟下必为 0，不排事件的观测结果与全扫一致）。
///
/// WHY 纯函数：与 `aged_priority_rank` / `priority_rank` 同属调度纯函数层，可独立
/// 单测边界（封顶/零阈值/溢出），不沾调度器状态。
fn promotion_deadline(
    threshold: Duration,
    enqueued_at: Instant,
    base_rank: u8,
    rank: u8,
) -> Option<Instant> {
    if rank >= 3 || threshold.is_zero() {
        return None;
    }
    let steps = u32::from(rank.saturating_sub(base_rank)) + 1;
    let span = threshold.checked_mul(steps)?;
    enqueued_at.checked_add(span)
}

/// 优先级数值秩 — Critical=3 > High=2 > Medium=1 > Low=0。
///
/// WHY 引入秩: `event_bus::TaskPriority` 未派生 `Ord`(避免 L1 承载调度语义),
/// 调度层用秩函数定义偏序, 保持类型职责清晰。
pub fn priority_rank(priority: TaskPriority) -> u8 {
    match priority {
        TaskPriority::Low => 0,
        TaskPriority::Medium => 1,
        TaskPriority::High => 2,
        TaskPriority::Critical => 3,
    }
}

/// 由数值秩还原 `TaskPriority`(与 `priority_rank` 互逆, 超界钳制到 Critical)。
pub fn priority_from_rank(rank: u8) -> TaskPriority {
    match rank {
        0 => TaskPriority::Low,
        1 => TaskPriority::Medium,
        2 => TaskPriority::High,
        // WHY >=3 一律 Critical: 秩最高档, 老化提权封顶于此, 不溢出。
        _ => TaskPriority::Critical,
    }
}

/// WSJF 权重 (§8.2 的 W1..W4)。
///
/// 默认各项为 1.0(等权);可经 PDCA Act 阶段回流调整(§20.3)。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WsjfWeights {
    /// W1 — 业务价值权重
    pub w1: f64,
    /// W2 — 时间敏感度权重
    pub w2: f64,
    /// W3 — 风险消减权重
    pub w3: f64,
    /// W4 — 依赖解锁度权重
    pub w4: f64,
}

impl Default for WsjfWeights {
    /// 默认等权(各 1.0)。
    fn default() -> Self {
        Self {
            w1: 1.0,
            w2: 1.0,
            w3: 1.0,
            w4: 1.0,
        }
    }
}

impl WsjfWeights {
    /// 构造自定义权重。
    pub fn new(w1: f64, w2: f64, w3: f64, w4: f64) -> Self {
        Self { w1, w2, w3, w4 }
    }
}

/// WSJF 评分输入 (§8.2, 各项 1~10 归一; `job_size` 为除数)。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WsjfInput {
    /// 业务价值 — 对项目目标 KPI 的贡献(1~10)。
    pub business_value: f64,
    /// 时间敏感度 — 里程碑临近程度(1~10, 越近越高)。
    pub time_criticality: f64,
    /// 风险消减 — 完成后消除的风险等级(1~10)。
    pub risk_reduction: f64,
    /// 依赖解锁度 — 完成后可解锁的下游任务数(1~10)。
    pub dependency_unlock: f64,
    /// 任务规模 — 预估工作量 / estimated_tokens(除数, ≥1)。
    pub job_size: f64,
}

impl WsjfInput {
    /// 构造并归一: 四项输入钳制到 `[1.0, 10.0]`, `job_size` 钳制到 `≥ 1.0`。
    ///
    /// WHY 钳制: 保证 §8.2「各项 1~10 归一」约束, 且 `job_size ≥ 1` 杜绝除零。
    pub fn new(
        business_value: f64,
        time_criticality: f64,
        risk_reduction: f64,
        dependency_unlock: f64,
        job_size: f64,
    ) -> Self {
        Self {
            business_value: business_value.clamp(1.0, 10.0),
            time_criticality: time_criticality.clamp(1.0, 10.0),
            risk_reduction: risk_reduction.clamp(1.0, 10.0),
            dependency_unlock: dependency_unlock.clamp(1.0, 10.0),
            job_size: job_size.max(1.0),
        }
    }
}

/// 计算 WSJF 评分 (§8.2)。
///
/// `Score = (w1·bv + w2·tc + w3·rr + w4·du) / job_size`。
///
/// 全程 f64(§4.4 无 f32 隐式转换); `job_size` 经 `WsjfInput::new` 保证 `≥ 1`,
/// 此处再兜底 `max(1.0)` 防御直接字面量构造的除零。
pub fn wsjf_score(input: &WsjfInput, weights: &WsjfWeights) -> f64 {
    let numerator = weights.w1 * input.business_value
        + weights.w2 * input.time_criticality
        + weights.w3 * input.risk_reduction
        + weights.w4 * input.dependency_unlock;
    numerator / input.job_size.max(1.0)
}

/// WSJF 评分 → 优先级的阈值 (§8.2: Score≥T1→Critical; ≥T2→High; ≥T3→Medium; else Low)。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PriorityThresholds {
    /// T1 — 达此分记为 Critical。
    pub t1_critical: f64,
    /// T2 — 达此分记为 High。
    pub t2_high: f64,
    /// T3 — 达此分记为 Medium; 低于则 Low。
    pub t3_medium: f64,
}

impl Default for PriorityThresholds {
    /// 默认阈值: 等权 4 项(和域 4~40)/ job_size 后的合理分档。
    ///
    /// WHY 8.0/5.0/2.5: 等权下单项满分贡献约 1~10; 综合 Score 常见落在 2~10,
    /// 取 8/5/2.5 使 Critical/High/Medium/Low 分布均衡, 可经 PDCA 回流微调。
    fn default() -> Self {
        Self {
            t1_critical: 8.0,
            t2_high: 5.0,
            t3_medium: 2.5,
        }
    }
}

/// 由 WSJF 评分映射到 `TaskPriority` (§8.2 阈值映射)。
pub fn score_to_priority(score: f64, thresholds: &PriorityThresholds) -> TaskPriority {
    if score >= thresholds.t1_critical {
        TaskPriority::Critical
    } else if score >= thresholds.t2_high {
        TaskPriority::High
    } else if score >= thresholds.t3_medium {
        TaskPriority::Medium
    } else {
        TaskPriority::Low
    }
}

/// 饥饿老化后的有效优先级秩 (§8.4 饥饿保护)。
///
/// 线性提权: 等待时间每满一个 `threshold` 间隔, 秩 +1, 封顶 Critical(3)。
/// `threshold` 为零时视为关闭老化(直接返回基础秩), 避免除零 / 瞬间封顶。
///
/// ## 示例
/// - base=Low(0), waited=0 → 0
/// - base=Low(0), waited=1×threshold → 1(Medium)
/// - base=Low(0), waited=3×threshold → 3(Critical)
/// - base=Medium(1), waited=5×threshold → 3(封顶 Critical)
pub fn aged_priority_rank(base: TaskPriority, waited: Duration, threshold: Duration) -> u8 {
    let base_rank = priority_rank(base);
    if threshold.is_zero() {
        return base_rank;
    }
    // WHY 迭代比较而非 `waited.as_nanos() / threshold.as_nanos()`（L9 优化 2.3 续）：
    // Duration 除法走 u128 路径（单次十数纳秒量级），而出队是 O(n) 遍历——1000 项队列时
    // 这条除法就是热路主体（实测 53µs/次出队，超 50µs SLO 门）。秩封顶在 3，
    // 故至多判 (3 - base_rank) 次即可定结果，**与原除法语义等价**：
    // 原 = min(base + ⌊waited/threshold⌋, 3)；下 = 从 base 起每满一个间隔 +1，封顶 3。
    let mut rank = base_rank;
    let mut boundary = threshold;
    while rank < 3 && waited >= boundary {
        rank += 1;
        // saturating_add：累加不再增长时下一轮 `waited >= boundary` 必为假而退出，无死循环
        boundary = boundary.saturating_add(threshold);
    }
    rank
}

/// 判断 `incoming` 是否应抢占正在执行的 `running` (§8.4 抢占规则)。
///
/// 规则: **仅 Critical 可抢占正在执行的 Low**。其余组合不抢占。
///
/// WHY 严格限制: 抢占须先让被抢占 Agent 落 checkpoint(复用 quest-engine 语义),
/// 代价较高, 故仅对"Critical vs Low"这一最大优先级落差启用, 避免频繁抢占抖动。
pub fn should_preempt(running: TaskPriority, incoming: TaskPriority) -> bool {
    matches!(
        (running, incoming),
        (TaskPriority::Low, TaskPriority::Critical)
    )
}

/// 调度队列条目 — 任务 + 其 WSJF 评分 + 入队时刻(用于饥饿老化)+ 入队序号。
#[derive(Debug, Clone)]
struct ScheduleEntry {
    task: AgentTask,
    wsjf: f64,
    enqueued_at: Instant,
    /// 单调递增入队序号 — 三维键全平局时的确定性最终保障（L9 优化第二轮）。
    ///
    /// WHY:`Instant` 分辨率内同刻入队使 `enqueued_at` 平局,而 swap_remove
    /// 打乱物理顺序 → 平局胜者取决于布局(非确定)。seq 作第四键(小者=
    /// 先入队)保证完全确定的出队序,不依赖存储布局。
    seq: u64,
}

/// 优先级调度器 (§8) — 按 (有效优先级秩, WSJF) 出队, 支持动态重排与饥饿保护。
///
/// 采用「惰性最佳选择」: 出队时按当前有效秩(含饥饿老化)+ WSJF 选出最优条目,
/// 因此队列始终返回当下最应调度的任务。
///
/// ## 出队选优：按**有效秩**分桶 + 到点搬桶（L9 优化第三轮）
///
/// 出队序由三维键决定:(有效秩 = 基础秩 + 饥饿老化, WSJF 降序, enqueued_at 升序)。
/// 曾试过按**基础秩**分 4 桶 + 桶内 FIFO，失败结论被记作「三维键不可约简为分桶」：
/// 桶内按 WSJF 排就丢老化、按 FIFO 排就丢 WSJF，故当时认定「必须全扫」。
///
/// 那个结论只对**无状态全扫**成立：缺的不是分桶本身，而是把老化从「扫描时现算」
/// 变成「到点搬桶」的机制。本实现补上这一块：
///
/// 1. **按有效秩（而非基础秩）分桶** —— `buckets[0..4]` 桶内按 `OrderKey`（WSJF 降序
///    → enqueued_at 升序 → seq 升序）有序，于是「最高非空桶的最大键」就是全局最优：
///    跨桶由桶号定胜负（有效秩高者胜），同桶由键定胜负（WSJF 契约 + FIFO 稳定性）。
/// 2. **`promotions` 即时堆记录下一个秩提升到点** —— 一次 `enqueue` 至多排 3 个事件
///    （秩封顶 3）；每次出队先把已到点的事件搬桶（均摊 O(log n)），未到点的条目秩必
///    仍等于全扫公式的取值（因为边界恰好就是那些未触发的事件）。
/// 3. **正确性由 debug 对拍守卫** —— `best_index`（O(n) 全扫）作为参考实现全量保留，
///    debug/测试构建下每次出队逐位对拍加速路径，release 零成本（见 `best_slot`）。
///
/// 实测（release，`scheduler_dequeue` bench，**同轮**对照 `peek`=全扫参照）：
/// n=32 起桶路径即胜（0.269 vs 0.312µs），n=1000 / n=10000 分别 15.8× / 62×；
/// 唯一新增成本是 `swap_remove` 必需的 seq 索引修正（~0.1µs，与选优路径无关）。
/// 曾实现过“小队列全扫回退”分支，同轮实测反而更慢（单路径更省）且多一套不变量，
/// 已按架构减法原则删除——本层保持**单一选优路径**。
/// 判据口径（`/1000 < 50µs`）与语义均不变，仅从 «单 O(n)» 变为 «单 O(log n)»。
#[derive(Debug)]
pub struct PriorityScheduler {
    /// WSJF 权重
    weights: WsjfWeights,
    /// 评分 → 优先级阈值
    thresholds: PriorityThresholds,
    /// 饥饿提权阈值(等待每满一个间隔提一级; 零表示关闭)
    starvation_threshold: Duration,
    /// 待调度条目
    entries: Vec<ScheduleEntry>,
    /// 下一个入队序号 — 单调递增,为每次 enqueue 分配唯一 seq(饥饿平局确定性)。
    next_seq: u64,
    /// 按**有效秩**分桶的有序索引（0=最低…3=Critical）：桶内按 `OrderKey`
    /// （WSJF 降序 → enqueued_at 升序 → seq 升序）排列，与出队键尾部同序。
    buckets: [BTreeSet<OrderKey>; 4],
    /// `seq` → `entries` 下标；`swap_remove` 后仅需修补被换位的那一条。
    seq_index: HashMap<u64, usize>,
    /// 到点搬桶事件堆（`Reverse` 使其成为最早到点在堆顶）；可能含已失效条目
    /// （条目已出队/已重建索引），靠 `seq_index` 现场验活。
    promotions: BinaryHeap<Reverse<(Instant, u64)>>,
}

impl Default for PriorityScheduler {
    /// 默认调度器: 等权 WSJF + 默认阈值 + 饥饿阈值 5 分钟。
    fn default() -> Self {
        Self::with_config(
            WsjfWeights::default(),
            PriorityThresholds::default(),
            Duration::from_secs(300),
        )
    }
}

impl PriorityScheduler {
    /// 创建默认配置调度器。
    pub fn new() -> Self {
        Self::default()
    }

    /// 创建自定义配置调度器。
    ///
    /// ## 参数
    /// - `weights`: WSJF 权重
    /// - `thresholds`: 评分 → 优先级阈值
    /// - `starvation_threshold`: 饥饿提权间隔(零表示关闭老化)
    pub fn with_config(
        weights: WsjfWeights,
        thresholds: PriorityThresholds,
        starvation_threshold: Duration,
    ) -> Self {
        Self {
            weights,
            thresholds,
            starvation_threshold,
            entries: Vec::new(),
            next_seq: 0,
            buckets: Default::default(),
            seq_index: HashMap::new(),
            promotions: BinaryHeap::new(),
        }
    }

    /// 队列中待调度任务数。
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 队列是否为空。
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 入队 — 计算 WSJF 评分、记录入队时刻与单调序号。
    ///
    /// 任务自身的 `priority` 字段保留为主排序键;WSJF 作为同优先级内的次排序键;
    /// seq 为完全平局时的确定性终键(递增,先入队者 seq 更小)。
    pub fn enqueue(&mut self, task: AgentTask, wsjf_input: &WsjfInput) {
        let raw = wsjf_score(wsjf_input, &self.weights);
        // NaN 归一（单点声明）：全序桶索引无法表达「与一切相等」的 NaN 语义，而 NaN 只
        // 会来自调用方传入非有限输入（内置公式已用 max(1.0) 守住除零）——按最低键归一，
        // 使加速路径与 debug 参考实现见到的 `wsjf` 完全一致（否则两者会合法分歧）。
        let wsjf = if raw.is_nan() { 0.0 } else { raw };
        let seq = self.next_seq;
        self.next_seq += 1;
        self.entries.push(ScheduleEntry {
            task,
            wsjf,
            enqueued_at: Instant::now(),
            seq,
        });
        let idx = self.entries.len() - 1;
        // 刚入队时限 `waited ≈ 0 < threshold`，故有效秩就是基础秩（零阈值时同式）。
        let rank = priority_rank(self.entries[idx].task.priority);
        self.index_insert(idx, rank);
    }

    /// 出队 — 移除并返回当前最应调度的任务。
    ///
    /// 选择规则: 先比较有效优先级秩(含饥饿老化), 秩相同再比 WSJF(高者先),      
    /// 仍相同则取先入队者(稳定)。队列为空返回 `None`。
    ///
    /// WHY `swap_remove`(L9 优化 2.3): 选中条目与末尾交换后弹出, O(1) 替代
    /// `Vec::remove` 的 O(n) 搬移。搬乱只影响物理布局——出队序由秩桶键
    /// (`enqueued_at` + `seq`) 重新定义，稳定性不受影响。
    pub fn dequeue(&mut self) -> Option<AgentTask> {
        let now = Instant::now();
        let best = self.best_slot(now)?;
        let removed = self.entries.swap_remove(best);
        self.seq_index.remove(&removed.seq);
        // swap_remove 把原末条搬到了 best 位：只修它的下标，键与所在桶均不变。
        if let Some(moved_seq) = self.entries.get(best).map(|e| e.seq) {
            self.seq_index.insert(moved_seq, best);
        }
        Some(removed.task)
    }

    /// 查看(不移除)当前最应调度任务的有效优先级。
    ///
    /// WHY 这里保留 O(n) 全扫（而非用加速桶）：本方法契约是 `&self`，而秩提升是
    /// **状态变更**（搬桶需 `&mut`）；它服务于抢占判定类低频查询，不在出队热路基准内。
    /// 全扫本身就是参考实现，故此处绝无两套语义分歧风险。
    pub fn peek_effective_priority(&self) -> Option<TaskPriority> {
        let now = Instant::now();
        let best = self.best_index(now)?;
        let entry = &self.entries[best];
        let rank = aged_priority_rank(
            entry.task.priority,
            now.saturating_duration_since(entry.enqueued_at),
            self.starvation_threshold,
        );
        Some(priority_from_rank(rank))
    }

    /// 动态重排 — 依据 WSJF 评分重新映射每个任务的 `priority` (§8.4)。
    ///
    /// WHY 显式方法: 让 WSJF 在需要时(新任务入队 / 风险登记册更新 / 里程碑推进)
    /// 覆盖初始优先级, 实现"根据风险和依赖关系实时优化"; 出队时始终按最新优先级选择。
    pub fn recompute_from_wsjf(&mut self) {
        for entry in &mut self.entries {
            entry.task.priority = score_to_priority(entry.wsjf, &self.thresholds);
        }
        // 基础秩整体变 → 桶划分与提升事件全失效：按当前时刻整重建（批量操作，
        // 不在出队热路；重建后每个条目的秩与提升点又与全扫公式对齐）。
        self.rebuild_index(Instant::now());
    }

    /// 把一个条目登记进 `rank` 桶，并排定其下一秩提升到点。
    fn index_insert(&mut self, idx: usize, rank: u8) {
        let (wsjf, enqueued_at, seq, base) = {
            let e = &self.entries[idx];
            (e.wsjf, e.enqueued_at, e.seq, priority_rank(e.task.priority))
        };
        let key = OrderKey {
            wsjf: WsjfOrd(wsjf),
            enqueued_asc: Reverse(enqueued_at),
            seq_asc: Reverse(seq),
        };
        self.buckets[usize::from(rank)].insert(key);
        self.seq_index.insert(seq, idx);
        if let Some(deadline) =
            promotion_deadline(self.starvation_threshold, enqueued_at, base, rank)
        {
            self.promotions.push(Reverse((deadline, seq)));
        }
    }

    /// `entries[idx]` 的桶内键（与 `index_insert` 同源拼键，避免两处漂移）。
    fn key_of(&self, idx: usize) -> OrderKey {
        let e = &self.entries[idx];
        OrderKey {
            wsjf: WsjfOrd(e.wsjf),
            enqueued_asc: Reverse(e.enqueued_at),
            seq_asc: Reverse(e.seq),
        }
    }

    /// 把已到点的秩提升事件落到桶上（惰性：失效事件靠 `seq_index` 现场验活跳过）。
    ///
    /// 均摊成本：每个条目一生至多 3 次搬桶（秩封顶 3），每次 O(log n)。
    fn apply_due_promotions(&mut self, now: Instant) {
        while let Some(&Reverse((deadline, seq))) = self.promotions.peek() {
            if deadline > now {
                break;
            }
            self.promotions.pop();
            let Some(&idx) = self.seq_index.get(&seq) else {
                continue; // 条目已出队/已被重建替换 → 该事件失效
            };
            let key = self.key_of(idx);
            let (priority, enqueued_at) = {
                let e = &self.entries[idx];
                (e.task.priority, e.enqueued_at)
            };
            let new_rank = aged_priority_rank(
                priority,
                now.saturating_duration_since(enqueued_at),
                self.starvation_threshold,
            );
            for bucket in &mut self.buckets {
                if bucket.remove(&key) {
                    break;
                }
            }
            self.buckets[usize::from(new_rank)].insert(key);
            let base = priority_rank(priority);
            if let Some(next) =
                promotion_deadline(self.starvation_threshold, enqueued_at, base, new_rank)
            {
                self.promotions.push(Reverse((next, seq)));
            }
        }
    }

    /// 加速路径：先应用到点提升，再取最高非空桶的最大键，并将该键从桶中移除、返回下标。
    ///
    /// ## Panics
    /// 桶内键必在 `seq_index` 内（不变式：桶集合与活条目集合严格互为镜像，两者只在
    /// `index_insert` / 本方法的删除处变动）；违反即开发期缺陷，显式 `panic!` 而非静默
    /// 降级（静默 `None` 会伪装成「队列为空」）。
    fn best_slot(&mut self, now: Instant) -> Option<usize> {
        self.apply_due_promotions(now);
        for rank in (0..self.buckets.len()).rev() {
            let Some(&key) = self.buckets[rank].last() else {
                continue;
            };
            let Some(&idx) = self.seq_index.get(&key.seq_asc.0) else {
                panic!(
                    "调度器桶/序号表失同步（开发期缺陷）：seq={} 不在 seq_index",
                    key.seq_asc.0
                );
            };
            // debug/测试构建：与全扫参考实现逐位对拍（release 编译期略除，零成本）。
            // 对拍用同一个 `now`，故它同时校验三件事：秩桶划分、到点搬桶、同秩键序。
            #[cfg(debug_assertions)]
            {
                assert_eq!(
                    self.best_index(now),
                    Some(idx),
                    "加速出队与全扫参考实现分歧（秩桶/promotion/键序回归）：idx={idx}"
                );
            }
            self.buckets[rank].remove(&key);
            return Some(idx);
        }
        None
    }

    /// 按 `now` 重算每个条目的有效秩，整重建桶 / 序号表 / 提升堆。
    fn rebuild_index(&mut self, now: Instant) {
        for bucket in &mut self.buckets {
            bucket.clear();
        }
        self.seq_index.clear();
        self.promotions.clear();
        for idx in 0..self.entries.len() {
            let rank = Self::rank_of(&self.entries[idx], now, self.starvation_threshold);
            self.index_insert(idx, rank);
        }
    }

    /// 内部: 返回当前最优条目的下标(含饥饿老化), 空则 `None`。
    ///
    /// **本方法已不再是出队路径**：它是 O(n) 全扫的**参考实现**，保留的用途是在
    /// debug/测试构建下与加速路径（`best_slot`）逐位对拍（见结构体文档第 3 条），
    /// 以及 `peek_effective_priority` 这个 `&self` 低频查询。
    ///
    /// WHY 不删：两套独立实现的逐位一致，是本仓验证同类优化的既有范式（参见
    /// `test_aged_rank_matches_division_formula_at_boundaries` 保留旧公式作对拍）；
    /// 且 `peek_effective_priority`（`&self` 契约）仍靠它给出答案，故 release 下也在用。
    fn best_index(&self, now: Instant) -> Option<usize> {
        if self.entries.is_empty() {
            return None;
        }
        let mut best_idx = 0usize;
        // 热路常数项：一次取齐阈值，避免每条目回读 self 字段
        let starvation = self.starvation_threshold;
        let first = &self.entries[0];
        let mut best_rank = Self::rank_of(first, now, starvation);
        let mut best_wsjf = first.wsjf;
        let mut best_enqueued = first.enqueued_at;
        let mut best_seq = first.seq;
        for (idx, entry) in self.entries.iter().enumerate().skip(1) {
            let rank = Self::rank_of(entry, now, starvation);
            // 四维键: 秩高者优先 → WSJF 高者优先 → enqueued_at 早者优先 → seq 小者优先。
            // WHY 显式比 enqueued_at + seq: swap_remove 打乱物理顺序后,不能靠下标隔定
            // 先入队者;时间戳保 FIFO,seq 在 Instant 分辨率内同刻入队时提供完全确定性。
            // WHY partial_cmp 而非 `>`/`==`: 避免 clippy::float_cmp 对 f64 直接比较告警。
            let higher = match rank.cmp(&best_rank) {
                std::cmp::Ordering::Greater => true,
                std::cmp::Ordering::Less => false,
                std::cmp::Ordering::Equal => match entry
                    .wsjf
                    .partial_cmp(&best_wsjf)
                    .unwrap_or(std::cmp::Ordering::Equal)
                {
                    std::cmp::Ordering::Greater => true,
                    std::cmp::Ordering::Less => false,
                    // WSJF 平局 → 先入队者(enqueued_at 更早)胜出;enqueued_at 也平局时比 seq
                    std::cmp::Ordering::Equal => match entry.enqueued_at.cmp(&best_enqueued) {
                        std::cmp::Ordering::Less => true,
                        std::cmp::Ordering::Greater => false,
                        // 时间戳同刻(Instant 分辨率内)→ seq 小者(先入队)胜出,完全确定
                        std::cmp::Ordering::Equal => entry.seq < best_seq,
                    },
                },
            };
            if higher {
                best_idx = idx;
                best_rank = rank;
                best_wsjf = entry.wsjf;
                best_enqueued = entry.enqueued_at;
                best_seq = entry.seq;
            }
        }
        Some(best_idx)
    }

    /// 内部: 单条目的有效优先级秩(基础优先级 + 饥饿老化)。
    ///
    /// WHY 取条目引用而非 `idx`：`best_index` 已在迭代里持有 `entry`，
    /// 传引用可省掉每条目一次的 `self.entries[idx]` 边界检查——出队是 O(n)
    /// 扫描，省下的正是遍历的常数项（与除法改比较同批优化）。
    fn rank_of(entry: &ScheduleEntry, now: Instant, starvation_threshold: Duration) -> u8 {
        aged_priority_rank(
            entry.task.priority,
            now.saturating_duration_since(entry.enqueued_at),
            starvation_threshold,
        )
    }
}

// ============================================================
// 单元测试(纯函数为主, 调度器行为集成级见 tests/scheduler_test.rs)
// ============================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn test_priority_rank_ordering() {
        assert!(priority_rank(TaskPriority::Critical) > priority_rank(TaskPriority::High));
        assert!(priority_rank(TaskPriority::High) > priority_rank(TaskPriority::Medium));
        assert!(priority_rank(TaskPriority::Medium) > priority_rank(TaskPriority::Low));
    }

    #[test]
    fn test_rank_roundtrip() {
        for p in [
            TaskPriority::Low,
            TaskPriority::Medium,
            TaskPriority::High,
            TaskPriority::Critical,
        ] {
            assert_eq!(priority_from_rank(priority_rank(p)), p);
        }
    }

    #[test]
    fn test_wsjf_score_formula() {
        // 等权, 各项 = job_size = 1 → (1+1+1+1)/1 = 4
        let input = WsjfInput::new(1.0, 1.0, 1.0, 1.0, 1.0);
        assert!((wsjf_score(&input, &WsjfWeights::default()) - 4.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_wsjf_job_size_never_divides_by_zero() {
        // 直接字面量构造 job_size=0, wsjf_score 兜底 max(1.0)
        let input = WsjfInput {
            business_value: 10.0,
            time_criticality: 10.0,
            risk_reduction: 10.0,
            dependency_unlock: 10.0,
            job_size: 0.0,
        };
        let score = wsjf_score(&input, &WsjfWeights::default());
        assert!(score.is_finite());
        assert!((score - 40.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_score_to_priority_thresholds() {
        let t = PriorityThresholds::default();
        assert_eq!(score_to_priority(9.0, &t), TaskPriority::Critical);
        assert_eq!(score_to_priority(6.0, &t), TaskPriority::High);
        assert_eq!(score_to_priority(3.0, &t), TaskPriority::Medium);
        assert_eq!(score_to_priority(1.0, &t), TaskPriority::Low);
    }

    #[test]
    fn test_aged_rank_linear_promotion() {
        let th = Duration::from_secs(60);
        assert_eq!(aged_priority_rank(TaskPriority::Low, Duration::ZERO, th), 0);
        assert_eq!(
            aged_priority_rank(TaskPriority::Low, Duration::from_secs(60), th),
            1
        );
        assert_eq!(
            aged_priority_rank(TaskPriority::Low, Duration::from_secs(180), th),
            3
        );
        // 封顶 Critical(3)
        assert_eq!(
            aged_priority_rank(TaskPriority::Medium, Duration::from_secs(600), th),
            3
        );
    }

    #[test]
    fn test_aged_rank_zero_threshold_disables_aging() {
        assert_eq!(
            aged_priority_rank(TaskPriority::Low, Duration::from_secs(9999), Duration::ZERO),
            0
        );
    }

    /// 热路优化的**语义等价安全网**：`aged_priority_rank` 已从
    /// `min(base + ⌊waited/threshold⌋, 3)`（u128 除法）改为至多 3 次比较的循环，
    /// 本测在六类阈值 × 四档基础秩 × 间隔 0..6 的边界三点（恰等 k×th /
    /// 前 1ns / 后 1ns）上逐点比对**保留在此的旧公式参考实现**，
    /// 杜绝“快了但改了老化语义”（封顶/零阈值/极端大阈值均入矩阵）。
    #[test]
    fn test_aged_rank_matches_division_formula_at_boundaries() {
        let reference = |base: TaskPriority, waited: Duration, threshold: Duration| -> u8 {
            let base_rank = priority_rank(base);
            if threshold.is_zero() {
                return base_rank;
            }
            let intervals = (waited.as_nanos() / threshold.as_nanos().max(1)) as u64;
            (u64::from(base_rank) + intervals).min(3) as u8
        };
        let thresholds = [
            Duration::ZERO,
            Duration::from_nanos(1),
            Duration::from_nanos(60),
            Duration::from_millis(1),
            Duration::from_secs(1),
            Duration::from_secs(3600),
        ];
        let bases = [
            TaskPriority::Low,
            TaskPriority::Medium,
            TaskPriority::High,
            TaskPriority::Critical,
        ];
        let mut probes_done = 0usize;
        for th in thresholds {
            for base in bases {
                for k in 0u32..6 {
                    let probes: Vec<Duration> = if th.is_zero() {
                        // 零阈值下“间隔倍数”无定义，只校不老化分支与极端输入
                        vec![Duration::from_secs(u64::from(k)), Duration::MAX]
                    } else {
                        let exact = th * k;
                        vec![
                            exact,
                            exact.saturating_sub(Duration::from_nanos(1)),
                            exact.saturating_add(Duration::from_nanos(1)),
                        ]
                    };
                    for waited in probes {
                        probes_done += 1;
                        assert_eq!(
                            aged_priority_rank(base, waited, th),
                            reference(base, waited, th),
                            "老化语义漂移: base={base:?} th={th:?} waited={waited:?}"
                        );
                    }
                }
            }
        }
        // 夹具自身不得空跑：零命中的矩阵会让本测变成假绿
        assert!(probes_done > 300, "边界矩阵未充分展开: {probes_done}");
    }

    #[test]
    fn test_should_preempt_only_critical_over_low() {
        assert!(should_preempt(TaskPriority::Low, TaskPriority::Critical));
        assert!(!should_preempt(
            TaskPriority::Medium,
            TaskPriority::Critical
        ));
        assert!(!should_preempt(TaskPriority::Low, TaskPriority::High));
        assert!(!should_preempt(
            TaskPriority::Critical,
            TaskPriority::Critical
        ));
    }

    // ============================================================
    // L9 优化第三轮：秩桶 + 到点搬桶（加速出队）
    // ============================================================

    /// 构造指定优先级的 AgentTask（默认 Medium 复杂度）
    fn make_sched_task(id: &str, priority: TaskPriority) -> AgentTask {
        let task = nexus_core::Task {
            task_id: id.into(),
            description: format!("task {id}"),
            status: nexus_core::TaskStatus::Pending,
            dependencies: vec![],
        };
        AgentTask::new(
            task,
            crate::delegation::TaskComplexity::Medium,
            1000,
            Duration::from_secs(60),
            crate::delegation::QualityLevel::Standard,
        )
        .with_priority(priority)
    }

    /// 秩提升到点纯函数的四类边界：封顶 / 零阈值 / 步数算术后溢出面
    #[test]
    fn test_promotion_deadline_boundaries() {
        let now = Instant::now();
        let th = Duration::from_secs(100);
        // 秩已封顶 / 老化关闭 → 均不再排事件
        assert_eq!(promotion_deadline(th, now, 0, 3), None, "封顶 3 不应再排");
        assert_eq!(
            promotion_deadline(Duration::ZERO, now, 0, 0),
            None,
            "零阈值=关闭老化，不应排"
        );
        // 步数 = 当前秩 − 基础秩 + 1：从基础秩尚未提升过 → 首个到点就是 +1×th
        assert_eq!(
            promotion_deadline(th, now, 0, 0),
            Some(now + Duration::from_secs(100))
        );
        // base=Low(0) 且已在秩 2 → 下一个边界是第 3 个间隔
        assert_eq!(
            promotion_deadline(th, now, 0, 2),
            Some(now + Duration::from_secs(300))
        );
        // base=High(2) 且秩=2（尚未因老化提升）→ 下一个边界仍是 +1×th
        assert_eq!(
            promotion_deadline(th, now, 2, 2),
            Some(now + Duration::from_secs(100))
        );
        // 溢出面：Duration::MAX 即使 ×1 可行，Instant + MAX 必溢出 → 返回 None（不 panic）
        assert_eq!(promotion_deadline(Duration::MAX, now, 0, 0), None);
        assert_eq!(promotion_deadline(Duration::MAX, now, 0, 1), None);
    }

    /// 入队/出队的排序契约：秩优先 → 同秩 WSJF 降序 → 全平局 FIFO（按入队序）
    #[test]
    fn test_dequeue_contract_rank_then_wsjf_then_fifo() {
        let mut s = PriorityScheduler::new();
        let low_score = WsjfInput::new(1.0, 1.0, 1.0, 1.0, 10.0);
        let high_score = WsjfInput::new(10.0, 10.0, 10.0, 10.0, 1.0);
        s.enqueue(make_sched_task("low-hi", TaskPriority::Low), &high_score);
        s.enqueue(make_sched_task("med-lo", TaskPriority::Medium), &low_score);
        s.enqueue(
            make_sched_task("med-hi-1", TaskPriority::Medium),
            &high_score,
        );
        s.enqueue(
            make_sched_task("med-hi-2", TaskPriority::Medium),
            &high_score,
        );
        assert_eq!(s.dequeue().unwrap().inner.task_id, "med-hi-1");
        assert_eq!(s.dequeue().unwrap().inner.task_id, "med-hi-2");
        assert_eq!(s.dequeue().unwrap().inner.task_id, "med-lo");
        assert_eq!(s.dequeue().unwrap().inner.task_id, "low-hi");
        assert!(s.dequeue().is_none());
    }

    /// NaN 归一（入队单点）：全序桶无法表达「与一切相等」的 NaN 语义，故按最低键归一。
    /// 夹具先用同一纯函数断言输入确实产 NaN，避免在窗口里跑成假绿。
    #[test]
    fn test_nan_wsjf_normalized_to_lowest_key() {
        let nan_input = WsjfInput {
            business_value: f64::NAN,
            time_criticality: 1.0,
            risk_reduction: 1.0,
            dependency_unlock: 1.0,
            job_size: 1.0,
        };
        assert!(
            wsjf_score(&nan_input, &WsjfWeights::default()).is_nan(),
            "夹具前提：该输入应产出 NaN"
        );
        let mut s = PriorityScheduler::new();
        s.enqueue(make_sched_task("nan", TaskPriority::Medium), &nan_input);
        let finite = WsjfInput::new(1.0, 1.0, 1.0, 1.0, 1.0);
        s.enqueue(make_sched_task("finite", TaskPriority::Medium), &finite);
        // 同优先级下有限分应胜出（若 NaN 仍按「与一切相等」会退化为按 FIFO→先行入队的 nan 先出）
        assert_eq!(s.dequeue().unwrap().inner.task_id, "finite");
        assert_eq!(s.dequeue().unwrap().inner.task_id, "nan");
    }

    /// 老化提权在加速路径下仍生效：阈值 1ms + 实等待 5 个间隔 → Low 封顶 Critical，压过后来的 High
    #[test]
    fn test_aging_promotes_waiting_entry_under_bucket_path() {
        let mut s = PriorityScheduler::with_config(
            WsjfWeights::default(),
            PriorityThresholds::default(),
            Duration::from_millis(1),
        );
        let input = WsjfInput::new(1.0, 1.0, 1.0, 1.0, 1.0);
        s.enqueue(make_sched_task("low-old", TaskPriority::Low), &input);
        std::thread::sleep(Duration::from_millis(5));
        s.enqueue(make_sched_task("high-new", TaskPriority::High), &input);
        // 老条目已老化到 3 → 先出；且 debug 下每次出队都与全扫参考实现对拍（见 best_slot）
        assert_eq!(s.dequeue().unwrap().inner.task_id, "low-old");
        assert_eq!(s.dequeue().unwrap().inner.task_id, "high-new");
    }

    /// `recompute_from_wsjf` 整体重排后：桶/序号表/提升堆必须已重建，出队序按新优先级
    #[test]
    fn test_recompute_rebuilds_index_and_order_follows_new_priority() {
        let mut s = PriorityScheduler::new();
        let weak = WsjfInput::new(1.0, 1.0, 1.0, 1.0, 1.0); // 分低
        let strong = WsjfInput::new(10.0, 10.0, 10.0, 10.0, 1.0); // 分高
        s.enqueue(make_sched_task("crit-weak", TaskPriority::Critical), &weak);
        s.enqueue(make_sched_task("low-strong", TaskPriority::Low), &strong);
        assert_eq!(
            s.dequeue().unwrap().inner.task_id,
            "crit-weak",
            "重排前：优先级主导"
        );
        assert_eq!(
            s.dequeue().unwrap().inner.task_id,
            "low-strong",
            "清空队列（否则它会以同分先行入队者身份抢头位）"
        );
        // 只留强/弱各一条后重排：强分被提档，弱分被降档
        s.enqueue(make_sched_task("weak2", TaskPriority::Critical), &weak);
        s.enqueue(make_sched_task("strong2", TaskPriority::Low), &strong);
        s.recompute_from_wsjf();
        assert_eq!(s.len(), 2);
        // 强分高于弱分，且 score→priority 单调 ⇒ 强分先出（同档时 WSJF 也保它先出）
        assert_eq!(s.dequeue().unwrap().inner.task_id, "strong2");
        assert_eq!(s.dequeue().unwrap().inner.task_id, "weak2");
        assert!(s.is_empty());
        // 不变式：桶集合与活条目集合严格互为镜像 → 空队列 ⇒ 桶与序号表都必空
        assert!(s.seq_index.is_empty());
        assert!(
            s.buckets.iter().all(BTreeSet::is_empty),
            "空队列不得残留任何桶键"
        );
    }

    /// 跨档连续作业后索引仍严格成镜像：先小规模多轮出入队，再撑大、再排空。
    ///
    /// WHY 单列此测：它锁的正是“桶集合 == 活条目集合”这条单一不变式在**长时间混合
    /// 负载**下的成立性（含 recompute 整重建、超时搬桶、连续 swap_remove 改正）。
    #[test]
    fn test_index_stays_exact_mirror_under_long_mixed_load() {
        let mut s = PriorityScheduler::with_config(
            WsjfWeights::default(),
            PriorityThresholds::default(),
            Duration::from_millis(1),
        );
        let input = WsjfInput::new(1.0, 1.0, 1.0, 1.0, 1.0);
        // 阶段 1：小规模多轮出入队（每轮 3 入 1 出，跨老化边界）
        for i in 0..8 {
            s.enqueue(
                make_sched_task(&format!("small{i}"), TaskPriority::Medium),
                &input,
            );
            if i % 3 == 2 {
                std::thread::sleep(Duration::from_millis(2));
                assert!(s.dequeue().is_some(), "非空队列应能出队");
            }
        }
        // 阶段 2：撑大后连续排空（跨多次超时搬桶）
        for i in 0..40 {
            s.enqueue(
                make_sched_task(&format!("big{i}"), TaskPriority::Low),
                &input,
            );
        }
        std::thread::sleep(Duration::from_millis(3));
        let mut drained = 0usize;
        while s.dequeue().is_some() {
            drained += 1;
        }
        let expected = 8 + 40 - 2; // 阶段 1 入 8、出 2（i∈{2,5}）；阶段 2 入 40；共余 46
        assert_eq!(drained, expected, "出入账必须平衡");
        assert!(s.is_empty());
        assert!(s.seq_index.is_empty(), "排空后序号表必须清空");
        assert!(
            s.buckets.iter().all(BTreeSet::is_empty),
            "排空后桶必须全空（镜像不变式在长负载下仍成立）"
        );
    }

    /// 结构不变量（跨混合操作）：序号表双射、桶键与条目现键一致、seq 不得跨桶重复
    #[test]
    fn test_index_invariants_after_mixed_ops() {
        const PRIOS: [TaskPriority; 4] = [
            TaskPriority::Low,
            TaskPriority::Medium,
            TaskPriority::High,
            TaskPriority::Critical,
        ];
        let mut s = PriorityScheduler::with_config(
            WsjfWeights::default(),
            PriorityThresholds::default(),
            Duration::from_millis(1),
        );
        let input = WsjfInput::new(1.0, 1.0, 1.0, 1.0, 1.0);
        let mut next = 0usize;
        for round in 0..12usize {
            // 每轮 40 条 → 存活量足以让超时搬桶真实发生（配合下方 sleep）
            for _ in 0..40 {
                s.enqueue(
                    make_sched_task(&format!("t{next}"), PRIOS[round % 4]),
                    &input,
                );
                next += 1;
            }
            if round % 4 == 3 {
                s.recompute_from_wsjf();
            }
            // 跨过至少一个饥饿间隔，让搬桶路径真实发生
            std::thread::sleep(Duration::from_millis(2));
            if !s.is_empty() {
                let _ = s.dequeue();
            }
            assert_eq!(s.seq_index.len(), s.entries.len(), "序号表与条目基数应恒等");
            for idx in 0..s.entries.len() {
                let seq = s.entries[idx].seq;
                assert_eq!(
                    s.seq_index.get(&seq).copied(),
                    Some(idx),
                    "seq→下标反查必须回到自身"
                );
            }
            let mut seen = std::collections::HashSet::new();
            for (rank, bucket) in s.buckets.iter().enumerate() {
                for key in bucket {
                    // 不变式：桶集合与活条目集合严格互为镜像——桶内每个键都必须对应一个
                    // 活条目，且键值与其现键逐位一致、不跨桶重复。
                    let Some(&idx) = s.seq_index.get(&key.seq_asc.0) else {
                        panic!("桶内键必须对应活条目：rank={rank} seq={}", key.seq_asc.0);
                    };
                    assert_eq!(
                        s.key_of(idx),
                        *key,
                        "桶内键必须与条目现键一致（rank={rank}）"
                    );
                    assert!(seen.insert(key.seq_asc.0), "活 seq 不得同时出现在两个桶");
                }
            }
            assert_eq!(seen.len(), s.entries.len(), "每个活条目必须恰有一个桶键");
        }
    }
}
