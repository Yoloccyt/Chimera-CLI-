//! MCP 发现结果的**数据形状**：`tools/list` 返回的工具条目、注解，以及装配期
//! 一次取好的发现快照与副作用判定。

use std::sync::Arc;

use nexus_contracts::tool_plan::SideEffectDecl;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::protocol::client::McpClient;

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

/// 一个已完成发现的服务器：工具清单 + 客户端句柄 + 该 server 的信任档。
///
/// 客户端句柄与工具清单放在一起，是为了让"有描述符 ⇒ 一定可调用"成为类型事实：
/// 拆成两个容器就可能出现"出现在工具表里但没有路由"的半接线态。
pub struct DiscoveredServer {
    /// 服务器 id（来自配置；`mcp:<id>/<tool>` 的前半段）
    pub id: String,
    /// 该服务器声明的工具
    pub tools: Vec<McpTool>,
    /// 是否采信该服务器的 `readOnlyHint`（默认 false，见 ADR-235 D1）
    pub trust_readonly_hints: bool,
    /// 会话句柄（发现与调用都走它）
    pub client: Arc<McpClient>,
}

/// 装配期一次取好的发现结果；进程内不会再变（工具集合在装配点冻结）。
#[derive(Default)]
pub struct DiscoverySnapshot {
    /// 已完成发现的服务器（不可达的服务器**不在**其中，见 ADR-235 D6）
    pub servers: Vec<DiscoveredServer>,
}

impl DiscoverySnapshot {
    /// 是否没有任何可暴露的工具。
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.tool_count() == 0
    }

    /// 工具总数（跨服务器合计）。
    #[must_use]
    pub fn tool_count(&self) -> usize {
        self.servers.iter().map(|server| server.tools.len()).sum()
    }

    /// 按 `(server_id, tool_name)` 找一条工具及其所属服务器。
    #[must_use]
    pub fn find(&self, server_id: &str, tool_name: &str) -> Option<(&DiscoveredServer, &McpTool)> {
        let server = self.servers.iter().find(|server| server.id == server_id)?;
        let tool = server.tools.iter().find(|tool| tool.name == tool_name)?;
        Some((server, tool))
    }
}

/// **唯一的副作用判定点**（ADR-235 D1）—— 纯函数，可穷举测试。
///
/// 规则：
/// - `trust_readonly_hints == false`（默认）⇒ 恒 `Write`。`readOnlyHint` 是服务器
///   自报的 hint，采信它等于放宽审批面，默认不给。
/// - 开了信任也只在 `read_only == true` **且** `destructive != true` 时降为
///   `ReadOnly`；缺省一律按 `Write`（fail-closed）。
#[must_use]
pub fn side_effect_for(tool: &McpTool, trust_readonly_hints: bool) -> SideEffectDecl {
    if !trust_readonly_hints {
        return SideEffectDecl::Write;
    }
    let annotations = tool.annotations.as_ref();
    let read_only = annotations
        .and_then(|hints| hints.read_only_hint)
        .unwrap_or(false);
    let destructive = annotations
        .and_then(|hints| hints.destructive_hint)
        .unwrap_or(false);
    if read_only && !destructive {
        SideEffectDecl::ReadOnly
    } else {
        SideEffectDecl::Write
    }
}
