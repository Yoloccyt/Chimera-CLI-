//! SubTask 15.11:高并发路由测试
//!
//! 验证:
//! - 100 线程并发 route,断言无 BlockNotFound 错误
//! - 1 线程 auto_rebalance + 100 线程 route,断言无错误
//!
//! WHY 100 线程:SubTask 15.11 要求,验证高并发下的路由一致性。
//! 现有 test_route_build_blocks_concurrency(10 线程)已验证基本并发安全,
//! 本测试将并发度提升到 100,覆盖更大规模的并发场景。
//!
//! # 并发安全性保障
//! SubTask 12.8 + 13.13 修复:route 一次性获取读锁读取 blocks 快照,
//! 锁外从 DashMap 无锁读取工具向量;auto_rebalance 仅更新 blocks(不修改 tool_vectors),
//! 确保 blocks 与 tool_vectors 始终一致,消除 BlockNotFound 竞态。

mod common;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use event_bus::EventBus;
use kvbsr_router::{KVBlockSemanticRouter, KvbsrError};
use nexus_core::CLV;

/// SubTask 15.11:100 线程并发 route,断言无 BlockNotFound 错误
///
/// 每个线程路由 10 次(共 1000 次),验证高并发下无 BlockNotFound、无 EmptyBlocks、无 panic。
/// 1000 次路由会触发 1 次自动重平衡(rebalance_interval=1000),验证自动重平衡与路由的并发安全。
#[tokio::test]
async fn test_100_concurrent_routes_no_block_not_found() {
    let bus = EventBus::new();
    let router = KVBlockSemanticRouter::new(bus);
    let (tools, co) = common::generate_test_data();
    router.build_blocks(tools, co).await.expect("构建块应成功");

    let test_cases = common::generate_test_cases();
    let clv_list: Vec<CLV> = test_cases.iter().map(|(clv, _)| clv.clone()).collect();

    let error_count = Arc::new(AtomicU64::new(0));
    let mut handles = Vec::with_capacity(100);

    for tid in 0..100 {
        let router_clone = router.clone();
        let clv = clv_list[tid % clv_list.len()].clone();
        let errors = error_count.clone();
        handles.push(tokio::spawn(async move {
            for _ in 0..10 {
                match router_clone.route(&clv).await {
                    Ok(result) => {
                        if result.selected_tools.is_empty() {
                            errors.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    Err(KvbsrError::BlockNotFound(_)) => {
                        errors.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(KvbsrError::EmptyBlocks) => {
                        errors.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(e) => {
                        eprintln!("route 错误: {e:?}");
                        errors.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }));
    }

    for handle in handles {
        handle.await.expect("route 线程应正常结束(无 panic)");
    }

    assert_eq!(
        error_count.load(Ordering::Relaxed),
        0,
        "100 线程并发路由出现错误(BlockNotFound 或空结果)"
    );
}

/// SubTask 15.11:1 线程 auto_rebalance + 100 线程 route,断言无错误
///
/// 1 个线程持续 auto_rebalance,100 个线程各路由 10 次,
/// 验证重平衡与路由的并发一致性(无 BlockNotFound、无 EmptyBlocks、无 panic)。
#[tokio::test]
async fn test_rebalance_with_100_concurrent_routes_no_error() {
    let bus = EventBus::new();
    let router = KVBlockSemanticRouter::new(bus);
    let (tools, co) = common::generate_test_data();
    router.build_blocks(tools, co).await.expect("构建块应成功");

    let test_cases = common::generate_test_cases();
    let clv_list: Vec<CLV> = test_cases.iter().map(|(clv, _)| clv.clone()).collect();

    let error_count = Arc::new(AtomicU64::new(0));
    let stop = Arc::new(AtomicBool::new(false));

    // 1 个 auto_rebalance 线程(持续运行直到所有 route 线程完成)
    let router_rebalance = router.clone();
    let stop_rebalance = stop.clone();
    let rebalance_handle = tokio::spawn(async move {
        while !stop_rebalance.load(Ordering::Relaxed) {
            let _ = router_rebalance.auto_rebalance().await;
            // 短暂让出,避免过度占用 CPU
            tokio::time::sleep(Duration::from_micros(100)).await;
        }
    });

    // 100 个 route 线程(每个路由 10 次)
    let mut route_handles = Vec::with_capacity(100);
    for tid in 0..100 {
        let router_clone = router.clone();
        let clv = clv_list[tid % clv_list.len()].clone();
        let errors = error_count.clone();
        route_handles.push(tokio::spawn(async move {
            for _ in 0..10 {
                match router_clone.route(&clv).await {
                    Ok(result) => {
                        if result.selected_tools.is_empty() {
                            errors.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    Err(KvbsrError::BlockNotFound(_)) => {
                        errors.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(KvbsrError::EmptyBlocks) => {
                        errors.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(e) => {
                        eprintln!("route 错误: {e:?}");
                        errors.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }));
    }

    // 等待所有 route 线程完成
    for handle in route_handles {
        handle.await.expect("route 线程应正常结束(无 panic)");
    }

    // 停止 rebalance 线程
    stop.store(true, Ordering::Relaxed);
    rebalance_handle.await.expect("rebalance 线程应正常结束");

    assert_eq!(
        error_count.load(Ordering::Relaxed),
        0,
        "1 线程 rebalance + 100 线程 route 出现错误"
    );
}
