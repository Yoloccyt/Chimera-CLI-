//! `mcp call` 命令面的四环准入前置（ADR-273 / W7）
//!
//! # WHY 这个文件
//!
//! ADR-256 只把 `mcp.<server>.<tool>` 接进了 **ToolExecutor 面**；命令面
//! （`chimera mcp call`）一直直调 `McpMesh::execute_transaction` —— 既不经过
//! execpolicy，也不经过沙箱门。本文件锁定 W7 补齐的三件事：
//!
//! 1. **策略拒绝在事务之前短路**（`policy:` 归因，无事务产生）；
//! 2. **沙箱拒绝在事务之前短路**（`sandbox:` 归因，注入形态参数被拦）；
//! 3. **Allow 路径行为不变**（前置放行后仍走既有 ephemeral mesh 执行）。
//!
//! 另加两条结构/分类锁：`mcp call` 必须带 runtime（缺了就是内部装配错误，
//! 不得静默跳过前置）、`list/inspect` 仍 `NoContext`（零装配税）。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom
#![forbid(unsafe_code)]

use chimera_cli::cli::{Commands, McpAction};
use chimera_cli::commands::mcp;
use chimera_cli::config::{ChimeraConfig, McpServerConfig};
use chimera_cli::permission::PermissionCtx;
use chimera_cli::spec::{classify, plan, CommandClass, CommandPlan, ShardingPolicy};
use gqep_executor::tool_runtime::ToolRuntime;
use seccore::execpolicy::{ExecPolicyRule, PolicyAction, RulePattern};

fn config_with_server(id: &str, url: &str) -> ChimeraConfig {
    let mut config = ChimeraConfig::default();
    config.mcp.servers = vec![McpServerConfig {
        id: id.to_string(),
        url: Some(url.to_string()),
    }];
    config
}

fn auto_perm() -> PermissionCtx {
    PermissionCtx {
        yes: true,
        no_permission: false,
    }
}

fn runtime_with(rules: Vec<ExecPolicyRule>, default: PolicyAction) -> ToolRuntime {
    ToolRuntime::with_optional_delegate(None, rules, default).expect("准入环组装应成功")
}

fn call_action(server: &str, tool: &str, args: Vec<String>) -> McpAction {
    McpAction::Call {
        server: server.to_string(),
        tool: tool.to_string(),
        args,
        network: false,
    }
}

/// 策略 Deny 必须在 `execute_transaction` 之前短路。
///
/// 判据：错误携带 `policy:` 归因与全名工具标识，且不含任何事务归因
/// （若前置被跳过，InProcess 传输会让调用**成功**，测试在 `expect_err` 处即红）。
#[tokio::test]
async fn mcp_call_policy_deny_blocks_before_transaction() {
    let config = config_with_server("dead", "203.0.113.1:9");
    let runtime = runtime_with(
        vec![ExecPolicyRule {
            pattern: RulePattern::new("mcp.dead.text.echo", "*"),
            action: PolicyAction::Deny,
        }],
        PolicyAction::Allow,
    );
    let action = call_action("dead", "text.echo", vec![r#"{"text":"hi"}"#.to_string()]);

    let err = mcp::execute(&action, &config, true, &auto_perm(), false, Some(&runtime))
        .await
        .expect_err("策略 Deny 必须在事务之前短路");

    let msg = format!("{err:?}");
    assert!(msg.contains("PermissionDenied"), "应归为权限拒绝: {msg}");
    assert!(
        msg.contains("policy:denied: policy rejected tool 'mcp.dead.text.echo'"),
        "必须带 policy 归因与全名工具: {msg}"
    );
    assert!(
        !msg.contains("transaction"),
        "拒绝时不得出现事务归因（说明未触达 execute_transaction）: {msg}"
    );
}

/// 沙箱拒绝：注入形态参数在事务之前被拦，归因 `sandbox:`。
///
/// 参数取 `;`（`CommandPolicy::default_secure` 的 Injection 拦截模式），
/// 且默认策略为 Allow —— 证明拦下来的是沙箱门而不是策略。
#[tokio::test]
async fn mcp_call_sandbox_blocks_injection_args() {
    let config = config_with_server("dead", "203.0.113.1:9");
    let runtime = runtime_with(vec![], PolicyAction::Allow);
    let action = call_action("dead", "text.echo", vec!["; rm -rf /".to_string()]);

    let err = mcp::execute(&action, &config, true, &auto_perm(), false, Some(&runtime))
        .await
        .expect_err("注入形态参数必须被沙箱前置拒绝");

    let msg = format!("{err:?}");
    assert!(
        msg.contains("sandbox:"),
        "归因必须落在沙箱层（默认策略 Allow 已排除 policy 归因）: {msg}"
    );
}

/// Allow 路径：前置放行后行为与改造前一致（仍走既有 ephemeral mesh）。
#[tokio::test]
async fn mcp_call_allow_proceeds_to_transaction() {
    let config = config_with_server("dead", "203.0.113.1:9");
    let runtime = runtime_with(vec![], PolicyAction::Allow);
    let action = call_action("dead", "text.echo", vec![r#"{"text":"hi"}"#.to_string()]);

    mcp::execute(&action, &config, true, &auto_perm(), false, Some(&runtime))
        .await
        .expect("Allow 路径应继续执行并成功");
}

/// 负对照：缺 runtime 不得静默跳过前置。
///
/// 这条防的是「为了兼容旧调用面把前置写成 `if let Some(...)` 可选」——
/// 那会让任何直调 `execute(..., None)` 的路径重新裸奔。
#[tokio::test]
async fn mcp_call_without_runtime_is_internal_error() {
    let config = config_with_server("dead", "203.0.113.1:9");
    let action = call_action("dead", "text.echo", vec![]);

    let err = mcp::execute(&action, &config, true, &auto_perm(), false, None)
        .await
        .expect_err("缺 ToolRuntime 必须报内部装配错误，而不是跳过前置");

    let msg = format!("{err:?}");
    assert!(msg.contains("ToolRuntime"), "{msg}");
}

/// 分类锁：只有 `mcp call` 升为 `FullContext{Off}`，其余三个动作零装配。
#[test]
fn mcp_classification_is_action_scoped() {
    let call = Commands::Mcp {
        action: call_action("s", "t", vec![]),
    };
    assert_eq!(classify(Some(&call)), CommandClass::FullContext);
    assert_eq!(
        plan(Some(&call)),
        CommandPlan::FullContext {
            sharding: ShardingPolicy::Off
        }
    );

    for action in [
        McpAction::List,
        McpAction::Inspect { server: "s".into() },
        McpAction::Serve,
    ] {
        let cmd = Commands::Mcp { action };
        assert_eq!(
            classify(Some(&cmd)),
            CommandClass::NoContext,
            "{cmd:?} 必须保持 NoContext（不付装配税）"
        );
        assert_eq!(
            plan(Some(&cmd)).class(),
            classify(Some(&cmd)),
            "plan 与 classify 必须一致: {cmd:?}"
        );
    }
}
