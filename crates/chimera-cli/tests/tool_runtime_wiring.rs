//! 工具运行时四环链生产接线验收（ADR-240 W2-T1 + ADR-244 D1/D2）
//!
//! # WHY 需要这个文件
//!
//! ADR-238 建立了强制沙箱派发链，但其验收证据全部来自 `gqep-executor` 的
//! 单元测试与 proptest。**生产路径无调用点** —— `git grep "ToolRuntime"` 在
//! ADR-238 落地时只命中 `lib.rs` 的 re-export。
//!
//! 这意味着「四环齐备、缺环即构造失败」这条安全承诺**从未在生产装配中被
//! 验证过**：组合根不装它，命令层也没有替代物 ⇒ 工具执行路径实际上
//! 既无策略层也无沙箱层。
//!
//! 本文件从 crate **外部边界**验证组合根确实装配了四环链，并验证：
//! 1. 装配成功（缺环会构造失败 —— 这是 ADR-238 D2 的核心不变量）；
//! 2. 沙箱层真实生效（注入形态被拦截）—— 这是接线**新增**的防护；
//! 3. **ADR-244 D2 迁移后的 fail-closed 契约**：组合根装配的是真实
//!    `BuiltinToolExecutor`（不是 `UnwiredToolExecutor`），因此
//!    - 已知内置工具（`text.echo`）必须经四环链真实返回 `Ok`；
//!    - 未知工具（`repo.search`）必须 `Err(DispatchError::Transport)` 且
//!      明确指出 `unknown builtin tool`，**绝不**返回 `Ok`。
//!
//! # 契约迁移说明（ADR-244 D2）
//!
//! 原断言（`not wired`）防的是**「以伪造成功掩盖传输环缺失」**这一类回归。
//! 收敛后传输环已真实装配，同类回归的新守门方式是：未知工具名必须被传输环
//! 如实拒绝（`unknown builtin tool`）—— 若有人用「统统返回 Ok 的假执行器」
//! 替换 delegate，本测试立即变红。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use chimera_cli::composition::{build, tool_runtime_policy};
use chimera_cli::ChimeraConfig;
use gqep_executor::tool_runtime::{DispatchError, DispatchRequest};

/// 组合根装配的工具运行时必须存在且可派发。
///
/// WHY 不是 `is_some()`：字段类型是 `Arc<ToolRuntime>`（非 Option）——
/// 组合根**必须**装配它，没有「不装配」这条路径。这正是本测试要锁的：
/// 若将来有人把字段改成 Option 并默认 None，本测试会红。
///
/// ADR-244 D2：组合根已装配真实 `BuiltinToolExecutor`。本断言从「未接线」
/// 迁移为「未知工具必须被如实拒绝」，守住同一类回归（伪造成功掩盖缺环）。
#[tokio::test]
async fn composition_root_wires_tool_runtime() {
    let ctx = build(&ChimeraConfig::default()).expect("组合根装配应成功");
    // 存在性：字段本身即证据（非 Option ⇒ 恒存在）
    let _ = &ctx.tool_runtime;

    // 可派发：四环齐备时，一个允许但**未在内置执行器注册**的工具名
    // 应走到传输环并被如实拒绝（而非在装配期失败，更不得返回 Ok）
    let req = DispatchRequest {
        tool_name: "repo.search".to_string(),
        args_json: r#"{"q":"hello"}"#.to_string(),
    };
    let result = ctx.tool_runtime.dispatch(&req).await;
    match result {
        // 已接线传输环：未知内置工具如实 fail-closed，归因 Transport
        Err(DispatchError::Transport(msg)) => {
            assert!(
                msg.contains("unknown builtin tool"),
                "未知工具必须由传输环如实拒绝（unknown builtin tool），实际: {msg}"
            );
        }
        // 沙箱层拦截注入形态 —— 也是合法路径，但归因必须是 Sandbox 而非 Transport
        Err(other) => panic!("期望传输环的未知工具拒绝，实际: {other:?}"),
        Ok(outcome) => panic!(
            "未知工具不得返回成功（会虚构功能），实际: {}",
            outcome.content_json
        ),
    }
}

/// ADR-244 D2 正向契约：已知内置工具必须经组合根四环链真实执行成功。
///
/// WHY 与上一条互补：只断言「未知工具被拒」可以靠一个「全拒绝」的假执行器
/// 蒙过去。本测试锁住「真实可执行」—— `text.echo` 必须 `Ok` 且回显参数，
/// 证明组合根 delegate 是真实执行器而非占位。
#[tokio::test]
async fn composition_root_executes_builtin_tool() {
    let ctx = build(&ChimeraConfig::default()).expect("组合根装配应成功");

    let args = r#"{"text":"hello"}"#;
    let req = DispatchRequest {
        tool_name: "text.echo".to_string(),
        args_json: args.to_string(),
    };
    let outcome = ctx
        .tool_runtime
        .dispatch(&req)
        .await
        .expect("text.echo 经组合根四环链应执行成功");
    assert!(
        outcome.content_json.contains("hello"),
        "text.echo 应回显参数，实际: {}",
        outcome.content_json
    );
}

/// 沙箱层在生产装配中真实生效 —— 注入形态参数被拦截。
///
/// WHY 这是本次接线的**核心新增防护**：接线前，生产路径既无策略层也无
/// 沙箱层；接线后沙箱层对 `tool_name + args_json` 完整生效（ADR-238 D4）。
/// 若该断言变红，说明沙箱层被绕过或失效 —— 这是安全回归的直接信号。
#[tokio::test]
async fn composition_root_tool_runtime_enforces_sandbox_gate() {
    let ctx = build(&ChimeraConfig::default()).expect("组合根装配应成功");

    // 注入形态参数
    let req = DispatchRequest {
        tool_name: "repo.search".to_string(),
        args_json: "$(rm -rf /)".to_string(),
    };
    let result = ctx.tool_runtime.dispatch(&req).await;
    assert!(
        matches!(result, Err(DispatchError::Sandbox(_))),
        "注入形态应被沙箱层拦截（归因 Sandbox），实际: {result:?}"
    );

    // 对照：正常参数不应被沙箱拦截 —— 证明上一条不是「全拒绝」
    let ok_req = DispatchRequest {
        tool_name: "repo.search".to_string(),
        args_json: r#"{"q":"safe"}"#.to_string(),
    };
    let ok_result = ctx.tool_runtime.dispatch(&ok_req).await;
    assert!(
        !matches!(ok_result, Err(DispatchError::Sandbox(_))),
        "正常参数不应被沙箱层拦截，实际: {ok_result:?}"
    );
}

/// 组合根派生的策略默认动作必须是 Allow —— 保持既有行为不削减。
///
/// WHY：策略的零信任默认是 `Deny`（`ExecPolicy::new()`）。若组合根用零信任
/// 默认，所有工具调用会从「可执行」变成「一律拒绝」⇒ **削减现有功能**。
/// 本测试锁定「默认 Allow」这一兼容姿态，防止后续批次无意改成零信任。
#[test]
fn tool_runtime_policy_default_action_is_allow() {
    let (_rules, default_action) = tool_runtime_policy();
    assert_eq!(
        default_action,
        seccore::execpolicy::PolicyAction::Allow,
        "默认动作必须是 Allow 以保持既有行为（零信任 Deny 会削减现有功能）"
    );
}

/// `--execute` 的「接线完成但开关关闭」姿态（Q-1 裁决 (b) + ADR-244 D3）。
///
/// WHY 这条断言锁定的是一个**架构状态**，不是行为细节：
/// 组合根已装配真实 `BuiltinToolExecutor`（ADR-244 D1），但 `run --execute`
/// 仍显式拒绝 —— 因为该命令路径不消费工具运行时，只有 `exec --plan` 消费。
///
/// 断言两件事：
/// 1. 拒绝路径**零副作用**（未建 Quest、未发事件）—— 既有 `run.rs` 单测已覆盖；
/// 2. 拒绝文案反映的是**真实**根因（执行器在组合根、仅 exec --plan 消费），
///    而非过时的「缺生产执行器」或「传输环 fail-closed」。
#[tokio::test]
async fn execute_refusal_reflects_real_root_cause() {
    // 文案稳定前缀（脚本按前缀 grep）
    let msg = chimera_cli::commands::run::EXECUTE_REFUSAL_MSG;
    assert!(
        msg.starts_with("execute_not_enabled:"),
        "拒绝文案需带稳定前缀，实际: {msg}"
    );
    // 真实根因：执行器已在组合根装配（不再宣称「缺 ToolExecutor」）
    assert!(
        msg.contains("composition root"),
        "拒绝文案需说明执行器在组合根装配，实际: {msg}"
    );
    // 真实可用路径：exec --plan
    assert!(
        msg.contains("chimera exec --plan"),
        "拒绝文案需指向唯一消费工具运行时的命令，实际: {msg}"
    );
    // 不得再出现过时措辞（ADR-240 前/后的两种旧说法）
    assert!(
        !msg.contains("not yet wired"),
        "文案不得再声称执行器未接线（会误导用户以为功能缺失），实际: {msg}"
    );
    // 可执行的替代路径
    assert!(
        msg.contains("without --execute"),
        "拒绝文案需给出可执行的替代路径，实际: {msg}"
    );
}

/// 组合根装配的四环链在**拒绝路径**上也必须先于传输环短路。
///
/// WHY：这是 fail-closed 的时序保证。注入形态参数必须在沙箱层被拦下，
/// **不得**触达传输环（哪怕传输环是未接线状态）—— 否则「沙箱生效」只是
/// 表面事实，错误归因会从 Sandbox 变成 Transport。
#[tokio::test]
async fn sandbox_rejection_precedes_transport_ring() {
    let ctx = build(&ChimeraConfig::default()).expect("组合根装配应成功");

    let req = DispatchRequest {
        tool_name: "repo.search".to_string(),
        args_json: "$(rm -rf /)".to_string(),
    };
    let result = ctx.tool_runtime.dispatch(&req).await;

    match result {
        Err(DispatchError::Sandbox(msg)) => {
            assert!(
                !msg.contains("not wired"),
                "沙箱拒绝的文案不应提及传输环未接线（说明短路发生在传输环之前），实际: {msg}"
            );
        }
        other => panic!("期望沙箱层拦截（归因 Sandbox），实际: {other:?}"),
    }
}

/// 反向断言：生产代码路径中不存在绕过 `tool_runtime` 的裸执行器装配。
///
/// WHY 反向断言：正向测试只能证明「组合根装了它」，无法证明「**只有**它」。
/// 若某命令层自行 `ExecPolicyToolExecutor::new(...)` 或直接持裸 delegate，
/// 沙箱层就会被绕过 —— 而这类绕过不会有任何编译错误或测试失败。
///
/// 实现方式：扫描 `crates/chimera-cli/src` 的源码文本，断言
/// `ExecPolicyToolExecutor::new` 与 `ToolExecutor` 的裸构造均不存在。
#[test]
fn production_code_does_not_bypass_tool_runtime() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut offenders: Vec<String> = Vec::new();

    let mut stack = vec![src.clone()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            // 跳过测试模块（单元测试允许构造替身）
            let Some(test_start) = text.find("#[cfg(test)]") else {
                // 无测试模块的文件：全量检查
                check_bypass(&text, &path, &mut offenders);
                continue;
            };
            let (prod, test_part) = text.split_at(test_start);
            check_bypass(prod, &path, &mut offenders);
            let _ = test_part;
        }
    }

    assert!(
        offenders.is_empty(),
        "生产代码中不得绕过 tool_runtime 装配执行器；发现: {offenders:?}"
    );
}

/// 逐文件检查绕过模式。
fn check_bypass(text: &str, path: &std::path::Path, offenders: &mut Vec<String>) {
    for (marker, why) in [
        (
            "ExecPolicyToolExecutor::new",
            "绕过四环链直接装配策略执行器",
        ),
        ("Box::new(InProcDispatcher", "绕过 builder 自行装配派发器"),
    ] {
        if text.contains(marker) {
            offenders.push(format!("{}: {marker} —— {why}", path.display()));
        }
    }
}
