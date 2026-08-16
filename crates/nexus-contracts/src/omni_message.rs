//! OmniMessage 协议 — 模型-环境解耦统一消息协议（设计文档 §5.3.2）
//!
//! 对应架构层: **L0 Contracts**（nexus-contracts）
//! 对应设计源: `Chimera CLI 十层架构深度打磨与优化方案 最新版.md` §5.3.2
//! 对应论文: PenguinHarness（OmniMessage 解耦模型与环境）
//!
//! # 核心职责
//!
//! 承载 OmniMessage 统一消息协议，解耦 LLM 调用与环境执行：
//! - 模型调用请求/响应（ModelRequest / ModelResponse）
//! - 工具调用请求/结果（ToolRequest / ToolResult）
//! - 状态更新（StateUpdate）
//! - 轨迹记录（TraceRecord）
//!
//! # 设计约束（ADR-033）
//!
//! - **纯类型 + 零逻辑**: 仅类型定义，不含业务逻辑
//! - **零 crate 依赖**: 仅 `serde` derive（ADR-033 白名单例外）
//! - **JSON 字段用 `Box<str>`**: 遵循 affinity.rs 先例，L0 零 crate 依赖铁律
//!   禁止引入 `serde_json::Value`；由 L10 Codec 负责 JSON 解析/构造，
//!   L0 仅承载传输形态（JSON 字符串）
//! - **f32 字段不 derive Eq/Hash**: reward 等浮点字段仅 `PartialEq`
//!
//! # 与 EventBus 的关系
//!
//! OmniMessage 是**应用层消息协议**（L10 Interface ↔ 外部环境），
//! EventBus 是**系统层事件通道**（L1-L10 内部跨层通信）。两者互补：
//! - OmniMessage 承载 LLM 交互与工具调用的标准化载荷
//! - EventBus 承载系统内部状态变更的广播与 Critical 事件 mpsc 保障
//!
//! # 示例
//!
//! ```
//! use nexus_contracts::omni_message::{OmniMessage, ModelConfig, TokenUsage};
//!
//! let request = OmniMessage::ModelRequest {
//!     request_id: "req-001".to_string(),
//!     prompt: "Hello, world!".to_string(),
//!     model_config: ModelConfig::default_config(),
//!     timestamp: 1_700_000_000_000,
//! };
//! let json = serde_json::to_string(&request).expect("序列化失败");
//! assert!(json.contains("ModelRequest"));
//! ```

use serde::{Deserialize, Serialize};

// ============================================================
// 子结构体定义
// ============================================================

/// 模型配置 — LLM 调用的采样与路由参数
///
/// 承载模型调用的完整配置，与 MCA 亲和体系（affinity.rs）协同：
/// `provider` / `model` 字段对应 `ProviderId` / 模型名，
/// `temperature` / `top_p` / `max_tokens` 对应 D3 生成控制契约。
///
/// # f32 约束
///
/// temperature / top_p 为 f32 字段，故仅 `PartialEq`（不 derive `Eq`/`Hash`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelConfig {
    /// 厂商标识（如 "zhipu" / "deepseek" / "moonshot"）
    pub provider: String,
    /// 模型名（如 "glm-5.2" / "deepseek-v4" / "kimi-k2.7"）
    pub model: String,
    /// 采样温度 [0.0, 2.0]
    pub temperature: f32,
    /// Top-P 采样阈值 [0.0, 1.0]
    pub top_p: f32,
    /// 最大输出 token 数
    pub max_tokens: usize,
    /// 是否启用流式响应
    pub stream: bool,
}

impl ModelConfig {
    /// 创建默认模型配置
    ///
    /// 默认值：provider="default" / model="default" / temperature=0.7 /
    /// top_p=0.9 / max_tokens=4096 / stream=false
    pub fn default_config() -> Self {
        Self {
            provider: "default".to_string(),
            model: "default".to_string(),
            temperature: 0.7,
            top_p: 0.9,
            max_tokens: 4096,
            stream: false,
        }
    }
}

/// Token 用量 — LLM 调用的 token 消耗统计
///
/// 与 MCA 亲和体系的 `UsageReport`（affinity.rs）语义对齐，
/// 但字段更精简（仅 prompt/completion/total 三项）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TokenUsage {
    /// 输入 token 数（prompt）
    pub prompt_tokens: u64,
    /// 输出 token 数（completion）
    pub completion_tokens: u64,
    /// 总 token 数（prompt + completion）
    pub total_tokens: u64,
}

impl TokenUsage {
    /// 创建 Token 用量记录
    pub fn new(prompt_tokens: u64, completion_tokens: u64) -> Self {
        Self {
            prompt_tokens,
            completion_tokens,
            total_tokens: prompt_tokens + completion_tokens,
        }
    }
}

// ============================================================
// OmniMessage 枚举
// ============================================================

/// OmniMessage — 统一消息协议，解耦 LLM 与环境
///
/// 6 个变体覆盖 Agent 与外部环境交互的完整生命周期：
///
/// ```text
/// Agent → ModelRequest → LLM → ModelResponse → Agent
/// Agent → ToolRequest → Environment → ToolResult → Agent
/// Agent → StateUpdate → Environment（状态同步）
/// Agent → TraceRecord → TrajectoryStore（轨迹记录）
/// ```
///
/// # 序列化
///
/// 使用 serde 默认的外部标签枚举序列化（`{"ModelRequest": {...}}`），
/// 与 EventBus 的 NexusEvent 序列化格式一致。
///
/// # f32 约束
///
/// TraceRecord 的 reward 为 f32 字段，故 OmniMessage 仅 `PartialEq`。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum OmniMessage {
    /// 模型调用请求 — Agent 向 LLM 发送的推理请求
    ModelRequest {
        /// 请求唯一标识（用于请求-响应配对）
        request_id: String,
        /// 提示词（完整 prompt，含 system/user/assistant 角色标记）
        prompt: String,
        /// 模型配置（采样参数 + 路由信息）
        model_config: ModelConfig,
        /// 时间戳（毫秒，UTC）
        timestamp: u64,
    },
    /// 模型响应 — LLM 返回的推理结果
    ModelResponse {
        /// 请求唯一标识（与 ModelRequest.request_id 配对）
        request_id: String,
        /// 生成内容（LLM 输出的完整文本）
        content: String,
        /// Token 用量统计
        usage: TokenUsage,
        /// 时间戳（毫秒，UTC）
        timestamp: u64,
    },
    /// 工具调用请求 — Agent 向环境发送的工具执行请求
    ToolRequest {
        /// 请求唯一标识（用于请求-结果配对）
        request_id: String,
        /// 工具名称（如 "read_file" / "edit_file" / "bash"）
        tool_name: String,
        /// 工具参数（JSON 字符串，由 L10 Codec 解析/构造）
        ///
        /// WHY `Box<str>` 而非 `serde_json::Value`: L0 零 crate 依赖铁律
        /// 禁止引入 serde_json；由 L10 Codec 负责 JSON 解析/构造，
        /// L0 仅承载传输形态（JSON 字符串）。遵循 affinity.rs 先例。
        parameters: Box<str>,
        /// 时间戳（毫秒，UTC）
        timestamp: u64,
    },
    /// 工具执行结果 — 环境返回的工具执行输出
    ToolResult {
        /// 请求唯一标识（与 ToolRequest.request_id 配对）
        request_id: String,
        /// 是否成功
        success: bool,
        /// 输出内容（工具执行的标准输出/返回值）
        output: String,
        /// 错误信息（success=false 时携带）
        error: Option<String>,
        /// 时间戳（毫秒，UTC）
        timestamp: u64,
    },
    /// 状态更新 — Agent 向环境同步的状态变更
    StateUpdate {
        /// 状态键（如 "current_file" / "cursor_position" / "active_panel"）
        key: String,
        /// 状态值（JSON 字符串，由 L10 Codec 解析/构造）
        ///
        /// WHY `Box<str>`: 同 ToolRequest.parameters，保持 L0 零 crate 依赖。
        value: Box<str>,
        /// 时间戳（毫秒，UTC）
        timestamp: u64,
    },
    /// 轨迹记录 — Agent 执行的步骤记录（RL 训练数据源）
    ///
    /// 与 RLExperience（rl_types.rs）语义对齐，但面向应用层轨迹记录
    /// 而非 RL 训练。TraceRecord 可被 OpenForge-Proxy 拦截并重建为
    /// 标准 RL 轨迹（设计文档 §6.3.1 TrajectoryReconstructor）。
    TraceRecord {
        /// 步骤序号（0 起）
        step: u32,
        /// 执行的动作描述（自然语言）
        action: String,
        /// 观测结果（环境反馈）
        observation: String,
        /// 奖励信号（f32，R2 冻结面外仅作观测）
        reward: f32,
        /// 时间戳（毫秒，UTC）
        timestamp: u64,
    },
}

impl OmniMessage {
    /// 返回消息类型名称（用于日志/调试）
    pub const fn message_type(&self) -> &'static str {
        match self {
            Self::ModelRequest { .. } => "ModelRequest",
            Self::ModelResponse { .. } => "ModelResponse",
            Self::ToolRequest { .. } => "ToolRequest",
            Self::ToolResult { .. } => "ToolResult",
            Self::StateUpdate { .. } => "StateUpdate",
            Self::TraceRecord { .. } => "TraceRecord",
        }
    }

    /// 返回消息时间戳（毫秒）
    pub const fn timestamp(&self) -> u64 {
        match self {
            Self::ModelRequest { timestamp, .. } => *timestamp,
            Self::ModelResponse { timestamp, .. } => *timestamp,
            Self::ToolRequest { timestamp, .. } => *timestamp,
            Self::ToolResult { timestamp, .. } => *timestamp,
            Self::StateUpdate { timestamp, .. } => *timestamp,
            Self::TraceRecord { timestamp, .. } => *timestamp,
        }
    }

    /// 返回请求 ID（若消息类型携带 request_id）
    ///
    /// ModelRequest / ModelResponse / ToolRequest / ToolResult 携带 request_id，
    /// StateUpdate / TraceRecord 不携带（返回 None）。
    pub fn request_id(&self) -> Option<&str> {
        match self {
            Self::ModelRequest { request_id, .. } => Some(request_id),
            Self::ModelResponse { request_id, .. } => Some(request_id),
            Self::ToolRequest { request_id, .. } => Some(request_id),
            Self::ToolResult { request_id, .. } => Some(request_id),
            Self::StateUpdate { .. } => None,
            Self::TraceRecord { .. } => None,
        }
    }
}

// ============================================================
// 测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ----------------------------------------------------------
    // 辅助构造函数
    // ----------------------------------------------------------

    fn make_model_config() -> ModelConfig {
        ModelConfig {
            provider: "zhipu".to_string(),
            model: "glm-5.2".to_string(),
            temperature: 0.7,
            top_p: 0.9,
            max_tokens: 4096,
            stream: false,
        }
    }

    fn make_token_usage() -> TokenUsage {
        TokenUsage::new(100, 50)
    }

    // ----------------------------------------------------------
    // ModelConfig 测试
    // ----------------------------------------------------------

    #[test]
    fn test_model_config_default() {
        let c = ModelConfig::default_config();
        assert_eq!(c.provider, "default");
        assert_eq!(c.model, "default");
        assert!((c.temperature - 0.7).abs() < f32::EPSILON);
        assert!((c.top_p - 0.9).abs() < f32::EPSILON);
        assert_eq!(c.max_tokens, 4096);
        assert!(!c.stream);
    }

    #[test]
    fn test_model_config_serde_roundtrip() {
        let c = make_model_config();
        let json = serde_json::to_string(&c).expect("序列化失败");
        let back: ModelConfig = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(c, back);
    }

    // ----------------------------------------------------------
    // TokenUsage 测试
    // ----------------------------------------------------------

    #[test]
    fn test_token_usage_new() {
        let u = TokenUsage::new(100, 50);
        assert_eq!(u.prompt_tokens, 100);
        assert_eq!(u.completion_tokens, 50);
        assert_eq!(u.total_tokens, 150);
    }

    #[test]
    fn test_token_usage_serde_roundtrip() {
        let u = make_token_usage();
        let json = serde_json::to_string(&u).expect("序列化失败");
        let back: TokenUsage = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(u, back);
    }

    // ----------------------------------------------------------
    // OmniMessage 变体测试
    // ----------------------------------------------------------

    #[test]
    fn test_model_request_serde_roundtrip() {
        let msg = OmniMessage::ModelRequest {
            request_id: "req-001".to_string(),
            prompt: "Hello".to_string(),
            model_config: make_model_config(),
            timestamp: 1_700_000_000_000,
        };
        let json = serde_json::to_string(&msg).expect("序列化失败");
        let back: OmniMessage = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(msg, back);
        assert_eq!(msg.message_type(), "ModelRequest");
        assert_eq!(msg.request_id(), Some("req-001"));
    }

    #[test]
    fn test_model_response_serde_roundtrip() {
        let msg = OmniMessage::ModelResponse {
            request_id: "req-001".to_string(),
            content: "Hi there".to_string(),
            usage: make_token_usage(),
            timestamp: 1_700_000_001_000,
        };
        let json = serde_json::to_string(&msg).expect("序列化失败");
        let back: OmniMessage = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(msg, back);
        assert_eq!(msg.message_type(), "ModelResponse");
        assert_eq!(msg.request_id(), Some("req-001"));
    }

    #[test]
    fn test_tool_request_serde_roundtrip() {
        let msg = OmniMessage::ToolRequest {
            request_id: "req-002".to_string(),
            tool_name: "read_file".to_string(),
            parameters: r#"{"path": "src/main.rs"}"#.into(),
            timestamp: 1_700_000_002_000,
        };
        let json = serde_json::to_string(&msg).expect("序列化失败");
        let back: OmniMessage = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(msg, back);
        assert_eq!(msg.message_type(), "ToolRequest");
        assert_eq!(msg.request_id(), Some("req-002"));
    }

    #[test]
    fn test_tool_result_serde_roundtrip() {
        let msg = OmniMessage::ToolResult {
            request_id: "req-002".to_string(),
            success: true,
            output: "fn main() {}".to_string(),
            error: None,
            timestamp: 1_700_000_003_000,
        };
        let json = serde_json::to_string(&msg).expect("序列化失败");
        let back: OmniMessage = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(msg, back);
        assert_eq!(msg.message_type(), "ToolResult");
    }

    #[test]
    fn test_tool_result_with_error_serde_roundtrip() {
        let msg = OmniMessage::ToolResult {
            request_id: "req-003".to_string(),
            success: false,
            output: String::new(),
            error: Some("file not found".to_string()),
            timestamp: 1_700_000_004_000,
        };
        let json = serde_json::to_string(&msg).expect("序列化失败");
        let back: OmniMessage = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(msg, back);
    }

    #[test]
    fn test_state_update_serde_roundtrip() {
        let msg = OmniMessage::StateUpdate {
            key: "current_file".to_string(),
            value: r#""src/lib.rs""#.into(),
            timestamp: 1_700_000_005_000,
        };
        let json = serde_json::to_string(&msg).expect("序列化失败");
        let back: OmniMessage = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(msg, back);
        assert_eq!(msg.message_type(), "StateUpdate");
        assert_eq!(msg.request_id(), None);
    }

    #[test]
    fn test_trace_record_serde_roundtrip() {
        let msg = OmniMessage::TraceRecord {
            step: 0,
            action: "read_file".to_string(),
            observation: "file contents".to_string(),
            reward: 0.5,
            timestamp: 1_700_000_006_000,
        };
        let json = serde_json::to_string(&msg).expect("序列化失败");
        let back: OmniMessage = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(msg, back);
        assert_eq!(msg.message_type(), "TraceRecord");
        assert_eq!(msg.request_id(), None);
    }

    // ----------------------------------------------------------
    // 线格式冻结测试（serde tag 冻结）
    // ----------------------------------------------------------

    #[test]
    fn test_omni_message_json_wire_format_frozen() {
        // 验证 serde 外部标签枚举格式：{"VariantName": {...}}
        let msg = OmniMessage::StateUpdate {
            key: "k".to_string(),
            value: "v".into(),
            timestamp: 0,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(
            json.contains("\"StateUpdate\""),
            "StateUpdate 变体 tag 应保留: {json}"
        );

        let msg = OmniMessage::TraceRecord {
            step: 0,
            action: "a".to_string(),
            observation: "o".to_string(),
            reward: 0.0,
            timestamp: 0,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(
            json.contains("\"TraceRecord\""),
            "TraceRecord 变体 tag 应保留: {json}"
        );

        let msg = OmniMessage::ModelRequest {
            request_id: "r".to_string(),
            prompt: "p".to_string(),
            model_config: ModelConfig::default_config(),
            timestamp: 0,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(
            json.contains("\"ModelRequest\""),
            "ModelRequest 变体 tag 应保留: {json}"
        );

        let msg = OmniMessage::ModelResponse {
            request_id: "r".to_string(),
            content: "c".to_string(),
            usage: TokenUsage::new(0, 0),
            timestamp: 0,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(
            json.contains("\"ModelResponse\""),
            "ModelResponse 变体 tag 应保留: {json}"
        );

        let msg = OmniMessage::ToolRequest {
            request_id: "r".to_string(),
            tool_name: "t".to_string(),
            parameters: "{}".into(),
            timestamp: 0,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(
            json.contains("\"ToolRequest\""),
            "ToolRequest 变体 tag 应保留: {json}"
        );

        let msg = OmniMessage::ToolResult {
            request_id: "r".to_string(),
            success: true,
            output: "o".to_string(),
            error: None,
            timestamp: 0,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(
            json.contains("\"ToolResult\""),
            "ToolResult 变体 tag 应保留: {json}"
        );
    }

    // ----------------------------------------------------------
    // timestamp() 方法测试
    // ----------------------------------------------------------

    #[test]
    fn test_timestamp_extraction() {
        let ts = 1_700_000_000_000u64;
        let msgs = vec![
            OmniMessage::ModelRequest {
                request_id: "r".to_string(),
                prompt: "p".to_string(),
                model_config: ModelConfig::default_config(),
                timestamp: ts,
            },
            OmniMessage::ModelResponse {
                request_id: "r".to_string(),
                content: "c".to_string(),
                usage: TokenUsage::new(0, 0),
                timestamp: ts,
            },
            OmniMessage::ToolRequest {
                request_id: "r".to_string(),
                tool_name: "t".to_string(),
                parameters: "{}".into(),
                timestamp: ts,
            },
            OmniMessage::ToolResult {
                request_id: "r".to_string(),
                success: true,
                output: "o".to_string(),
                error: None,
                timestamp: ts,
            },
            OmniMessage::StateUpdate {
                key: "k".to_string(),
                value: "v".into(),
                timestamp: ts,
            },
            OmniMessage::TraceRecord {
                step: 0,
                action: "a".to_string(),
                observation: "o".to_string(),
                reward: 0.0,
                timestamp: ts,
            },
        ];
        for msg in &msgs {
            assert_eq!(msg.timestamp(), ts);
        }
    }

    // ----------------------------------------------------------
    // 枚举闭集测试：未知变体拒绝
    // ----------------------------------------------------------

    #[test]
    fn test_omni_message_rejects_unknown_variant() {
        let err = serde_json::from_str::<OmniMessage>(r#"{"Unknown": {}}"#).unwrap_err();
        assert!(!err.to_string().is_empty());
    }
}
