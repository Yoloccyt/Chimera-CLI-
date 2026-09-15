//! define_event_registry! 属性测试 — 宏生成分类表与 serde 线格式一致性
//!
//! 对应任务: M5(架构重构方向 3-P1 同步点收敛)
//! 架构层: L1 Core (event-bus)
//!
//! # WHY
//! M5 后 severity()/type_name()/topic()/metadata() 由
//! `define_event_registry!` 单一声明点展开生成(types.rs/classification.rs/
//! topic.rs 三张手写巨型 match 已退役)。本文件钉住迁移后两条独立的
//! 不变量,作为 variant_count_test.rs 三层锁之外的**线格式回归网**:
//! 1. **分类表 ↔ serde 线格式同源**:`type_name()` 经 `stringify!` 生成,
//!    serde `type` tag 经 derive 生成,二者处于不同代码路径 —— proptest
//!    属性断言 JSON 线格式的 `type` 字段恒等于 `type_name()`,任一侧
//!    改名/漂移即红灯;
//! 2. **序列化往返恒等**:MessagePack(ADR-004 权威线格式)与 JSON 双格式
//!    往返必须恒等 —— serde 线格式零变更的机器可执行证明。
//!
//! 全量 145 变体的计数/分布/Critical 双清单互锁由
//! tests/variant_count_test.rs 守护(原样通过,本文件不改其检测手段);
//! 本文件代表性变体集横跨全部 10 类 topic × 3 级 severity,并逐变体
//! 钉死 (type_name, severity, topic) 三元组 —— 捕获"两个 Normal 变体
//! topic 互换"这类计数锁无法发现的错位。

#![forbid(unsafe_code)]

use event_bus::{
    deserialize_json, deserialize_msgpack, serialize_json, serialize_msgpack, ActionSource,
    EventMetadata, EventSeverity, EventTopic, NexusEvent,
};
use proptest::prelude::*;

/// 代表性变体构造器 — 字段形态镜像 tests/variant_count_test.rs 样本
/// (该文件是编译期契约,照抄其字段/字面量形态可保证类型正确)
fn representative_events() -> Vec<NexusEvent> {
    let meta = || EventMetadata::new("registry-property-test");
    vec![
        // ---- Quest ----
        NexusEvent::UserIntentEncoded {
            metadata: meta(),
            intent_id: "i-1".into(),
            raw_text: "raw".into(),
            risk_level: 1,
        },
        NexusEvent::CheckpointSaved {
            metadata: meta(),
            quest_id: "q-1".into(),
            checkpoint_id: "c-1".into(),
            memory_snapshot_hash: "h".into(),
        },
        NexusEvent::QuestCancelRequested {
            metadata: meta(),
            quest_id: "q-1".into(),
            requested_by: "op".into(),
        },
        // ---- Memory ----
        NexusEvent::NexusStateChanged {
            metadata: meta(),
            state_hash: "h".into(),
            prev_hash: "p".into(),
        },
        NexusEvent::NmcEncoded {
            metadata: meta(),
            modality: "Text".into(),
            content_hash: "h".into(),
            clv_dimension: 512,
        },
        NexusEvent::GhostMemoryDetected {
            metadata: meta(),
            ghost_rate: 0.5,
            ghost_count: 1,
            total_recalls: 1,
            current_strategy: "s".into(),
        },
        // ---- Security ----
        NexusEvent::SandboxViolation {
            metadata: meta(),
            violation_type: "t".into(),
            detail: "d".into(),
        },
        NexusEvent::SkepticVeto {
            metadata: meta(),
            quest_id: "q-1".into(),
            veto_reason: "r".into(),
            frozen_capabilities: vec![],
        },
        NexusEvent::ErrorSignatureMatched {
            metadata: meta(),
            error_hash: "h".into(),
            matched_card_ids: vec![],
        },
        // ---- Execution ----
        NexusEvent::OperationProduced {
            metadata: meta(),
            op_id: "o-1".into(),
            content_hash: "h".into(),
        },
        NexusEvent::OrphanCallDetected {
            metadata: meta(),
            operation_id: "o-1".into(),
            spawn_location: "l".into(),
        },
        // ---- Parliament ----
        NexusEvent::VoteCast {
            metadata: meta(),
            proposal_id: "p-1".into(),
            voter: "v-1".into(),
            vote: true,
        },
        NexusEvent::BudgetExceeded {
            metadata: meta(),
            budget_type: "t".into(),
            current: 1,
            limit: 1,
        },
        NexusEvent::StopRulingIssued {
            metadata: meta(),
            quest_id: "q-1".into(),
            reason: "r".into(),
            preserve_best: true,
        },
        // ---- System ----
        NexusEvent::SlowConsumerDropped {
            metadata: meta(),
            subscriber_id: "s-1".into(),
            lag: 1,
            dropped_count: 1,
        },
        NexusEvent::TuiActionRequested {
            metadata: meta(),
            request_id: "r-1".into(),
            action_id: "a-1".into(),
            payload: "p".into(),
            source: ActionSource::Palette,
        },
        NexusEvent::TuiChatResponseChunk {
            metadata: meta(),
            session_id: "s-1".into(),
            delta: "d".into(),
            cursor_hint: 1,
        },
        // ---- Knowledge ----
        NexusEvent::WikiUpdated {
            metadata: meta(),
            wiki_hash: "h".into(),
            delta: 1,
        },
        NexusEvent::R2FreezeViolation {
            metadata: meta(),
            violation_type: "t".into(),
            evidence: "e".into(),
        },
        // ---- Storage ----
        NexusEvent::CacheHit {
            metadata: meta(),
            cache_key: "k-1".into(),
        },
        NexusEvent::TokenLedgerRecorded {
            metadata: meta(),
            evidence_id: "e-1".into(),
            token_usage: 1,
        },
        // ---- Agent ----
        NexusEvent::AgentTaskFailed {
            metadata: meta(),
            from: "a".into(),
            to: "b".into(),
            task_id: "t-1".into(),
            error: "e".into(),
            retry_count: 1,
        },
        NexusEvent::DelegationCompleted {
            metadata: meta(),
            parent_id: "p-1".into(),
            quest_id: None,
            total_overhead_ms: 0.5,
            sub_task_count: 1,
            success_count: 1,
        },
    ]
}

/// 逐变体钉死 (type_name, severity, topic) 三元组 — 横跨 10 类 topic ×
/// 3 级 severity。防"两个同 severity 变体 topic 互换"这类计数锁盲区
/// (variant_count_test 只锁计数/分布/成员关系,不锁逐变体映射)。
#[test]
fn test_representative_classification_pinned() {
    // (type_name, severity, topic) — 与迁移前 classification.rs/topic.rs 逐行一致
    let pinned: Vec<(&str, EventSeverity, EventTopic)> = vec![
        (
            "UserIntentEncoded",
            EventSeverity::Normal,
            EventTopic::Quest,
        ),
        (
            "CheckpointSaved",
            EventSeverity::Critical,
            EventTopic::Quest,
        ),
        (
            "QuestCancelRequested",
            EventSeverity::Info,
            EventTopic::Quest,
        ),
        (
            "NexusStateChanged",
            EventSeverity::Normal,
            EventTopic::Memory,
        ),
        ("NmcEncoded", EventSeverity::Normal, EventTopic::Memory),
        (
            "GhostMemoryDetected",
            EventSeverity::Normal,
            EventTopic::Memory,
        ),
        (
            "SandboxViolation",
            EventSeverity::Normal,
            EventTopic::Security,
        ),
        ("SkepticVeto", EventSeverity::Critical, EventTopic::Security),
        (
            "ErrorSignatureMatched",
            EventSeverity::Critical,
            EventTopic::Security,
        ),
        (
            "OperationProduced",
            EventSeverity::Normal,
            EventTopic::Execution,
        ),
        (
            "OrphanCallDetected",
            EventSeverity::Critical,
            EventTopic::Execution,
        ),
        ("VoteCast", EventSeverity::Normal, EventTopic::Parliament),
        (
            "BudgetExceeded",
            EventSeverity::Critical,
            EventTopic::Parliament,
        ),
        (
            "StopRulingIssued",
            EventSeverity::Critical,
            EventTopic::Parliament,
        ),
        (
            "SlowConsumerDropped",
            EventSeverity::Critical,
            EventTopic::System,
        ),
        (
            "TuiActionRequested",
            EventSeverity::Info,
            EventTopic::System,
        ),
        (
            "TuiChatResponseChunk",
            EventSeverity::Normal,
            EventTopic::System,
        ),
        ("WikiUpdated", EventSeverity::Normal, EventTopic::Knowledge),
        (
            "R2FreezeViolation",
            EventSeverity::Critical,
            EventTopic::Knowledge,
        ),
        ("CacheHit", EventSeverity::Normal, EventTopic::Storage),
        (
            "TokenLedgerRecorded",
            EventSeverity::Normal,
            EventTopic::Storage,
        ),
        (
            "AgentTaskFailed",
            EventSeverity::Critical,
            EventTopic::Agent,
        ),
        (
            "DelegationCompleted",
            EventSeverity::Normal,
            EventTopic::Agent,
        ),
    ];
    let events = representative_events();
    assert_eq!(
        events.len(),
        pinned.len(),
        "representative_events 与 pinned 表必须一一对应"
    );
    for (event, (name, severity, topic)) in events.iter().zip(pinned.iter()) {
        assert_eq!(event.type_name(), *name, "type_name 漂移");
        assert_eq!(event.severity(), *severity, "{} severity 漂移", name);
        assert_eq!(event.topic(), *topic, "{} topic 漂移", name);
    }
    // type_name 唯一性:代表性集合内无重名(防构造器复制粘贴错位)
    let names: std::collections::HashSet<&str> = events.iter().map(|e| e.type_name()).collect();
    assert_eq!(names.len(), events.len(), "代表性集合 type_name 重复");
}

/// 任意事件策略 — 选取 String 字段为主的简单变体,proptest 生成任意输入
fn arb_event() -> impl Strategy<Value = NexusEvent> {
    let meta = || EventMetadata::new("proptest");
    prop_oneof![
        (any::<String>()).prop_map(move |cache_key| NexusEvent::CacheHit {
            metadata: meta(),
            cache_key,
        }),
        (any::<String>(), any::<String>(), any::<u8>()).prop_map(
            move |(intent_id, raw_text, risk_level)| NexusEvent::UserIntentEncoded {
                metadata: meta(),
                intent_id,
                raw_text,
                risk_level,
            }
        ),
        (any::<String>(), any::<String>()).prop_map(move |(violation_type, detail)| {
            NexusEvent::SandboxViolation {
                metadata: meta(),
                violation_type,
                detail,
            }
        }),
    ]
}

proptest! {
    /// 属性 1:MessagePack(ADR-004 权威线格式)+ JSON 双格式序列化往返恒等
    #[test]
    fn wire_roundtrip_identity(event in arb_event()) {
        let bytes = serialize_msgpack(&event).unwrap();
        prop_assert_eq!(&deserialize_msgpack(&bytes).unwrap(), &event);
        let json = serialize_json(&event).unwrap();
        prop_assert_eq!(&deserialize_json(&json).unwrap(), &event);
    }

    /// 属性 2:JSON 线格式 `type` tag 恒等于 type_name()
    /// (stringify! 宏路径与 serde derive 路径的同源性断言)
    #[test]
    fn serde_tag_equals_type_name(event in arb_event()) {
        let json = serialize_json(&event).unwrap();
        let value: serde_json::Value = serde_json::from_str(&json).unwrap();
        prop_assert_eq!(
            value["type"].as_str().unwrap(),
            event.type_name(),
            "serde 线格式 type tag 与 type_name() 漂移"
        );
    }
}
