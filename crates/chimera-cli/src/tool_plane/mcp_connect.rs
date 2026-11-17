//! 装配期 MCP 连接与能力发现（ADR-235 S6）。
//!
//! # 同步装配 × 异步发现
//! 组合根 [`crate::composition::build_tool_runtime`] 是同步函数（多处同步测试也在调），
//! 而 stdio 发现必须在 Tokio runtime 上跑。这里的做法：
//! - **没有 `enabled` 的 server ⇒ 直接返回空快照，不建 runtime**（默认路径零成本）；
//! - 有启用的 server 时，建一个**本模块自己持有的** runtime，用 `block_on` 完成
//!   发现，并把 `Runtime` 连同快照一起留在 [`McpPlane`] 里 —— 客户端任务跑在这个
//!   runtime 上，它一被 drop，任务与子进程随之结束。
//!
//! WHY 不把 `build_tool_runtime` 改成 async：那会波及 `OnceLock` 缓存与全部调用
//! 点（含同步测试），收益只是省一次线程阻塞；而阻塞只发生在**真的配置了 MCP
//! server** 时，且每个 server 有超时上限。
//!
//! # 失败语义
//! 单个 server 的建连/握手/发现失败 ⇒ `tracing::warn!` 后跳过它，**不阻塞启动**
//! （ADR-235 D6）。HTTP/Streamable HTTP 不在本波范围 ⇒ 显式 warn 并跳过（D2）。

use std::sync::{Arc, Mutex};

use crate::config_types::McpServerConfig;
use mcp_mesh::protocol::{
    ClientConfig, CommandGate, DiscoveredServer, DiscoverySnapshot, McpClient, StdioTransport,
    DEFAULT_REQUEST_TIMEOUT_MS,
};
use mcp_mesh::McpError;
use nexus_contracts::command_validation::CommandPolicy;
use seccore::os_backend::{ProcessFence, SandboxMode};

/// 生产闸门：把 SecCore [`ProcessFence`] 适配成 `CommandGate`。
///
/// 组合根注入它；`mcp-mesh` 只认 trait，不依赖 L4（避免 L10 反向依赖成环）。
pub struct FenceGate {
    fence: Mutex<ProcessFence>,
}

impl FenceGate {
    /// `default_secure` 白名单 + Standard 沙箱档（与 `ProcessToolProvider` 同源）。
    #[must_use]
    pub fn default_secure() -> Self {
        Self {
            fence: Mutex::new(ProcessFence::new(
                Arc::new(CommandPolicy::default_secure()),
                SandboxMode::Standard,
            )),
        }
    }
}

impl CommandGate for FenceGate {
    fn check(&self, program: &str, args: &[String]) -> Result<(), String> {
        // 临界区内只有一次纯内存校验，没有 await。
        let mut fence = self
            .fence
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        fence
            .check(program, args)
            .map_err(|error| error.to_string())
    }
}

/// 装配产物：发现快照 + 保活句柄。
pub struct McpPlane {
    /// 已完成发现的服务器（不可达/被拒的不在其中）
    pub snapshot: Arc<DiscoverySnapshot>,
    /// 客户端任务所在的 runtime；`None` = 没有启用的 server。
    /// **必须与 snapshot 同寿命**：drop 它等于终止所有 MCP 会话。
    _runtime: Option<tokio::runtime::Runtime>,
}

impl McpPlane {
    /// 空装配（不建 runtime，零成本）。
    #[must_use]
    pub fn empty() -> Self {
        Self {
            snapshot: Arc::new(DiscoverySnapshot::default()),
            _runtime: None,
        }
    }

    /// 可暴露的工具总数。
    #[must_use]
    pub fn tool_count(&self) -> usize {
        self.snapshot.tool_count()
    }
}

/// 对 `enabled = true` 的 server 建连并发现；其余一律不动。
///
/// WHY 发现跑在独立 OS 线程（W2）：本函数是同步签名，而 W2 起组合根 `build()`
/// 在四环链装配前预触发共享 tool_plane（[W2-b]）——`build()` 的既有调用方
/// 可能已身处 tokio runtime（如协议宿主/测试的 `#[tokio::test]`），私有
/// runtime 的 `block_on` 会触发「Cannot start a runtime from within a
/// runtime」panic。独立线程内建 runtime + block_on 对**任何调用方上下文**
/// 都安全；装配是一次性动作，线程 spawn 开销（≈293µs，ADR-275 D2 读数）
/// 相对冷启动发现（秒级）可忽略。会话 runtime 由 [`McpPlane::_runtime`]
/// 保活，与发现 runtime 同体——drop 即全会话终止（既有语义不变）。
#[must_use]
pub fn assemble(servers: &[McpServerConfig], gate: Arc<dyn CommandGate>) -> McpPlane {
    let enabled: Vec<McpServerConfig> = servers.iter().filter(|s| s.enabled).cloned().collect();
    if enabled.is_empty() {
        return McpPlane::empty();
    }
    // 发现与会话共用一个 runtime（spawn 的 MCP 客户端任务跑在它上面）；
    // join handle 失败（线程 panic）按 D6 fail-closed：跳过发现不拖住启动。
    let discovery = std::thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(error) => {
                tracing::warn!(%error, "MCP runtime unavailable; skipping discovery (startup continues)");
                return None;
            }
        };
        let snapshot = runtime.block_on(discover_all(enabled, gate));
        Some((snapshot, runtime))
    });
    match discovery.join() {
        Ok(Some((snapshot, runtime))) => McpPlane {
            snapshot: Arc::new(snapshot),
            _runtime: Some(runtime),
        },
        Ok(None) => McpPlane::empty(),
        Err(_) => {
            tracing::warn!("MCP discovery thread panicked; skipping discovery (startup continues)");
            McpPlane::empty()
        }
    }
}

async fn discover_all(
    servers: Vec<McpServerConfig>,
    gate: Arc<dyn CommandGate>,
) -> DiscoverySnapshot {
    let mut discovered = Vec::new();
    for server in servers {
        match discover_one(&server, gate.as_ref()).await {
            Ok(one) => {
                tracing::info!(
                    server = %one.id,
                    tools = one.tools.len(),
                    "MCP server discovered"
                );
                discovered.push(one);
            }
            Err(error) => {
                // fail-closed：一个 server 坏了只影响它自己（D6）。
                tracing::warn!(
                    server = %server.id,
                    %error,
                    "MCP server skipped (startup continues)"
                );
            }
        }
    }
    DiscoverySnapshot {
        servers: discovered,
    }
}

async fn discover_one(
    server: &McpServerConfig,
    gate: &dyn CommandGate,
) -> Result<DiscoveredServer, McpError> {
    let Some(command) = server.command.as_deref() else {
        return Err(McpError::Protocol {
            reason: "only stdio transport is implemented in this wave (ADR-235 D2)".into(),
        });
    };
    let args = server.args.clone().unwrap_or_default();
    let timeout_ms = if server.timeout_ms == 0 {
        DEFAULT_REQUEST_TIMEOUT_MS
    } else {
        server.timeout_ms
    };
    // fence 在 spawn 之前：被拒 ⇒ Denied，进程根本不会起来。
    let transport = StdioTransport::spawn(command, &args, &server.env, gate)?;
    let client = Arc::new(McpClient::spawn(
        transport,
        ClientConfig {
            request_timeout_ms: timeout_ms,
        },
    ));
    let outcome = async {
        client.initialize().await?;
        client.list_tools().await
    }
    .await;
    match outcome {
        Ok(tools) => Ok(DiscoveredServer {
            id: server.id.clone(),
            tools,
            trust_readonly_hints: server.trust_readonly_hints,
            client,
        }),
        Err(error) => {
            // 半接通的会话要显式收尾（kill+wait），否则子进程会拖到 runtime 关闭。
            client.shutdown().await;
            Err(error)
        }
    }
}
