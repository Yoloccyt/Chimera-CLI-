//! MAS 子系统稳定性守护 — Task 19 §19 系统稳定运行与功能完整闭环
//!
//! 架构层归属: L9 Quest(chimera-mas 内部子模块)
//! 核心职责: 零孤儿终态保证 + 故障隔离 + 降级链(CircuitBreaker 三态机)
//!
//! ## 三大组件(对应 SubTask 19.7-19.9)
//!
//! 1. **CircuitBreaker** — 三态断路器(Closed/Open/HalfOpen),基于 `AtomicU8` CAS,
//!    防止级联故障(§4.4 反模式 1:不持锁跨 `.await`)
//! 2. **StabilityGuard** — 零孤儿终态保证 + 故障隔离
//!    - `ensure_terminal_state(task_id)`:校验任务终态(Completed/Failed)
//!    - `isolate_failure(subtree_id)`:故障隔离,防止级联(INV-3/INV-4)
//! 3. **DegradationChain** — 降级链
//!    - `apply(pressure_source)`:返回降级步骤序列
//!
//! ## 零孤儿终态保证(§6.1 红线 + §19)
//!
//! 每个任务必须以 `AgentTaskCompleted` 或 `AgentTaskFailed` 之一结束,
//! 不允许"孤儿任务"(既未完成也未失败的悬挂态)。
//! `ensure_terminal_state()` 在 `RootOrchestrator::monitor()` 中调用校验。
//!
//! ## 故障隔离(INV-3/INV-4 验证)
//!
//! 某象限孙代理崩溃只影响其子树,不级联到其他象限。
//! `isolate_failure(subtree_id)` 标记子树为隔离态,后续派生拒绝进入隔离子树。
//!
//! ## 降级链(§19)
//!
//! | PressureSource | 降级步骤序列 |
//! |----------------|------------|
//! | MemoryNearBudget | [HcwCompress, TierDemote, RejectNewAgent] |
//! | ExpertOverload | [FallbackToLocalMlc, FallbackToWiki] |
//! | ArchiveIoContention | [DeferArchiveToLowPeak] |
//!
//! ## 红线对齐
//!
//! - §4.1: 库层 thiserror,无 unwrap/expect
//! - §4.4 反模式 1: AtomicU8 CAS,不持锁跨 `.await`
//! - §6.1: 单函数 ≤ 200 行
//! - §6.2: Critical 安全事件用 mpsc(`AgentTaskFailed` 走 mpsc channel 确保送达)
//! - `#![forbid(unsafe_code)]`: crate 级已在 lib.rs 声明,本模块无需重复
//!
//! ## TDD 状态(SubTask 19.1 + 19.7-19.9 GREEN 已完成)
//!
//! RED 阶段:仅声明类型,测试编译失败(25 个 E0599 错误,方法未实现)。
//! GREEN 阶段:补充 `impl` 块,所有测试通过。

// GREEN 阶段(SubTask 19.7-19.9)impl 块所需导入
use crate::error::{MasError, Result};
use dashmap::DashMap;
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};

// ============================================================
// 常量 — CircuitBreaker 三态(SubTask 19.7)
// ============================================================

/// Closed 状态常量 — 正常运行,允许请求通过
pub const STATE_CLOSED: u8 = 0;

/// Open 状态常量 — 断路,拒绝所有请求
pub const STATE_OPEN: u8 = 1;

/// HalfOpen 状态常量 — 半开,允许探测请求
pub const STATE_HALF_OPEN: u8 = 2;

// ============================================================
// CircuitBreaker — 三态断路器(SubTask 19.7)类型声明
// ============================================================

/// CircuitBreaker — 三态断路器(§19.7)
///
/// 基于 `AtomicU8` CAS 实现,无锁同步,避免 §4.4 反模式 1(持锁跨 `.await`)。
///
/// ## 状态机
///
/// ```text
/// Closed ──(failure_count >= threshold)──> Open
///   ↑                                        │
///   │                                        │
///   └──(reset_timeout elapsed)── HalfOpen <──┘
///   │                              │
///   └──(success)──────────────────┘
///   HalfOpen ──(failure)──> Open
/// ```
///
/// ## 线程安全
///
/// - `state: AtomicU8`:无锁 CAS 状态转换
/// - `failure_count: AtomicU32`:无锁失败计数累加
#[derive(Debug)]
pub struct CircuitBreaker {
    /// 当前状态(AtomicU8,无锁 CAS)
    pub(super) state: AtomicU8,
    /// 当前失败次数(AtomicU32,无锁累加)
    pub(super) failure_count: AtomicU32,
    /// 触发 Open 的失败次数阈值
    pub(super) threshold: u32,
    /// Open→HalfOpen 的重置超时(毫秒)
    pub(super) reset_timeout_ms: u64,
}

// ============================================================
// TerminalState — 任务终态(SubTask 19.8)类型声明
// ============================================================

/// 任务终态 — 零孤儿校验依据
///
/// 每个任务必须以 `Completed` 或 `Failed` 之一结束(§6.1 红线 + §19)。
/// `StabilityGuard::record_terminal()` 注册终态,
/// `ensure_terminal_state()` 校验终态已注册。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TerminalState {
    /// 任务已完成(发布 `AgentTaskCompleted`)
    Completed,
    /// 任务已失败(发布 `AgentTaskFailed`,走 mpsc,Critical 级)
    Failed,
}

// ============================================================
// StabilityGuard — 零孤儿终态保证 + 故障隔离(SubTask 19.8)类型声明
// ============================================================

/// StabilityGuard — 零孤儿终态保证 + 故障隔离(§19.8)
///
/// ## 核心职责
///
/// 1. **零孤儿终态**: 每个任务必须以 `Completed` 或 `Failed` 终态结束,
///    `ensure_terminal_state(task_id)` 校验任务终态已注册,
///    未注册则返回 `MasError::Internal`(孤儿任务)
/// 2. **故障隔离**: `isolate_failure(subtree_id)` 标记子树为隔离态,
///    不影响其他象限(INV-3/INV-4 验证)
///
/// ## 线程安全
///
/// - `terminals: DashMap<String, TerminalState>`:并发安全终态注册表
/// - `isolated_subtrees: DashMap<String, bool>`:并发安全隔离子树注册表
#[derive(Debug)]
pub struct StabilityGuard {
    /// 任务终态注册表(task_id → TerminalState)
    pub(super) terminals: DashMap<String, TerminalState>,
    /// 已隔离子树注册表(subtree_id → true)
    pub(super) isolated_subtrees: DashMap<String, bool>,
}

// ============================================================
// PressureSource — 降级链触发条件(SubTask 19.9)类型声明
// ============================================================

/// 压力源 — 降级链触发条件(§19)
///
/// 每个压力源对应一组有序降级步骤,由 `DegradationChain::apply()` 返回。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PressureSource {
    /// 内存压力近 130MB 预算(`M_total > 130MB × 0.9 = 117MB`)
    ///
    /// 触发降级链:[HcwCompress, TierDemote, RejectNewAgent]
    MemoryNearBudget,
    /// 专家过载(ExpertAgent 队列深度超阈值)
    ///
    /// 触发降级链:[FallbackToLocalMlc, FallbackToWiki]
    ExpertOverload,
    /// 归档 IO 竞争(CMT 三级归档触发磁盘 IO 高峰)
    ///
    /// 触发降级链:[DeferArchiveToLowPeak]
    ArchiveIoContention,
}

// ============================================================
// DegradationStep — 降级步骤(SubTask 19.9)类型声明
// ============================================================

/// 降级步骤 — 降级链中的单个动作(§19)
///
/// 每个 `PressureSource` 对应一个有序的 `DegradationStep` 序列,
/// 调用方按顺序执行降级步骤以缓解压力。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DegradationStep {
    /// HCW 压缩(0.9 系数,§15.4 INV-7)
    ///
    /// 触发 `hcw_window::HierarchicalWindow::select()` 稀疏化,
    /// 压缩 Agent 上下文驻留到 0.9 倍。
    HcwCompress,
    /// tier 降级(§17.2 CMT 冷热分层)
    ///
    /// 将 Agent 记忆从 Hot → Warm / Warm → Cold / Cold → Ice 降级。
    TierDemote,
    /// 拒新派生排队(§15.3 派生准入闸)
    ///
    /// 拒绝新的 Agent 派生请求,排队等待内存释放。
    RejectNewAgent,
    /// 回退到本地 MLC(§15.4 专家过载)
    ///
    /// 专家不可用时回退到本地 MLC 记忆检索。
    FallbackToLocalMlc,
    /// 回退到 Wiki(§15.4 专家过载)
    ///
    /// 本地 MLC 不可用时回退到 RepoWiki 全文检索。
    FallbackToWiki,
    /// 推迟归档到低峰期(§17.2 归档调度)
    ///
    /// 归档 IO 竞争时推迟归档到低峰期(02:00-04:00 UTC)。
    DeferArchiveToLowPeak,
}

// ============================================================
// DegradationChain — 降级链(SubTask 19.9)类型声明
// ============================================================

/// DegradationChain — 降级链(§19.9)
///
/// 根据 `PressureSource` 返回降级步骤序列。
/// 设计为无状态工具类(所有方法为关联函数),便于在压力检测点直接调用。
///
/// ## 降级链顺序(§19)
///
/// | PressureSource | 降级步骤序列 |
/// |----------------|------------|
/// | MemoryNearBudget | [HcwCompress, TierDemote, RejectNewAgent] |
/// | ExpertOverload | [FallbackToLocalMlc, FallbackToWiki] |
/// | ArchiveIoContention | [DeferArchiveToLowPeak] |
pub struct DegradationChain;

// ============================================================
// impl CircuitBreaker — 三态断路器实现(SubTask 19.7 GREEN)
// ============================================================

impl CircuitBreaker {
    /// 创建断路器,初始状态 Closed
    ///
    /// ## 参数
    /// - `threshold`: 触发 Open 的失败次数阈值(必须 ≥ 1)
    /// - `reset_timeout_ms`: Open→HalfOpen 的重置超时(毫秒)
    pub fn new(threshold: u32, reset_timeout_ms: u64) -> Self {
        Self {
            state: AtomicU8::new(STATE_CLOSED),
            failure_count: AtomicU32::new(0),
            threshold,
            reset_timeout_ms,
        }
    }

    /// 返回当前状态(0=Closed, 1=Open, 2=HalfOpen)
    pub fn state(&self) -> u8 {
        self.state.load(Ordering::Acquire)
    }

    /// 是否处于 Open 状态
    pub fn is_open(&self) -> bool {
        self.state() == STATE_OPEN
    }

    /// Closed → Open(失败次数超阈值时调用)
    ///
    /// WHY 无锁 store:无竞争场景下 Store 比 CAS 快;有竞争时最终一致(都写入 Open)
    pub fn trip_open(&self) {
        self.state.store(STATE_OPEN, Ordering::Release);
    }

    /// Open → HalfOpen(重置超时后调用,CAS 确保只转换一次)
    ///
    /// ## 返回
    /// - `true`: 成功从 Open 转换到 HalfOpen
    /// - `false`: 当前状态非 Open(如已 Closed 或 HalfOpen),不转换
    pub fn try_half_open(&self) -> bool {
        self.state
            .compare_exchange(
                STATE_OPEN,
                STATE_HALF_OPEN,
                Ordering::AcqRel,
                Ordering::Relaxed,
            )
            .is_ok()
    }

    /// HalfOpen → Closed(探测成功后调用)
    ///
    /// 同时重置 `failure_count` 为 0,允许新一轮失败计数。
    pub fn reset(&self) {
        self.state.store(STATE_CLOSED, Ordering::Release);
        self.failure_count.store(0, Ordering::Release);
    }

    /// 记录一次失败,失败计数 +1
    ///
    /// ## 返回
    /// - `true`: 失败次数达到阈值,触发 `trip_open()`(Closed→Open)
    /// - `false`: 失败次数尚未达阈值
    pub fn record_failure(&self) -> bool {
        // fetch_add 返回旧值,+1 得到本次累加后的值
        let count = self.failure_count.fetch_add(1, Ordering::AcqRel) + 1;
        if count >= self.threshold {
            self.trip_open();
            true
        } else {
            false
        }
    }

    /// 返回重置超时(毫秒)
    #[allow(dead_code)]
    pub fn reset_timeout_ms(&self) -> u64 {
        self.reset_timeout_ms
    }
}

// ============================================================
// impl StabilityGuard — 零孤儿终态保证 + 故障隔离(SubTask 19.8 GREEN)
// ============================================================

impl StabilityGuard {
    /// 创建守护器,初始状态无终态注册、无隔离子树
    pub fn new() -> Self {
        Self {
            terminals: DashMap::new(),
            isolated_subtrees: DashMap::new(),
        }
    }

    /// 注册任务终态(零孤儿保证的核心入口)
    ///
    /// ## 参数
    /// - `task_id`: 任务 ID
    /// - `state`: 终态(Completed / Failed)
    ///
    /// ## 语义
    ///
    /// 同一 task_id 重复注册以最后一次为准(覆盖)。
    /// 调用方在发布 `AgentTaskCompleted` / `AgentTaskFailed` 事件后调用本方法。
    pub fn record_terminal(&self, task_id: String, state: TerminalState) {
        self.terminals.insert(task_id, state);
    }

    /// 校验任务终态已注册(零孤儿校验)
    ///
    /// ## 参数
    /// - `task_id`: 任务 ID
    ///
    /// ## 返回
    /// - `Ok(())`: 任务终态已注册(Completed 或 Failed)
    /// - `Err(MasError::Internal)`: 任务未注册终态(孤儿任务,违反 §6.1 零孤儿红线)
    ///
    /// ## 使用场景
    ///
    /// `RootOrchestrator::monitor()` 在心跳收集循环中调用本方法,
    /// 校验已结束任务是否发布了终态事件。
    pub fn ensure_terminal_state(&self, task_id: &str) -> Result<()> {
        if self.terminals.contains_key(task_id) {
            Ok(())
        } else {
            Err(MasError::Internal(format!(
                "Task {task_id} has no terminal state (orphan) — violates §6.1 zero-orphan rule"
            )))
        }
    }

    /// 隔离故障子树(只影响该子树,不级联到其他象限)
    ///
    /// ## 参数
    /// - `subtree_id`: 故障子树 ID(通常为孙代理 agent_id)
    ///
    /// ## 语义
    ///
    /// 将 `subtree_id` 标记为隔离态,后续派生拒绝进入该子树。
    /// 其他象限(不同 subtree_id)不受影响,继续正常运行。
    ///
    /// ## 返回
    /// - `Ok(())`: 隔离成功(包括重复隔离,幂等)
    pub fn isolate_failure(&self, subtree_id: String) -> Result<()> {
        self.isolated_subtrees.insert(subtree_id, true);
        Ok(())
    }

    /// 检查子树是否已隔离
    ///
    /// ## 参数
    /// - `subtree_id`: 子树 ID
    ///
    /// ## 返回
    /// - `true`: 子树已隔离(派生应拒绝进入)
    /// - `false`: 子树未隔离(正常运行)
    pub fn is_isolated(&self, subtree_id: &str) -> bool {
        self.isolated_subtrees
            .get(subtree_id)
            .map(|v| *v)
            .unwrap_or(false)
    }

    /// 返回已注册终态的任务数量(供测试与监控指标使用)
    pub fn terminal_count(&self) -> usize {
        self.terminals.len()
    }

    /// 返回已隔离子树数量(供测试与监控指标使用)
    pub fn isolated_count(&self) -> usize {
        self.isolated_subtrees.len()
    }
}

impl Default for StabilityGuard {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// impl DegradationChain — 降级链(SubTask 19.9 GREEN)
// ============================================================

impl DegradationChain {
    /// 根据压力源返回降级步骤序列
    ///
    /// ## 参数
    /// - `source`: 压力源枚举
    ///
    /// ## 返回
    ///
    /// 有序降级步骤序列,调用方按顺序执行以缓解压力。
    ///
    /// ## 降级链顺序(§19)
    ///
    /// | PressureSource | 降级步骤序列 |
    /// |----------------|------------|
    /// | MemoryNearBudget | [HcwCompress, TierDemote, RejectNewAgent] |
    /// | ExpertOverload | [FallbackToLocalMlc, FallbackToWiki] |
    /// | ArchiveIoContention | [DeferArchiveToLowPeak] |
    pub fn apply(source: PressureSource) -> Vec<DegradationStep> {
        match source {
            PressureSource::MemoryNearBudget => vec![
                DegradationStep::HcwCompress,
                DegradationStep::TierDemote,
                DegradationStep::RejectNewAgent,
            ],
            PressureSource::ExpertOverload => vec![
                DegradationStep::FallbackToLocalMlc,
                DegradationStep::FallbackToWiki,
            ],
            PressureSource::ArchiveIoContention => vec![DegradationStep::DeferArchiveToLowPeak],
        }
    }
}
