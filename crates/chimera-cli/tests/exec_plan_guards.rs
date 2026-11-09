//! W3 / ADR-269 — `PlanGuards` 三个 CLI 覆写入口（只读投影 / 步数 / 回填预算）。
//!
//! # 要锁的两件事
//! 1. **覆写真的生效**：只读投影能拒写节点、步数能封顶、预算能收紧。
//! 2. **缺省逐字不变**：不传覆写时行为与改造前完全一致（否则 W2 的加性契约被破坏）。
//!
//! WHY 用 `PlanGuardOverrides` 直连而不是走 clap 解析：本文件的判据是**守卫语义**，
//! 不是 CLI 解析（后者由 `cli_contract_snapshot` 与 clap 自身保证）。直接构造覆写
//! 让失败原因单一可归因。
//!
//! 运行：`cargo test -p chimera-cli --test exec_plan_guards`

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use chimera_cli::commands::exec;
use chimera_cli::composition::build;
use chimera_cli::exec_plan::PlanGuardOverrides;
use chimera_cli::ChimeraCliError;
use chimera_cli::ChimeraConfig;

fn write_plan(dir: &Path, name: &str, json: &str) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, json).expect("写计划文件应成功");
    path
}

fn ctx() -> chimera_cli::composition::AppContext {
    build(&ChimeraConfig::default()).expect("组合根装配应成功")
}

/// 含一个显式 Write 节点的计划（`text.echo` 本身无副作用，声明才是判据来源）。
fn write_node_plan() -> &'static str {
    r#"{"id":"w","nodes":[{"id":"n1","op":"tool_call","tool_name":"text.echo","args_json":"{\"a\":1}","side_effect":"write"}],"edges":[]}"#
}

#[tokio::test]
async fn plan_readonly_flag_rejects_write_node() {
    let dir = tempfile::tempdir().expect("tempdir 应成功");
    let plan = write_plan(dir.path(), "w.plan.json", write_node_plan());
    let ctx = ctx();
    let err = exec::execute_plan_with_timeouts(
        &ctx,
        &plan,
        4,
        true,
        None,
        None,
        false,
        PlanGuardOverrides {
            readonly: true,
            ..Default::default()
        },
    )
    .await
    .expect_err("只读投影必须拒绝 Write 节点");
    let ce = err
        .downcast_ref::<ChimeraCliError>()
        .expect("应为 ChimeraCliError");
    assert_eq!(
        exec::exec_exit_code(ce),
        3,
        "计划校验类失败归 EngineError→3，不进 4（不是工具失败）"
    );
}

/// 反向对照：**不传**只读覆写时，同一个计划照常执行 —— 证明上一条的红来自覆写本身，
/// 而不是该计划天生不可执行。
#[tokio::test]
async fn plan_readonly_default_off_unchanged() {
    let dir = tempfile::tempdir().expect("tempdir 应成功");
    let plan = write_plan(dir.path(), "w2.plan.json", write_node_plan());
    let ctx = ctx();
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
    if let Err(e) = r {
        panic!("缺省覆写下该计划应可执行（只读投影未开启），实得: {e}");
    }
}

#[tokio::test]
async fn plan_max_steps_flag_caps() {
    // 3 个节点成链；覆写封顶 2 步 ⇒ 校验期即拒（PlanError → EngineError → 3）
    let plan_json = r#"{"id":"chain","nodes":[{"id":"a","op":"tool_call","tool_name":"text.echo","args_json":"{}","side_effect":"read_only"},{"id":"b","op":"map","field":"x"},{"id":"c","op":"map","field":"x"}],"edges":[{"from":"a","to":"b"},{"from":"b","to":"c"}]}"#;
    let dir = tempfile::tempdir().expect("tempdir 应成功");
    let plan = write_plan(dir.path(), "chain.plan.json", plan_json);
    let ctx = ctx();
    let err = exec::execute_plan_with_timeouts(
        &ctx,
        &plan,
        4,
        true,
        None,
        None,
        false,
        PlanGuardOverrides {
            max_steps: Some(2),
            ..Default::default()
        },
    )
    .await
    .expect_err("步数覆写必须生效");
    let ce = err
        .downcast_ref::<ChimeraCliError>()
        .expect("应为 ChimeraCliError");
    assert_eq!(exec::exec_exit_code(ce), 3);
}

/// 预算覆写生效：与 W2 同一「70 KiB 回显」计划，但把预算收到 1 KiB ⇒ 仍截断；
/// 再叠加 `--plan-strict` ⇒ 退出码 4 且原因归 `budget_bytes`。
///
/// WHY 与 W2 的用例区分：W2 证明「默认预算会截断」，本用例证明「CLI 能把预算收得更紧」——
/// 这是 F-08（覆写入口存在）的可执行证据，而不是同一条断言的重复。
#[tokio::test]
async fn plan_budget_bytes_flag_tightens() {
    let big_args = format!("\"{}\"", "x".repeat(70 * 1024));
    let args_json_literal = serde_json::to_string(&big_args).expect("序列化 args 应成功");
    let plan_json = format!(
        r#"{{"id":"tight","nodes":[{{"id":"n1","op":"tool_call","tool_name":"text.echo","args_json":{args_json_literal},"side_effect":"read_only"}}],"edges":[]}}"#
    );
    let dir = tempfile::tempdir().expect("tempdir 应成功");
    let plan = write_plan(dir.path(), "tight.plan.json", &plan_json);
    let ctx = ctx();
    let err = exec::execute_plan_with_timeouts(
        &ctx,
        &plan,
        4,
        true,
        None,
        None,
        true,
        PlanGuardOverrides {
            budget_bytes: Some(1024),
            ..Default::default()
        },
    )
    .await
    .expect_err("收紧后的预算必然被 70KiB 回显突破");
    let msg = err.to_string();
    assert!(
        msg.contains("budget_bytes"),
        "收紧预算触发的截断应归因 budget_bytes，实得: {msg}"
    );
    let ce = err
        .downcast_ref::<ChimeraCliError>()
        .expect("应为 ChimeraCliError");
    assert_eq!(exec::exec_exit_code(ce), 4);
}
