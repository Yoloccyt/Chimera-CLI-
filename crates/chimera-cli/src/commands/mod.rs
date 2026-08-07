//! 子命令业务骨架 — 各命令的入口分发与实现
//!
//! 分发逻辑:
//! - 有显式子命令时路由到对应 handler
//! - 无子命令时默认启动 TUI 交互界面(品牌统一后 `chimera` 命令即进入可视化面板)
//!
//! # v2.9.0-omega Task 1.7 / 1.11 / 1.12 / 2.2 全局 flag 传递
//!
//! `dispatch` 从 `Cli` 提取以下全局 flag 并传递到各命令:
//! - `json: bool`(Task 1.7):控制输出格式(JSON / 人类可读)
//! - `PermissionCtx`(Task 1.11):封装 `--yes` / `--no-permission` 跳过逻辑
//! - `dry_run: bool`(Task 2.2):仅破坏性命令消费,`true` 时只输出预览不执行
//!
//! `--no-color`(Task 1.12)由 `output::init_color_mode` 在 `main` 入口设置全局,
//! 各命令通过 `output::print_*` helper 自动感知,无需逐命令传递。

use anyhow::Result;

use crate::cli::{Cli, Commands};
use crate::config::ChimeraConfig;
use crate::permission::PermissionCtx;

/// Agent 生命周期管理子命令(Task 1.10)
pub mod agent;
/// 红队安全审计子命令(Task 1.9)
pub mod audit;
/// chat REPL 流式对话子命令(Task 1.5 / 1.6)
pub mod chat;
/// shell 补全脚本生成子命令(Task 1.14)
pub mod completions;
/// 配置管理子命令
pub mod config;
/// 系统健康检查子命令(Task 1.13)
pub mod doctor;
/// EXAMPLES 一级入口(Task 5 of spec)
pub mod help;
/// LLM Provider 管理子命令(Task 2 of spec)
pub mod llm;
/// MCP 量子网格管理子命令(Task 1.8)
pub mod mcp;
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
    // 从 Cli 构造 PermissionCtx(各命令按需消费,不破坏不需要 prompt 的命令签名)
    let perm = PermissionCtx::from_cli(cli);
    match &cli.command {
        // Task 5 of spec: EXAMPLES 一级入口 — 顶级命令位置(Run 之前)
        // 不消费 json/perm(纯字符串输出)
        Some(Commands::Help { command }) => help::execute(command.as_deref(), cli).await,
        Some(Commands::Run { prompt }) => run::execute(prompt, cfg, cli.json, &perm).await,
        // Task 1.5: chat REPL 不消费 json flag(REPL 内部统一人类可读),
        // 但消费 perm(--no-permission 自动允许 tool 调用,CI 友好)
        Some(Commands::Chat) => chat::execute(cli, cfg).await,
        // v3-engine M2(ADR-061):传递 `--no-v3-engine` flag 到 tui::execute,
        // 由其设置 CHIMERA_NO_V3_ENGINE 环境变量控制渲染路径回退。
        // TUI 不消费 json/perm(TUI 有自己的渲染管线,不走 stdout 输出 helper)
        Some(Commands::Tui { no_v3_engine }) => tui::execute(cfg, *no_v3_engine).await,
        // Quest:全局 --json 优先,子命令级 --json 作为兼容回退(Task 1.7 统一前保留)
        Some(Commands::Quest { action, json }) => {
            quest::execute(action, cfg, cli.json || *json, &perm, cli.dry_run).await
        }
        Some(Commands::Config { action }) => config::execute(action, cfg, cli.json).await,
        // Wiki:全局 --json 优先,子命令级 --json 作为兼容回退(Task 1.7 统一前保留);
        // --limit 由子命令参数传递(SubTask 1.3.3,默认 10)
        Some(Commands::Wiki { query, json, limit }) => {
            wiki::execute(query, cfg, cli.json || *json, *limit).await
        }
        // Parliament:全局 --json 优先,子命令级 --json 作为兼容回退;perm 预留供未来权限检查
        Some(Commands::Parliament { proposal, json }) => {
            parliament::execute(proposal, cfg, cli.json || *json, &perm).await
        }
        // Task 1.8: MCP 量子网格管理 — 全局 --json 传递;perm 供 mcp call 使用;dry_run 供 mcp call 预览
        Some(Commands::Mcp { action }) => {
            mcp::execute(action, cfg, cli.json, &perm, cli.dry_run).await
        }
        // Task 1.9: 红队安全审计 — 全局 --json 优先,子命令级 --json 作为兼容回退
        Some(Commands::Audit { json, severity }) => {
            audit::execute(cfg, cli.json || *json, severity.as_deref()).await
        }
        // Task 1.10: Agent 生命周期管理 — 全局 --json 传递;perm 供 agent cancel 使用;dry_run 供 agent cancel 预览
        Some(Commands::Agent { action, parallel }) => {
            agent::execute(action, cfg, cli.json, *parallel, &perm, cli.dry_run).await
        }
        // Task 1.13: 系统健康检查 — 全局 --json 优先,子命令级 --json 作为兼容回退
        Some(Commands::Doctor { json, fix }) => doctor::execute(cfg, cli.json || *json, *fix).await,
        // Task 1.14: 生成 shell 补全脚本 — 不消费 json/perm,直接输出到 stdout
        Some(Commands::Completions { shell }) => completions::execute(*shell).await,
        // Task 2 of spec: LLM Provider 管理 — 全局 --json 传递;perm 供 set-default/strategy 使用
        Some(Commands::Llm { action }) => llm::execute(action, cfg, cli.json, &perm).await,
        None => {
            // 无子命令:默认启动 TUI 交互界面(默认启用 v3-engine)
            // --help/--version 由 Clap 在 Cli::parse() 阶段内置处理,不会进入此分支
            tui::execute(cfg, false).await
        }
    }
}
