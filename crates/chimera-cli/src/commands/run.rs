//! `chimera run <prompt>` — 单次任务运行,真实接入 L9 QuestEngine
//!
//! v2.9.0-omega Task 1.1:替换 NotImplemented 占位,真实调用 QuestEngine 分解任务。
//!
//! # 流程
//! 1. 从组合根 AppContext 取共享 bus + engine（dispatch 共享装配；独立调用经
//!    wrapper 转组合根 ephemeral 装配，M4-P1 后无自建 EventBus::new）
//! 2. 将 prompt 封装为 `UserIntent`,交由 `QuestEngine::create_quest` 分解为任务 DAG
//! 3. 成功:复用 `orchestrator::build_quest_reply` 生成回复文本,逐字符流式打印到 stdout
//! 4. 完成后输出 `[done]` 标记(SubTask 1.1.3)
//! 5. `--json` 时输出 Quest 结构的 envelope JSON(替代流式文本)
//!
//! # 设计决策(WHY)
//! - **进程内 ephemeral 引擎**:CLI 单次运行不共享 TUI 的长生命周期 QuestEngine,
//!   每次调用创建独立实例。Quest 分解结果不持久化,适合"快速分解看任务图"场景。
//!   需要持久化的 Quest 管理请用 `chimera tui`。
//! - **复用 orchestrator 纯函数**:`build_quest_reply` / `plan_chunks` 已在 TUI 编排器
//!   实现,此处直接复用,保证 CLI 与 TUI 输出格式一致(单一真相源)。
//! - **流式输出到 stdout 而非 EventBus**:spec SubTask 1.1.2 明确要求"非 EventBus",
//!   逐字符打印到 stdout 供 shell 管道消费(`chimera run "..." | tee log.txt`)。
//!
//! v2.9.0-omega Task 1.7:接受 `json` flag(JSON 成功 envelope 在本命令输出)
//! v2.9.0-omega Task 1.11:接受 `perm`(Task 1.1 真实接入外部命令执行时使用)

use std::time::Duration;

use anyhow::Result;
use gqep_executor::tool_runtime::{BatchLimits, BatchOutcome, DispatchError, DispatchRequest};
use nexus_core::{MultimodalInput, Quest, UserIntent};
use uuid::Uuid;

use crate::call_budget::{call_with_budget, CallBudgetError, DEFAULT_CALL_BUDGET};
use crate::composition::AppContext;
use crate::config::ChimeraConfig;
use crate::error::ChimeraCliError;
use crate::orchestrator::{build_error_reply, build_quest_reply, plan_chunks};
use crate::output;
use crate::permission::{confirm_with, PermissionCtx, PermissionDecision, PermissionGate};

/// CallBudgetError → ChimeraCliError 映射(M2 接入点共用)
///
/// WHY 走既有矩阵而非新增变体:`Timeout` 对应 ADR-060 退出码 6(timeout),
/// `Cancelled` 对应退出码 4(user_cancelled)——纯增量防护复用既有语义,
/// 不扩展错误面(对外 CLI 行为无感)。
fn call_budget_to_cli_error(e: CallBudgetError) -> ChimeraCliError {
    match e {
        CallBudgetError::Timeout {
            operation_id,
            budget_ms,
        } => ChimeraCliError::Timeout(format!("{operation_id} 超过 {budget_ms}ms 未返回")),
        CallBudgetError::Cancelled { .. } => ChimeraCliError::UserCancelled,
    }
}

/// 流式 chunk 之间的延迟(制造逐字符浮现观感)
///
/// WHY 固定 20ms:与 TUI 编排器默认值一致(OrchestratorConfig::default().chunk_delay),
/// 每 tick 约 12 字符,视觉流畅且不拖慢脚本消费。测试场景可通过环境变量
/// `CHIMERA_RUN_CHUNK_DELAY_MS=0` 设为零延迟(仅 CLI run 路径生效)。
///
/// W8 清理: chat.rs 原同值常量去重,统一引用本定义(单点维护)
pub(crate) const DEFAULT_CHUNK_DELAY_MS: u64 = 20;

/// `--execute` 未获授权时的稳定拒绝文案(可被测试与文档引用)
///
/// WHY 文案在 ADR-247 后再次修正：传输环与四环链自 ADR-244 起在组合根真实
/// 装配，且 `run --execute` 已真实接入 `execute_batch`（F-13 首个生产调用方）——
/// 拒绝的根因**不再是「不消费工具运行时」**，而是**授权前置未通过**：
/// 非交互环境的权限三态为 `Unknown`，按 fail-closed 语义降级为 `Deny`
/// （`PermissionGate::authorize`），不读 stdin、零副作用。需要执行时：交互
/// 终端确认、`--yes` 显式授权，或改用 `chimera exec --plan`（CI 管道场景）。
///
/// 稳定前缀 `execute_not_enabled:` 不变（脚本按前缀 grep）；文案正文保留
/// 既有 5 条断言所需的子串（sandbox / composition root / chimera exec --plan /
/// without --execute），既有测试零修改。
pub const EXECUTE_REFUSAL_MSG: &str =
    "execute_not_enabled: tool execution was not authorized (non-interactive session: the \
     permission gate fail-closed an Unknown decision to Deny — pass --yes, confirm in a \
     terminal, or run without --execute). The four-ring chain (policy/sandbox/route/transport) \
     and the ToolExecutor are wired at the composition root; non-interactive pipelines should \
     use `chimera exec --plan <PATH|->` instead";

/// 构造 `--execute` 拒绝错误(`NotImplemented`,退出码 2)
#[must_use]
pub fn execute_refusal() -> ChimeraCliError {
    ChimeraCliError::NotImplemented(EXECUTE_REFUSAL_MSG.to_string())
}

/// `--execute` 授权前置（ADR-247 D1,fail-closed）
///
/// - `Allow`（`--yes` / `--no-permission`）→ 直接放行,不读 stdin;
/// - 非交互 `Unknown` → gate 已 fail-closed 降级为 `Deny`（计数器 + warn）
///   → 稳定前缀拒绝（退出码 2、零副作用、不读 stdin）——脚本 grep 契约不变;
/// - 交互 `Unknown` → confirm prompt（Ctrl+C 竞争返回 `UserCancelled`）;
///   用户明确拒绝 → `UserCancelled`（退出码 4,而非 2——主动放弃不是「未实装」）。
async fn ensure_execute_authorized(
    gate: &PermissionGate,
    perm: &PermissionCtx,
    action: &str,
) -> Result<(), ChimeraCliError> {
    match gate.authorize(perm).decision {
        PermissionDecision::Allow => Ok(()),
        PermissionDecision::Deny => Err(execute_refusal()),
        PermissionDecision::Unknown => {
            if confirm_with(gate, perm, "run --execute 工具执行", action).await? {
                Ok(())
            } else {
                Err(ChimeraCliError::UserCancelled)
            }
        }
    }
}

/// Quest 任务 → `text.echo` 派发请求批量（ADR-247 D3,纯函数）
///
/// WHY 固定 `text.echo`:首个生产批量的工具来源为内置只读执行器（任务描述
/// 回显,可端到端验证输入序契约);真实工具路由（P1 McpToolExecutor 等）扩展
/// 此映射,不在本批引入网络/进程副作用面。
#[must_use]
pub fn build_batch_requests(quest: &Quest) -> Vec<DispatchRequest> {
    quest
        .tasks
        .iter()
        .map(|task| DispatchRequest {
            tool_name: "text.echo".to_string(),
            args_json: serde_json::json!({ "text": task.description }).to_string(),
        })
        .collect()
}

/// 经组合根唯一 ToolRuntime 批量执行 Quest 任务（F-13 首个生产调用方）
///
/// # 语义契约
/// - 有界并发 + 输入序结果（`results[i]` ↔ 任务列表第 i 项,gqep 契约）;
/// - `BatchLimits::default()`（8/30s/120s 双层超时）——并发度配置化留待 W5;
/// - `call_with_budget` 包裹整批（120s 进程级预算 + 取消令牌,超时发既有
///   `OperationTimedOut` 事件）——与 `create_quest` 同一防护口径;
/// - 空分解（0 任务）为合法成功:零派发、退出码 0。
pub async fn execute_quest_tool_batch(ctx: &AppContext, quest: &Quest) -> Result<BatchOutcome> {
    let requests = build_batch_requests(quest);
    if requests.is_empty() {
        return Ok(BatchOutcome::empty());
    }
    let token = crate::call_budget::cancel_root().clone();
    let outcome = call_with_budget(
        ctx.tool_runtime
            .execute_batch(&requests, BatchLimits::default()),
        DEFAULT_CALL_BUDGET,
        &token,
        "run.execute_batch",
        &ctx.bus,
    )
    .await
    .map_err(call_budget_to_cli_error)?;
    tracing::info!(
        total = outcome.results.len(),
        succeeded = outcome.succeeded,
        failed = outcome.failed,
        abandoned = outcome.abandoned,
        not_started = outcome.not_started,
        "run --execute 批量派发完成(F-13 首个生产调用方,ADR-247)"
    );
    Ok(outcome)
}

/// 批量结果 → CLI 错误归因（ADR-060 单一真值源,纯函数,不新造退出码）
///
/// 取**首个**失败逐条归因（顺序即输入序,可定位到第几条任务）:
/// 策略/沙箱拒绝 → `PermissionDenied`(5);路由或非 deadline 传输失败 →
/// `EngineError`(3);整批 deadline 超时 → `Timeout`(6)。
/// WHY 无 `#[must_use]`:返回 `Result` 本身已天然 must_use（叠加即 double_must_use）。
pub fn batch_outcome_to_cli_error(outcome: &BatchOutcome) -> Result<(), ChimeraCliError> {
    for res in &outcome.results {
        match res {
            Ok(_) => {}
            Err(DispatchError::Policy(msg)) | Err(DispatchError::Sandbox(msg)) => {
                return Err(ChimeraCliError::PermissionDenied(msg.clone()));
            }
            Err(DispatchError::Route(msg)) => {
                return Err(ChimeraCliError::EngineError(format!("route: {msg}")));
            }
            Err(DispatchError::Transport(msg)) => {
                if msg.starts_with(gqep_executor::tool_runtime::BATCH_DEADLINE_PREFIX) {
                    return Err(ChimeraCliError::Timeout(msg.clone()));
                }
                return Err(ChimeraCliError::EngineError(format!("transport: {msg}")));
            }
        }
    }
    Ok(())
}

/// 输出批量结果摘要（`json=true` 输出机器可读 envelope,否则人类可读逐条行）
fn report_batch_outcome(outcome: &BatchOutcome, json: bool) -> Result<()> {
    if json {
        let items: Vec<serde_json::Value> = outcome
            .results
            .iter()
            .map(|r| match r {
                Ok(o) => serde_json::json!({ "status": "ok", "content": o.content_json }),
                Err(e) => serde_json::json!({ "status": "error", "error": e.to_string() }),
            })
            .collect();
        output::print_json(&serde_json::json!({
            "results": items,
            "succeeded": outcome.succeeded,
            "failed": outcome.failed,
            "abandoned": outcome.abandoned,
            "not_started": outcome.not_started,
        }))?;
        return Ok(());
    }
    let total = outcome.results.len();
    for (i, r) in outcome.results.iter().enumerate() {
        match r {
            Ok(o) => println!("[tool {}/{}] ok: {}", i + 1, total, o.content_json),
            Err(e) => println!("[tool {}/{}] failed: {e}", i + 1, total),
        }
    }
    println!("[done]");
    Ok(())
}

/// 执行 run 命令 — 兼容层 thin wrapper（M4-P1）
///
/// 独立调用场景（库调用方/既有测试）经组合根装配 ephemeral AppContext，
/// 语义与迁移前"进程内 ephemeral 引擎"一致。dispatch 主链不走本 wrapper，
/// 直接调 [`execute_with_ctx`] 共享 dispatch 级 AppContext（C12 唯一装配点）。
///
/// `prompt` 为用户意图原始文本,`config` 为已加载的合并配置。
///
/// `json` flag(Task 1.7):`true` 时输出 JSON envelope,取代逐字符流式文本。
///
/// `gate`(ADR-247):权限门（生产入口传 `PermissionGate::from_stdin()`，测试
/// 注入 `PermissionGate::new(interactive)`）——授权逻辑统收在
/// [`execute_with_ctx`]，本 wrapper 不自带拒绝分支（E-1 半闭环由构造消除）。
///
/// `perm`(Task 1.11):`--yes` / `--no-permission` 上下文，经 gate 消费。
///
/// `execute`(T1-3 + ADR-247):`true` 时先经 [`ensure_execute_authorized`]
/// 授权前置（非交互 fail-closed 拒绝），通过后走 [`execute_quest_tool_batch`]
/// 真实批量执行;`false`(默认)时行为与改动前逐字等价。
pub async fn execute(
    prompt: &str,
    config: &ChimeraConfig,
    json: bool,
    gate: &PermissionGate,
    perm: &PermissionCtx,
    execute: bool,
) -> Result<()> {
    let ctx = crate::composition::build(config)?;
    execute_with_ctx(&ctx, prompt, json, gate, perm, execute).await
}

/// run 命令主体 — 从组合根 AppContext 取共享 bus + engine（M4-P1）
///
/// 迁移要点：原 `EventBus::new() + QuestEngine::new()` 样板删除，bus/engine
/// 一律来自 `ctx`（dispatch 持有 AppContext，命令借用；engine 为 owned，
/// 经 `&ctx.engine` 借用调用 `create_quest(&self)`）。
///
/// `execute`(T1-3 + ADR-247):`true` 时在**任何副作用之前**（未建 Quest、未发
/// 事件）做授权前置——非交互 `Unknown → Deny` 返回 [`execute_refusal`]（退出码
/// 2、稳定前缀、不读 stdin），交互 `Unknown` 走 confirm prompt，仅 `Allow` 才
/// 继续;通过后 Quest 分解结果经 [`execute_quest_tool_batch`] 真实批量执行
/// （F-13 首个生产调用方），结果按 ADR-060 矩阵归因退出码。
pub async fn execute_with_ctx(
    ctx: &AppContext,
    prompt: &str,
    json: bool,
    gate: &PermissionGate,
    perm: &PermissionCtx,
    execute: bool,
) -> Result<()> {
    // ADR-247 D1:授权前置必须在 create_quest 之前（拒绝路径零副作用——
    // 既有测试锁定共享 bus 上零 Quest 事件）。授权点全命令唯一（E-1）。
    if execute {
        ensure_execute_authorized(gate, perm, prompt).await?;
    }

    tracing::info!(prompt = %prompt, "收到单次任务");

    // 1. 共享 bus/engine 来自组合根（C12）。WHY 非 with_checkpoints:CLI 单次
    //    运行无需持久化,且 ~/.chimera 目录可能不存在导致 CheckpointManager 初始化失败。
    //    WHY bus 引用:call_with_budget 需持 bus 发布 OperationTimedOut
    //    事件(M2 本层超时兜底,engine 只消费不持有发布权)。
    let bus = &ctx.bus;
    let engine = &ctx.engine;

    // 2. 封装 UserIntent(UUIDv7 时间有序,便于审计追溯)
    let intent = UserIntent {
        intent_id: format!("intent-{}", Uuid::now_v7()),
        raw_text: prompt.to_string(),
        multimodal_inputs: vec![MultimodalInput::Text(prompt.to_string())],
        risk_level: 0,
    };

    // 3. 真实 L9 分解:query → UserIntent → Quest
    //    create_quest 内部会广播 QuestCreated 事件——经共享 bus 对 dispatch 级
    //    订阅者可见(M4-P1 前:私有 bus,事件零可见)。
    //    M2:经 call_with_budget 注入本层超时(120s,对齐 mca-gateway per-endpoint
    //    口径)——绕过 gateway 时 create_quest 挂起不再永久阻塞;超时发
    //    OperationTimedOut 事件并映射为既有 Timeout 退出码 6(纯增量防护,
    //    不收紧正常路径行为)。token 取进程级取消根（ADR-270）：Ctrl+C 即取消。
    let token = crate::call_budget::cancel_root().clone();
    let quest = call_with_budget(
        engine.create_quest(intent),
        DEFAULT_CALL_BUDGET,
        &token,
        "run.create_quest",
        bus,
    )
    .await
    .map_err(call_budget_to_cli_error)?
    .map_err(|e| ChimeraCliError::EngineError(build_error_reply(&e)))?;

    // 4. 输出 / 执行
    if execute {
        // ADR-247(F-02/F-13):授权已通过——Quest 分解结果映射为 text.echo 批量,
        // 经组合根唯一 ToolRuntime 有界并发派发(F-13 首个生产调用方),结果按
        // ADR-060 矩阵归因退出码。W3 起逐批归因统一由 ToolBatchAttributed 承载,
        // 本处 tracing::info! 为过渡态(不新增事件变体,棘轮余量 I2 顶格)。
        let outcome = execute_quest_tool_batch(ctx, &quest).await?;
        report_batch_outcome(&outcome, json)?;
        batch_outcome_to_cli_error(&outcome)?;
        return Ok(());
    }
    if json {
        // JSON 模式:输出 Quest 结构的 envelope(机器可读)
        output::print_json(&quest)?;
    } else {
        // 人类可读模式:逐字符流式输出回复文本 + [done] 标记
        let reply = build_quest_reply(&quest);
        stream_to_stdout(&reply).await;
        println!("[done]");
    }

    Ok(())
}

/// 逐字符将回复文本流式打印到 stdout(SubTask 1.1.2)
///
/// WHY 独立 async fn:chunk_delay 涉及 `tokio::time::sleep`,需在 async 上下文执行。
/// 测试可通过 `CHIMERA_RUN_CHUNK_DELAY_MS=0` 环境变量禁用延迟以加速。
async fn stream_to_stdout(reply: &str) {
    let delay_ms = std::env::var("CHIMERA_RUN_CHUNK_DELAY_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_CHUNK_DELAY_MS);
    let delay = Duration::from_millis(delay_ms);

    // 复用 orchestrator::plan_chunks 保证 CLI 与 TUI 切分逻辑一致
    use std::io::Write;
    let stdout = std::io::stdout();
    for delta in plan_chunks(reply) {
        // WHY 锁作用域只包住单次写入:`StdoutLock` 是**进程级全局锁**，持它跨
        // `sleep().await` 会让其他任务（tracing 输出、进度提示）在整段流式
        // 节奏期间全部排队。写完即放，把 await 留在锁外。
        {
            let mut lock = stdout.lock();
            // WHY write! 而非 print!:逐字符 flush,保证管道实时可见
            let _ = write!(lock, "{delta}");
            let _ = lock.flush();
        }
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }
    // 末尾换行(与 [done] 标记分行)
    let mut lock = stdout.lock();
    let _ = writeln!(lock);
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::composition;
    use crate::permission::{PermissionCtx, PermissionGate};
    use event_bus::NexusEvent;

    /// M4-P1：run 经 execute_with_ctx 走组合根共享 bus ——
    /// QuestCreated 对外部订阅者可见（此前命令自建私有 bus，事件零可见）。
    /// 此测试先行失败（红）：execute_with_ctx 尚不存在。
    #[tokio::test]
    async fn run_events_visible_on_shared_context_bus() {
        let ctx = composition::build(&ChimeraConfig::default()).expect("装配应成功");
        // §4.4 反模式 3：先 subscribe 再驱动命令，否则事件静默丢失
        let mut rx = ctx.bus.subscribe();
        let perm = PermissionCtx::default();
        let gate = PermissionGate::new(false);
        execute_with_ctx(&ctx, "分析依赖并给出优化建议", false, &gate, &perm, false)
            .await
            .expect("run 正常路径应成功");
        let mut saw_quest_created = false;
        // 超时轮询：命令已同步完成，事件在 broadcast 缓冲中等待接收
        for _ in 0..40 {
            match tokio::time::timeout(Duration::from_millis(50), rx.recv()).await {
                Ok(Ok(NexusEvent::QuestCreated { .. })) => {
                    saw_quest_created = true;
                    break;
                }
                Ok(Ok(_)) => continue, // 其他事件跳过
                _ => break,
            }
        }
        assert!(
            saw_quest_created,
            "QuestCreated 应在组合根共享 bus 上可见（命令不再自建私有 bus）"
        );
    }

    /// T1-3：`--execute` 未启用时的拒绝必须明确、可测试（非静默 no-op）。
    #[test]
    fn execute_flag_returns_refusal_without_side_effects() {
        assert!(
            EXECUTE_REFUSAL_MSG.starts_with("execute_not_enabled:"),
            "拒绝文案需带稳定前缀，便于脚本 grep：{EXECUTE_REFUSAL_MSG}"
        );
        assert!(
            EXECUTE_REFUSAL_MSG.contains("sandbox"),
            "拒绝文案需指明沙箱派发链这一根因"
        );
        assert!(
            EXECUTE_REFUSAL_MSG.contains("composition root"),
            "Adr-244 后必须说明执行器已在组合根装配，避免误导为「缺执行器」"
        );
        assert!(
            EXECUTE_REFUSAL_MSG.contains("chimera exec --plan"),
            "拒绝文案需给出真实可执行路径"
        );
        assert!(
            EXECUTE_REFUSAL_MSG.contains("without --execute"),
            "拒绝文案需给出可执行的替代路径"
        );
        let err = execute_refusal();
        assert_eq!(err.exit_code_value(), 2, "NotImplemented 退出码应为 2");
        assert!(matches!(err, ChimeraCliError::NotImplemented(_)));
    }

    /// T1-3：`execute=true` 必须在任何副作用之前拒绝 —— 共享 bus 上零 Quest 事件。
    #[tokio::test]
    async fn execute_path_refuses_before_side_effects() {
        let ctx = composition::build(&ChimeraConfig::default()).expect("装配应成功");
        // §4.4 反模式 3：先 subscribe 再驱动命令，否则事件静默丢失
        let mut rx = ctx.bus.subscribe();
        let perm = PermissionCtx::default();
        let gate = PermissionGate::new(false);
        let result = execute_with_ctx(&ctx, "执行高风险工具链", false, &gate, &perm, true).await;
        let err = result.expect_err("--execute 应被拒绝");
        let cli_err = err
            .downcast_ref::<ChimeraCliError>()
            .expect("拒绝错误应为 ChimeraCliError");
        assert_eq!(cli_err.exit_code_value(), 2);
        assert!(matches!(cli_err, ChimeraCliError::NotImplemented(_)));
        let mut saw_quest_created = false;
        for _ in 0..20 {
            match tokio::time::timeout(Duration::from_millis(10), rx.recv()).await {
                Ok(Ok(NexusEvent::QuestCreated { .. })) => {
                    saw_quest_created = true;
                    break;
                }
                Ok(Ok(_)) => continue, // 其他事件跳过
                _ => break,
            }
        }
        assert!(
            !saw_quest_created,
            "拒绝须发生在 create_quest 之前，不得产生 Quest 事件"
        );
    }

    /// T1-3：`--execute` 必须被 clap 解析进 `Commands::Run.execute`（默认 false）。
    #[test]
    fn cli_parses_run_execute_flag() {
        use clap::Parser;
        let cli = crate::cli::Cli::try_parse_from(["chimera", "run", "--execute", "hi"])
            .expect("run --execute 应解析成功");
        match cli.command {
            Some(crate::cli::Commands::Run { execute, .. }) => assert!(execute),
            _ => panic!("应解析为 Run 命令"),
        }
        let cli = crate::cli::Cli::try_parse_from(["chimera", "run", "hi"])
            .expect("run 无 flag 应解析成功");
        match cli.command {
            Some(crate::cli::Commands::Run { execute, .. }) => assert!(!execute),
            _ => panic!("应解析为 Run 命令"),
        }
    }

    /// M4-P1 兼容层回归：原签名 execute wrapper 行为不变 ——
    /// 独立调用场景（库调用方/既有测试）经组合根 ephemeral 装配，正常路径仍 Ok。
    #[tokio::test]
    async fn run_wrapper_normal_path_unchanged() {
        let config = ChimeraConfig::default();
        let perm = PermissionCtx::default();
        let gate = PermissionGate::new(false);
        execute(
            "分解需求：设计评审流程",
            &config,
            false,
            &gate,
            &perm,
            false,
        )
        .await
        .expect("wrapper 正常路径应成功");
    }
}
