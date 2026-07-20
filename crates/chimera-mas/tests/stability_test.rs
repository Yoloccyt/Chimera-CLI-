#![forbid(unsafe_code)]

//! Task 19(RED): 系统稳定性守护测试 — §19 系统稳定运行与功能完整闭环
//!
//! 对应设计文档 §19 + §6.1(零孤儿)+ INV-3/INV-4(故障隔离)。
//! 覆盖 4 类场景:
//! 1. SubTask 19.3 — 零孤儿终态(StabilityGuard::ensure_terminal_state)
//! 2. SubTask 19.4 — 故障隔离(StabilityGuard::isolate_failure)
//! 3. SubTask 19.5 — CircuitBreaker 三态(Closed/Open/HalfOpen)
//! 4. SubTask 19.6 — 降级链(DegradationChain::apply)
//!
//! TDD RED 阶段:本文件引用尚未实现的 API(`CircuitBreaker::new` /
//! `StabilityGuard::ensure_terminal_state` / `DegradationChain::apply` 等),
//! 编译失败为预期。GREEN 阶段实现 `src/stability.rs` 的 `impl` 块后全部通过。
//!
//! ## 红线对齐
//!
//! - §4.1: proptest 1.11+ block-named 语法(`fn name(x in 0..100u32) { body }`)
//! - §4.4 反模式 1: AtomicU8 CAS,不持锁跨 `.await`
//! - §6.1: 零孤儿终态(每个任务必须发布 Completed 或 Failed)
//! - §6.2: Critical 安全事件用 mpsc(`AgentTaskFailed`)
//! - `#![forbid(unsafe_code)]`: 纯测试,无 unsafe 需求

use chimera_mas::stability::{
    CircuitBreaker, DegradationChain, DegradationStep, PressureSource, StabilityGuard,
    TerminalState, STATE_CLOSED, STATE_HALF_OPEN, STATE_OPEN,
};
use chimera_mas::MasError;
use proptest::prelude::*;

// ============================================================
// SubTask 19.3 — 零孤儿终态测试
// ============================================================
//
// 语义(§6.1 红线 + §19):
// 每个任务必须以 `AgentTaskCompleted` 或 `AgentTaskFailed` 之一结束,
// 不允许"孤儿任务"(既未完成也未失败的悬挂态)。
// `StabilityGuard::ensure_terminal_state(task_id)` 校验任务终态已注册。

/// 单任务注册 Completed 后,ensure_terminal_state 应返回 Ok
#[test]
fn ensure_terminal_state_ok_after_completed_registered() {
    let guard = StabilityGuard::new();
    guard.record_terminal("task-1".into(), TerminalState::Completed);
    let result = guard.ensure_terminal_state("task-1");
    assert!(result.is_ok(), "注册 Completed 后应通过零孤儿校验");
}

/// 单任务注册 Failed 后,ensure_terminal_state 应返回 Ok
#[test]
fn ensure_terminal_state_ok_after_failed_registered() {
    let guard = StabilityGuard::new();
    guard.record_terminal("task-2".into(), TerminalState::Failed);
    let result = guard.ensure_terminal_state("task-2");
    assert!(result.is_ok(), "注册 Failed 后应通过零孤儿校验");
}

/// 未注册终态的任务应返回 Err(孤儿任务)
#[test]
fn ensure_terminal_state_err_for_orphan_task() {
    let guard = StabilityGuard::new();
    let result = guard.ensure_terminal_state("orphan-task");
    assert!(
        result.is_err(),
        "未注册终态的任务应返回 Err(孤儿任务违反 §6.1)"
    );
    let err = result.unwrap_err();
    assert!(
        matches!(err, MasError::Internal(_)),
        "孤儿任务应返回 MasError::Internal,实际: {err:?}"
    );
}

/// 重复注册同一 task_id 以最后一次为准(覆盖语义)
#[test]
fn record_terminal_overwrites_previous_state() {
    let guard = StabilityGuard::new();
    guard.record_terminal("task-3".into(), TerminalState::Completed);
    guard.record_terminal("task-3".into(), TerminalState::Failed);
    // 覆盖后仍应通过零孤儿校验
    let result = guard.ensure_terminal_state("task-3");
    assert!(result.is_ok(), "覆盖注册后仍应通过零孤儿校验");
    assert_eq!(guard.terminal_count(), 1, "覆盖注册后任务计数应为 1");
}

/// proptest: 1000 次随机任务,每个注册终态后都应通过零孤儿校验
///
/// §4.1 block-named 语法:`fn name(id in strategy) { body }`
#[test]
fn proptest_zero_orphan_terminal_state_1000_tasks() {
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1000))]
        fn test_random_task_always_has_terminal_state(idx in 0u32..1000) {
            let guard = StabilityGuard::new();
            let task_id = format!("proptest-task-{idx}");
            // 随机选择终态(偶数→Completed,奇数→Failed)
            let state = if idx % 2 == 0 {
                TerminalState::Completed
            } else {
                TerminalState::Failed
            };
            guard.record_terminal(task_id.clone(), state);
            // 注册后必须通过零孤儿校验
            prop_assert!(
                guard.ensure_terminal_state(&task_id).is_ok(),
                "任务 {} 注册终态 {:?} 后应通过零孤儿校验",
                task_id,
                state
            );
        }
    }
}

/// proptest: 未注册的随机任务 ID 必须返回 Err
#[test]
fn proptest_unregistered_task_returns_err() {
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(1000))]
        fn test_unregistered_task_id_returns_err(idx in 0u32..1000) {
            let guard = StabilityGuard::new();
            let task_id = format!("unregistered-{idx}");
            let result = guard.ensure_terminal_state(&task_id);
            prop_assert!(
                result.is_err(),
                "未注册的任务 {} 必须返回 Err(孤儿任务)",
                task_id
            );
        }
    }
}

// ============================================================
// SubTask 19.4 — 故障隔离测试(INV-3/INV-4 验证)
// ============================================================
//
// 语义(§19):
// 某象限孙代理崩溃只影响其子树,不级联到其他象限。
// `StabilityGuard::isolate_failure(subtree_id)` 标记子树为隔离态。

/// 隔离某子树后,该子树标记为已隔离
#[test]
fn isolate_failure_marks_subtree_isolated() {
    let guard = StabilityGuard::new();
    let result = guard.isolate_failure("subtree-Q1-crash".into());
    assert!(result.is_ok(), "isolate_failure 应返回 Ok");
    assert!(
        guard.is_isolated("subtree-Q1-crash"),
        "隔离后 is_isolated 应返回 true"
    );
}

/// 隔离某子树不影响其他象限(关键 INV-3/INV-4 验证)
#[test]
fn isolate_failure_does_not_cascade_to_other_quadrants() {
    let guard = StabilityGuard::new();
    // 隔离 Q1 象限的子树
    guard.isolate_failure("subtree-Q1-crash".into()).unwrap();

    // 其他象限(Q2/Q3/Q4)不应被隔离
    assert!(
        !guard.is_isolated("subtree-Q2"),
        "Q1 隔离不应级联到 Q2(INV-3/INV-4 故障隔离)"
    );
    assert!(
        !guard.is_isolated("subtree-Q3"),
        "Q1 隔离不应级联到 Q3(INV-3/INV-4 故障隔离)"
    );
    assert!(
        !guard.is_isolated("subtree-Q4"),
        "Q1 隔离不应级联到 Q4(INV-3/INV-4 故障隔离)"
    );
}

/// 未隔离的子树 is_isolated 返回 false
#[test]
fn is_isolated_returns_false_for_unisolated_subtree() {
    let guard = StabilityGuard::new();
    assert!(
        !guard.is_isolated("subtree-never-isolated"),
        "未调用 isolate_failure 的子树应返回 false"
    );
}

/// 重复隔离同一子树应幂等(返回 Ok,is_isolated 仍为 true)
#[test]
fn isolate_failure_is_idempotent() {
    let guard = StabilityGuard::new();
    guard.isolate_failure("subtree-dup".into()).unwrap();
    let second = guard.isolate_failure("subtree-dup".into());
    assert!(second.is_ok(), "重复隔离应幂等返回 Ok");
    assert!(guard.is_isolated("subtree-dup"));
    assert_eq!(guard.isolated_count(), 1, "重复隔离计数应仍为 1");
}

/// 隔离多个子树后,isolated_count 应反映正确数量
#[test]
fn isolated_count_reflects_multiple_isolations() {
    let guard = StabilityGuard::new();
    guard.isolate_failure("subtree-A".into()).unwrap();
    guard.isolate_failure("subtree-B".into()).unwrap();
    guard.isolate_failure("subtree-C".into()).unwrap();
    assert_eq!(guard.isolated_count(), 3, "隔离 3 个子树后计数应为 3");
}

// ============================================================
// SubTask 19.5 — CircuitBreaker 三态测试
// ============================================================
//
// 三态机:Closed(0) → Open(1) → HalfOpen(2) → Closed(0)
// 基于 AtomicU8 CAS(§4.4 反模式 1,不持锁跨 .await)

/// 新建的 CircuitBreaker 应处于 Closed 状态
#[test]
fn circuit_breaker_new_is_closed() {
    let cb = CircuitBreaker::new(3, 5000);
    assert_eq!(cb.state(), STATE_CLOSED, "新建断路器应为 Closed(0)");
    assert!(!cb.is_open(), "Closed 状态 is_open 应为 false");
}

/// trip_open() 将状态从 Closed 转换为 Open
#[test]
fn circuit_breaker_trip_open_transitions_closed_to_open() {
    let cb = CircuitBreaker::new(3, 5000);
    cb.trip_open();
    assert_eq!(cb.state(), STATE_OPEN, "trip_open 后状态应为 Open(1)");
    assert!(cb.is_open(), "Open 状态 is_open 应为 true");
}

/// try_half_open() 将状态从 Open 转换为 HalfOpen
#[test]
fn circuit_breaker_try_half_open_transitions_open_to_half_open() {
    let cb = CircuitBreaker::new(3, 5000);
    cb.trip_open();
    let transitioned = cb.try_half_open();
    assert!(transitioned, "Open→HalfOpen 转换应返回 true");
    assert_eq!(
        cb.state(),
        STATE_HALF_OPEN,
        "try_half_open 后状态应为 HalfOpen(2)"
    );
}

/// reset() 将状态从 HalfOpen 转换为 Closed
#[test]
fn circuit_breaker_reset_transitions_half_open_to_closed() {
    let cb = CircuitBreaker::new(3, 5000);
    cb.trip_open();
    cb.try_half_open();
    cb.reset();
    assert_eq!(cb.state(), STATE_CLOSED, "reset 后状态应为 Closed(0)");
    assert!(!cb.is_open(), "Closed 状态 is_open 应为 false");
}

/// try_half_open() 在非 Open 状态应返回 false(不转换)
#[test]
fn circuit_breaker_try_half_open_returns_false_when_not_open() {
    let cb = CircuitBreaker::new(3, 5000);
    // 初始 Closed 状态调用 try_half_open 应返回 false
    let transitioned = cb.try_half_open();
    assert!(
        !transitioned,
        "Closed 状态调用 try_half_open 应返回 false(不转换)"
    );
    assert_eq!(cb.state(), STATE_CLOSED, "状态应保持 Closed(0)");
}

/// record_failure() 累加失败计数,达到阈值时触发 trip_open
#[test]
fn circuit_breaker_record_failure_triggers_open_at_threshold() {
    let cb = CircuitBreaker::new(3, 5000);
    // 前 2 次失败不应触发 Open
    assert!(
        !cb.record_failure(),
        "第 1 次失败不应触发 Open(threshold=3)"
    );
    assert!(
        !cb.record_failure(),
        "第 2 次失败不应触发 Open(threshold=3)"
    );
    assert_eq!(cb.state(), STATE_CLOSED, "前 2 次失败后状态应保持 Closed");

    // 第 3 次失败应触发 Open
    assert!(
        cb.record_failure(),
        "第 3 次失败应触发 Open(达到 threshold=3)"
    );
    assert_eq!(cb.state(), STATE_OPEN, "达到阈值后状态应为 Open(1)");
    assert!(cb.is_open());
}

/// reset() 重置失败计数,允许新一轮失败计数
#[test]
fn circuit_breaker_reset_clears_failure_count() {
    let cb = CircuitBreaker::new(2, 5000);
    // 第 2 次失败触发 Open
    cb.record_failure();
    assert!(cb.record_failure(), "第 2 次失败应触发 Open");
    assert!(cb.is_open());

    // reset 后失败计数清零
    cb.reset();
    assert_eq!(cb.state(), STATE_CLOSED);

    // reset 后需要再次累计 2 次失败才触发 Open
    assert!(!cb.record_failure(), "reset 后第 1 次失败不应触发 Open");
    assert!(cb.record_failure(), "reset 后第 2 次失败应触发 Open");
}

/// 状态机完整循环:Closed → Open → HalfOpen → Closed
#[test]
fn circuit_breaker_full_state_machine_cycle() {
    let cb = CircuitBreaker::new(1, 100);

    // Closed → Open(threshold=1,1 次失败即触发)
    assert!(cb.record_failure(), "threshold=1 时 1 次失败即触发 Open");
    assert_eq!(cb.state(), STATE_OPEN);

    // Open → HalfOpen
    assert!(cb.try_half_open(), "Open→HalfOpen 应成功");
    assert_eq!(cb.state(), STATE_HALF_OPEN);

    // HalfOpen → Closed(探测成功)
    cb.reset();
    assert_eq!(cb.state(), STATE_CLOSED);
    assert!(!cb.is_open(), "完整循环后应回到 Closed");
}

/// proptest: 任意阈值 ≥ 1,失败次数达阈值时必触发 Open
#[test]
fn proptest_circuit_breaker_threshold_always_triggers_open() {
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(256))]
        fn test_threshold_triggers_open(threshold in 1u32..100) {
            let cb = CircuitBreaker::new(threshold, 5000);
            // 累计 threshold-1 次失败,不应触发
            for _ in 0..(threshold - 1) {
                prop_assert!(!cb.record_failure(), "未达阈值不应触发 Open");
            }
            prop_assert_eq!(cb.state(), STATE_CLOSED);
            // 第 threshold 次失败应触发
            prop_assert!(cb.record_failure(), "达到阈值 {} 应触发 Open", threshold);
            prop_assert_eq!(cb.state(), STATE_OPEN);
        }
    }
}

// ============================================================
// SubTask 19.6 — 降级链测试
// ============================================================
//
// 降级链顺序(§19):
// - MemoryNearBudget → [HcwCompress, TierDemote, RejectNewAgent]
// - ExpertOverload → [FallbackToLocalMlc, FallbackToWiki]
// - ArchiveIoContention → [DeferArchiveToLowPeak]

/// MemoryNearBudget 降级链应返回 [HcwCompress, TierDemote, RejectNewAgent]
#[test]
fn degradation_chain_memory_near_budget_returns_correct_sequence() {
    let steps = DegradationChain::apply(PressureSource::MemoryNearBudget);
    assert_eq!(
        steps,
        vec![
            DegradationStep::HcwCompress,
            DegradationStep::TierDemote,
            DegradationStep::RejectNewAgent,
        ],
        "MemoryNearBudget 降级链顺序应为 [HcwCompress, TierDemote, RejectNewAgent]"
    );
}

/// ExpertOverload 降级链应返回 [FallbackToLocalMlc, FallbackToWiki]
#[test]
fn degradation_chain_expert_overload_returns_correct_sequence() {
    let steps = DegradationChain::apply(PressureSource::ExpertOverload);
    assert_eq!(
        steps,
        vec![
            DegradationStep::FallbackToLocalMlc,
            DegradationStep::FallbackToWiki,
        ],
        "ExpertOverload 降级链顺序应为 [FallbackToLocalMlc, FallbackToWiki]"
    );
}

/// ArchiveIoContention 降级链应返回 [DeferArchiveToLowPeak]
#[test]
fn degradation_chain_archive_io_contention_returns_correct_sequence() {
    let steps = DegradationChain::apply(PressureSource::ArchiveIoContention);
    assert_eq!(
        steps,
        vec![DegradationStep::DeferArchiveToLowPeak],
        "ArchiveIoContention 降级链应为 [DeferArchiveToLowPeak]"
    );
}

/// 降级链步骤顺序不可颠倒(HcwCompress 必须在 TierDemote 之前)
#[test]
fn degradation_chain_memory_steps_order_matters() {
    let steps = DegradationChain::apply(PressureSource::MemoryNearBudget);
    assert!(!steps.is_empty(), "降级链不应为空");
    // 第一步必须是 HcwCompress(先压缩,再降级,最后拒新派生)
    assert_eq!(
        steps[0],
        DegradationStep::HcwCompress,
        "第一步必须是 HcwCompress(先压缩再降级)"
    );
    // 最后一步必须是 RejectNewAgent(最后才拒绝新派生)
    assert_eq!(
        steps[steps.len() - 1],
        DegradationStep::RejectNewAgent,
        "最后一步必须是 RejectNewAgent(最后才拒新派生)"
    );
}

/// proptest: 任意 PressureSource 都应返回非空降级步骤序列
#[test]
fn proptest_degradation_chain_always_returns_non_empty() {
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(100))]
        fn test_apply_always_non_empty(source_idx in 0u8..3) {
            let source = match source_idx {
                0 => PressureSource::MemoryNearBudget,
                1 => PressureSource::ExpertOverload,
                _ => PressureSource::ArchiveIoContention,
            };
            let steps = DegradationChain::apply(source);
            prop_assert!(!steps.is_empty(), "降级链不应为空: source={:?}", source);
        }
    }
}
