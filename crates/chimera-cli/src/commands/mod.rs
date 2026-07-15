//! 子命令业务骨架 — 各命令的入口分发与实现
//!
//! 分发逻辑:
//! - 有显式子命令时路由到对应 handler
//! - 无子命令时默认启动 TUI 交互界面(品牌统一后 `chimera` 命令即进入可视化面板)

use anyhow::Result;

use crate::cli::{Cli, Commands};
use crate::config::ChimeraConfig;

/// 配置管理子命令
pub mod config;
/// 议会审议子命令
pub mod parliament;
/// Quest 管理子命令
pub mod quest;
/// 单次任务运行子命令
pub mod run;
/// TUI 交互界面子命令
pub mod tui;
/// Wiki 查询子命令
pub mod wiki;

/// 命令分发入口
///
/// 根据 `Cli.command` 路由到对应子命令处理函数。
/// 无子命令时默认启动 TUI 交互界面,用户可直接输入 `chimera` 进入可视化面板。
///
/// 注:参数命名为 `cfg` 而非 `config`,避免遮蔽 `pub mod config;` 声明的模块名,
/// 否则 `config::execute(...)` 会被解析为对 `&ChimeraConfig` 参数的方法调用。
pub async fn dispatch(cli: &Cli, cfg: &ChimeraConfig) -> Result<()> {
    match &cli.command {
        Some(Commands::Run { prompt }) => run::execute(prompt, cfg).await,
        Some(Commands::Tui) => tui::execute(cfg).await,
        Some(Commands::Quest { action }) => quest::execute(action, cfg).await,
        Some(Commands::Config { action }) => config::execute(action, cfg).await,
        Some(Commands::Wiki { query }) => wiki::execute(query, cfg).await,
        Some(Commands::Parliament { proposal }) => parliament::execute(proposal, cfg).await,
        None => {
            // 无子命令:默认启动 TUI 交互界面
            // --help/--version 由 Clap 在 Cli::parse() 阶段内置处理,不会进入此分支
            tui::execute(cfg).await
        }
    }
}
