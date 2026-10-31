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
use nexus_core::{MultimodalInput, UserIntent};
use uuid::Uuid;

use crate::call_budget::{call_with_budget, CallBudgetError, DEFAULT_CALL_BUDGET};
use crate::composition::AppContext;
use crate::config::ChimeraConfig;
use crate::error::ChimeraCliError;
use crate::orchestrator::{build_error_reply, build_quest_reply};
use crate::output;
use crate::permission::PermissionCtx;
use tokio_util::sync::CancellationToken;

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

/// 执行 run 命令 — 兼容层 thin wrapper（M4-P1）
///
/// 独立调用场景（库调用方/既有测试）经组合根装配 ephemeral AppContext，
/// 语义与迁移前"进程内 ephemeral 引擎"一致。dispatch 主链不走本 wrapper，
/// 直接调 [`execute_with_ctx`] 共享 dispatch 级 AppContext（C12 唯一装配点）。
///
/// `prompt` 为用户意图原始文本,`config` 为已加载的合并配置。
///
/// `json` flag(Task 1.7):`true` 时输出 Quest 结构的 JSON envelope,
/// 取代逐字符流式文本(便于脚本 `jq` 消费)。
///
/// `perm`(Task 1.11):当前未消费,预留供未来外部命令执行确认
/// (如 `chimera run "执行 rm -rf"` 时调用 `permission::confirm`)。
pub async fn execute(
    prompt: &str,
    config: &ChimeraConfig,
    json: bool,
    _perm: &PermissionCtx,
) -> Result<()> {
    let ctx = crate::composition::build(config)?;
    execute_with_ctx(&ctx, prompt, json, _perm).await
}

/// run 命令主体 — 从组合根 AppContext 取共享 bus + engine（M4-P1）
///
/// 迁移要点：原 `EventBus::new() + QuestEngine::new()` 样板删除，bus/engine
/// 一律来自 `ctx`（dispatch 持有 AppContext，命令借用；engine 为 owned，
/// 经 `&ctx.engine` 借用调用 `create_quest(&self)`）。
pub async fn execute_with_ctx(
    ctx: &AppContext,
    prompt: &str,
    json: bool,
    _perm: &PermissionCtx,
) -> Result<()> {
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
    //    不收紧正常路径行为)。token 为新建未触发令牌,预留全局取消链路接线。
    let token = CancellationToken::new();
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

    // 4. 输出
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
///
/// Phase 6A/T6A.1:写入下沉到 [`stream_to`],批量宽度经
/// `CHIMERA_STREAM_BATCH_CHARS` 控制(默认 1 = 逐字符,与旧行为逐字节相同)。
async fn stream_to_stdout(reply: &str) {
    let delay_ms = std::env::var("CHIMERA_RUN_CHUNK_DELAY_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_CHUNK_DELAY_MS);
    // WHY 借 `Stdout` 句柄而非 `StdoutLock`：句柄每次 write 内部取放锁,
    // 不会把**进程级全局锁**跨 `sleep().await` 持有(旧实现靠逐次 lock 作用域达成)。
    let _ = stream_to(
        reply,
        Duration::from_millis(delay_ms),
        stream_batch_chars_from_env(),
        &mut std::io::stdout(),
    )
    .await;
}

/// stdout 流式输出的默认批宽（1 = 逐字符,保持既有观感与 flush 语义）
///
/// WHY 默认 1 而非 8:事件发布面(H-a `orchestrator`)早已批聚合,但**终端逐字符
/// 打字机节奏是用户可见行为**;批宽只减少 write/flush 次数,总延迟经
/// [`batch_delay`] 摊销保持不变,故启用批量是用户显式选择而非默认变更。
pub const DEFAULT_STREAM_BATCH_CHARS: usize = 1;

/// 每批的 sleep 时长 = 单字符延迟 × 该批字符数（保持总墙钟与逐字符路径一致）
#[must_use]
pub fn batch_delay(per_char_delay: Duration, chars: usize) -> Duration {
    per_char_delay.saturating_mul(chars.max(1) as u32)
}

/// 从 `CHIMERA_STREAM_BATCH_CHARS` 读批宽（非法/0 值回退默认,不 panic）
#[must_use]
pub fn stream_batch_chars_from_env() -> usize {
    std::env::var("CHIMERA_STREAM_BATCH_CHARS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|v| *v > 0)
        .unwrap_or(DEFAULT_STREAM_BATCH_CHARS)
}

/// 按批把 `reply` 写入任意 `sink`,每批一次 write + flush,末尾补一个换行
///
/// # 语义
/// - **字节等价**:`batch=1` 与 `batch>1` 写出的字节完全相同（切分复用
///   `orchestrator::plan_chunks_batched`,按 `char` 而非 byte,多字节安全）;
/// - **节奏不变**:每批 sleep = [`batch_delay`],故总墙钟与逐字符路径相同;
///   `per_char_delay` 为零时完全不 sleep（测试/CI 加速路径）;
/// - **不吞换行**:末尾 `writeln!` 与旧实现一致（只 write,不额外 flush）。
///
/// WHY 泛型 sink:`run` / `chat` 两条流式路径此前各抄一份逐字符循环,
/// 批量语义只能改两处；抽到这里后测试可用内存 sink 断言字节与写次数,
/// 不必捕获进程 stdout。
pub async fn stream_to<W: std::io::Write>(
    reply: &str,
    per_char_delay: Duration,
    batch: usize,
    sink: &mut W,
) -> std::io::Result<()> {
    use crate::orchestrator::plan_chunks_batched;
    for delta in plan_chunks_batched(reply, batch) {
        write!(sink, "{delta}")?;
        sink.flush()?;
        let sleep_for = batch_delay(per_char_delay, delta.chars().count());
        if !sleep_for.is_zero() {
            tokio::time::sleep(sleep_for).await;
        }
    }
    writeln!(sink)?;
    Ok(())
}

// ============================================================
// Phase 7（ADR-232）—— LLM 工具循环入口，默认关闭
// ============================================================

/// 无旗调用点（chat / TUI `agent.chat`）的灰度判据：`CHIMERA_LLM_LOOP=apply|on`
///
/// WHY 不在这里看 `--llm`：`--llm` 是 `run` 的单次旗，chat/TUI 没有该旗；
/// 两条路径共用 [llm_answer]，但启用方式不同（显式旗 vs 环境档）。
#[must_use]
pub fn llm_loop_enabled() -> bool {
    matches!(
        std::env::var("CHIMERA_LLM_LOOP").ok().as_deref(),
        Some("apply") | Some("on")
    )
}

/// 跑一轮 LLM 工具循环，返回循环产出（最终文本 + 计数 + 停止原因）
///
/// WHY 公开给 chat / TUI 复用：三条入口各自抄一份"装配客户端 + 运行时 + 审批"
/// 一定会漂（工具集、审批口径、预算配置三处最容易分叉），收敛成一个函数后
/// 只有输出形态不同。
pub async fn llm_answer(
    bus: &event_bus::EventBus,
    prompt: &str,
    perm: &PermissionCtx,
) -> Result<crate::llm::LlmLoopOutcome> {
    llm_answer_with(bus, prompt, &crate::llm::approval::CliApproval::new(*perm)).await
}

/// [`llm_answer`] 的审批注入版
///
/// WHY 拆两层：CLI/chat 有 stdin 可以用 `CliApproval` 逐条问人，而 TUI 拥有终端
/// 控制权、不能从后台任务读 stdin —— 它必须换一档（当前取 `DenyWrites`，
/// 写工具的 TUI 弹窗审批属后续波次，见 ADR-232 待办）。
pub async fn llm_answer_with(
    bus: &event_bus::EventBus,
    prompt: &str,
    approval: &dyn crate::llm::approval::ToolApproval,
) -> Result<crate::llm::LlmLoopOutcome> {
    use crate::llm::tool_loop::{LlmToolLoop, LlmToolLoopConfig};

    // WHY 默认配置：build_tool_runtime / build_llm_client 的口径都走 env + 内置默认，
    // chat 的 repl_loop 与 TUI 编排器都只持 bus（Arc 廉价共享，同源事件面）
    let config = ChimeraConfig::default();
    let client = crate::composition::build_llm_client(&config)?;
    let runtime = crate::composition::build_tool_runtime(&config)
        .map_err(|e| ChimeraCliError::EngineError(format!("工具执行面装配失败: {e}")))?;
    // 暴露给模型的工具集 = 运行时已注册全集（单一真相源，避免 CLI 侧另列清单）
    let tools = runtime.registry().descriptors();
    let chain = LlmToolLoop::new(client, runtime, bus.clone(), LlmToolLoopConfig::from_env());
    let outcome = chain
        .run(prompt, &tools, &CancellationToken::new(), approval)
        .await
        .map_err(llm_loop_to_cli_error)?;
    Ok(outcome)
}

/// 循环错误 → CLI 错误码映射
///
/// WHY 分流：provider 未配置属"用户可纠正的配置问题"(退出码 1)；
/// 取消沿用既有 `UserCancelled`(4)；其余按引擎故障(3)。
fn llm_loop_to_cli_error(error: crate::llm::LlmLoopError) -> ChimeraCliError {
    use crate::llm::client::{LlmError, LlmLoopError};
    match error {
        LlmLoopError::Cancelled { .. } => ChimeraCliError::UserCancelled,
        LlmLoopError::Client(LlmError::ProviderUnavailable(message)) => {
            ChimeraCliError::ConfigError(format!("LLM provider 不可用: {message}"))
        }
        LlmLoopError::Client(other) => ChimeraCliError::EngineError(other.to_string()),
    }
}

/// `run --llm` 的 JSON 载荷（typed 字段，编译期校验）
#[derive(serde::Serialize)]
struct LlmPayload<'a> {
    /// 模型最终文本
    text: &'a str,
    /// 补全轮数
    turns: usize,
    /// 派发的工具调用数
    tool_calls: usize,
    /// 因审批未通过而未执行的调用数
    denied_tool_calls: usize,
    /// 停止原因（final / max_turns / max_tool_calls / max_repeat / budget）
    stop: &'a str,
}

fn stop_label(stop: crate::llm::LlmLoopStop) -> &'static str {
    use crate::llm::LlmLoopStop as S;
    match stop {
        S::Final => "final",
        S::MaxTurns => "max_turns",
        S::MaxToolCalls => "max_tool_calls",
        S::MaxRepeatToolCalls => "max_repeat_tool_calls",
        S::Budget => "budget",
    }
}

/// `chimera run --llm "..."` 主体
pub async fn execute_llm_with_ctx(
    ctx: &AppContext,
    prompt: &str,
    json: bool,
    perm: &PermissionCtx,
) -> Result<()> {
    tracing::info!(prompt = %prompt, "LLM 工具循环单次任务");
    let outcome = llm_answer(&ctx.bus, prompt, perm).await?;

    if json {
        let payload = LlmPayload {
            text: &outcome.text,
            turns: outcome.turns,
            tool_calls: outcome.tool_calls,
            denied_tool_calls: outcome.denied_tool_calls,
            stop: stop_label(outcome.stop),
        };
        return output::print_json(&payload).map_err(Into::into);
    }

    stream_to_stdout(&outcome.text).await;
    println!("[done]");
    Ok(())
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::composition;
    use crate::permission::PermissionCtx;
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
        execute_with_ctx(&ctx, "分析依赖并给出优化建议", false, &perm)
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

    /// M4-P1 兼容层回归：原签名 execute wrapper 行为不变 ——
    /// 独立调用场景（库调用方/既有测试）经组合根 ephemeral 装配，正常路径仍 Ok。
    #[tokio::test]
    async fn run_wrapper_normal_path_unchanged() {
        let config = ChimeraConfig::default();
        let perm = PermissionCtx::default();
        execute("分解需求：设计评审流程", &config, false, &perm)
            .await
            .expect("wrapper 正常路径应成功");
    }
}
