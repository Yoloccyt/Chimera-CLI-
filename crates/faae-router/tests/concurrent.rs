//! SubTask 28.6:FaaE + EDSB 并发测试
//!
//! 验证:
//! - 10 线程并发路由,无 panic、无数据竞争
//! - 并发注册 + 路由,无错误
//! - usage_count 在并发路由下正确递增
//!
//! # 并发安全性保障
//! - expert_registry: Arc<RwLock<HashMap<...>>>,读多写少
//! - 内层 Arc<RwLock<ExpertProfile>>:每个专家独立锁
//! - usage_count: AtomicU64,无锁原子更新
//! - route 路径:获取读锁 → clone Arc → 释放锁 → 锁外计算

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use event_bus::EventBus;
use faae_router::{ExpertProfile, FaaeError, FaaeRouter, ToolId};

/// 构造测试用专家画像(64 维向量,不同维度有高值确保区分度)
fn make_test_profile(name: &str, dim_idx: usize) -> ExpertProfile {
    let mut v = vec![0.0; 64];
    v[dim_idx % 64] = 1.0;
    ExpertProfile::new(name, v, vec!["test".into()], 1.0)
}

/// 10 线程并发路由,断言无 panic、无 RoutingFailed 错误
#[tokio::test]
async fn test_10_concurrent_routes_no_error() {
    let bus = EventBus::new();
    let router = Arc::new(FaaeRouter::new(bus));

    // 注册 20 个工具(每个在不同维度有高值)
    for i in 0..20 {
        let profile = make_test_profile(&format!("tool-{i}"), i);
        router.register_expert(profile).await;
    }

    let error_count = Arc::new(AtomicU64::new(0));
    let mut handles = Vec::with_capacity(10);

    for tid in 0..10 {
        let router_clone = router.clone();
        let errors = error_count.clone();
        handles.push(tokio::spawn(async move {
            // 每个线程路由 10 次
            for _ in 0..10 {
                let mut clv = vec![0.0; 64];
                clv[tid % 64] = 1.0; // 匹配不同工具
                let candidates: Vec<ToolId> =
                    (0..20).map(|j| ToolId::new(format!("tool-{j}"))).collect();
                match router_clone.route(&clv, &candidates).await {
                    Ok(result) => {
                        if result.candidates.is_empty() {
                            errors.fetch_add(1, Ordering::Relaxed);
                        }
                    }
                    Err(FaaeError::RoutingFailed { .. }) => {
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
        "10 线程并发路由出现错误"
    );
}

/// 并发注册 + 路由,验证无错误
#[tokio::test]
async fn test_concurrent_register_and_route() {
    let bus = EventBus::new();
    let router = Arc::new(FaaeRouter::new(bus));

    // 先注册 10 个工具
    for i in 0..10 {
        let profile = make_test_profile(&format!("tool-{i}"), i);
        router.register_expert(profile).await;
    }

    let error_count = Arc::new(AtomicU64::new(0));
    let mut handles = Vec::new();

    // 5 个线程并发注册新工具
    for i in 10..15 {
        let router_clone = router.clone();
        let errors = error_count.clone();
        handles.push(tokio::spawn(async move {
            let profile = make_test_profile(&format!("tool-{i}"), i);
            router_clone.register_expert(profile).await;
            // 注册不应产生错误
            let _ = errors;
        }));
    }

    // 5 个线程并发路由
    for tid in 0..5 {
        let router_clone = router.clone();
        let errors = error_count.clone();
        handles.push(tokio::spawn(async move {
            let mut clv = vec![0.0; 64];
            clv[tid] = 1.0;
            let candidates: Vec<ToolId> =
                (0..10).map(|j| ToolId::new(format!("tool-{j}"))).collect();
            if let Err(e) = router_clone.route(&clv, &candidates).await {
                eprintln!("并发路由错误: {e:?}");
                errors.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }

    for handle in handles {
        handle.await.expect("线程应正常结束(无 panic)");
    }

    assert_eq!(
        error_count.load(Ordering::Relaxed),
        0,
        "并发注册+路由出现错误"
    );
    assert_eq!(router.expert_count().await, 15);
}

/// 验证 usage_count 在并发路由下正确递增
#[tokio::test]
async fn test_concurrent_usage_count_increment() {
    let bus = EventBus::new();
    let router = Arc::new(FaaeRouter::new(bus));

    // 注册 1 个工具
    let profile = make_test_profile("tool-0", 0);
    router.register_expert(profile).await;

    let route_count = 50;
    let mut handles = Vec::with_capacity(10);

    for _ in 0..10 {
        let router_clone = router.clone();
        handles.push(tokio::spawn(async move {
            let clv = vec![1.0; 64];
            let candidates = vec![ToolId::new("tool-0")];
            for _ in 0..route_count / 10 {
                router_clone.route(&clv, &candidates).await.unwrap();
            }
        }));
    }

    for handle in handles {
        handle.await.unwrap();
    }

    // 验证 usage_count = route_count(无丢失)
    let registry = router.registry();
    let reg = registry.read().await;
    let profile = reg.get(&ToolId::new("tool-0")).unwrap().read().await;
    assert_eq!(
        profile.get_usage_count(),
        route_count,
        "并发路由后 usage_count 应精确等于路由次数"
    );
}

/// 并发注销 + 路由,验证无 panic
#[tokio::test]
async fn test_concurrent_unregister_and_route() {
    let bus = EventBus::new();
    let router = Arc::new(FaaeRouter::new(bus));

    // 注册 20 个工具
    for i in 0..20 {
        let profile = make_test_profile(&format!("tool-{i}"), i);
        router.register_expert(profile).await;
    }

    let panic_count = Arc::new(AtomicU64::new(0));
    let mut handles = Vec::new();

    // 1 个线程持续注销工具
    let router_unregister = router.clone();
    let panics = panic_count.clone();
    handles.push(tokio::spawn(async move {
        for i in 0..10 {
            let _ = router_unregister
                .unregister_expert(&ToolId::new(format!("tool-{i}")))
                .await;
        }
        let _ = panics;
    }));

    // 5 个线程持续路由
    for _ in 0..5 {
        let router_clone = router.clone();
        let panics = panic_count.clone();
        handles.push(tokio::spawn(async move {
            for _ in 0..20 {
                let clv = vec![0.5; 64];
                let candidates: Vec<ToolId> =
                    (0..20).map(|j| ToolId::new(format!("tool-{j}"))).collect();
                // 路由可能因工具被注销而失败,但不应 panic
                let _ = router_clone.route(&clv, &candidates).await;
            }
            let _ = panics;
        }));
    }

    for handle in handles {
        handle.await.expect("线程应正常结束(无 panic)");
    }

    assert_eq!(
        panic_count.load(Ordering::Relaxed),
        0,
        "并发注销+路由出现 panic"
    );
}
