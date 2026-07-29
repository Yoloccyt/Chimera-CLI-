//! NexusEvent proptest 属性测试 — severity / type_name / topic 不变量
//!
//! 对应任务: T6-6 proptest 属性测试集成
//! 架构层: L1 Core (event-bus)
//!
//! # 验证的不变量
//! 1. severity() 幂等性 — 同一事件多次调用结果相同
//! 2. type_name() 非空 — 所有事件变体的类型名不为空字符串
//! 3. topic() 分类完整性 — 每个事件的 topic 在合法的 EventTopic 集合内
//!
//! # 语法约束(§4.4)
//! proptest 1.11+ 用 block-named 语法: `fn name(arg in strategy) { body }`

#![forbid(unsafe_code)]

use event_bus::topic::EventTopic;
use event_bus::types::{EventMetadata, NexusEvent};
use proptest::prelude::*;

/// 构造一个代表性的 NexusEvent(覆盖全部 10 个 topic × 3 个 severity 级别)
///
/// WHY 代表性构造而非穷举: NexusEvent 有 90+ 变体,全部构造代码量过大。
/// 选取每个 topic 至少一个代表变体 + Critical/Info/Normal 各至少一个,
/// 确保 proptest 随机选择时能覆盖全部分类路径。
fn make_event(variant: u8) -> NexusEvent {
    let m = || EventMetadata::new("proptest");
    match variant % 20 {
        0 => NexusEvent::UserIntentEncoded {
            metadata: m(),
            intent_id: "i-1".into(),
            raw_text: "hello".into(),
            risk_level: 50,
        },
        1 => NexusEvent::QuestCreated {
            metadata: m(),
            quest_id: "q-1".into(),
            title: "test".into(),
            task_count: 3,
        },
        2 => NexusEvent::CheckpointSaved {
            metadata: m(),
            quest_id: "q-1".into(),
            checkpoint_id: "c-1".into(),
            memory_snapshot_hash: "abc123".into(),
        },
        3 => NexusEvent::ConsensusReached {
            metadata: m(),
            quest_id: "q-1".into(),
            decision_hash: "d-1".into(),
            dpo_pair_id: None,
        },
        4 => NexusEvent::ExpertRegistered {
            metadata: m(),
            tool_id: "t-1".into(),
        },
        5 => NexusEvent::NmcEncoded {
            metadata: m(),
            modality: "Text".into(),
            content_hash: "h".into(),
            clv_dimension: 512,
        },
        6 => NexusEvent::CapabilityFrozen {
            metadata: m(),
            capability_id: "cap-1".into(),
            reason: "test".into(),
        },
        7 => NexusEvent::OperationProduced {
            metadata: m(),
            op_id: "op-1".into(),
            content_hash: "hash".into(),
        },
        8 => NexusEvent::VoteCast {
            metadata: m(),
            proposal_id: "p-1".into(),
            voter: "v-1".into(),
            vote: true,
        },
        9 => NexusEvent::CacheHit {
            metadata: m(),
            cache_key: "k-1".into(),
        },
        10 => NexusEvent::WikiUpdated {
            metadata: m(),
            wiki_hash: "wh".into(),
            delta: 5,
        },
        11 => NexusEvent::SlowConsumerDropped {
            metadata: m(),
            subscriber_id: "s-1".into(),
            lag: 10,
            dropped_count: 5,
        },
        12 => NexusEvent::SkepticVeto {
            metadata: m(),
            quest_id: "q-1".into(),
            veto_reason: "unsafe".into(),
            frozen_capabilities: vec![],
        },
        13 => NexusEvent::BudgetExceeded {
            metadata: m(),
            budget_type: "token".into(),
            current: 1000,
            limit: 500,
        },
        14 => NexusEvent::OrphanCallDetected {
            metadata: m(),
            operation_id: "orphan-1".into(),
            spawn_location: "test.rs:42".into(),
        },
        15 => NexusEvent::QuestCancelRequested {
            metadata: m(),
            quest_id: "q-1".into(),
            requested_by: "user".into(),
        },
        16 => NexusEvent::TuiActionRequested {
            metadata: m(),
            action_id: "quest.plan".into(),
            payload: "{}".into(),
            source: event_bus::types::ActionSource::Palette,
        },
        17 => NexusEvent::AgentTaskFailed {
            metadata: m(),
            from: "root".into(),
            to: "sub-1".into(),
            task_id: "t-1".into(),
            error: "timeout".into(),
            retry_count: 0,
        },
        18 => NexusEvent::AsaIntervention {
            metadata: m(),
            operation_id: "op-1".into(),
            action: "Block".into(),
            safety_score: 0.2,
            block_reason: Some("injection".into()),
            alternative_suggestion: None,
        },
        _ => NexusEvent::RedTeamAudit {
            metadata: m(),
            vulnerability_type: "prompt_injection".into(),
            failed_probes: 3,
            total_probes: 10,
            detection_rate: 0.3,
            remediation_suggestion: "sanitize".into(),
        },
    }
}

proptest! {
    /// 不变量 1: severity() 幂等性 — 同一事件多次调用 severity() 结果相同
    ///
    /// WHY: severity() 是纯函数(无内部可变状态),幂等性是纯函数的基本性质。
    /// 若幂等性被破坏,说明内部有副作用,背压策略可能做出不一致的决策。
    #[test]
    fn prop_severity_idempotent(variant in 0u8..20u8) {
        let event = make_event(variant);
        let s1 = event.severity();
        let s2 = event.severity();
        prop_assert_eq!(s1, s2, "severity() must be idempotent for variant {}", variant);
    }

    /// 不变量 2: type_name() 非空 — 所有事件变体的类型名长度 > 0
    ///
    /// WHY: type_name() 用作序列化 tag 与日志标识,空字符串会导致
    /// 日志无法区分事件类型、序列化歧义。
    #[test]
    fn prop_type_name_non_empty(variant in 0u8..20u8) {
        let event = make_event(variant);
        let name = event.type_name();
        prop_assert!(
            !name.is_empty(),
            "type_name() must not be empty for variant {} (got {:?})",
            variant, event
        );
    }

    /// 不变量 3: topic() 分类完整性 — 返回的 topic 在 EventTopic::all() 集合内
    ///
    /// WHY: topic() 用于 FilteredSubscriber 选择性订阅。若返回不在合法集合内的
    /// 值,订阅者永远收不到对应事件,导致消息静默丢失。
    #[test]
    fn prop_topic_in_valid_set(variant in 0u8..20u8) {
        let event = make_event(variant);
        let topic = event.topic();
        let all = EventTopic::all();
        prop_assert!(
            all.contains(&topic),
            "topic() returned {:?} which is not in EventTopic::all() for variant {}",
            topic, variant
        );
    }
}
