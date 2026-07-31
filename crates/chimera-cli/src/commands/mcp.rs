//! `chimera mcp <action>` — MCP 量子网格管理,真实接入 L10 mcp-mesh crate
//!
//! v2.9.0-omega Task 1.8:提供 MCP 服务器的 CLI 管理入口。
//!
//! # 子命令
//! - `mcp list` — 列出所有已注册 MCP 服务器(表格 / JSON)
//! - `mcp serve` — 启动 MCP 服务器(当前 NotImplemented,指引 TUI 替代)
//! - `mcp call <server> <tool> [args]` — 调用 MCP 工具(触发 permission prompt)
//! - `mcp inspect <server>` — 输出服务器详情(注册时间 / 心跳 / 工具清单)
//!
//! # 设计决策(WHY)
//! - **进程内 ephemeral mesh**:与 `chimera run` / `quest` 一致,每次调用创建独立 McpMesh。
//!   这意味着 `mcp list` 在新进程中返回空列表(注册表不跨进程持久化)。
//!   真实 MCP 服务器管理请用 `chimera tui`(长生命周期 mesh + 后台探活)。
//! - **`mcp serve` 为 NotImplemented**:真实服务器启动需要绑定网络端口、加载 TLS 证书等,
//!   不适合 CLI 一次性启动。指引替代方案:`chimera tui` 或独立部署 mcp-mesh 服务。
//! - **`mcp call` 触发 permission prompt**:调用外部 MCP 工具属破坏性操作(可能执行任意代码),
//!   必须调用 `permission::confirm` 获取用户确认(除非 `--yes` / `--no-permission`)。

use anyhow::Result;
use mcp_mesh::{McpMesh, MeshConfig};

use crate::cli::McpAction;
use crate::config::ChimeraConfig;
use crate::error::ChimeraCliError;
use crate::output;
use crate::permission::{self, PermissionCtx};

/// 执行 mcp 子命令 — 真实接入 mcp-mesh API
///
/// `json` flag(Task 1.7):`true` 时各子命令输出 JSON envelope。
/// `perm`(Task 1.11):仅 `mcp call` 消费,用于破坏性操作前确认。
/// `dry_run`(Task 2.2):仅 `mcp call` 消费,`true` 时只输出预览不执行。
pub async fn execute(
    action: &McpAction,
    _config: &ChimeraConfig,
    json: bool,
    perm: &PermissionCtx,
    dry_run: bool,
) -> Result<()> {
    tracing::info!(?action, dry_run, "MCP 量子网格管理操作");

    // 构造进程内 ephemeral McpMesh(与 chimera run 一致的设计)
    let mesh = McpMesh::new(MeshConfig::default());

    match action {
        McpAction::List => list_servers(&mesh, json).await,
        McpAction::Serve => serve_server().await,
        McpAction::Call { server, tool, args } => {
            call_tool(&mesh, server, tool, args, perm, json, dry_run).await
        }
        McpAction::Inspect { server } => inspect_server(&mesh, server, json).await,
    }
}

/// `mcp list` — 列出所有已注册 MCP 服务器(SubTask 1.8.2)
///
/// 默认表格输出(`comfy-table`),`--json` 时输出 JSON 数组 envelope。
/// 进程内 ephemeral mesh 的注册表为空,输出友好提示。
async fn list_servers(mesh: &McpMesh, json: bool) -> Result<()> {
    let server_ids = mesh.registry().list_all();

    if json {
        // JSON envelope: { "status": "ok", "data": [...] }
        let payload = serde_json::json!({
            "servers": server_ids,
            "count": server_ids.len(),
        });
        output::print_json(&payload)?;
    } else if server_ids.is_empty() {
        // 空列表友好提示(到 stderr,不污染 stdout 数据流)
        output::print_info("当前无 MCP 服务器(进程内 ephemeral mesh,不持久化)");
    } else {
        // 表格输出:Server ID / Endpoint / Capabilities / 存活状态
        let rows: Vec<Vec<String>> = server_ids
            .iter()
            .filter_map(|sid| mesh.registry().get(sid))
            .map(|server| {
                // 先借 server 调用 is_alive,再 move server_id/endpoint
                // WHY 顺序敏感:is_alive(&self) 借用整个 server,
                // 若先 move server_id/endpoint 会导致 server 部分移动,
                // 后续 is_alive 借用会触发 E0382(borrow of partially moved value)
                let alive = server.is_alive(30_000);
                vec![
                    server.server_id,
                    server.endpoint,
                    server.capabilities.join(", "),
                    if alive { "alive".into() } else { "dead".into() },
                ]
            })
            .collect();
        output::print_table(&["Server ID", "Endpoint", "Capabilities", "状态"], &rows);
    }
    Ok(())
}

/// `mcp serve` — 启动 MCP 服务器(SubTask 1.8.3)
///
/// WHY NotImplemented:真实服务器启动需要:
/// 1. 绑定网络端口(0.0.0.0:8080 等)
/// 2. 加载 TLS 证书(生产环境)
/// 3. 注册到服务发现(集群部署)
/// 4. 后台守护进程化(脱离 CLI 会话)
///
/// 这些配置不适合 CLI 一次性启动,应通过 `chimera tui` 或独立部署 mcp-mesh 服务实现。
async fn serve_server() -> Result<()> {
    Err(ChimeraCliError::NotImplemented(
        "mcp serve 命令尚未接入真实服务器启动逻辑。\
         生产环境请使用 `chimera tui`(长生命周期 mesh + 后台探活)或独立部署 mcp-mesh 服务。"
            .into(),
    )
    .into())
}

/// `mcp call <server> <tool> [args]` — 调用 MCP 工具(SubTask 1.8.4)
///
/// 触发 permission prompt(除非 `--yes` / `--no-permission`)。
/// 确认后通过 `McpMesh::execute_transaction` 调用工具。
/// 进程内 ephemeral mesh 无注册服务器,调用必返回 EngineError(ServerNotFound)。
///
/// `dry_run=true`(Task 2.2):permission 确认后只输出预览,不调用 execute_transaction。
async fn call_tool(
    mesh: &McpMesh,
    server: &str,
    tool: &str,
    args: &[String],
    perm: &PermissionCtx,
    json: bool,
    dry_run: bool,
) -> Result<()> {
    // Task 1.11.4:破坏性操作前调用 confirm(MCP 工具可能执行任意代码)
    let details = format!("Server: {server}, Tool: {tool}, Args: {:?}", args);
    let confirmed = permission::confirm(perm, "调用 MCP 工具", &details).await?;
    if !confirmed {
        return Err(
            ChimeraCliError::PermissionDenied(format!("用户拒绝调用 MCP 工具 {tool}")).into(),
        );
    }

    // Task 2.2:dry-run 模式只输出预览,不实际执行
    // WHY 在 permission 之后:确保预览前仍经过权限确认,避免绕过安全检查
    if dry_run {
        eprintln!(
            "[dry-run] 将调用 MCP 工具 {server}/{tool} {:?},不执行",
            args
        );
        return Ok(());
    }

    // 真实调用 McpMesh::execute_transaction
    // WHY 返回 EngineError:进程内 ephemeral mesh 无注册服务器,
    // execute_transaction 对未注册 server_id 返回 ServerNotFound
    let result = mesh
        .execute_transaction(vec![server.to_string()], tool.to_string())
        .await
        .map_err(|e| ChimeraCliError::EngineError(format!("MCP 工具调用失败: {e}")))?;

    if json {
        let payload = serde_json::json!({
            "server": server,
            "tool": tool,
            "args": args,
            "result": {
                "success": result.success,
                "transaction_id": result.transaction_id,
            },
        });
        output::print_json(&payload)?;
    } else {
        output::print_success(&format!(
            "MCP 工具调用完成: {server}/{tool}(transaction: {})",
            result.transaction_id
        ));
    }
    Ok(())
}

/// `mcp inspect <server>` — 输出服务器详情(SubTask 1.8.5)
///
/// 输出注册时间 / 心跳 / 支持工具清单。
/// 不存在时返回 EngineError(退出码 3)。
async fn inspect_server(mesh: &McpMesh, server_id: &str, json: bool) -> Result<()> {
    match mesh.registry().get(server_id) {
        Some(server) => {
            if json {
                output::print_json(&server)?;
            } else {
                // 人类可读:逐行打印服务器字段
                println!("Server ID: {}", server.server_id);
                println!("Endpoint: {}", server.endpoint);
                println!("注册时间: {}", server.last_heartbeat);
                println!("Capabilities: {}", server.capabilities.join(", "));
                println!(
                    "存活状态: {}",
                    if server.is_alive(30_000) {
                        "alive"
                    } else {
                        "dead"
                    }
                );
            }
            Ok(())
        }
        None => Err(ChimeraCliError::EngineError(format!("MCP 服务器不存在: {server_id}")).into()),
    }
}
