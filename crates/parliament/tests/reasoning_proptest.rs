//! ReasoningState proptest — 三性质验证(无死锁 / 全可达 / 无意外循环)
//!
//! 对应文档:NEXUS-OMEGA_v5.0_系统性完整设计文档.md §9.3
//! 对齐规格:INV-7/8/9 的 1000 次 proptest 先例(chimera-mas invariants.rs)
//!
//! # 三性质
//! 1. **无死锁**:所有非终态状态至少有一个有效后继事件
//! 2. **全可达**:从 Idle 出发,BFS 可达全部 7 个状态
//! 3. **无意外循环**:唯一允许的循环是终态 → Idle → (下一轮审议)
//!
//! # 语法约束(§4.1 规范)
//! proptest 1.11+ 用 block-named 语法:`fn name(arg in strategy) { body }`
//! 禁止 closure 形式(某些 pattern 解析失败)
//!
//! # 零参数测试说明
//! proptest! 宏要求至少一个参数(`$($parm:pat in $strategy:expr),+`),
//! 因此纯不变量函数(无随机输入)用普通 `#[test]` 而非 `proptest!` 块

#![forbid(unsafe_code)]

use parliament::reasoning::{
    invariant_all_reachable, invariant_no_deadlocks, invariant_no_unexpected_cycles, transition,
    ReasoningEvent, ReasoningState,
};
use proptest::prelude::*;

// ============================================================
// 策略生成器
// ============================================================

/// 生成任意 ReasoningState 的策略
fn any_state() -> impl Strategy<Value = ReasoningState> {
    prop_oneof![
        Just(ReasoningState::Idle),
        Just(ReasoningState::VetoCheck),
        Just(ReasoningState::Debating),
        Just(ReasoningState::Voting),
        Just(ReasoningState::Accepted),
        Just(ReasoningState::Rejected),
        Just(ReasoningState::Vetoed),
    ]
}

/// 生成任意 ReasoningEvent 的策略
fn any_event() -> impl Strategy<Value = ReasoningEvent> {
    prop_oneof![
        Just(ReasoningEvent::ProposalSubmitted),
        Just(ReasoningEvent::VetoTriggered),
        Just(ReasoningEvent::DebateStarted),
        Just(ReasoningEvent::OpinionsCollected),
        Just(ReasoningEvent::ConsensusReached),
        Just(ReasoningEvent::ConsensusFailed),
        Just(ReasoningEvent::Reset),
    ]
}

/// 生成事件序列的策略(长度 1-100)
fn event_sequence() -> impl Strategy<Value = Vec<ReasoningEvent>> {
    prop::collection::vec(any_event(), 1..100)
}

/// 生成任意终态 ReasoningState 的策略
/// WHY 专属策略:终态只占 7 个状态中的 3 个(Accepted/Rejected/Vetoed),
/// 用 `any_state() + prop_assume!(is_terminal)` 会拒绝 4/7 case 触发"Too many global rejects"
fn any_terminal_state() -> impl Strategy<Value = ReasoningState> {
    prop_oneof![
        Just(ReasoningState::Accepted),
        Just(ReasoningState::Rejected),
        Just(ReasoningState::Vetoed),
    ]
}

// ============================================================
// 零参数不变量测试 — 普通 #[test]
// proptest! 宏要求至少一个参数,这些纯函数不变量检查用普通测试
// ============================================================

#[test]
fn invariant_no_deadlocks_always_ok() {
    let result = invariant_no_deadlocks();
    assert!(
        result.is_ok(),
        "invariant_no_deadlocks 失败: {:?}",
        result.err()
    );
}

#[test]
fn invariant_all_reachable_always_ok() {
    let result = invariant_all_reachable();
    assert!(
        result.is_ok(),
        "invariant_all_reachable 失败: {:?}",
        result.err()
    );
}

#[test]
fn invariant_no_unexpected_cycles_always_ok() {
    let result = invariant_no_unexpected_cycles();
    assert!(
        result.is_ok(),
        "invariant_no_unexpected_cycles 失败: {:?}",
        result.err()
    );
}

// ============================================================
// 性质 1:无死锁 — 所有非终态状态至少有一个有效后继
// ============================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    /// P1.1:对任意非终态状态,至少存在一个事件使 transition 返回 Some(_)
    #[test]
    fn proptest_no_deadlock_non_terminal_states(state in any_state()) {
        prop_assume!(state.is_non_terminal());
        let has_valid_successor = ReasoningEvent::all()
            .iter()
            .any(|e| transition(state, *e).is_some());
        prop_assert!(has_valid_successor, "非终态状态无有效后继事件: {:?}", state);
    }

    /// P1.2:对任意 (state, event) 组合,transition 要么返回 None(非法),
    /// 要么返回 Some(next) 且 next ≠ state(无自环,除终态 → Idle 外)
    #[test]
    fn proptest_no_self_loop_except_terminal_reset(
        state in any_state(),
        event in any_event()
    ) {
        if let Some(next) = transition(state, event) {
            // 终态 → Idle 是允许的"重置"(非自环)
            let is_allowed_reset = state.is_terminal() && next == ReasoningState::Idle;
            if !is_allowed_reset {
                prop_assert_ne!(next, state);
            }
        }
    }
}

// ============================================================
// 性质 2:全可达 — 从 Idle 出发 BFS 可达所有状态
// ============================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    /// P2.1:从 Idle 出发,应用随机事件序列,任何可达状态都应在 all() 列表中
    /// (验证不产生"幽灵状态")
    #[test]
    fn proptest_no_ghost_state_from_idle(events in event_sequence()) {
        let mut current = ReasoningState::Idle;
        for event in &events {
            if let Some(next) = transition(current, *event) {
                prop_assert!(
                    ReasoningState::all().contains(&next),
                    "事件从 {:?} 转移到幽灵状态 {:?}",
                    current, next
                );
                current = next;
            }
            // transition 返回 None 是合法的(非法转移被拒绝),不更新 current
        }
    }

    /// P2.2:正向完整流程可达 Accepted
    /// Idle → VetoCheck → Debating → Voting → Accepted
    #[test]
    fn proptest_consensus_reached_flow_reachable(_seed in 0u32..1) {
        let s1 = transition(ReasoningState::Idle, ReasoningEvent::ProposalSubmitted);
        prop_assert_eq!(s1, Some(ReasoningState::VetoCheck));

        let s2 = transition(ReasoningState::VetoCheck, ReasoningEvent::DebateStarted);
        prop_assert_eq!(s2, Some(ReasoningState::Debating));

        let s3 = transition(ReasoningState::Debating, ReasoningEvent::OpinionsCollected);
        prop_assert_eq!(s3, Some(ReasoningState::Voting));

        let s4 = transition(ReasoningState::Voting, ReasoningEvent::ConsensusReached);
        prop_assert_eq!(s4, Some(ReasoningState::Accepted));
    }

    /// P2.3:否决流程可达 Vetoed
    #[test]
    fn proptest_vetoed_flow_reachable(_seed in 0u32..1) {
        let s1 = transition(ReasoningState::Idle, ReasoningEvent::ProposalSubmitted);
        prop_assert_eq!(s1, Some(ReasoningState::VetoCheck));

        let s2 = transition(ReasoningState::VetoCheck, ReasoningEvent::VetoTriggered);
        prop_assert_eq!(s2, Some(ReasoningState::Vetoed));
    }

    /// P2.4:拒绝流程可达 Rejected
    #[test]
    fn proptest_rejected_flow_reachable(_seed in 0u32..1) {
        let s1 = transition(ReasoningState::Idle, ReasoningEvent::ProposalSubmitted);
        prop_assert_eq!(s1, Some(ReasoningState::VetoCheck));

        let s2 = transition(ReasoningState::VetoCheck, ReasoningEvent::DebateStarted);
        prop_assert_eq!(s2, Some(ReasoningState::Debating));

        let s3 = transition(ReasoningState::Debating, ReasoningEvent::OpinionsCollected);
        prop_assert_eq!(s3, Some(ReasoningState::Voting));

        let s4 = transition(ReasoningState::Voting, ReasoningEvent::ConsensusFailed);
        prop_assert_eq!(s4, Some(ReasoningState::Rejected));
    }
}

// ============================================================
// 性质 3:无意外循环 — 唯一允许的循环是终态 → Idle
// ============================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(1000))]

    /// P3.1:从 Idle 出发,应用任意事件序列,若回到 Idle,则必经终态
    /// (验证唯一允许的循环路径:Idle → ... → 终态 → Idle)
    #[test]
    fn proptest_return_to_idle_only_via_terminal(events in event_sequence()) {
        let mut current = ReasoningState::Idle;
        let mut visited_terminal = false;

        for event in &events {
            if let Some(next) = transition(current, *event) {
                // 检测回到 Idle
                if next == ReasoningState::Idle && current != ReasoningState::Idle {
                    prop_assert!(
                        visited_terminal,
                        "从 {:?} 回到 Idle 但未经过终态(意外循环)",
                        current
                    );
                    // 重置标志(允许下一轮审议)
                    visited_terminal = false;
                }
                // 标记经过终态
                if next.is_terminal() {
                    visited_terminal = true;
                }
                current = next;
            }
        }
    }

    /// P3.2:终态只能通过 Reset 事件回到 Idle,不能通过其他事件"逃逸"
    #[test]
    fn proptest_terminal_only_escape_via_reset(
        state in any_terminal_state(),
        event in any_event()
    ) {
        let result = transition(state, event);

        if result.is_some() {
            // 终态的唯一合法转移是 → Idle(通过 Reset)
            prop_assert_eq!(result, Some(ReasoningState::Idle));
            prop_assert_eq!(event, ReasoningEvent::Reset);
        }
    }
}
