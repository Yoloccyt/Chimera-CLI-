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
use std::time::{Duration, Instant};

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
    // WHY as u64 后相除: Duration 无直接除法得整数倍, 用纳秒比值取整得"满几个间隔"。
    let intervals = (waited.as_nanos() / threshold.as_nanos().max(1)) as u64;
    let boosted = base_rank as u64 + intervals;
    boosted.min(3) as u8
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

/// 调度队列条目 — 任务 + 其 WSJF 评分 + 入队时刻(用于饥饿老化)。
#[derive(Debug, Clone)]
struct ScheduleEntry {
    task: AgentTask,
    wsjf: f64,
    enqueued_at: Instant,
}

/// 优先级调度器 (§8) — 按 (有效优先级秩, WSJF) 出队, 支持动态重排与饥饿保护。
///
/// 采用「惰性最佳选择」: 出队时按当前有效秩(含饥饿老化)+ WSJF 选出最优条目,
/// 因此队列始终返回当下最应调度的任务, 无需维护堆的键稳定性。
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

    /// 入队 — 计算 WSJF 评分并记录入队时刻。
    ///
    /// 任务自身的 `priority` 字段保留为主排序键;WSJF 作为同优先级内的次排序键。
    pub fn enqueue(&mut self, task: AgentTask, wsjf_input: &WsjfInput) {
        let wsjf = wsjf_score(wsjf_input, &self.weights);
        self.entries.push(ScheduleEntry {
            task,
            wsjf,
            enqueued_at: Instant::now(),
        });
    }

    /// 出队 — 移除并返回当前最应调度的任务。
    ///
    /// 选择规则: 先比较有效优先级秩(含饥饿老化), 秩相同再比 WSJF(高者先),
    /// 仍相同则取先入队者(稳定)。队列为空返回 `None`。
    pub fn dequeue(&mut self) -> Option<AgentTask> {
        let now = Instant::now();
        let best = self.best_index(now)?;
        Some(self.entries.remove(best).task)
    }

    /// 查看(不移除)当前最应调度任务的有效优先级。
    pub fn peek_effective_priority(&self) -> Option<TaskPriority> {
        let now = Instant::now();
        let best = self.best_index(now)?;
        let entry = &self.entries[best];
        let rank = aged_priority_rank(
            entry.task.priority,
            now - entry.enqueued_at,
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
    }

    /// 内部: 返回当前最优条目的下标(含饥饿老化), 空则 `None`。
    fn best_index(&self, now: Instant) -> Option<usize> {
        if self.entries.is_empty() {
            return None;
        }
        let mut best_idx = 0usize;
        let mut best_rank = self.effective_rank(0, now);
        let mut best_wsjf = self.entries[0].wsjf;
        for (idx, entry) in self.entries.iter().enumerate().skip(1) {
            let rank = self.effective_rank(idx, now);
            // 主键: 秩更高优先; 次键: 秩相同则 WSJF 更高优先。
            let higher = rank > best_rank
                || (rank == best_rank
                    && entry.wsjf.partial_cmp(&best_wsjf) == Some(std::cmp::Ordering::Greater));
            if higher {
                best_idx = idx;
                best_rank = rank;
                best_wsjf = entry.wsjf;
            }
        }
        Some(best_idx)
    }

    /// 内部: 第 `idx` 条目的有效优先级秩(基础优先级 + 饥饿老化)。
    fn effective_rank(&self, idx: usize, now: Instant) -> u8 {
        let entry = &self.entries[idx];
        aged_priority_rank(
            entry.task.priority,
            now.saturating_duration_since(entry.enqueued_at),
            self.starvation_threshold,
        )
    }
}

// ============================================================
// 单元测试(纯函数为主, 调度器行为集成级见 tests/scheduler_test.rs)
// ============================================================

#[cfg(test)]
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
}
