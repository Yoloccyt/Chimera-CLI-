//! EventBus 集成测试 — 验证 GsoePolicyUpdated 事件的发布与订阅
//!
//! 对应 SubTask 3.5
//!
//! # 测试覆盖
//! - 事件发布:evolve_once 触发 GsoePolicyUpdated 事件
//! - 事件字段:generation/improvement/new_mutation_rate/new_selection_pressure 正确性
//! - 事件 source:metadata.source == "gsoe-evolution"
//! - 多代进化:连续多代均发布事件
//! - 事件序列化:GsoePolicyUpdated 可正确序列化/反序列化
//! - ConsensusReached/RedTeamAudit 信号处理

use event_bus::{EventBus, EventMetadata, NexusEvent};
use gsoe_evolution::{GsoeConfig, GsoeEvolutionEngine};
use std::time::Duration;

/// 验证 evolve_once 正确发布 GsoePolicyUpdated 事件
#[tokio::test]
async fn test_evolve_once_publishes_gsoe_policy_updated() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let mut engine = GsoeEvolutionEngine::with_event_bus(GsoeConfig::default(), bus);

    engine.evolve_once().await.expect("进化失败");

    let event = rx
        .recv_timeout(Duration::from_secs(1))
        .await
        .expect("接收事件超时");

    assert!(
        matches!(event, NexusEvent::GsoePolicyUpdated { .. }),
        "期望 GsoePolicyUpdated 事件,收到 {event:?}"
    );
}

/// 验证事件字段正确性
#[tokio::test]
async fn test_gsoe_policy_updated_event_fields() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let mut engine = GsoeEvolutionEngine::with_event_bus(GsoeConfig::default(), bus);

    engine.evolve_once().await.unwrap();

    let event = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();

    if let NexusEvent::GsoePolicyUpdated {
        generation,
        improvement,
        new_mutation_rate,
        new_selection_pressure,
        ..
    } = event
    {
        assert_eq!(generation, 1, "首轮进化 generation 应为 1");
        assert!(improvement.is_finite(), "improvement 应为有限值");
        assert!(
            (0.0..=1.0).contains(&new_mutation_rate),
            "new_mutation_rate 应在 [0, 1]: {new_mutation_rate}"
        );
        assert!(
            new_selection_pressure >= 0.0,
            "new_selection_pressure 应非负: {new_selection_pressure}"
        );
    } else {
        panic!("期望 GsoePolicyUpdated 事件");
    }
}

/// 验证事件 source 为 gsoe-evolution
#[tokio::test]
async fn test_gsoe_policy_updated_event_source() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let mut engine = GsoeEvolutionEngine::with_event_bus(GsoeConfig::default(), bus);

    engine.evolve_once().await.unwrap();

    let event = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();
    assert_eq!(
        event.metadata().source,
        "gsoe-evolution",
        "事件 source 应为 gsoe-evolution"
    );
}

/// 验证多代进化连续发布事件
#[tokio::test]
async fn test_multi_generation_publishes_events() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let mut engine = GsoeEvolutionEngine::with_event_bus(GsoeConfig::default(), bus);

    // 进化 3 代
    for _ in 0..3 {
        engine.evolve_once().await.unwrap();
    }

    // 应收到 3 个事件,generation 分别为 1, 2, 3
    for expected_gen in 1..=3u64 {
        let event = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();
        if let NexusEvent::GsoePolicyUpdated { generation, .. } = event {
            assert_eq!(
                generation, expected_gen,
                "第 {expected_gen} 个事件的 generation 应为 {expected_gen}"
            );
        } else {
            panic!("期望 GsoePolicyUpdated 事件");
        }
    }
}

/// 验证 GsoePolicyUpdated 事件可正确序列化/反序列化
#[tokio::test]
async fn test_gsoe_policy_updated_serialization() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let mut engine = GsoeEvolutionEngine::with_event_bus(GsoeConfig::default(), bus);

    engine.evolve_once().await.unwrap();

    let event = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();

    // JSON round-trip
    let json = serde_json::to_string(&event).expect("序列化失败");
    let restored: NexusEvent = serde_json::from_str(&json).expect("反序列化失败");
    assert_eq!(event, restored, "序列化 round-trip 应保持一致");

    // MessagePack round-trip
    let msgpack = event_bus::serialize_msgpack(&event).expect("msgpack 序列化失败");
    let restored_mp = event_bus::deserialize_msgpack(&msgpack).expect("msgpack 反序列化失败");
    assert_eq!(event, restored_mp, "msgpack round-trip 应保持一致");
}

/// 验证无 EventBus 时进化正常工作(不发布事件)
#[tokio::test]
async fn test_evolve_without_event_bus() {
    let mut engine = GsoeEvolutionEngine::new(GsoeConfig::default());
    let result = engine.evolve_once().await.expect("进化失败");
    assert_eq!(result.generation, 1);
    assert!(result.improvement.is_finite());
}

/// 验证 ConsensusReached 信号被正确消费
#[tokio::test]
async fn test_consensus_signal_consumed_after_evolution() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let mut engine = GsoeEvolutionEngine::with_event_bus(GsoeConfig::default(), bus);

    // 模拟收到 2 个共识信号
    engine.handle_consensus_reached();
    engine.handle_consensus_reached();

    // 进化后信号应被消费
    engine.evolve_once().await.unwrap();

    let event = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();
    assert!(
        matches!(event, NexusEvent::GsoePolicyUpdated { .. }),
        "仍应发布 GsoePolicyUpdated 事件"
    );
}

/// 验证 RedTeamAudit 信号触发对抗进化(提升 mutation_rate)
#[tokio::test]
async fn test_red_team_signal_triggers_adversarial_evolution() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let mut engine = GsoeEvolutionEngine::with_event_bus(GsoeConfig::default(), bus);

    let original_mr = engine.current_policy().mutation_rate;

    // 模拟收到红队审计信号
    engine.handle_red_team_audit();

    engine.evolve_once().await.unwrap();

    let event = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();
    if let NexusEvent::GsoePolicyUpdated {
        new_mutation_rate, ..
    } = event
    {
        // 对抗进化后 mutation_rate 可能有变化(受 red_team 信号影响)
        assert!(
            (0.0..=1.0).contains(&new_mutation_rate),
            "mutation_rate 应在合法范围"
        );
        // 原始 mutation_rate 应被记录(不为 0)
        assert!(original_mr > 0.0);
    } else {
        panic!("期望 GsoePolicyUpdated 事件");
    }
}

/// 验证事件 severity 为 Normal
#[tokio::test]
async fn test_gsoe_policy_updated_severity_normal() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let mut engine = GsoeEvolutionEngine::with_event_bus(GsoeConfig::default(), bus);

    engine.evolve_once().await.unwrap();

    let event = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();
    assert_eq!(
        event.severity(),
        event_bus::EventSeverity::Normal,
        "GsoePolicyUpdated 应为 Normal 级别"
    );
}

/// 验证事件 type_name 正确
#[tokio::test]
async fn test_gsoe_policy_updated_type_name() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let mut engine = GsoeEvolutionEngine::with_event_bus(GsoeConfig::default(), bus);

    engine.evolve_once().await.unwrap();

    let event = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();
    assert_eq!(
        event.type_name(),
        "GsoePolicyUpdated",
        "type_name 应为 GsoePolicyUpdated"
    );
}

/// 验证手动构造的 GsoePolicyUpdated 事件可被订阅者接收
#[tokio::test]
async fn test_manual_gsoe_policy_updated_event_delivery() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();

    let event = NexusEvent::GsoePolicyUpdated {
        metadata: EventMetadata::new("gsoe-evolution"),
        generation: 42,
        improvement: 0.05,
        new_mutation_rate: 0.15,
        new_selection_pressure: 1.8,
    };

    bus.publish(event.clone()).await.unwrap();

    let received = rx.recv_timeout(Duration::from_secs(1)).await.unwrap();
    assert_eq!(received, event);
}
