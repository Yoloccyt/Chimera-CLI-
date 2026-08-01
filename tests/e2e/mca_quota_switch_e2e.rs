//! MCA M3 配额耗尽跨通道切换 E2E(ADR-068 熔断降级链)
//!
//! 对应计划:M3 T3.4 —— 通道 A 耗尽 → B 接管 → 会话连续性校验
//!
//! # 全链路闭环
//! 1. `AffinityQuotaExhausted`(Critical)经 event-bus mpsc 旁路确保送达
//! 2. csn `select_substitute` 按 CapabilitySet 能力相似度选替代通道
//! 3. session `migrate_history` 按新通道守恒策略迁移会话(思考块转译/丢弃留痕)
//! 4. 验证会话连续性:可见文本内容跨通道切换不丢
//!
//! # feature 门控(ADR-065 决策 6)
//! 本 E2E 门控于 `mca` feature,仅在 CI `--features mca` 旁路 job 运行,
//! 默认轨编译为空(双轨隔离,主干恒绿)。
#![cfg(feature = "mca")]

use event_bus::{EventBus, EventMetadata, NexusEvent};
use mca_gateway::migrate_history;
use nexus_contracts::affinity::{
    AffinityMessage, CapabilitySet, ContentBlock, MessageRole, StatePreservationPolicy,
    ThinkingSupport,
};

/// 构造能力集(测试辅助)
fn caps(
    tool_calling: bool,
    thinking: ThinkingSupport,
    window: u32,
    state: StatePreservationPolicy,
) -> CapabilitySet {
    let mut c = CapabilitySet::minimal_text(window, 8192);
    c.tool_calling = tool_calling;
    c.thinking = thinking;
    c.state_preservation = state;
    c
}

#[tokio::test]
async fn quota_exhausted_delivered_via_critical_mpsc() {
    // 步骤 1:AffinityQuotaExhausted 必须经 Critical mpsc 旁路送达(不因 broadcast Lagged 丢失)
    let bus = EventBus::new();
    let mut critical_rx = bus.subscribe_critical_events();
    bus.publish(NexusEvent::AffinityQuotaExhausted {
        metadata: EventMetadata::new("mca-gateway"),
        route_key: "deep_seek/deepseek-v4-flash".into(),
        reason: "monthly quota exhausted".into(),
    })
    .await
    .unwrap();

    let received = tokio::time::timeout(std::time::Duration::from_millis(200), critical_rx.recv())
        .await
        .expect("不应超时")
        .expect("Critical 旁路必须送达 AffinityQuotaExhausted");
    assert_eq!(received.type_name(), "AffinityQuotaExhausted");
}

#[tokio::test]
async fn quota_switch_full_chain_preserves_session_continuity() {
    // ---- 步骤 1:通道 A(DeepSeek,隐式缓存)配额耗尽 ----
    let bus = EventBus::new();
    let mut critical_rx = bus.subscribe_critical_events();
    let exhausted_key = "deep_seek/deepseek-v4-flash";
    bus.publish(NexusEvent::AffinityQuotaExhausted {
        metadata: EventMetadata::new("mca-gateway"),
        route_key: exhausted_key.into(),
        reason: "429 quota".into(),
    })
    .await
    .unwrap();
    let evt = tokio::time::timeout(std::time::Duration::from_millis(200), critical_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(evt.type_name(), "AffinityQuotaExhausted");

    // ---- 步骤 2:csn 按能力相似度选替代通道 ----
    // 耗尽通道能力:工具 + OnOff 思考 + 1M 窗口 + None 守恒(DeepSeek)
    let exhausted_caps = caps(
        true,
        ThinkingSupport::OnOff,
        1_000_000,
        StatePreservationPolicy::None,
    );
    let candidates = vec![
        // 候选 A:GLM(工具 + EffortLevels + 1M + BlockPreservation)—— 能力接近
        (
            "zhipu/glm-5.2".to_string(),
            caps(
                true,
                ThinkingSupport::EffortLevels(vec!["low".into(), "high".into()]),
                1_000_000,
                StatePreservationPolicy::BlockPreservation,
            ),
        ),
        // 候选 B:Step(缺工具 + 256K 小窗口)—— 能力差,距离大
        (
            "step_fun/step-3.5-flash-2603".to_string(),
            caps(
                false,
                ThinkingSupport::OnOff,
                262_144,
                StatePreservationPolicy::None,
            ),
        ),
    ];
    let substitute =
        csn_substitutor::select_substitute(&exhausted_caps, &candidates).expect("必须选出替代通道");
    assert_eq!(substitute, "zhipu/glm-5.2", "应选能力最相似的 GLM 接管");

    // ---- 步骤 3:会话状态按新通道(GLM,BlockPreservation)迁移 ----
    // 会话历史:含思考块 + 可见文本 + 工具调用
    let sentinel = "<<CONTINUITY-2026>>";
    let history = vec![
        AffinityMessage {
            role: MessageRole::User,
            blocks: vec![ContentBlock::Text {
                text: format!("{sentinel}用户问题{sentinel}").into(),
            }],
        },
        AffinityMessage {
            role: MessageRole::Assistant,
            blocks: vec![
                ContentBlock::Thinking {
                    thinking: "耗尽前的推理".into(),
                    signature: None,
                },
                ContentBlock::Text {
                    text: "耗尽前的回答".into(),
                },
            ],
        },
    ];
    // 新通道 GLM 为 BlockPreservation:思考块保留
    let migrated = migrate_history(&history, StatePreservationPolicy::BlockPreservation);
    assert_eq!(
        migrated.dropped_thinking_blocks, 0,
        "BlockPreservation 不丢思考块"
    );

    // ---- 步骤 4:会话连续性校验 ----
    // 用户消息(含哨兵)逐字幸存;可见文本跨通道切换不丢
    let user_survived = migrated.messages[0].blocks.iter().any(|b| {
        matches!(
            b,
            ContentBlock::Text { text } if text.contains(sentinel)
        )
    });
    assert!(user_survived, "用户消息哨兵必须跨通道切换幸存(会话连续性)");
    let assistant_text_survived = migrated.messages[1].blocks.iter().any(|b| {
        matches!(
            b,
            ContentBlock::Text { text } if text.as_ref() == "耗尽前的回答"
        )
    });
    assert!(assistant_text_survived, "助手可见文本必须跨通道幸存");
}

#[tokio::test]
async fn quota_switch_to_none_channel_drops_thinking_with_trace() {
    // 降级到 None 守恒通道(如 DeepSeek→豆包):思考块安全丢弃并留痕,可见内容保留
    let history = vec![AffinityMessage {
        role: MessageRole::Assistant,
        blocks: vec![
            ContentBlock::Thinking {
                thinking: "将被安全丢弃的思考".into(),
                signature: None,
            },
            ContentBlock::Text {
                text: "保留的可见回答".into(),
            },
        ],
    }];
    let migrated = migrate_history(&history, StatePreservationPolicy::None);
    // 留痕:丢弃 1 个思考块(驱动 E4 明确告知)
    assert_eq!(migrated.dropped_thinking_blocks, 1);
    // 可见文本保留(会话连续性:降级不丢可见内容)
    assert!(migrated.messages[0]
        .blocks
        .iter()
        .any(|b| matches!(b, ContentBlock::Text { text } if text.as_ref() == "保留的可见回答")));
    assert!(!migrated.messages[0]
        .blocks
        .iter()
        .any(|b| matches!(b, ContentBlock::Thinking { .. })));
}
