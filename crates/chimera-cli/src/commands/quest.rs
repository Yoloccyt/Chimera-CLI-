//! `aether quest <action>` — Quest 管理骨架
//!
//! 后续接入 `quest-engine` crate,支持 Quest 的创建/查询/取消/检查点。
//! 当前仅打印动作占位信息。

use anyhow::Result;

use crate::cli::QuestAction;
use crate::config::ChimeraConfig;

/// 执行 quest 子命令
pub async fn execute(action: &QuestAction, config: &ChimeraConfig) -> Result<()> {
    tracing::info!(?action, "Quest 管理操作");
    match action {
        QuestAction::List => {
            println!("[quest list] 列出所有 Quest");
            println!(
                "[quest list] 最大任务数/Quest:{}",
                config.quest.max_tasks_per_quest
            );
        }
        QuestAction::Show { id } => {
            println!("[quest show] 查看 Quest 详情:{}", id);
        }
        QuestAction::Cancel { id } => {
            println!("[quest cancel] 取消 Quest:{}", id);
            println!(
                "[quest cancel] (将触发检查点保存,间隔 {} 次操作)",
                config.quest.checkpoint_interval_ops
            );
        }
        QuestAction::Checkpoint { id } => {
            println!("[quest checkpoint] 为 Quest 创建检查点:{}", id);
        }
    }
    println!("[quest] (骨架:Stage 8 RC,L10 接线延后到 v1.1)");
    Ok(())
}
