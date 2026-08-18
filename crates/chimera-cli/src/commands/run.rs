//! `chimera run <prompt>` — 单次任务运行,真实接入 L9 QuestEngine
//!
//! v2.9.0-omega Task 1.1:替换 NotImplemented 占位,真实调用 QuestEngine 分解任务。
//!
//! # 流程
//! 1. 构造进程内 EventBus + QuestEngine(ephemeral,无持久化)
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
use event_bus::EventBus;
use nexus_core::{MultimodalInput, UserIntent};
use quest_engine::QuestEngine;
use uuid::Uuid;

use crate::config::ChimeraConfig;
use crate::error::ChimeraCliError;
use crate::orchestrator::{build_error_reply, build_quest_reply, plan_chunks, OrchestratorConfig};
use crate::output;
use crate::permission::PermissionCtx;

/// 流式 chunk 之间的延迟(制造逐字符浮现观感)
///
/// WHY 固定 20ms:与 TUI 编排器默认值一致(OrchestratorConfig::default().chunk_delay),
/// 每 tick 约 12 字符,视觉流畅且不拖慢脚本消费。测试场景可通过环境变量
/// `CHIMERA_RUN_CHUNK_DELAY_MS=0` 设为零延迟(仅 CLI run 路径生效)。
///
/// W8 清理: chat.rs 原同值常量去重,统一引用本定义(单点维护)
pub(crate) const DEFAULT_CHUNK_DELAY_MS: u64 = 20;

/// 执行 run 命令 — 真实接入 QuestEngine 分解任务并流式输出
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
    _config: &ChimeraConfig,
    json: bool,
    _perm: &PermissionCtx,
) -> Result<()> {
    tracing::info!(prompt = %prompt, "收到单次任务");

    // 1. 构造进程内 ephemeral EventBus + QuestEngine
    //    WHY new() 而非 with_checkpoints:CLI 单次运行无需持久化,
    //    且 ~/.chimera 目录可能不存在导致 CheckpointManager 初始化失败。
    let bus = EventBus::new();
    let engine = QuestEngine::new(bus);

    // 2. 封装 UserIntent(UUIDv7 时间有序,便于审计追溯)
    let intent = UserIntent {
        intent_id: format!("intent-{}", Uuid::now_v7()),
        raw_text: prompt.to_string(),
        multimodal_inputs: vec![MultimodalInput::Text(prompt.to_string())],
        risk_level: 0,
    };

    // 3. 真实 L9 分解:query → UserIntent → Quest
    //    create_quest 内部会广播 QuestCreated 事件,但本进程无订阅者,
    //    publish 失败被容忍(engine 内部用 `?` 传播,EventBus 无订阅者时 publish 成功)。
    let quest = engine
        .create_quest(intent)
        .await
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
async fn stream_to_stdout(reply: &str) {
    let delay_ms = std::env::var("CHIMERA_RUN_CHUNK_DELAY_MS")
        .ok()
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(DEFAULT_CHUNK_DELAY_MS);
    let delay = Duration::from_millis(delay_ms);

    // 复用 orchestrator::plan_chunks 保证 CLI 与 TUI 切分逻辑一致
    use std::io::Write;
    let stdout = std::io::stdout();
    let mut lock = stdout.lock();
    for delta in plan_chunks(reply) {
        // WHY write! 而非 print!:逐字符 flush,保证管道实时可见
        let _ = write!(lock, "{delta}");
        let _ = lock.flush();
        if !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
    }
    // 末尾换行(与 [done] 标记分行)
    let _ = writeln!(lock);
}

/// 暴露 OrchestratorConfig 供测试配置 chunk_delay
///
/// WHY pub(crate):仅 chimera-cli 内部测试需要零延迟配置,不暴露到公开 API。
#[allow(dead_code)]
pub(crate) fn test_config() -> OrchestratorConfig {
    OrchestratorConfig {
        chunk_delay: Duration::ZERO,
    }
}
