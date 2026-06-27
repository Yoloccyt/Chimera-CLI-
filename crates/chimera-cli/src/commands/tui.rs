//! `aether tui` — TUI 交互界面骨架
//!
//! 后续将调用 `chimera-tui` crate 启动 ratatui 终端界面。
//! 当前仅打印占位信息。

use anyhow::Result;

use crate::config::ChimeraConfig;

/// 执行 tui 命令
pub async fn execute(_config: &ChimeraConfig) -> Result<()> {
    tracing::info!("启动 TUI 交互界面");
    println!("[tui] TUI 启动中...");
    println!("[tui] (骨架:待 chimera-tui crate 实现后接入,Week 6)");
    Ok(())
}
