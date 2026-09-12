//! AutoDPO 冻结回滚守卫集成测试 — 真实 EventBus + Critical 旁路
//!
//! 对应架构层:L5 Knowledge
//!
//! # 测试目标
//! - 用真实 `EventBus::new()` 验证 `attempt_rollback_with_guard` 在回滚失败时
//!   经 Critical mpsc 旁路发布 `R2FreezeRollbackFailed` 事件;
//! - 验证该事件可由 `subscribe_critical_events()` 可靠接收(§6.2 红线旁路投递保证);
//! - 验证 `severity() == Critical` 与 metadata.source 发布方标识。
//!
//! # 设计约束(§4.4 反模式 3)
//! - 必须在发布前调用 `subscribe_critical_events()`,否则会错过广播/旁路事件。
//! - 用 `Arc::new(EventBus::new())` 共享总线,与生产调用形态一致。

#![forbid(unsafe_code)]

use std::sync::Arc;

use auto_dpo::freeze_guard::{
    attempt_rollback_with_guard, FreezeGuardError, PendingState, PendingUpdate,
};
use event_bus::{EventBus, EventSeverity, NexusEvent};

#[tokio::test]
async fn rollback_failure_publishes_r2_freeze_rollback_failed_on_real_bus() {
    // 真实 EventBus
    let bus = Arc::new(EventBus::new());
    // 先订阅 Critical 旁路(§4.4 反模式 3:先 subscribe 再发布)
    let mut rx = bus.subscribe_critical_events();
    // 构造一个处于「应用中」的 pending 更新,必然回滚失败
    let mut pending = PendingUpdate {
        update_id: "int-upd-1".to_string(),
        state: PendingState::Applying,
    };

    let result = attempt_rollback_with_guard(&bus, &mut pending);

    // 回滚失败需返回 Err(RollbackFailed)
    assert!(matches!(
        result,
        Err(FreezeGuardError::RollbackFailed { .. })
    ));

    // 真实总线 + 真实旁路:异步接收 R2FreezeRollbackFailed(事件已入队,recv 立即返回)
    let event = rx
        .recv()
        .await
        .expect("Critical 旁路必须投递 R2FreezeRollbackFailed 事件");
    assert!(
        matches!(event, NexusEvent::R2FreezeRollbackFailed { .. }),
        "接收到的应是 R2FreezeRollbackFailed 变体"
    );
    // severity 红线:必须为 Critical
    assert_eq!(
        event.severity(),
        EventSeverity::Critical,
        "R2FreezeRollbackFailed 必须为 Critical 级"
    );
    // 发布方标识
    assert_eq!(event.metadata().source, "auto-dpo:freeze_guard");
}

#[tokio::test]
async fn rollback_success_is_silent_on_real_bus() {
    let bus = Arc::new(EventBus::new());
    let mut rx = bus.subscribe_critical_events();
    let mut pending = PendingUpdate::new("int-upd-2");

    let result = attempt_rollback_with_guard(&bus, &mut pending);

    assert!(result.is_ok());
    assert_eq!(pending.state, PendingState::RolledBack);
    // 成功回滚:旁路应保持静默(直接判定没有待收事件)
    assert!(rx.try_recv().is_err(), "回滚成功不应发布任何 Critical 事件");
}
