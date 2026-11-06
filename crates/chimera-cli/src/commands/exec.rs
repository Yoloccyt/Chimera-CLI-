//! `chimera exec <prompt>` — 非交互执行契约（WI-02）
//!
//! # stdout 纪律（WI-02 核心）
//! - 默认模式: stdout 仅最终结果（无流式动画、无 [done] 噪音——CI 管道
//!   `chimera exec "x" | jq .` 可直接消费）
//! - `--json` 模式: stdout = JSONL（每行一 AppEvent 形态，保持与全局
//!   envelope 一致的 jq 可解性）
//! - 日志/进度全走 stderr（tracing + eprintln），stdout 纯净断言进 CI
//!
//! # 退出码四类语义（WI-02 契约）
//! | 码 | 语义 | 映射 |
//! |----|------|------|
//! | 0 | 成功 | Ok |
//! | 2 | 审批拒否 | `PermissionDenied`（权限策略拒绝） |
//! | 3 | 预算耗尽 | `EngineError`（引擎/预算类故障） |
//! | 4 | 工具失败 | 其他错误（工具执行失败等） |
//!
//! # 与全局矩阵的关系（ADR-060）
//! exec 是**独立子命令契约**，其 0/2/3/4 语义不改变全局 0-6 矩阵
//! （兼容红线：全局矩阵服务所有命令，exec 映射仅在 dispatch 的
//! Exec 分支应用，经 [`exec_exit_code`] 显式转换）。

use anyhow::Result;
use nexus_contracts::spec::tool_plan::ToolPlan;
use nexus_core::{MultimodalInput, UserIntent};
use serde::Serialize;
use uuid::Uuid;

use crate::call_budget::{call_with_budget, CallBudgetError, DEFAULT_CALL_BUDGET};
use crate::composition::AppContext;
use crate::config::ChimeraConfig;
use crate::error::ChimeraCliError;
use crate::orchestrator::{build_error_reply, build_quest_reply};
use crate::permission::PermissionCtx;
use tokio_util::sync::CancellationToken;

/// CallBudgetError → ChimeraCliError 映射(exec WI-02 契约版,M2)
///
/// WHY 与 run.rs 的全局映射不同:exec 是独立退出码契约(0/2/3/4),
/// 预算/引擎类故障归 3(经 EngineError,见 exec_exit_code);若映射全局
/// `Timeout` 变体,会被 `exec_exit_code` 归入"工具失败"4(见
/// `error.rs::exit_code_for` 的 `ExitProfile::Exec` 分支)——分解预算耗尽≠工具执行超时,语义错配。
/// `Cancelled` 非预算/引擎类,按 UserCancelled 落契约 catch-all 4。
fn call_budget_to_exec_error(e: CallBudgetError) -> ChimeraCliError {
    match e {
        CallBudgetError::Timeout {
            operation_id,
            budget_ms,
        } => ChimeraCliError::EngineError(format!(
            "Quest 分解超时:{operation_id} 超过 {budget_ms}ms 未返回"
        )),
        CallBudgetError::Cancelled { .. } => ChimeraCliError::UserCancelled,
    }
}

/// 执行 exec 命令 — 兼容层 thin wrapper（M4-P1）
///
/// 独立调用场景（库调用方/既有测试）经组合根装配 ephemeral AppContext，
/// 语义与迁移前一致。dispatch 主链直接调 [`execute_with_ctx`] 共享
/// dispatch 级 AppContext（C12 唯一装配点）。
///
/// # 与 `chimera run` 的关系
/// 复用相同 QuestEngine 分解路径（单一真相源），差异在**输出纪律**：
/// run 保留流式动画（人类消费），exec 输出纯净（机器消费）。
pub async fn execute(
    prompt: &str,
    config: &ChimeraConfig,
    json: bool,
    _perm: &PermissionCtx,
) -> Result<()> {
    let ctx = crate::composition::build(config)?;
    execute_with_ctx(&ctx, prompt, json, _perm).await
}

/// exec 命令主体 — 从组合根 AppContext 取共享 bus + engine（M4-P1）
pub async fn execute_with_ctx(
    ctx: &AppContext,
    prompt: &str,
    json: bool,
    _perm: &PermissionCtx,
) -> Result<()> {
    tracing::info!(prompt = %prompt, "exec: 收到非交互任务");

    // 1. 共享 bus/engine 来自组合根（与 run 同源）
    //    WHY bus 引用:call_with_budget 需持 bus 发布 OperationTimedOut
    //    事件(M2 本层超时兜底; Parliament 路径同理保留 engine 的 clone)。
    let bus = &ctx.bus;
    let engine = &ctx.engine;

    // 2. 封装 UserIntent（UUIDv7 时间有序）
    let intent = UserIntent {
        intent_id: format!("exec-{}", Uuid::now_v7()),
        raw_text: prompt.to_string(),
        multimodal_inputs: vec![MultimodalInput::Text(prompt.to_string())],
        risk_level: 0,
    };

    // 3. 真实 L9 分解（错误 → 人类可读到 stderr，不污染 stdout）
    //    M2:create_quest 经 call_with_budget 注入本层超时(120s,对齐
    //    mca-gateway per-endpoint 口径)+ CancellationToken;超时发
    //    OperationTimedOut 事件。预算错误映射走 call_budget_to_exec_error
    //    (WI-02 契约:预算类故障归退出码 3,不落全局 Timeout 变体)。
    //    token 为新建未触发令牌,预留全局取消链路接线。
    let token = CancellationToken::new();
    let quest = match call_with_budget(
        engine.create_quest(intent),
        DEFAULT_CALL_BUDGET,
        &token,
        "exec.create_quest",
        bus,
    )
    .await
    {
        Ok(Ok(q)) => q,
        Ok(Err(e)) => return Err(ChimeraCliError::EngineError(build_error_reply(&e)).into()),
        Err(e) => return Err(call_budget_to_exec_error(e).into()),
    };

    // 4. 输出纪律（WI-02 核心）
    if json {
        // JSONL 模式: 每行一 AppEvent（ThreadStarted / TurnCompleted 骨架流，
        // 后续挂接真实 AppEvent 流——协议面类型已就绪 nexus-contracts::app）
        let payload = ExecEventPayload {
            event: "quest_created",
            quest_id: &quest.quest_id,
            task_count: quest.tasks.len(),
        };
        println!("{}", serde_json::to_string(&payload)?);
    } else {
        // 纯净模式: stdout 仅最终结果（单行回复，无流式动画）
        println!("{}", build_quest_reply(&quest));
    }

    Ok(())
}

/// 执行 `exec --plan` — 兼容层 thin wrapper（独立调用/库测试场景）
///
/// 语义与 [`execute`] 对齐：经组合根装配 ephemeral `AppContext`，
/// 计划文件（或 `-` stdin）解析为 [`ToolPlan`] 后交 [`crate::exec_plan::run_plan`]。
pub async fn execute_plan(
    plan_path: &std::path::Path,
    config: &ChimeraConfig,
    concurrency: usize,
    json: bool,
) -> Result<()> {
    let ctx = crate::composition::build(config)?;
    execute_plan_with_ctx(&ctx, plan_path, concurrency, json).await
}

/// `exec --plan` 命令主体 — 复用 dispatch 级 `AppContext`
///
/// # 退出码语义
/// - 计划读取/解析失败 → `EngineError`（Exec 退出码 3）
/// - 计划校验失败 → `EngineError`（3）
/// - 存在失败节点 → `IoError`（Exec 退出码 4；不改公共枚举，见注释）
pub async fn execute_plan_with_ctx(
    ctx: &AppContext,
    plan_path: &std::path::Path,
    concurrency: usize,
    json: bool,
) -> Result<()> {
    execute_plan_with_timeouts(ctx, plan_path, concurrency, json, None, None).await
}

/// `exec --plan` 的可显式超时版本（非破坏性扩展入口）
///
/// 超时解析见 [`resolve_timeouts`]：缺省值与既有行为**逐字一致**
/// （单操作 5s / 计划级 30s），显式值只能收紧、不能越过全局 `gather_deadline_ms`。
pub async fn execute_plan_with_timeouts(
    ctx: &AppContext,
    plan_path: &std::path::Path,
    concurrency: usize,
    json: bool,
    op_timeout_ms: Option<u64>,
    plan_timeout_ms: Option<u64>,
) -> Result<()> {
    let (plan_ms, op_ms) = resolve_timeouts(ctx, op_timeout_ms, plan_timeout_ms)?;
    let plan = load_tool_plan(plan_path).await?;
    let (summary, failed_nodes) = crate::exec_plan::run_plan_with_failures_timed(
        ctx,
        &plan,
        concurrency,
        plan_ms,
        Some(op_ms),
    )
    .await?;

    let report = PlanRunReport {
        status: if failed_nodes == 0 { "ok" } else { "failed" },
        plan_id: &summary.plan_id,
        summary: &summary.summary,
        tool_calls: summary.tool_calls,
        nodes_executed: summary.nodes_executed,
        failed_nodes,
        truncated: summary.truncated,
        duration_ms: summary.duration_ms,
    };
    crate::output::emit(json, &report, || {
        println!(
            "plan {}: {} node(s), {} tool call(s), {}ms{}",
            report.plan_id,
            report.nodes_executed,
            report.tool_calls,
            report.duration_ms,
            if report.truncated { " (truncated)" } else { "" }
        );
        println!("{}", report.summary);
        Ok(())
    })?;

    if failed_nodes > 0 {
        // WHY 用 IoError 承载「工具失败」：Exec profile 将 IoError 稳定映射为 4，
        // 且不新增公共枚举变体（避免破坏性 API）。计划/解析类故障已由 EngineError 归 3。
        return Err(ChimeraCliError::IoError(std::io::Error::other(format!(
            "plan {} failed: {failed_nodes} node(s) returned tool errors",
            summary.plan_id
        )))
        .into());
    }
    Ok(())
}

/// 解析 `exec --plan` 的 `(计划级, 单操作)` 超时
///
/// # 规则（优先级从内到外收敛）
/// 1. 单操作 = `--op-timeout-ms`，缺省 `GqepConfig::default_timeout_ms`（5s，既有默认）；
/// 2. 计划级 = `--plan-timeout-ms`，缺省 `PlanGuards::default().timeout_ms`（30s，既有默认）；
/// 3. 逐层取 min：单操作 ≤ 计划级；`gather_deadline_ms > 0` 时单操作 ≤ 全局截止
///    （CLI 只能**收紧**全局上限，不能越过）。`gather_deadline_ms = 0` 表示关闭
///    全局截止，不参与收敛（既有语义）。
///
/// 超大值（`> MAX_OP_TIMEOUT_MS` / `> MAX_PLAN_TIMEOUT_MS`）显式报 `EngineError`（退出码 3），
/// 不静默截断用户意图；`0` 与负数由 clap `range(1..)` 在解析期拒绝（退出码 2）。
pub fn resolve_timeouts(
    ctx: &AppContext,
    op_timeout_ms: Option<u64>,
    plan_timeout_ms: Option<u64>,
) -> Result<(u64, u64), ChimeraCliError> {
    if let Some(raw) = op_timeout_ms {
        if raw > crate::exec_plan::MAX_OP_TIMEOUT_MS {
            return Err(ChimeraCliError::EngineError(format!(
                "--op-timeout-ms={raw} 超出上限 {}ms",
                crate::exec_plan::MAX_OP_TIMEOUT_MS
            )));
        }
    }
    if let Some(raw) = plan_timeout_ms {
        if raw > crate::exec_plan::MAX_PLAN_TIMEOUT_MS {
            return Err(ChimeraCliError::EngineError(format!(
                "--plan-timeout-ms={raw} 超出上限 {}ms",
                crate::exec_plan::MAX_PLAN_TIMEOUT_MS
            )));
        }
    }

    let cfg = ctx.gqep.config();
    let plan_ms =
        plan_timeout_ms.unwrap_or_else(|| gqep_executor::PlanGuards::default().timeout_ms);
    let op_ms = crate::exec_plan::effective_op_timeout_ms(
        op_timeout_ms,
        plan_ms,
        cfg.default_timeout_ms,
        cfg.gather_deadline_ms,
    );
    Ok((plan_ms, op_ms))
}

/// 读取 ToolPlan 源（文件或 `-` stdin）并反序列化
async fn load_tool_plan(plan_path: &std::path::Path) -> Result<ToolPlan, ChimeraCliError> {
    use tokio::io::AsyncReadExt;

    let text = if plan_path == std::path::Path::new("-") {
        let mut buffer = String::new();
        let mut stdin = tokio::io::stdin();
        stdin.read_to_string(&mut buffer).await.map_err(|e| {
            ChimeraCliError::IoError(std::io::Error::other(format!("读取 stdin 失败: {e}")))
        })?;
        buffer
    } else {
        tokio::fs::read_to_string(plan_path).await.map_err(|e| {
            ChimeraCliError::IoError(std::io::Error::other(format!(
                "读取 plan 文件 {} 失败: {e}",
                plan_path.display()
            )))
        })?
    };

    serde_json::from_str::<ToolPlan>(&text)
        .map_err(|e| ChimeraCliError::EngineError(format!("解析 ToolPlan 失败: {e}")))
}

/// `exec --plan` 输出载荷（`--json` 走统一 envelope，人类模式打印摘要）
#[derive(Debug, Serialize)]
struct PlanRunReport<'a> {
    status: &'static str,
    plan_id: &'a str,
    summary: &'a str,
    tool_calls: usize,
    nodes_executed: usize,
    failed_nodes: usize,
    truncated: bool,
    duration_ms: u64,
}

/// `exec --json` 的单行事件载荷（JSONL，每行一个）。
#[derive(Debug, Serialize)]
struct ExecEventPayload<'a> {
    event: &'static str,
    quest_id: &'a str,
    task_count: usize,
}

/// exec 退出码映射 — WI-02 四类语义（0/2/3/4）
///
/// # 映射原则
/// - 审批拒否（权限策略拒绝）→ 2（与全局 PermissionDenied=5 不同——exec 契约）
/// - 引擎/预算类故障 → 3
/// - 其余（工具失败等）→ 4
///
/// 返回 `u8`；`Ok` 路径的 0 由调用方直接返回（dispatch 层）。
pub fn exec_exit_code(err: &ChimeraCliError) -> u8 {
    err.exit_code_for(crate::error::ExitProfile::Exec)
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn exec_exit_code_semantics() {
        // WI-02 四类语义: 2 审批拒否 / 3 预算耗尽 / 4 工具失败
        assert_eq!(
            exec_exit_code(&ChimeraCliError::PermissionDenied("denied".into())),
            2
        );
        assert_eq!(
            exec_exit_code(&ChimeraCliError::EngineError("budget exhausted".into())),
            3
        );
        assert_eq!(
            exec_exit_code(&ChimeraCliError::Timeout("tool timeout".into())),
            4,
            "工具超时按工具失败归 4"
        );
        assert_eq!(
            exec_exit_code(&ChimeraCliError::IoError(std::io::Error::other("tool io"))),
            4,
            "工具 IO 失败归 4"
        );
    }

    #[test]
    fn exec_does_not_override_global_matrix() {
        // 兼容红线: 全局矩阵保持 ADR-060 语义（exec 映射仅局部应用）
        assert_eq!(
            ChimeraCliError::PermissionDenied("x".into()).exit_code_value(),
            5,
            "全局矩阵 PermissionDenied 仍为 5（exec 的 2 仅在 Exec 分支应用）"
        );
    }

    // ---- M2 复核增量:call_with_budget 接入 exec 的映射验证(TDD:先写验证) ----

    #[test]
    fn exec_budget_timeout_maps_to_engine_error_exit_3() {
        // WI-02 契约:预算/引擎类故障 → 退出码 3(非全局 Timeout=6,亦非工具超时的 4)
        let err = call_budget_to_exec_error(CallBudgetError::Timeout {
            operation_id: "exec.create_quest".into(),
            budget_ms: 120_000,
        });
        assert!(
            matches!(err, ChimeraCliError::EngineError(_)),
            "exec 分解预算超时应按引擎/预算类映射 EngineError,实际: {err:?}"
        );
        assert_eq!(exec_exit_code(&err), 3);
    }

    #[test]
    fn exec_budget_cancelled_maps_to_user_cancelled_exit_4() {
        // 取消非预算/引擎类故障:按 UserCancelled 落 exec 契约 catch-all 4
        let err = call_budget_to_exec_error(CallBudgetError::Cancelled {
            operation_id: "exec.create_quest".into(),
        });
        assert!(matches!(err, ChimeraCliError::UserCancelled));
        assert_eq!(exec_exit_code(&err), 4);
    }

    #[tokio::test]
    async fn exec_create_quest_normal_path_with_budget() {
        // 包装后的正常路径回归(行为无感):端到端 execute 仍 Ok
        let config = ChimeraConfig::default();
        let perm = PermissionCtx::default();
        execute("评审增量验证:分解并总结", &config, false, &perm)
            .await
            .expect("exec 正常路径应成功");
    }

    /// M4-P1：exec 经 execute_with_ctx 走组合根共享 bus ——
    /// QuestCreated 对外部订阅者可见（此前命令自建私有 bus，事件零可见）。
    /// 此测试先行失败（红）：execute_with_ctx 尚不存在。
    #[tokio::test]
    async fn exec_events_visible_on_shared_context_bus() {
        use std::time::Duration;
        let ctx = crate::composition::build(&ChimeraConfig::default()).expect("装配应成功");
        // §4.4 反模式 3：先 subscribe 再驱动命令，否则事件静默丢失
        let mut rx = ctx.bus.subscribe();
        let perm = PermissionCtx::default();
        execute_with_ctx(&ctx, "评审增量验证:分解并总结", false, &perm)
            .await
            .expect("exec 正常路径应成功");
        let mut saw_quest_created = false;
        for _ in 0..40 {
            match tokio::time::timeout(Duration::from_millis(50), rx.recv()).await {
                Ok(Ok(event_bus::NexusEvent::QuestCreated { .. })) => {
                    saw_quest_created = true;
                    break;
                }
                Ok(Ok(_)) => continue,
                _ => break,
            }
        }
        assert!(
            saw_quest_created,
            "QuestCreated 应在组合根共享 bus 上可见（命令不再自建私有 bus）"
        );
    }
}
