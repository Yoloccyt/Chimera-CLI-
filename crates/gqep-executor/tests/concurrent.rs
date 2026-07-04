//! GQEP 并发测试 — 100 操作并发聚集
//!
//! 对应 SubTask 24.6:验证大规模并发聚集的稳定性
//! - 100 操作并发聚集,无 panic、无孤儿调用
//! - 混合成功/失败场景,验证结果统计正确

use std::time::Duration;

use event_bus::EventBus;
use gqep_executor::{GqepConfig, GqepError, GqepExecutor, GqepFuture};

/// 验证 100 个操作并发聚集,全部成功
#[tokio::test]
async fn test_concurrent_100_all_success() {
    let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());

    let futures: Vec<GqepFuture<String>> = (0..100)
        .map(|i| Box::pin(async move { Ok(format!("result-{i}")) }) as GqepFuture<String>)
        .collect();

    let result = executor.gather(futures).await;

    assert_eq!(result.total, 100, "total 应为 100");
    assert_eq!(result.succeeded, 100, "全部应成功");
    assert_eq!(result.failed, 0, "无失败");
    assert!(result.errors.is_empty());
    assert_eq!(executor.orphan_count(), 0, "并发聚集不应产生孤儿调用");
}

/// 验证 100 个操作并发聚集,部分失败
#[tokio::test]
async fn test_concurrent_100_partial_failure() {
    let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());

    // 每 10 个操作中第 0 个失败(共 10 个失败)
    let futures: Vec<GqepFuture<String>> = (0..100)
        .map(|i| {
            if i % 10 == 0 {
                Box::pin(async move {
                    Err(GqepError::OperationFailed {
                        operation_id: format!("op-{i}"),
                        reason: "intentional failure".to_string(),
                    })
                }) as GqepFuture<String>
            } else {
                Box::pin(async move { Ok(format!("result-{i}")) }) as GqepFuture<String>
            }
        })
        .collect();

    let result = executor.gather(futures).await;

    assert_eq!(result.total, 100);
    assert_eq!(result.succeeded, 90, "90 个应成功");
    assert_eq!(result.failed, 10, "10 个应失败");
    assert_eq!(result.errors.len(), 10);
    assert_eq!(executor.orphan_count(), 0, "不应产生孤儿");
}

/// 验证并发聚集无 panic(混合成功/失败/延迟)
#[tokio::test]
async fn test_concurrent_no_panic() {
    let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());

    let futures: Vec<GqepFuture<String>> = (0..100)
        .map(|i| {
            let delay = if i % 3 == 0 { 10 } else { 0 };
            Box::pin(async move {
                if delay > 0 {
                    tokio::time::sleep(Duration::from_millis(delay)).await;
                }
                if i % 5 == 0 {
                    Err(GqepError::OperationFailed {
                        operation_id: format!("op-{i}"),
                        reason: "mixed failure".to_string(),
                    })
                } else {
                    Ok(format!("result-{i}"))
                }
            }) as GqepFuture<String>
        })
        .collect();

    // 不应 panic
    let result = executor.gather(futures).await;

    assert_eq!(result.total, 100);
    assert_eq!(result.succeeded + result.failed, 100);
    assert_eq!(executor.orphan_count(), 0);
}

/// 验证并发聚集的流式特性:慢操作不阻塞快操作
#[tokio::test]
async fn test_concurrent_streaming_property() {
    let executor = GqepExecutor::new(
        GqepConfig {
            default_timeout_ms: 5000,
            ..Default::default()
        },
        EventBus::new(),
    );

    // 50 个快操作(0ms) + 50 个慢操作(50ms)
    let futures: Vec<GqepFuture<String>> = (0..100)
        .map(|i| {
            if i < 50 {
                Box::pin(async move { Ok(format!("fast-{i}")) }) as GqepFuture<String>
            } else {
                Box::pin(async move {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    Ok(format!("slow-{i}"))
                }) as GqepFuture<String>
            }
        })
        .collect();

    let start = std::time::Instant::now();
    let result = executor.gather(futures).await;
    let elapsed = start.elapsed();

    assert_eq!(result.succeeded, 100);
    // 流式并发:总耗时应远小于串行(50 * 50ms = 2500ms)
    assert!(
        elapsed < Duration::from_millis(500),
        "并发聚集应快于串行,实际耗时: {:?}",
        elapsed
    );
}

/// 验证大规模并发聚集的孤儿检测:正常完成无孤儿
#[tokio::test]
async fn test_concurrent_large_scale_no_orphan() {
    let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());

    let futures: Vec<GqepFuture<String>> = (0..200)
        .map(|i| {
            Box::pin(async move {
                // 模拟不同耗时的操作
                if i % 7 == 0 {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
                Ok(format!("op-{i}"))
            }) as GqepFuture<String>
        })
        .collect();

    let result = executor.gather(futures).await;

    assert_eq!(result.total, 200);
    assert_eq!(result.succeeded, 200);
    assert_eq!(executor.orphan_count(), 0, "200 操作并发不应产生孤儿");
    assert_eq!(executor.completed_count(), 200);
}

/// 性能断言测试:100 操作聚集应在合理时间内完成
#[tokio::test]
#[ignore = "性能断言测试,标记 ignore 避免在常规测试中运行"]
async fn test_perf_100_ops_within_threshold() {
    let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());

    let futures: Vec<GqepFuture<String>> = (0..100)
        .map(|i| Box::pin(async move { Ok(format!("result-{i}")) }) as GqepFuture<String>)
        .collect();

    let start = std::time::Instant::now();
    let result = executor.gather(futures).await;
    let elapsed = start.elapsed();

    assert_eq!(result.succeeded, 100);
    // 100 个即时操作聚集应在 100ms 内完成(含 QEEP entangle 开销)
    assert!(
        elapsed < Duration::from_millis(100),
        "100 操作聚集应在 100ms 内完成,实际: {:?}",
        elapsed
    );
}
