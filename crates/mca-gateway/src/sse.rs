//! SSE 流式归一器 — 三方言流式语法 → 统一 `StreamEvent`
//!
//! 全体系最热路径(TTFT 红线 E1 的直接承载者)。设计要点:
//!
//! - **增量状态机**: `SseParser` 持有跨 chunk 残留缓冲,事件边界跨 chunk 时
//!   仅拷贝一次残留;`feed()` 每次消费一个网络 chunk,产出零或多个完整帧
//! - **数据面不进 event-bus**(ADR-065 决策 4): 归一后的 `StreamEvent` 由
//!   适配器经 bounded mpsc(256)直连调用方,per-token delta 不走 broadcast
//! - **P3 双向容错**: 不认识的事件类型/JSON 结构归入 `StreamEvent::Unknown`
//!   (原文留存),绝不报错中断流——留痕驱动 spec 更新
//!
//! # SSE 帧语法(两方言共用的传输层)
//! ```text
//! event: content_block_delta\n      ← 可选(Anthropic 用,OpenAI 无)
//! data: {"type":"..."}\n
//! \n                                ← 空行 = 帧结束
//! ```

use nexus_contracts::affinity::{FinishReason, ProtocolDialect, UsageReport};
use serde_json::Value;

/// 归一后的统一流事件 — 数据面契约(bounded mpsc 载荷)
///
/// WHY 不入 L0 契约: 流事件是网关与直连调用方之间的进程内数据面类型,
/// 不跨 event-bus 序列化传播(ADR-065 决策 4),留在 L10 避免 L0 膨胀。
#[derive(Debug, Clone, PartialEq)]
pub enum StreamEvent {
    /// 文本增量
    TextDelta(String),
    /// 思考增量(TUI Thinking 面板逐字流渲染,E2 不变量数据源)
    ThinkingDelta(String),
    /// 工具调用开始(携带调用元信息)
    ToolCallStart {
        /// 块/调用序号(方言内唯一,关联后续 Delta/End)
        index: u32,
        /// 调用标识(OpenAI 首帧携带;Anthropic content_block_start 携带)
        id: String,
        /// 工具名
        name: String,
    },
    /// 工具入参 JSON 增量片段(按序拼接得完整入参)
    ToolCallDelta {
        /// 关联的调用序号
        index: u32,
        /// JSON 文本片段
        args_fragment: String,
    },
    /// 工具调用结束
    ToolCallEnd {
        /// 关联的调用序号
        index: u32,
    },
    /// token 计量(厂商在流尾或独立帧返回)
    Usage(UsageReport),
    /// 流结束(归一终止原因)
    Done(FinishReason),
    /// 未知事件/结构 — P3 容错,原文留存(适配器发 AffinityUnknownField 留痕)
    Unknown(String),
}

/// 单个 SSE 帧(传输层解析产物,方言无关)
#[derive(Debug, Clone, PartialEq)]
pub struct SseFrame {
    /// `event:` 字段(OpenAI 方言无;Anthropic 方言为事件类型)
    pub event: Option<String>,
    /// `data:` 字段(多行 data 按 SSE 规范以 \n 连接)
    pub data: String,
}

/// SSE 传输层增量解析器 — 字节 chunk → 完整帧
///
/// # 跨 chunk 边界处理(WHY)
/// 网络 chunk 与 SSE 帧边界无对齐保证:一帧可能分落多个 chunk,一个 chunk
/// 可能含多帧。残留缓冲只保存"最后一个未完成帧"的字节,完整帧即时产出,
/// 内存占用 O(单帧最大长度) 而非 O(流总长)。
#[derive(Debug, Default)]
pub struct SseParser {
    /// 跨 chunk 残留缓冲(未凑齐帧边界的字节)
    buf: Vec<u8>,
}

impl SseParser {
    /// 创建空解析器
    pub fn new() -> Self {
        Self::default()
    }

    /// 喂入一个网络 chunk,返回本次凑齐的完整帧序列
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<SseFrame> {
        self.buf.extend_from_slice(chunk);
        let mut frames = Vec::new();
        // 帧边界 = 空行(\n\n 或 \r\n\r\n);逐帧切割直到无完整帧
        while let Some(boundary) = find_frame_boundary(&self.buf) {
            let frame_bytes: Vec<u8> = self.buf.drain(..boundary.end).collect();
            if let Some(frame) = parse_frame(&frame_bytes[..boundary.start]) {
                frames.push(frame);
            }
        }
        frames
    }

    /// 残留缓冲字节数(诊断/测试用)
    pub fn pending_bytes(&self) -> usize {
        self.buf.len()
    }
}

/// 帧边界位置:start = 帧内容结束偏移,end = 含分隔空行的消费偏移
struct FrameBoundary {
    start: usize,
    end: usize,
}

/// 查找首个帧边界(空行);兼容 \n\n 与 \r\n\r\n 混用
fn find_frame_boundary(buf: &[u8]) -> Option<FrameBoundary> {
    let mut i = 0;
    while i < buf.len() {
        if buf[i] == b'\n' {
            // \n\n
            if buf.get(i + 1) == Some(&b'\n') {
                return Some(FrameBoundary {
                    start: i,
                    end: i + 2,
                });
            }
            // \n\r\n(帧尾 LF + 空行 CRLF)
            if buf.get(i + 1) == Some(&b'\r') && buf.get(i + 2) == Some(&b'\n') {
                return Some(FrameBoundary {
                    start: i,
                    end: i + 3,
                });
            }
        }
        i += 1;
    }
    None
}

/// 帧字节 → SseFrame;纯注释帧(`:` 开头)/空帧返回 None
fn parse_frame(bytes: &[u8]) -> Option<SseFrame> {
    let text = String::from_utf8_lossy(bytes);
    let mut event = None;
    let mut data_lines: Vec<&str> = Vec::new();
    for line in text.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if let Some(rest) = line.strip_prefix("event:") {
            event = Some(rest.trim_start().to_string());
        } else if let Some(rest) = line.strip_prefix("data:") {
            data_lines.push(rest.strip_prefix(' ').unwrap_or(rest));
        }
        // `:` 注释行与其他字段(id:/retry:)按 SSE 规范忽略
    }
    if event.is_none() && data_lines.is_empty() {
        return None;
    }
    Some(SseFrame {
        event,
        data: data_lines.join("\n"),
    })
}

/// 方言流归一器 — SSE 帧 → 统一 StreamEvent(含跨帧方言状态)
///
/// WHY 有状态: Anthropic 方言的 content_block_stop 只携带 index,块类型
/// 必须从对应 content_block_start 记住(工具块 stop → ToolCallEnd,
/// 文本/思考块 stop → 无事件);OpenAI 方言无此需求但接口统一。
#[derive(Debug)]
pub struct StreamNormalizer {
    parser: SseParser,
    dialect: ProtocolDialect,
    /// Anthropic:index → 是否为 tool_use 块(块类型记忆)
    tool_block_indices: Vec<u32>,
}

impl StreamNormalizer {
    /// 按方言创建归一器
    pub fn new(dialect: ProtocolDialect) -> Self {
        Self {
            parser: SseParser::new(),
            dialect,
            tool_block_indices: Vec::new(),
        }
    }

    /// 喂入网络 chunk,产出归一后的统一流事件序列
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<StreamEvent> {
        let frames = self.parser.feed(chunk);
        let mut events = Vec::new();
        for frame in frames {
            match self.dialect {
                ProtocolDialect::OpenAiChat => normalize_openai_frame(&frame, &mut events),
                ProtocolDialect::AnthropicMessages => {
                    self.normalize_anthropic_frame(&frame, &mut events)
                }
                ProtocolDialect::OpenAiResponses => {
                    self.normalize_responses_frame(&frame, &mut events)
                }
            }
        }
        events
    }

    /// Anthropic 方言帧归一(event 字段驱动)
    fn normalize_anthropic_frame(&mut self, frame: &SseFrame, out: &mut Vec<StreamEvent>) {
        let Ok(root) = serde_json::from_str::<Value>(&frame.data) else {
            if !frame.data.is_empty() {
                out.push(StreamEvent::Unknown(frame.data.clone()));
            }
            return;
        };
        match frame.event.as_deref().unwrap_or_default() {
            "content_block_start" => {
                let index = root.get("index").and_then(Value::as_u64).unwrap_or(0) as u32;
                let block = root.get("content_block");
                if block.and_then(|b| b.get("type")).and_then(Value::as_str) == Some("tool_use") {
                    // 记住工具块 index,content_block_stop 时据此产出 ToolCallEnd
                    self.tool_block_indices.push(index);
                    out.push(StreamEvent::ToolCallStart {
                        index,
                        id: str_field(block, "id"),
                        name: str_field(block, "name"),
                    });
                }
                // text/thinking 块的 start 无载荷,不产事件
            }
            "content_block_delta" => {
                let index = root.get("index").and_then(Value::as_u64).unwrap_or(0) as u32;
                let delta = root.get("delta");
                match delta.and_then(|d| d.get("type")).and_then(Value::as_str) {
                    Some("text_delta") => {
                        out.push(StreamEvent::TextDelta(str_field(delta, "text")));
                    }
                    Some("thinking_delta") => {
                        out.push(StreamEvent::ThinkingDelta(str_field(delta, "thinking")));
                    }
                    Some("input_json_delta") => {
                        out.push(StreamEvent::ToolCallDelta {
                            index,
                            args_fragment: str_field(delta, "partial_json"),
                        });
                    }
                    // signature_delta 等已知无需转发的增量静默跳过;
                    // 真正未知的 delta 类型整帧留痕(P3)
                    Some("signature_delta") => {}
                    _ => out.push(StreamEvent::Unknown(frame.data.clone())),
                }
            }
            "content_block_stop" => {
                let index = root.get("index").and_then(Value::as_u64).unwrap_or(0) as u32;
                if let Some(pos) = self.tool_block_indices.iter().position(|&i| i == index) {
                    self.tool_block_indices.swap_remove(pos);
                    out.push(StreamEvent::ToolCallEnd { index });
                }
            }
            "message_delta" => {
                // 流尾:stop_reason + 累计 usage
                if let Some(reason) = root.pointer("/delta/stop_reason").and_then(Value::as_str) {
                    if let Some(usage) = root.get("usage") {
                        out.push(StreamEvent::Usage(anthropic_usage(usage)));
                    }
                    out.push(StreamEvent::Done(map_anthropic_stop(reason)));
                }
            }
            // message_start(首帧 usage 输入侧)转发计量;ping/message_stop 静默
            "message_start" => {
                if let Some(usage) = root.pointer("/message/usage") {
                    out.push(StreamEvent::Usage(anthropic_usage(usage)));
                }
            }
            "ping" | "message_stop" => {}
            // 未知事件类型:整帧留痕(P3)
            _ => out.push(StreamEvent::Unknown(frame.data.clone())),
        }
    }

    /// Responses 方言帧归一(event 字段携带 `response.*` 事件类型)
    ///
    /// Responses 流式事件与 Chat 不同:用具体事件名驱动(同 Anthropic 风格),
    /// 工具调用通过 output_item.added/done 包裹 function_call 项。
    fn normalize_responses_frame(&mut self, frame: &SseFrame, out: &mut Vec<StreamEvent>) {
        let Ok(root) = serde_json::from_str::<Value>(&frame.data) else {
            if !frame.data.is_empty() {
                out.push(StreamEvent::Unknown(frame.data.clone()));
            }
            return;
        };
        match frame.event.as_deref().unwrap_or_default() {
            "response.output_text.delta" => {
                out.push(StreamEvent::TextDelta(str_field(Some(&root), "delta")));
            }
            // 思考摘要增量(reasoning summary 逐字流)
            "response.reasoning_summary_text.delta" | "response.reasoning_text.delta" => {
                out.push(StreamEvent::ThinkingDelta(str_field(Some(&root), "delta")));
            }
            // 新 output 项:仅 function_call 项产出 ToolCallStart(记住 index)
            "response.output_item.added" => {
                let index = root
                    .get("output_index")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as u32;
                let item = root.get("item");
                if item.and_then(|i| i.get("type")).and_then(Value::as_str) == Some("function_call")
                {
                    self.tool_block_indices.push(index);
                    out.push(StreamEvent::ToolCallStart {
                        index,
                        // Responses function_call 用 call_id 关联工具结果
                        id: str_field(item, "call_id"),
                        name: str_field(item, "name"),
                    });
                }
            }
            "response.function_call_arguments.delta" => {
                let index = root
                    .get("output_index")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as u32;
                out.push(StreamEvent::ToolCallDelta {
                    index,
                    args_fragment: str_field(Some(&root), "delta"),
                });
            }
            "response.output_item.done" => {
                let index = root
                    .get("output_index")
                    .and_then(Value::as_u64)
                    .unwrap_or(0) as u32;
                if let Some(pos) = self.tool_block_indices.iter().position(|&i| i == index) {
                    self.tool_block_indices.swap_remove(pos);
                    out.push(StreamEvent::ToolCallEnd { index });
                }
            }
            // 流尾:response.completed 携带累计 usage,并归一终止原因
            "response.completed" | "response.incomplete" => {
                if let Some(usage) = root.pointer("/response/usage") {
                    out.push(StreamEvent::Usage(responses_usage(usage)));
                }
                let has_tool = root
                    .pointer("/response/output")
                    .and_then(Value::as_array)
                    .is_some_and(|arr| {
                        arr.iter()
                            .any(|i| i.get("type").and_then(Value::as_str) == Some("function_call"))
                    });
                out.push(StreamEvent::Done(if has_tool {
                    FinishReason::ToolUse
                } else if frame.event.as_deref() == Some("response.incomplete") {
                    FinishReason::MaxTokens
                } else {
                    FinishReason::Stop
                }));
            }
            // 已知无需转发的事件(内部生命周期)静默
            "response.created"
            | "response.in_progress"
            | "response.output_text.done"
            | "response.content_part.added"
            | "response.content_part.done"
            | "response.reasoning_summary_text.done"
            | "response.function_call_arguments.done" => {}
            // 未知事件类型:整帧留痕(P3)
            _ => out.push(StreamEvent::Unknown(frame.data.clone())),
        }
    }
}

/// OpenAI 方言帧归一(data 内 choices[0].delta 驱动,无 event 字段)
fn normalize_openai_frame(frame: &SseFrame, out: &mut Vec<StreamEvent>) {
    // [DONE] 是传输层终止哨兵;Done 语义已由 finish_reason 帧承载,静默吞掉
    if frame.data.trim() == "[DONE]" {
        return;
    }
    let Ok(root) = serde_json::from_str::<Value>(&frame.data) else {
        if !frame.data.is_empty() {
            out.push(StreamEvent::Unknown(frame.data.clone()));
        }
        return;
    };
    // usage 独立帧(stream_options.include_usage)或尾帧携带
    if let Some(usage) = root.get("usage").filter(|u| !u.is_null()) {
        out.push(StreamEvent::Usage(openai_usage(usage)));
    }
    let Some(choice) = root.pointer("/choices/0") else {
        // 无 choices 且无 usage 的帧无法归类,留痕(P3)
        if root.get("usage").is_none() {
            out.push(StreamEvent::Unknown(frame.data.clone()));
        }
        return;
    };
    if let Some(delta) = choice.get("delta") {
        if let Some(thinking) = delta.get("reasoning_content").and_then(Value::as_str) {
            if !thinking.is_empty() {
                out.push(StreamEvent::ThinkingDelta(thinking.to_string()));
            }
        }
        if let Some(text) = delta.get("content").and_then(Value::as_str) {
            if !text.is_empty() {
                out.push(StreamEvent::TextDelta(text.to_string()));
            }
        }
        if let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) {
            for call in calls {
                normalize_openai_tool_delta(call, out);
            }
        }
    }
    if let Some(reason) = choice.get("finish_reason").and_then(Value::as_str) {
        // OpenAI 方言无显式 ToolCallEnd,finish_reason 帧即全部调用结束
        out.push(StreamEvent::Done(map_openai_finish(reason)));
    }
}

/// OpenAI tool_calls 增量:首片(含 function.name)→ Start,后续片 → Delta
fn normalize_openai_tool_delta(call: &Value, out: &mut Vec<StreamEvent>) {
    let index = call.get("index").and_then(Value::as_u64).unwrap_or(0) as u32;
    let function = call.get("function");
    let name = function
        .and_then(|f| f.get("name"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !name.is_empty() {
        out.push(StreamEvent::ToolCallStart {
            index,
            id: call
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            name: name.to_string(),
        });
    }
    if let Some(args) = function
        .and_then(|f| f.get("arguments"))
        .and_then(Value::as_str)
    {
        if !args.is_empty() {
            out.push(StreamEvent::ToolCallDelta {
                index,
                args_fragment: args.to_string(),
            });
        }
    }
}

/// 从 Value 可选容器提取字符串字段(缺失归空,P3 不报错)
fn str_field(container: Option<&Value>, key: &str) -> String {
    container
        .and_then(|c| c.get(key))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// OpenAI usage 命名 → UsageReport
fn openai_usage(usage: &Value) -> UsageReport {
    let get = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    UsageReport {
        input_tokens: get("prompt_tokens"),
        output_tokens: get("completion_tokens"),
        cache_hit_tokens: get("prompt_cache_hit_tokens"),
        thinking_tokens: usage
            .pointer("/completion_tokens_details/reasoning_tokens")
            .and_then(Value::as_u64),
    }
}

/// Anthropic usage 命名 → UsageReport
fn anthropic_usage(usage: &Value) -> UsageReport {
    let get = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    UsageReport {
        input_tokens: get("input_tokens"),
        output_tokens: get("output_tokens"),
        cache_hit_tokens: get("cache_read_input_tokens"),
        thinking_tokens: None,
    }
}

/// Responses usage 命名 → UsageReport(cached_tokens 在 input_tokens_details 下)
fn responses_usage(usage: &Value) -> UsageReport {
    let get = |key: &str| usage.get(key).and_then(Value::as_u64).unwrap_or(0);
    UsageReport {
        input_tokens: get("input_tokens"),
        output_tokens: get("output_tokens"),
        cache_hit_tokens: usage
            .pointer("/input_tokens_details/cached_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0),
        thinking_tokens: usage
            .pointer("/output_tokens_details/reasoning_tokens")
            .and_then(Value::as_u64),
    }
}

/// OpenAI finish_reason → 归一(与 codec::openai_chat 保持一致)
fn map_openai_finish(raw: &str) -> FinishReason {
    match raw {
        "stop" => FinishReason::Stop,
        "length" => FinishReason::MaxTokens,
        "tool_calls" => FinishReason::ToolUse,
        "content_filter" => FinishReason::ContentFilter,
        _ => FinishReason::Other,
    }
}

/// Anthropic stop_reason → 归一(与 codec::anthropic 保持一致)
fn map_anthropic_stop(raw: &str) -> FinishReason {
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

    // ---------- 传输层帧解析 ----------

    #[test]
    fn parser_single_frame() {
        let mut p = SseParser::new();
        let frames = p.feed(b"data: {\"a\":1}\n\n");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data, "{\"a\":1}");
        assert_eq!(frames[0].event, None);
        assert_eq!(p.pending_bytes(), 0);
    }

    #[test]
    fn parser_multiple_frames_one_chunk() {
        let mut p = SseParser::new();
        let frames = p.feed(b"data: 1\n\ndata: 2\n\n");
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].data, "1");
        assert_eq!(frames[1].data, "2");
    }

    #[test]
    fn parser_frame_split_across_chunks() {
        // 跨 chunk 边界:帧被任意切割也必须正确重组(SSE 归一器核心不变量)
        let mut p = SseParser::new();
        assert!(p.feed(b"data: {\"he").is_empty());
        assert!(p.feed(b"llo\":tr").is_empty());
        let frames = p.feed(b"ue}\n\n");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data, "{\"hello\":true}");
    }

    #[test]
    fn parser_crlf_and_event_field() {
        let mut p = SseParser::new();
        let frames = p.feed(b"event: message_start\r\ndata: {}\r\n\r\n");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].event.as_deref(), Some("message_start"));
        assert_eq!(frames[0].data, "{}");
    }

    #[test]
    fn parser_ignores_comment_frames() {
        let mut p = SseParser::new();
        // 纯注释帧(心跳)不产出;后续正常帧不受影响
        let frames = p.feed(b": keepalive\n\ndata: x\n\n");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].data, "x");
    }

    // ---------- OpenAI 方言归一 ----------

    fn feed_openai(payload: &str) -> Vec<StreamEvent> {
        let mut n = StreamNormalizer::new(ProtocolDialect::OpenAiChat);
        n.feed(format!("data: {payload}\n\n").as_bytes())
    }

    #[test]
    fn openai_text_and_thinking_delta() {
        let events =
            feed_openai(r#"{"choices":[{"delta":{"reasoning_content":"想","content":"好"}}]}"#);
        assert_eq!(
            events,
            vec![
                StreamEvent::ThinkingDelta("想".into()),
                StreamEvent::TextDelta("好".into()),
            ]
        );
    }

    #[test]
    fn openai_tool_call_start_then_delta() {
        let start = feed_openai(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"read_file","arguments":""}}]}}]}"#,
        );
        assert_eq!(
            start,
            vec![StreamEvent::ToolCallStart {
                index: 0,
                id: "c1".into(),
                name: "read_file".into()
            }]
        );
        let delta = feed_openai(
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"{\"pa"}}]}}]}"#,
        );
        assert_eq!(
            delta,
            vec![StreamEvent::ToolCallDelta {
                index: 0,
                args_fragment: "{\"pa".into()
            }]
        );
    }

    #[test]
    fn openai_finish_reason_maps_to_done() {
        let events = feed_openai(r#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#);
        assert_eq!(events, vec![StreamEvent::Done(FinishReason::ToolUse)]);
    }

    #[test]
    fn openai_done_sentinel_is_silent() {
        let mut n = StreamNormalizer::new(ProtocolDialect::OpenAiChat);
        assert!(n.feed(b"data: [DONE]\n\n").is_empty());
    }

    #[test]
    fn openai_usage_frame() {
        let events = feed_openai(
            r#"{"choices":[],"usage":{"prompt_tokens":7,"completion_tokens":3,"prompt_cache_hit_tokens":5}}"#,
        );
        assert_eq!(
            events,
            vec![StreamEvent::Usage(UsageReport {
                input_tokens: 7,
                output_tokens: 3,
                cache_hit_tokens: 5,
                thinking_tokens: None
            })]
        );
    }

    #[test]
    fn openai_unknown_payload_preserved() {
        // P3 容错:非 JSON 与无法归类的 JSON 都以 Unknown 留痕,不 panic 不丢弃
        let mut n = StreamNormalizer::new(ProtocolDialect::OpenAiChat);
        let events = n.feed(b"data: not-json-at-all\n\n");
        assert_eq!(events, vec![StreamEvent::Unknown("not-json-at-all".into())]);
    }

    // ---------- Anthropic 方言归一 ----------

    fn feed_anthropic(event: &str, payload: &str) -> Vec<StreamEvent> {
        let mut n = StreamNormalizer::new(ProtocolDialect::AnthropicMessages);
        n.feed(format!("event: {event}\ndata: {payload}\n\n").as_bytes())
    }

    #[test]
    fn anthropic_text_thinking_deltas() {
        assert_eq!(
            feed_anthropic(
                "content_block_delta",
                r#"{"index":0,"delta":{"type":"text_delta","text":"你"}}"#
            ),
            vec![StreamEvent::TextDelta("你".into())]
        );
        assert_eq!(
            feed_anthropic(
                "content_block_delta",
                r#"{"index":0,"delta":{"type":"thinking_delta","thinking":"思"}}"#
            ),
            vec![StreamEvent::ThinkingDelta("思".into())]
        );
    }

    #[test]
    fn anthropic_tool_block_lifecycle() {
        // Start → Delta → Stop 全周期:stop 只对工具块产出 End(块类型记忆)
        let mut n = StreamNormalizer::new(ProtocolDialect::AnthropicMessages);
        let start = n.feed(
            b"event: content_block_start\ndata: {\"index\":1,\"content_block\":{\"type\":\"tool_use\",\"id\":\"t1\",\"name\":\"edit_file\"}}\n\n",
        );
        assert_eq!(
            start,
            vec![StreamEvent::ToolCallStart {
                index: 1,
                id: "t1".into(),
                name: "edit_file".into()
            }]
        );
        let delta = n.feed(
            b"event: content_block_delta\ndata: {\"index\":1,\"delta\":{\"type\":\"input_json_delta\",\"partial_json\":\"{\\\"p\"}}\n\n",
        );
        assert_eq!(
            delta,
            vec![StreamEvent::ToolCallDelta {
                index: 1,
                args_fragment: "{\"p".into()
            }]
        );
        let stop = n.feed(b"event: content_block_stop\ndata: {\"index\":1}\n\n");
        assert_eq!(stop, vec![StreamEvent::ToolCallEnd { index: 1 }]);
        // 非工具块的 stop 不产事件
        let text_stop = n.feed(b"event: content_block_stop\ndata: {\"index\":0}\n\n");
        assert!(text_stop.is_empty());
    }

    #[test]
    fn anthropic_message_delta_yields_usage_and_done() {
        let events = feed_anthropic(
            "message_delta",
            r#"{"delta":{"stop_reason":"end_turn"},"usage":{"input_tokens":10,"output_tokens":8,"cache_read_input_tokens":6}}"#,
        );
        assert_eq!(
            events,
            vec![
                StreamEvent::Usage(UsageReport {
                    input_tokens: 10,
                    output_tokens: 8,
                    cache_hit_tokens: 6,
                    thinking_tokens: None
                }),
                StreamEvent::Done(FinishReason::Stop),
            ]
        );
    }

    #[test]
    fn anthropic_ping_silent_unknown_event_preserved() {
        assert!(feed_anthropic("ping", "{}").is_empty());
        // 未知事件类型整帧留痕(P3:虚构事件注入不报错)
        let events = feed_anthropic("fictional_event", r#"{"x":1}"#);
        assert_eq!(events, vec![StreamEvent::Unknown(r#"{"x":1}"#.into())]);
    }

    #[test]
    fn anthropic_signature_delta_is_silent() {
        // signature_delta 是已知无需转发的增量(签名在非流式回传时校验)
        assert!(feed_anthropic(
            "content_block_delta",
            r#"{"index":0,"delta":{"type":"signature_delta","signature":"abc"}}"#
        )
        .is_empty());
    }

    // ---------- Responses 方言归一 ----------

    fn feed_responses(event: &str, payload: &str) -> Vec<StreamEvent> {
        let mut n = StreamNormalizer::new(ProtocolDialect::OpenAiResponses);
        n.feed(format!("event: {event}\ndata: {payload}\n\n").as_bytes())
    }

    #[test]
    fn responses_text_and_reasoning_delta() {
        assert_eq!(
            feed_responses("response.output_text.delta", r#"{"delta":"你好"}"#),
            vec![StreamEvent::TextDelta("你好".into())]
        );
        assert_eq!(
            feed_responses(
                "response.reasoning_summary_text.delta",
                r#"{"delta":"思考"}"#
            ),
            vec![StreamEvent::ThinkingDelta("思考".into())]
        );
    }

    #[test]
    fn responses_tool_call_lifecycle() {
        let mut n = StreamNormalizer::new(ProtocolDialect::OpenAiResponses);
        let start = n.feed(
            b"event: response.output_item.added\ndata: {\"output_index\":0,\"item\":{\"type\":\"function_call\",\"call_id\":\"fc1\",\"name\":\"run_tests\"}}\n\n",
        );
        assert_eq!(
            start,
            vec![StreamEvent::ToolCallStart {
                index: 0,
                id: "fc1".into(),
                name: "run_tests".into()
            }]
        );
        let delta = n.feed(
            b"event: response.function_call_arguments.delta\ndata: {\"output_index\":0,\"delta\":\"{\\\"p\"}\n\n",
        );
        assert_eq!(
            delta,
            vec![StreamEvent::ToolCallDelta {
                index: 0,
                args_fragment: "{\"p".into()
            }]
        );
        let done = n.feed(b"event: response.output_item.done\ndata: {\"output_index\":0}\n\n");
        assert_eq!(done, vec![StreamEvent::ToolCallEnd { index: 0 }]);
    }

    #[test]
    fn responses_completed_yields_usage_and_done() {
        let events = feed_responses(
            "response.completed",
            r#"{"response":{"output":[{"type":"message"}],"usage":{"input_tokens":12,"output_tokens":8,"input_tokens_details":{"cached_tokens":4}}}}"#,
        );
        assert_eq!(
            events,
            vec![
                StreamEvent::Usage(UsageReport {
                    input_tokens: 12,
                    output_tokens: 8,
                    cache_hit_tokens: 4,
                    thinking_tokens: None
                }),
                StreamEvent::Done(FinishReason::Stop),
            ]
        );
    }

    #[test]
    fn responses_lifecycle_events_silent_unknown_preserved() {
        assert!(feed_responses("response.created", "{}").is_empty());
        assert!(feed_responses("response.in_progress", "{}").is_empty());
        // 未知事件整帧留痕(P3)
        let events = feed_responses("response.fictional_event", r#"{"x":1}"#);
        assert_eq!(events, vec![StreamEvent::Unknown(r#"{"x":1}"#.into())]);
    }
}
