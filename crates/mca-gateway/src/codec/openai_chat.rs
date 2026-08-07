//! OpenAI Chat Completions 码器 — `/v1/chat/completions` 方言
//!
//! 覆盖厂商:DeepSeek / 豆包(火山方舟)/ Qwen(DashScope 兼容)/ Step /
//! GLM(OpenAI 端点)/ MiniMax(OpenAI 端点)/ Custom(vLLM/Ollama 等)。
//!
//! # 职责边界(M0)
//! - 请求构造:messages(system/user/assistant/tool)+ tools(function 声明)
//! - 响应解析:choices[0].message → 统一块模型(Text/Thinking/ToolUse)
//! - 思考参数映射(TTG ↔ reasoning_effort/enable_thinking)属 M2 能力协商
//!   引擎交付,本码器 M0 不发任何思考参数(P3:描述符未声明的字段一律不发)
//!
//! # 解析容错(P3)
//! 响应中不认识的字段一律忽略(serde_json::Value 按需取字段而非强类型
//! 反序列化整体),绝不因未知字段报错中断——Claude Code"报错或静默"两难教训。

use nexus_contracts::affinity::{
    AffinityRequest, CacheSupport, ContentBlock, FinishReason, MessageRole, ModelAffinitySpec,
    ProtocolDialect, UsageReport,
};
use serde_json::{json, Value};
// L3 缓存亲和:CacheAffinityIntegration 缓存策略决策(CacheSupport 已由 L0 nexus-contracts 提供)
use super::DecodedResponse;
use crate::capability::{negotiate_thinking, ThinkingDirective};
use crate::error::AffinityError;
use scc_cache::CacheAffinityIntegration;

/// OpenAI Chat Completions 无状态码器
#[derive(Debug, Clone, Copy, Default)]
pub struct OpenAiChatCodec;

impl OpenAiChatCodec {
    /// 构造非流式请求体
    ///
    /// WHY 输出 `serde_json::Value` 而非强类型: 方言保真(P2)要求后续
    /// 里程碑可透传 `vendor_extra` 任意字段,Value 拼装比强类型 struct
    /// 更适合"基础字段 + 可选扩展"的开放结构。
    pub fn build_request(
        &self,
        spec: &ModelAffinitySpec,
        request: &AffinityRequest,
    ) -> Result<Value, AffinityError> {
        let messages = build_messages(request, spec)?;
        let mut body = json!({
            "model": spec.model.as_ref(),
            "messages": messages,
        });
        if !request.tools.is_empty() {
            body["tools"] = build_tools(request)?;
        }

        // 思考参数映射:能力协商产出 → 方言原生参数(P0-3 厂商 thinking 参数原生映射)
        //   - DeepSeek 风格:enable_thinking=true(OnOff 通道)
        //   - GLM 风格:reasoning_effort=level(EffortLevels 通道)
        //   - 不支持思考(OnOff 下 Fast 偏好)或 None 通道:不发 thinking 参数(P3:未声明的字段一律不发)
        let (thinking_dir, _degraded) =
            negotiate_thinking(&spec.capabilities.thinking, request.thinking_pref);
        match thinking_dir {
            ThinkingDirective::Off => { /* 不发 thinking 参数 */ }
            ThinkingDirective::On => {
                body["enable_thinking"] = json!(true);
            }
            ThinkingDirective::Effort(level) => {
                body["reasoning_effort"] = json!(level.as_ref());
            }
        }

        // 缓存亲和参数注入:cache_control 断点(MCA A3,P0-2)
        //   - ExplicitControl 族(Anthropic 族):在系统消息中注入 cache_control
        //   - None/Implicit 族:不注入(隐式族靠会话粘性)
        //   - cache_control 仅对系统提示生效(最稳定前缀,见 scc-cache plan_breakpoints)
        // WHY 仅系统消息:Anthropic 最多 4 个断点配额,系统提示是最长公共前缀,
        // 对系统提示加 cache_control 即可覆盖所有请求的相同前缀部分。
        // 用户消息的动态内容随轮次变化,打断点收益低且消耗配额。
        if CacheAffinityIntegration::should_inject_cache_control(spec.capabilities.prompt_caching) {
            // 系统消息是顶层字段(OpenAI Chat 中 system 是 messages 数组的第一项)
            // 在系统消息的 content 中添加 cache_control: { "type": "ephemeral" }
            if let Some(messages) = body.get_mut("messages").and_then(Value::as_array_mut) {
                if let Some(first) = messages.first_mut() {
                    if first.get("role").and_then(Value::as_str) == Some("system") {
                        if let Some(content) = first.get_mut("content") {
                            // 如果 content 是字符串,转换为块数组格式
                            if content.is_string() {
                                let text = content.take();
                                *content = json!([{
                                    "type": "text",
                                    "text": text,
                                    "cache_control": { "type": "ephemeral" }
                                }]);
                            } else if let Some(blocks) = content.as_array_mut() {
                                // content 已是块数组,在第一个文本块上添加 cache_control
                                if let Some(first_block) = blocks.first_mut() {
                                    if first_block.get("type").and_then(Value::as_str)
                                        == Some("text")
                                    {
                                        first_block["cache_control"] = json!({"type": "ephemeral"});
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(body)
    }

    /// 解析非流式响应体 → 统一块模型
    pub fn parse_response(&self, body: &[u8]) -> Result<DecodedResponse, AffinityError> {
        let root: Value = serde_json::from_slice(body).map_err(|e| AffinityError::Protocol {
            dialect: ProtocolDialect::OpenAiChat,
            reason: format!("invalid JSON: {e}"),
        })?;
        let message =
            root.pointer("/choices/0/message")
                .ok_or_else(|| AffinityError::Protocol {
                    dialect: ProtocolDialect::OpenAiChat,
                    reason: "missing choices[0].message".into(),
                })?;

        let mut blocks = Vec::new();
        // DeepSeek/GLM 等厂商在 OpenAI 方言下用 reasoning_content 携带思考内容
        if let Some(thinking) = message.get("reasoning_content").and_then(Value::as_str) {
            if !thinking.is_empty() {
                blocks.push(ContentBlock::Thinking {
                    thinking: thinking.into(),
                    signature: None,
                });
            }
        }
        if let Some(text) = message.get("content").and_then(Value::as_str) {
            if !text.is_empty() {
                blocks.push(ContentBlock::Text { text: text.into() });
            }
        }
        if let Some(calls) = message.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                blocks.push(parse_tool_call(call)?);
            }
        }

        let finish_reason = root
            .pointer("/choices/0/finish_reason")
            .and_then(Value::as_str)
            .map(map_finish_reason)
            .unwrap_or(FinishReason::Other);

        Ok(DecodedResponse {
            blocks,
            usage: parse_usage(root.get("usage")),
            finish_reason,
            request_id: root.get("id").and_then(Value::as_str).map(Into::into),
        })
    }
}

/// 会话历史 → OpenAI messages 数组
///
/// 块模型到方言的映射规则:
/// - System: 文本块拼接为 content 字符串;当 `ExplicitControl` 时转为块数组并注入
///   `cache_control`(P0-2 缓存亲和,MiniMax/GLM 显式缓存族)
/// - User: 文本块拼接为 content 字符串
/// - Assistant: Text 块 → content;ToolUse 块 → tool_calls;Thinking 块在
///   OpenAI 方言 M0 不回传(VerbatimThinking 守恒属 M1 session 专项)
/// - Tool: 每个 ToolResult 块展开为一条 role=tool 消息(OpenAI 方言要求
///   工具结果逐条对应 tool_call_id)
fn build_messages(
    request: &AffinityRequest,
    spec: &ModelAffinitySpec,
) -> Result<Value, AffinityError> {
    let mut messages = Vec::new();
    for msg in &request.messages {
        match msg.role {
            MessageRole::System | MessageRole::User => {
                let role = if msg.role == MessageRole::System {
                    "system"
                } else {
                    "user"
                };
                let is_explicit = spec.capabilities.prompt_caching == CacheSupport::ExplicitControl
                    && msg.role == MessageRole::System;
                let content = if is_explicit {
                    // ExplicitControl 族:系统消息转为块数组并注入 cache_control
                    // 使厂商缓存系统提示前缀,降低重复输入 token 消耗
                    let blocks: Vec<Value> = msg
                        .blocks
                        .iter()
                        .map(|b| match b {
                            ContentBlock::Text { text } => json!({
                                "type": "text",
                                "text": text.as_ref(),
                                "cache_control": {"type": "ephemeral"}
                            }),
                            _ => json!({"type": "text", "text": ""}),
                        })
                        .collect();
                    Value::Array(blocks)
                } else {
                    Value::String(join_text(&msg.blocks))
                };
                messages.push(json!({ "role": role, "content": content }));
            }
            MessageRole::Assistant => {
                let mut entry = json!({ "role": "assistant" });
                let text = join_text(&msg.blocks);
                if !text.is_empty() {
                    entry["content"] = Value::String(text);
                }
                let tool_calls = build_assistant_tool_calls(&msg.blocks)?;
                if !tool_calls.is_empty() {
                    entry["tool_calls"] = Value::Array(tool_calls);
                }
                messages.push(entry);
            }
            MessageRole::Tool => {
                for block in &msg.blocks {
                    if let ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        ..
                    } = block
                    {
                        messages.push(json!({
                            "role": "tool",
                            "tool_call_id": tool_use_id.as_ref(),
                            "content": content.as_ref(),
                        }));
                    }
                }
            }
        }
    }
    Ok(Value::Array(messages))
}

/// 工具声明 → OpenAI tools 数组(function 形态)
fn build_tools(request: &AffinityRequest) -> Result<Value, AffinityError> {
    let mut tools = Vec::with_capacity(request.tools.len());
    for tool in &request.tools {
        // L0 契约中 schema 为原始 JSON 文本,在系统边界(此处)解析校验
        let schema: Value =
            serde_json::from_str(&tool.parameters_schema).map_err(|e| AffinityError::Protocol {
                dialect: ProtocolDialect::OpenAiChat,
                reason: format!("tool '{}' parameters_schema invalid: {e}", tool.name),
            })?;
        tools.push(json!({
            "type": "function",
            "function": {
                "name": tool.name.as_ref(),
                "description": tool.description.as_ref(),
                "parameters": schema,
            }
        }));
    }
    Ok(Value::Array(tools))
}

/// assistant 历史中的 ToolUse 块 → tool_calls 数组(多轮工具调用回传)
fn build_assistant_tool_calls(blocks: &[ContentBlock]) -> Result<Vec<Value>, AffinityError> {
    let mut calls = Vec::new();
    for block in blocks {
        if let ContentBlock::ToolUse {
            id,
            name,
            input_json,
        } = block
        {
            calls.push(json!({
                "id": id.as_ref(),
                "type": "function",
                // WHY arguments 保持原始 JSON 字符串: OpenAI 方言的
                // function.arguments 本身就是字符串编码的 JSON,直通零转换
                "function": { "name": name.as_ref(), "arguments": input_json.as_ref() },
            }));
        }
    }
    Ok(calls)
}

/// 响应中的单个 tool_call → ToolUse 块
fn parse_tool_call(call: &Value) -> Result<ContentBlock, AffinityError> {
    let function = call
        .get("function")
        .ok_or_else(|| AffinityError::Protocol {
            dialect: ProtocolDialect::OpenAiChat,
            reason: "tool_call missing 'function'".into(),
        })?;
    let name = function
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| AffinityError::Protocol {
            dialect: ProtocolDialect::OpenAiChat,
            reason: "tool_call missing function.name".into(),
        })?;
    Ok(ContentBlock::ToolUse {
        id: call
            .get("id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .into(),
        name: name.into(),
        input_json: function
            .get("arguments")
            .and_then(Value::as_str)
            .unwrap_or("{}")
            .into(),
    })
}

/// 拼接消息内的全部文本块
fn join_text(blocks: &[ContentBlock]) -> String {
    let mut out = String::new();
    for block in blocks {
        if let ContentBlock::Text { text } = block {
            out.push_str(text);
        }
    }
    out
}

/// usage 字段解析 — 缺失字段按 0 处理(P3:厂商未返回不报错)
fn parse_usage(usage: Option<&Value>) -> UsageReport {
    let get = |key: &str| -> u64 {
        usage
            .and_then(|u| u.get(key))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    UsageReport {
        input_tokens: get("prompt_tokens"),
        output_tokens: get("completion_tokens"),
        // DeepSeek 隐式缓存族返回 prompt_cache_hit_tokens(命中价 1/100)
        cache_hit_tokens: get("prompt_cache_hit_tokens"),
        thinking_tokens: usage
            .and_then(|u| u.pointer("/completion_tokens_details/reasoning_tokens"))
            .and_then(Value::as_u64),
    }
}

/// finish_reason 字符串 → 归一枚举(未知值归 Other,P3 容错)
fn map_finish_reason(raw: &str) -> FinishReason {
    match raw {
        "stop" => FinishReason::Stop,
        "length" => FinishReason::MaxTokens,
        "tool_calls" => FinishReason::ToolUse,
        "content_filter" => FinishReason::ContentFilter,
        _ => FinishReason::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_contracts::affinity::{
        AffinityMessage, AffinityOverrides, OutputFormat, ProviderId, SamplingParams,
        ThinkingPreference, ToolDecl,
    };

    fn sample_request(with_tools: bool) -> AffinityRequest {
        AffinityRequest {
            intent_id: "intent-1".into(),
            messages: vec![
                AffinityMessage {
                    role: MessageRole::System,
                    blocks: vec![ContentBlock::Text {
                        text: "你是编码助手".into(),
                    }],
                },
                AffinityMessage {
                    role: MessageRole::User,
                    blocks: vec![ContentBlock::Text {
                        text: "读取 main.rs".into(),
                    }],
                },
            ],
            tools: if with_tools {
                vec![ToolDecl {
                    name: "read_file".into(),
                    description: "读文件".into(),
                    parameters_schema: r#"{"type":"object"}"#.into(),
                }]
            } else {
                Vec::new()
            },
            thinking_pref: ThinkingPreference::Standard,
            budget_hint_micro: None,
            overrides: AffinityOverrides::default(),
            sampling: SamplingParams::default(),
            output_format: OutputFormat::default(),
        }
    }

    fn spec() -> ModelAffinitySpec {
        ModelAffinitySpec::minimal(
            ProviderId::DeepSeek,
            "deepseek-v4-flash",
            ProtocolDialect::OpenAiChat,
        )
    }

    #[test]
    fn build_request_basic_qa() {
        let body = OpenAiChatCodec
            .build_request(&spec(), &sample_request(false))
            .unwrap();
        assert_eq!(body["model"], "deepseek-v4-flash");
        assert_eq!(body["messages"][0]["role"], "system");
        assert_eq!(body["messages"][1]["content"], "读取 main.rs");
        assert!(body.get("tools").is_none(), "无工具时不发 tools 字段(P3)");
    }

    #[test]
    fn build_request_with_tools() {
        let body = OpenAiChatCodec
            .build_request(&spec(), &sample_request(true))
            .unwrap();
        assert_eq!(body["tools"][0]["function"]["name"], "read_file");
        assert_eq!(body["tools"][0]["type"], "function");
    }

    #[test]
    fn build_request_rejects_bad_tool_schema() {
        let mut req = sample_request(true);
        req.tools[0].parameters_schema = "not json".into();
        let err = OpenAiChatCodec.build_request(&spec(), &req).unwrap_err();
        assert!(matches!(err, AffinityError::Protocol { .. }));
    }

    #[test]
    fn build_request_replays_assistant_tool_calls() {
        // 多轮工具调用:assistant 的 ToolUse 与 tool 的 ToolResult 必须正确回传
        let mut req = sample_request(false);
        req.messages.push(AffinityMessage {
            role: MessageRole::Assistant,
            blocks: vec![ContentBlock::ToolUse {
                id: "call-1".into(),
                name: "read_file".into(),
                input_json: r#"{"path":"main.rs"}"#.into(),
            }],
        });
        req.messages.push(AffinityMessage {
            role: MessageRole::Tool,
            blocks: vec![ContentBlock::ToolResult {
                tool_use_id: "call-1".into(),
                content: "fn main() {}".into(),
                is_error: false,
            }],
        });
        let body = OpenAiChatCodec.build_request(&spec(), &req).unwrap();
        let messages = body["messages"].as_array().unwrap();
        assert_eq!(messages[2]["tool_calls"][0]["id"], "call-1");
        assert_eq!(messages[3]["role"], "tool");
        assert_eq!(messages[3]["tool_call_id"], "call-1");
    }

    #[test]
    fn parse_response_text_and_usage() {
        let body = r#"{
            "id": "req-123",
            "choices": [{"message": {"content": "你好"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 10, "completion_tokens": 5, "prompt_cache_hit_tokens": 8}
        }"#;
        let decoded = OpenAiChatCodec.parse_response(body.as_bytes()).unwrap();
        assert_eq!(decoded.blocks.len(), 1);
        assert!(
            matches!(&decoded.blocks[0], ContentBlock::Text { text } if text.as_ref() == "你好")
        );
        assert_eq!(decoded.usage.input_tokens, 10);
        assert_eq!(decoded.usage.cache_hit_tokens, 8);
        assert_eq!(decoded.finish_reason, FinishReason::Stop);
        assert_eq!(decoded.request_id.as_deref(), Some("req-123"));
    }

    #[test]
    fn parse_response_reasoning_and_tool_calls() {
        // DeepSeek 风格:reasoning_content 携带思考 + tool_calls 携带工具调用
        let body = r#"{
            "choices": [{
                "message": {
                    "reasoning_content": "先读文件",
                    "content": null,
                    "tool_calls": [{
                        "id": "call-9",
                        "type": "function",
                        "function": {"name": "read_file", "arguments": "{\"path\":\"a.rs\"}"}
                    }]
                },
                "finish_reason": "tool_calls"
            }]
        }"#;
        let decoded = OpenAiChatCodec.parse_response(body.as_bytes()).unwrap();
        assert_eq!(decoded.blocks.len(), 2);
        assert!(
            matches!(&decoded.blocks[0], ContentBlock::Thinking { thinking, .. } if thinking.as_ref() == "先读文件")
        );
        assert!(
            matches!(&decoded.blocks[1], ContentBlock::ToolUse { name, .. } if name.as_ref() == "read_file")
        );
        assert_eq!(decoded.finish_reason, FinishReason::ToolUse);
    }

    #[test]
    fn parse_response_tolerates_unknown_fields() {
        // P3 容错:注入虚构字段不报错
        let body = br#"{
            "choices": [{"message": {"content": "ok"}, "finish_reason": "stop", "fictional": 1}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1},
            "vendor_specific_field": {"deeply": ["unknown"]}
        }"#;
        assert!(OpenAiChatCodec.parse_response(body).is_ok());
    }

    #[test]
    fn parse_response_rejects_malformed() {
        assert!(matches!(
            OpenAiChatCodec.parse_response(b"not json"),
            Err(AffinityError::Protocol { .. })
        ));
        assert!(matches!(
            OpenAiChatCodec.parse_response(br#"{"no_choices": true}"#),
            Err(AffinityError::Protocol { .. })
        ));
    }

    // ── 缓存亲和集成测试(P0-2) ──

    #[test]
    fn openai_chat_explicit_control_injects_cache_control() {
        // ExplicitControl 族:系统消息应注入 cache_control
        let mut spec =
            ModelAffinitySpec::minimal(ProviderId::Zhipu, "glm-5.2", ProtocolDialect::OpenAiChat);
        spec.capabilities.prompt_caching = CacheSupport::ExplicitControl;
        let request = simple_affinity_request("test", "你好");
        let codec = OpenAiChatCodec;
        let body = codec.build_request(&spec, &request).unwrap();
        let messages = body.get("messages").and_then(Value::as_array).unwrap();
        let system_msg = messages.first().unwrap();
        let content = system_msg.get("content").unwrap();
        // ExplicitControl 下 content 应为块数组
        assert!(content.is_array(), "ExplicitControl 下 content 应为块数组");
        let blocks = content.as_array().unwrap();
        if let Some(first_block) = blocks.first() {
            assert_eq!(
                first_block.get("type").and_then(Value::as_str),
                Some("text")
            );
            assert!(
                first_block.get("cache_control").is_some(),
                "ExplicitControl 下应注入 cache_control"
            );
        }
    }

    #[test]
    fn openai_chat_none_does_not_inject_cache_control() {
        // None 族:不注入 cache_control
        let mut spec = ModelAffinitySpec::minimal(
            ProviderId::DeepSeek,
            "deepseek-v4-flash",
            ProtocolDialect::OpenAiChat,
        );
        spec.capabilities.prompt_caching = CacheSupport::None;
        let request = simple_affinity_request("test", "你好");
        let codec = OpenAiChatCodec;
        let body = codec.build_request(&spec, &request).unwrap();
        let messages = body.get("messages").and_then(Value::as_array).unwrap();
        let system_msg = messages.first().unwrap();
        let content = system_msg.get("content").unwrap();
        // None 下 content 保持字符串(不转为块数组)
        assert!(content.is_string(), "None 下 content 应为字符串");
    }

    #[test]
    fn openai_chat_implicit_does_not_inject_cache_control() {
        // Implicit 族:不注入 cache_control(隐式缓存靠会话粘性)
        let mut spec = ModelAffinitySpec::minimal(
            ProviderId::DeepSeek,
            "deepseek-v4-flash",
            ProtocolDialect::OpenAiChat,
        );
        spec.capabilities.prompt_caching = CacheSupport::Implicit;
        let request = simple_affinity_request("test", "你好");
        let codec = OpenAiChatCodec;
        let body = codec.build_request(&spec, &request).unwrap();
        let messages = body.get("messages").and_then(Value::as_array).unwrap();
        let system_msg = messages.first().unwrap();
        let content = system_msg.get("content").unwrap();
        // Implicit 下 content 保持字符串
        assert!(content.is_string(), "Implicit 下 content 应为字符串");
    }

    /// 创建简单亲和请求:单条系统消息 + 默认思考偏好
    fn simple_affinity_request(intent_id: &str, text: &str) -> AffinityRequest {
        AffinityRequest {
            intent_id: intent_id.into(),
            messages: vec![AffinityMessage {
                role: MessageRole::System,
                blocks: vec![ContentBlock::Text { text: text.into() }],
            }],
            tools: Vec::new(),
            thinking_pref: ThinkingPreference::Standard,
            budget_hint_micro: None,
            overrides: AffinityOverrides::default(),
            sampling: SamplingParams::default(),
            output_format: OutputFormat::default(),
        }
    }
}
