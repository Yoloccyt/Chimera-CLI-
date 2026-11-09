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
//!   每次 `mcp list` 会先按 `config.mcp().servers` 注册（url 型可注册；stdio 型显式 warn 后跳过），
//!   因此列表反映当次配置源，但注册表不跨进程持久化。
//! - **`mcp serve` 为 NotImplemented**:真实服务器启动需要绑定网络端口、加载 TLS 证书等,
//!   不适合 CLI 一次性启动。指引替代方案:`chimera tui` 或独立部署 mcp-mesh 服务。
//! - **`mcp call` 触发 permission prompt**:调用外部 MCP 工具属破坏性操作(可能执行任意代码),
//!   必须调用 `permission::confirm` 获取用户确认(除非 `--yes` / `--no-permission`)。

use anyhow::Result;
use mcp_mesh::{
    InProcessClient, McpMesh, MeshConfig, MeshServer, ParticipantClient, TcpParticipantClient,
};
use serde::Serialize;
use std::sync::Arc;

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
/// `runtime`(ADR-273):仅 `mcp call` 消费 —— 组合根**唯一** ToolRuntime，
/// 用于执行前的 policy + sandbox 准入前置（`list/inspect/serve` 传 `None`，
/// 三者分类为 NoContext，不付装配税）。
pub async fn execute(
    action: &McpAction,
    config: &ChimeraConfig,
    json: bool,
    perm: &PermissionCtx,
    dry_run: bool,
    runtime: Option<&gqep_executor::tool_runtime::ToolRuntime>,
) -> Result<()> {
    tracing::info!(?action, dry_run, "MCP 量子网格管理操作");

    // 构造进程内 ephemeral McpMesh(与 chimera run 一致的设计)
    // 默认 `InProcessClient`：**零网络副作用**。仅 `mcp call --network` 显式开关
    // 才切到 `TcpParticipantClient`（真实 TCP，会连服务器 endpoint）。
    let use_network = matches!(action, McpAction::Call { network: true, .. });
    let mesh = build_mesh(use_network);
    // E4′:注册必须发生在 list/call/inspect 之前，否则 registry 恒空
    register_configured_servers(&mesh, config)?;

    match action {
        McpAction::List => list_servers(&mesh, json).await,
        McpAction::Serve => serve_server().await,
        McpAction::Call {
            server,
            tool,
            args,
            network,
        } => {
            if *network {
                // 副作用可见性（ADR-245 D4）：真实 TCP 生效时在 stderr 留痕
                tracing::info!(
                    server = %server,
                    tool = %tool,
                    "mcp call: 已显式启用真实 TCP 传输（可能产生网络请求）"
                );
            }
            call_tool(&mesh, server, tool, args, perm, json, dry_run, runtime).await
        }
        McpAction::Inspect { server } => inspect_server(&mesh, server, json).await,
    }
}

/// 按显式开关装配 participant 客户端
///
/// # 副作用边界
/// - `use_network = false`（默认）：[`InProcessClient`]，进程内伪传输，**不发任何网络请求**；
/// - `use_network = true`：[`TcpParticipantClient`]，真实 TCP + 4 字节长度前缀 JSON 帧，
///   会连接注册端点（受 registry SSRF 校验约束；loopback/保留地址在 `register` 阶段即被拒）。
fn build_mesh(use_network: bool) -> McpMesh {
    if use_network {
        let client: Arc<dyn ParticipantClient> = Arc::new(TcpParticipantClient::new());
        McpMesh::with_participant_client(MeshConfig::default(), None, client)
    } else {
        let client: Arc<dyn ParticipantClient> = Arc::new(InProcessClient::new());
        McpMesh::with_participant_client(MeshConfig::default(), None, client)
    }
}

/// 将 `omega.yaml` 的 `mcp.servers` 注册进 ephemeral mesh registry（E4′）
///
/// - `url` 型：直接以 `url` 作为 endpoint 注册，`validate_endpoint`（SSRF）拒绝时
///   显式 warn（不静默跳过）；
/// - stdio 型（无 `url`、有 `command`）：MCP mesh 只支持网络 endpoint，stdio 无法
///   真实注册。**直接显式 warn 后跳过**——不再合成 `stdio://localhost/<id>` 必败
///   端点去借道 SSRF 拒绝（旧迂回模式），行为等价（registry 本就不收录 stdio），
///   但意图直达、不依赖无关防线的副作用。
fn register_configured_servers(mesh: &McpMesh, config: &ChimeraConfig) -> Result<()> {
    let mcp = &config.mcp;

    for server in &mcp.servers {
        if server.id.is_empty() {
            tracing::warn!("MCP 配置项缺少 server id，已跳过");
            continue;
        }

        // stdio 型:mesh 仅支持网络 endpoint,显式 warn(不静默跳过)
        let Some(url) = server.url.as_deref().filter(|u| !u.is_empty()) else {
            tracing::warn!(
                server_id = %server.id,
                "stdio 服务器暂不支持注册（仅 url 型可注册），已跳过"
            );
            continue;
        };

        let candidate =
            MeshServer::new(server.id.clone(), url.to_string(), vec!["url".to_string()]);
        match mesh.registry().register(candidate) {
            Ok(()) => {
                tracing::info!(server_id = %server.id, endpoint = %url, "MCP 服务器已注册");
            }
            Err(err) => {
                tracing::warn!(
                    server_id = %server.id,
                    endpoint = %url,
                    error = %err,
                    "MCP 服务器注册被拒（SSRF 校验）"
                );
            }
        }
    }
    Ok(())
}

/// `mcp list` 的 JSON 载荷（typed，字段名编译期校验）。
#[derive(Debug, Serialize)]
struct McpListPayload<'a> {
    servers: &'a [String],
    count: usize,
}

/// `mcp list` — 列出所有已注册 MCP 服务器(SubTask 1.8.2)
///
/// 默认表格输出(`comfy-table`),`--json` 时输出 JSON 数组 envelope。
/// 输出源为本次 `execute` 已按 `config.mcp().servers` 注册后的 registry。
async fn list_servers(mesh: &McpMesh, json: bool) -> Result<()> {
    let mut server_ids = mesh.registry().list_all();
    // registry 底层是 DashMap，排序保证输出确定性与快照稳定
    server_ids.sort();

    let payload = McpListPayload {
        servers: server_ids.as_slice(),
        count: server_ids.len(),
    };
    output::emit(json, &payload, || {
        if server_ids.is_empty() {
            // 空列表友好提示(到 stderr,不污染 stdout 数据流)
            output::print_info("当前无已注册的 MCP 服务器（配置源未提供可注册的 url 型服务器）");
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
    })
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

/// `mcp call` 的 JSON 载荷（typed，字段名编译期校验）。
#[derive(Debug, Serialize)]
struct McpCallResult<'a> {
    success: bool,
    transaction_id: &'a str,
}

#[derive(Debug, Serialize)]
struct McpCallPayload<'a> {
    server: &'a str,
    tool: &'a str,
    args: &'a [String],
    result: McpCallResult<'a>,
}

/// `mcp call <server> <tool> [args]` — 调用 MCP 工具(SubTask 1.8.4)
///
/// 触发 permission prompt(除非 `--yes` / `--no-permission`)。
/// 确认后先过 policy + sandbox 准入前置（ADR-273），再通过
/// `McpMesh::execute_transaction` 调用工具。
/// 目标服务器未在本次注册中出现时，仍返回 `McpError::ServerNotFound`（既有语义）。
///
/// `dry_run=true`(Task 2.2):permission 与准入前置都通过后只输出预览,
/// 不调用 execute_transaction（预览必须反映真实结论，故前置不可跳过）。
#[allow(clippy::too_many_arguments)] // 7 个参数均为命令面既有语义位;合并成结构体属独立重构
async fn call_tool(
    mesh: &McpMesh,
    server: &str,
    tool: &str,
    args: &[String],
    perm: &PermissionCtx,
    json: bool,
    dry_run: bool,
    runtime: Option<&gqep_executor::tool_runtime::ToolRuntime>,
) -> Result<()> {
    // Task 1.11.4:破坏性操作前调用 confirm(MCP 工具可能执行任意代码)
    let details = format!("Server: {server}, Tool: {tool}, Args: {:?}", args);
    let confirmed = permission::confirm(perm, "调用 MCP 工具", &details).await?;
    if !confirmed {
        return Err(
            ChimeraCliError::PermissionDenied(format!("用户拒绝调用 MCP 工具 {tool}")).into(),
        );
    }

    // ADR-273：命令面安全前置 —— 复用组合根**唯一** ToolRuntime 的 policy+sandbox 两环。
    //
    // WHY 在 confirm 之后：confirm 是「用户意图」闸门，policy/sandbox 是「机器规则」
    // 闸门，两道独立 —— 用户同意不能替代策略放行，策略放行也不能替代用户同意。
    // WHY 在 dry_run 之前：预览必须反映真实结论；若预览说"会调用"而实跑被策略拒绝，
    // dry-run 就失去意义。
    // WHY 用 admit 而非 dispatch：dispatch 会落到传输环**真的执行**工具，造成二次
    // 执行，且执行者是组合根那套 MCP 执行器而非本命令的 ephemeral mesh（两套生命周期）。
    let admission_req = gqep_executor::tool_runtime::DispatchRequest {
        tool_name: format!("mcp.{server}.{tool}"),
        args_json: serde_json::to_string(args).unwrap_or_else(|_| "[]".into()),
    };
    runtime
        .ok_or_else(|| {
            ChimeraCliError::EngineError(
                "内部装配错误：mcp call 缺少 ToolRuntime（分类应为 FullContext）".into(),
            )
        })?
        .admit(&admission_req)
        .map_err(|e| {
            ChimeraCliError::PermissionDenied(format!("MCP 工具调用被安全前置拒绝: {e}"))
        })?;

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
    // WHY 返回 EngineError:目标 server 若未出现在本次注册（含 stdio 跳过/SSRF 拒绝），
    // execute_transaction 仍返回 ServerNotFound（既有语义保持不变）。
    // args 全量透传（ADR-253 D1）：随 Prepare 帧抵达参与者，不再在 CLI 层静默丢弃。
    let result = mesh
        .execute_transaction(vec![server.to_string()], tool.to_string(), args.to_vec())
        .await
        .map_err(|e| ChimeraCliError::EngineError(format!("MCP 工具调用失败: {e}")))?;

    let payload = McpCallPayload {
        server,
        tool,
        args,
        result: McpCallResult {
            success: result.success,
            transaction_id: &result.transaction_id,
        },
    };
    output::emit(json, &payload, || {
        output::print_success(&format!(
            "MCP 工具调用完成: {server}/{tool}(transaction: {})",
            result.transaction_id
        ));
        Ok(())
    })
}

/// `mcp inspect <server>` — 输出服务器详情(SubTask 1.8.5)
///
/// 输出注册时间 / 心跳 / 支持工具清单。
/// 不存在时返回 EngineError(退出码 3)。
async fn inspect_server(mesh: &McpMesh, server_id: &str, json: bool) -> Result<()> {
    match mesh.registry().get(server_id) {
        Some(server) => {
            output::emit(json, &server, || {
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
                Ok(())
            })
        }
        None => Err(ChimeraCliError::EngineError(format!("MCP 服务器不存在: {server_id}")).into()),
    }
}
