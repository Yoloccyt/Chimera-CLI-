//! `side_effect_for` 的穷举表（ADR-235 D1）。
//!
//! WHY 逐格测而不是只测"默认 Write"：降级只在两个条件同时成立时发生，
//! 少测一格（比如 `destructive=true` 或"没注解"）就等于允许某类写工具绕过审批。
//! 四类输入 × 两档信任 = 下面七格，是这张小真值表的全部边界。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production
#![forbid(unsafe_code)]

use mcp_mesh::protocol::{side_effect_for, McpTool, ToolAnnotations};
use nexus_contracts::tool_plan::SideEffectDecl;
use serde_json::Map;

fn tool(read_only: Option<bool>, destructive: Option<bool>) -> McpTool {
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
        name: "t".to_string(),
        description: String::new(),
        input_schema: None,
        annotations,
        extra: Map::new(),
    }
}

#[test]
fn untrusted_server_is_always_write() {
    for (read_only, destructive) in [
        (Some(true), Some(false)),
        (Some(true), None),
        (Some(false), Some(false)),
        (None, None),
    ] {
        assert_eq!(
            side_effect_for(&tool(read_only, destructive), false),
            SideEffectDecl::Write,
            "未开信任时 read_only={read_only:?} destructive={destructive:?} 必须仍是 Write"
        );
    }
}

#[test]
fn trusted_server_downgrades_only_readonly_and_non_destructive() {
    assert_eq!(
        side_effect_for(&tool(Some(true), Some(false)), true),
        SideEffectDecl::ReadOnly,
        "只读且明示非破坏 ⇒ 可降级"
    );
    assert_eq!(
        side_effect_for(&tool(Some(true), None), true),
        SideEffectDecl::ReadOnly,
        "未声明 destructive 视为非破坏"
    );
}

#[test]
fn trusted_server_still_keeps_write_for_the_dangerous_cells() {
    for (read_only, destructive, why) in [
        (Some(true), Some(true), "同时声明只读与破坏 ⇒ 按破坏处理"),
        (Some(false), Some(false), "只读声明为假"),
        (None, None, "无注解 ⇒ fail-closed"),
        (None, Some(false), "只缺 readOnlyHint ⇒ fail-closed"),
    ] {
        assert_eq!(
            side_effect_for(&tool(read_only, destructive), true),
            SideEffectDecl::Write,
            "{why}（read_only={read_only:?} destructive={destructive:?}）"
        );
    }
}
