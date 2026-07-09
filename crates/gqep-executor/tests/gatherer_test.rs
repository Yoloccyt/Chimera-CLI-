//! GQEP gather 全局超时治理测试 — Phase V Task V-3 [N14]
//!
//! 对应架构红线:大规模 gather 时单操作超时累积可能导致整体执行时间过长,
//! 需用全局 gather_deadline_ms 包裹整个 stream.next() 循环,杜绝整体失控。
//!
//! # 双层超时设计
//! - 单操作超时:entangle 内部 tokio::time::timeout(保护单个 future)
//! - 全局超时:gather 用 tokio::time::timeout 包裹整个收集循环
//!   (保护整个 gather 流程,防止单操作超时累积导致整体失控)
//!
//! # TDD-RED
//! 本文件先于实现编写,预期在 GqepConfig.gather_deadline_ms /
//! GqepError::GlobalTimedOut / NexusEvent::GatherTimedOut 实现前编译失败。

#![forbid(unsafe_code)]

use std::time::{Duration, Instant};

use event_bus::{EventBus, NexusEvent};
use gqep_executor::{GqepConfig, GqepError, GqepExecutor, GqepFuture};

// ============================================================
// 测试 1:全局超时触发,限制总耗时
// ============================================================

/// 全局超时触发:多个慢操作 + 短 deadline → 整个 gather 在 deadline 内返回 GlobalTimedOut
///
/// WHY 修正原 spec:原 spec "5 个 sleep(2s) + deadline=3000" 因 FuturesUnordered
/// 并发执行,5 个 2s future 会在 ~2s 内全部完成(2s < 3s),无法触发全局超时。
/// 此处改用足够慢的 future(各 sleep 5s)+ 短 deadline(300ms),确保单操作远未
/// 完成时全局超时先触发;同时 default_timeout_ms 设大,确保全局超时先于单操作超时。
#[tokio::test]
async fn test_gather_global_deadline_limits_total_duration() {
    let config = GqepConfig {
        default_timeout_ms: 30_000, // 单操作超时设大,确保全局超时先触发
        gather_deadline_ms: 300,     // 全局 deadline 300ms
        ..Default::default()
    };
    let executor = GqepExecutor::new(config, EventBus::new());

    // 5 个各 sleep 5s 的 future(并发执行,但每个都远超 300ms deadline)
    let futures: Vec<GqepFuture<String>> = (0..5)
        .map(|_| {
            Box::pin(async {
                tokio::time::sleep(Duration::from_secs(5)).await;
                Ok("done".to_string())
            }) as GqepFuture<String>
        })
        .collect();

    let start = Instant::now();
    let result = executor.gather(futures).await;
    let elapsed = start.elapsed();

    // 全局超时应在 ~300ms 触发(允许调度抖动,2s 余量)
    assert!(
        elapsed < Duration::from_secs(2),
        "全局超时应快速返回,实际耗时: {:?}",
        elapsed
    );
    // 没有任何 future 在 deadline 内完成
    assert_eq!(result.succeeded, 0, "deadline 内不应有操作成功");
    // 应包含 GlobalTimedOut 错误(区分于单操作超时 OperationTimeout)
    assert!(
        result
            .errors
            .iter()
            .any(|e| matches!(e, GqepError::GlobalTimedOut { deadline_ms: 300, .. })),
        "应返回 GlobalTimedOut 错误(deadline_ms=300),实际 errors: {:?}",
        result.errors
    );
}

// ============================================================
// 测试 2:全局超时未触发,正常完成
// ============================================================

/// 全局超时未触发:快速 future + 宽松 deadline → 正常完成,无 GlobalTimedOut
#[tokio::test]
async fn test_gather_global_deadline_not_triggered() {
    let config = GqepConfig {
        gather_deadline_ms: 5_000,
        ..Default::default()
    };
    let executor = GqepExecutor::new(config, EventBus::new());

    let futures: Vec<GqepFuture<String>> = (0..3)
        .map(|i| Box::pin(async move { Ok(format!("ok-{i}")) }) as GqepFuture<String>)
        .collect();

    let result = executor.gather(futures).await;

    assert_eq!(result.total, 3);
    assert_eq!(result.succeeded, 3, "全部应成功");
    assert_eq!(result.failed, 0);
    assert!(
        !result
            .errors
            .iter()
            .any(|e| matches!(e, GqepError::GlobalTimedOut { .. })),
        "正常完成不应产生 GlobalTimedOut 错误"
    );
}

// ============================================================
// 测试 3:gather_deadline_ms=0 禁用全局超时(向后兼容)
// ============================================================

/// gather_deadline_ms=0 表示禁用全局超时,行为与无全局超时一致
///
/// WHY 0=禁用:与 with_timeout 的 timeout_ms=0 语义保持一致(单操作超时同样以 0 禁用),
/// 且不破坏既有 API(默认值非 0 时才生效)。验证:慢操作(各 200ms)在 deadline=0 下
/// 能全部完成,不会被全局超时打断。
#[tokio::test]
async fn test_gather_global_deadline_zero_disables() {
    let config = GqepConfig {
        default_timeout_ms: 30_000, // 单操作超时设大,排除单操作超时干扰
        gather_deadline_ms: 0,      // 0 = 禁用全局超时
        ..Default::default()
    };
    let executor = GqepExecutor::new(config, EventBus::new());

    // 3 个各 sleep 200ms 的 future;若全局超时启用(如 100ms)会被打断,
    // 但 deadline=0 禁用,应全部完成
    let futures: Vec<GqepFuture<String>> = (0..3)
        .map(|i| {
            Box::pin(async {
                tokio::time::sleep(Duration::from_millis(200)).await;
                Ok(format!("ok-{i}"))
            }) as GqepFuture<String>
        })
        .collect();

    let start = Instant::now();
    let result = executor.gather(futures).await;
    let elapsed = start.elapsed();

    assert_eq!(result.succeeded, 3, "deadline=0 应禁用全局超时,全部完成");
    assert_eq!(result.failed, 0);
    assert!(
        !result
            .errors
            .iter()
            .any(|e| matches!(e, GqepError::GlobalTimedOut { .. })),
        "deadline=0 不应产生 GlobalTimedOut 错误"
    );
    // 应完整跑完 ~200ms(并发),未被全局超时打断
    assert!(
        elapsed >= Duration::from_millis(150),
        "deadline=0 不应提前打断,实际耗时: {:?}",
        elapsed
    );
}

// ============================================================
// 测试 4:全局超时触发时发布 GatherTimedOut 事件
// ============================================================

/// 全局超时触发时通过 EventBus 发布 GatherTimedOut 事件
///
/// WHY 发布事件:供 efficiency-monitor 等订阅者记录全局超时指标,
/// 对应架构红线"所有异步操作必须有 GQEP 聚集/超时处理"的可观测性要求。
#[tokio::test]
async fn test_gather_global_deadline_publishes_event() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let config = GqepConfig {
        default_timeout_ms: 30_000,
        gather_deadline_ms: 200,
        ..Default::default()
    };
    let executor = GqepExecutor::new(config, bus);

    // 1 个 sleep 5s 的 future,200ms deadline 必触发全局超时
    let futures: Vec<GqepFuture<String>> = vec![Box::pin(async {
        tokio::time::sleep(Duration::from_secs(5)).await;
        Ok("done".to_string())
    })];

    let _ = executor.gather(futures).await;

    // 应收到 GatherTimedOut 事件
    let mut found = false;
    for _ in 0..6 {
        match tokio::time::timeout(Duration::from_millis(150), rx.recv()).await {
            Ok(Ok(NexusEvent::GatherTimedOut { deadline_ms, .. })) => {
                assert_eq!(deadline_ms, 200, "事件应携带 deadline_ms=200");
                found = true;
                break;
            }
            Ok(Ok(_)) => continue,
            Ok(Err(_)) | Err(_) => break,
        }
    }
    assert!(found, "应发布 GatherTimedOut 事件");
}
