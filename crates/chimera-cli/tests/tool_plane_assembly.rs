//! 默认装配的**工具面组成**（ADR-233 W3）。
//!
//! 把"MCP provider 在默认装配里不在场"从**偶然状态**变成**被钉的事实**。
//!
//! WHY 这条测试现在长这样：`composition::build_tool_runtime` 只在
//! `mcp_provider.descriptors()` 非空时才注册 MCP（空 provider 会被注册表
//! all-or-nothing 拒掉），而它构造的 `McpMesh` 用的是空注册表 + `InProcessClient`
//! ⇒ descriptors 恒空 ⇒ MCP 永不在场。今天没有任何断言说过"应该在"或"不应该"，
//! 所以这个事实只存在于代码的巧合里。
//!
//! WHY 不顺手接上真实装配（本波实测，理由与翻转条件见 ADR-233 追加节六）：
//! `mcp-mesh` 的 participant client 说的是自家 `TwoPcRequest` / `QueryRequest` /
//! `ToolInvokeRequest` 协议，`grep -rn "jsonrpc" crates/mcp-mesh/src` **零命中**，
//! 而 `client_v2.rs` 的头注自陈原实现在 2026-08-28 磁盘 ENOSPC 事故中丢失、现仅按
//! WI-22/ADR-150 语义重建了三项数据结构骨架，"未恢复丢失前的完整连接器/协议握手逻辑"
//! ⇒ 真实 MCP 能力发现（`initialize` → `tools/list`）是一个**新子系统**，
//! 不是本波能补的一根线。给它加一个返回空的 `list_tools` 只会让 `chimera tool list`
//! 看上去接好了而永远返回空——那是装饰性零生产者面，比不做更糟。
//!
//! 接好之后的改法（写在这里，免得下一个人在这里重新推一遍）：期望集合改成
//! `{echo, proc}` ∪ `{mcp 当且仅当 config.mcp.servers 中有可发现的 server}`。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::collections::BTreeSet;

use chimera_cli::config::ChimeraConfig;
use gqep_executor::tool_plane::ToolProvider;

/// 默认装配出的 provider 集合，恰为 `{echo, proc}`。
///
/// 用 `BTreeSet` 而不是顺序比较：注册顺序不是本判据的一部分，"在场集合"才是。
#[test]
fn default_assembly_exposes_echo_and_proc_only() {
    let runtime = chimera_cli::composition::build_tool_runtime(&ChimeraConfig::default())
        .expect("default assembly must build");
    let providers: BTreeSet<String> = runtime
        .registry()
        .descriptors()
        .iter()
        .map(|descriptor| descriptor.provider.clone())
        .collect();

    // W1 翻转（L2 方案 §7 W1-c / ADR-233 追加节六翻转条件）：fs 只读工具
    // 补齐进 tool_plane —— LLM 发布面与计划面共享同一 fs 工具集。
    assert_eq!(
        providers,
        BTreeSet::from(["echo".to_string(), "fs".to_string(), "proc".to_string()]),
        "默认装配的工具面组成变了 —— 若这是有意接入新 provider，请连同本测试与 \
         ADR-233 追加节六的翻转条件一起更新，而不是改期望值了事"
    );
}

/// MCP 工具名/描述/schema/副作用由**发现快照**单方面决定（ADR-235 S5）。
///
/// WHY 保留原断言口径：命名 `mcp:<server>/<tool>` 与"默认全 Write"是接入侧
/// （CLI 解析 / 审批面）依赖的两条事实；换后端（mesh 注册表 → `tools/list` 快照）
/// 不该改它们。旧夹具用 mesh 的 `capabilities: Vec<String>`；现在用真实发现的
/// `McpTool`，描述与 `inputSchema` 才有来源可断言。
#[tokio::test]
async fn mcp_descriptors_come_from_the_discovery_snapshot() {
    use mcp_mesh::protocol::{
        ClientConfig, DiscoveredServer, DiscoverySnapshot, DuplexTransport, McpClient, McpTool,
    };
    use std::sync::Arc;

    // 客户端只是"可调用"的类型占位：本用例只断言描述符，不产生任何调用。
    // 对端立刻丢弃 ⇒ 若将来有人误在 descriptors 里发请求，会立刻 Closed 而不是静默挂住。
    let (transport, peer) = DuplexTransport::pair(1024);
    drop(peer);
    let client = Arc::new(McpClient::spawn(transport, ClientConfig::default()));
    let snapshot = Arc::new(DiscoverySnapshot {
        servers: vec![DiscoveredServer {
            id: "filesrv".to_string(),
            tools: vec![McpTool {
                name: "read".to_string(),
                description: String::new(),
                input_schema: Some(serde_json::json!({"type": "object"})),
                annotations: None,
                extra: serde_json::Map::new(),
            }],
            trust_readonly_hints: false,
            client,
        }],
    });

    let provider = chimera_cli::tool_plane::McpToolProvider::new(snapshot);
    let descriptors = provider.descriptors();
    let names: Vec<String> = descriptors.iter().map(|d| d.name.clone()).collect();
    assert_eq!(names, vec!["mcp:filesrv/read".to_string()]);

    // 默认副作用：能力名不携带读写信息 ⇒ 一律 Write（fail-closed，ADR-235 D1）。
    let sides: Vec<bool> = descriptors
        .iter()
        .map(|d| format!("{:?}", d.side_effect) == "Write")
        .collect();
    assert_eq!(sides, vec![true], "MCP 工具默认必须是 Write（fail-closed）");

    // 描述兜底与 inputSchema 透传都来自快照（不是写死的模板串）。
    assert!(
        descriptors[0].description.contains("filesrv"),
        "空描述应回落成包含 server id 的兜底文案，实得 {}",
        descriptors[0].description
    );
    assert_eq!(
        descriptors[0].input_schema.as_deref(),
        Some(r#"{"type":"object"}"#),
        "inputSchema 必须原样透传（模型据此填参）"
    );
}

/// 同时钉住"开了信任才降级"的另一半（默认档的反向）。
#[tokio::test]
async fn trusting_readonly_hints_only_downgrades_non_destructive_tools() {
    use mcp_mesh::protocol::{
        ClientConfig, DiscoveredServer, DiscoverySnapshot, DuplexTransport, McpClient, McpTool,
        ToolAnnotations,
    };
    use std::sync::Arc;

    let (transport, peer) = DuplexTransport::pair(1024);
    drop(peer);
    let client = Arc::new(McpClient::spawn(transport, ClientConfig::default()));
    let tool = |name: &str, read_only: bool, destructive: bool| McpTool {
        name: name.to_string(),
        description: String::new(),
        input_schema: None,
        annotations: Some(ToolAnnotations {
            read_only_hint: Some(read_only),
            destructive_hint: Some(destructive),
            idempotent_hint: None,
            open_world_hint: None,
        }),
        extra: serde_json::Map::new(),
    };
    let snapshot = Arc::new(DiscoverySnapshot {
        servers: vec![DiscoveredServer {
            id: "trusted".to_string(),
            tools: vec![
                tool("safe_read", true, false),
                tool("sneaky_write", true, true),
            ],
            trust_readonly_hints: true,
            client,
        }],
    });
    let provider = chimera_cli::tool_plane::McpToolProvider::new(snapshot);
    let sides: Vec<(String, bool)> = provider
        .descriptors()
        .iter()
        .map(|d| (d.name.clone(), format!("{:?}", d.side_effect) == "ReadOnly"))
        .collect();
    assert_eq!(
        sides,
        vec![
            ("mcp:trusted/safe_read".to_string(), true),
            // readOnlyHint + destructiveHint 同时出现 ⇒ 按 Write 处理（fail-closed）。
            ("mcp:trusted/sneaky_write".to_string(), false),
        ],
        "开信任只降\"只读且非破坏\"的工具"
    );
}

/// mesh 的注册路径仍拒绝保留地址。
///
/// WHY 单列：这条守的是 2PC `register_server`（ADR-230），**不在** MCP provider 的
/// 调用链上；HTTP 传输与端点 SSRF 校验随该传输一起交付（ADR-235 D2）。把它与 MCP
/// 描述符断言分开，避免"改了 MCP 后端就以为 SSRF 也覆盖到了"。
#[test]
fn mesh_registration_still_rejects_reserved_endpoints() {
    let mesh = mcp_mesh::McpMesh::new(mcp_mesh::MeshConfig::default());
    let reserved = mcp_mesh::MeshServer::new(
        "internal".to_string(),
        "127.0.0.1:9".to_string(),
        vec!["read".to_string()],
    );
    let err = mesh
        .register_server(reserved)
        .expect_err("loopback endpoint must be rejected at register time");
    assert!(
        format!("{err:?}").contains("SsrfBlocked"),
        "期望 SsrfBlocked，实得 {err:?}"
    );
}
