//! 控制事件序列化/反序列化与 severity 测试 — Task 1 新增 4 个变体
//!
//! 覆盖 QuestCancelRequested / QuestCancelled /
//! QuestPriorityChanged / QuestPriorityAdjusted 的:
//! - MessagePack round-trip(经 crate 公开 API serialize_msgpack/deserialize_msgpack)
//! - severity() 返回 EventSeverity::Info(非 Critical,控制事件不阻断系统)
//! - new_priority 边界值(0 / 128 / 255)

use event_bus::{deserialize_msgpack, serialize_msgpack, EventMetadata, EventSeverity, NexusEvent};

/// 构造测试用元数据(单参数 source,与 crate 公开签名一致)
fn test_metadata() -> EventMetadata {
    EventMetadata::new("test-source")
}

// ============================================================
// QuestCancelRequested — L10 Interface → L9 Quest(取消请求)
// ============================================================
#[test]
fn quest_cancel_requested_roundtrip() {
    let event = NexusEvent::QuestCancelRequested {
        metadata: test_metadata(),
        quest_id: "quest-001".to_string(),
        requested_by: "tui-operator".to_string(),
    };
    let bytes = serialize_msgpack(&event).expect("序列化失败");
    let decoded: NexusEvent = deserialize_msgpack(&bytes).expect("反序列化失败");
    assert_eq!(event, decoded);
}

#[test]
fn quest_cancel_requested_severity_is_info() {
    let event = NexusEvent::QuestCancelRequested {
        metadata: test_metadata(),
        quest_id: "quest-001".to_string(),
        requested_by: "tui-operator".to_string(),
    };
    assert_eq!(event.severity(), EventSeverity::Info);
    // 控制事件不能被误判为 Critical(避免触发 mpsc 旁路投递)
    assert_ne!(event.severity(), EventSeverity::Critical);
}

// ============================================================
// QuestCancelled — L9 Quest → L10 Interface(取消反馈)
// ============================================================
#[test]
fn quest_cancelled_roundtrip() {
    let event = NexusEvent::QuestCancelled {
        metadata: test_metadata(),
        quest_id: "quest-001".to_string(),
        requested_by: "tui-operator".to_string(),
    };
    let bytes = serialize_msgpack(&event).expect("序列化失败");
    let decoded: NexusEvent = deserialize_msgpack(&bytes).expect("反序列化失败");
    assert_eq!(event, decoded);
}

#[test]
fn quest_cancelled_severity_is_info() {
    let event = NexusEvent::QuestCancelled {
        metadata: test_metadata(),
        quest_id: "quest-001".to_string(),
        requested_by: "tui-operator".to_string(),
    };
    assert_eq!(event.severity(), EventSeverity::Info);
}

// ============================================================
// QuestPriorityChanged — L10 Interface → L9 Quest(优先级变更请求)
// ============================================================
#[test]
fn quest_priority_changed_roundtrip() {
    let event = NexusEvent::QuestPriorityChanged {
        metadata: test_metadata(),
        quest_id: "quest-001".to_string(),
        new_priority: 128,
        requested_by: "tui-operator".to_string(),
    };
    let bytes = serialize_msgpack(&event).expect("序列化失败");
    let decoded: NexusEvent = deserialize_msgpack(&bytes).expect("反序列化失败");
    assert_eq!(event, decoded);
}

#[test]
fn quest_priority_changed_severity_is_info() {
    let event = NexusEvent::QuestPriorityChanged {
        metadata: test_metadata(),
        quest_id: "quest-001".to_string(),
        new_priority: 128,
        requested_by: "tui-operator".to_string(),
    };
    assert_eq!(event.severity(), EventSeverity::Info);
}

/// new_priority 边界值:0(最低)
#[test]
fn quest_priority_changed_boundary_zero() {
    let event = NexusEvent::QuestPriorityChanged {
        metadata: test_metadata(),
        quest_id: "quest-001".to_string(),
        new_priority: 0,
        requested_by: "tui-operator".to_string(),
    };
    let bytes = serialize_msgpack(&event).expect("序列化失败");
    let decoded: NexusEvent = deserialize_msgpack(&bytes).expect("反序列化失败");
    assert_eq!(event, decoded);
    assert_eq!(decoded.severity(), EventSeverity::Info);
}

/// new_priority 边界值:255(最高)
#[test]
fn quest_priority_changed_boundary_max() {
    let event = NexusEvent::QuestPriorityChanged {
        metadata: test_metadata(),
        quest_id: "quest-001".to_string(),
        new_priority: 255,
        requested_by: "tui-operator".to_string(),
    };
    let bytes = serialize_msgpack(&event).expect("序列化失败");
    let decoded: NexusEvent = deserialize_msgpack(&bytes).expect("反序列化失败");
    assert_eq!(event, decoded);
}

// ============================================================
// QuestPriorityAdjusted — L9 Quest → L10 Interface(优先级调整反馈)
// ============================================================
#[test]
fn quest_priority_adjusted_roundtrip() {
    let event = NexusEvent::QuestPriorityAdjusted {
        metadata: test_metadata(),
        quest_id: "quest-001".to_string(),
        new_priority: 128,
        requested_by: "tui-operator".to_string(),
    };
    let bytes = serialize_msgpack(&event).expect("序列化失败");
    let decoded: NexusEvent = deserialize_msgpack(&bytes).expect("反序列化失败");
    assert_eq!(event, decoded);
}

#[test]
fn quest_priority_adjusted_severity_is_info() {
    let event = NexusEvent::QuestPriorityAdjusted {
        metadata: test_metadata(),
        quest_id: "quest-001".to_string(),
        new_priority: 128,
        requested_by: "tui-operator".to_string(),
    };
    assert_eq!(event.severity(), EventSeverity::Info);
}

/// new_priority 边界值:0 与 255 反序列化后字段保持一致
#[test]
fn quest_priority_adjusted_boundary_values() {
    for priority in [0u8, 128, 255] {
        let event = NexusEvent::QuestPriorityAdjusted {
            metadata: test_metadata(),
            quest_id: "quest-001".to_string(),
            new_priority: priority,
            requested_by: "tui-operator".to_string(),
        };
        let bytes = serialize_msgpack(&event).expect("序列化失败");
        let decoded: NexusEvent = deserialize_msgpack(&bytes).expect("反序列化失败");
        assert_eq!(event, decoded, "priority={priority} round-trip 失败");
    }
}
