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
//! assert!(matches!(cli.command, Some(Commands::Quest { action: QuestAction::List })));
//!
//! // ChimeraConfig 实现 Default,提供内置兜底配置(对应 omega.yaml 缺省值)
//! let _config = ChimeraConfig::default();
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

/// Clap 子命令定义
pub mod cli;
/// 子命令业务骨架
pub mod commands;
/// Figment 多源配置加载
pub mod config;

// === 公开 API 重导出 ===
pub use cli::{Cli, Commands, ConfigAction, QuestAction};
pub use config::{ChimeraConfig, LazyConfig};

/// Crate 版本(从 workspace.package.version 派生)
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
