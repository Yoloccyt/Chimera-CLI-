//! 子命令业务骨架 — 各命令的入口分发与实现
//!
//! 当前 Stage 0 阶段,所有子命令仅打印占位信息并返回 `NotImplemented` 错误,
//! 后续按 8 周计划逐步接入真实业务逻辑。

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
/// 无子命令时打印帮助信息(不执行任何重活,对齐 §6 红线)。
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
            // 无子命令:打印帮助提示(不调用 clap 的 print_help 以避免耦合)
            println!("NEXUS-OMEGA AI Coding Agent v{}\n", crate::VERSION);
            println!("用法: aether <COMMAND> [OPTIONS]");
            println!("运行 `aether --help` 查看可用命令");
            Ok(())
        }
    }
}
