//! `mcp call` 传输选择验收（收尾批次 A3）
//!
//! # 契约
//!
//! - **默认零网络副作用**：`mcp call` 未显式 `--network` 时装配 `InProcessClient`
//!   （进程内伪传输），不打开任何 socket；
//! - **真实 TCP 仅在显式开关下**：`--network` 装配 `TcpParticipantClient`
//!   （4 字节 BE 长度前缀 + JSON 帧），会连接注册端点；
//! - **SSRF 守卫在开关之前**：registry 的 `validate_endpoint` 先于任何 connect，
//!   `--network` 也不能绕过（loopback/保留地址在 `register` 阶段即被拒）。
//!
//! # 受控性说明（为什么不在这里连本地 mock server）
//!
//! CLI 层注册表强制 SSRF 校验，**拒绝 loopback**（127.0.0.0/8 与 `localhost`），
//! 因此无法在 CLI 集成测试里注册一个 127.0.0.1 的 mock 端点。
//! 真实 TCP 帧协议的端到端受控验证已由 `mcp-mesh` 内部
//! `participant_client.rs` 的本地 mock server 测试覆盖（不触网）。
//! 本文件只锁 CLI 层的两条边界：默认不发网络请求、SSRF 守卫不被开关绕过。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom
#![forbid(unsafe_code)]

use chimera_cli::cli::McpAction;
use chimera_cli::commands::mcp;
use chimera_cli::config::{ChimeraConfig, McpServerConfig};
use chimera_cli::permission::PermissionCtx;

fn config_with_server(id: &str, url: &str) -> ChimeraConfig {
    let mut config = ChimeraConfig::default();
    // ADR-255:McpServerConfig 收口为 id + url(死字段 command/args/auth 已移除)
    config.mcp.servers = vec![McpServerConfig {
        id: id.to_string(),
        url: Some(url.to_string()),
    }];
    config
}

fn auto_perm() -> PermissionCtx {
    PermissionCtx {
        yes: true,
        no_permission: false,
    }
}

/// 准入前置全放行的 ToolRuntime（本文件只验传输选择，不验策略）
///
/// delegate=None ⇒ 传输环装恒拒绝执行器，但本文件从不派发（只调 `admit`），
/// 故不影响断言；policy 显式 Allow 以保持与 ADR-246 缺省一致。
fn allow_runtime() -> gqep_executor::tool_runtime::ToolRuntime {
    gqep_executor::tool_runtime::ToolRuntime::with_optional_delegate(
        None,
        vec![],
        seccore::execpolicy::PolicyAction::Allow,
    )
    .expect("准入环组装应成功")
}

/// 默认路径必须成功且**不发网络请求**。
///
/// 证据选择：端点用 RFC 5737 TEST-NET-3（`203.0.113.1:9`，可注册但不可路由）。
/// 若默认路径真的发起 TCP connect，该地址必然失败/超时 ⇒ 不会 `Ok`。
/// 返回 `Ok` 只能来自进程内伪传输，即**没有打开 socket**。
#[tokio::test]
async fn default_call_never_opens_network() {
    let config = config_with_server("dead", "203.0.113.1:9");
    let action = McpAction::Call {
        server: "dead".to_string(),
        tool: "text.echo".to_string(),
        args: vec![r#"{"text":"hi"}"#.to_string()],
        network: false,
    };

    let runtime = allow_runtime();
    mcp::execute(&action, &config, true, &auto_perm(), false, Some(&runtime))
        .await
        .expect("默认（InProcess）路径应成功，且不触网");
}

/// `--network` 不得绕过 SSRF 守卫：loopback 端点在注册阶段即被拒，
/// 因此调用在**任何 connect 之前**就失败（错误是 ServerNotFound 语义）。
#[tokio::test]
async fn network_flag_does_not_bypass_ssrf_guard() {
    let config = config_with_server("loopback", "mcp://127.0.0.1:9");
    let action = McpAction::Call {
        server: "loopback".to_string(),
        tool: "text.echo".to_string(),
        args: vec![],
        network: true,
    };

    let runtime = allow_runtime();
    let err = mcp::execute(&action, &config, true, &auto_perm(), false, Some(&runtime))
        .await
        .expect_err("loopback 端点必须被 SSRF 校验拒绝");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("服务器未找到") || msg.contains("ServerNotFound"),
        "应归因 registry 未注册（SSRF 拒绝），实际: {msg}"
    );
}
