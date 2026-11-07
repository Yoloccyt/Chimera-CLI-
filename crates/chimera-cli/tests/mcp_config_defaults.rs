//! MCP 配置默认值契约测试 — ADR-255 配置诚实化收口(进程内,零子进程)
//!
//! 覆盖:
//! - 默认 servers 为空(类型层契约)
//! - 注入 url 型 server 后「配置 → 注册 → 列表」链路经 lib API 端到端成立
//!
//! WHY 进程内直调(零子进程):默认契约在类型层断言;链路经
//! `commands::mcp::execute` 直调,与既有子进程形态测试互补,且不新增
//! spawn 点位(Mimosa 门对新增 `Command::new` 保持拦截)。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code

use chimera_cli::cli::McpAction;
use chimera_cli::commands::mcp;
use chimera_cli::config::{ChimeraConfig, McpServerConfig};
use chimera_cli::permission::PermissionCtx;

/// ADR-255 契约:默认 servers 为空——历史默认三条(filesystem=stdio 不被
/// 注册端支持 / postgres=localhost 被 SSRF 拒 / github=api.github.com 404)
/// 无一可真实调用,不再分发。
#[test]
fn default_mcp_servers_empty_by_default() {
    let cfg = ChimeraConfig::default();
    assert!(
        cfg.mcp.servers.is_empty(),
        "默认 servers 应为空(ADR-255 诚实化收口)"
    );
}

/// 注入一条 RFC 5737 TEST-NET-3 url 型 server(公网字面量,SSRF 放行),
/// 经 lib API 走完整 mcp list 链路(配置 → 注册 → 列表 → 输出)。
///
/// WHY 注入式而非默认:ADR-255 后默认列表为空,注册链路的回归门改为
/// 显式注入驱动(与 mcp-mesh 自身集成测试互补,覆盖 cli 侧映射段)。
#[tokio::test]
async fn list_chain_with_injected_url_server_succeeds() {
    let mut cfg = ChimeraConfig::default();
    cfg.mcp.servers.push(McpServerConfig {
        id: "ci-probe".to_string(),
        url: Some("https://203.0.113.1:8443".to_string()),
    });

    // --yes 跳过交互确认(list 本身非破坏性,构造合法 PermissionCtx 即可)
    let perm = PermissionCtx {
        yes: true,
        no_permission: false,
    };

    mcp::execute(&McpAction::List, &cfg, false, &perm, false)
        .await
        .expect("mcp list 链路应成功(注册 → 列表 → 输出)");
}

/// 注入一条 loopback url 型 server,验证 SSRF 拒绝路径不 panic、命令仍成功
/// (warn 后跳过,列表为空)——「显式 warn 不静默」的进程内契约。
#[tokio::test]
async fn list_chain_with_ssrf_blocked_server_still_succeeds() {
    let mut cfg = ChimeraConfig::default();
    cfg.mcp.servers.push(McpServerConfig {
        id: "loopback-probe".to_string(),
        url: Some("https://127.0.0.1:8443".to_string()),
    });

    let perm = PermissionCtx {
        yes: true,
        no_permission: false,
    };

    mcp::execute(&McpAction::List, &cfg, false, &perm, false)
        .await
        .expect("SSRF 被拒时应 warn 跳过而非整体失败");
}
