//! ReasoningState 七态状态机 — 显式控制流 + 不变量 proptest 对齐
//!
//! 对应架构层:L8 Parliament
//! 对应文档:NEXUS-OMEGA_v5.0_系统性完整设计文档.md §9.3
//!
//! # 设计目标(§9.3)
//! ReasoningState 七态转移表(纯函数),proptest 三性质(无死锁 / 全可达 / 无意外循环)。
//! 对齐既有 INV-7/8 的 1000 次 proptest 先例(chimera-mas invariants.rs)。
//!
//! # 七态设计(对应 `debate.rs::Parliament::deliberate` 流程)
//!
//! ```text
//!                  ProposalSubmitted
//!       Idle ──────────────────────────► VetoCheck
//!        ▲                                 │
//!        │                                 ├── VetoTriggered ──► Vetoed (终态)
//!        │                                 │
//!        │                            DebateStarted
//!        │                                 │
//!        │                                 ▼
//!        │                              Debating
//!        │                                 │
//!        │                          OpinionsCollected
//!        │                                 │
//!        │                                 ▼
//!        │                                Voting
//!        │                                 │
//!        │                      ┌──────────┴──────────┐
//!        │                      │                     │
//!        │               ConsensusReached      ConsensusFailed
//!        │                      │                     │
//!        │                      ▼                     ▼
//!        │                  Accepted (终态)      Rejected (终态)
//!        │                      │                     │
//!        └──────────Reset───────┴─────────────────────┘
//! ```
//!
//! # 状态语义
//! - **Idle**: 无活跃审议,等待新提案
//! - **VetoCheck**: Skeptic 恶意意图检测(辩论前,红队防线)
//! - **Debating**: 5 角色并行提供 Opinion(Architect/Skeptic/Optimizer/Librarian/Bard)
//! - **Voting**: 投票计数(VoteCounter::count_votes)
//! - **Accepted**: 共识达成(终态,赞成率 ≥ 阈值且无否决)
//! - **Rejected**: 赞成率不足(终态,approval < threshold)
//! - **Vetoed**: Skeptic 否决(终态,安全机制,与 Rejected 区分以触发能力冻结)
//!
//! # 纯函数设计(WHY)
//! `transition()` 是纯函数(无副作用,无状态),输入 `(state, event)` 输出 `Option<state>`。
//! - **可测试性**: 无需 mock,直接断言输入输出对
//! - **可组合**: 可嵌入任意状态机容器(Parliament/audit_trail/replay)
//! - **可证明**: proptest 验证三性质,数学归纳法可证明无死锁
//!
//! # 不变量(三性质,§9.3)
//! 1. **无死锁**(No Deadlocks): 所有非终态状态至少有一个有效后继事件
//! 2. **全可达**(All Reachable): 从 Idle 出发,BFS 可达全部 7 个状态
//! 3. **无意外循环**(No Unexpected Cycles): 唯一允许的循环是终态 → Idle → (下一轮审议)

use serde::{Deserialize, Serialize};

// ============================================================
// ReasoningState — 七态枚举
// ============================================================

/// ReasoningState 七态状态机 — 跟踪 Parliament 审议流程的完整生命周期
///
/// 对应 `debate.rs::Parliament::deliberate` 的 7 个阶段:
/// Idle → VetoCheck → (Vetoed | Debating → Voting → (Accepted | Rejected)) → Idle
///
/// # 设计决策(WHY)
/// - Vetoed 独立于 Rejected:否决是安全机制(红队防线),需触发能力冻结,
///   与普通拒绝(赞成率不足)语义不同(对齐 `Consensus::Vetoed` 独立于 `Consensus::Rejected`)
/// - 终态(Accepted/Rejected/Vetoed)只能通过 Reset 回到 Idle,
///   避免审议流程中途被篡改(单次审议一旦结束不可逆)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ReasoningState {
    /// 无活跃审议,等待新提案(初始态)
    Idle,
    /// Skeptic 恶意意图检测(辩论前,红队防线)
    VetoCheck,
    /// 5 角色并行提供 Opinion
    Debating,
    /// 投票计数(VoteCounter::count_votes)
    Voting,
    /// 共识达成(终态,赞成率 ≥ 阈值且无否决)
    Accepted,
    /// 赞成率不足(终态,approval < threshold)
    Rejected,
    /// Skeptic 否决(终态,安全机制,触发能力冻结)
    Vetoed,
}

impl ReasoningState {
    /// 返回所有状态(固定顺序,用于遍历与 proptest 策略生成)
    pub fn all() -> [Self; 7] {
        [
            Self::Idle,
            Self::VetoCheck,
            Self::Debating,
            Self::Voting,
            Self::Accepted,
            Self::Rejected,
            Self::Vetoed,
        ]
    }

    /// 判断是否为终态(Accepted/Rejected/Vetoed)
    ///
    /// 终态只能通过 Reset 事件回到 Idle,不可直接转移到其他非终态。
    pub fn is_terminal(&self) -> bool {
        matches!(self, Self::Accepted | Self::Rejected | Self::Vetoed)
    }

    /// 判断是否为非终态(Idle/VetoCheck/Debating/Voting)
    pub fn is_non_terminal(&self) -> bool {
        !self.is_terminal()
    }

    /// 返回状态的字符串标识(用于日志、序列化、proptest 诊断)
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::VetoCheck => "veto_check",
            Self::Debating => "debating",
            Self::Voting => "voting",
            Self::Accepted => "accepted",
            Self::Rejected => "rejected",
            Self::Vetoed => "vetoed",
        }
    }
}

impl std::fmt::Display for ReasoningState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ============================================================
// ReasoningEvent — 触发状态转移的事件
// ============================================================

/// ReasoningEvent — 触发 ReasoningState 转移的事件
///
/// 每个事件对应 `debate.rs::Parliament::deliberate` 流程中的一个步骤触发点:
/// - ProposalSubmitted: 提案提交(步骤 0 之前,Idle → VetoCheck)
/// - VetoTriggered: Skeptic 检测到恶意意图(步骤 0,VetoCheck → Vetoed)
/// - DebateStarted: 辩论开始(步骤 1,VetoCheck → Debating)
/// - OpinionsCollected: 5 角色 Opinion 收集完成(步骤 2-3,Debating → Voting)
/// - ConsensusReached: 共识达成(步骤 5,Voting → Accepted)
/// - ConsensusFailed: 共识未达成(步骤 5,Voting → Rejected)
/// - Reset: 审议结束,重置回 Idle(终态 → Idle)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ReasoningEvent {
    /// 提案提交(Idle → VetoCheck)
    ProposalSubmitted,
    /// Skeptic 检测到恶意意图(VetoCheck → Vetoed)
    VetoTriggered,
    /// 辩论开始(VetoCheck → Debating)
    DebateStarted,
    /// 5 角色 Opinion 收集完成(Debating → Voting)
    OpinionsCollected,
    /// 共识达成(Voting → Accepted)
    ConsensusReached,
    /// 共识未达成(Voting → Rejected)
    ConsensusFailed,
    /// 重置回 Idle(终态 → Idle)
    Reset,
}

impl ReasoningEvent {
    /// 返回所有事件(固定顺序,用于遍历与 proptest 策略生成)
    pub fn all() -> [Self; 7] {
        [
            Self::ProposalSubmitted,
            Self::VetoTriggered,
            Self::DebateStarted,
            Self::OpinionsCollected,
            Self::ConsensusReached,
            Self::ConsensusFailed,
            Self::Reset,
        ]
    }

    /// 返回事件的字符串标识(用于日志、序列化、proptest 诊断)
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ProposalSubmitted => "proposal_submitted",
            Self::VetoTriggered => "veto_triggered",
            Self::DebateStarted => "debate_started",
            Self::OpinionsCollected => "opinions_collected",
            Self::ConsensusReached => "consensus_reached",
            Self::ConsensusFailed => "consensus_failed",
            Self::Reset => "reset",
        }
    }
}

impl std::fmt::Display for ReasoningEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ============================================================
// transition — 纯函数状态转移表
// ============================================================

/// 纯函数状态转移表 — ReasoningState 状态机的核心
///
/// 输入 `(current_state, event)`,输出 `Option<next_state>`:
/// - `Some(next)`: 事件在当前状态下有效,转移到 next
/// - `None`: 事件在当前状态下无效(非法转移),调用方应记录或拒绝
///
/// # 转移表(完整枚举)
///
/// | 当前状态 | 事件 | 下一状态 | 说明 |
/// |----------|------|----------|------|
/// | Idle | ProposalSubmitted | VetoCheck | 提案提交,进入否决检查 |
/// | VetoCheck | VetoTriggered | Vetoed | Skeptic 否决触发(终态) |
/// | VetoCheck | DebateStarted | Debating | 无否决,进入辩论 |
/// | Debating | OpinionsCollected | Voting | Opinion 收集完成,进入投票 |
/// | Voting | ConsensusReached | Accepted | 共识达成(终态) |
/// | Voting | ConsensusFailed | Rejected | 共识未达成(终态) |
/// | Accepted | Reset | Idle | 重置回初始态 |
/// | Rejected | Reset | Idle | 重置回初始态 |
/// | Vetoed | Reset | Idle | 重置回初始态 |
///
/// 其余 (state, event) 组合返回 `None`(非法转移)。
///
/// # 纯函数性质(WHY)
/// - 无副作用:不修改任何外部状态,不发布事件,不记录日志
/// - 确定性:相同输入总是产生相同输出
/// - 可测试:无需 mock,直接断言 `(state, event) → Option<state>`
/// - 可组合:可嵌入 Parliament::deliberate / audit_trail / replay 等容器
///
/// # 示例
/// ```
/// use parliament::reasoning::{transition, ReasoningState, ReasoningEvent};
///
/// // 合法转移:Idle + ProposalSubmitted → VetoCheck
/// let next = transition(ReasoningState::Idle, ReasoningEvent::ProposalSubmitted);
/// assert_eq!(next, Some(ReasoningState::VetoCheck));
///
/// // 非法转移:Idle + ConsensusReached → None(不能从 Idle 直接达成共识)
/// let invalid = transition(ReasoningState::Idle, ReasoningEvent::ConsensusReached);
/// assert_eq!(invalid, None);
/// ```
pub fn transition(state: ReasoningState, event: ReasoningEvent) -> Option<ReasoningState> {
    match (state, event) {
        // Idle → VetoCheck(提案提交)
        (ReasoningState::Idle, ReasoningEvent::ProposalSubmitted) => {
            Some(ReasoningState::VetoCheck)
        }

        // VetoCheck → Vetoed(Skeptic 否决,终态)
        (ReasoningState::VetoCheck, ReasoningEvent::VetoTriggered) => Some(ReasoningState::Vetoed),
        // VetoCheck → Debating(无否决,进入辩论)
        (ReasoningState::VetoCheck, ReasoningEvent::DebateStarted) => {
            Some(ReasoningState::Debating)
        }

        // Debating → Voting(Opinion 收集完成)
        (ReasoningState::Debating, ReasoningEvent::OpinionsCollected) => {
            Some(ReasoningState::Voting)
        }

        // Voting → Accepted(共识达成,终态)
        (ReasoningState::Voting, ReasoningEvent::ConsensusReached) => {
            Some(ReasoningState::Accepted)
        }
        // Voting → Rejected(共识未达成,终态)
        (ReasoningState::Voting, ReasoningEvent::ConsensusFailed) => Some(ReasoningState::Rejected),

        // 终态 → Idle(重置,唯一允许的终态出口)
        (ReasoningState::Accepted, ReasoningEvent::Reset) => Some(ReasoningState::Idle),
        (ReasoningState::Rejected, ReasoningEvent::Reset) => Some(ReasoningState::Idle),
        (ReasoningState::Vetoed, ReasoningEvent::Reset) => Some(ReasoningState::Idle),

        // 其余 (state, event) 组合均为非法转移
        _ => None,
    }
}

// ============================================================
// 不变量验证(三性质,§9.3)
// ============================================================

/// 不变量 1:无死锁 — 所有非终态状态至少有一个有效后继事件
///
/// 遍历 `ReasoningState::all()`,对每个非终态状态检查是否至少存在一个事件
/// 使 `transition(state, event)` 返回 `Some(_)`。
///
/// # 返回
/// - `Ok(())`: 所有非终态状态至少有一个有效后继
/// - `Err(deadlocked_states)`: 列出所有无后继的非终态状态(违反不变量)
pub fn invariant_no_deadlocks() -> Result<(), Vec<ReasoningState>> {
    let deadlocked: Vec<ReasoningState> = ReasoningState::all()
        .iter()
        .copied()
        .filter(|s| s.is_non_terminal())
        .filter(|s| {
            // 检查所有事件是否都无法转移
            ReasoningEvent::all()
                .iter()
                .all(|e| transition(*s, *e).is_none())
        })
        .collect();

    if deadlocked.is_empty() {
        Ok(())
    } else {
        Err(deadlocked)
    }
}

/// 不变量 2:全可达 — 从 Idle 出发,BFS 可达全部 7 个状态
///
/// 从 `ReasoningState::Idle` 出发,使用 BFS 遍历所有有效转移,
/// 检查是否可达 `ReasoningState::all()` 中的全部 7 个状态。
///
/// # 返回
/// - `Ok(())`: 从 Idle 可达所有 7 个状态
/// - `Err(unreachable_states)`: 列出从 Idle 不可达的状态(违反不变量)
pub fn invariant_all_reachable() -> Result<(), Vec<ReasoningState>> {
    use std::collections::HashSet;

    let mut visited: HashSet<ReasoningState> = HashSet::new();
    let mut frontier: Vec<ReasoningState> = vec![ReasoningState::Idle];

    while let Some(state) = frontier.pop() {
        if visited.insert(state) {
            // 探索所有有效后继
            for event in ReasoningEvent::all() {
                if let Some(next) = transition(state, event) {
                    if !visited.contains(&next) {
                        frontier.push(next);
                    }
                }
            }
        }
    }

    let unreachable: Vec<ReasoningState> = ReasoningState::all()
        .iter()
        .copied()
        .filter(|s| !visited.contains(s))
        .collect();

    if unreachable.is_empty() {
        Ok(())
    } else {
        Err(unreachable)
    }
}

/// 不变量 3:无意外循环 — 唯一允许的循环是终态 → Idle → (下一轮审议)
///
/// 检查转移图中不存在除终态 → Idle 之外的循环。具体策略:
/// - 将"终态 → Idle"边暂时移除
/// - 检查剩余图是否为 DAG(有向无环图)
/// - 若剩余图有环,则存在意外循环(违反不变量)
///
/// # 返回
/// - `Ok(())`: 除终态 → Idle 外无循环
/// - `Err(cycle_path)`: 列出意外循环中的状态序列(违反不变量)
pub fn invariant_no_unexpected_cycles() -> Result<(), Vec<ReasoningState>> {
    // 使用 DFS 三色标记法检测环(与 chimera-mas INV-9 同算法,CLSR §22.3)
    // 颜色:White(未访问) / Gray(在当前 DFS 路径上) / Black(已完成)
    use std::collections::HashMap;

    #[derive(Clone, Copy, PartialEq)]
    enum Color {
        White,
        Gray,
        Black,
    }

    // 初始化所有状态为 White
    let mut color: HashMap<ReasoningState, Color> = ReasoningState::all()
        .iter()
        .map(|s| (*s, Color::White))
        .collect();

    // 用于记录环路径
    let mut cycle_path: Vec<ReasoningState> = Vec::new();
    let mut has_cycle = false;

    // DFS 递归(使用显式栈避免栈溢出,虽然 7 个状态不会溢出,但保持惯用模式)
    fn dfs_visit(
        state: ReasoningState,
        color: &mut HashMap<ReasoningState, Color>,
        path: &mut Vec<ReasoningState>,
        found_cycle: &mut bool,
    ) {
        if *found_cycle {
            return;
        }

        color.insert(state, Color::Gray);
        path.push(state);

        // 探索所有有效后继,但跳过"终态 → Idle"边(允许的循环)
        for event in ReasoningEvent::all() {
            if let Some(next) = transition(state, event) {
                // 跳过终态 → Idle(允许的唯一循环)
                let is_allowed_cycle = state.is_terminal() && next == ReasoningState::Idle;
                if is_allowed_cycle {
                    continue;
                }

                match color.get(&next).copied() {
                    Some(Color::White) => {
                        dfs_visit(next, color, path, found_cycle);
                    }
                    Some(Color::Gray) => {
                        // 发现回边 → 存在环
                        *found_cycle = true;
                        path.push(next); // 闭合环路径
                        return;
                    }
                    Some(Color::Black) | None => {
                        // 已完成或不存在,跳过
                    }
                }
            }
        }

        color.insert(state, Color::Black);
        path.pop();
    }

    for start in ReasoningState::all() {
        if color.get(&start).copied() == Some(Color::White) {
            dfs_visit(start, &mut color, &mut cycle_path, &mut has_cycle);
            if has_cycle {
                break;
            }
        }
    }

    if has_cycle {
        Err(cycle_path)
    } else {
        Ok(())
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // === 状态枚举测试 ===

    #[test]
    fn test_reasoning_state_all_returns_seven() {
        let all = ReasoningState::all();
        assert_eq!(all.len(), 7, "ReasoningState 应有 7 个变体");
    }

    #[test]
    fn test_reasoning_state_is_terminal() {
        assert!(!ReasoningState::Idle.is_terminal());
        assert!(!ReasoningState::VetoCheck.is_terminal());
        assert!(!ReasoningState::Debating.is_terminal());
        assert!(!ReasoningState::Voting.is_terminal());
        assert!(ReasoningState::Accepted.is_terminal());
        assert!(ReasoningState::Rejected.is_terminal());
        assert!(ReasoningState::Vetoed.is_terminal());
    }

    #[test]
    fn test_reasoning_state_is_non_terminal() {
        assert!(ReasoningState::Idle.is_non_terminal());
        assert!(ReasoningState::VetoCheck.is_non_terminal());
        assert!(ReasoningState::Debating.is_non_terminal());
        assert!(ReasoningState::Voting.is_non_terminal());
        assert!(!ReasoningState::Accepted.is_non_terminal());
        assert!(!ReasoningState::Rejected.is_non_terminal());
        assert!(!ReasoningState::Vetoed.is_non_terminal());
    }

    #[test]
    fn test_reasoning_state_as_str() {
        assert_eq!(ReasoningState::Idle.as_str(), "idle");
        assert_eq!(ReasoningState::VetoCheck.as_str(), "veto_check");
        assert_eq!(ReasoningState::Debating.as_str(), "debating");
        assert_eq!(ReasoningState::Voting.as_str(), "voting");
        assert_eq!(ReasoningState::Accepted.as_str(), "accepted");
        assert_eq!(ReasoningState::Rejected.as_str(), "rejected");
        assert_eq!(ReasoningState::Vetoed.as_str(), "vetoed");
    }

    #[test]
    fn test_reasoning_state_display() {
        assert_eq!(ReasoningState::Idle.to_string(), "idle");
        assert_eq!(ReasoningState::VetoCheck.to_string(), "veto_check");
    }

    // === 事件枚举测试 ===

    #[test]
    fn test_reasoning_event_all_returns_seven() {
        let all = ReasoningEvent::all();
        assert_eq!(all.len(), 7, "ReasoningEvent 应有 7 个变体");
    }

    #[test]
    fn test_reasoning_event_as_str() {
        assert_eq!(
            ReasoningEvent::ProposalSubmitted.as_str(),
            "proposal_submitted"
        );
        assert_eq!(ReasoningEvent::VetoTriggered.as_str(), "veto_triggered");
        assert_eq!(ReasoningEvent::DebateStarted.as_str(), "debate_started");
        assert_eq!(
            ReasoningEvent::OpinionsCollected.as_str(),
            "opinions_collected"
        );
        assert_eq!(
            ReasoningEvent::ConsensusReached.as_str(),
            "consensus_reached"
        );
        assert_eq!(ReasoningEvent::ConsensusFailed.as_str(), "consensus_failed");
        assert_eq!(ReasoningEvent::Reset.as_str(), "reset");
    }

    // === transition 合法转移测试 ===

    #[test]
    fn test_transition_idle_to_veto_check() {
        let next = transition(ReasoningState::Idle, ReasoningEvent::ProposalSubmitted);
        assert_eq!(next, Some(ReasoningState::VetoCheck));
    }

    #[test]
    fn test_transition_veto_check_to_vetoed() {
        let next = transition(ReasoningState::VetoCheck, ReasoningEvent::VetoTriggered);
        assert_eq!(next, Some(ReasoningState::Vetoed));
    }

    #[test]
    fn test_transition_veto_check_to_debating() {
        let next = transition(ReasoningState::VetoCheck, ReasoningEvent::DebateStarted);
        assert_eq!(next, Some(ReasoningState::Debating));
    }

    #[test]
    fn test_transition_debating_to_voting() {
        let next = transition(ReasoningState::Debating, ReasoningEvent::OpinionsCollected);
        assert_eq!(next, Some(ReasoningState::Voting));
    }

    #[test]
    fn test_transition_voting_to_accepted() {
        let next = transition(ReasoningState::Voting, ReasoningEvent::ConsensusReached);
        assert_eq!(next, Some(ReasoningState::Accepted));
    }

    #[test]
    fn test_transition_voting_to_rejected() {
        let next = transition(ReasoningState::Voting, ReasoningEvent::ConsensusFailed);
        assert_eq!(next, Some(ReasoningState::Rejected));
    }

    #[test]
    fn test_transition_terminal_to_idle_via_reset() {
        // 三个终态都应能通过 Reset 回到 Idle
        assert_eq!(
            transition(ReasoningState::Accepted, ReasoningEvent::Reset),
            Some(ReasoningState::Idle)
        );
        assert_eq!(
            transition(ReasoningState::Rejected, ReasoningEvent::Reset),
            Some(ReasoningState::Idle)
        );
        assert_eq!(
            transition(ReasoningState::Vetoed, ReasoningEvent::Reset),
            Some(ReasoningState::Idle)
        );
    }

    // === transition 非法转移测试 ===

    #[test]
    fn test_transition_idle_illegal_events_return_none() {
        // Idle 只接受 ProposalSubmitted
        assert_eq!(
            transition(ReasoningState::Idle, ReasoningEvent::VetoTriggered),
            None
        );
        assert_eq!(
            transition(ReasoningState::Idle, ReasoningEvent::DebateStarted),
            None
        );
        assert_eq!(
            transition(ReasoningState::Idle, ReasoningEvent::OpinionsCollected),
            None
        );
        assert_eq!(
            transition(ReasoningState::Idle, ReasoningEvent::ConsensusReached),
            None
        );
        assert_eq!(
            transition(ReasoningState::Idle, ReasoningEvent::ConsensusFailed),
            None
        );
        assert_eq!(
            transition(ReasoningState::Idle, ReasoningEvent::Reset),
            None
        );
    }

    #[test]
    fn test_transition_veto_check_illegal_events_return_none() {
        // VetoCheck 只接受 VetoTriggered 和 DebateStarted
        assert_eq!(
            transition(ReasoningState::VetoCheck, ReasoningEvent::ProposalSubmitted),
            None
        );
        assert_eq!(
            transition(ReasoningState::VetoCheck, ReasoningEvent::OpinionsCollected),
            None
        );
        assert_eq!(
            transition(ReasoningState::VetoCheck, ReasoningEvent::ConsensusReached),
            None
        );
        assert_eq!(
            transition(ReasoningState::VetoCheck, ReasoningEvent::ConsensusFailed),
            None
        );
        assert_eq!(
            transition(ReasoningState::VetoCheck, ReasoningEvent::Reset),
            None
        );
    }

    #[test]
    fn test_transition_terminal_illegal_events_return_none() {
        // 终态只接受 Reset
        for terminal in [
            ReasoningState::Accepted,
            ReasoningState::Rejected,
            ReasoningState::Vetoed,
        ] {
            assert_eq!(
                transition(terminal, ReasoningEvent::ProposalSubmitted),
                None
            );
            assert_eq!(transition(terminal, ReasoningEvent::VetoTriggered), None);
            assert_eq!(transition(terminal, ReasoningEvent::DebateStarted), None);
            assert_eq!(
                transition(terminal, ReasoningEvent::OpinionsCollected),
                None
            );
            assert_eq!(transition(terminal, ReasoningEvent::ConsensusReached), None);
            assert_eq!(transition(terminal, ReasoningEvent::ConsensusFailed), None);
        }
    }

    // === 完整审议流程测试(正向 + 否决路径)===

    #[test]
    fn test_full_consensus_reached_flow() {
        // 正向流程:Idle → VetoCheck → Debating → Voting → Accepted → Idle
        let s1 = transition(ReasoningState::Idle, ReasoningEvent::ProposalSubmitted);
        assert_eq!(s1, Some(ReasoningState::VetoCheck));

        let s2 = transition(ReasoningState::VetoCheck, ReasoningEvent::DebateStarted);
        assert_eq!(s2, Some(ReasoningState::Debating));

        let s3 = transition(ReasoningState::Debating, ReasoningEvent::OpinionsCollected);
        assert_eq!(s3, Some(ReasoningState::Voting));

        let s4 = transition(ReasoningState::Voting, ReasoningEvent::ConsensusReached);
        assert_eq!(s4, Some(ReasoningState::Accepted));

        let s5 = transition(ReasoningState::Accepted, ReasoningEvent::Reset);
        assert_eq!(s5, Some(ReasoningState::Idle));
    }

    #[test]
    fn test_full_rejected_flow() {
        // 拒绝流程:Idle → VetoCheck → Debating → Voting → Rejected → Idle
        let s1 = transition(ReasoningState::Idle, ReasoningEvent::ProposalSubmitted);
        assert_eq!(s1, Some(ReasoningState::VetoCheck));

        let s2 = transition(ReasoningState::VetoCheck, ReasoningEvent::DebateStarted);
        assert_eq!(s2, Some(ReasoningState::Debating));

        let s3 = transition(ReasoningState::Debating, ReasoningEvent::OpinionsCollected);
        assert_eq!(s3, Some(ReasoningState::Voting));

        let s4 = transition(ReasoningState::Voting, ReasoningEvent::ConsensusFailed);
        assert_eq!(s4, Some(ReasoningState::Rejected));

        let s5 = transition(ReasoningState::Rejected, ReasoningEvent::Reset);
        assert_eq!(s5, Some(ReasoningState::Idle));
    }

    #[test]
    fn test_full_vetoed_flow() {
        // 否决流程:Idle → VetoCheck → Vetoed → Idle
        let s1 = transition(ReasoningState::Idle, ReasoningEvent::ProposalSubmitted);
        assert_eq!(s1, Some(ReasoningState::VetoCheck));

        let s2 = transition(ReasoningState::VetoCheck, ReasoningEvent::VetoTriggered);
        assert_eq!(s2, Some(ReasoningState::Vetoed));

        let s3 = transition(ReasoningState::Vetoed, ReasoningEvent::Reset);
        assert_eq!(s3, Some(ReasoningState::Idle));
    }

    // === 不变量验证测试 ===

    #[test]
    fn test_invariant_no_deadlocks() {
        let result = invariant_no_deadlocks();
        assert!(result.is_ok(), "非终态状态不应有死锁: {:?}", result.err());
    }

    #[test]
    fn test_invariant_all_reachable() {
        let result = invariant_all_reachable();
        assert!(result.is_ok(), "从 Idle 应可达所有状态: {:?}", result.err());
    }

    #[test]
    fn test_invariant_no_unexpected_cycles() {
        let result = invariant_no_unexpected_cycles();
        assert!(result.is_ok(), "不应存在意外循环: {:?}", result.err());
    }

    // === 序列化往返测试 ===

    #[test]
    fn test_serde_roundtrip_reasoning_state() {
        for state in ReasoningState::all() {
            let json = serde_json::to_string(&state).expect("序列化失败");
            let restored: ReasoningState = serde_json::from_str(&json).expect("反序列化失败");
            assert_eq!(state, restored, "序列化往返失败: {}", state);
        }
    }

    #[test]
    fn test_serde_roundtrip_reasoning_event() {
        for event in ReasoningEvent::all() {
            let json = serde_json::to_string(&event).expect("序列化失败");
            let restored: ReasoningEvent = serde_json::from_str(&json).expect("反序列化失败");
            assert_eq!(event, restored, "序列化往返失败: {}", event);
        }
    }
}
