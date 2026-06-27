//! SSRA 融合引擎集成测试 — 验证 EventBus 集成与事件契约
//!
//! 对应 SubTask 1.4:事件发布/订阅/配对
//!
//! # 验证场景
//! 1. 融合成功后发布 `SsraFusionCompleted` 事件(字段正确性)
//! 2. 无 EventBus 时不发布事件(不 panic)
//! 3. 订阅 `ConsensusReached` 触发防御性适配(预编译模板)
//! 4. 订阅 `RedTeamAudit` 触发防御性适配(预编译模板)
//! 5. 多次融合发布多个事件(事件流连续性)
//! 6. 事件 severity 为 Normal

#![forbid(unsafe_code)]

use event_bus::{EventBus, EventMetadata, EventSeverity, NexusEvent};
use ssra_fusion::{FusionRequest, FusionStrategy, SlimeFusionEngine, SlimeTemplate, SsraConfig};
use std::time::Duration;

/// 辅助:创建带模板的引擎(无 EventBus)
fn make_engine(templates: Vec<(&str, f32)>) -> SlimeFusionEngine {
    let config = SsraConfig::default();
    let engine = SlimeFusionEngine::new(config);
    for (id, weight) in templates {
        let t = SlimeTemplate::new(id, vec!["x".into()], FusionStrategy::TopK).with_weight(weight);
        engine.registry().register(t).expect("注册失败");
    }
    engine
}

/// 辅助:创建带模板的引擎(绑定 EventBus)
fn make_engine_with_bus(bus: EventBus, templates: Vec<(&str, f32)>) -> SlimeFusionEngine {
    let config = SsraConfig::default();
    let engine = SlimeFusionEngine::with_event_bus(config, bus);
    for (id, weight) in templates {
        let t = SlimeTemplate::new(id, vec!["x".into()], FusionStrategy::TopK).with_weight(weight);
        engine.registry().register(t).expect("注册失败");
    }
    engine
}

/// 辅助:创建融合请求
fn make_request(source: Vec<&str>) -> FusionRequest {
    FusionRequest::new(
        "q-test",
        source.into_iter().map(String::from).collect(),
        "target",
        20,
        8,
    )
}

// === 1. 融合成功发布 SsraFusionCompleted 事件(字段正确性) ===

#[tokio::test]
async fn test_fuse_publishes_ssra_fusion_completed() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let engine = make_engine_with_bus(bus, vec![("cap-1", 0.8)]);

    let req = make_request(vec!["cap-1"]);
    let result = engine.fuse(req).await.expect("融合失败");

    let event = rx.recv().await.expect("应收到事件");
    match event {
        NexusEvent::SsraFusionCompleted {
            quest_id,
            fused_template_id,
            latency_ms,
            confidence,
            ..
        } => {
            assert_eq!(quest_id, "q-test");
            assert_eq!(fused_template_id, result.fused_template_id);
            assert_eq!(latency_ms, result.latency_ms);
            assert!((confidence - result.confidence).abs() < 1e-5);
        }
        _ => panic!("期望 SsraFusionCompleted 事件,得到 {:?}", event.type_name()),
    }
}

// === 2. 无 EventBus 时不发布事件(不 panic) ===

#[tokio::test]
async fn test_fuse_without_event_bus_succeeds() {
    let engine = make_engine(vec![("cap-1", 0.8)]);
    let req = make_request(vec!["cap-1"]);

    // 无 EventBus,融合应正常完成,不 panic
    let result = engine.fuse(req).await.expect("融合失败");
    assert!(!result.fused_template_id.is_empty());
}

// === 3. 订阅 ConsensusReached 触发防御性适配 ===

#[tokio::test]
async fn test_defensive_adapter_on_consensus_reached() {
    let bus = EventBus::new();
    let engine = make_engine_with_bus(bus.clone(), vec![]);

    // 启动后台订阅任务
    let handle = engine.start_defensive_adapter().expect("应启动订阅");

    // 发布 ConsensusReached 事件
    bus.publish(NexusEvent::ConsensusReached {
        metadata: EventMetadata::new("parliament"),
        quest_id: "q-consensus".into(),
        decision_hash: "hash-abc".into(),
        dpo_pair_id: None,
    })
    .await
    .expect("发布失败");

    // 等待后台任务处理(防御性适配预编译并注册模板)
    tokio::time::sleep(Duration::from_millis(100)).await;

    // 验证:应注册了以 quest_id 为基础的防御性模板
    let template = engine.registry().get("defensive-q-consensus");
    assert!(
        template.is_some(),
        "ConsensusReached 应触发防御性适配模板注册"
    );

    handle.abort();
}

// === 4. 订阅 RedTeamAudit 触发防御性适配 ===

#[tokio::test]
async fn test_defensive_adapter_on_red_team_audit() {
    let bus = EventBus::new();
    let engine = make_engine_with_bus(bus.clone(), vec![]);

    let handle = engine.start_defensive_adapter().expect("应启动订阅");

    // 发布 RedTeamAudit 事件
    bus.publish(NexusEvent::RedTeamAudit {
        metadata: EventMetadata::new("parliament"),
        vulnerability_type: "prompt_injection".into(),
        failed_probes: 5,
        total_probes: 20,
        detection_rate: 0.25,
        remediation_suggestion: "add input sanitization".into(),
    })
    .await
    .expect("发布失败");

    tokio::time::sleep(Duration::from_millis(100)).await;

    // 验证:应注册了以 vulnerability_type 为基础的防御性模板
    let template = engine.registry().get("defensive-prompt_injection");
    assert!(template.is_some(), "RedTeamAudit 应触发防御性适配模板注册");

    handle.abort();
}

// === 5. 多次融合发布多个事件(事件流连续性) ===

#[tokio::test]
async fn test_multiple_fuse_multiple_events() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let engine = make_engine_with_bus(bus, vec![("cap-1", 0.5), ("cap-2", 0.9)]);

    // 连续发起 3 次融合
    for _ in 0..3 {
        let req = make_request(vec!["cap-1", "cap-2"]);
        engine.fuse(req).await.expect("融合失败");
    }

    // 应收到 3 个 SsraFusionCompleted 事件
    let mut count = 0;
    for _ in 0..3 {
        let event = rx.recv().await.expect("应收到事件");
        if let NexusEvent::SsraFusionCompleted { .. } = event {
            count += 1;
        }
    }
    assert_eq!(count, 3, "应收到 3 个 SsraFusionCompleted 事件");
}

// === 6. SsraFusionCompleted 事件 severity 为 Normal ===

#[tokio::test]
async fn test_ssra_fusion_completed_severity_normal() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let engine = make_engine_with_bus(bus, vec![("cap-1", 0.8)]);

    let req = make_request(vec!["cap-1"]);
    engine.fuse(req).await.expect("融合失败");

    let event = rx.recv().await.expect("应收到事件");
    assert_eq!(
        event.severity(),
        EventSeverity::Normal,
        "SsraFusionCompleted 应为 Normal 级别"
    );
    assert_eq!(event.type_name(), "SsraFusionCompleted");
    assert_eq!(event.metadata().source, "ssra-fusion");
}

// === 7. 防御性适配模板使用正确策略 ===

#[tokio::test]
async fn test_defensive_adapter_strategy() {
    let bus = EventBus::new();
    let engine = make_engine_with_bus(bus.clone(), vec![]);

    let handle = engine.start_defensive_adapter().expect("应启动订阅");

    // ConsensusReached → WeightedAverage 策略
    bus.publish(NexusEvent::ConsensusReached {
        metadata: EventMetadata::new("parliament"),
        quest_id: "q-1".into(),
        decision_hash: "h".into(),
        dpo_pair_id: None,
    })
    .await
    .expect("发布失败");

    tokio::time::sleep(Duration::from_millis(100)).await;

    let template = engine
        .registry()
        .get("defensive-q-1")
        .expect("应注册防御性模板");
    assert_eq!(
        template.fusion_strategy,
        FusionStrategy::WeightedAverage,
        "ConsensusReached 防御性模板应用 WeightedAverage 策略"
    );

    handle.abort();
}
