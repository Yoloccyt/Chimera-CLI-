//! Agent 生命周期管理 — 状态机实现(Task 8.3)
//!
//! 本模块定义 `LifecycleState` 状态枚举与 `AgentLifecycle` 状态机管理器,
//! 负责管理 Agent 的生命周期状态转换。
//!
//! ## 状态机规则(ADR-026 决策 5 + Task 8.3)
//!
//! ```text
//!     create(AgentFactory::create_agent)
//!       │
//!       ▼
//!    ┌──────┐  start    ┌─────────┐  pause   ┌────────┐
//!    │ Idle │ ─────────▶ │ Running │ ─────────▶ │ Paused │
//!    └──────┘             └─────────┘           └────────┘
//!      ▲  │                  │  │                   │ resume
//!      │  │ restart          │  │                   ▼
//!      │  │ (终态→Idle)       │  │              ┌─────────┐
//!      │  │                  │  │              │ Running │
//!      │  │                  │  │              └─────────┘
//!      │  │          success  │  │ failure
//!      │  │                   ▼  ▼
//!      │  │            ┌───────────┐  ┌────────┐
//!      │  └─────────── │ Completed │  │ Failed │ ◀── crash(非终态)
//!      │     restart    └───────────┘  └────────┘
//!      │                                   ▲
//!      │                                   │
//!      └─────────── restart ───────────────┘
//!      │
//!      │            destroy(任意状态)
//!      │                ▼
//!      │           ┌────────────┐
//!      └restart─── │ Terminated │
//!                  └────────────┘
//! ```
//!
//! ## 合法转换表
//!
//! | 当前状态 | 方法 | 目标状态 | 说明 |
//! |---------|------|---------|------|
//! | Idle | start | Running | 启动 Agent |
//! | Running | pause | Paused | 暂停 Agent |
//! | Paused | resume | Running | 恢复 Agent |
//! | Running | complete | Completed | 任务成功 |
//! | Running/Failed | fail | Failed | 任务失败(Failed 幂等) |
//! | 非终态 | crash | Failed | 崩溃(静默,无错误返回) |
//! | 任意 | destroy | Terminated | 销毁(幂等) |
//! | 终态 | restart | Idle | 重启(回到 Idle) |
//!
//! 非法转换返回 `MasError::InvalidAgentState`。

use crate::agent::meta::AgentStatus;
use crate::error::{MasError, Result};
use serde::{Deserialize, Serialize};

// ============================================================
// LifecycleState — 生命周期状态枚举
// ============================================================

/// Agent 生命周期状态 — 独立于 `AgentStatus` 的状态机枚举
///
/// WHY 独立定义而非复用 `AgentStatus`:
/// - `AgentStatus`(meta.rs)无 `Terminated` 变体,仅有 `Crashed`(panic 崩溃)
/// - 语义上 `Terminated`(主动销毁)与 `Crashed`(意外崩溃)不同,需区分
/// - 不修改 `AgentStatus`(Task 7 已完成,修改会破坏既有测试与契约)
/// - 通过 `as_agent_status()` 提供 映射,保持与 `AgentMeta.status` 同步
///
/// ## 终态判定
///
/// - `Completed` / `Failed` / `Terminated` 为终态(`is_terminal = true`)
/// - `Idle` / `Running` / `Paused` 为非终态
/// - 终态可通过 `restart` 回到 `Idle`
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
pub enum LifecycleState {
    /// 空闲:已创建但未启动(初始状态)
    #[default]
    Idle,
    /// 运行中:正在执行任务
    Running,
    /// 已暂停:被外部暂停,可恢复
    Paused,
    /// 已完成:任务成功结束(终态)
    Completed,
    /// 已失败:任务执行失败(终态)
    Failed,
    /// 已终止:被主动销毁,资源已回收(终态)
    Terminated,
}

impl LifecycleState {
    /// 转换为 `AgentStatus` — 与 `AgentMeta.status` 字段同步
    ///
    /// ## 映射规则
    ///
    /// | LifecycleState | AgentStatus | 说明 |
    /// |----------------|-------------|------|
    /// | Idle | Idle | 一一映射 |
    /// | Running | Running | 一一映射 |
    /// | Paused | Paused | 一一映射 |
    /// | Completed | Completed | 一一映射 |
    /// | Failed | Failed | 一一映射 |
    /// | Terminated | Crashed | `AgentStatus` 无 Terminated,映射为 Crashed |
    ///
    /// WHY Terminated → Crashed:两者均表示"已终止不可恢复",
    /// `AgentStatus::Crashed` 语义为"不可恢复",与 Terminated 语义一致。
    /// 若未来 `AgentStatus` 新增 `Terminated` 变体,此处应改为一一映射。
    pub fn as_agent_status(self) -> AgentStatus {
        match self {
            Self::Idle => AgentStatus::Idle,
            Self::Running => AgentStatus::Running,
            Self::Paused => AgentStatus::Paused,
            Self::Completed => AgentStatus::Completed,
            Self::Failed => AgentStatus::Failed,
            Self::Terminated => AgentStatus::Crashed,
        }
    }

    /// 是否为终态(不可继续推进,只能 restart)
    ///
    /// 终态:`Completed` / `Failed` / `Terminated`
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Terminated)
    }
}

impl std::fmt::Display for LifecycleState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

// ============================================================
// AgentLifecycle — 生命周期状态机管理器
// ============================================================

/// Agent 生命周期状态机管理器 — 持有状态并执行合法转换
///
/// ## 设计
///
/// - 持有 `state: LifecycleState` 与 `transition_count: u64`
/// - 所有转换方法用 `&mut self`(修改内部状态)
/// - 合法转换返回 `Ok(())`,非法转换返回 `MasError::InvalidAgentState`
/// - `crash` 方法例外:无错误返回(崩溃是不可恢复事件,静默转 Failed)
///
/// ## 与 Agent 的关系
///
/// `Agent` 持有 `AgentLifecycle`,状态转换时:
/// 1. 调用 `AgentLifecycle` 方法更新内部 state
/// 2. 同步更新 `AgentMeta.status = lifecycle.current_state().as_agent_status()`
///
/// 这样 `AgentMeta.status`(对外可见)与 `AgentLifecycle.state`(状态机真实状态)
/// 保持一致,避免双重状态源不一致。
#[derive(Debug, Clone)]
pub struct AgentLifecycle {
    /// 当前生命周期状态
    state: LifecycleState,
    /// 累计状态转换次数(用于诊断与监控,重启不归零)
    transition_count: u64,
}

impl AgentLifecycle {
    /// 创建新的生命周期管理器,初始状态为 `Idle`
    pub fn new() -> Self {
        Self {
            state: LifecycleState::Idle,
            transition_count: 0,
        }
    }

    /// 返回当前生命周期状态
    pub fn current_state(&self) -> LifecycleState {
        self.state
    }

    /// 返回累计状态转换次数
    ///
    /// 每次成功转换(含幂等转换)递增 1,用于诊断状态机活跃度。
    pub fn transition_count(&self) -> u64 {
        self.transition_count
    }

    /// 启动 Agent: Idle → Running
    ///
    /// ## 错误
    /// - `MasError::InvalidAgentState`: 当前状态不是 `Idle`
    pub fn start(&mut self, agent_id: &str) -> Result<()> {
        match self.state {
            LifecycleState::Idle => {
                self.state = LifecycleState::Running;
                self.transition_count += 1;
                Ok(())
            }
            _ => Err(self.invalid_state_err(agent_id, "Idle")),
        }
    }

    /// 暂停 Agent: Running → Paused
    ///
    /// ## 错误
    /// - `MasError::InvalidAgentState`: 当前状态不是 `Running`
    pub fn pause(&mut self, agent_id: &str) -> Result<()> {
        match self.state {
            LifecycleState::Running => {
                self.state = LifecycleState::Paused;
                self.transition_count += 1;
                Ok(())
            }
            _ => Err(self.invalid_state_err(agent_id, "Running")),
        }
    }

    /// 恢复 Agent: Paused → Running
    ///
    /// ## 错误
    /// - `MasError::InvalidAgentState`: 当前状态不是 `Paused`
    pub fn resume(&mut self, agent_id: &str) -> Result<()> {
        match self.state {
            LifecycleState::Paused => {
                self.state = LifecycleState::Running;
                self.transition_count += 1;
                Ok(())
            }
            _ => Err(self.invalid_state_err(agent_id, "Paused")),
        }
    }

    /// 标记任务成功: Running → Completed
    ///
    /// ## 错误
    /// - `MasError::InvalidAgentState`: 当前状态不是 `Running`
    pub fn complete(&mut self, agent_id: &str) -> Result<()> {
        match self.state {
            LifecycleState::Running => {
                self.state = LifecycleState::Completed;
                self.transition_count += 1;
                Ok(())
            }
            _ => Err(self.invalid_state_err(agent_id, "Running")),
        }
    }

    /// 标记任务失败: Running | Failed → Failed
    ///
    /// `Failed → Failed` 幂等(任务描述: Running/Failed → Failed),
    /// 允许重复调用 fail 而不报错。
    ///
    /// ## 错误
    /// - `MasError::InvalidAgentState`: 当前状态既非 `Running` 也非 `Failed`
    pub fn fail(&mut self, agent_id: &str) -> Result<()> {
        match self.state {
            LifecycleState::Running | LifecycleState::Failed => {
                self.state = LifecycleState::Failed;
                self.transition_count += 1;
                Ok(())
            }
            _ => Err(self.invalid_state_err(agent_id, "Running | Failed")),
        }
    }

    /// 标记 Agent 崩溃: 非终态 → Failed(静默,无错误返回)
    ///
    /// 语义:Agent 发生 panic 或不可恢复错误,强制转为 `Failed`。
    /// 已终态(`Completed`/`Failed`/`Terminated`)调用 crash 为 no-op(静默忽略),
    /// 因为终态已不可恢复,crash 无法进一步改变状态。
    ///
    /// WHY 无错误返回:崩溃是不可恢复事件,调用方无法处理"崩溃失败"的情况,
    /// 静默忽略终态 crash 避免调用方需要额外错误处理。
    pub fn crash(&mut self) {
        match self.state {
            LifecycleState::Idle | LifecycleState::Running | LifecycleState::Paused => {
                self.state = LifecycleState::Failed;
                self.transition_count += 1;
            }
            // 终态:Completed/Failed/Terminated,crash 为 no-op
            _ => {}
        }
    }

    /// 销毁 Agent: 任意状态 → Terminated(幂等)
    ///
    /// 语义:主动终止 Agent 并回收资源。任意状态均可 destroy,
    /// 已 `Terminated` 再次 destroy 幂等成功(不重复计数)。
    ///
    /// ## 返回
    /// - `Ok(())`: 总是成功(destroy 是幂等终态转换)
    ///
    /// WHY 返回 Result 而非 ():保留扩展空间,未来 destroy 可能涉及
    /// 资源回收失败(如持久化状态写盘失败),届时可返回错误。
    pub fn destroy(&mut self, _agent_id: &str) -> Result<()> {
        if self.state != LifecycleState::Terminated {
            self.state = LifecycleState::Terminated;
            self.transition_count += 1;
        }
        // 已 Terminated 幂等,不重复计数
        Ok(())
    }

    /// 重启 Agent: 终态 → Idle
    ///
    /// 语义:将 Agent 从终态(`Completed`/`Failed`/`Terminated`)重置为 `Idle`,
    /// 允许重新 start。非终态调用 restart 为非法转换。
    ///
    /// ## 错误
    /// - `MasError::InvalidAgentState`: 当前状态不是终态
    pub fn restart(&mut self, agent_id: &str) -> Result<()> {
        match self.state {
            LifecycleState::Completed | LifecycleState::Failed | LifecycleState::Terminated => {
                self.state = LifecycleState::Idle;
                self.transition_count += 1;
                Ok(())
            }
            _ => Err(self.invalid_state_err(agent_id, "Completed | Failed | Terminated")),
        }
    }

    /// 构造非法状态错误(内部辅助方法)
    ///
    /// 统一生成 `MasError::InvalidAgentState`,包含 agent_id、当前状态、期望状态。
    fn invalid_state_err(&self, agent_id: &str, expected: &str) -> MasError {
        MasError::InvalidAgentState {
            agent_id: agent_id.to_string(),
            current_state: self.state.to_string(),
            expected_state: expected.to_string(),
        }
    }
}

impl Default for AgentLifecycle {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lifecycle_new_initial_state_idle() {
        let lc = AgentLifecycle::new();
        assert_eq!(lc.current_state(), LifecycleState::Idle);
        assert_eq!(lc.transition_count(), 0);
    }

    #[test]
    fn test_lifecycle_default_is_idle() {
        let lc = AgentLifecycle::default();
        assert_eq!(lc.current_state(), LifecycleState::Idle);
    }

    #[test]
    fn test_lifecycle_start_from_idle_ok() {
        let mut lc = AgentLifecycle::new();
        lc.start("a-1").unwrap();
        assert_eq!(lc.current_state(), LifecycleState::Running);
        assert_eq!(lc.transition_count(), 1);
    }

    #[test]
    fn test_lifecycle_start_from_running_err() {
        let mut lc = AgentLifecycle::new();
        lc.start("a-1").unwrap();
        let err = lc.start("a-1").unwrap_err();
        assert!(matches!(err, MasError::InvalidAgentState { .. }));
    }

    #[test]
    fn test_lifecycle_full_happy_path() {
        let mut lc = AgentLifecycle::new();
        lc.start("a-1").unwrap();
        lc.pause("a-1").unwrap();
        lc.resume("a-1").unwrap();
        lc.complete("a-1").unwrap();
        assert_eq!(lc.current_state(), LifecycleState::Completed);
        assert_eq!(lc.transition_count(), 4);
    }

    #[test]
    fn test_lifecycle_fail_idempotent() {
        let mut lc = AgentLifecycle::new();
        lc.start("a-1").unwrap();
        lc.fail("a-1").unwrap();
        // Failed → Failed 幂等
        lc.fail("a-1").unwrap();
        assert_eq!(lc.current_state(), LifecycleState::Failed);
    }

    #[test]
    fn test_lifecycle_crash_from_non_terminal() {
        let mut lc = AgentLifecycle::new();
        lc.start("a-1").unwrap();
        lc.crash();
        assert_eq!(lc.current_state(), LifecycleState::Failed);
    }

    #[test]
    fn test_lifecycle_crash_from_terminal_noop() {
        let mut lc = AgentLifecycle::new();
        lc.start("a-1").unwrap();
        lc.complete("a-1").unwrap();
        let count_before = lc.transition_count();
        lc.crash();
        assert_eq!(lc.current_state(), LifecycleState::Completed);
        assert_eq!(lc.transition_count(), count_before, "终态 crash 不应计数");
    }

    #[test]
    fn test_lifecycle_destroy_idempotent() {
        let mut lc = AgentLifecycle::new();
        lc.destroy("a-1").unwrap();
        assert_eq!(lc.current_state(), LifecycleState::Terminated);
        let count_after_first = lc.transition_count();
        lc.destroy("a-1").unwrap();
        assert_eq!(
            lc.transition_count(),
            count_after_first,
            "重复 destroy 不应计数"
        );
    }

    #[test]
    fn test_lifecycle_restart_from_terminal() {
        let mut lc = AgentLifecycle::new();
        lc.start("a-1").unwrap();
        lc.complete("a-1").unwrap();
        lc.restart("a-1").unwrap();
        assert_eq!(lc.current_state(), LifecycleState::Idle);
    }

    #[test]
    fn test_lifecycle_restart_from_non_terminal_err() {
        let mut lc = AgentLifecycle::new();
        lc.start("a-1").unwrap();
        let err = lc.restart("a-1").unwrap_err();
        assert!(matches!(err, MasError::InvalidAgentState { .. }));
    }

    #[test]
    fn test_lifecycle_state_is_terminal() {
        assert!(!LifecycleState::Idle.is_terminal());
        assert!(!LifecycleState::Running.is_terminal());
        assert!(!LifecycleState::Paused.is_terminal());
        assert!(LifecycleState::Completed.is_terminal());
        assert!(LifecycleState::Failed.is_terminal());
        assert!(LifecycleState::Terminated.is_terminal());
    }

    #[test]
    fn test_lifecycle_state_as_agent_status() {
        // 验证 LifecycleState -> AgentStatus 的映射规则(见 as_agent_status 注释)
        assert_eq!(LifecycleState::Idle.as_agent_status(), AgentStatus::Idle);
        assert_eq!(
            LifecycleState::Running.as_agent_status(),
            AgentStatus::Running
        );
        assert_eq!(
            LifecycleState::Paused.as_agent_status(),
            AgentStatus::Paused
        );
        assert_eq!(
            LifecycleState::Completed.as_agent_status(),
            AgentStatus::Completed
        );
        assert_eq!(
            LifecycleState::Failed.as_agent_status(),
            AgentStatus::Failed
        );
        // Terminated -> Crashed(AgentStatus 无 Terminated 变体,语义映射)
        assert_eq!(
            LifecycleState::Terminated.as_agent_status(),
            AgentStatus::Crashed
        );
    }
}
