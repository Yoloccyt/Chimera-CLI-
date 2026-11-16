//! `chimera quest <action>` — Quest 管理,真实接入 L9 QuestEngine API
//!
//! v2.9.0-omega Task 1.2:替换 NotImplemented 占位,真实调用 QuestEngine 方法。
//!
//! # 子命令
//! - `quest list` — 列出所有 Quest(表格 / JSON)
//! - `quest show <id>` — 查看 Quest 详情(人类可读 / JSON)
//! - `quest cancel <id>` — 取消 Quest(触发 permission prompt,除非 `--yes`)
//! - `quest checkpoint <id>` — 创建检查点,输出 checkpoint_id
//! - `quest execute` — 执行任务计划(Phase 5,默认关闭:`CHIMERA_QUEST_EXEC=apply`)
//!
//! # 设计决策(WHY)
//! - **共享组合根引擎**:与 `chimera run` 一致,经 dispatch 共享 `ctx.engine`
//!   (进程内装配；注册表不跨进程持久化)。
//!   这意味着 `quest list` 在新进程中返回空列表(Quest 注册表不跨进程持久化)。
//!   真实 Quest 管理请用 `chimera tui`(长生命周期引擎)。CLI quest 命令主要用于:
//!   1. 脚本化场景:配合 `chimera run` 的输出 Quest ID 管道传递
//!   2. 检查点管理:对已持久化的 Quest 创建检查点
//!   3. 测试与诊断:验证 QuestEngine API 可达性
//! - **`quest cancel` 的 idempotent 语义**:QuestEngine::cancel_quest 对不存在的
//!   quest_id 视为幂等成功(见 `QuestEngine::cancel_quest` 的幂等语义注释),不会返回错误。
//!
//! v2.9.0-omega Task 1.7:接受 `json` flag(成功 envelope 在各子命令输出)
//! v2.9.0-omega Task 1.11:`quest cancel` 调用 `confirm` 触发 permission prompt

use anyhow::Result;
use quest_engine::QuestEngine;
use serde::Serialize;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::cli::QuestAction;
use crate::composition::AppContext;
use crate::config::ChimeraConfig;
use crate::error::ChimeraCliError;
use crate::output;
use crate::permission::{self, PermissionCtx};
use crate::quest_exec::{JsonPlanPlanner, QuestExecConfig, QuestExecMode, QuestExecutor};

/// 执行 quest 子命令 — 兼容层 thin wrapper（M4-P1）
///
/// 独立调用场景（库调用方/既有测试）经组合根装配 ephemeral AppContext，
/// 语义与迁移前"进程内 ephemeral 引擎"一致。dispatch 主链直接调
/// [`execute_with_ctx`] 共享 dispatch 级 AppContext（C12 唯一装配点）。
///
/// `json` flag(Task 1.7):`true` 时各子命令输出 JSON envelope。
///
/// `perm`(Task 1.11):仅 `quest cancel` 消费,用于破坏性操作前确认。
///
/// `dry_run`(Task 2.2):仅 `quest cancel` 消费,`true` 时只输出预览不执行。
pub async fn execute(
    action: &QuestAction,
    config: &ChimeraConfig,
    json: bool,
    perm: &PermissionCtx,
    dry_run: bool,
) -> Result<()> {
    let ctx = crate::composition::build(config)?;
    execute_with_ctx(&ctx, action, json, perm, dry_run).await
}

/// quest 子命令主体 — 从组合根 AppContext 取共享 engine（M4-P1）
pub async fn execute_with_ctx(
    ctx: &AppContext,
    action: &QuestAction,
    json: bool,
    perm: &PermissionCtx,
    dry_run: bool,
) -> Result<()> {
    tracing::info!(?action, dry_run, "Quest 管理操作");

    // 共享 engine 来自组合根（与 chimera run 同源的 ephemeral 设计）：
    // 每次进程装配独立 QuestEngine，注册表不跨进程持久化。
    let engine = &ctx.engine;

    match action {
        QuestAction::List => list_quests(engine, json).await,
        QuestAction::Show { id } => show_quest(engine, id, json).await,
        QuestAction::Cancel { id } => cancel_quest(engine, id, perm, json, dry_run).await,
        QuestAction::Checkpoint { id } => checkpoint_quest(engine, id, json).await,
        QuestAction::Execute {
            plans,
            id,
            prompt,
            strict,
        } => {
            if dry_run {
                eprintln!(
                    "[dry-run] 将执行 Quest 计划 {plans}(id={id:?} prompt={prompt:?}),不执行"
                );
                return Ok(());
            }
            execute_quest(
                ctx,
                plans,
                id.as_deref(),
                prompt.as_deref(),
                *strict,
                perm,
                json,
            )
            .await
        }
    }
}

/// `quest list` — 列出所有 Quest(SubTask 1.2.2)
///
/// 默认表格输出(`comfy-table`),`--json` 时输出 JSON 数组 envelope。
async fn list_quests(engine: &QuestEngine, json: bool) -> Result<()> {
    let quests = engine.list_quests();

    output::emit(json, &quests, || {
        if quests.is_empty() {
            // 空列表友好提示(到 stderr,不污染 stdout 数据流)
            output::print_info("当前无 Quest(进程内 ephemeral 引擎,不持久化)");
        } else {
            // 表格输出:ID / 标题 / 任务数 / 思考模式 / 优先级
            let rows: Vec<Vec<String>> = quests
                .iter()
                .map(|q| {
                    vec![
                        q.quest_id.clone(),
                        q.title.clone(),
                        q.tasks.len().to_string(),
                        format!("{:?}", q.thinking_mode),
                        q.priority.to_string(),
                    ]
                })
                .collect();
            output::print_table(&["ID", "标题", "任务数", "思考模式", "优先级"], &rows);
        }
        Ok(())
    })
}

/// `quest show <id>` — 查看 Quest 详情(SubTask 1.2.3)
///
/// 默认人类可读格式,`--json` 时输出 Quest 结构 JSON。
/// 不存在时返回 EngineError(退出码 3)。
async fn show_quest(engine: &QuestEngine, id: &str, json: bool) -> Result<()> {
    match engine.get_quest(id) {
        Some(quest) => output::emit(json, &quest, || {
            // 人类可读:逐行打印 Quest 字段 + 任务列表
            println!("Quest ID: {}", quest.quest_id);
            println!("标题: {}", quest.title);
            println!("思考模式: {:?}", quest.thinking_mode);
            println!("优先级: {}", quest.priority);
            if let Some(cp) = &quest.checkpoint_id {
                println!("最近检查点: {}", cp);
            }
            println!("任务列表 ({}):", quest.tasks.len());
            for (i, task) in quest.tasks.iter().enumerate() {
                println!(
                    "  {}. [{:?}] {} (依赖: {:?})",
                    i + 1,
                    task.status,
                    task.description,
                    task.dependencies
                );
            }
            Ok(())
        }),
        None => Err(ChimeraCliError::EngineError(format!("Quest 不存在: {id}")).into()),
    }
}

/// `quest cancel` 的 JSON 载荷（typed，字段名编译期校验）。
#[derive(Debug, Serialize)]
struct CancelPayload<'a> {
    quest_id: &'a str,
    cancelled: bool,
    requested_by: &'static str,
}

/// `quest cancel <id>` — 取消 Quest(SubTask 1.2.4)
///
/// 触发 permission prompt(除非 `--yes` / `--no-permission`)。
/// 取消后输出确认信息。QuestEngine::cancel_quest 对不存在的 quest_id 幂等成功。
///
/// `dry_run=true`(Task 2.2):permission 确认后只输出预览,不调用 cancel_quest API。
async fn cancel_quest(
    engine: &QuestEngine,
    id: &str,
    perm: &PermissionCtx,
    json: bool,
    dry_run: bool,
) -> Result<()> {
    // Task 1.11.4:破坏性操作前调用 confirm
    let confirmed = permission::confirm(perm, "取消 Quest", &format!("Quest ID: {id}")).await?;
    if !confirmed {
        return Err(ChimeraCliError::PermissionDenied(format!("用户拒绝取消 Quest {id}")).into());
    }

    // Task 2.2:dry-run 模式只输出预览,不实际执行
    // WHY 在 permission 之后:确保预览前仍经过权限确认,避免绕过安全检查
    if dry_run {
        eprintln!("[dry-run] 将取消 Quest {id},不执行");
        return Ok(());
    }

    // 真实调用 QuestEngine::cancel_quest(requested_by 标识来源为 CLI)
    engine
        .cancel_quest(id, "chimera-cli")
        .await
        .map_err(|e| ChimeraCliError::EngineError(e.to_string()))?;

    // JSON envelope:返回取消确认；双通道统一走 emit。
    let payload = CancelPayload {
        quest_id: id,
        cancelled: true,
        requested_by: "chimera-cli",
    };
    output::emit(json, &payload, || {
        output::print_success(&format!("Quest {id} 已取消"));
        Ok(())
    })
}

/// `quest checkpoint <id>` — 创建检查点(SubTask 1.2.5)
///
/// 调用 QuestEngine::save_checkpoint,输出 checkpoint_id。
/// QuestEngine::new() 未配置 CheckpointManager,返回 CheckpointSaveFailed 错误。
/// 需要检查点功能时,应使用 `chimera tui`(长生命周期引擎 + 检查点持久化)。
async fn checkpoint_quest(engine: &QuestEngine, id: &str, json: bool) -> Result<()> {
    match engine.save_checkpoint(id).await {
        Ok(checkpoint) => output::emit(json, &checkpoint, || {
            output::print_success(&format!(
                "Quest {id} 检查点已创建: {}",
                checkpoint.checkpoint_id
            ));
            Ok(())
        }),
        Err(e) => Err(ChimeraCliError::EngineError(format!("检查点创建失败: {e}")).into()),
    }
}

/// `quest execute` — Phase 5 任务执行面入口（默认关闭）
///
/// WHY 自建 ephemeral `Arc<QuestEngine>`：CLI 是一次性进程，Quest 注册表不跨进程
/// 持久化（见模块头），故 `--prompt` 是唯一能造出 Quest 的路径；`--id` 保留给长生命
/// 周期宿主（TUI / app-server）复用本函数时寻址既有 Quest —— 本进程查不到就诚实报错，
/// 不做任何"假装找到"的降级。
#[allow(clippy::too_many_arguments)] // CLI adapter: mirrors clap flags 1:1
async fn execute_quest(
    ctx: &AppContext,
    plans_path: &str,
    id: Option<&str>,
    prompt: Option<&str>,
    strict: bool,
    perm: &PermissionCtx,
    json: bool,
) -> Result<()> {
    // 1. 灰度门（默认 Off —— 未显式启用绝不起子进程/不动工具面）
    let cfg = QuestExecConfig::from_env();
    if cfg.mode == QuestExecMode::Off {
        return Err(ChimeraCliError::ConfigError(
            "quest execute 默认关闭：设 CHIMERA_QUEST_EXEC=apply 启用；\
多根并行另需 CHIMERA_QUEST_DAG=multi_root"
                .to_string(),
        )
        .into());
    }

    // 2. 计划表（task_id -> ToolPlan）
    let planner = JsonPlanPlanner::from_path(Path::new(plans_path)).map_err(|error| {
        ChimeraCliError::ConfigError(format!("计划文件 {plans_path} 不可用: {error}"))
    })?;

    // 3. ephemeral 引擎：dag_mode 必须透传给 decompose，否则 MultiRoot 只放开了并发窗口
    //    而任务仍是伪链（永远只有 1 个就绪），等于窗口形同虚设。
    //    装配走组合根 `build_quest_engine`（ADR-231 D1）：QuestEngine 的构造点必须
    //    只有 composition.rs 一处，否则"进程里有几台引擎"重新变成不可数的事实。
    let engine = crate::composition::build_quest_engine(ctx.bus.clone(), cfg.dag_mode);
    let quest_id = match (id, prompt) {
        (Some(existing), _) => existing.to_string(),
        (None, Some(text)) => {
            let intent = nexus_contracts::domain::UserIntent {
                intent_id: next_intent_id(),
                raw_text: text.to_string(),
                multimodal_inputs: vec![],
                // 计划由用户显式提供（非模型生成），风险取低档；副作用仍由工具面把关
                risk_level: 30,
            };
            engine
                .create_quest(intent)
                .await
                .map_err(|error| ChimeraCliError::EngineError(format!("Quest 创建失败: {error}")))?
                .quest_id
        }
        (None, None) => {
            return Err(ChimeraCliError::ConfigError(
                "必须提供 --prompt <文本> 或 --id <quest-id>".to_string(),
            )
            .into())
        }
    };

    // 4. --strict：缺计划即红（默认跳过，让"补计划后重跑同一 Quest"成立）
    if strict {
        let quest = engine
            .get_quest(&quest_id)
            .ok_or_else(|| ChimeraCliError::EngineError(format!("Quest 不存在: {quest_id}")))?;
        let unplanned: Vec<&str> = quest
            .tasks
            .iter()
            .filter(|t| !planner.contains(&t.task_id))
            .map(|t| t.task_id.as_str())
            .collect();
        if !unplanned.is_empty() {
            return Err(ChimeraCliError::ConfigError(format!(
                "--strict: 任务无计划: {unplanned:?}（补齐 {plans_path} 或去掉 --strict 按跳过处理）"
            ))
            .into());
        }
    }

    // 5. 破坏性操作前确认（与 `quest cancel` 同口径：permission → dry-run → 执行）
    let details = format!(
        "quest={quest_id}, plans={plans_path}, dag={:?}, tasks={}",
        cfg.dag_mode,
        engine.get_quest(&quest_id).map_or(0, |q| q.tasks.len())
    );
    if !permission::confirm(perm, "执行 Quest 任务计划", &details).await? {
        return Err(
            ChimeraCliError::PermissionDenied(format!("用户拒绝执行 Quest {quest_id}")).into(),
        );
    }

    // 6. 执行（限额与开关按 env 装配；组合根侧 build_tool_runtime 不收总线，ADR-229 D5）
    let runtime = crate::composition::build_tool_runtime(&ChimeraConfig::default())
        .map_err(|error| ChimeraCliError::EngineError(format!("工具执行面装配失败: {error}")))?;
    let executor = QuestExecutor::new(
        Arc::clone(&engine),
        runtime,
        Arc::new(planner),
        Arc::clone(&ctx.sched),
        cfg,
    );
    let report = executor
        .run(&quest_id, CancellationToken::new())
        .await
        .map_err(|error| ChimeraCliError::EngineError(format!("Quest 执行失败: {error}")))?;

    // 7. 输出（typed payload 走 emit，人类可读走 print_success）
    output::emit(json, &report, || {
        output::print_success(&format!(
            "Quest {quest_id} 执行完成: completed={} failed={} skipped={} cancelled={} ({}ms)",
            report.completed, report.failed, report.skipped, report.cancelled, report.duration_ms
        ));
        Ok(())
    })
}

/// 进程内唯一 intent_id —— WHY 不引 uuid 依赖：一次性 CLI 路径，pid + 原子序号足够
fn next_intent_id() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(1);
    format!(
        "intent-{}-{}",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::SeqCst)
    )
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

    /// M4-P1：quest 子命令经 execute_with_ctx 使用组合根共享 engine/bus ——
    /// 共享 bus 来自 build() 标准装配（C3 旁路订阅者已注册）。
    /// 此测试先行失败（红）：execute_with_ctx 尚不存在。
    #[tokio::test]
    async fn quest_list_runs_on_shared_context() {
        let ctx = composition::build(&ChimeraConfig::default()).expect("装配应成功");
        let perm = PermissionCtx::default();
        execute_with_ctx(&ctx, &QuestAction::List, true, &perm, false)
            .await
            .expect("quest list 应成功");
        assert!(
            ctx.bus.has_critical_subscribers(),
            "共享 bus 应已注册 Critical 旁路订阅者（M4-P2 标准装配步骤）"
        );
    }

    /// M4-P1：取消不存在的 Quest 在共享引擎上保持幂等成功语义（行为无感回归）。
    #[tokio::test]
    async fn quest_cancel_nonexistent_idempotent_on_shared_context() {
        let ctx = composition::build(&ChimeraConfig::default()).expect("装配应成功");
        // --yes 跳过 permission prompt（非交互测试环境）
        let perm = PermissionCtx {
            yes: true,
            no_permission: false,
        };
        execute_with_ctx(
            &ctx,
            &QuestAction::Cancel {
                id: "quest-nonexistent".into(),
            },
            true,
            &perm,
            false,
        )
        .await
        .expect("cancel 不存在的 Quest 应幂等成功");
    }

    /// M4-P1 兼容层回归：原签名 execute wrapper 行为不变。
    #[tokio::test]
    async fn quest_wrapper_normal_path_unchanged() {
        let config = ChimeraConfig::default();
        let perm = PermissionCtx::default();
        execute(&QuestAction::List, &config, true, &perm, false)
            .await
            .expect("wrapper 正常路径应成功");
    }
}
