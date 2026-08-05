//! Chimera CLI — NEXUS-OMEGA AI 编码代理的命令行入口
//!
//! 对应架构层:L10 Interface
//! 对应创新点:无(用户交互入口,但承载 Figment 多源配置合并)
//!
//! ## 模块组织
//! - [`cli`][]:Clap 子命令定义(`Cli`、`Commands`、`QuestAction`、`ConfigAction`)
//! - [`config`][]:Figment 配置加载(`ChimeraConfig` 及多源合并逻辑)
//! - [`commands`][]:各子命令的业务骨架(`run`/`tui`/`quest`/`config`/`wiki`/`parliament`)
//!
//! ## 配置优先级
//! Figment 合并顺序(后者覆盖前者):
//! 1. 内置默认值(`ChimeraConfig::default`)
//! 2. 配置文件(默认 `~/.chimera/omega.yaml`,可由 `--config` 覆盖)
//! 3. 环境变量(前缀 `CHIMERA_`,嵌套用 `__` 分隔,如 `CHIMERA_QUEST__MAX_TASKS_PER_QUEST`)
//! 4. CLI 参数(目前仅 `--config` 影响配置加载路径,后续可扩展)
//!
//! ## 热加载方案(注释说明,骨架暂不实现)
//! 配置热加载计划通过两种机制实现:
//! - **Unix**:捕获 `SIGHUP` 信号,触发 `ChimeraConfig::load` 重载
//! - **跨平台**:使用 `notify` crate 监听 `omega.yaml` 文件变更,debounce 500ms 后重载
//!
//!   重载后通过 `event-bus` 广播 `ConfigReloaded` 事件,各子系统订阅并应用新配置。
//!   当前 Week 8 已完成静态加载,热加载为未来增强项(优先级 P3)。
//!
//! # 快速示例
//! WHY 选此示例:展示最常用路径 —— `Cli::parse_from` 解析参数 + `ChimeraConfig::default` 内置默认,
//! 覆盖 CLI 入口与配置加载两条核心 API,且无需 IO 可在 doctest 直接运行。
//! ```
//! use chimera_cli::{Cli, Commands, QuestAction, ChimeraConfig};
//! use clap::Parser;
//!
//! // Cli 实现 clap::Parser,可从字符串切片解析(便于测试与脚本调用)
//! let cli = Cli::parse_from(["chimera", "quest", "list"]);
//! // Quest 变体含子命令级 `json` flag(Task 1.7 引入,与全局 --json 兼容回退),
//! // matches! 模式用 `..` 忽略其他字段,保持 doctest 对字段扩展的健壮性
//! assert!(matches!(cli.command, Some(Commands::Quest { action: QuestAction::List, .. })));
//!
//! // ChimeraConfig 实现 Default,提供内置兜底配置(对应 omega.yaml 缺省值)
//! let _config = ChimeraConfig::default();
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

/// Action 编排器(消费 TuiActionRequested,路由 quest.* 域到 QuestEngine 真实执行)
pub mod action_orchestrator;
/// Clap 子命令定义
pub mod cli;
/// 子命令业务骨架
pub mod commands;
/// Figment 多源配置加载
pub mod config;
/// CLI 错误类型(结构化错误码,Task 0.2 将据此映射 ExitCode 矩阵)
pub mod error;
/// Quest 分解编排器(消费 TuiChatSubmitted,经 QuestEngine 真实分解并流式回发)
pub mod orchestrator;
// PROBE P3.2: 超窗兜底桥（kvbsr→repo-wiki→hcw 两级检索真实链路）
pub mod overwindow_bridge;
// PROBE P2.2: 选择器学习编排器（S4 → holder → window 注入链）
/// 统一输出 helper(Task 1.7 JSON / Task 1.12 彩色 + 表格 + 进度)
pub mod output;
/// Permission prompt 机制(Task 1.11)
pub mod permission;
pub mod selector_orchestrator;

// === 公开 API 重导出 ===
pub use cli::{Cli, Commands, ConfigAction, QuestAction};
pub use config::{ChimeraConfig, LazyConfig};
pub use error::ChimeraCliError;

/// Crate 版本(从 workspace.package.version 派生)
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
