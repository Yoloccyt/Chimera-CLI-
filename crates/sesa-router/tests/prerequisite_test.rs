//! Phase IV Task N9: SESA 前置事件校验集成测试
//!
//! 验证 PrerequisiteChecker 在 SesaRouter::activate() 入口校验五层路由顺序:
//! - OSA 完成 → OmniSparseMasksComputed
//! - KVBSR/FaaE 完成 → ToolsRouted
//! - FaaE 路由完成 → ExpertRouted
//!
//! 三者齐备才允许 SESA 激活,强制五层路由顺序(安全优先)。

use event_bus::{EventBus, EventMetadata, NexusEvent};
use sesa_router::{ActivationRequest, ExpertDescriptor, SesaConfig, SesaError, SesaRouter};

/// 构造 OmniSparseMasksComputed 事件(OSA 完成信号)
fn make_osa_event() -> NexusEvent {
    NexusEvent::OmniSparseMasksComputed {
        metadata: EventMetadata::new("osa-coordinator"),
        mask_hash: "mask-001".into(),
        sparsity: 0.6,
        context_mask: vec!["file-1".into()],
    }
}

/// 构造 ToolsRouted 事件(KVBSR/FaaE 完成信号)
fn make_tools_routed_event() -> NexusEvent {
    NexusEvent::ToolsRouted {
        metadata: EventMetadata::new("kvbsr-router"),
        routed_count: 8,
        top_tool: "tool-1".into(),
        routed_tools: vec!["tool-1".into()],
    }
}

/// 构造 ExpertRouted 事件(FaaE 路由完成信号)
fn make_expert_routed_event() -> NexusEvent {
    NexusEvent::ExpertRouted {
        metadata: EventMetadata::new("faae-router"),
        routed_tool: "tool-1".into(),
        confidence: 0.92,
    }
}

/// 构造已注册专家的 router,用于激活测试
fn make_router_with_experts(bus: EventBus) -> SesaRouter {
    let router = SesaRouter::with_event_bus(SesaConfig::default(), bus);
    for i in 0..10 {
        let expert = ExpertDescriptor::new(format!("expert-{i}"), vec![0.1 * i as f32; 64]);
        router.register_expert(expert).expect("注册失败");
    }
    router
}

// ============================================================
// 测试 1: 无上游事件时 activate 应被阻塞
// ============================================================

#[tokio::test]
async fn test_blocks_activation_without_upstream_events() {
    let bus = EventBus::new();
    let router = make_router_with_experts(bus);

    // 不发布任何上游事件,直接激活
    let req = ActivationRequest::new("req-1", vec![0.5; 64], 8, 50);
    let result = router.activate(req).await;

    assert!(
        result.is_err(),
        "无上游事件时 activate 必须返回错误(强制五层路由顺序)"
    );
    assert!(
        matches!(result, Err(SesaError::PrerequisiteNotMet { .. })),
        "错误类型应为 PrerequisiteNotMet, 实际: {:?}",
        result
    );
}

// ============================================================
// 测试 2: 三事件齐备后 activate 应成功
// ============================================================

#[tokio::test]
async fn test_allows_activation_with_upstream_events() {
    let bus = EventBus::new();
    let router = make_router_with_experts(bus.clone());

    // 发布三个上游事件(用 publish_blocking 同步发布,避免测试中 await 顺序问题)
    bus.publish_blocking(make_osa_event())
        .expect("发布 OSA 事件失败");
    bus.publish_blocking(make_tools_routed_event())
        .expect("发布 ToolsRouted 事件失败");
    bus.publish_blocking(make_expert_routed_event())
        .expect("发布 ExpertRouted 事件失败");

    // 三事件齐备,activate 应成功
    let req = ActivationRequest::new("req-2", vec![0.5; 64], 8, 50);
    let result = router.activate(req).await;

    assert!(
        result.is_ok(),
        "三上游事件齐备后 activate 应成功, 实际错误: {:?}",
        result.err()
    );
    let (_mask, profile) = result.expect("已断言 is_ok");
    assert!(
        profile.sparsity_ratio < 0.4,
        "稀疏度应 < 40%, got {}",
        profile.sparsity_ratio
    );
}

// ============================================================
// 测试 3: SesaConfig 默认启用 PrerequisiteChecker
// ============================================================

#[test]
fn test_prerequisite_checker_default_enabled() {
    let config = SesaConfig::default();
    assert!(
        config.prerequisite_check_enabled,
        "SesaConfig 默认应启用 PrerequisiteChecker(安全优先,强制五层路由顺序)"
    );
}
