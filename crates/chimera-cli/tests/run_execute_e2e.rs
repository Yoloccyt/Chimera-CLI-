//! `run --execute` 闭环 + `execute_batch` 首个生产调用方验收（F-02/F-13/U-13/U-17，ADR-247）
//!
//! # WHY 需要这个文件
//!
//! ADR-244 时代 `run --execute` 在**两处**独立分支（wrapper 与 `execute_with_ctx`）
//! 恒返回 `NotImplemented`（E-1），`ToolRuntime::execute_batch` 在生产路径**零调用**
//! （F-13/U-13——4.4× 并行加速只在 bench 成立）。ADR-247 闭环：
//!
//! 1. **授权前置（D1 fail-closed）**：`PermissionGate` 三态裁决 —— 非交互
//!    `Unknown → Deny → 稳定前缀拒绝`（退出码 2、零副作用、不读 stdin，脚本
//!    grep 契约不变）；交互 `Allow`（`--yes`/prompt 确认）才进入执行。
//!    授权点**唯一**（`execute_with_ctx` 内、`create_quest` 之前）—— E-1 的
//!    「两处分支半闭环」风险由构造消除：wrapper 不再自带拒绝分支。
//! 2. **批量执行（F-13）**：Quest 分解结果映射为 `text.echo` 批量
//!    （D3：零新 CLI 参数），经组合根唯一 `ctx.tool_runtime.execute_batch`
//!    有界并发派发 —— 结果输入序 + 四段守恒（gqep 既有 property 锁定）。
//! 3. **退出码归因（ADR-060 单一真值源）**：全成功 0 / 策略或沙箱拒绝
//!    `PermissionDenied`=5 / 用户取消 4 / deadline 超时 6 / 其余传输失败
//!    `EngineError`=3。**不新造退出码**。
//!
//! 权限门经参数注入（`PermissionGate::new(interactive)`）——不依赖进程 tty，
//! 测试在本机交互终端与 CI 上行为一致。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use chimera_cli::commands::run::{
    batch_outcome_to_cli_error, build_batch_requests, execute, execute_quest_tool_batch,
    execute_with_ctx, EXECUTE_REFUSAL_MSG,
};
use chimera_cli::composition::build;
use chimera_cli::error::ChimeraCliError;
use chimera_cli::permission::{PermissionCtx, PermissionGate};
use chimera_cli::ChimeraConfig;
use event_bus::NexusEvent;
use gqep_executor::tool_runtime::{
    BatchOutcome, DispatchError, DispatchOutcome, BATCH_DEADLINE_PREFIX,
};
use nexus_core::{Quest, Task, TaskStatus};
use proptest::prelude::*;
use std::time::Duration;

/// 合成 n 个任务的 Quest（D3：任务描述 → text.echo 回显内容）。
fn quest_with_tasks(descriptions: &[&str]) -> Quest {
    Quest {
        quest_id: "quest-w2e2e".to_string(),
        title: "W2 E2E".to_string(),
        tasks: descriptions
            .iter()
            .enumerate()
            .map(|(i, d)| Task {
                task_id: format!("task-{i}"),
                description: (*d).to_string(),
                status: TaskStatus::Pending,
                dependencies: vec![],
            })
            .collect(),
        thinking_mode: nexus_core::ThinkingMode::Standard,
        checkpoint_id: None,
        priority: 128,
    }
}

fn silent_perm() -> PermissionCtx {
    PermissionCtx::default()
}

// ============================================================
// 1. 授权前置（D1 fail-closed）
// ============================================================

/// 非交互 + 未授权：恒 `NotImplemented`（退出码 2、稳定前缀），
/// 且 fail-closed 降级计数器真实递增（L2 可观测）。
#[tokio::test]
async fn execute_without_authorization_still_refuses() {
    let ctx = build(&ChimeraConfig::default()).expect("装配应成功");
    let before = chimera_cli::permission::fail_closed_downgrades();
    let gate = PermissionGate::new(false);
    let result = execute_with_ctx(&ctx, "执行工具链", false, &gate, &silent_perm(), true).await;
    let err = result.expect_err("非交互未授权必须拒绝");
    let cli_err = err
        .downcast_ref::<ChimeraCliError>()
        .expect("拒绝错误应为 ChimeraCliError");
    assert_eq!(cli_err.exit_code_value(), 2, "拒绝退出码必须是 2");
    assert!(matches!(cli_err, ChimeraCliError::NotImplemented(_)));
    assert!(
        EXECUTE_REFUSAL_MSG.starts_with("execute_not_enabled:"),
        "稳定前缀必须保留（脚本 grep 契约）"
    );
    assert!(
        chimera_cli::permission::fail_closed_downgrades() > before,
        "非交互 Unknown → Deny 必须计入 fail-closed 降级计数器"
    );
}

/// 拒绝必须发生在 `create_quest` 之前 —— 共享 bus 上零 Quest 事件（既有契约）。
#[tokio::test]
async fn refusal_precedes_quest_side_effects() {
    let ctx = build(&ChimeraConfig::default()).expect("装配应成功");
    let mut rx = ctx.bus.subscribe();
    let gate = PermissionGate::new(false);
    let result =
        execute_with_ctx(&ctx, "执行高风险工具链", false, &gate, &silent_perm(), true).await;
    assert!(result.is_err(), "非交互 --execute 应被拒绝");
    let mut saw_quest_event = false;
    for _ in 0..20 {
        match tokio::time::timeout(Duration::from_millis(10), rx.recv()).await {
            Ok(Ok(NexusEvent::QuestCreated { .. })) => {
                saw_quest_event = true;
                break;
            }
            Ok(Ok(_)) => continue,
            _ => break,
        }
    }
    assert!(
        !saw_quest_event,
        "拒绝须发生在 create_quest 之前，不得产生 Quest 事件"
    );
}

/// 交互 + 显式 Allow（`--yes`）：真实进入批量执行路径（E-1 的另一分支）。
///
/// WHY 用 `PermissionGate::new(true)` + `yes:true`：不依赖进程 tty 与 stdin，
/// 断言「Allow 才执行」这一授权语义本身；真实批量断言在 t5 直调批量函数完成。
#[tokio::test]
async fn execute_with_explicit_allow_enters_batch_path() {
    let ctx = build(&ChimeraConfig::default()).expect("装配应成功");
    let gate = PermissionGate::new(true);
    let perm = PermissionCtx {
        yes: true,
        no_permission: false,
    };
    // create_quest 对简单 prompt 可能分解出 0 个任务 —— 空批量按契约成功
    // （无请求 ⇒ 无结果 ⇒ 退出码 0）。本断言锁定「Allow 放行到执行」，
    // 批量内容断言由 t5/t6 直调批量函数承担。
    execute_with_ctx(&ctx, "hello world", false, &gate, &perm, true)
        .await
        .expect("交互 Allow 路径应真实执行（空分解为合法成功）");
}

/// wrapper 路径同样被授权前置覆盖（E-1：两处入口都要闭环，由唯一授权点构造保证）。
#[tokio::test]
async fn wrapper_execute_refuses_without_authorization() {
    let config = ChimeraConfig::default();
    let gate = PermissionGate::new(false);
    let result = execute("执行工具链", &config, false, &gate, &silent_perm(), true).await;
    let err = result.expect_err("wrapper 路径的非交互 --execute 同样必须拒绝");
    let cli_err = err.downcast_ref::<ChimeraCliError>().unwrap();
    assert_eq!(cli_err.exit_code_value(), 2);
    assert!(matches!(cli_err, ChimeraCliError::NotImplemented(_)));
}

// ============================================================
// 2. 批量执行（F-13：execute_batch 首个生产调用方）
// ============================================================

/// D3 映射：Quest.tasks → `text.echo` 批量，输入序保持、逐条可解析。
#[test]
fn build_batch_requests_maps_tasks_in_order() {
    let quest = quest_with_tasks(&["alpha", "beta", "gamma"]);
    let requests = build_batch_requests(&quest);
    assert_eq!(requests.len(), 3, "请求与任务一一对应");
    for (req, desc) in requests.iter().zip(["alpha", "beta", "gamma"]) {
        assert_eq!(req.tool_name, "text.echo", "工具来源固定 text.echo（D3）");
        assert!(
            req.args_json.contains(desc),
            "args_json 必须携带任务描述，实际: {}",
            req.args_json
        );
    }
}

/// 空分解（0 任务）是合法成功：空批量零派发、退出码 0。
#[tokio::test]
async fn empty_quest_batch_is_vacuous_success() {
    let ctx = build(&ChimeraConfig::default()).expect("装配应成功");
    let quest = quest_with_tasks(&[]);
    let outcome = execute_quest_tool_batch(&ctx, &quest)
        .await
        .expect("空批量应成功");
    assert_eq!(outcome.results.len(), 0);
    assert!(outcome.is_conserved());
    assert!(
        batch_outcome_to_cli_error(&outcome).is_ok(),
        "空批量退出码 0"
    );
}

/// 真实批量执行：3 个任务经 `ctx.tool_runtime.execute_batch` 回显，
/// 结果输入序、四段守恒、全部成功 —— F-13 的首个生产调用方实证。
#[tokio::test]
async fn quest_tools_execute_via_batch_in_input_order() {
    let ctx = build(&ChimeraConfig::default()).expect("装配应成功");
    let quest = quest_with_tasks(&["任务一", "任务二", "任务三"]);
    let outcome = execute_quest_tool_batch(&ctx, &quest)
        .await
        .expect("批量执行应成功");
    assert_eq!(outcome.results.len(), 3);
    assert!(outcome.is_conserved(), "四段守恒必须成立");
    assert_eq!(outcome.succeeded, 3, "只读工具应全部成功");
    for (i, res) in outcome.results.iter().enumerate() {
        let out = res
            .as_ref()
            .unwrap_or_else(|e| panic!("第 {i} 条应成功: {e:?}"));
        let expected = ["任务一", "任务二", "任务三"][i];
        assert!(
            out.content_json.contains(expected),
            "结果必须输入序回显（results[{i}] 应含 {expected}），实际: {}",
            out.content_json
        );
    }
    assert!(
        batch_outcome_to_cli_error(&outcome).is_ok(),
        "全成功退出码 0"
    );
}

/// 沙箱拒绝经批量如实归因：注入形态描述在沙箱环被拦，
/// 批量本身 Ok（逐请求结果承载失败），退出码归因 PermissionDenied。
#[tokio::test]
async fn sandbox_denial_surfaces_through_batch() {
    let ctx = build(&ChimeraConfig::default()).expect("装配应成功");
    let quest = quest_with_tasks(&["$(rm -rf /)", "safe"]);
    let outcome = execute_quest_tool_batch(&ctx, &quest)
        .await
        .expect("批量本身应完成（失败在逐请求结果中如实承载）");
    assert_eq!(outcome.succeeded, 1, "safe 任务应成功");
    assert_eq!(outcome.failed, 1, "注入形态应被沙箱拦截");
    let err = batch_outcome_to_cli_error(&outcome).expect_err("存在失败必须非零退出");
    assert_eq!(err.exit_code_value(), 5, "沙箱拒绝归因 PermissionDenied");
    assert!(matches!(err, ChimeraCliError::PermissionDenied(_)));
}

// ============================================================
// 3. 退出码归因纯函数（ADR-060 单一真值源，不新造退出码）
// ============================================================

/// 合成 BatchOutcome（results + 四段计数自洽，`is_conserved` 成立）。
fn outcome_with(results: Vec<Result<DispatchOutcome, DispatchError>>) -> BatchOutcome {
    let succeeded = results.iter().filter(|r| r.is_ok()).count();
    let failed = results.len() - succeeded;
    BatchOutcome {
        results,
        succeeded,
        failed,
        abandoned: 0,
        not_started: 0,
    }
}

fn ok_outcome(content: &str) -> Result<DispatchOutcome, DispatchError> {
    Ok(DispatchOutcome {
        content_json: content.to_string(),
    })
}

#[test]
fn policy_denial_maps_to_permission_denied() {
    let outcome = outcome_with(vec![
        Err(DispatchError::Policy("denied: execpolicy".to_string())),
        ok_outcome("{}"),
    ]);
    let err = batch_outcome_to_cli_error(&outcome).expect_err("存在 Policy 拒绝必须非零");
    assert_eq!(err.exit_code_value(), 5, "策略拒绝 → PermissionDenied");
}

#[test]
fn route_and_generic_transport_map_to_engine_error() {
    let outcome = outcome_with(vec![Err(DispatchError::Route(
        "unknown builtin tool".to_string(),
    ))]);
    let err = batch_outcome_to_cli_error(&outcome).expect_err("Route 失败必须非零");
    assert_eq!(err.exit_code_value(), 3, "路由失败 → EngineError");

    let outcome = outcome_with(vec![Err(DispatchError::Transport(
        "unknown builtin tool".to_string(),
    ))]);
    let err = batch_outcome_to_cli_error(&outcome).expect_err("传输失败必须非零");
    assert_eq!(
        err.exit_code_value(),
        3,
        "非 deadline 传输失败 → EngineError"
    );
}

#[test]
fn batch_deadline_maps_to_timeout() {
    let outcome = outcome_with(vec![Err(DispatchError::Transport(format!(
        "{BATCH_DEADLINE_PREFIX}: 120s"
    )))]);
    let err = batch_outcome_to_cli_error(&outcome).expect_err("deadline 超时必须非零");
    assert_eq!(err.exit_code_value(), 6, "整批 deadline → Timeout");
    assert!(matches!(err, ChimeraCliError::Timeout(_)));
}

#[test]
fn sandbox_error_maps_to_permission_denied() {
    let outcome = outcome_with(vec![Err(DispatchError::Sandbox(
        "sandbox: injection blocked".to_string(),
    ))]);
    let err = batch_outcome_to_cli_error(&outcome).expect_err("沙箱拒绝必须非零");
    assert_eq!(err.exit_code_value(), 5, "沙箱拒绝 → PermissionDenied");
}

// ============================================================
// 4. proptest：D3 映射不变量
// ============================================================

proptest::proptest! {
    /// 任意任务描述序列：请求与任务一一对应、工具名恒 text.echo、
    /// args_json 恒为合法 JSON 且携带描述（D3 契约对任意输入成立）。
    #[test]
    fn prop_batch_requests_mirror_tasks(descriptions in proptest::collection::vec(".*", 0..24)) {
        let quest = quest_with_tasks(
            &descriptions.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        let requests = build_batch_requests(&quest);
        prop_assert_eq!(requests.len(), descriptions.len());
        for (req, desc) in requests.iter().zip(&descriptions) {
            prop_assert_eq!(req.tool_name.clone(), "text.echo".to_string());
            let parsed: serde_json::Value = serde_json::from_str(&req.args_json)
                .map_err(|e| proptest::test_runner::TestCaseError::fail(format!("bad json: {e}")))?;
            prop_assert_eq!(
                parsed.get("text").cloned().unwrap_or(serde_json::Value::Null),
                serde_json::Value::String(desc.clone())
            );
        }
    }
}
