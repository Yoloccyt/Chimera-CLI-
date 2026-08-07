//! Anthropic Messages 码器 — `/v1/messages` 方言
//!
//! 覆盖厂商:Kimi K3(原生优先路径)/ GLM(Anthropic 端点)/ MiniMax
//! (Anthropic 端点,VerbatimThinking 怪癖的宿主方言)。
//!
//! # 方言特征
//! - 请求:`system` 为顶层字段(不入 messages);消息 content 为**块数组**
//!   (text/thinking/tool_use/tool_result),与 L0 统一块模型天然同构
//! - 响应:`content` 块数组直接映射 ContentBlock,thinking 块含 signature
//!   (回传校验用,P4 状态守恒的关键字段)
//! - `max_tokens` 为必填字段(取 spec.capabilities.max_output)
//!
//! # 解析容错(P3)
//! 不认识的块类型跳过不报错(未知块是厂商演进信号,由适配器层发
//! `AffinityUnknownField` 事件留痕,驱动 spec 更新)。

use nexus_contracts::affinity::{
    AffinityRequest, CacheSupport, ContentBlock, FinishReason, MessageRole, ModelAffinitySpec,
    ProtocolDialect, UsageReport,
};
use serde_json::{json, Value};
// L3 缓存亲和:CacheAffinityIntegration 缓存策略决策(CacheSupport 已由 L0 nexus-contracts 提供)
use scc_cache::CacheAffinityIntegration;

use super::DecodedResponse;
use crate::capability::{negotiate_thinking, ThinkingDirective};
use crate::error::AffinityError;

/// Anthropic Messages 无状态码器
#[derive(Debug, Clone, Copy, Default)]
pub struct AnthropicCodec;

impl AnthropicCodec {
    /// 构造非流式请求体
    pub fn build_request(
        &self,
        spec: &ModelAffinitySpec,
        request: &AffinityRequest,
    ) -> Result<Value, AffinityError> {
        let mut body = json!({
            "model": spec.model.as_ref(),
            // Anthropic 方言 max_tokens 必填;取描述符实测输出上限(P6 不信宣传值)
            "max_tokens": spec.capabilities.max_output,
            "messages": build_messages(request)?,
        });
        // system 提示是顶层字段:从消息历史中提取全部 System 消息文本
        // ExplicitControl 族:转为块数组并注入 cache_control(P0-2 缓存亲和)
        let system = build_system_field(request, spec);
        if !system.is_null() {
            body["system"] = system;
        }
        if !request.tools.is_empty() {
            body["tools"] = build_tools(request)?;
        }

        // 思考参数映射:能力协商产出 → Anthropic 方言原生参数(P0-3)
        //   - Anthropic 方言:thinking.type=enabled + budget_tokens(Effort 时用 spec 输出上限)
        //   - 不支持思考或 Fast 偏好:不发 thinking 参数(P3:未声明的字段一律不发)
        let (thinking_dir, _degraded) =
            negotiate_thinking(&spec.capabilities.thinking, request.thinking_pref);
        match thinking_dir {
            ThinkingDirective::Off => { /* 不发 thinking 参数 */ }
            ThinkingDirective::On => {
                body["thinking"] = json!({"type": "enabled"});
            }
            ThinkingDirective::Effort(_level) => {
                // Anthropic 方言:thinking.type=enabled + 使用 spec 的 max_output 作为 budget_tokens
                // 对于 GLM Anthropic 端点,thinking.budget_tokens 控制思考深度
                // 注意:Anthropic 原生不支持 effort 档位概念,但兼容 GLM 端点的 effort 语义
                body["thinking"] = json!({
                    "type": "enabled",
                    "budget_tokens": spec.capabilities.max_output.min(65536)
                });
            }
        }

        // 缓存亲和参数注入:cache_control 断点(MCA A3,P0-2)
        //   - Anthropic 方言:system 字段是顶层字符串,改为块数组并加 cache_control
        //   - 仅 ExplicitControl 族注入;None/Implicit 族不注入
        //   - Anthropic 最多 4 个断点配额,仅对 system 层打断点
        if CacheAffinityIntegration::should_inject_cache_control(spec.capabilities.prompt_caching) {
            if let Some(system) = body.get_mut("system") {
                if system.is_string() {
                    let text = system.take();
                    *system = json!([{
                        "type": "text",
                        "text": text,
                        "cache_control": { "type": "ephemeral" }
                    }]);
                }
            }
        }

        Ok(body)
    }

    /// 解析非流式响应体 → 统一块模型
    pub fn parse_response(&self, body: &[u8]) -> Result<DecodedResponse, AffinityError> {
        let root: Value = serde_json::from_slice(body).map_err(|e| AffinityError::Protocol {
            dialect: ProtocolDialect::AnthropicMessages,
            reason: format!("invalid JSON: {e}"),
        })?;
        let content = root
            .get("content")
            .and_then(Value::as_array)
            .ok_or_else(|| AffinityError::Protocol {
                dialect: ProtocolDialect::AnthropicMessages,
                reason: "missing 'content' block array".into(),
            })?;

        let mut blocks = Vec::with_capacity(content.len());
        for block in content {
            // 未知块类型返回 None,跳过不报错(P3)
            if let Some(parsed) = parse_content_block(block) {
                blocks.push(parsed);
            }
        }

        let finish_reason = root
            .get("stop_reason")
            .and_then(Value::as_str)
            .map(map_stop_reason)
            .unwrap_or(FinishReason::Other);

        Ok(DecodedResponse {
            blocks,
            usage: parse_usage(root.get("usage")),
            finish_reason,
            request_id: root.get("id").and_then(Value::as_str).map(Into::into),
        })
    }
}

/// 会话历史 → Anthropic messages 数组(System 消息不入内,见 build_request)
///
/// Anthropic 方言的块数组结构与 L0 统一块模型同构,映射近乎直通;
/// ToolResult 在 Anthropic 方言中属于 **user** 角色消息(方言规定)。
fn build_messages(request: &AffinityRequest) -> Result<Value, AffinityError> {
    let mut messages = Vec::new();
    for msg in &request.messages {
        match msg.role {
            // System 由顶层 system 字段承载,此处跳过
            MessageRole::System => continue,
            MessageRole::User => {
                messages.push(json!({
                    "role": "user",
                    "content": blocks_to_dialect(&msg.blocks)?,
                }));
            }
            MessageRole::Assistant => {
                // P4 状态守恒:assistant 的 thinking/tool_use 块原序回传
                // (BlockPreservation 策略,Kimi K3 明确要求完整内容块)
                messages.push(json!({
                    "role": "assistant",
                    "content": blocks_to_dialect(&msg.blocks)?,
                }));
            }
            MessageRole::Tool => {
                // 方言规定:tool_result 块放在 user 角色消息内
                messages.push(json!({
                    "role": "user",
                    "content": blocks_to_dialect(&msg.blocks)?,
                }));
            }
        }
    }
    Ok(Value::Array(messages))
}

/// 统一块序列 → Anthropic content 块数组(原序保真)
fn blocks_to_dialect(blocks: &[ContentBlock]) -> Result<Value, AffinityError> {
    let mut out = Vec::with_capacity(blocks.len());
    for block in blocks {
        out.push(match block {
            ContentBlock::Text { text } => json!({ "type": "text", "text": text.as_ref() }),
            ContentBlock::Thinking {
                thinking,
                signature,
            } => {
                let mut b = json!({ "type": "thinking", "thinking": thinking.as_ref() });
                // signature 缺失时不发字段(P3:未声明的字段一律不发)
                if let Some(sig) = signature {
                    b["signature"] = Value::String(sig.to_string());
                }
                b
            }
            ContentBlock::ToolUse {
                id,
                name,
                input_json,
            } => {
                // Anthropic 方言 input 是 JSON 对象(非字符串),在边界解析
                let input: Value =
                    serde_json::from_str(input_json).map_err(|e| AffinityError::Protocol {
                        dialect: ProtocolDialect::AnthropicMessages,
                        reason: format!("tool_use '{name}' input_json invalid: {e}"),
                    })?;
                json!({ "type": "tool_use", "id": id.as_ref(), "name": name.as_ref(), "input": input })
            }
            ContentBlock::ToolResult {
                tool_use_id,
                content,
                is_error,
            } => json!({
                "type": "tool_result",
                "tool_use_id": tool_use_id.as_ref(),
                "content": content.as_ref(),
                "is_error": is_error,
            }),
        });
    }
    Ok(Value::Array(out))
}

/// 构建 system 顶层字段 — 根据缓存策略返回字符串或块数组
///
/// - `None` / `Implicit`: 返回 `Value::String`(文本拼接,兼容现有行为)
/// - `ExplicitControl`: 返回 `Value::Array`(块数组 + `cache_control` 注入)
fn build_system_field(request: &AffinityRequest, spec: &ModelAffinitySpec) -> Value {
    let is_explicit = spec.capabilities.prompt_caching == CacheSupport::ExplicitControl;
    let mut out = String::new();
    for msg in &request.messages {
        if msg.role == MessageRole::System {
            for block in &msg.blocks {
                if let ContentBlock::Text { text } = block {
                    out.push_str(text);
                }
            }
        }
    }
    if out.is_empty() {
        return Value::Null;
    }
    if is_explicit {
        Value::Array(vec![json!({
            "type": "text",
            "text": out,
            "cache_control": {"type": "ephemeral"}
        })])
    } else {
        Value::String(out)
    }
}

/// 工具声明 → Anthropic tools 数组(input_schema 形态)
fn build_tools(request: &AffinityRequest) -> Result<Value, AffinityError> {
    let mut tools = Vec::with_capacity(request.tools.len());
    for tool in &request.tools {
        let schema: Value =
            serde_json::from_str(&tool.parameters_schema).map_err(|e| AffinityError::Protocol {
                dialect: ProtocolDialect::AnthropicMessages,
                reason: format!("tool '{}' parameters_schema invalid: {e}", tool.name),
            })?;
        tools.push(json!({
            "name": tool.name.as_ref(),
            "description": tool.description.as_ref(),
            "input_schema": schema,
        }));
    }
    Ok(Value::Array(tools))
}

/// 响应 content 块 → 统一块;未知类型返回 None(P3 跳过留痕)
fn parse_content_block(block: &Value) -> Option<ContentBlock> {
    match block.get("type").and_then(Value::as_str)? {
        "text" => Some(ContentBlock::Text {
            text: block.get("text").and_then(Value::as_str)?.into(),
        }),
        "thinking" => Some(ContentBlock::Thinking {
            thinking: block.get("thinking").and_then(Value::as_str)?.into(),
            signature: block
                .get("signature")
                .and_then(Value::as_str)
                .map(Into::into),
        }),
        "tool_use" => Some(ContentBlock::ToolUse {
            id: block
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .into(),
            name: block.get("name").and_then(Value::as_str)?.into(),
            // 方言 input 是 JSON 对象 → 序列化回原始字符串(L0 契约形态)
            input_json: block
                .get("input")
                .map(Value::to_string)
                .unwrap_or_else(|| "{}".into())
                .into(),
        }),
        // 未知块类型:跳过(适配器层负责 AffinityUnknownField 事件留痕)
        _ => None,
    }
}

/// usage 解析 — Anthropic 命名(input_tokens/output_tokens/cache_read_input_tokens)
fn parse_usage(usage: Option<&Value>) -> UsageReport {
    let get = |key: &str| -> u64 {
        usage
            .and_then(|u| u.get(key))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    UsageReport {
        input_tokens: get("input_tokens"),
        output_tokens: get("output_tokens"),
        // 显式缓存族(cache_control)命中读取数
        cache_hit_tokens: get("cache_read_input_tokens"),
        thinking_tokens: None,
    }
}

/// stop_reason → 归一枚举(未知值归 Other,P3 容错)
fn map_stop_reason(raw: &str) -> FinishReason {
    match raw {
        "end_turn" | "stop_sequence" => FinishReason::Stop,
        "max_tokens" => FinishReason::MaxTokens,
        "tool_use" => FinishReason::ToolUse,
        "refusal" => FinishReason::ContentFilter,
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

    fn sample_request() -> AffinityRequest {
        AffinityRequest {
            intent_id: "intent-2".into(),
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
                        text: "重构这个函数".into(),
                    }],
                },
            ],
            tools: vec![ToolDecl {
                name: "edit_file".into(),
                description: "编辑文件".into(),
                parameters_schema: r#"{"type":"object"}"#.into(),
            }],
            thinking_pref: ThinkingPreference::Deep,
            budget_hint_micro: None,
            overrides: AffinityOverrides::default(),
            sampling: SamplingParams::default(),
            output_format: OutputFormat::default(),
        }
    }

    fn spec() -> ModelAffinitySpec {
        ModelAffinitySpec::minimal(
            ProviderId::Moonshot,
            "kimi-k3",
            ProtocolDialect::AnthropicMessages,
        )
    }

    #[test]
    fn build_request_system_is_top_level() {
        let body = AnthropicCodec
            .build_request(&spec(), &sample_request())
            .unwrap();
        assert_eq!(body["system"], "你是编码助手");
        // messages 内不含 system 角色
        for m in body["messages"].as_array().unwrap() {
            assert_ne!(m["role"], "system");
        }
        assert_eq!(body["max_tokens"], 4096, "max_tokens 取 spec 实测上限");
        assert_eq!(body["tools"][0]["input_schema"]["type"], "object");
    }

    #[test]
    fn build_request_preserves_assistant_blocks_in_order() {
        // P4 状态守恒:thinking → tool_use 原序回传(Kimi/MiniMax 要求)
        let mut req = sample_request();
        req.messages.push(AffinityMessage {
            role: MessageRole::Assistant,
            blocks: vec![
                ContentBlock::Thinking {
                    thinking: "先分析依赖".into(),
                    signature: Some("sig-abc".into()),
                },
                ContentBlock::ToolUse {
                    id: "call-2".into(),
                    name: "edit_file".into(),
                    input_json: r#"{"path":"lib.rs"}"#.into(),
                },
            ],
        });
        let body = AnthropicCodec.build_request(&spec(), &req).unwrap();
        let content = body["messages"][1]["content"].as_array().unwrap();
        assert_eq!(content[0]["type"], "thinking");
        assert_eq!(content[0]["signature"], "sig-abc");
        assert_eq!(content[1]["type"], "tool_use");
        assert_eq!(
            content[1]["input"]["path"], "lib.rs",
            "input 应为 JSON 对象"
        );
    }

    #[test]
    fn build_request_tool_result_goes_to_user_role() {
        let mut req = sample_request();
        req.messages.push(AffinityMessage {
            role: MessageRole::Tool,
            blocks: vec![ContentBlock::ToolResult {
                tool_use_id: "call-2".into(),
                content: "done".into(),
                is_error: false,
            }],
        });
        let body = AnthropicCodec.build_request(&spec(), &req).unwrap();
        let last = body["messages"].as_array().unwrap().last().unwrap().clone();
        assert_eq!(last["role"], "user", "方言规定 tool_result 属 user 角色");
        assert_eq!(last["content"][0]["type"], "tool_result");
    }

    #[test]
    fn parse_response_blocks_and_usage() {
        let body = r#"{
            "id": "msg-1",
            "content": [
                {"type": "thinking", "thinking": "分析中", "signature": "sig-1"},
                {"type": "text", "text": "建议如下"},
                {"type": "tool_use", "id": "call-3", "name": "edit_file", "input": {"path": "a.rs"}}
            ],
            "stop_reason": "tool_use",
            "usage": {"input_tokens": 20, "output_tokens": 15, "cache_read_input_tokens": 12}
        }"#;
        let decoded = AnthropicCodec.parse_response(body.as_bytes()).unwrap();
        assert_eq!(decoded.blocks.len(), 3);
        assert!(
            matches!(&decoded.blocks[0], ContentBlock::Thinking { signature: Some(s), .. } if s.as_ref() == "sig-1")
        );
        assert!(
            matches!(&decoded.blocks[2], ContentBlock::ToolUse { input_json, .. } if input_json.contains("a.rs"))
        );
        assert_eq!(decoded.usage.cache_hit_tokens, 12);
        assert_eq!(decoded.finish_reason, FinishReason::ToolUse);
        assert_eq!(decoded.request_id.as_deref(), Some("msg-1"));
    }

    #[test]
    fn parse_response_skips_unknown_block_types() {
        // P3 容错:虚构块类型跳过,已知块正常解析,不报错
        let body = br#"{
            "content": [
                {"type": "fictional_block", "payload": 42},
                {"type": "text", "text": "ok"}
            ],
            "stop_reason": "end_turn"
        }"#;
        let decoded = AnthropicCodec.parse_response(body).unwrap();
        assert_eq!(decoded.blocks.len(), 1);
        assert_eq!(decoded.finish_reason, FinishReason::Stop);
    }

    #[test]
    fn parse_response_rejects_missing_content() {
        assert!(matches!(
            AnthropicCodec.parse_response(br#"{"no_content": true}"#),
            Err(AffinityError::Protocol { .. })
        ));
    }

    // ── 缓存亲和集成测试(P0-2) ──

    #[test]
    fn anthropic_explicit_control_injects_cache_control() {
        // ExplicitControl 族:system 字段应注入 cache_control
        let mut spec = ModelAffinitySpec::minimal(
            ProviderId::Moonshot,
            "kimi-k3",
            ProtocolDialect::AnthropicMessages,
        );
        spec.capabilities.prompt_caching = CacheSupport::ExplicitControl;
        let mut request = simple_affinity_request("test", "你好");
        request.messages.insert(
            0,
            AffinityMessage {
                role: MessageRole::System,
                blocks: vec![ContentBlock::Text {
                    text: "You are a helpful assistant.".into(),
                }],
            },
        );
        let codec = AnthropicCodec;
        let body = codec.build_request(&spec, &request).unwrap();
        let system = body.get("system").unwrap();
        // ExplicitControl 下 system 应为块数组
        assert!(system.is_array(), "ExplicitControl 下 system 应为块数组");
        let blocks = system.as_array().unwrap();
        if let Some(first_block) = blocks.first() {
            assert_eq!(
                first_block.get("type").and_then(Value::as_str),
                Some("text")
            );
            assert!(
                first_block.get("cache_control").is_some(),
                "ExplicitControl 下 system 应注入 cache_control"
            );
        }
    }

    #[test]
    fn anthropic_none_keeps_system_as_string() {
        // None 族:system 保持字符串
        let mut spec = ModelAffinitySpec::minimal(
            ProviderId::Moonshot,
            "kimi-k3",
            ProtocolDialect::AnthropicMessages,
        );
        spec.capabilities.prompt_caching = CacheSupport::None;
        let mut request = simple_affinity_request("test", "你好");
        request.messages.insert(
            0,
            AffinityMessage {
                role: MessageRole::System,
                blocks: vec![ContentBlock::Text {
                    text: "You are a helpful assistant.".into(),
                }],
            },
        );
        let codec = AnthropicCodec;
        let body = codec.build_request(&spec, &request).unwrap();
        let system = body.get("system").unwrap();
        // None 下 system 保持字符串
        assert!(system.is_string(), "None 下 system 应为字符串");
    }

    /// 创建简单亲和请求:单条用户消息 + 默认思考偏好
    fn simple_affinity_request(intent_id: &str, text: &str) -> AffinityRequest {
        AffinityRequest {
            intent_id: intent_id.into(),
            messages: vec![AffinityMessage {
                role: MessageRole::User,
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
