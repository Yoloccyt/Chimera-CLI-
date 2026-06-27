//! 事件总线集成测试 — 覆盖发布订阅、广播、背压、序列化、吞吐基准
//!
//! 这些测试通过 event_bus crate 的公开 API 验证端到端行为,
//! 单元测试见各模块的 #[cfg(test)] 子模块。

use std::time::{Duration, Instant};

use event_bus::{
    is_critical_event, BackpressurePolicy, EventBus, EventBusError, EventMetadata, EventSeverity,
    NexusEvent, SlowConsumerDetector,
};

/// 构造一个普通测试事件
fn make_normal_event(id: u32) -> NexusEvent {
    NexusEvent::CacheHit {
        metadata: EventMetadata::new("scc-cache"),
        cache_key: format!("key-{id}"),
    }
}

/// 构造一个关键测试事件(CheckpointSaved)
fn make_critical_event(id: u32) -> NexusEvent {
    NexusEvent::CheckpointSaved {
        metadata: EventMetadata::new("quest-engine"),
        quest_id: format!("quest-{id}"),
        checkpoint_id: format!("ckpt-{id}"),
        memory_snapshot_hash: format!("hash-{id}"),
    }
}

// ============================================================
// 测试 1:基本发布订阅
// ============================================================
#[tokio::test]
async fn test_publish_subscribe() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();

    let event = make_normal_event(1);
    bus.publish(event.clone()).await.unwrap();

    let received = rx.recv().await.unwrap();
    assert_eq!(received, event, "收到的事件应与发布的一致");
}

// ============================================================
// 测试 2:多订阅者广播
// ============================================================
#[tokio::test]
async fn test_multiple_subscribers() {
    let bus = EventBus::new();
    let mut rx1 = bus.subscribe();
    let mut rx2 = bus.subscribe();
    let mut rx3 = bus.subscribe();

    let event = make_normal_event(2);
    bus.publish(event.clone()).await.unwrap();

    // 每个接收者独立收到一份拷贝
    assert_eq!(rx1.recv().await.unwrap(), event);
    assert_eq!(rx2.recv().await.unwrap(), event);
    assert_eq!(rx3.recv().await.unwrap(), event);
}

// ============================================================
// 测试 3:背压丢弃最旧(DropOldest / broadcast 默认行为)
// ============================================================
#[tokio::test]
async fn test_backpressure_drop_oldest() {
    // 容量设为 4,发布 10 个事件,慢消费者会丢失中间事件
    let bus = EventBus::with_capacity(4);
    let mut slow_rx = bus.subscribe();

    // 发布 10 个事件,超出容量,旧事件被覆盖
    for i in 0..10 {
        bus.publish(make_normal_event(i)).await.unwrap();
    }

    // 慢消费者应收到 Lagged 错误(broadcast 通道行为)
    let result = slow_rx.recv().await;
    assert!(
        matches!(result, Err(EventBusError::SlowConsumerDropped { .. })),
        "慢消费者应收到 SlowConsumerDropped 错误,实际: {result:?}"
    );

    // 错误后可继续接收最新事件(broadcast 自动跳过丢失部分)
    let latest = slow_rx.recv().await;
    assert!(latest.is_ok(), "跳过丢失后应能继续接收");
}

// ============================================================
// 测试 3b:慢消费者检测器生成告警事件
// ============================================================
#[tokio::test]
async fn test_slow_consumer_detector() {
    let mut detector = SlowConsumerDetector::new("sub-test", 5);

    // lag 低于阈值,无告警
    assert!(detector.record_lag(3).is_none());

    // lag 超过阈值,生成告警
    let alert = detector.record_lag(10).expect("应生成告警事件");
    match alert {
        NexusEvent::SlowConsumerDropped {
            subscriber_id,
            lag,
            dropped_count,
            ..
        } => {
            assert_eq!(subscriber_id, "sub-test");
            assert_eq!(lag, 10);
            assert_eq!(dropped_count, 10, "累计丢弃应等于 lag");
        }
        _ => panic!("应为 SlowConsumerDropped 事件"),
    }
}

// ============================================================
// 测试 4:事件序列化往返(MessagePack + JSON)
// ============================================================
#[test]
fn test_event_serialization() {
    let event = NexusEvent::ConsensusReached {
        metadata: EventMetadata::new("parliament"),
        quest_id: "q-consensus".into(),
        decision_hash: "dec-abc".into(),
        dpo_pair_id: Some("dpo-001".into()),
    };

    // MessagePack 往返(ADR-004 主序列化协议)
    let msgpack_bytes = event_bus::serialize_msgpack(&event).unwrap();
    let decoded_mp = event_bus::deserialize_msgpack(&msgpack_bytes).unwrap();
    assert_eq!(decoded_mp, event, "MessagePack 往返应保持一致");

    // JSON 往返(降级/调试通道)
    let json_str = event_bus::serialize_json(&event).unwrap();
    let decoded_json = event_bus::deserialize_json(&json_str).unwrap();
    assert_eq!(decoded_json, event, "JSON 往返应保持一致");

    // MessagePack 应比 JSON 更紧凑
    assert!(
        msgpack_bytes.len() < json_str.len(),
        "MessagePack 应比 JSON 更紧凑: mp={}B vs json={}B",
        msgpack_bytes.len(),
        json_str.len()
    );
}

// ============================================================
// 测试 5:吞吐基准 — 1000 事件/秒
// ============================================================
#[tokio::test]
async fn test_1000_events_per_second() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();

    let event_count = 1000u32;
    let start = Instant::now();

    // 发布 1000 个事件
    for i in 0..event_count {
        bus.publish(make_normal_event(i)).await.unwrap();
    }

    // 接收 1000 个事件
    for _ in 0..event_count {
        let _ = rx.recv().await.unwrap();
    }

    let elapsed = start.elapsed();
    // 验收标准:1000 事件应在 1 秒内完成(即 ≥1000 事件/秒)
    assert!(
        elapsed < Duration::from_secs(1),
        "1000 事件处理耗时 {elapsed:?} 超过 1 秒,未达 1000 事件/秒基准"
    );

    // 打印吞吐量供观察(不影响测试结果)
    let throughput = event_count as f64 / elapsed.as_secs_f64();
    eprintln!("吞吐基准: {throughput:.0} 事件/秒 (1000 事件耗时 {elapsed:?})");
}

// ============================================================
// 测试 6:关键事件优先级标注
// ============================================================
#[test]
fn test_critical_event_priority() {
    // 关键事件:CheckpointSaved
    let critical = make_critical_event(1);
    assert_eq!(critical.severity(), EventSeverity::Critical);
    assert!(is_critical_event(&critical), "CheckpointSaved 应为关键事件");

    // 关键事件:ConsensusReached(修正 V3/V4 违规的核心事件)
    let consensus = NexusEvent::ConsensusReached {
        metadata: EventMetadata::new("parliament"),
        quest_id: "q".into(),
        decision_hash: "h".into(),
        dpo_pair_id: None,
    };
    assert_eq!(consensus.severity(), EventSeverity::Critical);
    assert!(is_critical_event(&consensus));

    // 关键事件:SlowConsumerDropped(系统健康告警)
    let alert = NexusEvent::SlowConsumerDropped {
        metadata: EventMetadata::new("event-bus"),
        subscriber_id: "s".into(),
        lag: 100,
        dropped_count: 100,
    };
    assert_eq!(alert.severity(), EventSeverity::Critical);
    assert!(is_critical_event(&alert));

    // 普通事件:CacheHit
    let normal = make_normal_event(2);
    assert_eq!(normal.severity(), EventSeverity::Normal);
    assert!(!is_critical_event(&normal));
}

// ============================================================
// 测试 7:背压策略配置
// ============================================================
#[test]
fn test_backpressure_policy_config() {
    let lag_policy = BackpressurePolicy::LagThreshold { max_lag: 512 };
    assert_eq!(lag_policy.max_lag(), Some(512));
    assert_eq!(lag_policy.broadcast_capacity(), 1024);

    let drop_policy = BackpressurePolicy::DropOldest;
    assert_eq!(drop_policy.max_lag(), None);

    let mpsc_policy = BackpressurePolicy::CriticalMpsc {
        broadcast_capacity: 2048,
    };
    assert_eq!(mpsc_policy.broadcast_capacity(), 2048);
}

// ============================================================
// 测试 8:关键事件在总线上正常投递
// ============================================================
#[tokio::test]
async fn test_critical_event_delivery() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();

    let critical = make_critical_event(42);
    assert!(is_critical_event(&critical));

    bus.publish(critical.clone()).await.unwrap();
    let received = rx.recv().await.unwrap();
    assert_eq!(received, critical);
    assert!(is_critical_event(&received), "接收到的关键事件应保持标注");
}

// ============================================================
// 测试 9:通道关闭感知
// ============================================================
#[tokio::test]
async fn test_channel_closed_detection() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();

    // drop 所有 Sender(包括 bus 内部的)
    drop(bus);

    let result = rx.recv().await;
    assert!(
        matches!(result, Err(EventBusError::ChannelClosed)),
        "Sender 全部 drop 后应返回 ChannelClosed,实际: {result:?}"
    );
}

// ============================================================
// 测试 10:超时接收
// ============================================================
#[tokio::test]
async fn test_recv_timeout_behavior() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();

    let start = Instant::now();
    let result = rx.recv_timeout(Duration::from_millis(100)).await;
    let elapsed = start.elapsed();

    assert!(
        matches!(result, Err(EventBusError::RecvTimeout(_))),
        "无事件时应超时,实际: {result:?}"
    );
    assert!(
        elapsed >= Duration::from_millis(90),
        "应等待约 100ms,实际 {elapsed:?}"
    );
}

// ============================================================
// 测试 11-14:Week 3 新增事件变体的序列化往返
//
// WHY:Week 3 引入 HCW/CMT/KVBSR 三个 crate,它们通过 EventBus
// 跨层通信,必须确保新增的 4 个事件变体在 JSON/MessagePack
// 序列化往返中字段不丢失、类型不漂移。
// ============================================================

// 测试 11:ContextWindowSwitched(HCW 窗口切换)序列化往返
#[test]
fn test_context_window_switched_serialization() {
    let event = NexusEvent::ContextWindowSwitched {
        metadata: EventMetadata::new("hcw-window"),
        from_tier: "L0".into(),
        to_tier: "L1".into(),
        reason: "L0 capacity exceeded".into(),
    };

    // JSON 往返
    let json_str = event_bus::serialize_json(&event).unwrap();
    let decoded = event_bus::deserialize_json(&json_str).unwrap();
    assert_eq!(decoded, event, "JSON 往返应保持一致");

    // MessagePack 往返
    let msgpack_bytes = event_bus::serialize_msgpack(&event).unwrap();
    let decoded_mp = event_bus::deserialize_msgpack(&msgpack_bytes).unwrap();
    assert_eq!(decoded_mp, event, "MessagePack 往返应保持一致");

    // 验证 type_name 与 severity
    assert_eq!(event.type_name(), "ContextWindowSwitched");
    assert_eq!(event.severity(), EventSeverity::Normal);
}

// 测试 12:ContextCompressed(HCW 上下文压缩)序列化往返
#[test]
fn test_context_compressed_serialization() {
    let event = NexusEvent::ContextCompressed {
        metadata: EventMetadata::new("hcw-window"),
        original_size: 131072,
        compressed_size: 32768,
        ratio: 0.25,
    };

    let json_str = event_bus::serialize_json(&event).unwrap();
    let decoded = event_bus::deserialize_json(&json_str).unwrap();
    assert_eq!(decoded, event, "JSON 往返应保持一致");

    let msgpack_bytes = event_bus::serialize_msgpack(&event).unwrap();
    let decoded_mp = event_bus::deserialize_msgpack(&msgpack_bytes).unwrap();
    assert_eq!(decoded_mp, event, "MessagePack 往返应保持一致");

    assert_eq!(event.type_name(), "ContextCompressed");
    assert_eq!(event.severity(), EventSeverity::Normal);
}

// 测试 13:CapabilityTiered(CMT 能力分层迁移)序列化往返
#[test]
fn test_capability_tiered_serialization() {
    let event = NexusEvent::CapabilityTiered {
        metadata: EventMetadata::new("cmt-tiering"),
        capability_id: "cap-001".into(),
        from_tier: "Hot".into(),
        to_tier: "Warm".into(),
        reason: "decay priority below threshold".into(),
    };

    let json_str = event_bus::serialize_json(&event).unwrap();
    let decoded = event_bus::deserialize_json(&json_str).unwrap();
    assert_eq!(decoded, event, "JSON 往返应保持一致");

    let msgpack_bytes = event_bus::serialize_msgpack(&event).unwrap();
    let decoded_mp = event_bus::deserialize_msgpack(&msgpack_bytes).unwrap();
    assert_eq!(decoded_mp, event, "MessagePack 往返应保持一致");

    assert_eq!(event.type_name(), "CapabilityTiered");
    assert_eq!(event.severity(), EventSeverity::Normal);
}

// 测试 14:BlocksRebalanced(KVBSR 块重平衡)序列化往返
#[test]
fn test_blocks_rebalanced_serialization() {
    let event = NexusEvent::BlocksRebalanced {
        metadata: EventMetadata::new("kvbsr-router"),
        old_block_count: 15,
        new_block_count: 22,
    };

    let json_str = event_bus::serialize_json(&event).unwrap();
    let decoded = event_bus::deserialize_json(&json_str).unwrap();
    assert_eq!(decoded, event, "JSON 往返应保持一致");

    let msgpack_bytes = event_bus::serialize_msgpack(&event).unwrap();
    let decoded_mp = event_bus::deserialize_msgpack(&msgpack_bytes).unwrap();
    assert_eq!(decoded_mp, event, "MessagePack 往返应保持一致");

    assert_eq!(event.type_name(), "BlocksRebalanced");
    assert_eq!(event.severity(), EventSeverity::Normal);
}

// 测试 15:ToolsRouted(KVBSR 工具路由)序列化往返 — SubTask 17.3
#[test]
fn test_tools_routed_serialization() {
    let event = NexusEvent::ToolsRouted {
        metadata: EventMetadata::new("kvbsr-router"),
        routed_count: 8,
        top_tool: "tool-0-0".into(),
        routed_tools: vec!["tool-0-0".into(), "tool-0-1".into(), "tool-0-2".into()],
    };

    // JSON 往返
    let json_str = event_bus::serialize_json(&event).unwrap();
    let decoded = event_bus::deserialize_json(&json_str).unwrap();
    assert_eq!(decoded, event, "JSON 往返应保持一致");

    // MessagePack 往返
    let msgpack_bytes = event_bus::serialize_msgpack(&event).unwrap();
    let decoded_mp = event_bus::deserialize_msgpack(&msgpack_bytes).unwrap();
    assert_eq!(decoded_mp, event, "MessagePack 往返应保持一致");

    assert_eq!(event.type_name(), "ToolsRouted");
    assert_eq!(event.severity(), EventSeverity::Normal);
}

// 测试 16:ToolsRouted 向后兼容 — 旧格式(无 routed_tools)反序列化为空 Vec
//
// WHY SubTask 17.3:验证 `#[serde(default)]` 生效,旧格式数据(不含 routed_tools 字段)
// 能正常反序列化,且 routed_tools 默认为空 Vec,确保向后兼容不破坏现有消费者。
#[test]
fn test_tools_routed_backward_compatibility() {
    // 模拟旧格式 JSON(无 routed_tools 字段)
    let old_json = r#"{"type":"ToolsRouted","data":{"metadata":{"event_id":"0192b8a0-0000-7000-8000-000000000001","timestamp":"2026-06-24T00:00:00Z","source":"kvbsr-router"},"routed_count":5,"top_tool":"tool-old"}}"#;

    let decoded: NexusEvent = event_bus::deserialize_json(old_json).unwrap();
    match decoded {
        NexusEvent::ToolsRouted {
            routed_count,
            top_tool,
            routed_tools,
            ..
        } => {
            assert_eq!(routed_count, 5);
            assert_eq!(top_tool, "tool-old");
            // 旧格式无 routed_tools,#[serde(default)] 使其反序列化为空 Vec
            assert!(
                routed_tools.is_empty(),
                "旧格式数据反序列化后 routed_tools 应为空 Vec"
            );
        }
        other => panic!("期望 ToolsRouted,实际: {other:?}"),
    }
}

// 测试 17:MemoryTiered(MLC 记忆分层)序列化往返 — SubTask 17.4
#[test]
fn test_memory_tiered_serialization() {
    let event = NexusEvent::MemoryTiered {
        metadata: EventMetadata::new("mlc-engine"),
        tier: "L0".into(),
        item_count: 1,
        memory_id: Some("m-001".into()),
    };

    // JSON 往返
    let json_str = event_bus::serialize_json(&event).unwrap();
    let decoded = event_bus::deserialize_json(&json_str).unwrap();
    assert_eq!(decoded, event, "JSON 往返应保持一致");

    // MessagePack 往返
    let msgpack_bytes = event_bus::serialize_msgpack(&event).unwrap();
    let decoded_mp = event_bus::deserialize_msgpack(&msgpack_bytes).unwrap();
    assert_eq!(decoded_mp, event, "MessagePack 往返应保持一致");

    assert_eq!(event.type_name(), "MemoryTiered");
    assert_eq!(event.severity(), EventSeverity::Normal);
}

// 测试 18:MemoryTiered 向后兼容 — 旧格式(无 memory_id)反序列化为 None
//
// WHY SubTask 17.4:验证 `#[serde(default)]` 生效,旧格式数据(不含 memory_id 字段)
// 能正常反序列化,且 memory_id 默认为 None,确保向后兼容不破坏现有消费者。
#[test]
fn test_memory_tiered_backward_compatibility() {
    // 模拟旧格式 JSON(无 memory_id 字段)
    let old_json = r#"{"type":"MemoryTiered","data":{"metadata":{"event_id":"0192b8a0-0000-7000-8000-000000000002","timestamp":"2026-06-24T00:00:00Z","source":"mlc-engine"},"tier":"L1","item_count":10}}"#;

    let decoded: NexusEvent = event_bus::deserialize_json(old_json).unwrap();
    match decoded {
        NexusEvent::MemoryTiered {
            tier,
            item_count,
            memory_id,
            ..
        } => {
            assert_eq!(tier, "L1");
            assert_eq!(item_count, 10);
            // 旧格式无 memory_id,#[serde(default)] + Option 使其反序列化为 None
            assert_eq!(memory_id, None, "旧格式数据反序列化后 memory_id 应为 None");
        }
        other => panic!("期望 MemoryTiered,实际: {other:?}"),
    }
}

// ============================================================
// SubTask 17.2:Critical 事件无订阅者告警测试
// ============================================================

/// SubTask 17.2:验证 Critical 事件无订阅者时记录 warn 日志
///
/// 使用 `tracing_test::traced_test` 捕获 tracing 日志,
/// 验证发布 Critical 事件(CheckpointSaved)且无订阅者时,日志中包含 WARN 级别告警。
#[tracing_test::traced_test]
#[tokio::test]
async fn test_critical_event_no_subscriber_warns() {
    let bus = EventBus::new();
    assert_eq!(bus.subscriber_count(), 0, "应无订阅者");

    // 发布 Critical 事件(CheckpointSaved),无订阅者
    bus.publish(make_critical_event(1)).await.unwrap();

    // 验证 warn 日志已记录
    assert!(
        logs_contain("WARN"),
        "Critical 事件无订阅者时应记录 WARN 日志"
    );
    assert!(
        logs_contain("Critical 事件无订阅者"),
        "日志应包含 'Critical 事件无订阅者' 告警信息"
    );
    assert!(
        logs_contain("CheckpointSaved"),
        "日志应包含事件类型 CheckpointSaved"
    );
}

/// SubTask 17.2:验证 Normal 事件无订阅者时不记录 warn 日志
///
/// Normal 级事件无订阅者时静默丢弃,不产生 warn 日志(避免日志噪声)。
#[tracing_test::traced_test]
#[tokio::test]
async fn test_normal_event_no_subscriber_silent() {
    let bus = EventBus::new();
    assert_eq!(bus.subscriber_count(), 0, "应无订阅者");

    // 发布 Normal 事件(CacheHit),无订阅者
    bus.publish(make_normal_event(1)).await.unwrap();

    // 验证无 warn 日志(Normal 级静默丢弃)
    assert!(
        !logs_contain("Critical 事件无订阅者"),
        "Normal 事件无订阅者时不应记录 Critical 告警"
    );
}

/// SubTask 17.2:验证 Critical 事件有订阅者时不记录 warn 日志
///
/// 有订阅者时事件正常投递,无需告警。
#[tracing_test::traced_test]
#[tokio::test]
async fn test_critical_event_with_subscriber_no_warn() {
    let bus = EventBus::new();
    let _rx = bus.subscribe();
    assert_eq!(bus.subscriber_count(), 1, "应有 1 个订阅者");

    // 发布 Critical 事件,有订阅者
    bus.publish(make_critical_event(2)).await.unwrap();

    // 验证无 warn 日志(有订阅者,事件正常投递)
    assert!(
        !logs_contain("Critical 事件无订阅者"),
        "有订阅者时不应记录 'Critical 事件无订阅者' 告警"
    );
}

/// SubTask 17.2:验证 ConsensusReached(Critical)无订阅者时也告警
#[tracing_test::traced_test]
#[tokio::test]
async fn test_consensus_reached_no_subscriber_warns() {
    let bus = EventBus::new();
    assert_eq!(bus.subscriber_count(), 0);

    let event = NexusEvent::ConsensusReached {
        metadata: EventMetadata::new("parliament"),
        quest_id: "q-test".into(),
        decision_hash: "hash-test".into(),
        dpo_pair_id: None,
    };
    bus.publish(event).await.unwrap();

    assert!(
        logs_contain("WARN"),
        "ConsensusReached(Critical)无订阅者时应告警"
    );
    assert!(logs_contain("ConsensusReached"));
}

/// SubTask 17.2:验证 publish_blocking 同样对 Critical 事件无订阅者告警
#[tracing_test::traced_test]
#[test]
fn test_publish_blocking_critical_no_subscriber_warns() {
    let bus = EventBus::new();
    assert_eq!(bus.subscriber_count(), 0);

    bus.publish_blocking(make_critical_event(3)).unwrap();

    assert!(
        logs_contain("WARN"),
        "publish_blocking 发布 Critical 事件无订阅者时应告警"
    );
    assert!(logs_contain("同步发布"));
}
