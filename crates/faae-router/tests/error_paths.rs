//! FaaE 语义路由错误路径测试 — 验证关键故障场景的错误传播
//!
//! 对应 SubTask 29.6:为 faae-router 补充错误路径测试
//!
//! # 测试覆盖
//! 1. 专家不存在:unregister_expert 对未注册工具返回 ExpertNotFound
//! 2. 路由失败(空候选集):route 对空候选集返回 RoutingFailed
//! 3. 路由失败(无已注册候选):route 对未注册候选返回 RoutingFailed
//! 4. 熵计算边界:compute_entropy 对空/单工具 profiles 返回 Ok(1.0)
//! 5. 并发注册冲突:多线程并发注册不同专家,验证不 panic 且计数正确

#![forbid(unsafe_code)]

use std::sync::Arc;

use event_bus::EventBus;
use faae_router::{EdsbBalancer, ExpertProfile, FaaeConfig, FaaeError, FaaeRouter, ToolId};

/// 专家不存在:unregister_expert 对未注册工具返回 ExpertNotFound
///
/// WHY:注销未注册的工具是调用方错误,必须返回 ExpertNotFound 而非静默成功。
/// 此测试验证错误传播的正确性。
#[tokio::test]
async fn test_unregister_nonexistent_returns_expert_not_found() {
    let router = FaaeRouter::new(EventBus::new());

    let result = router.unregister_expert(&ToolId::new("nonexistent")).await;
    let err = match result {
        Ok(_) => panic!("注销未注册工具应返回错误"),
        Err(e) => e,
    };
    assert!(
        matches!(err, FaaeError::ExpertNotFound { .. }),
        "应为 ExpertNotFound,实际: {err:?}"
    );
    let msg = format!("{err}");
    assert!(
        msg.contains("nonexistent"),
        "错误信息应包含工具 ID,实际: {msg}"
    );
}

/// 路由失败(空候选集):route 对空候选集返回 RoutingFailed
///
/// WHY:空候选集是无效输入,KVBSR 粗筛应至少返回 1 个候选。
/// 此测试验证空候选集的错误传播。
#[tokio::test]
async fn test_route_empty_candidates_returns_routing_failed() {
    let router = FaaeRouter::new(EventBus::new());
    let clv = vec![0.5; 64];

    let result = router.route(&clv, &[]).await;
    let err = match result {
        Ok(_) => panic!("空候选集应返回错误"),
        Err(e) => e,
    };
    assert!(
        matches!(err, FaaeError::RoutingFailed { .. }),
        "应为 RoutingFailed,实际: {err:?}"
    );
    let msg = format!("{err}");
    assert!(
        msg.contains("候选工具集为空"),
        "错误信息应包含'候选工具集为空',实际: {msg}"
    );
}

/// 路由失败(无已注册候选):route 对未注册候选返回 RoutingFailed
///
/// WHY:候选集中的工具可能未注册(如刚注销),route 应返回 RoutingFailed 而非 panic。
/// 此测试验证候选集全部未注册时的错误传播。
#[tokio::test]
async fn test_route_no_registered_candidates_returns_routing_failed() {
    let router = FaaeRouter::new(EventBus::new());
    let clv = vec![0.5; 64];
    let candidates = vec![ToolId::new("unregistered-1"), ToolId::new("unregistered-2")];

    let result = router.route(&clv, &candidates).await;
    let err = match result {
        Ok(_) => panic!("全部未注册的候选集应返回错误"),
        Err(e) => e,
    };
    assert!(
        matches!(err, FaaeError::RoutingFailed { .. }),
        "应为 RoutingFailed,实际: {err:?}"
    );
    let msg = format!("{err}");
    assert!(
        msg.contains("无已注册的专家"),
        "错误信息应包含'无已注册的专家',实际: {msg}"
    );
}

/// 熵计算边界:compute_entropy 对空/单工具 profiles 返回 Ok(1.0)
///
/// WHY 边界处理:空 profiles 或单工具无法计算有意义的熵,
/// 返回 1.0(视为均匀)避免除零错误。
/// 此测试验证边界条件的健壮性。
#[tokio::test]
async fn test_compute_entropy_boundary_returns_one() {
    let balancer = EdsbBalancer::new(FaaeConfig::default(), EventBus::new());

    // 空 profiles
    let empty_profiles = std::collections::HashMap::new();
    let entropy = balancer.compute_entropy(&empty_profiles).await;
    assert!(entropy.is_ok(), "空 profiles 应返回 Ok");
    assert_eq!(entropy.unwrap(), 1.0, "空 profiles 熵应为 1.0");

    // 单工具 profiles
    let mut single_profile = std::collections::HashMap::new();
    let profile = ExpertProfile::with_usage_count("t1", vec![0.5; 64], vec![], 0.5, 100);
    single_profile.insert(
        ToolId::new("t1"),
        Arc::new(tokio::sync::RwLock::new(profile)),
    );
    let entropy = balancer.compute_entropy(&single_profile).await;
    assert!(entropy.is_ok(), "单工具 profiles 应返回 Ok");
    assert_eq!(entropy.unwrap(), 1.0, "单工具 profiles 熵应为 1.0");

    // 全零使用计数
    let mut zero_profiles = std::collections::HashMap::new();
    for name in &["t1", "t2", "t3"] {
        let profile = ExpertProfile::with_usage_count(*name, vec![0.5; 64], vec![], 0.5, 0);
        zero_profiles.insert(
            ToolId::new(*name),
            Arc::new(tokio::sync::RwLock::new(profile)),
        );
    }
    let entropy = balancer.compute_entropy(&zero_profiles).await;
    assert!(entropy.is_ok());
    assert_eq!(entropy.unwrap(), 1.0, "全零使用计数熵应为 1.0");
}

/// 并发注册冲突:多线程并发注册不同专家,验证不 panic 且计数正确
///
/// WHY:FaaE 路由器是并发安全的,多线程并发注册不同专家时不应丢失注册。
/// 此测试验证并发场景下的线程安全性。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_concurrent_register_no_conflict() {
    let router = Arc::new(FaaeRouter::new(EventBus::new()));

    let mut handles = Vec::new();

    // 4 线程并发注册,每线程注册 5 个不同专家
    for thread_id in 0..4 {
        let router_clone = Arc::clone(&router);
        handles.push(tokio::spawn(async move {
            for i in 0..5 {
                let tool_id = format!("t{thread_id}-{i}");
                let profile = ExpertProfile::new(tool_id, vec![0.5; 64], vec!["test".into()], 0.8);
                router_clone.register_expert(profile).await;
            }
        }));
    }

    // 等待所有线程完成
    for handle in handles {
        handle.await.expect("并发注册任务 panic");
    }

    // 应注册 20 个专家(4 线程 × 5 专家)
    assert_eq!(router.expert_count().await, 20, "并发注册后专家数应为 20");
}
