//! 录播回放集成测试 — GLM(Anthropic 方言)+ DeepSeek(OpenAI 方言)双通道
//!
//! 对应计划:M0 W3 T0.10 / PR-5(录播 fixture 回放,CI 离线零网络依赖)
//!
//! # 录播测试策略(设计文档 §9.1)
//! fixture 是厂商真实响应的录像,Codec 对其解码必须产出正确的统一块模型。
//! M0 阶段 fixture 为按厂商文档格式合成(标注 `_fixture_note`),真实录制
//! 待 Action Item 1(七厂商 API Key)就绪后原位替换——测试断言不变,
//! 这正是录播测试的价值:替换录像即验证真实协议兼容性。

use mca_gateway::codec::Codec;
use mca_gateway::prelude::*;
use nexus_contracts::affinity::{ContentBlock, FinishReason, OutputFormat, SamplingParams};

/// 读取 fixture 文件字节(测试专用,路径相对 crate 根)
fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("fixture {path} 读取失败: {e}"))
}

/// 双通道注册:GLM(Anthropic 优先)+ DeepSeek(OpenAI)
fn dual_channel_gateway() -> McaGateway {
    let gateway = McaGateway::new(McaGatewayConfig::default());
    gateway.register_spec(ModelAffinitySpec::minimal(
        ProviderId::Zhipu,
        "glm-5.2",
        ProtocolDialect::AnthropicMessages,
    ));
    gateway.register_spec(ModelAffinitySpec::minimal(
        ProviderId::DeepSeek,
        "deepseek-v4-flash",
        ProtocolDialect::OpenAiChat,
    ));
    gateway
}

#[test]
fn dual_channel_registration() {
    let gateway = dual_channel_gateway();
    assert_eq!(gateway.spec_count(), 2);
    assert!(gateway.lookup_spec("zhipu/glm-5.2").is_some());
    assert!(gateway.lookup_spec("deep_seek/deepseek-v4-flash").is_some());
}

#[test]
fn replay_deepseek_basic_qa() {
    // DeepSeek OpenAI 方言:reasoning_content + content + 隐式缓存命中
    let gateway = dual_channel_gateway();
    let spec = gateway.lookup_spec("deep_seek/deepseek-v4-flash").unwrap();
    let codec = Codec::for_dialect(spec.preferred_dialect().unwrap()).unwrap();

    let decoded = codec
        .parse_response(&fixture("deepseek_chat_basic.json"))
        .expect("fixture 回放解码必须成功");

    // 思考块在前、文本块在后(块序即厂商响应语义序)
    assert_eq!(decoded.blocks.len(), 2);
    assert!(matches!(&decoded.blocks[0], ContentBlock::Thinking { .. }));
    assert!(
        matches!(&decoded.blocks[1], ContentBlock::Text { text } if text.contains("quicksort"))
    );
    // usage 三元组:输入/输出/缓存命中(DeepSeek 隐式缓存族)
    assert_eq!(decoded.usage.input_tokens, 24);
    assert_eq!(decoded.usage.cache_hit_tokens, 16);
    assert_eq!(decoded.usage.thinking_tokens, Some(32));
    assert_eq!(decoded.finish_reason, FinishReason::Stop);
}

#[test]
fn replay_deepseek_tool_call() {
    let gateway = dual_channel_gateway();
    let spec = gateway.lookup_spec("deep_seek/deepseek-v4-flash").unwrap();
    let codec = Codec::for_dialect(spec.preferred_dialect().unwrap()).unwrap();

    let decoded = codec
        .parse_response(&fixture("deepseek_chat_toolcall.json"))
        .unwrap();

    assert_eq!(decoded.blocks.len(), 1);
    match &decoded.blocks[0] {
        ContentBlock::ToolUse {
            id,
            name,
            input_json,
        } => {
            assert_eq!(id.as_ref(), "call_ds_001");
            assert_eq!(name.as_ref(), "read_file");
            // 入参保持原始 JSON 字符串形态(L0 契约),内容可被上层解析
            let parsed: serde_json::Value = serde_json::from_str(input_json).unwrap();
            assert_eq!(parsed["path"], "src/main.rs");
        }
        other => panic!("期望 ToolUse 块,实际 {other:?}"),
    }
    assert_eq!(decoded.finish_reason, FinishReason::ToolUse);
}

#[test]
fn replay_zhipu_basic_qa() {
    // GLM Anthropic 方言:thinking 块含 signature(P4 状态守恒关键字段)
    let gateway = dual_channel_gateway();
    let spec = gateway.lookup_spec("zhipu/glm-5.2").unwrap();
    let codec = Codec::for_dialect(spec.preferred_dialect().unwrap()).unwrap();

    let decoded = codec
        .parse_response(&fixture("zhipu_anthropic_basic.json"))
        .unwrap();

    assert_eq!(decoded.blocks.len(), 2);
    assert!(matches!(
        &decoded.blocks[0],
        ContentBlock::Thinking { signature: Some(s), .. } if s.as_ref() == "glm-sig-0001"
    ));
    // 显式缓存族:cache_read_input_tokens 回读
    assert_eq!(decoded.usage.cache_hit_tokens, 24);
    assert_eq!(decoded.finish_reason, FinishReason::Stop);
}

#[test]
fn replay_zhipu_tool_call_roundtrip() {
    // 端到端语义闭环:解码 GLM 工具调用 → 块回传构造下一轮请求(P4 守恒)
    let gateway = dual_channel_gateway();
    let spec = gateway.lookup_spec("zhipu/glm-5.2").unwrap();
    let codec = Codec::for_dialect(spec.preferred_dialect().unwrap()).unwrap();

    let decoded = codec
        .parse_response(&fixture("zhipu_anthropic_toolcall.json"))
        .unwrap();
    assert_eq!(decoded.finish_reason, FinishReason::ToolUse);

    // 用解码出的块构造回传请求:thinking + tool_use 原序入 assistant 历史
    use nexus_contracts::affinity::{
        AffinityMessage, AffinityOverrides, AffinityRequest, MessageRole, ThinkingPreference,
    };
    let request = AffinityRequest {
        intent_id: "intent-replay".into(),
        messages: vec![
            AffinityMessage {
                role: MessageRole::User,
                blocks: vec![ContentBlock::Text {
                    text: "重构 lib.rs".into(),
                }],
            },
            AffinityMessage {
                role: MessageRole::Assistant,
                blocks: decoded.blocks.clone(),
            },
            AffinityMessage {
                role: MessageRole::Tool,
                blocks: vec![ContentBlock::ToolResult {
                    tool_use_id: "toolu_glm_001".into(),
                    content: "pub fn lib() {}".into(),
                    is_error: false,
                }],
            },
        ],
        tools: Vec::new(),
        thinking_pref: ThinkingPreference::Standard,
        budget_hint_micro: None,
        overrides: AffinityOverrides::default(),
        sampling: SamplingParams::default(),
        output_format: OutputFormat::default(),
    };
    let body = codec.build_request(&spec, &request).unwrap();

    // 断言状态守恒:assistant 消息中 thinking 块(含 signature)与 tool_use 块原序回传
    let assistant_content = body["messages"][1]["content"].as_array().unwrap();
    assert_eq!(assistant_content[0]["type"], "thinking");
    assert_eq!(assistant_content[0]["signature"], "glm-sig-0002");
    assert_eq!(assistant_content[1]["type"], "tool_use");
    assert_eq!(assistant_content[1]["input"]["path"], "src/lib.rs");
    // tool_result 按方言规定落在 user 角色
    assert_eq!(body["messages"][2]["role"], "user");
}
