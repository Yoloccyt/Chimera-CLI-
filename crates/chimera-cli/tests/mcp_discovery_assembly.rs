//! ADR-235 S6：MCP 配置面与"装配期发现"的行为锁。
//!
//! 四条：
//! 1. 旧 YAML（没有新字段）必须能反序列化，且新字段落到**最保守**默认；
//! 2. 默认那三条服务器全是 `enabled = false` ⇒ 默认装配仍然一个 MCP 工具都没有；
//! 3. `enabled = true` 但命令被 SecCore fence 拒（npx）⇒ 跳过本 server、装配照常返回；
//! 4. 允许的命令但不是 MCP 服务器（`cat`）⇒ 在它自己的超时内被跳过，不拖住启动。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production
#![forbid(unsafe_code)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use chimera_cli::config::ChimeraConfig;
use chimera_cli::tool_plane::{assemble_mcp, FenceGate};
use mcp_mesh::protocol::CommandGate;
use nexus_core::config::{default_mcp_servers, McpServerConfig};

mod common;

fn server(id: &str) -> McpServerConfig {
    McpServerConfig {
        id: id.to_string(),
        ..Default::default()
    }
}

#[test]
fn legacy_yaml_without_new_fields_gets_safe_defaults() {
    // 真实 omega.yaml 里现存的就是这种形状（只有 id/command/args）——
    // 加了新字段必须向后兼容，且默认档必须是最保守的：不启用、不采信 hint。
    let yaml = r#"
mcp:
  servers:
    - id: legacy
      command: npx
      args: ["-y", "@modelcontextprotocol/server-filesystem"]
"#;
    let config: ChimeraConfig = serde_yaml::from_str(yaml).expect("legacy yaml must parse");
    let legacy = &config.mcp.servers[0];
    assert_eq!(legacy.id, "legacy");
    assert_eq!(legacy.command.as_deref(), Some("npx"));
    assert!(!legacy.enabled, "新字段 enabled 默认必须是 false（D2）");
    assert!(
        !legacy.trust_readonly_hints,
        "新字段 trust_readonly_hints 默认必须是 false（D1）"
    );
    assert_eq!(legacy.timeout_ms, 0, "0 = 走默认超时");
    assert!(legacy.env.is_empty());
}

#[test]
fn default_servers_are_all_disabled() {
    for configured in default_mcp_servers() {
        assert!(
            !configured.enabled,
            "默认服务器 {} 必须 enabled=false：本波不默认连任何 MCP（D2）",
            configured.id
        );
    }
}

#[test]
fn enabled_stdio_server_with_denied_command_is_skipped() {
    let mut blocked = server("blocked-npx");
    blocked.command = Some("npx".to_string());
    blocked.args = Some(vec!["-y".to_string(), "some-server".to_string()]);
    blocked.enabled = true;

    // 真闸门（SecCore default_secure）：npx 不在白名单 ⇒ Denied ⇒ 跳过。
    let plane = assemble_mcp(&[blocked], Arc::new(FenceGate::default_secure()));
    assert_eq!(plane.tool_count(), 0, "被 fence 拒的 server 不得产出工具");
}

#[test]
fn enabled_http_server_is_skipped_explicitly() {
    let mut http = server("http-only");
    http.url = Some("https://example.invalid/mcp".to_string());
    http.enabled = true;

    // HTTP/Streamable HTTP 本波未实现（D2）⇒ 显式拒绝，不静默当"发现成功但零工具"。
    let plane = assemble_mcp(&[http], Arc::new(FenceGate::default_secure()));
    assert_eq!(plane.tool_count(), 0);
}

#[test]
fn allowed_but_non_mcp_command_is_skipped_within_its_timeout() {
    let mut silent = server("cat-not-mcp");
    silent.command = Some("cat".to_string());
    silent.enabled = true;
    silent.timeout_ms = 200;

    // cat 在 fence 白名单里，会 spawn 并把请求原样回显 —— 但永远不会有合法的
    // `initialize` 响应 ⇒ 在 200ms 请求超时后跳过，启动不被拖住。
    let started = Instant::now();
    let plane = assemble_mcp(&[silent], Arc::new(FenceGate::default_secure()));
    assert_eq!(plane.tool_count(), 0, "非 MCP 对端不得产出工具");
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "不可达 server 必须在自己的超时内被跳过，实测 {:?}",
        started.elapsed()
    );
}

/// 记录每次校验并拒绝 —— 用来证明 gate **真的在 spawn 之前**被调用。
///
/// WHY 不看耗时：反证实测过，绕过闸门时 `cat` 也会很快失败（0.05s），
/// 时间判据对"闸门被绕过"无感。直接数调用次数才是可判定的证据。
struct SpyGate {
    calls: std::sync::Mutex<Vec<(String, Vec<String>)>>,
}

impl SpyGate {
    fn new() -> Self {
        Self {
            calls: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<(String, Vec<String>)> {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }
}

impl CommandGate for SpyGate {
    fn check(&self, program: &str, args: &[String]) -> Result<(), String> {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((program.to_string(), args.to_vec()));
        Err("denied by spy gate".to_string())
    }
}

#[test]
fn gate_is_consulted_before_spawn_for_every_enabled_server() {
    let mut mcp = server("spied");
    mcp.command = Some("some-mcp-server".to_string());
    mcp.args = Some(vec!["--serve".to_string()]);
    mcp.enabled = true;

    let gate = Arc::new(SpyGate::new());
    let plane = assemble_mcp(&[mcp], gate.clone());
    assert_eq!(plane.tool_count(), 0, "被闸门拒的 server 不得产出工具");
    assert_eq!(
        gate.calls(),
        vec![("some-mcp-server".to_string(), vec!["--serve".to_string()])],
        "闸门必须以**原样的程序名+参数**被调用恰一次；空列表即装配绕过了闸门"
    );
}

/// 只放行 bash 的闸门（正控用：不依赖 SecCore 白名单，专测装配接线）。
struct AllowBash;

impl CommandGate for AllowBash {
    fn check(&self, program: &str, _args: &[String]) -> Result<(), String> {
        if common::is_posix_shell(program) {
            Ok(())
        } else {
            Err(format!("{program} is not allowlisted"))
        }
    }
}

/// 一个**真的能用**的假 MCP 服务器（bash + while read 逐行回包）。
fn working_fake_server(id: &str) -> McpServerConfig {
    let script = r#"while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      id=$(printf '%s' "$line" | sed -E 's/.*"id":([0-9]+).*/\1/')
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-03-26","serverInfo":{"name":"fake-mcp","version":"1.0"},"capabilities":{}}}\n' "$id"
      ;;
    *'"method":"tools/list"'*)
      id=$(printf '%s' "$line" | sed -E 's/.*"id":([0-9]+).*/\1/')
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"fake.echo","description":"fake","inputSchema":{"type":"object"},"annotations":{"readOnlyHint":true}}]}}\n' "$id"
      ;;
  esac
done"#;
    let mut fake = server(id);
    fake.command = Some(common::shell_program().to_string());
    fake.args = Some(vec!["-c".to_string(), script.to_string()]);
    fake.enabled = true;
    fake.timeout_ms = 5_000;
    fake.trust_readonly_hints = true;
    fake
}

#[test]
fn enabled_stdio_server_is_discovered_end_to_end() {
    // 正控：证明 assemble 真的走通了 spawn → initialize → tools/list，
    // 而不是"所有测试都断言零工具"（后者对"发现永远失败"的回归无感）。
    let plane = assemble_mcp(&[working_fake_server("fake")], Arc::new(AllowBash));
    assert_eq!(plane.tool_count(), 1, "假服务器必须被发现出一个工具");
    let (discovered, tool) = plane
        .snapshot
        .find("fake", "fake.echo")
        .expect("快照里必须有 fake/fake.echo");
    assert_eq!(tool.description, "fake");
    assert!(
        discovered.trust_readonly_hints,
        "逐 server 信任开关必须透传"
    );
    assert_eq!(
        mcp_mesh::protocol::side_effect_for(tool, discovered.trust_readonly_hints),
        nexus_contracts::tool_plan::SideEffectDecl::ReadOnly,
        "正则上的假服务器声明 readOnlyHint=true 且非 destructive ⇒ 信任时应降为 ReadOnly"
    );
}

#[test]
fn gate_denial_prevents_discovery_of_a_working_server() {
    // 与上一条同一个能正常应答的服务器：放行 ⇒ 1 个工具，拒绝 ⇒ 0 个。
    // 若装配忽略闸门结果，这里会变成 1 并判红 —— 这是"拒绝被无视"的唯一有牙判据。
    let plane = assemble_mcp(
        &[working_fake_server("fake-denied")],
        Arc::new(SpyGate::new()),
    );
    assert_eq!(
        plane.tool_count(),
        0,
        "闸门拒绝必须压过'服务器其实能用'：0 ≠ 1 才说明拒绝被当真"
    );
}
