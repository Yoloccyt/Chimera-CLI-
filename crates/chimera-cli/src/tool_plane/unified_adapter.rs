//! UnifiedExecutorAdapter — 四环链 delegate → tool_plane 共享执行面的正式桥。
//!
//! W2（L2 工具层重构方案 §6.5 / §4.4）：取代 `CompositeToolExecutor(Builtin,
//! Mcp-2PC 模拟)` 装配。计划面（exec --plan / quest 批量）与 LLM 发布面自此共享
//! **同一 provider 生态**——MCP 调用直达 StdioTransport JSON-RPC 真执行，
//! 根除 2PC `InProcessClient` 模拟路径的语义伪造（方案病灶 P2）。
//!
//! # 六项职责（方案 §6.5）
//!
//! 1. **命名归一**：`mcp.<s>.<t>` → canonical `mcp:<s>/<tool>`（L0 [`parse_tool_name`]）；
//!    `text.echo` → `echo.echo`（builtin 历史名 → tool_plane provider 命名规范）；
//!    其余 Plain 原样。
//! 2. **审批穿透**（MCP 形态）：节点声明 > 配置声明 > 能力自报的 ReadOnly 证据链
//!    任一命中 → 放行；Write 且 `approve_writes` → 放行；**Write 无放行位 / 未知
//!    → AskRequired 显式失败**（fail-closed；行为变化登记：批路径写类/未知 MCP
//!    从「2PC 模拟成功」变为显式拒绝——方案 fail-loud 裁决）。
//! 3. **取消穿透**：`ctx.plan.cancel` → [`ToolRuntime::invoke`] 的令牌。
//! 4. **call_id 可追溯**：`{plan_id}-{node_id}`（legacy 路径为 `{tool}-{unix_nanos}`）。
//! 5. **超时**：`invocation.timeout_ms = None` ⇒ descriptor 级默认
//!    （单操作超时已由外层 [`OpTimeoutExecutor`] 承担，不重复叠加）。
//! 6. **状态映射**：[`ToolStatus`] → `Err` 文案保留四环链语义前缀
//!    （`policy:` / `route:` / `transport:` / `timeout:` / `cancelled:`），
//!    PlanRunner 写回与 W4 `from_legacy_err` 归因兼容。
//!
//! # 能力声明真值源（W2 阶段，W3 收敛到 L0 SideEffectResolver）
//!
//! - `capability_of`：tool_plane 注册表 descriptor 声明（LLM 面与计划面同源）；
//! - `declared_side_effects`：`McpConfig.declared_side_effects` 配置表
//!   （原 MCP 模拟执行器的 from_config 装配语义逐字迁移——R11 W1 打通的
//!   装饰器转发链在此继续生效，断链即并行封顶回归 0）。

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use gqep_executor::tool_plane::{
    ToolDescriptor, ToolInvocation, ToolOrigin, ToolOutcome, ToolPriority, ToolRuntime, ToolStatus,
};
use gqep_executor::toolplan_runner::NodeExecCtx;
use gqep_executor::ToolExecutor;
use nexus_contracts::tool_name::{canonical_name, parse_tool_name, ParsedToolName};
use nexus_contracts::tool_plan::SideEffectDecl;
use tokio_util::sync::CancellationToken;

use crate::config::ChimeraConfig;

/// builtin 历史名 → tool_plane canonical 名（provider 命名规范 `<provider>.<tool>`）。
///
/// WHY 显式别名表而非双向注册：两面的同名工具在 W2 装配切换后只剩 tool_plane
/// 一份实现，别名是「旧计划文件兼容」而非「双实现并存」——计划面既有 `text.echo`
/// 输入继续可用，canonical 输出恒为 `echo.echo`。
const PLAIN_TOOL_ALIASES: &[(&str, &str)] = &[("text.echo", "echo.echo")];

/// 四环链 delegate → tool_plane 共享执行面的适配器（组合根唯一装配点）。
pub struct UnifiedExecutorAdapter {
    /// 共享 tool_plane 运行时（与 LLM 面同一实例：同一注册表/调度器/会话池）
    plane: Arc<ToolRuntime>,
    /// 配置驱动的副作用声明（`mcp.<server>.<tool>` 点号键，fail-closed 缺省空集）
    declared_side_effects: Vec<(String, SideEffectDecl)>,
}

impl UnifiedExecutorAdapter {
    /// 组合根唯一构造入口：取共享 tool_plane 单例 + 配置声明表。
    ///
    /// WHY 经 `shared_tool_plane_with` 而非自建：MCP 会话池（`McpPlane`）由
    /// SharedToolPlane 保活，自建会造出第二条会话生命周期（限额与指标口径分裂）。
    #[must_use]
    pub fn from_shared_plane(plane: Arc<ToolRuntime>, config: &ChimeraConfig) -> Self {
        Self {
            plane,
            declared_side_effects: config
                .mcp
                .declared_side_effects
                .iter()
                .map(|e| (e.tool.clone(), e.effect))
                .collect(),
        }
    }

    /// 命名归一（职责 1）：MCP 双格式 → canonical 斜杠；builtin 别名映射；其余原样。
    fn resolve_canonical(tool_name: &str) -> String {
        match parse_tool_name(tool_name) {
            Ok(parsed @ ParsedToolName::Mcp { .. }) => canonical_name(&parsed),
            Ok(ParsedToolName::Plain { name }) => PLAIN_TOOL_ALIASES
                .iter()
                .find(|(from, _)| from == &name)
                .map_or(name, |(_, to)| (*to).to_string()),
            Err(_) => tool_name.to_string(),
        }
    }

    /// MCP 形态的审批判定（职责 2）——readonly 证据链 + 显式写放行位。
    ///
    /// 证据优先序（W3 将收敛为 L0 `SideEffectResolver`，本处为收敛前形态）：
    /// 1. 节点显式声明（计划作者意图，[`NodeExecCtx::side_effect`]）——
    ///    ReadOnly 直接放行；Write 看 `approve_writes`；
    /// 2. 配置声明（`omega.yaml declared_side_effects`，用户显式意图）；
    /// 3. 能力自报（tool_plane descriptor 声明，装配期事实）。
    /// 皆无 → fail-closed（未知按可能写）。
    fn mcp_approved(
        &self,
        tool_name: &str,
        node_decl: Option<SideEffectDecl>,
        approve_writes: bool,
    ) -> bool {
        match node_decl {
            Some(SideEffectDecl::ReadOnly) => return true,
            Some(SideEffectDecl::Write) => return approve_writes,
            None => {}
        }
        let declared_ro = self
            .declared_side_effects
            .iter()
            .any(|(n, e)| n == tool_name && *e == SideEffectDecl::ReadOnly);
        let capability_ro = self.capability_of(tool_name) == Some(SideEffectDecl::ReadOnly);
        declared_ro || capability_ro
    }

    /// 经共享 tool_plane 执行（归一后调用），状态映射为四环链 Err 文案。
    async fn invoke_via_plane(
        &self,
        tool_name: &str,
        args_json: &str,
        approved: bool,
        cancel: &CancellationToken,
        call_id: String,
    ) -> Result<String, String> {
        let canonical = Self::resolve_canonical(tool_name);
        let invocation = ToolInvocation {
            call_id,
            tool: canonical.clone(),
            args_json: args_json.to_string(),
            // 超时由外层 OpTimeoutExecutor 承担；descriptor 级默认兜底
            timeout_ms: None,
            priority: ToolPriority::Normal,
            // 批/计划面无更细归因来源，诚实取默认（方案 §6.5 职责 5）
            origin: ToolOrigin::Cli,
            quest_id: None,
            task_id: None,
            approved,
        };
        let outcome = self.plane.invoke(invocation, cancel).await;
        map_outcome(outcome)
    }
}

/// [`ToolOutcome`] 终态 → 四环链 Err 文案（保留语义前缀，W4 归因兼容）。
fn map_outcome(outcome: ToolOutcome) -> Result<String, String> {
    let detail = || {
        outcome
            .error
            .clone()
            .unwrap_or_else(|| format!("{:?}", outcome.status))
    };
    match outcome.status {
        ToolStatus::Ok => Ok(outcome.payload.unwrap_or_default()),
        ToolStatus::Skipped => Err(format!("route: tool call skipped: {}", outcome.tool)),
        ToolStatus::NotFound => Err(format!("route: tool not found: {}", outcome.tool)),
        ToolStatus::InvalidArgs => Err(format!(
            "route: invalid args for {}: {}",
            outcome.tool,
            detail()
        )),
        ToolStatus::Denied => Err(format!("policy: denied {}: {}", outcome.tool, detail())),
        ToolStatus::AskRequired => Err(format!(
            "policy: approval required for {} (write-class or undeclared MCP tool): {}",
            outcome.tool,
            detail()
        )),
        ToolStatus::Timeout => Err(format!("timeout: {}: {}", outcome.tool, detail())),
        ToolStatus::Cancelled => Err(format!("cancelled: {}: {}", outcome.tool, detail())),
        ToolStatus::Overloaded => Err(format!(
            "route: overloaded for {}: {}",
            outcome.tool,
            detail()
        )),
        ToolStatus::ProviderError => Err(format!("transport: {}: {}", outcome.tool, detail())),
    }
}

/// 由 [`ToolError`]（tool_plane provider 层）构造 ToolOutcome 错误终态时保留文案。
impl UnifiedExecutorAdapter {
    /// descriptor 视图（供测试与诊断；执行路径走 `capability_of`）
    #[must_use]
    pub fn plane(&self) -> &Arc<ToolRuntime> {
        &self.plane
    }
}

#[async_trait]
impl ToolExecutor for UnifiedExecutorAdapter {
    /// legacy 单发（批路径 `execute_batch` 实际调用）：无节点上下文 ⇒ 无写放行位。
    ///
    /// MCP 审批按「未声明」路径走证据链（readonly 证据放行 / 写与未知 fail-closed）；
    /// 非 MCP 工具（builtin/echo/fs）不经审批闸（只读实现或由 tool_plane 内部
    /// policy 评估守门）。
    async fn execute(&self, tool_name: &str, args_json: &str) -> Result<String, String> {
        let is_mcp = parse_tool_name(tool_name)
            .map(|p| matches!(p, ParsedToolName::Mcp { .. }))
            .unwrap_or_else(|_| {
                // 解析失败的 MCP 前缀形态仍按 MCP 审批口径 fail-closed
                tool_name.starts_with("mcp:") || tool_name.starts_with("mcp.")
            });
        let approved = if is_mcp {
            self.mcp_approved(tool_name, None, false)
        } else {
            true
        };
        let call_id = legacy_call_id(tool_name);
        let cancel = CancellationToken::new();
        self.invoke_via_plane(tool_name, args_json, approved, &cancel, call_id)
            .await
    }

    /// 带节点上下文（[`PlanRunner::run_with`] 主路径）：审批/取消/call_id 全穿透。
    async fn execute_node(
        &self,
        ctx: &NodeExecCtx<'_>,
        tool_name: &str,
        args_json: &str,
    ) -> Result<String, String> {
        let is_mcp = parse_tool_name(tool_name)
            .map(|p| matches!(p, ParsedToolName::Mcp { .. }))
            .unwrap_or_else(|_| tool_name.starts_with("mcp:") || tool_name.starts_with("mcp."));
        let approved = if is_mcp {
            self.mcp_approved(tool_name, ctx.side_effect, ctx.plan.approve_writes)
        } else {
            true
        };
        let call_id = format!("{}-{}", ctx.plan.plan_id, ctx.node_id);
        let cancel = ctx
            .plan
            .cancel
            .cloned()
            .unwrap_or_else(CancellationToken::new);
        self.invoke_via_plane(tool_name, args_json, approved, &cancel, call_id)
            .await
    }

    /// 能力视图：tool_plane 注册表 descriptor 声明（LLM 面同源，单真值）。
    fn capability_of(&self, tool_name: &str) -> Option<SideEffectDecl> {
        let canonical = Self::resolve_canonical(tool_name);
        self.plane
            .registry()
            .resolve(&canonical)
            .map(|(_, descriptor)| descriptor.side_effect)
    }

    /// 路由表：tool_plane 注册表全集（canonical 名；含 MCP/echo/fs/process）。
    fn declared_tools(&self) -> Vec<String> {
        self.plane
            .registry()
            .descriptors()
            .iter()
            .map(|d: &ToolDescriptor| d.name.clone())
            .collect()
    }

    /// 配置声明表逐字透出（原 `McpToolExecutor::declared_side_effects` 装配语义）。
    ///
    /// WHY 不与 `capability_of` 合并：PlanRunner 的 `effective_side_effect`
    /// 三级优先序（节点 > capability > declared）需要两个源独立呈现；
    /// 本方法键为计划面点号格式（与 omega.yaml 声明格式一致），W3 收敛为
    /// L0 resolver 后此处退化为 resolver 的数据供给方。
    fn declared_side_effects(&self) -> Vec<(String, SideEffectDecl)> {
        self.declared_side_effects.clone()
    }
}

/// legacy 路径 call_id：工具名 + 纳秒时间戳（唯一性优先——legacy 路径无节点
/// 身份可借，纳秒时间戳在同进程内的碰撞概率可忽略）。
fn legacy_call_id(tool_name: &str) -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("{tool_name}-{nanos}")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::sync::Arc as StdArc;

    use crate::tool_plane::EchoToolProvider;

    /// 测试专用 plane：只注册 echo（不触共享 OnceLock 单例与真实 MCP 发现）。
    fn test_plane() -> Arc<ToolRuntime> {
        let mut registry = gqep_executor::tool_plane::ToolRegistry::new();
        registry
            .register(StdArc::new(EchoToolProvider::new()))
            .expect("测试注册表无重名");
        let scheduler = Arc::new(gqep_executor::tool_plane::BoundedToolScheduler::new(
            4, 4, 16, 4,
        ));
        Arc::new(ToolRuntime::new(
            Arc::new(registry),
            None,
            scheduler,
            gqep_executor::tool_plane::ToolRuntimeConfig {
                interactive_inflight: 4,
                background_inflight: 4,
                max_waiters: 16,
                max_provider_inflight: 4,
                default_timeout_ms: 5_000,
                max_payload_bytes: 64 * 1024,
                max_batch: 16,
            },
        ))
    }

    fn adapter_with_declarations(decls: &[(&str, SideEffectDecl)]) -> UnifiedExecutorAdapter {
        UnifiedExecutorAdapter {
            plane: test_plane(),
            declared_side_effects: decls.iter().map(|(n, e)| ((*n).to_string(), *e)).collect(),
        }
    }

    // === 职责 1：命名归一 ===

    #[test]
    fn canonical_resolution_unifies_both_mcp_formats() {
        assert_eq!(
            UnifiedExecutorAdapter::resolve_canonical("mcp.a.b"),
            "mcp:a/b"
        );
        assert_eq!(
            UnifiedExecutorAdapter::resolve_canonical("mcp:a/b"),
            "mcp:a/b"
        );
        // builtin 历史别名
        assert_eq!(
            UnifiedExecutorAdapter::resolve_canonical("text.echo"),
            "echo.echo"
        );
        // fs 工具两面同名，原样
        assert_eq!(
            UnifiedExecutorAdapter::resolve_canonical("fs.read"),
            "fs.read"
        );
    }

    // === echo 透传（别名映射端到端） ===

    #[tokio::test]
    async fn legacy_execute_aliases_text_echo_and_passes_args_through() {
        let adapter = adapter_with_declarations(&[]);
        let payload = adapter
            .execute("text.echo", r#"{"hello":"w2"}"#)
            .await
            .expect("echo 透传应成功");
        assert_eq!(payload, r#"{"hello":"w2"}"#);
    }

    // === 职责 2：MCP 审批 fail-closed ===

    #[tokio::test]
    async fn undeclared_mcp_call_fails_closed_not_found() {
        // 无注册 server、无声明 ⇒ 未声明 MCP 形态经证据链判 fail-closed，
        // registry miss 后归 route: not found（而非 2PC 模拟的假成功）
        let adapter = adapter_with_declarations(&[]);
        let err = adapter.execute("mcp.x.y", "{}").await.unwrap_err();
        assert!(
            err.starts_with("route: tool not found"),
            "fail-closed 归因: {err}"
        );
    }

    #[test]
    fn declared_readonly_evidence_grants_approval() {
        let adapter = adapter_with_declarations(&[("mcp.acme.search", SideEffectDecl::ReadOnly)]);
        assert!(
            adapter.mcp_approved("mcp.acme.search", None, false),
            "配置声明 readonly 证据 ⇒ 放行"
        );
        assert!(
            !adapter.mcp_approved("mcp.other.t", None, false),
            "无证据 ⇒ fail-closed"
        );
        assert!(
            !adapter.mcp_approved("mcp.acme.search", Some(SideEffectDecl::Write), false),
            "节点声明 Write 且无放行位 ⇒ 拒"
        );
        assert!(
            adapter.mcp_approved("mcp.acme.search", Some(SideEffectDecl::Write), true),
            "节点声明 Write 且显式放行 ⇒ 放行"
        );
        assert!(
            adapter.mcp_approved("mcp.any.t", Some(SideEffectDecl::ReadOnly), false),
            "节点声明 readonly ⇒ 放行（计划作者意图最高优先）"
        );
    }

    // === 职责 6 + 能力视图 ===

    #[test]
    fn capability_of_aliases_and_unknown() {
        let adapter = adapter_with_declarations(&[]);
        // 经别名映射命中 echo descriptor（ReadOnly）
        assert_eq!(
            adapter.capability_of("text.echo"),
            Some(SideEffectDecl::ReadOnly)
        );
        // canonical 名直查同源
        assert_eq!(
            adapter.capability_of("echo.echo"),
            Some(SideEffectDecl::ReadOnly)
        );
        // 未注册工具 None（诚实语义）
        assert_eq!(adapter.capability_of("no.such"), None);
    }

    #[test]
    fn declared_tools_expose_plane_registry() {
        let adapter = adapter_with_declarations(&[]);
        let tools = adapter.declared_tools();
        assert!(tools.contains(&"echo.echo".to_string()), "tools={tools:?}");
    }

    #[test]
    fn declared_side_effects_passthrough_config() {
        let adapter = adapter_with_declarations(&[("mcp.acme.search", SideEffectDecl::ReadOnly)]);
        let decls = adapter.declared_side_effects();
        assert_eq!(decls.len(), 1);
        assert_eq!(
            decls[0],
            ("mcp.acme.search".to_string(), SideEffectDecl::ReadOnly)
        );
    }

    // === 职责 4：call_id 可追溯（execute_node 主路径） ===

    #[tokio::test]
    async fn execute_node_builds_traceable_call_id() {
        // 用 echo 直接驱动 execute_node：PlanExecCtx 借用栈构造
        let adapter = adapter_with_declarations(&[]);
        let cancel = CancellationToken::new();
        let plan = gqep_executor::toolplan_runner::PlanExecCtx {
            plan_id: "plan-1",
            approve_writes: false,
            cancel: Some(&cancel),
        };
        let ctx = NodeExecCtx {
            plan,
            node_id: "node-7",
            side_effect: None,
        };
        // execute_node 成功即证明 call_id 构造与取消穿透路径无 panic；
        // echo 返回 args 原样
        let payload = adapter
            .execute_node(&ctx, "text.echo", r#"{"ok":1}"#)
            .await
            .expect("execute_node 透传");
        assert_eq!(payload, r#"{"ok":1}"#);
    }
}
