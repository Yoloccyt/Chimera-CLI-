//! `chimera quest <action>` — Quest 管理,真实接入 L9 QuestEngine API
//!
//! v2.9.0-omega Task 1.2:替换 NotImplemented 占位,真实调用 QuestEngine 方法。
//!
//! # 子命令
//! - `quest list` — 列出所有 Quest(表格 / JSON)
//! - `quest show <id>` — 查看 Quest 详情(人类可读 / JSON)
//! - `quest cancel <id>` — 取消 Quest(触发 permission prompt,除非 `--yes`)
//! - `quest checkpoint <id>` — 创建检查点,输出 checkpoint_id
//!
//! # 设计决策(WHY)
//! - **进程内 ephemeral 引擎**:与 `chimera run` 一致,每次调用创建独立 QuestEngine。
//!   这意味着 `quest list` 在新进程中返回空列表(Quest 注册表不跨进程持久化)。
//!   真实 Quest 管理请用 `chimera tui`(长生命周期引擎)。CLI quest 命令主要用于:
//!   1. 脚本化场景:配合 `chimera run` 的输出 Quest ID 管道传递
//!   2. 检查点管理:对已持久化的 Quest 创建检查点
//!   3. 测试与诊断:验证 QuestEngine API 可达性
//! - **`quest cancel` 的 idempotent 语义**:QuestEngine::cancel_quest 对不存在的
//!   quest_id 视为幂等成功(见 engine.rs:745 注释),不会返回错误。
//!
//! v2.9.0-omega Task 1.7:接受 `json` flag(成功 envelope 在各子命令输出)
//! v2.9.0-omega Task 1.11:`quest cancel` 调用 `confirm` 触发 permission prompt

use anyhow::Result;
use event_bus::EventBus;
use quest_engine::QuestEngine;

use crate::cli::QuestAction;
use crate::config::ChimeraConfig;
use crate::error::ChimeraCliError;
use crate::output;
use crate::permission::{self, PermissionCtx};

/// 执行 quest 子命令 — 真实接入 QuestEngine API
///
/// `json` flag(Task 1.7):`true` 时各子命令输出 JSON envelope。
///
/// `perm`(Task 1.11):仅 `quest cancel` 消费,用于破坏性操作前确认。
///
/// `dry_run`(Task 2.2):仅 `quest cancel` 消费,`true` 时只输出预览不执行。
pub async fn execute(
    action: &QuestAction,
    _config: &ChimeraConfig,
    json: bool,
    perm: &PermissionCtx,
    dry_run: bool,
) -> Result<()> {
    tracing::info!(?action, dry_run, "Quest 管理操作");

    // 构造进程内 ephemeral QuestEngine(与 chimera run 一致的设计)
    let bus = EventBus::new();
    let engine = QuestEngine::new(bus);

    match action {
        QuestAction::List => list_quests(&engine, json).await,
        QuestAction::Show { id } => show_quest(&engine, id, json).await,
        QuestAction::Cancel { id } => cancel_quest(&engine, id, perm, json, dry_run).await,
        QuestAction::Checkpoint { id } => checkpoint_quest(&engine, id, json).await,
    }
}

/// `quest list` — 列出所有 Quest(SubTask 1.2.2)
///
/// 默认表格输出(`comfy-table`),`--json` 时输出 JSON 数组 envelope。
async fn list_quests(engine: &QuestEngine, json: bool) -> Result<()> {
    let quests = engine.list_quests();

    if json {
        // JSON envelope: { "status": "ok", "data": [...] }
        output::print_json(&quests)?;
    } else if quests.is_empty() {
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
}

/// `quest show <id>` — 查看 Quest 详情(SubTask 1.2.3)
///
/// 默认人类可读格式,`--json` 时输出 Quest 结构 JSON。
/// 不存在时返回 EngineError(退出码 3)。
async fn show_quest(engine: &QuestEngine, id: &str, json: bool) -> Result<()> {
    match engine.get_quest(id) {
        Some(quest) => {
            if json {
                output::print_json(&quest)?;
            } else {
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
            }
            Ok(())
        }
        None => Err(ChimeraCliError::EngineError(format!("Quest 不存在: {id}")).into()),
    }
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

    if json {
        // JSON envelope:返回取消确认
        let payload = serde_json::json!({
            "quest_id": id,
            "cancelled": true,
            "requested_by": "chimera-cli",
        });
        output::print_json(&payload)?;
    } else {
        output::print_success(&format!("Quest {id} 已取消"));
    }
    Ok(())
}

/// `quest checkpoint <id>` — 创建检查点(SubTask 1.2.5)
///
/// 调用 QuestEngine::save_checkpoint,输出 checkpoint_id。
/// QuestEngine::new() 未配置 CheckpointManager,返回 CheckpointSaveFailed 错误。
/// 需要检查点功能时,应使用 `chimera tui`(长生命周期引擎 + 检查点持久化)。
async fn checkpoint_quest(engine: &QuestEngine, id: &str, json: bool) -> Result<()> {
    match engine.save_checkpoint(id).await {
        Ok(checkpoint) => {
            if json {
                output::print_json(&checkpoint)?;
            } else {
                output::print_success(&format!(
                    "Quest {id} 检查点已创建: {}",
                    checkpoint.checkpoint_id
                ));
            }
            Ok(())
        }
        Err(e) => Err(ChimeraCliError::EngineError(format!("检查点创建失败: {e}")).into()),
    }
}
