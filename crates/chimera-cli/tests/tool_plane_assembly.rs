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

    assert_eq!(
        providers,
        BTreeSet::from(["echo".to_string(), "proc".to_string()]),
        "默认装配的工具面组成变了 —— 若这是有意接入新 provider，请连同本测试与 \
         ADR-233 追加节六的翻转条件一起更新，而不是改期望值了事"
    );
}

/// MCP 工具名的形状（`mcp:<server>/<capability>`）即使当前不在场也要钉住：
/// 它由 `McpToolProvider::descriptors` 单方面决定，接入后 CLI 侧解析依赖这个形状，
/// 而形状变了会让 `tool call mcp:...` 静默解析失败。
///
/// WHY 在 provider 缺席时仍能测：这里直接构造 provider（不经装配），断言的是
/// **命名规则**而不是**在场性**——两条判据不该绑在一个测试里，否则接入 MCP 后
/// 这条命名锁会跟着上一条一起被改掉。
#[test]
fn mcp_tool_names_are_prefixed_and_server_scoped() {
    let mesh = std::sync::Arc::new(mcp_mesh::McpMesh::new(mcp_mesh::MeshConfig::default()));
    // 端点必须是非保留地址：`register_server` 自己就 enforce SSRF（实测用
    // `127.0.0.1:9` 直接被 `SsrfBlocked` 拒），所以这里用 TEST-NET-3（203.0.113.0/24，
    // RFC 5737 文档专用段，永不路由到真实主机）。本例只断言命名与副作用语义，
    // 不产生任何流量。
    mesh.register_server(mcp_mesh::MeshServer::new(
        "filesrv".to_string(),
        "203.0.113.7:4430".to_string(),
        vec!["read".to_string()],
    ))
    .expect("register a non-reserved endpoint");

    let provider = chimera_cli::tool_plane::McpToolProvider::new(std::sync::Arc::clone(&mesh));
    let names: Vec<String> = provider
        .descriptors()
        .iter()
        .map(|d| d.name.clone())
        .collect();

    assert_eq!(names, vec!["mcp:filesrv/read".to_string()]);
    // 副作用语义：能力名不携带读写信息 ⇒ 一律按 Write（fail-closed 方向，见 mcp.rs 头注）。
    // 真实发现（readOnlyHint）落地后这条要改成"显式只读才降级"。
    let sides: Vec<bool> = provider
        .descriptors()
        .iter()
        .map(|d| format!("{:?}", d.side_effect) == "Write")
        .collect();
    assert_eq!(sides, vec![true], "MCP 能力默认必须是 Write（fail-closed）");

    // 同一条注册路径必须拒绝保留地址：本波接不了真实发现，但"将来接的时候不能把
    // 内网端点放进来"这条界现在就钉住（真实发现会遍历 config.mcp.servers 逐个注册，
    // 少这道界就等于把 SSRF 的守门推给尚未存在的代码）。
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
