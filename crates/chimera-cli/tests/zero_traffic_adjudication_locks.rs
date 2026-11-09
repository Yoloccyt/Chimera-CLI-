//! ADR-276 零流量面 / 结构性死臂的裁决锁（W10）
//!
//! # WHY 这个文件
//!
//! 三类「看起来像缺陷、裁决后是刻意」的形态需要一个可机检的锚点，
//! 否则下一轮审计会把它们重新提成「待修」：
//!
//! 1. `PermissionGate::evaluate` 只产 Allow/Unknown ⇒ `authorize` 的 `Deny` 臂
//!    生产不可达。**保留**：三态枚举完整性（删臂即把「未知」压成两态）；
//! 2. `McpMesh` 生产构造的 `event_bus` 恒 `None` ⇒ mesh 事件零流量。**保留**：
//!    命令面是一次性 ephemeral mesh，没有长驻订阅者，接总线等于虚构消费；
//! 3. 审计第 6 维 `budget_discipline` 恒 NEUTRAL。**保留**，但文案必须自陈
//!    「无载体」，不得让 NEUTRAL 被读成「预算纪律合格」。
//!
//! 判据分层（ADR-234）：行为断言（D1）+ 源码结构锁（D2/D3/D5）。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom
#![forbid(unsafe_code)]

use std::path::Path;

use chimera_cli::permission::{PermissionCtx, PermissionDecision, PermissionGate};

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("crates/<crate> 应有两级父目录")
}

fn read_src(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("读不到 {path:?}: {e}"))
}

/// ADR-276 D1：三态结构 + fail-closed 降级是唯一的生产 Deny 源。
#[test]
fn permission_deny_arm_is_structural() {
    // 行为层：evaluate 只有两态；authorize 在非交互下把 Unknown 降级为 Deny
    let ctx = PermissionCtx {
        yes: false,
        no_permission: false,
    };
    let non_interactive = PermissionGate::new(false);
    assert_eq!(
        non_interactive.evaluate(&ctx),
        PermissionDecision::Unknown,
        "evaluate 不得凭空产出 Deny（规则式拒绝属策略层）"
    );
    let auth = non_interactive.authorize(&ctx);
    assert_eq!(auth.decision, PermissionDecision::Deny, "fail-closed 降级");
    assert!(auth.downgraded, "必须标记降级来源");

    let interactive = PermissionGate::new(true);
    assert_eq!(
        interactive.authorize(&ctx).decision,
        PermissionDecision::Unknown,
        "交互环境下保持 Unknown（交给 confirm）"
    );

    let yes = PermissionCtx {
        yes: true,
        no_permission: false,
    };
    assert_eq!(
        interactive.authorize(&yes).decision,
        PermissionDecision::Allow
    );

    // 结构层：Deny 臂与裁决标记必须在场（删臂会让本断言红）
    let src = read_src("crates/chimera-cli/src/permission.rs");
    assert!(src.contains("ADR-276 D1"), "裁决必须就近登记");
    assert!(
        src.contains("PermissionDecision::Deny => PermissionAuthorization"),
        "authorize 的 Deny 臂不得删除（三态完整性）"
    );
}

/// ADR-276 D2：mesh 事件的 `event_bus` 恒 `None`，且裁决就近登记。
#[test]
fn mcp_mesh_event_bus_none_is_deliberate() {
    for rel in [
        "crates/chimera-cli/src/commands/mcp.rs",
        "crates/chimera-cli/src/mcp_tool_executor.rs",
    ] {
        let src = read_src(rel);
        assert!(src.contains("ADR-276 D2"), "{rel}: 缺裁决登记标记");
        let total = src.matches("with_participant_client(").count();
        let none_bus = src
            .matches("with_participant_client(MeshConfig::default(), None")
            .count();
        assert!(total > 0, "{rel}: 应存在 mesh 构造点");
        assert_eq!(
            none_bus, total,
            "{rel}: 所有 mesh 构造都必须传 event_bus=None（否则零流量面变成了半接）"
        );
    }
}

/// ADR-276 D5：第 6 维必须自陈「无载体」，不得只给 NEUTRAL 数字。
#[test]
fn auditor_sixth_dimension_is_labelled_no_carrier() {
    let src = read_src("crates/efficiency-monitor/src/auditor.rs");
    assert!(src.contains("ADR-276 D5"), "缺裁决登记标记");
    assert!(
        src.contains("无载体"),
        "必须显式写「无载体」—— NEUTRAL 单独出现会被读成「已评估且合格」"
    );
}
