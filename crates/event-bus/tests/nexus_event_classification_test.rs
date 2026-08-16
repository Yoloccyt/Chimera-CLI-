//! NexusEvent 分类集成测试 — severity()/type_name()/metadata() 公共 API 守护(P1-3 测试外移)
//!
//! 对应架构层:L1 Core(event-bus)
//!
//! # 职责(WHY 本文件存在)
//! types.rs 的 severity()/type_name() 两大 match 已外移至 classification.rs。
//! 本集成测试从**消费方视角**(仅用公共 API `event_bus::*`)守护分类行为,
//! 作为内联穷举测试之外的独立回归面:
//! - Critical 档:安全/状态红线事件(背压不可丢弃)
//! - Info 档:控制/交互请求事件(不占 mpsc 旁路)
//! - Normal 档:观测/高频流式事件(走 broadcast 通配符)
//! - type_name 与 metadata 的稳定性与可取性
//!
//! # 架构红线对齐
//! severity() 判定逻辑留在 event-bus(Critical mpsc 保障);本测试与
//! bus.rs `is_critical_mpsc_event` 双清单守护互补,任何 Critical 清单
//! 漂移会同时触发本文件与内联穷举测试失败。

use event_bus::{ActionSource, EventMetadata, EventSeverity, NexusEvent};

// ---------------------------------------------------------------
// metadata() 基础
// ---------------------------------------------------------------

#[test]
fn metadata_creation_and_access() {
    let meta = EventMetadata::new("osa-coordinator");
    assert_eq!(meta.source, "osa-coordinator");
    assert!(!meta.event_id.to_string().is_empty());

    let e = NexusEvent::NexusStateChanged {
        metadata: EventMetadata::new("quest-engine"),
        state_hash: "h".into(),
        prev_hash: "".into(),
    };
    assert_eq!(e.metadata().source, "quest-engine");
}

// ---------------------------------------------------------------
// severity() 三档分类
// ---------------------------------------------------------------

/// Critical 档:CheckpointSaved 为状态红线事件,背压场景不可丢弃
#[test]
fn critical_severity_checkpoint_saved() {
    let e = NexusEvent::CheckpointSaved {
        metadata: EventMetadata::new("quest-engine"),
        quest_id: "q1".into(),
        checkpoint_id: "c1".into(),
        memory_snapshot_hash: "abc".into(),
    };
    assert_eq!(e.severity(), EventSeverity::Critical);
}

/// Normal 档:CacheHit / NexusStateChanged / ModelRouteSelected 走通配符 Normal
#[test]
fn normal_severity_observational_events() {
    let cache_hit = NexusEvent::CacheHit {
        metadata: EventMetadata::new("scc-cache"),
        cache_key: "k1".into(),
    };
    assert_eq!(cache_hit.severity(), EventSeverity::Normal);

    let state_changed = NexusEvent::NexusStateChanged {
        metadata: EventMetadata::new("quest-engine"),
        state_hash: "h".into(),
        prev_hash: "".into(),
    };
    assert_eq!(state_changed.severity(), EventSeverity::Normal);

    let route = NexusEvent::ModelRouteSelected {
        metadata: EventMetadata::new("model-router"),
        quest_id: "q1".into(),
        model_id: "m1".into(),
        route_reason: "auto".into(),
    };
    assert_eq!(route.severity(), EventSeverity::Normal);
}

/// Info 档:TUI 交互请求;高频流式必须 Normal(不占 mpsc 旁路)
#[test]
fn tui_protocol_severity_info_vs_normal() {
    let requested = NexusEvent::TuiActionRequested {
        metadata: EventMetadata::new("chimera-tui"),
        action_id: "quest.pause".into(),
        payload: "{\"quest_id\":\"q1\"}".into(),
        source: ActionSource::Palette,
    };
    assert_eq!(requested.severity(), EventSeverity::Info);

    let chunk = NexusEvent::TuiChatResponseChunk {
        metadata: EventMetadata::new("chimera-cli"),
        session_id: "s1".into(),
        delta: "hello".into(),
        cursor_hint: 0,
    };
    assert_eq!(
        chunk.severity(),
        EventSeverity::Normal,
        "高频 token 流必须为 Normal,避免冲垮 mpsc 旁路"
    );
}

// ---------------------------------------------------------------
// type_name() 稳定性 + metadata 可取性(跨变体)
// ---------------------------------------------------------------

/// type_name 为稳定字符串标签,metadata.source 对所有变体可取
#[test]
fn type_name_stable_and_metadata_accessible() {
    let events: Vec<(NexusEvent, &str)> = vec![
        (
            NexusEvent::NexusStateChanged {
                metadata: EventMetadata::new("quest-engine"),
                state_hash: "h".into(),
                prev_hash: "".into(),
            },
            "NexusStateChanged",
        ),
        (
            NexusEvent::ModelRouteSelected {
                metadata: EventMetadata::new("model-router"),
                quest_id: "q".into(),
                model_id: "m".into(),
                route_reason: "r".into(),
            },
            "ModelRouteSelected",
        ),
        (
            NexusEvent::CheckpointSaved {
                metadata: EventMetadata::new("quest-engine"),
                quest_id: "q".into(),
                checkpoint_id: "c".into(),
                memory_snapshot_hash: "h".into(),
            },
            "CheckpointSaved",
        ),
        (
            NexusEvent::CacheHit {
                metadata: EventMetadata::new("scc-cache"),
                cache_key: "k".into(),
            },
            "CacheHit",
        ),
    ];
    for (e, expected_name) in &events {
        assert_eq!(e.type_name(), *expected_name, "type_name 应稳定");
        assert!(!e.metadata().source.is_empty(), "metadata.source 应可取");
    }
}
