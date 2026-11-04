//! ADR-235 S7：MCP 接入**不得削减审批面**的正反锁。
//!
//! 三条：
//! 1. 默认装配（`trust_readonly_hints` 未配置 = false）下，即使服务器自报
//!    `readOnlyHint: true`，发现出来的每个 MCP 工具也必须是 `Write`；
//! 2. 只有逐 server 显式开信任，且 `readOnlyHint == true` 且
//!    `destructiveHint != true` 才降为 `ReadOnly`；缺注解/自相矛盾一律 `Write`；
//! 3. **降级不等于免审批**：任何未获审批的调用仍然 `AskRequired`。
//!
//! 反证（实施期实测）：删掉 `side_effect_for` 的 `&& !destructive` ⇒ 第 2 条红；
//! 把 `trust_readonly_hints` 的默认值改成 true ⇒ 第 1 条红。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production
#![forbid(unsafe_code)]

use std::sync::Arc;

use chimera_cli::config::ChimeraConfig;
use chimera_cli::tool_plane::{assemble_mcp, McpToolProvider};
use gqep_executor::tool_plane::{
    ToolError, ToolInvocation, ToolOrigin, ToolPriority, ToolProvider,
};
use mcp_mesh::protocol::{
    ClientConfig, CommandGate, DiscoveredServer, DiscoverySnapshot, DuplexTransport, McpClient,
    McpTool, ToolAnnotations,
};
use nexus_contracts::tool_plan::SideEffectDecl;
use tokio_util::sync::CancellationToken;

/// 只放行 bash 的闸门（这些用例专测审批面，不测 SecCore 白名单）。
struct AllowBash;

impl CommandGate for AllowBash {
    fn check(&self, program: &str, _args: &[String]) -> Result<(), String> {
        if program == "bash" {
            Ok(())
        } else {
            Err(format!("{program} is not allowlisted"))
        }
    }
}

/// 一个真的能完成 `initialize` + `tools/list` 的假 MCP 服务器，
/// 其唯一工具声明 `readOnlyHint: true`（这正是"不可信 hint"的诱饵）。
fn fake_server_config(id: &str) -> chimera_cli::config::ChimeraConfig {
    let script = r#"while IFS= read -r line; do
  case "$line" in
    *'"method":"initialize"'*)
      id=$(printf '%s' "$line" | sed -E 's/.*"id":([0-9]+).*/\1/')
      printf '{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-03-26","serverInfo":{"name":"fake-mcp","version":"1.0"},"capabilities":{}}}\n' "$id"
      ;;
    *'"method":"tools/list"'*)
      id=$(printf '%s' "$line" | sed -E 's/.*"id":([0-9]+).*/\1/')
      printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"fake.echo","description":"fake","inputSchema":{"type":"object"},"annotations":{"readOnlyHint":true,"destructiveHint":false}}]}}\n' "$id"
      ;;
  esac
done"#;
    // 走 YAML 反序列化而不是手写结构体：要证明"未配置 trust"这个**配置默认值**
    // 本身就是最保守档，而不是被测试自己设成 false。
    let yaml = format!(
        "mcp:\n  servers:\n    - id: {id}\n      command: bash\n      enabled: true\n      timeout_ms: 5000\n"
    );
    let mut config: ChimeraConfig = serde_yaml::from_str(&yaml).expect("yaml must parse");
    config.mcp.servers[0].args = Some(vec!["-c".to_string(), script.to_string()]);
    assert!(
        !config.mcp.servers[0].trust_readonly_hints,
        "前提：未写 trust_readonly_hints 时必须落到 false"
    );
    config
}

#[test]
fn default_assembly_keeps_every_mcp_tool_write() {
    // 发现链是真的（spawn → initialize → tools/list），hint 也是真的 readOnly，
    // 但默认档不采信 ⇒ 描述符必须全 Write。若默认 trust 被改成 true，这里红。
    let config = fake_server_config("untrusted");
    let plane = assemble_mcp(&config.mcp.servers, Arc::new(AllowBash));
    assert_eq!(plane.tool_count(), 1, "前提：假服务器必须被发现");
    let provider = McpToolProvider::new(Arc::clone(&plane.snapshot));
    let descriptors = provider.descriptors();
    assert_eq!(descriptors.len(), 1);
    assert_eq!(
        descriptors[0].name, "mcp:untrusted/fake.echo",
        "命名口径不得变（审批面与 CLI 解析依赖它）"
    );
    assert_eq!(
        descriptors[0].side_effect,
        SideEffectDecl::Write,
        "默认档必须全 Write：服务器自报的 readOnlyHint 不可信（D1）"
    );
}

fn tool_with(name: &str, read_only: Option<bool>, destructive: Option<bool>) -> McpTool {
    let annotations = if read_only.is_none() && destructive.is_none() {
        None
    } else {
        Some(ToolAnnotations {
            read_only_hint: read_only,
            destructive_hint: destructive,
            idempotent_hint: None,
            open_world_hint: None,
        })
    };
    McpTool {
        name: name.to_string(),
        description: String::new(),
        input_schema: None,
        annotations,
        extra: serde_json::Map::new(),
    }
}

/// 信任档的穷举对照：只有 read_only 且非 destructive 才降；缺注解也是 Write。
#[tokio::test]
async fn trusted_server_only_downgrades_non_destructive_readonly() {
    let (transport, peer) = DuplexTransport::pair(1024);
    drop(peer);
    let client = Arc::new(McpClient::spawn(transport, ClientConfig::default()));
    let snapshot = Arc::new(DiscoverySnapshot {
        servers: vec![DiscoveredServer {
            id: "trusted".to_string(),
            tools: vec![
                tool_with("safe_read", Some(true), Some(false)),
                tool_with("sneaky_write", Some(true), Some(true)),
                tool_with("no_hints", None, None),
            ],
            trust_readonly_hints: true,
            client,
        }],
    });
    let provider = McpToolProvider::new(snapshot);
    let sides: Vec<(String, SideEffectDecl)> = provider
        .descriptors()
        .iter()
        .map(|d| (d.name.clone(), d.side_effect))
        .collect();
    assert_eq!(
        sides,
        vec![
            (
                "mcp:trusted/safe_read".to_string(),
                SideEffectDecl::ReadOnly
            ),
            // readOnly 与 destructive 同时为真 ⇒ 按 Write（fail-closed）。
            (
                "mcp:trusted/sneaky_write".to_string(),
                SideEffectDecl::Write
            ),
            ("mcp:trusted/no_hints".to_string(), SideEffectDecl::Write),
        ],
        "开信任只降\"只读且非破坏\"的工具；缺注解一律 Write"
    );
}

/// 降级 **不等于** 免审批：三个工具（含已降为 ReadOnly 的）未审批调用都 AskRequired。
#[tokio::test]
async fn downgrade_never_skips_the_approval_gate() {
    let (transport, peer) = DuplexTransport::pair(1024);
    drop(peer);
    let client = Arc::new(McpClient::spawn(transport, ClientConfig::default()));
    let snapshot = Arc::new(DiscoverySnapshot {
        servers: vec![DiscoveredServer {
            id: "trusted".to_string(),
            tools: vec![
                tool_with("safe_read", Some(true), Some(false)),
                tool_with("sneaky_write", Some(true), Some(true)),
                tool_with("no_hints", None, None),
            ],
            trust_readonly_hints: true,
            client,
        }],
    });
    let provider = McpToolProvider::new(snapshot);
    for name in [
        "mcp:trusted/safe_read",
        "mcp:trusted/sneaky_write",
        "mcp:trusted/no_hints",
    ] {
        let invocation = ToolInvocation {
            call_id: "s7".to_string(),
            tool: name.to_string(),
            args_json: "{}".to_string(),
            timeout_ms: None,
            priority: ToolPriority::Normal,
            origin: ToolOrigin::Cli,
            quest_id: None,
            task_id: None,
            approved: false,
        };
        let error = provider
            .invoke(&invocation, CancellationToken::new())
            .await
            .expect_err("未审批的 MCP 调用必须被拦下");
        assert!(
            matches!(error, ToolError::AskRequired(_)),
            "{name} 期望 AskRequired，实得 {error:?}"
        );
    }
}
