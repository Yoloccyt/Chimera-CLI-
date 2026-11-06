//! `exec --plan` 真实接线验收测试（E1′/E3′/E4′ 首批次）
//!
//! 覆盖：
//! 1. `--plan <file>` 端到端（text.echo 成功 + 回显内容可见）
//! 2. 未知工具 → 退出码 4（工具失败，不得当成功）
//! 3. 非法 JSON / 非法计划 → `EngineError` → 退出码 3
//! 4. 策略拒绝归因 `policy:` / 沙箱拦截归因 `sandbox:`
//! 5. SccCache 单点回写（`len()` / `contains()` / `stats()` 计数增长）
//! 6. `--plan -`（stdin）经真实二进制可用
//!
//! 说明：全部经库函数（`execute_plan_with_ctx` / `run_plan_with_failures`）驱动，
//! 仅 stdin 用例走 `CARGO_BIN_EXE_chimera` 以避免进程内伪造 stdin。

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::process::Stdio;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;

use chimera_cli::commands::exec;
use chimera_cli::composition;
use chimera_cli::config::ChimeraConfig;
use chimera_cli::error::ChimeraCliError;
use chimera_cli::exec_plan::{self, BuiltinToolExecutor, RuntimeToolExecutor};
use gqep_executor::tool_runtime::ToolRuntime;
use gqep_executor::ToolExecutor;
use nexus_contracts::spec::tool_plan::{PlanEdge, SideEffectDecl, ToolNode, ToolOp, ToolPlan};
use scc_cache::ContextId;
use seccore::execpolicy::{ExecPolicyRule, PolicyAction, RulePattern};
use tempfile::TempDir;

fn build_ctx() -> composition::AppContext {
    composition::build(&ChimeraConfig::default()).expect("组合根装配应成功")
}

fn write_plan(dir: &TempDir, name: &str, json: &str) -> std::path::PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, json).expect("写入 plan 文件");
    path
}

fn echo_plan() -> ToolPlan {
    ToolPlan {
        id: "echo-plan".into(),
        nodes: vec![ToolNode {
            id: "n1".into(),
            op: ToolOp::ToolCall,
            tool_name: Some("text.echo".into()),
            args_json: Some(r#"{"msg":"hello-from-plan"}"#.into()),
            field: None,
            predicate: None,
            side_effect: Some(SideEffectDecl::ReadOnly),
        }],
        edges: vec![],
    }
}

fn unknown_tool_plan() -> ToolPlan {
    ToolPlan {
        id: "unknown-plan".into(),
        nodes: vec![ToolNode {
            id: "n1".into(),
            op: ToolOp::ToolCall,
            tool_name: Some("no.such.tool".into()),
            args_json: Some("{}".into()),
            field: None,
            predicate: None,
            side_effect: Some(SideEffectDecl::ReadOnly),
        }],
        edges: vec![],
    }
}

fn error_as_chimera(err: &anyhow::Error) -> &ChimeraCliError {
    err.downcast_ref::<ChimeraCliError>()
        .expect("错误应携带 ChimeraCliError（用于退出码映射）")
}

/// 计数型 delegate — 用于证明前置环拒绝时**真实 IO 从未发生**
struct CountingDelegate {
    hits: Arc<AtomicUsize>,
}

#[async_trait]
impl gqep_executor::ToolExecutor for CountingDelegate {
    async fn execute(&self, _tool: &str, _args: &str) -> Result<String, String> {
        self.hits.fetch_add(1, Ordering::SeqCst);
        Ok("{}".into())
    }
}

// 1. `--plan <file>` 端到端：text.echo 成功，回显内容可见
#[tokio::test]
async fn plan_file_echo_succeeds_and_echo_is_visible() {
    let dir = TempDir::new().expect("tempdir");
    let plan = echo_plan();
    let json = serde_json::to_string(&plan).expect("序列化 plan");
    let path = write_plan(&dir, "echo.json", &json);

    let ctx = build_ctx();
    exec::execute_plan_with_ctx(&ctx, &path, 4, false)
        .await
        .expect("text.echo 计划应成功");

    let (summary, failed) = exec_plan::run_plan_with_failures(&ctx, &plan, 4, 30_000)
        .await
        .expect("plan 应执行成功");
    assert_eq!(failed, 0, "text.echo 不应有失败节点");
    assert!(
        summary.summary.contains("hello-from-plan"),
        "回填结果应含回显内容，实际: {}",
        summary.summary
    );
}

// 2. 未知工具 → 退出码 4 语义（不得成功）
#[tokio::test]
async fn unknown_tool_yields_exit_code_4() {
    let dir = TempDir::new().expect("tempdir");
    let plan = unknown_tool_plan();
    let json = serde_json::to_string(&plan).expect("序列化 plan");
    let path = write_plan(&dir, "unknown.json", &json);

    let ctx = build_ctx();
    let err = exec::execute_plan_with_ctx(&ctx, &path, 2, false)
        .await
        .expect_err("未知工具不得被当作成功");
    let ce = error_as_chimera(&err);
    assert_eq!(exec::exec_exit_code(ce), 4, "工具失败应映射退出码 4");
}

// 3a. 非法 JSON → EngineError → 退出码 3
#[tokio::test]
async fn invalid_json_yields_engine_error_exit_3() {
    let dir = TempDir::new().expect("tempdir");
    let path = write_plan(&dir, "broken.json", "{ this is not json");

    let ctx = build_ctx();
    let err = exec::execute_plan_with_ctx(&ctx, &path, 1, false)
        .await
        .expect_err("非法 JSON 必须失败");
    let ce = error_as_chimera(&err);
    assert!(
        matches!(ce, ChimeraCliError::EngineError(_)),
        "非法 JSON 应映射 EngineError，实际: {ce:?}"
    );
    assert_eq!(exec::exec_exit_code(ce), 3);
}

// 3b. 非法计划（悬空边）→ EngineError → 退出码 3
#[tokio::test]
async fn invalid_plan_yields_engine_error_exit_3() {
    let plan = ToolPlan {
        id: "bad-graph".into(),
        nodes: vec![ToolNode {
            id: "n1".into(),
            op: ToolOp::ToolCall,
            tool_name: Some("text.echo".into()),
            args_json: Some("{}".into()),
            field: None,
            predicate: None,
            side_effect: Some(SideEffectDecl::ReadOnly),
        }],
        edges: vec![PlanEdge {
            from: "n1".into(),
            to: "ghost".into(),
        }],
    };
    let ctx = build_ctx();
    let err = exec_plan::run_plan_with_failures(&ctx, &plan, 1, 30_000)
        .await
        .expect_err("悬空边计划必须被拒绝");
    assert!(
        matches!(err, ChimeraCliError::EngineError(_)),
        "校验失败应映射 EngineError，实际: {err:?}"
    );
    assert_eq!(exec::exec_exit_code(&err), 3);
}

// 4a. 策略拒绝归因 `policy:`
#[tokio::test]
async fn policy_denial_is_attributed_to_policy_layer() {
    let runtime = ToolRuntime::with_optional_delegate(
        Some(Arc::new(BuiltinToolExecutor)),
        vec![ExecPolicyRule {
            pattern: RulePattern::new("text.echo", "*"),
            action: PolicyAction::Deny,
        }],
        PolicyAction::Allow,
    )
    .expect("四环链装配应成功");
    let executor = RuntimeToolExecutor {
        inner: Arc::new(runtime),
    };
    let err = executor
        .execute("text.echo", r#"{"msg":"denied"}"#)
        .await
        .expect_err("策略拒绝必须失败");
    assert!(err.starts_with("policy:"), "应归因 policy:，实际: {err}");
}

// 4b. 沙箱拦截归因 `sandbox:`
#[tokio::test]
async fn sandbox_block_is_attributed_to_sandbox_layer() {
    let runtime = ToolRuntime::with_optional_delegate(
        Some(Arc::new(BuiltinToolExecutor)),
        vec![],
        PolicyAction::Allow,
    )
    .expect("四环链装配应成功");
    let executor = RuntimeToolExecutor {
        inner: Arc::new(runtime),
    };
    let err = executor
        .execute("text.echo", r#""$(rm -rf /)""#)
        .await
        .expect_err("注入参数必须被沙箱拦截");
    assert!(err.starts_with("sandbox:"), "应归因 sandbox:，实际: {err}");
}

// 5. SccCache 单点回写：len 增长 + contains + stats 快照计数增长
#[tokio::test]
async fn plan_run_writes_single_scc_entry() {
    let ctx = build_ctx();
    assert_eq!(ctx.scc.len(), 0, "新建组合根缓存应为空");

    let plan = echo_plan();
    let (summary, _failed) = exec_plan::run_plan_with_failures(&ctx, &plan, 1, 30_000)
        .await
        .expect("plan 应执行成功");

    let entry_id = ContextId::new(format!("exec-plan:{}", summary.plan_id));
    assert_eq!(ctx.scc.len(), 1, "单次运行只允许回写 1 条");
    assert!(ctx.scc.contains(&entry_id), "应包含 exec-plan 回写条目");

    // stats() 是快照（每 100 次访问刷新一次），此处主动触达以验证计数
    for _ in 0..100 {
        let _ = ctx.scc.get_or_prefetch(&entry_id);
    }
    assert_eq!(
        ctx.scc.stats().entry_count,
        1,
        "stats 快照应反映 1 条回写条目"
    );
}

// 6. `--plan -`（stdin）经真实二进制可用
#[tokio::test]
async fn plan_from_stdin_is_supported() {
    // CLI 层先验证 `-` 被接受为 plan 路径
    use clap::Parser;
    let cli = chimera_cli::Cli::try_parse_from(["chimera", "exec", "--plan", "-"])
        .expect("应接受 stdin 计划");
    match cli.command {
        Some(chimera_cli::Commands::Exec { plan, .. }) => {
            assert_eq!(
                plan.as_deref(),
                Some(Path::new("-")),
                "--plan - 应解析为 stdin 标记"
            );
        }
        other => panic!("应为 Exec 命令，实际: {other:?}"),
    }

    // 二进制端到端：stdin 喂入 plan JSON
    let plan = echo_plan();
    let json = serde_json::to_string(&plan).expect("序列化 plan");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_chimera"))
        .args(["exec", "--plan", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;
            child
                .stdin
                .as_mut()
                .expect("stdin")
                .write_all(json.as_bytes())?;
            child.wait_with_output()
        })
        .expect("二进制应可执行");
    assert!(
        output.status.success(),
        "stdin plan 应成功，stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("hello-from-plan"),
        "stdout 应含回显内容，实际: {}",
        String::from_utf8_lossy(&output.stdout)
    );
}

// 9. 策略拒绝必须发生在真实 IO 之前（delegate 零命中）
#[tokio::test]
async fn policy_denial_skips_delegate_invocation() {
    let hits = Arc::new(AtomicUsize::new(0));
    let runtime = ToolRuntime::with_optional_delegate(
        Some(Arc::new(CountingDelegate {
            hits: Arc::clone(&hits),
        })),
        vec![ExecPolicyRule {
            pattern: RulePattern::new("text.echo", "*"),
            action: PolicyAction::Deny,
        }],
        PolicyAction::Allow,
    )
    .expect("四环链装配应成功");
    let executor = RuntimeToolExecutor {
        inner: Arc::new(runtime),
    };
    let err = executor
        .execute("text.echo", r#"{"msg":"x"}"#)
        .await
        .expect_err("策略拒绝必须失败");
    assert!(err.starts_with("policy:"), "应归因 policy:，实际: {err}");
    // WHY 断言 0:policy 环是四环链最外层，拒绝即短路，delegate 不应被触达
    assert_eq!(hits.load(Ordering::SeqCst), 0, "策略拒绝不得触达真实 IO");
}

// 11. 超时默认值与既有行为逐字一致（单操作 5s / 计划级 30s）
#[tokio::test]
async fn resolve_timeouts_defaults_are_unchanged() {
    let ctx = build_ctx();
    let (plan_ms, op_ms) = exec::resolve_timeouts(&ctx, None, None).expect("缺省值应合法");
    assert_eq!(
        plan_ms,
        gqep_executor::PlanGuards::default().timeout_ms,
        "计划级缺省必须等于既有 PlanGuards 默认（30s）"
    );
    assert_eq!(
        op_ms,
        ctx.gqep.config().default_timeout_ms,
        "单操作缺省必须等于既有 GqepConfig 默认（5s）"
    );
}

// 12. 单操作不得越过计划级（CLI 只能收紧）
#[tokio::test]
async fn resolve_timeouts_op_cannot_exceed_plan_scope() {
    let ctx = build_ctx();
    let (plan_ms, op_ms) =
        exec::resolve_timeouts(&ctx, Some(120_000), Some(1_000)).expect("合法值");
    assert_eq!(plan_ms, 1_000);
    assert_eq!(op_ms, 1_000, "单操作必须被计划级收紧，实际: {op_ms}");
}

// 13. 全局 gather_deadline_ms 是硬上限（CLI 显式值不能越过）
#[tokio::test]
async fn resolve_timeouts_gather_deadline_caps_op() {
    let ctx = build_ctx();
    let gather = ctx.gqep.config().gather_deadline_ms;
    assert!(gather > 0, "默认配置应开启全局截止（本用例前提）");
    let (_, op_ms) = exec::resolve_timeouts(&ctx, Some(10_000), None).expect("合法值");
    assert_eq!(op_ms, gather, "单操作必须被全局截止收紧，实际: {op_ms}");
}

// 14. 超大值显式拒绝（EngineError → Exec 退出码 3），不静默截断
#[tokio::test]
async fn resolve_timeouts_rejects_oversized_values() {
    let ctx = build_ctx();

    let err = exec::resolve_timeouts(&ctx, Some(exec_plan::MAX_OP_TIMEOUT_MS + 1), None)
        .expect_err("超上限的单操作超时必须被拒绝");
    assert!(
        matches!(&err, ChimeraCliError::EngineError(m) if m.contains("--op-timeout-ms")),
        "应归因 EngineError(--op-timeout-ms)，实际: {err:?}"
    );
    assert_eq!(
        exec::exec_exit_code(&err),
        3,
        "EngineError 在 Exec profile 下必须映射为退出码 3"
    );

    let err = exec::resolve_timeouts(&ctx, None, Some(exec_plan::MAX_PLAN_TIMEOUT_MS + 1))
        .expect_err("超上限的计划级超时必须被拒绝");
    assert!(
        matches!(&err, ChimeraCliError::EngineError(m) if m.contains("--plan-timeout-ms")),
        "应归因 EngineError(--plan-timeout-ms)，实际: {err:?}"
    );
    assert_eq!(exec::exec_exit_code(&err), 3);
}

// 15. 合法边界值（上限本身）必须被接受
#[tokio::test]
async fn resolve_timeouts_accepts_upper_bound() {
    let ctx = build_ctx();
    let (plan_ms, op_ms) = exec::resolve_timeouts(
        &ctx,
        Some(exec_plan::MAX_OP_TIMEOUT_MS),
        Some(exec_plan::MAX_PLAN_TIMEOUT_MS),
    )
    .expect("上限值本身应合法");
    assert_eq!(plan_ms, exec_plan::MAX_PLAN_TIMEOUT_MS);
    assert!(
        op_ms <= plan_ms,
        "上限组合下单操作仍不得越过计划级: {op_ms} vs {plan_ms}"
    );
}

// 16. CLI 解析：合法值透传；0/负数在解析期拒绝（退出码 2）
#[test]
fn cli_timeout_flags_parse_and_reject_invalid() {
    use clap::Parser;

    let cli = chimera_cli::Cli::try_parse_from([
        "chimera",
        "exec",
        "--plan",
        "p.json",
        "--op-timeout-ms",
        "2500",
        "--plan-timeout-ms",
        "60000",
    ])
    .expect("合法超时值应解析成功");
    match cli.command {
        Some(chimera_cli::Commands::Exec {
            op_timeout_ms,
            plan_timeout_ms,
            ..
        }) => {
            assert_eq!(op_timeout_ms, Some(2_500));
            assert_eq!(plan_timeout_ms, Some(60_000));
        }
        other => panic!("应为 Exec 命令，实际: {other:?}"),
    }

    // 0：range(1..) 在解析期拒绝 → clap 退出码 2
    let err = chimera_cli::Cli::try_parse_from([
        "chimera",
        "exec",
        "--plan",
        "p.json",
        "--op-timeout-ms",
        "0",
    ])
    .expect_err("0 必须被拒绝");
    assert_eq!(err.exit_code(), 2, "非法值的 clap 退出码应为 2");

    // 负数：以 `=` 形式交给 value_parser，确保走数值校验而非「未知 flag」
    let err = chimera_cli::Cli::try_parse_from([
        "chimera",
        "exec",
        "--plan",
        "p.json",
        "--plan-timeout-ms=-1",
    ])
    .expect_err("负数必须被拒绝");
    assert_eq!(err.exit_code(), 2, "负数的 clap 退出码应为 2");
}

#[tokio::test]
async fn sandbox_block_skips_delegate_invocation() {
    let hits = Arc::new(AtomicUsize::new(0));
    let runtime = ToolRuntime::with_optional_delegate(
        Some(Arc::new(CountingDelegate {
            hits: Arc::clone(&hits),
        })),
        vec![],
        PolicyAction::Allow,
    )
    .expect("四环链装配应成功");
    let executor = RuntimeToolExecutor {
        inner: Arc::new(runtime),
    };
    let err = executor
        .execute("text.echo", r#""$(rm -rf /)""#)
        .await
        .expect_err("注入参数必须被沙箱拦截");
    assert!(err.starts_with("sandbox:"), "应归因 sandbox:，实际: {err}");
    // WHY 断言 0:sandbox 环在 transport 之前，拦截时真实 IO 不应发生
    assert_eq!(hits.load(Ordering::SeqCst), 0, "沙箱拦截不得触达真实 IO");
}

// 17. 单操作 < 全局 gather 截止：显式值原样保留（全局只在更小时收紧）
#[tokio::test]
async fn resolve_timeouts_op_below_gather_is_preserved() {
    let ctx = build_ctx();
    let gather = ctx.gqep.config().gather_deadline_ms;
    assert!(
        gather > 1,
        "本用例前提：默认配置开启全局截止（实测 gather={gather}ms）"
    );
    let (_, op_ms) = exec::resolve_timeouts(&ctx, Some(1_000), None).expect("合法值");
    assert_eq!(
        op_ms, 1_000,
        "单操作({op_ms}ms)小于全局截止({gather}ms)时不得被改写"
    );
}

// 18. 超时归因：`Timeout` 在 Exec 档映射为退出码 4（工具/计划超时）
#[test]
fn timeout_maps_to_exec_exit_code_4() {
    let err = ChimeraCliError::Timeout("plan outer wrapper timed out".into());
    assert_eq!(
        exec::exec_exit_code(&err),
        4,
        "Exec 档超时必须归 4，实际: {}",
        exec::exec_exit_code(&err)
    );
    assert_eq!(err.kind(), "Timeout", "JSON 的 kind 字段应稳定为 Timeout");
}
