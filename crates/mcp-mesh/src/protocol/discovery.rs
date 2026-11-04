//! MCP 发现结果的**数据形状**：`tools/list` 返回的工具条目与注解。
//!
//! 副作用判定 `side_effect_for` 在下一片（S5）落到这里；本片先只做形状，
//! 让 `McpClient::list_tools` 有类型可返回。

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// MCP 工具注解（**全部是服务器自报的 hint，不可信**）。
///
/// 规范把它们定义为"提示"而非保证；本项目据此把"采信 readOnlyHint"做成
/// 逐 server 的显式开启项（ADR-235 D1），默认一律按 Write 处理。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolAnnotations {
    /// 服务器声称"只读"（不可信）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub read_only_hint: Option<bool>,
    /// 服务器声称"有破坏性"（不可信）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub destructive_hint: Option<bool>,
    /// 服务器声称"幂等"（不可信）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idempotent_hint: Option<bool>,
    /// 服务器声称"与外部世界交互"（不可信）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open_world_hint: Option<bool>,
}

/// `tools/list` 里的一个工具。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpTool {
    /// 工具名（同一 server 内唯一；`mcp:<server>/<name>` 的后半段）
    pub name: String,
    /// 人类可读描述（规范里可选；缺省为空串而不是报错）
    #[serde(default)]
    pub description: String,
    /// 输入 JSON Schema（原样透传给模型；缺失即无 schema）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
    /// 注解（hint，不可信）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub annotations: Option<ToolAnnotations>,
    /// 未识别成员原样保留
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}
