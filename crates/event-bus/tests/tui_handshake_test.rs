//! TUI ↔ 编排器协议握手事件测试 — Concord W10 T10.1(ADR-082)
//!
//! 对应架构层:L1 Core(event-bus)
//!
//! # 覆盖
//! - 单表分类真值源:NexusEvent 的 TuiHello/TuiHelloAck type_name 稳定
//!   且 severity 恒为 Info(不随 CompatLevel 三态升降级);
//! - MessagePack 序列化往返(ADR-004 协议;三态 CompatLevel 全覆盖);
//! - topic 映射 System + severity Info(一次性信道建立事件);
//! - 握手幂等构造:同参数构造等值(proptest 守卫字段守恒)。

#![forbid(unsafe_code)]

use event_bus::{CompatLevel, EventMetadata, EventSeverity, EventTopic, NexusEvent};

fn hello() -> NexusEvent {
    NexusEvent::TuiHello {
        metadata: EventMetadata::new("chimera-tui"),
        proto: "1.0.0".into(),
        tui_version: "2.26.0".into(),
        caps: vec!["orchestrated-commands".into(), "agent-tree".into()],
    }
}

fn ack(compat: CompatLevel) -> NexusEvent {
    NexusEvent::TuiHelloAck {
        metadata: EventMetadata::new("chimera-cli"),
        proto: "1.0.0".into(),
        compat,
        server_version: "2.26.0".into(),
    }
}

// ============================================================
// A. 单表分类真值源:type_name / severity
// ============================================================

/// 原 `dual_table_type_names_match`(依赖 event_types.rs 分层子枚举镜像做双表
/// 比对)。镜像退役后改为单表断言,并把 TuiHelloAck 扩到 CompatLevel 三态,
/// 守卫"变体标签不随载荷取值漂移"。
#[test]
fn handshake_type_names_are_stable() {
    let pairs = [
        (hello(), "TuiHello"),
        (ack(CompatLevel::Full), "TuiHelloAck"),
        (ack(CompatLevel::Refused), "TuiHelloAck"),
        (
            ack(CompatLevel::Degraded(vec!["agent-tree".into()])),
            "TuiHelloAck",
        ),
    ];
    for (event, expected) in pairs {
        assert_eq!(event.type_name(), expected, "NexusEvent type_name 漂移");
    }
}

/// 原 `dual_table_severity_matches`。握手是一次性信道建立事件,severity 必须
/// 恒为 Info 且不随 CompatLevel 三态升降级;额外显式断言不得落入 Critical —
/// Critical 会占用 mpsc 旁路配额并触发"必须确保投递"语义(§6.2 红线)。
#[test]
fn handshake_severity_is_info_across_compat_levels() {
    assert_eq!(hello().severity(), EventSeverity::Info);
    for compat in [
        CompatLevel::Full,
        CompatLevel::Degraded(vec!["agent-tree".into(), "multi-model".into()]),
        CompatLevel::Refused,
    ] {
        assert_eq!(
            ack(compat).severity(),
            EventSeverity::Info,
            "CompatLevel 变化不应影响握手 severity"
        );
    }
    assert!(
        !matches!(hello().severity(), EventSeverity::Critical),
        "握手事件不得占用 Critical 旁路配额"
    );
    assert!(
        !matches!(
            ack(CompatLevel::Refused).severity(),
            EventSeverity::Critical
        ),
        "握手事件不得占用 Critical 旁路配额"
    );
}

// ============================================================
// B. topic 映射与元数据
// ============================================================

#[test]
fn handshake_events_map_to_system_topic() {
    assert_eq!(
        hello().topic(),
        EventTopic::System,
        "TuiHello 应归 System 主题"
    );
    assert_eq!(
        ack(CompatLevel::Degraded(vec!["agent-tree".into()])).topic(),
        EventTopic::System,
        "TuiHelloAck 应归 System 主题"
    );
}

#[test]
fn metadata_accessible() {
    assert_eq!(hello().metadata().source, "chimera-tui");
    assert_eq!(ack(CompatLevel::Full).metadata().source, "chimera-cli");
}

// ============================================================
// C. MessagePack 序列化往返(ADR-004)
// ============================================================

#[test]
fn msgpack_roundtrip_hello() {
    let event = hello();
    // WHY to_vec_named:NexusEvent 为相邻标签枚举(#[serde(tag="type", content="data")]),
    // compact 序列形态无法反序列化为 struct variant,需 map(命名)形态往返
    let bytes = rmp_serde::to_vec_named(&event).expect("serialize TuiHello");
    let back: NexusEvent = rmp_serde::from_slice(&bytes).expect("deserialize TuiHello");
    assert_eq!(back.type_name(), "TuiHello");
    match back {
        NexusEvent::TuiHello {
            proto,
            tui_version,
            caps,
            ..
        } => {
            assert_eq!(proto, "1.0.0");
            assert_eq!(tui_version, "2.26.0");
            assert_eq!(caps.len(), 2, "caps 字段守恒");
        }
        other => panic!("往返后变体漂移: {other:?}"),
    }
}

#[test]
fn msgpack_roundtrip_ack_all_compat_levels() {
    // 三态 CompatLevel 全覆盖(Degraded 携列表为关键载荷)
    for compat in [
        CompatLevel::Full,
        CompatLevel::Degraded(vec!["agent-tree".into(), "multi-model".into()]),
        CompatLevel::Refused,
    ] {
        let event = ack(compat.clone());
        let bytes = rmp_serde::to_vec_named(&event).expect("serialize TuiHelloAck");
        let back: NexusEvent = rmp_serde::from_slice(&bytes).expect("deserialize TuiHelloAck");
        match back {
            NexusEvent::TuiHelloAck {
                compat: got,
                server_version,
                ..
            } => {
                assert_eq!(got, compat, "CompatLevel 往返漂移");
                assert_eq!(server_version, "2.26.0");
            }
            other => panic!("往返后变体漂移: {other:?}"),
        }
    }
}

// ============================================================
// D. proptest:握手事件构造字段守恒
// ============================================================

mod proptests {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// 任意 proto/版本/caps 构造的 TuiHello 经 MessagePack 往返字段守恒
        #[test]
        fn hello_roundtrip_preserves_fields(
            proto in "[0-9]{1,2}\\.[0-9]{1,2}\\.[0-9]{1,2}",
            version in "[0-9]{1,2}\\.[0-9]{1,2}\\.[0-9]{1,2}",
            caps in proptest::collection::vec("[a-z-]{1,20}", 0..8),
        ) {
            let event = NexusEvent::TuiHello {
                metadata: EventMetadata::new("chimera-tui"),
                proto: proto.clone(),
                tui_version: version.clone(),
                caps: caps.clone(),
            };
            let bytes = rmp_serde::to_vec_named(&event).expect("serialize");
            let back: NexusEvent = rmp_serde::from_slice(&bytes).expect("deserialize");
            match back {
                NexusEvent::TuiHello { proto: p, tui_version: v, caps: c, .. } => {
                    prop_assert_eq!(p, proto);
                    prop_assert_eq!(v, version);
                    prop_assert_eq!(c, caps);
                }
                _ => prop_assert!(false, "变体漂移"),
            }
        }
    }
}
