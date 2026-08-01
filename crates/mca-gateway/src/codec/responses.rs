//! OpenAI Responses API 码器 — `/responses` 方言(第三方言)
//!
//! 覆盖厂商:DeepSeek V4-Flash(2026-07-31 正式版原生支持 Responses API)。
//! 未来 OpenAI 兼容的 Responses 端点亦复用本码器。
//!
//! # 方言特征(与 Chat Completions 的差异)
//! - 请求:`input` 数组(非 `messages`);tools 为**扁平** function 结构
//!   (`{type,name,description,parameters}`,不嵌套在 `function` 键下)
//! - 用户/系统消息:content 为 `input_text` 块;工具结果为独立
//!   `function_call_output` 项(`call_id` + `output`)
//! - 响应:`output` 数组含 `reasoning`/`message`(内 `output_text`)/
//!   `function_call`(`call_id`+`name`+`arguments`)三类项
//! - usage:`input_tokens`/`output_tokens`/`input_tokens_details.cached_tokens`
//!
//! # 容错(P3)
//! `output` 中未知项类型跳过不报错;缺失字段按空处理,绝不中断。

use nexus_contracts::affinity::{
    AffinityRequest, ContentBlock, FinishReason, MessageRole, ModelAffinitySpec, ProtocolDialect,
    UsageReport,
};
use serde_json::{json, Value};

use super::DecodedResponse;
use crate::error::AffinityError;

/// OpenAI Responses API 无状态码器
#[derive(Debug, Clone, Copy, Default)]
pub struct ResponsesCodec;

impl ResponsesCodec {
    /// 构造非流式请求体(`input` 数组 + 扁平 tools)
    pub fn build_request(
        &self,
        spec: &ModelAffinitySpec,
        request: &AffinityRequest,
    ) -> Result<Value, AffinityError> {
        let mut body = json!({
            "model": spec.model.as_ref(),
            "input": build_input(request)?,
        });
        if !request.tools.is_empty() {
            body["tools"] = build_tools(request)?;
        }
        Ok(body)
    }

    /// 解析非流式响应体(`output` 数组 → 统一块模型)
    pub fn parse_response(&self, body: &[u8]) -> Result<DecodedResponse, AffinityError> {
        let root: Value = serde_json::from_slice(body).map_err(|e| AffinityError::Protocol {
            dialect: ProtocolDialect::OpenAiResponses,
            reason: format!("invalid JSON: {e}"),
        })?;
        let output = root
            .get("output")
            .and_then(Value::as_array)
            .ok_or_else(|| AffinityError::Protocol {
                dialect: ProtocolDialect::OpenAiResponses,
                reason: "missing 'output' array".into(),
            })?;

        let mut blocks = Vec::new();
        for item in output {
            parse_output_item(item, &mut blocks);
        }

        // Responses 无显式 finish_reason:含 function_call 项 → ToolUse,否则 Stop
        let has_tool = blocks
            .iter()
            .any(|b| matches!(b, ContentBlock::ToolUse { .. }));
        let finish_reason = root
            .get("status")
            .and_then(Value::as_str)
            .map(|s| map_status(s, has_tool))
            .unwrap_or(if has_tool {
                FinishReason::ToolUse
            } else {
                FinishReason::Stop
            });

        Ok(DecodedResponse {
            blocks,
            usage: parse_usage(root.get("usage")),
            finish_reason,
            request_id: root.get("id").and_then(Value::as_str).map(Into::into),
        })
    }
}

/// 会话历史 → Responses `input` 数组
fn build_input(request: &AffinityRequest) -> Result<Value, AffinityError> {
    let mut input = Vec::new();
    for msg in &request.messages {
        match msg.role {
            MessageRole::System | MessageRole::User => {
                let role = if msg.role == MessageRole::System {
                    "system"
                } else {
                    "user"
                };
                input.push(json!({
                    "role": role,
                    "content": [{ "type": "input_text", "text": join_text(&msg.blocks) }],
                }));
            }
            MessageRole::Assistant => {
                // assistant 文本 → output_text 项;工具调用 → function_call 项
                let text = join_text(&msg.blocks);
                if !text.is_empty() {
                    input.push(json!({
                        "role": "assistant",
                        "content": [{ "type": "output_text", "text": text }],
                    }));
                }
                for block in &msg.blocks {
                    if let ContentBlock::ToolUse {
                        id,
                        name,
                        input_json,
                    } = block
                    {
                        input.push(json!({
                            "type": "function_call",
                            "call_id": id.as_ref(),
                            "name": name.as_ref(),
                            "arguments": input_json.as_ref(),
                        }));
                    }
                }
            }
            MessageRole::Tool => {
                for block in &msg.blocks {
                    if let ContentBlock::ToolResult {
                        tool_use_id,
                        content,
                        ..
                    } = block
                    {
                        input.push(json!({
                            "type": "function_call_output",
                            "call_id": tool_use_id.as_ref(),
                            "output": content.as_ref(),
                        }));
                    }
                }
            }
        }
    }
    Ok(Value::Array(input))
}

/// 工具声明 → Responses 扁平 function 数组
fn build_tools(request: &AffinityRequest) -> Result<Value, AffinityError> {
    let mut tools = Vec::with_capacity(request.tools.len());
    for tool in &request.tools {
        let schema: Value =
            serde_json::from_str(&tool.parameters_schema).map_err(|e| AffinityError::Protocol {
                dialect: ProtocolDialect::OpenAiResponses,
                reason: format!("tool '{}' parameters_schema invalid: {e}", tool.name),
            })?;
        // Responses tools 扁平结构:type/name/description/parameters 平级
        tools.push(json!({
            "type": "function",
            "name": tool.name.as_ref(),
            "description": tool.description.as_ref(),
            "parameters": schema,
        }));
    }
    Ok(Value::Array(tools))
}

/// 单个 output 项 → 追加统一块(未知项类型跳过,P3)
fn parse_output_item(item: &Value, blocks: &mut Vec<ContentBlock>) {
    match item.get("type").and_then(Value::as_str) {
        Some("reasoning") => {
            // reasoning 项:summary 数组或 content 字段承载思考文本
            let text = item
                .get("summary")
                .and_then(Value::as_array)
                .map(|arr| {
                    arr.iter()
                        .filter_map(|s| s.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("")
                })
                .filter(|s| !s.is_empty())
                .or_else(|| item.get("content").and_then(Value::as_str).map(Into::into));
            if let Some(text) = text.filter(|t| !t.is_empty()) {
                blocks.push(ContentBlock::Thinking {
                    thinking: text.into(),
                    signature: None,
                });
            }
        }
        Some("message") => {
            // message 项:content 内 output_text 块
            if let Some(content) = item.get("content").and_then(Value::as_array) {
                for c in content {
                    if c.get("type").and_then(Value::as_str) == Some("output_text") {
                        if let Some(text) = c.get("text").and_then(Value::as_str) {
                            if !text.is_empty() {
                                blocks.push(ContentBlock::Text { text: text.into() });
                            }
                        }
                    }
                }
            }
        }
        Some("function_call") => {
            blocks.push(ContentBlock::ToolUse {
                id: item
                    .get("call_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .into(),
                name: item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .into(),
                input_json: item
                    .get("arguments")
                    .and_then(Value::as_str)
                    .unwrap_or("{}")
                    .into(),
            });
        }
        // 未知项类型:跳过(P3 容错)
        _ => {}
    }
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

/// usage 解析(Responses 命名 + cached_tokens 在 input_tokens_details 下)
fn parse_usage(usage: Option<&Value>) -> UsageReport {
    let get = |key: &str| {
        usage
            .and_then(|u| u.get(key))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    UsageReport {
        input_tokens: get("input_tokens"),
        output_tokens: get("output_tokens"),
        cache_hit_tokens: usage
            .and_then(|u| u.pointer("/input_tokens_details/cached_tokens"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
        thinking_tokens: usage
            .and_then(|u| u.pointer("/output_tokens_details/reasoning_tokens"))
            .and_then(Value::as_u64),
    }
}

/// status → 归一(未知归 Other;completed 依 has_tool 细分)
fn map_status(raw: &str, has_tool: bool) -> FinishReason {
    match raw {
        "completed" if has_tool => FinishReason::ToolUse,
        "completed" => FinishReason::Stop,
        "incomplete" => FinishReason::MaxTokens,
        _ => FinishReason::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_contracts::affinity::{
        AffinityMessage, AffinityOverrides, ProviderId, ThinkingPreference, ToolDecl,
    };

    fn sample_request(with_tools: bool) -> AffinityRequest {
        AffinityRequest {
            intent_id: "intent-3".into(),
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
                        text: "生成测试".into(),
                    }],
                },
            ],
            tools: if with_tools {
                vec![ToolDecl {
                    name: "run_tests".into(),
                    description: "运行测试".into(),
                    parameters_schema: r#"{"type":"object"}"#.into(),
                }]
            } else {
                Vec::new()
            },
            thinking_pref: ThinkingPreference::Standard,
            budget_hint_micro: None,
            overrides: AffinityOverrides::default(),
        }
    }

    fn spec() -> ModelAffinitySpec {
        ModelAffinitySpec::minimal(
            ProviderId::DeepSeek,
            "deepseek-v4-flash",
            ProtocolDialect::OpenAiResponses,
        )
    }

    #[test]
    fn build_request_uses_input_array() {
        let body = ResponsesCodec
            .build_request(&spec(), &sample_request(false))
            .unwrap();
        assert_eq!(body["model"], "deepseek-v4-flash");
        // input 而非 messages
        assert!(body.get("messages").is_none());
        let input = body["input"].as_array().unwrap();
        assert_eq!(input[0]["role"], "system");
        assert_eq!(input[1]["content"][0]["type"], "input_text");
        assert_eq!(input[1]["content"][0]["text"], "生成测试");
    }

    #[test]
    fn build_request_flat_tools() {
        let body = ResponsesCodec
            .build_request(&spec(), &sample_request(true))
            .unwrap();
        // 扁平结构:name 平级,不嵌套在 function 键下
        assert_eq!(body["tools"][0]["type"], "function");
        assert_eq!(body["tools"][0]["name"], "run_tests");
        assert!(body["tools"][0].get("function").is_none());
    }

    #[test]
    fn build_request_tool_result_as_function_call_output() {
        let mut req = sample_request(false);
        req.messages.push(AffinityMessage {
            role: MessageRole::Assistant,
            blocks: vec![ContentBlock::ToolUse {
                id: "fc_1".into(),
                name: "run_tests".into(),
                input_json: "{}".into(),
            }],
        });
        req.messages.push(AffinityMessage {
            role: MessageRole::Tool,
            blocks: vec![ContentBlock::ToolResult {
                tool_use_id: "fc_1".into(),
                content: "3 passed".into(),
                is_error: false,
            }],
        });
        let body = ResponsesCodec.build_request(&spec(), &req).unwrap();
        let input = body["input"].as_array().unwrap();
        // function_call 项 + function_call_output 项
        assert!(input
            .iter()
            .any(|i| i["type"] == "function_call" && i["call_id"] == "fc_1"));
        assert!(input
            .iter()
            .any(|i| i["type"] == "function_call_output" && i["output"] == "3 passed"));
    }

    #[test]
    fn parse_response_reasoning_message_toolcall() {
        let body = r#"{
            "id": "resp_1",
            "status": "completed",
            "output": [
                {"type": "reasoning", "summary": [{"type": "summary_text", "text": "先跑测试"}]},
                {"type": "message", "role": "assistant", "content": [{"type": "output_text", "text": "结果如下"}]},
                {"type": "function_call", "call_id": "fc_9", "name": "run_tests", "arguments": "{\"path\":\"tests\"}"}
            ],
            "usage": {"input_tokens": 30, "output_tokens": 20, "input_tokens_details": {"cached_tokens": 10}}
        }"#;
        let decoded = ResponsesCodec.parse_response(body.as_bytes()).unwrap();
        assert_eq!(decoded.blocks.len(), 3);
        assert!(
            matches!(&decoded.blocks[0], ContentBlock::Thinking { thinking, .. } if thinking.as_ref() == "先跑测试")
        );
        assert!(
            matches!(&decoded.blocks[1], ContentBlock::Text { text } if text.as_ref() == "结果如下")
        );
        assert!(
            matches!(&decoded.blocks[2], ContentBlock::ToolUse { name, .. } if name.as_ref() == "run_tests")
        );
        assert_eq!(decoded.usage.cache_hit_tokens, 10);
        assert_eq!(decoded.finish_reason, FinishReason::ToolUse);
    }

    #[test]
    fn parse_response_skips_unknown_output_items() {
        let body = r#"{
            "output": [
                {"type": "fictional_item", "data": 1},
                {"type": "message", "content": [{"type": "output_text", "text": "ok"}]}
            ],
            "status": "completed"
        }"#;
        let decoded = ResponsesCodec.parse_response(body.as_bytes()).unwrap();
        assert_eq!(decoded.blocks.len(), 1);
        assert_eq!(decoded.finish_reason, FinishReason::Stop);
    }

    #[test]
    fn parse_response_rejects_missing_output() {
        assert!(matches!(
            ResponsesCodec.parse_response(br#"{"no_output": true}"#),
            Err(AffinityError::Protocol { .. })
        ));
    }
}
