//! `mcp query` 命令面 — 超位置查询转正（WP-H / ADR-284，R7 批次）
//!
//! # WHY 这个文件
//!
//! `superposition_query` 自实现以来是工单 T2 登记的结构死臂（零生产调用方）。
//! 本文件锁定转正后的三件事：
//!
//! 1. **分类锁**：`mcp query` 归 `NoContext`（只读查询零装配税，与
//!    `list/inspect` 同档）；
//! 2. **生产路径锁**：命令走 `McpMesh::superposition_query`（含事件发布与
//!    fanout 并发上界），不得绕行直调自由函数；
//! 3. **行为**：聚合输出成功（已注册 server 扇出）、未注册 server 前置拒绝
//!    （`EngineError`）、空 `--servers` 参数拒绝。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom
#![forbid(unsafe_code)]

use chimera_cli::cli::{Commands, McpAction};
use chimera_cli::commands::mcp;
use chimera_cli::config::{ChimeraConfig, McpServerConfig};
use chimera_cli::permission::PermissionCtx;
use chimera_cli::spec::{classify, CommandClass};

fn config_with_servers() -> ChimeraConfig {
    let mut config = ChimeraConfig::default();
    config.mcp.servers = vec![
        McpServerConfig {
            id: "s-a".into(),
            url: Some("http://203.0.113.10:8001".into()),
        },
        McpServerConfig {
            id: "s-b".into(),
            url: Some("http://203.0.113.10:8002".into()),
        },
    ];
    config
}

fn query_action(servers: &str) -> Commands {
    Commands::Mcp {
        action: McpAction::Query {
            query: "probe".into(),
            servers: servers.split(',').map(str::to_string).collect(),
            deadline_ms: 500,
            network: false,
        },
    }
}

/// 分类锁：`mcp query` 必须归 NoContext（只读查询零装配税）
///
/// 与 `mcp_command_admission.rs` 的 "list/inspect 仍 NoContext" 同一防线——
/// 转正不得悄悄把新命令拖进 FullContext 装配面。
#[test]
fn query_is_classified_no_context() {
    assert_eq!(
        classify(Some(&query_action("s-a,s-b"))),
        CommandClass::NoContext
    );
}

/// 生产路径锁：query 实现必须走 `McpMesh::superposition_query`
/// （事件发布 + fanout 上界齐全的生产入口），不得绕行直调自由函数。
#[test]
fn query_uses_mesh_production_path() {
    let src = include_str!("../src/commands/mcp.rs");
    let prod = src.split("#[cfg(test)]").next().unwrap_or("");
    assert!(
        prod.contains("mesh.superposition_query(query)"),
        "mcp query 必须走 McpMesh::superposition_query 生产路径（ADR-284）"
    );
    assert!(
        !prod.contains("execute_superposition_query("),
        "命令面不得绕过 mesh 直调自由函数（丢事件发布与并发上界）"
    );
}

/// 行为：已注册 server 扇出聚合成功（InProcess 客户端，零网络副作用）
#[tokio::test]
async fn query_aggregates_registered_servers() {
    let config = config_with_servers();
    let perm = PermissionCtx::default();
    let action = McpAction::Query {
        query: "probe".into(),
        servers: vec!["s-a".into(), "s-b".into()],
        deadline_ms: 500,
        network: false,
    };
    let result = mcp::execute(&action, &config, false, &perm, false, None).await;
    assert!(result.is_ok(), "聚合查询必须成功: {result:?}");
}

/// 行为：未注册 server → 前置校验整体拒绝（EngineError，退出码 3）
#[tokio::test]
async fn query_rejects_unregistered_server() {
    let config = config_with_servers();
    let perm = PermissionCtx::default();
    let action = McpAction::Query {
        query: "probe".into(),
        servers: vec!["s-a".into(), "unknown".into()],
        deadline_ms: 500,
        network: false,
    };
    let err = mcp::execute(&action, &config, false, &perm, false, None)
        .await
        .expect_err("未注册 server 必须拒绝");
    assert!(
        err.to_string().contains("未找到") || err.to_string().contains("not registered"),
        "错误应可归因到服务器缺失，实际: {err}"
    );
}

/// 行为：空 --servers → 参数拒绝（EngineError）
#[tokio::test]
async fn query_rejects_empty_servers() {
    let config = config_with_servers();
    let perm = PermissionCtx::default();
    let action = McpAction::Query {
        query: "probe".into(),
        servers: vec![],
        deadline_ms: 500,
        network: false,
    };
    let err = mcp::execute(&action, &config, false, &perm, false, None)
        .await
        .expect_err("空 servers 必须拒绝");
    assert!(
        err.to_string().contains("--servers"),
        "错误应指引 --servers 用法，实际: {err}"
    );
}
