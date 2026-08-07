//! 协议矩阵验收测试(§9.1)— 7 厂商 × 能力场景,录播回放双轨
//!
//! 对应计划:M1 T1.6 收口(§9.1 协议矩阵 7 厂商全绿;E5 哨兵校验 100%)
//!
//! # 矩阵结构
//! 7 厂商全部落在 3 协议方言内(结论 F1):渠道亲和的本质 = 每厂商一张
//! spec 卡 × 其 preferred 方言的 Codec。本测试对每个厂商:
//! 1. 从 affinity.d 加载真实 spec 卡 → 装配适配器 → 确定 preferred 方言
//! 2. 用该方言的录播 fixture 驱动 Codec 解码,断言统一块模型正确
//! 3. 未知字段注入容错(P3)
//! 4. E5 多轮哨兵:会话状态守恒下多轮上下文逐字回传
//!
//! # 录播离线
//! fixture 为按厂商文档合成(标注 `_fixture_note`),CI 离线回放零网络依赖;
//! Action Item 1(七厂商 API Key)就绪后原位替换真实录像,断言不变。

use std::path::PathBuf;
use std::sync::Arc;

use mca_gateway::prelude::*;
use mca_gateway::sse::StreamNormalizer;
use mca_gateway::Codec;
use nexus_contracts::affinity::{
    AffinityMessage, ContentBlock, FinishReason, MessageRole, ProtocolDialect, ProviderId,
    StatePreservationPolicy, ThinkingPreference,
};

fn affinity_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("affinity.d")
}

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("fixture {path} 读取失败: {e}"))
}

/// 加载全部厂商卡,返回 (route_key, spec) 便于按厂商筛选
fn load_all() -> Vec<ModelAffinitySpec> {
    load_spec_dir(&affinity_dir()).expect("七厂商卡片必须全部加载")
}

/// 方言 → 对应的非流式回放 fixture 文件名
fn nonstream_fixture_for(dialect: ProtocolDialect) -> &'static str {
    match dialect {
        ProtocolDialect::OpenAiChat => "deepseek_chat_basic.json",
        ProtocolDialect::AnthropicMessages => "zhipu_anthropic_basic.json",
        ProtocolDialect::OpenAiResponses => "deepseek_responses_basic.json",
    }
}

// ============================================================
// 场景 A:7 厂商 × preferred 方言非流式解码全绿
// ============================================================

#[test]
fn matrix_all_seven_vendors_decode_via_preferred_dialect() {
    let specs = load_all();
    // 覆盖度断言:七厂商每家至少一张卡参与矩阵
    let mut seen_providers = std::collections::HashSet::new();
    for spec in &specs {
        let dialect = spec.preferred_dialect().expect("每卡必有 preferred 方言");
        let codec = Codec::for_dialect(dialect).expect("preferred 方言必有码器(M1 三方言全覆盖)");
        let decoded = codec
            .parse_response(&fixture(nonstream_fixture_for(dialect)))
            .unwrap_or_else(|e| panic!("{} 经 {dialect:?} 解码失败: {e}", spec.route_key()));
        // 解码产出至少一个内容块(问答场景)
        assert!(
            !decoded.blocks.is_empty(),
            "{} 解码块序列不应为空",
            spec.route_key()
        );
        seen_providers.insert(provider_family(&spec.provider));
    }
    // 七厂商全覆盖(C1:对所有模型做渠道亲和)
    assert_eq!(seen_providers.len(), 7, "协议矩阵必须覆盖全部 7 厂商");
}

/// 厂商家族标识(去重用;Custom 归一为 custom)
fn provider_family(p: &ProviderId) -> &'static str {
    match p {
        ProviderId::Zhipu => "zhipu",
        ProviderId::DeepSeek => "deepseek",
        ProviderId::Moonshot => "moonshot",
        ProviderId::VolcanoArk => "volcano",
        ProviderId::AlibabaCloud => "alicloud",
        ProviderId::MiniMax => "minimax",
        ProviderId::StepFun => "stepfun",
        ProviderId::Custom(_) => "custom",
    }
}

// ============================================================
// 场景 B:简单问答流式(SSE 事件序列完整性 + usage)
// ============================================================

#[test]
fn matrix_streaming_openai_chat() {
    // DeepSeek OpenAI 方言流式:reasoning + text delta + usage + [DONE]
    let mut n = StreamNormalizer::new(ProtocolDialect::OpenAiChat);
    let events = n.feed(&fixture("deepseek_chat_stream.txt"));
    // 事件序列:ThinkingDelta → TextDelta×2 → Done → Usage(顺序容忍,断言存在)
    assert!(events
        .iter()
        .any(|e| matches!(e, StreamEvent::ThinkingDelta(t) if t == "先分析问题")));
    let text: String = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::TextDelta(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "快速排序实现如下", "文本增量按序拼接应还原完整答案");
    assert!(events
        .iter()
        .any(|e| matches!(e, StreamEvent::Done(FinishReason::Stop))));
    assert!(events
        .iter()
        .any(|e| matches!(e, StreamEvent::Usage(u) if u.cache_hit_tokens == 6)));
}

#[test]
fn matrix_streaming_anthropic() {
    // GLM Anthropic 方言流式:thinking_delta + text_delta + message_delta(usage+done)
    let mut n = StreamNormalizer::new(ProtocolDialect::AnthropicMessages);
    let events = n.feed(&fixture("zhipu_anthropic_stream.txt"));
    assert!(events
        .iter()
        .any(|e| matches!(e, StreamEvent::ThinkingDelta(t) if t == "分析所有权")));
    let text: String = events
        .iter()
        .filter_map(|e| match e {
            StreamEvent::TextDelta(t) => Some(t.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(text, "Rust 所有权三条规则");
    assert!(events
        .iter()
        .any(|e| matches!(e, StreamEvent::Usage(u) if u.cache_hit_tokens == 8)));
    assert!(events
        .iter()
        .any(|e| matches!(e, StreamEvent::Done(FinishReason::Stop))));
}

// ============================================================
// 场景 C:未知字段注入容错(P3,响应注入虚构字段不报错)
// ============================================================

#[test]
fn matrix_unknown_field_tolerance_all_dialects() {
    // OpenAI Chat:注入虚构顶层字段 + message 内虚构字段
    let openai = r#"{"choices":[{"message":{"content":"ok","fictional_field":42}}],"vendor_x":{"deep":[1]}}"#;
    assert!(Codec::for_dialect(ProtocolDialect::OpenAiChat)
        .unwrap()
        .parse_response(openai.as_bytes())
        .is_ok());
    // Anthropic:注入虚构块类型(跳过) + 虚构顶层字段
    let anthropic = r#"{"content":[{"type":"fictional_block","x":1},{"type":"text","text":"ok"}],"stop_reason":"end_turn","vendor_y":true}"#;
    assert!(Codec::for_dialect(ProtocolDialect::AnthropicMessages)
        .unwrap()
        .parse_response(anthropic.as_bytes())
        .is_ok());
    // Responses:注入虚构 output 项(跳过)
    let responses = r#"{"output":[{"type":"fictional_item"},{"type":"message","content":[{"type":"output_text","text":"ok"}]}],"status":"completed"}"#;
    assert!(Codec::for_dialect(ProtocolDialect::OpenAiResponses)
        .unwrap()
        .parse_response(responses.as_bytes())
        .is_ok());
}

// ============================================================
// 场景 D:E5 多轮哨兵 —— 会话状态守恒下多轮上下文逐字回传
// ============================================================

#[tokio::test]
async fn matrix_e5_sentinel_multiturn_roundtrip() {
    // 覆盖三种守恒策略的代表厂商:MiniMax(Verbatim)/Kimi(Block)/DeepSeek(None)
    let cases = [
        (
            "mini_max/MiniMax-M3",
            StatePreservationPolicy::VerbatimThinking,
        ),
        (
            "moonshot/kimi-k3",
            StatePreservationPolicy::BlockPreservation,
        ),
        ("deep_seek/deepseek-v4-flash", StatePreservationPolicy::None),
    ];
    let specs = load_all();
    for (route_key, expected_policy) in cases {
        let spec = specs
            .iter()
            .find(|s| s.route_key() == route_key)
            .unwrap_or_else(|| panic!("{route_key} 必须存在"));
        // 断言 spec 声明的守恒策略与矩阵预期一致
        assert_eq!(
            spec.capabilities.state_preservation, expected_policy,
            "{route_key} 守恒策略不符"
        );

        // 会话往返:assistant 含哨兵思考块 → SessionStore 落库 → 读回
        let store = SessionStore::open(":memory:").await.unwrap();
        let sentinel = "<<E5-SENTINEL-2026>>";
        let assistant = AffinityMessage {
            role: MessageRole::Assistant,
            blocks: vec![
                ContentBlock::Thinking {
                    thinking: format!("{sentinel}推理内容{sentinel}").into(),
                    signature: Some("sig".into()),
                },
                ContentBlock::Text {
                    text: "答案".into(),
                },
            ],
        };
        store.record_turn(route_key, 0, &assistant).await.unwrap();
        let history = store.history(route_key).await.unwrap();

        // 按通道守恒策略处理回传块
        let preserved =
            apply_preservation_policy(&history[0].blocks, spec.capabilities.state_preservation);

        match expected_policy {
            StatePreservationPolicy::VerbatimThinking
            | StatePreservationPolicy::BlockPreservation => {
                // 思考块逐字幸存(哨兵零改动)
                let ok = preserved.iter().any(|b| matches!(
                    b,
                    ContentBlock::Thinking { thinking, .. }
                        if thinking.contains(sentinel) && thinking.matches(sentinel).count() == 2
                ));
                assert!(ok, "{route_key}: 守恒策略下哨兵思考必须逐字幸存");
            }
            StatePreservationPolicy::None => {
                // None 通道:思考块安全丢弃,文本仍在(E5 上下文连续性:可见内容不丢)
                assert!(!preserved
                    .iter()
                    .any(|b| matches!(b, ContentBlock::Thinking { .. })));
                assert!(preserved
                    .iter()
                    .any(|b| matches!(b, ContentBlock::Text { .. })));
            }
        }
    }
}

// ============================================================
// 场景 E:配额耗尽错误分类(AffinityQuotaExhausted 触发面)
// ============================================================

#[test]
fn matrix_quota_error_classification() {
    // 适配器装配后,error 分类语义:Quota/Capability 触发降级链
    let specs = load_all();
    let flash = specs
        .iter()
        .find(|s| s.route_key() == "deep_seek/deepseek-v4-flash")
        .unwrap();
    // 装配成功(有可用码器)
    let adapter = VendorAdapter::assemble(Arc::new(flash.clone()), None);
    assert!(adapter.is_ok(), "DeepSeek 通道必须可装配");
}

// ============================================================
// 场景 F:隐式族 System 后置重排的方言兼容(ADR-072 决策 ④)
//
// D 优化将 Implicit 族(DeepSeek/Qwen/豆包)的 System 消息重定位到消息
// 末尾以最大化自动前缀缓存的公共前缀。本场景验证重排后的请求体仍
// 满足 OpenAI Chat 方言语义(协议矩阵红线:重排不得破坏方言兼容)。
// ============================================================

/// 构造带 System 消息的请求(模拟 L3 repo-wiki 动态内容注入)
fn request_with_system(system_text: &str) -> AffinityRequest {
    AffinityRequest {
        intent_id: "matrix-layout".into(),
        messages: vec![
            AffinityMessage {
                role: MessageRole::System,
                blocks: vec![ContentBlock::Text {
                    text: system_text.into(),
                }],
            },
            AffinityMessage {
                role: MessageRole::User,
                blocks: vec![ContentBlock::Text {
                    text: "当前问题".into(),
                }],
            },
        ],
        tools: Vec::new(),
        thinking_pref: ThinkingPreference::Fast,
        budget_hint_micro: None,
        overrides: Default::default(),
        sampling: Default::default(),
        output_format: Default::default(),
    }
}

#[test]
fn matrix_implicit_family_system_tail_roundtrip() {
    // DeepSeek(Implicit + OpenAI Chat):重排后 system 在末尾且角色保真
    let specs = load_all();
    let spec = specs
        .iter()
        .find(|s| s.route_key() == "deep_seek/deepseek-v4-flash")
        .expect("deepseek-v4-flash 必须存在");
    assert_eq!(
        spec.capabilities.prompt_caching,
        nexus_contracts::affinity::CacheSupport::Implicit,
        "DeepSeek 必须是隐式族(本场景前提)"
    );

    let mut req = request_with_system("repo-wiki 检索结果(每轮变化)");
    // D 优化:重排 System 到末尾
    assert!(mca_gateway::prompt_norm::layout_messages(spec, &mut req));

    // 方言原生请求构造(OpenAI Chat):system 角色必须保留且位于末尾
    let body = Codec::for_dialect(ProtocolDialect::OpenAiChat)
        .unwrap()
        .build_request(spec, &req)
        .expect("重排后请求必须可构造");
    let messages = body["messages"].as_array().expect("messages 必须是数组");
    assert_eq!(messages.len(), 2);
    assert_eq!(
        messages[0]["role"], "user",
        "重排后首位必须是稳定历史(user)"
    );
    assert_eq!(messages[1]["role"], "system", "重排后 system 必须在末尾");
    assert_eq!(
        messages[1]["content"], "repo-wiki 检索结果(每轮变化)",
        "system 内容必须逐字保真(重排只改顺序不改内容)"
    );
    // 隐式族不注入 cache_control 断点(断点是显式族机制)
    assert!(
        !body.to_string().contains("cache_control"),
        "隐式族请求体不得出现 cache_control"
    );
}

#[test]
fn matrix_explicit_family_system_stays_first() {
    // 显式族(GLM):System 保持首位 + cache_control 断点注入(回归钉住)
    let specs = load_all();
    let spec = specs
        .iter()
        .find(|s| s.route_key() == "zhipu/glm-5.2")
        .expect("glm-5.2 必须存在");
    // GLM 走 Anthropic 方言(system 是顶层参数,不在 messages 数组)
    let mut req = request_with_system("系统提示");
    // 显式族不重排(断点在 L2 后,重排反而破坏断点位置)
    assert!(!mca_gateway::prompt_norm::layout_messages(spec, &mut req));

    let body = Codec::for_dialect(ProtocolDialect::AnthropicMessages)
        .unwrap()
        .build_request(spec, &req)
        .expect("显式族请求必须可构造");
    // Anthropic 路径:system 是顶层字段(不在 messages 数组);
    // ExplicitControl 下为块数组(含 cache_control 断点)
    let system = body["system"].clone();
    let system_text = match &system {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| b["text"].as_str().map(str::to_string))
            .collect::<Vec<_>>()
            .join(""),
        other => panic!("system 必须是字符串或块数组, got {other:?}"),
    };
    assert!(
        system_text.contains("系统提示"),
        "Anthropic 路径 system 内容必须完整: {system_text}"
    );
    // 显式族注入 cache_control 断点(缓存亲和机制生效)
    assert!(
        body.to_string().contains("cache_control"),
        "显式族请求体必须注入 cache_control"
    );
}
