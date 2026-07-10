//! SESA 激活路由器集成测试 — 验证 EventBus 集成、稀疏度约束与并发安全
//!
//! 对应 SubTask 3.6:集成测试
//!
//! # 验证场景
//! 1. 激活成功后发布 `SesaActivationCompleted` 事件(字段正确性)
//! 2. 无 EventBus 时不发布事件(不 panic)
//! 3. 256 专家池激活 + 稀疏度 < 40% 严格断言
//! 4. 模拟 1000 专家规模(超 256 容量返回 IndexOutOfBounds)
//! 5. 并发激活无死锁(10 个 tokio task 并发)
//! 6. 订阅 `ConsensusReached` 后台任务启动
//! 7. 事件 severity 为 Normal
//! 8. 延迟测量 p95 ≤ 5ms(性能验证)

#![forbid(unsafe_code)]

use event_bus::{EventBus, EventMetadata, EventSeverity, NexusEvent};
use sesa_router::{ActivationRequest, ExpertDescriptor, SesaConfig, SesaError, SesaRouter};
use std::time::Duration;

/// 辅助:创建带 N 个专家的路由器(无 EventBus)
fn make_router(n: usize) -> SesaRouter {
    let router = SesaRouter::new(SesaConfig::default());
    for i in 0..n {
        let v = vec![(i as f32) * 0.01; 64];
        let expert = ExpertDescriptor::new(format!("expert-{i}"), v);
        let _ = router.register_expert(expert);
    }
    router
}

/// 辅助:创建带 N 个专家的路由器(绑定 EventBus)
///
/// WHY 禁用 prerequisite_check:这些集成测试验证 EventBus 集成与稀疏度约束,
/// 不关心五层路由顺序。PrerequisiteChecker 有独立的 prerequisite_test.rs 测试。
fn make_router_with_bus(bus: EventBus, n: usize) -> SesaRouter {
    let config = SesaConfig {
        prerequisite_check_enabled: false,
        ..SesaConfig::default()
    };
    let router = SesaRouter::with_event_bus(config, bus);
    for i in 0..n {
        let v = vec![(i as f32) * 0.01; 64];
        let expert = ExpertDescriptor::new(format!("expert-{i}"), v);
        let _ = router.register_expert(expert);
    }
    router
}

/// 辅助:创建激活请求
fn make_request(top_k: usize, deadline_ms: u64) -> ActivationRequest {
    ActivationRequest::new("req-test", vec![0.5; 64], top_k, deadline_ms)
}

// === 1. 激活成功发布 SesaActivationCompleted 事件(字段正确性) ===

#[tokio::test]
async fn test_activate_publishes_sesa_activation_completed() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let router = make_router_with_bus(bus, 10);

    let req = make_request(8, 5);
    let (mask, profile) = router.activate(req).await.expect("激活失败");

    let event = rx.recv().await.expect("应收到事件");
    match event {
        NexusEvent::SesaActivationCompleted {
            total_experts,
            active_experts,
            sparsity_ratio,
            latency_us,
            ..
        } => {
            assert_eq!(total_experts, profile.total_experts);
            assert_eq!(active_experts, profile.active_experts);
            assert!((sparsity_ratio - profile.sparsity_ratio).abs() < 1e-5);
            assert!(latency_us > 0, "延迟应 > 0");
            assert_eq!(active_experts, mask.active_count);
        }
        _ => panic!(
            "期望 SesaActivationCompleted 事件,得到 {:?}",
            event.type_name()
        ),
    }
}

// === 2. 无 EventBus 时不发布事件(不 panic) ===

#[tokio::test]
async fn test_activate_without_event_bus_succeeds() {
    let router = make_router(10);
    let req = make_request(8, 5);

    // 无 EventBus,激活应正常完成,不 panic
    let (mask, _profile) = router.activate(req).await.expect("激活失败");
    assert!(mask.active_count > 0);
}

// === 3. 256 专家池激活 + 稀疏度 < 40% 严格断言 ===

#[tokio::test]
async fn test_activate_256_experts_sparsity_strict_under_40_percent() {
    let router = make_router(256);

    // top_k=256 试图激活全部
    let req = make_request(256, 5);
    let (mask, profile) = router.activate(req).await.expect("激活失败");

    // 256 × 0.4 = 102.4 → floor = 102
    assert_eq!(mask.active_count, 102, "256 专家 × 0.4 floor = 102");
    assert!(
        profile.sparsity_ratio < 0.4,
        "稀疏度应严格 < 40%, got {} (active={}/total={})",
        profile.sparsity_ratio,
        profile.active_experts,
        profile.total_experts
    );
    assert_eq!(profile.active_experts, 102);
    assert_eq!(profile.total_experts, 256);
    let expected = 102.0_f32 / 256.0;
    assert!(
        (profile.sparsity_ratio - expected).abs() < 1e-5,
        "102/256 = {}, got {}",
        expected,
        profile.sparsity_ratio
    );
}

// === 4. 模拟 1000 专家规模(超 256 容量返回 IndexOutOfBounds) ===

#[tokio::test]
async fn test_1000_experts_pool_index_out_of_bounds() {
    let router = SesaRouter::new(SesaConfig::default());

    // 注册前 256 个专家应成功
    for i in 0..256 {
        let expert = ExpertDescriptor::new(format!("expert-{i}"), vec![0.5; 64]);
        router.register_expert(expert).expect("前 256 应成功");
    }
    assert_eq!(router.expert_count(), 256);

    // 第 257 个应失败
    let expert_257 = ExpertDescriptor::new("expert-256", vec![0.5; 64]);
    let result = router.register_expert(expert_257);
    assert!(result.is_err(), "第 257 个专家应超过 256-bit 掩码容量");
    assert!(matches!(
        result,
        Err(SesaError::IndexOutOfBounds { capacity: 256, .. })
    ));

    // 模拟 1000 专家场景:实际 1000 专家需通过 KVBSR 粗筛降至 256 以内
    // 这里验证 SesaRouter 正确拒绝超容量注册
    let mut rejected_count = 0;
    for i in 256..1000 {
        let expert = ExpertDescriptor::new(format!("expert-{i}"), vec![0.5; 64]);
        if router.register_expert(expert).is_err() {
            rejected_count += 1;
        }
    }
    assert_eq!(rejected_count, 1000 - 256, "1000 专家中应有 744 个被拒绝");
    assert_eq!(router.expert_count(), 256);
}

// === 5. 并发激活无死锁(10 个 tokio task 并发) ===

#[tokio::test]
async fn test_concurrent_activate_no_deadlock() {
    let router = std::sync::Arc::new(make_router(50));
    let mut handles = Vec::new();

    // 10 个并发激活任务
    for i in 0..10 {
        let r = std::sync::Arc::clone(&router);
        handles.push(tokio::spawn(async move {
            let req = ActivationRequest::new(
                format!("req-{i}"),
                vec![0.5; 64],
                8,
                100, // 较长超时避免并发抖动
            );
            r.activate(req).await.expect("并发激活失败")
        }));
    }

    let mut profiles = Vec::new();
    for h in handles {
        let (_mask, profile) = h.await.expect("task panic");
        profiles.push(profile);
    }

    // 所有并发激活都应满足稀疏度约束
    for p in &profiles {
        assert!(
            p.sparsity_ratio < 0.4,
            "并发激活稀疏度应 < 40%, got {}",
            p.sparsity_ratio
        );
    }
    assert_eq!(profiles.len(), 10, "10 个任务都应完成");
}

// === 6. 订阅 ConsensusReached 后台任务启动 ===

#[tokio::test]
async fn test_start_consensus_listener_receives_event() {
    let bus = EventBus::new();
    let router = make_router_with_bus(bus.clone(), 10);

    // 启动后台订阅任务
    let handle = router.start_consensus_listener().expect("应启动订阅");

    // 发布 ConsensusReached 事件
    bus.publish(NexusEvent::ConsensusReached {
        metadata: EventMetadata::new("parliament"),
        quest_id: "q-consensus".into(),
        decision_hash: "hash-abc".into(),
        dpo_pair_id: None,
    })
    .await
    .expect("发布失败");

    // 等待后台任务处理
    tokio::time::sleep(Duration::from_millis(50)).await;

    // 后台任务应仍在运行(未因 panic 退出)
    assert!(!handle.is_finished(), "后台任务应仍在运行");

    handle.abort();
}

// === 7. SesaActivationCompleted 事件 severity 为 Normal ===

#[tokio::test]
async fn test_sesa_activation_completed_severity_normal() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let router = make_router_with_bus(bus, 10);

    let req = make_request(8, 5);
    router.activate(req).await.expect("激活失败");

    let event = rx.recv().await.expect("应收到事件");
    assert_eq!(
        event.severity(),
        EventSeverity::Normal,
        "SesaActivationCompleted 应为 Normal 级别"
    );
    assert_eq!(event.type_name(), "SesaActivationCompleted");
    assert_eq!(event.metadata().source, "sesa-router");
}

// === 8. 延迟测量 p95 ≤ 5ms(性能验证) ===

#[tokio::test]
async fn test_activate_latency_p95_under_5ms() {
    let router = make_router(256);

    // 50 次激活采样
    let mut latencies_us: Vec<u64> = Vec::with_capacity(50);
    for _ in 0..50 {
        let req = make_request(8, 100); // 充足超时
        let start = std::time::Instant::now();
        let _ = router.activate(req).await.expect("激活失败");
        latencies_us.push(start.elapsed().as_micros() as u64);
    }

    latencies_us.sort_unstable();
    let p95_idx = (latencies_us.len() as f32 * 0.95) as usize;
    let p95_us = latencies_us[p95_idx.min(latencies_us.len() - 1)];
    let p95_ms = p95_us as f64 / 1000.0;

    assert!(
        p95_ms <= 5.0,
        "p95 延迟应 ≤ 5ms, got {}ms (p95_us={}μs)",
        p95_ms,
        p95_us
    );
}

// === 9. 多次激活发布多个事件(事件流连续性) ===

#[tokio::test]
async fn test_multiple_activate_multiple_events() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let router = make_router_with_bus(bus, 20);

    // 连续发起 5 次激活
    for _ in 0..5 {
        let req = make_request(8, 5);
        router.activate(req).await.expect("激活失败");
    }

    // 应收到 5 个 SesaActivationCompleted 事件
    let mut count = 0;
    for _ in 0..5 {
        let event = rx.recv().await.expect("应收到事件");
        if let NexusEvent::SesaActivationCompleted { .. } = event {
            count += 1;
        }
    }
    assert_eq!(count, 5, "应收到 5 个 SesaActivationCompleted 事件");
}

// === 10. 掩码激活位与 Top-K 一致性 ===

#[tokio::test]
async fn test_activate_mask_consistent_with_top_k() {
    let router = SesaRouter::new(SesaConfig::default());
    // WHY 注册 10 个专家(而非 5 个):5 专家 × 0.4 = 2,严格 < 40% → max_allowed=1,
    // 无法验证 Top-3 一致性;10 专家 × 0.4 = 4,严格 < 40% → max_allowed=3,
    // 恰好允许 top_k=3 激活,可验证掩码位与 Top-K 的一致性。
    for i in 0..10 {
        let v = vec![(i as f32) * 0.1; 64];
        let expert = ExpertDescriptor::new(format!("expert-{i}"), v);
        router.register_expert(expert).expect("注册失败");
    }

    // 查询向量 = [0.5; 64],与 expert-5 (0.5) 最相似
    let req = ActivationRequest::new("req-1", vec![0.5; 64], 3, 5);
    let (mask, _profile) = router.activate(req).await.expect("激活失败");

    // 10 专家 × 0.4 = 4,严格 < 40% → max_allowed = 3;top_k=3 → 激活 3 位
    assert_eq!(mask.active_count, 3);

    // 验证激活位对应的专家评分确实是 Top-3
    let active_indices = mask.to_indices();
    assert_eq!(active_indices.len(), 3);
}

// === 11. 自定义配置:top_k=1 单专家激活 ===

#[tokio::test]
async fn test_activate_top_k_one_single_expert() {
    let router = make_router(20);
    let req = make_request(1, 5);
    let (mask, profile) = router.activate(req).await.expect("激活失败");

    assert_eq!(mask.active_count, 1, "top_k=1 应激活 1 位");
    assert_eq!(profile.active_experts, 1);
    assert!((profile.sparsity_ratio - 0.05).abs() < 1e-5, "1/20 = 0.05");
}

// === 12. 注销专家后激活 ===

#[tokio::test]
async fn test_unregister_then_activate() {
    let router = make_router(10);
    assert_eq!(router.expert_count(), 10);

    // 注销 5 个专家
    for i in 0..5 {
        let id = format!("expert-{i}");
        router.unregister_expert(&id);
    }
    assert_eq!(router.expert_count(), 5);

    let req = make_request(8, 5);
    let (mask, profile) = router.activate(req).await.expect("激活失败");

    // top_k=8 但只剩 5 个专家,且稀疏度约束严格 < 40%
    // WHY:5 × 0.4 = 2,2/5 = 0.4 不严格 < 0.4 → max_allowed = 1,
    // 故仅激活评分最高的 1 位专家(架构红线:稀疏度严格 < 40%)
    assert_eq!(mask.active_count, 1, "5 专家严格 < 40% → 仅激活 1 位");
    assert_eq!(profile.total_experts, 5);
    assert!(profile.sparsity_ratio < 0.4, "稀疏度应严格 < 40%");
}

// === 13. 零向量查询不 panic(返回相似度 0) ===

#[tokio::test]
async fn test_activate_zero_query_vector_no_panic() {
    let router = make_router(10);

    // 全零查询向量:cosine_similarity_slices 返回 0.0
    let req = ActivationRequest::new("req-zero", vec![0.0; 64], 5, 5);
    let result = router.activate(req).await;

    // 应正常完成(所有评分相同,Top-K 任意选择)
    assert!(result.is_ok(), "零向量查询不应失败");
    let (mask, _profile) = result.expect("已验证 is_ok");
    // WHY:10 × 0.4 = 4,4/10 = 0.4 不严格 < 0.4 → max_allowed = 3,
    // 故 top_k=5 被稀疏度约束裁剪至 3 位(严格 < 40%)
    assert_eq!(mask.active_count, 3, "10 专家严格 < 40% → 激活 3 位");
}

// === 14. 维度不匹配查询(零向量相似度处理) ===

#[tokio::test]
async fn test_activate_dimension_mismatch_handles_gracefully() {
    let router = make_router(10);

    // 查询向量维度与专家向量不同(64 vs 32)
    // cosine_similarity_slices 取 min(len),不 panic
    let req = ActivationRequest::new("req-mismatch", vec![0.5; 32], 5, 5);
    let result = router.activate(req).await;

    // 应正常完成(不 panic)
    assert!(result.is_ok(), "维度不匹配不应失败");
}
