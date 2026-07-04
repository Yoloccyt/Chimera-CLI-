//! GQEP 聚集查询执行协议错误路径测试 — 验证关键故障场景的错误传播
//!
//! 对应 SubTask 29.6:为 gqep-executor 补充错误路径测试
//!
//! # 测试覆盖
//! 1. 操作超时:with_timeout 短超时 + 长运行 future → OperationTimeout
//! 2. 操作失败:future 返回 Err → OperationFailed 保留
//! 3. 批量原子性失败:第 N 个失败触发 BatchAtomicFailure + 回滚
//! 4. 孤儿调用检测:entangle_spawn + abort → OrphanCallDetected
//! 5. 空操作聚集:gather(vec![]) → total=0

#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;

use event_bus::{EventBus, NexusEvent};
use gqep_executor::{
    with_timeout, GatherResult, GqepConfig, GqepError, GqepExecutor, GqepFuture, RollbackFn,
};

// ============================================================
// 错误路径 1:操作超时
// ============================================================

/// with_timeout 短超时 + 长运行 future → OperationTimeout
///
/// WHY:对应尸检教训"5.4% 孤儿调用中,部分源于无超时控制导致永久挂起"。
/// 验证超时机制能正确中断长运行操作并返回明确错误
#[tokio::test]
async fn test_operation_timeout_triggered() {
    let bus = EventBus::new();
    let long_future: GqepFuture<String> = Box::pin(async {
        tokio::time::sleep(Duration::from_secs(10)).await;
        Ok("done".to_string())
    });

    // 50ms 超时,长运行 future(10s)必超时
    let result = with_timeout(long_future, 50, "op-timeout-test", bus).await;

    let err = result.unwrap_err();
    assert!(
        matches!(err, GqepError::OperationTimeout { ref operation_id, timeout_ms: 50 } if operation_id == "op-timeout-test"),
        "应为 OperationTimeout,实际: {err:?}"
    );
}

/// with_timeout timeout_ms=0 表示不超时,future 正常完成
///
/// WHY:0 是特殊值,表示调用方明确不需要超时(如已知快速操作)
#[tokio::test]
async fn test_timeout_zero_means_no_timeout() {
    let bus = EventBus::new();
    let future: GqepFuture<String> = Box::pin(async { Ok("done".to_string()) });

    let result = with_timeout(future, 0, "op-no-timeout", bus).await;
    assert_eq!(result.unwrap(), "done");
}

/// 超时事件通过 EventBus 发布
///
/// WHY:供 efficiency-monitor 等订阅者记录超时指标
#[tokio::test]
async fn test_timeout_publishes_event() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let long_future: GqepFuture<String> = Box::pin(async {
        tokio::time::sleep(Duration::from_secs(10)).await;
        Ok("done".to_string())
    });

    let _ = with_timeout(long_future, 50, "op-evt", bus).await;

    let event = tokio::time::timeout(Duration::from_millis(200), rx.recv())
        .await
        .expect("timeout waiting for event")
        .expect("recv failed");
    assert!(
        matches!(event, NexusEvent::OperationTimedOut { ref operation_id, timeout_ms: 50, .. } if operation_id == "op-evt"),
        "应为 OperationTimedOut 事件,实际: {event:?}"
    );
}

// ============================================================
// 错误路径 2:操作失败
// ============================================================

/// future 返回 Err → with_timeout 保留原错误(非超时错误)
///
/// WHY:超时包装不应吞掉原 future 的业务错误,需原样传递
#[tokio::test]
async fn test_operation_failure_preserved() {
    let bus = EventBus::new();
    let failing_future: GqepFuture<String> = Box::pin(async {
        Err(GqepError::OperationFailed {
            operation_id: "inner-op".into(),
            reason: "intentional failure".into(),
        })
    });

    let result = with_timeout(failing_future, 5000, "op-outer", bus).await;
    let err = result.unwrap_err();
    assert!(
        matches!(err, GqepError::OperationFailed { ref reason, .. } if reason == "intentional failure"),
        "应保留原 future 错误,实际: {err:?}"
    );
}

/// gather 中混合成功与失败,失败错误被收集到 errors 列表
#[tokio::test]
async fn test_gather_collects_failure_errors() {
    let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());

    let futures: Vec<GqepFuture<String>> = vec![
        Box::pin(async { Ok("ok-1".to_string()) }),
        Box::pin(async {
            Err(GqepError::OperationFailed {
                operation_id: "op-2".into(),
                reason: "fail-2".into(),
            })
        }),
        Box::pin(async { Ok("ok-3".to_string()) }),
        Box::pin(async {
            Err(GqepError::OperationFailed {
                operation_id: "op-4".into(),
                reason: "fail-4".into(),
            })
        }),
    ];

    let result = executor.gather(futures).await;

    assert_eq!(result.total, 4);
    assert_eq!(result.succeeded, 2);
    assert_eq!(result.failed, 2);
    assert_eq!(result.errors.len(), 2, "应收集 2 个错误");
    // 验证错误类型
    for err in &result.errors {
        assert!(
            matches!(err, GqepError::OperationFailed { .. }),
            "错误应为 OperationFailed,实际: {err:?}"
        );
    }
}

// ============================================================
// 错误路径 3:批量原子性失败
// ============================================================

/// gather_atomic 第 N 个失败触发 BatchAtomicFailure + 回滚
///
/// WHY:批量原子性要求"任一失败时,后续操作不执行"。
/// 验证失败索引正确、回滚被触发、后续操作未执行
#[tokio::test]
async fn test_gather_atomic_failure_triggers_rollback() {
    let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());
    let counter = Arc::new(AtomicU32::new(0));
    let rollback_called = Arc::new(AtomicU32::new(0));

    // 5 个操作,第 3 个(index=2)失败
    let counter_clone = counter.clone();
    let futures: Vec<GqepFuture<String>> = (0..5)
        .map(|i| {
            let c = counter_clone.clone();
            Box::pin(async move {
                c.fetch_add(1, Ordering::SeqCst);
                if i == 2 {
                    Err(GqepError::OperationFailed {
                        operation_id: String::new(),
                        reason: "intentional failure at index 2".into(),
                    })
                } else {
                    Ok("success".to_string())
                }
            }) as GqepFuture<String>
        })
        .collect();

    let rollback_called_clone = rollback_called.clone();
    let rollback: RollbackFn = Box::new(move || {
        let flag = rollback_called_clone.clone();
        Box::pin(async move {
            flag.fetch_add(1, Ordering::SeqCst);
        })
    });

    let result = executor.gather_atomic(futures, rollback).await;

    // 验证结果
    assert_eq!(result.total, 5, "total 应为传入的 5");
    assert_eq!(result.succeeded, 2, "前 2 个成功");
    assert_eq!(result.failed, 1, "第 3 个失败");
    assert_eq!(
        counter.load(Ordering::SeqCst),
        3,
        "应只执行前 3 个(2 成功 + 1 失败),后 2 个不执行"
    );
    assert_eq!(rollback_called.load(Ordering::SeqCst), 1, "应触发回滚");

    // 验证错误为 BatchAtomicFailure,failed_index=2
    assert_eq!(result.errors.len(), 1);
    assert!(
        matches!(
            &result.errors[0],
            GqepError::BatchAtomicFailure {
                failed_index: 2,
                ..
            }
        ),
        "错误应为 BatchAtomicFailure, failed_index=2, 实际: {:?}",
        result.errors[0]
    );
}

/// gather_atomic 第一个操作失败:0 成功,1 失败,触发回滚
#[tokio::test]
async fn test_gather_atomic_first_failure() {
    let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());
    let rollback_called = Arc::new(AtomicU32::new(0));

    let futures: Vec<GqepFuture<String>> = (0..5)
        .map(|i| {
            Box::pin(async move {
                if i == 0 {
                    Err(GqepError::OperationFailed {
                        operation_id: String::new(),
                        reason: "first fails".into(),
                    })
                } else {
                    Ok("success".to_string())
                }
            }) as GqepFuture<String>
        })
        .collect();

    let rollback_called_clone = rollback_called.clone();
    let rollback: RollbackFn = Box::new(move || {
        let flag = rollback_called_clone.clone();
        Box::pin(async move {
            flag.fetch_add(1, Ordering::SeqCst);
        })
    });

    let result = executor.gather_atomic(futures, rollback).await;

    assert_eq!(result.total, 5);
    assert_eq!(result.succeeded, 0);
    assert_eq!(result.failed, 1);
    assert_eq!(rollback_called.load(Ordering::SeqCst), 1, "应触发回滚");
    assert!(
        matches!(
            &result.errors[0],
            GqepError::BatchAtomicFailure {
                failed_index: 0,
                ..
            }
        ),
        "failed_index 应为 0"
    );
}

/// batch_atomic_enabled=false 时失败不触发回滚
#[tokio::test]
async fn test_gather_atomic_disabled_no_rollback() {
    let executor = GqepExecutor::new(
        GqepConfig {
            batch_atomic_enabled: false,
            ..Default::default()
        },
        EventBus::new(),
    );
    let rollback_called = Arc::new(AtomicU32::new(0));

    let futures: Vec<GqepFuture<String>> = (0..5)
        .map(|i| {
            Box::pin(async move {
                if i == 2 {
                    Err(GqepError::OperationFailed {
                        operation_id: String::new(),
                        reason: "fail".into(),
                    })
                } else {
                    Ok("ok".to_string())
                }
            }) as GqepFuture<String>
        })
        .collect();

    let rollback_called_clone = rollback_called.clone();
    let rollback: RollbackFn = Box::new(move || {
        let flag = rollback_called_clone.clone();
        Box::pin(async move {
            flag.fetch_add(1, Ordering::SeqCst);
        })
    });

    let result = executor.gather_atomic(futures, rollback).await;

    assert_eq!(result.succeeded, 2);
    assert_eq!(result.failed, 1);
    assert_eq!(
        rollback_called.load(Ordering::SeqCst),
        0,
        "batch_atomic_enabled=false 不应触发回滚"
    );
}

// ============================================================
// 错误路径 4:孤儿调用检测
// ============================================================

/// entangle_spawn + abort → 检测到孤儿调用
///
/// WHY:对应 Claude Code 尸检 5.4% 孤儿调用教训。
/// 验证 QEEP OrphanGuard 能检测到未完成的 future drop
#[tokio::test]
async fn test_orphan_detection_via_abort() {
    let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());

    // 创建长时间运行的纠缠调用,然后 abort
    let handle = executor.entangle_spawn(async {
        tokio::time::sleep(Duration::from_secs(10)).await;
        Ok("done".to_string())
    });

    // 等待任务启动(被 poll,OrphanGuard 已创建)
    tokio::time::sleep(Duration::from_millis(50)).await;

    // abort 任务,触发 future drop,OrphanGuard 检测到未完成
    handle.abort();
    tokio::time::sleep(Duration::from_millis(50)).await;

    assert!(
        executor.orphan_count() > 0,
        "abort 后应检测到孤儿调用,实际: {}",
        executor.orphan_count()
    );
}

/// 孤儿调用事件在 gather 时发布
#[tokio::test]
async fn test_orphan_event_published_on_gather() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let executor = GqepExecutor::new(GqepConfig::default(), bus);

    // 创建孤儿:entangle_spawn + abort
    let handle = executor.entangle_spawn(async {
        tokio::time::sleep(Duration::from_secs(10)).await;
        Ok("done".to_string())
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    handle.abort();
    tokio::time::sleep(Duration::from_millis(50)).await;

    assert!(executor.orphan_count() > 0, "应已检测到孤儿");

    // 调用 gather,触发孤儿事件发布
    let _ = executor
        .gather(vec![Box::pin(async { Ok("ok".to_string()) })])
        .await;

    // 接收事件:应有 OrphanCallDetected
    // WHY rx.recv() 返回 Result<NexusEvent, EventBusError>,非 Option<NexusEvent>
    let mut found_orphan = false;
    for _ in 0..5 {
        match tokio::time::timeout(Duration::from_millis(100), rx.recv()).await {
            Ok(Ok(NexusEvent::OrphanCallDetected { .. })) => {
                found_orphan = true;
                break;
            }
            Ok(Ok(_)) => continue,
            Ok(Err(_)) => break,
            Err(_) => break,
        }
    }
    assert!(found_orphan, "应发布 OrphanCallDetected 事件");
}

// ============================================================
// 错误路径 5:空操作聚集
// ============================================================

/// gather(vec![]) → total=0,不 panic
///
/// WHY:空聚集是合法操作(如初始化阶段),不应 panic
#[tokio::test]
async fn test_gather_empty_no_panic() {
    let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());
    let result: GatherResult = executor.gather(vec![]).await;

    assert_eq!(result.total, 0);
    assert_eq!(result.succeeded, 0);
    assert_eq!(result.failed, 0);
    assert!(result.errors.is_empty());
    assert!(result.is_all_success(), "空聚集应视为全成功");
}

/// gather_atomic(vec![], rollback) → 不执行任何操作,不触发回滚
#[tokio::test]
async fn test_gather_atomic_empty_no_rollback() {
    let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());
    let rollback_called = Arc::new(AtomicU32::new(0));

    let rollback_called_clone = rollback_called.clone();
    let rollback: RollbackFn = Box::new(move || {
        let flag = rollback_called_clone.clone();
        Box::pin(async move {
            flag.fetch_add(1, Ordering::SeqCst);
        })
    });

    let result = executor.gather_atomic(vec![], rollback).await;

    assert_eq!(result.total, 0);
    assert_eq!(result.succeeded, 0);
    assert_eq!(result.failed, 0);
    assert_eq!(
        rollback_called.load(Ordering::SeqCst),
        0,
        "空操作不应触发回滚"
    );
}
