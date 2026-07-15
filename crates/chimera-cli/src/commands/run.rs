//! `chimera run <prompt>` — 单次任务运行骨架
//!
//! 后续接入流程:NMC 编码 → TTG 切换 → PVL 生产验证 → GQEP 聚集。
//! 当前仅打印 prompt 占位。

use anyhow::Result;

use crate::config::ChimeraConfig;

/// 执行 run 命令
///
/// `prompt` 为用户意图原始文本,`config` 为已加载的合并配置。
pub async fn execute(prompt: &str, config: &ChimeraConfig) -> Result<()> {
    tracing::info!(prompt = %prompt, "收到单次任务");
    println!("[run] 任务提示词:{}", prompt);
    println!("[run] 当前思考模式:{}", config.thinking_toggle.default_mode);
    println!("[run] 模型路由策略:{}", config.model_router.strategy);
    println!("[run] (骨架:Stage 8 RC,L10 接线延后到 v1.1)");
    Ok(())
}
