//! McpToolExecutor — 将 `mcp.<server>.<tool>` 形态的工具调用接入四环传输环（ADR-256）
//!
//! WHY L10 落点：mcp-mesh(L10) 与 chimera-cli(L10) 同层互引合法；
//! ToolExecutor trait 属 L7（gqep-executor），L10→L7 向下依赖合法。
//! WHY 不在 commands/mcp.rs 内实现：命令面是 ephemeral 一次性构造，
//! 执行器要被组合根长持（composition.rs tool_delegate），生命周期不同，分文件。
//!
//! # 传输与安全边界（ADR-256 D4/D5/D7）
//! - 传输恒 [`InProcessClient`](mcp_mesh::InProcessClient)：默认零网络副作用
//!   （与 commands/mcp.rs `build_mesh(false)` 同范式）；
//! - 注册复用 `commands/mcp.rs::register_configured_servers` 语义：url 型经
//!   registry SSRF 校验（拒绝 warn 跳过）、stdio 型 warn 跳过、空 id warn 跳过；
//! - 破坏性操作确认：非 CLI 上下文不直调 permission，由 policy 环 Ask 语义承担
//!   （组合根 `tool_runtime_policy` 的规则可定向 `mcp.<server>.<tool>` 全名——
//!   ADR-256 D6 + Z-1 订正：program 匹配是精确小写，通配 `mcp.*` 不可行）。
//!
//! # ADR-253 边界调和（D7）
//! 复用 mcp-mesh 的 2PC 参与者面（`execute_transaction`），**不新造 JSON-RPC 栈**；
//! 两面共享 registry 与 SSRF 守卫。`mcp serve` 保持 NotImplemented（显式排除）。

use std::sync::Arc;

use crate::config::ChimeraConfig;
use async_trait::async_trait;
use gqep_executor::ToolExecutor;
use mcp_mesh::{InProcessClient, McpMesh, MeshConfig, MeshServer, ParticipantClient};
use nexus_contracts::tool_plan::SideEffectDecl;

/// 工具名命名空间前缀（ADR-256 D1）
pub const MCP_TOOL_PREFIX: &str = "mcp.";

/// server_id 白名单上界（ADR-256 D1：配置层 id 无校验，解析消歧在执行器入口收紧；
/// 不动 L1 配置 schema，避免 parity 门重派生）
const MAX_SERVER_ID_LEN: usize = 64;

/// MCP 工具名解析错误 — 语义归因 `route:`（ADR-256 D3：外层机制归因 Transport，
/// 文案前缀保留 route 语义供诊断）
#[derive(Debug, thiserror::Error)]
#[error("route: mcp tool name malformed ({kind}): `{raw}`")]
pub struct McpRouteError {
    /// 错误类别（决定诊断文案的语义段）
    pub kind: McpRouteErrorKind,
    /// 原始工具名（未裁剪，供上层日志携带完整上下文）
    pub raw: String,
}

/// 工具名解析错误类别（ADR-256 D1/D3：三类边界各归其一）
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum McpRouteErrorKind {
    /// `mcp.` 前缀后为空或全为分隔符
    #[error("empty or all-separator body")]
    EmptySegment,
    /// server_id 含非法字符（白名单外）；连续点号（server 段边界歧义）亦归此类
    #[error("server id outside [a-zA-Z0-9_-] whitelist (1..=64)")]
    IllegalServerId,
    /// tool 段为空
    #[error("tool segment missing")]
    EmptyTool,
}

/// server_id 白名单校验（ADR-256 D1）。
///
/// 规则：长度 1..=64；仅 `[a-zA-Z0-9_-]`；点号不在白名单内（保证「首点切分」
/// 的解析无歧义——这是 [`parse_tool_name`] 把连续点号归入 [`McpRouteErrorKind::
/// IllegalServerId`] 的语义依据）。
fn is_legal_server_id(id: &str) -> bool {
    (1..=MAX_SERVER_ID_LEN).contains(&id.len())
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// 生产 ephemeral mesh 的唯一构造点：进程内**零延迟**客户端 + 默认（durable）配置。
///
/// WHY 收口：`mcp_tool_executor` 与 `commands/mcp` 两处生产根曾各建一次，
/// 任一处漏改都会让「生产仍付 2–4ms 模拟 RTT」无声复活（ADR-280 D2）。
/// `MeshConfig::default()` 刻意不变：WAL durability 与写入前的敏感载荷扫描保留。
pub(crate) fn in_process_mesh() -> McpMesh {
    let client: Arc<dyn ParticipantClient> = Arc::new(InProcessClient::instant());
    // ADR-276 D2：event_bus 恒 None —— ephemeral mesh 无长驻订阅者；接总线即虚构消费。
    McpMesh::with_participant_client(MeshConfig::default(), None, client)
}

/// MCP 工具执行器 — 组合根长持的 ephemeral mesh 委托（ADR-256 D2/D4）
pub struct McpToolExecutor {
    /// 进程内 ephemeral mesh（同 mcp.rs 范式；executor 与宿主同生命周期）
    mesh: McpMesh,
}

impl McpToolExecutor {
    /// 组合根唯一构造入口。注册语义逐字复用 commands/mcp.rs:102-137：
    /// url 型注册 + SSRF 拒绝 warn 跳过；stdio 型（无 url）warn 跳过；空 id warn 跳过。
    /// 默认传输恒 InProcessClient（零网络副作用）；`None` 事件绑定 ⇒ 零事件流量
    /// （ADR-256 §6.5：P1 无性能主张、无新事件发布点）。
    #[must_use]
    pub fn from_config(config: &ChimeraConfig) -> Self {
        // ADR-276 D2：event_bus 恒 None（同 `commands/mcp.rs::build_mesh`）——
        // 执行器持的是诊断/执行面 mesh，无长驻订阅者；接总线即虚构消费。
        let mesh = in_process_mesh();
        for server in &config.mcp.servers {
            if server.id.is_empty() {
                tracing::warn!("MCP 配置项缺少 server id，已跳过（mcp 工具执行器装配）");
                continue;
            }
            let Some(url) = server.url.as_deref().filter(|u| !u.is_empty()) else {
                tracing::warn!(
                    server_id = %server.id,
                    "stdio 服务器暂不支持注册（仅 url 型可注册），已跳过（mcp 工具执行器装配）"
                );
                continue;
            };
            let candidate =
                MeshServer::new(server.id.clone(), url.to_string(), vec!["url".to_string()]);
            match mesh.registry().register(candidate) {
                Ok(()) => {
                    tracing::info!(
                        server_id = %server.id,
                        endpoint = %url,
                        "MCP 服务器已注册（mcp 工具执行器装配）"
                    );
                }
                Err(err) => {
                    tracing::warn!(
                        server_id = %server.id,
                        endpoint = %url,
                        error = %err,
                        "MCP 服务器注册被拒（SSRF 校验，mcp 工具执行器装配）"
                    );
                }
            }
        }
        Self { mesh }
    }

    /// 命名解析（纯函数，供 proptest 穷举）：`mcp.<server>.<tool>` → (server, tool)。
    ///
    /// 切分规则（ADR-256 D1）：`server_id` 白名单排除点号 ⇒ **首个 `.` 即边界**；
    /// tool 段 = 首点之后全部内容（允许内含点号，如 `mcp.a.b.c` → ("a", "b.c")）。
    ///
    /// 错误归类：
    /// - 无前缀体 / 前缀体全为分隔符 → `EmptySegment`；
    /// - 无点号（缺 tool 段）或 tool 段为空 → `EmptyTool`；
    /// - server 段非法（含空段——rest 以点号开头）或**连续点号**（首点后紧跟点号，
    ///   server 白名单排除点号的消歧前提被破坏）→ `IllegalServerId`。
    pub fn parse_tool_name(tool_name: &str) -> Result<(String, String), McpRouteError> {
        let kind_err = |kind| {
            Err(McpRouteError {
                kind,
                raw: tool_name.to_string(),
            })
        };
        let Some(rest) = tool_name.strip_prefix(MCP_TOOL_PREFIX) else {
            return kind_err(McpRouteErrorKind::EmptySegment);
        };
        if rest.is_empty() {
            return kind_err(McpRouteErrorKind::EmptySegment);
        }
        let Some((server, tool)) = rest.split_once('.') else {
            // 无点号：只有 server 段，tool 缺失
            return kind_err(McpRouteErrorKind::EmptyTool);
        };
        if !is_legal_server_id(server) {
            return kind_err(McpRouteErrorKind::IllegalServerId);
        }
        if tool.is_empty() {
            return kind_err(McpRouteErrorKind::EmptyTool);
        }
        if tool.starts_with('.') {
            // 连续点号（如 `mcp.cai..tool`）：server 段边界歧义，归入白名单违规
            return kind_err(McpRouteErrorKind::IllegalServerId);
        }
        Ok((server.to_string(), tool.to_string()))
    }

    /// 注册表查询（测试与诊断面；执行路径经 [`Self::execute`] 内部查询）
    #[must_use]
    pub fn registered_servers(&self) -> Vec<String> {
        self.mesh.registry().list_all()
    }
}

#[async_trait]
impl ToolExecutor for McpToolExecutor {
    async fn execute(&self, tool_name: &str, args_json: &str) -> Result<String, String> {
        // 1. 前缀守卫（防直接注入本执行器——组合根经 CompositeToolExecutor 前缀
        //    分派，此守卫是独立防线：绕过复合器直调本执行器时 fail-closed）
        let Some(rest) = tool_name.strip_prefix(MCP_TOOL_PREFIX) else {
            return Err(format!(
                "route: mcp executor received non-mcp tool: {tool_name}"
            ));
        };
        let _ = rest;
        // 2. 解析边界（Err 文案前缀 `route:`）
        let (server_id, tool) = Self::parse_tool_name(tool_name).map_err(|e| e.to_string())?;
        // 3. 目标服务器注册校验（真值源 = from_config 注册结果；运行时注册态错误
        //    走内联 Err 文案——`route:` 前缀供诊断，与解析错误同一语义前缀、不同机制，
        //    见 ADR-256 D3〔修订·审读〕裁决：parse 纯函数查不了注册表）
        let Some(server) = self.mesh.registry().get(&server_id) else {
            return Err(format!("route: mcp server not registered: {server_id}"));
        };
        let _ = server;
        // 4. 经 2PC 参与者面执行（ADR-253 边界：复用 execute_transaction，不新造 JSON-RPC）
        let result = self
            .mesh
            .execute_transaction(
                vec![server_id],
                tool.to_string(),
                vec![args_json.to_string()],
            )
            .await
            .map_err(|e| format!("transport: mcp transaction failed: {e}"))?;
        // 5. 结果序列化为单层 JSON（勿双层编码）
        Ok(serde_json::json!({
            "success": result.success,
            "transaction_id": result.transaction_id,
        })
        .to_string())
    }

    // ADR-256 D2：能力声明诚实语义 —— 未知副作用 → None（计划层保守串行 fail-closed）；
    // 工具名全集空集 → 路由 fallback（与既有行为逐字一致）。「诚实优于虚构」。
    fn capability_of(&self, _tool_name: &str) -> Option<SideEffectDecl> {
        None
    }
    fn declared_tools(&self) -> Vec<String> {
        Vec::new()
    }
}

/// 复合执行器 — Builtin 与 Mcp 两个 delegate 的前缀分派组合（ADR-256 D2）
///
/// WHY 独立类型而非 enum dispatch：两分支生命周期/构造依赖不同。
/// WHY 能力/路由表只转发 Builtin：mcp 分支 `None`/空集是诚实声明（副作用未知
/// fail-closed、路由走 fallback），转发 Builtin 保持 ADR-254 D4 路由表语义——
/// `mcp.*` 不进路由表 ⇒ 恒走 fallback 进 mcp 分支（组合根装配后自动成立）。
pub struct CompositeToolExecutor {
    builtin: crate::exec_plan::BuiltinToolExecutor,
    mcp: McpToolExecutor,
}

impl CompositeToolExecutor {
    /// 组合根唯一构造入口（builtin 与 mcp 双 delegate 注入；分派规则见
    /// [`ToolExecutor for CompositeToolExecutor`] 实现）。
    #[must_use]
    pub fn new(builtin: crate::exec_plan::BuiltinToolExecutor, mcp: McpToolExecutor) -> Self {
        Self { builtin, mcp }
    }
}

#[async_trait]
impl ToolExecutor for CompositeToolExecutor {
    async fn execute(&self, tool_name: &str, args_json: &str) -> Result<String, String> {
        if tool_name.starts_with(MCP_TOOL_PREFIX) {
            self.mcp.execute(tool_name, args_json).await
        } else {
            self.builtin.execute(tool_name, args_json).await
        }
    }
    fn capability_of(&self, tool_name: &str) -> Option<SideEffectDecl> {
        self.builtin.capability_of(tool_name)
    }
    fn declared_tools(&self) -> Vec<String> {
        self.builtin.declared_tools()
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    use crate::exec_plan::BuiltinToolExecutor;
    use proptest::prelude::*;

    /// 结构锁（ADR-280 D2）：生产 ephemeral mesh 必须用零延迟客户端。
    /// 只检查 `#[cfg(test)]` 之前的源码，避免本条断言文本自指。
    #[test]
    fn production_mesh_uses_instant_client_not_simulated() {
        let src = include_str!("mcp_tool_executor.rs");
        let prod = src.split("#[cfg(test)]").next().unwrap_or("");
        assert!(
            prod.contains("InProcessClient::instant()"),
            "生产 ephemeral mesh 必须使用零延迟客户端（ADR-280 D2）"
        );
        assert!(
            !prod.contains("InProcessClient::new()"),
            "生产路径不得回退到模拟延迟客户端（ADR-280 D2）"
        );
    }

    #[test]
    fn parse_rejects_empty_prefix_body() {
        for raw in ["mcp.", "mcp"] {
            let err = McpToolExecutor::parse_tool_name(raw).unwrap_err();
            assert_eq!(err.kind, McpRouteErrorKind::EmptySegment, "raw={raw:?}");
            assert!(
                err.to_string().starts_with("route:"),
                "文案前缀 route: {err}"
            );
        }
    }

    #[test]
    fn parse_rejects_illegal_server_id() {
        for raw in [
            "mcp.cai..tool", // 连续点号 → server 段边界歧义
            "mcp.a b.tool",  // 空格不在白名单
            "mcp.c@i.tool",  // 非法字符
            "mcp.a..b",      // 连续点号变体
        ] {
            let err = McpToolExecutor::parse_tool_name(raw).unwrap_err();
            assert_eq!(err.kind, McpRouteErrorKind::IllegalServerId, "raw={raw:?}");
        }
        // 超长 server 段（>64）
        let long = format!("mcp.{}.t", "a".repeat(65));
        let err = McpToolExecutor::parse_tool_name(&long).unwrap_err();
        assert_eq!(err.kind, McpRouteErrorKind::IllegalServerId);
    }

    #[test]
    fn parse_rejects_empty_tool() {
        for raw in ["mcp.server.", "mcp.server"] {
            let err = McpToolExecutor::parse_tool_name(raw).unwrap_err();
            assert_eq!(err.kind, McpRouteErrorKind::EmptyTool, "raw={raw:?}");
        }
    }

    #[test]
    fn parse_accepts_legal_name() {
        assert_eq!(
            McpToolExecutor::parse_tool_name("mcp.github.list_issues").unwrap(),
            ("github".to_string(), "list_issues".to_string())
        );
        // `-`/`_` 合法
        assert_eq!(
            McpToolExecutor::parse_tool_name("mcp.my-server.my_tool").unwrap(),
            ("my-server".to_string(), "my_tool".to_string())
        );
        // tool 段可含点号（首点即边界）
        assert_eq!(
            McpToolExecutor::parse_tool_name("mcp.a.b.c").unwrap(),
            ("a".to_string(), "b.c".to_string())
        );
    }

    #[test]
    fn capability_none_and_declared_empty() {
        let exec = McpToolExecutor::from_config(&ChimeraConfig::default());
        // 诚实语义锁定：恒 None / 恒空（ADR-256 D2）
        assert_eq!(exec.capability_of("mcp.x.y"), None);
        assert!(exec.declared_tools().is_empty());
        // 空配置 ⇒ 零注册
        assert!(exec.registered_servers().is_empty());
    }

    #[test]
    fn composite_forwards_capability_and_declared() {
        let composite = CompositeToolExecutor::new(
            BuiltinToolExecutor,
            McpToolExecutor::from_config(&ChimeraConfig::default()),
        );
        // declared_tools == builtin 3 项（ADR-254 D4 路由表语义保持）
        assert_eq!(
            composite.declared_tools(),
            vec![
                "text.echo".to_string(),
                "fs.read".to_string(),
                "fs.list".to_string()
            ]
        );
        assert_eq!(
            composite.capability_of("text.echo"),
            Some(SideEffectDecl::ReadOnly)
        );
        // mcp 分支 None（诚实声明——不进路由表，恒走 fallback）
        assert_eq!(composite.capability_of("mcp.x.y"), None);
    }

    // 任意串分派：合法形态 ⇔ 前缀 + 白名单 server 段 + 非空 tool 段
    // （block-named 语法，proptest 1.11+；WHY 普通注释而非 doc 注释：
    // rustdoc 不为宏调用生成文档，doc 注释触发 unused_doc_comment lint）
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        #[test]
        fn prop_parse_boundary(s in ".*") {
            // 谓词：与 parse_tool_name 的判定逻辑同构（单真源=parse 本体，
            // 谓词为独立推导的镜像——两者不一致即解析语义分叉）
            let predicate = |s: &str| -> bool {
                let Some(rest) = s.strip_prefix("mcp.") else { return false };
                if rest.is_empty() {
                    return false;
                }
                let Some((server, tool)) = rest.split_once('.') else { return false };
                let legal = (1..=MAX_SERVER_ID_LEN).contains(&server.len())
                    && server.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
                legal && !tool.is_empty() && !tool.starts_with('.')
            };
            let parsed = McpToolExecutor::parse_tool_name(&s).is_ok();
            prop_assert_eq!(parsed, predicate(&s), "s={:?}", s);
        }
    }
}
