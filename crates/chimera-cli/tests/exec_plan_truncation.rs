//! W2 / ADR-268 — 截断可见化与 `--plan-strict` 的加性契约（CLI 层）。
//!
//! # 要锁的两条
//! 1. **默认档零变化**：计划被截断时，未开 `--plan-strict` 仍返回 `Ok` ——
//!    退出码 0、`status` 仍是 `"ok"`，与改动前逐字一致。
//! 2. **显式严格档才收紧**：`--plan-strict` 开启且计划确实截断 ⇒ 返回错误，
//!    经既有 `IoError→4` 稳定映射退出码 4，且消息携带**截断原因**。
//!
//! # 为什么这对测试必须成对
//! 若只测第 1 条，一个「计划压根没被截断」的实现也会通过（严格档同样 Ok）——
//! 那就是空转。第 2 条同时证明**该计划确实被截断**，第 1 条才有意义。
//!
//! 运行：`cargo test -p chimera-cli --test exec_plan_truncation`

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use chimera_cli::commands::exec;
use chimera_cli::composition::build;
use chimera_cli::exec_plan::PlanGuardOverrides;
use chimera_cli::ChimeraConfig;

/// 造一个**必然触发回填预算截断**的计划：n1 的 args_json 是一条 70 KiB 的 JSON 字符串，
/// 其回显结果超过 `PlanGuards::default().budget_bytes`（64 KiB）；
/// n2 依赖 n1 ⇒ 预算在层 1 后触发熔断 ⇒ n2 从未执行（`unexecuted_nodes == 1`）。
///
/// WHY 不用 fs.read 造大输出：CLI 的 `fs.read` 以进程 CWD 解析相对路径，
/// 在测试里造文件会污染 crate 目录；`text.echo` 的回显大小由参数直接决定，零 IO。
fn write_truncating_plan(dir: &Path) -> PathBuf {
    let big_args = format!("\"{}\"", "x".repeat(70 * 1024));
    let args_json_literal = serde_json::to_string(&big_args).expect("序列化 args 应成功");
    let plan_json = format!(
        r#"{{"id":"trunc-plan","nodes":[{{"id":"n1","op":"tool_call","tool_name":"text.echo","args_json":{args_json_literal},"side_effect":"read_only"}},{{"id":"n2","op":"map","field":"len"}}],"edges":[{{"from":"n1","to":"n2"}}]}}"#
    );
    let path = dir.join("trunc.plan.json");
    std::fs::write(&path, plan_json).expect("写计划文件应成功");
    path
}

fn ctx() -> chimera_cli::composition::AppContext {
    build(&ChimeraConfig::default()).expect("组合根装配应成功")
}

#[tokio::test]
async fn truncated_plan_default_off_still_ok() {
    let dir = tempfile::tempdir().expect("tempdir 应成功");
    let plan = write_truncating_plan(dir.path());
    let ctx = ctx();
    // 默认档：strict = false
    let r = exec::execute_plan_with_timeouts(
        &ctx,
        &plan,
        4,
        true,
        None,
        None,
        false,
        PlanGuardOverrides::default(),
    )
    .await;
    // 注意：断言消息里的表达式会被**先求值**，故不能在这里写 expect_err
    // （Ok 时它会先 panic，掩盖真正的失败原因）。
    if let Err(e) = &r {
        panic!("默认档下截断不得改变退出行为（加性优先，ADR-268），实得: {e}");
    }
}

#[tokio::test]
async fn truncated_plan_strict_on_yields_exit_4_with_reason() {
    let dir = tempfile::tempdir().expect("tempdir 应成功");
    let plan = write_truncating_plan(dir.path());
    let ctx = ctx();
    let err = exec::execute_plan_with_timeouts(
        &ctx,
        &plan,
        4,
        true,
        None,
        None,
        true,
        PlanGuardOverrides::default(),
    )
    .await
    .expect_err("--plan-strict 下截断必须失败");
    let msg = err.to_string();
    assert!(msg.contains("truncated"), "消息须显式说明截断，实得: {msg}");
    assert!(
        msg.contains("budget_bytes"),
        "消息须携带结构化截断原因（budget_bytes），实得: {msg}"
    );
    assert!(
        msg.contains("1 node(s) not executed"),
        "须报告未执行节点数（n2 因预算熔断未执行），实得: {msg}"
    );
    let ce = err
        .downcast_ref::<chimera_cli::ChimeraCliError>()
        .expect("截断应携带 ChimeraCliError（IoError 承载）");
    assert_eq!(
        exec::exec_exit_code(ce),
        4,
        "截断走既有 IoError→4 稳定映射，不新增退出码"
    );
}

#[tokio::test]
async fn non_truncated_plan_is_ok_under_strict() {
    // 反向对照：严格档只对**截断**收紧，正常计划不受影响。
    let dir = tempfile::tempdir().expect("tempdir 应成功");
    let plan_json = r#"{"id":"small","nodes":[{"id":"n1","op":"tool_call","tool_name":"text.echo","args_json":"{\"ok\":true}","side_effect":"read_only"}],"edges":[]}"#;
    let path = dir.path().join("small.plan.json");
    std::fs::write(&path, plan_json).expect("写计划文件应成功");
    let ctx = ctx();
    let r = exec::execute_plan_with_timeouts(
        &ctx,
        &path,
        4,
        true,
        None,
        None,
        true,
        PlanGuardOverrides::default(),
    )
    .await;
    assert!(r.is_ok(), "未截断的计划在严格档下必须照常成功");
}
