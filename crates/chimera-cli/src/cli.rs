//! Clap 子命令定义 — NEXUS-OMEGA CLI 的命令行界面
//!
//! 命令树:
//! ```text
//! chimera
//!   ├── run <prompt>          # 运行单次任务
//!   ├── tui                    # 启动 TUI 交互界面
//!   ├── quest <action>         # Quest 管理
//!   │     ├── list             # 列出所有 Quest
//!   │     ├── show <id>        # 查看 Quest 详情
//!   │     ├── cancel <id>      # 取消 Quest
//!   │     └── checkpoint <id>  # 创建检查点
//!   ├── config <action>        # 配置管理
//!   │     ├── init             # 生成默认 omega.yaml
//!   │     ├── list             # 列出当前配置
//!   │     ├── show             # 显示完整配置(JSON)
//!   │     └── path             # 显示配置文件路径
//!   ├── wiki <query>           # Wiki 查询
//!   └── parliament <proposal>  # 议会审议
//! ```

use std::path::PathBuf;

use clap::{Parser, Subcommand};

/// 顶层 CLI 解析结构
///
/// 使用 `Option<Commands>` 而非 `Commands`,使得无子命令时仍可显示帮助
/// (符合 §6 红线:避免暴力加载,无命令时不应执行任何重活)。
#[derive(Parser, Debug)]
#[command(
    name = "chimera",
    version,
    about = "NEXUS-OMEGA AI Coding Agent — 全维稀疏架构的下一代编码代理"
)]
pub struct Cli {
    /// 子命令(可选,缺省时启动 TUI 交互界面)
    #[command(subcommand)]
    pub command: Option<Commands>,

    /// 配置文件路径(默认 ~/.chimera/omega.yaml)
    ///
    /// 全局参数,可在任意子命令前使用,如 `chimera --config ./x.yaml run "hi"`
    #[arg(long, global = true, value_name = "FILE")]
    pub config: Option<PathBuf>,

    /// 启用详细日志(等价于 RUST_LOG=debug)
    #[arg(long, short = 'v', global = true)]
    pub verbose: bool,
}

/// 一级子命令枚举
#[derive(Subcommand, Debug)]
pub enum Commands {
    /// 运行单次任务(不进入 Quest 长期任务流程)
    Run {
        /// 任务提示词(用户意图的原始文本)
        prompt: String,
    },
    /// 启动 TUI 交互界面(对应 `chimera-tui` crate)
    Tui,
    /// Quest 管理(长期任务的创建/查询/取消/检查点)
    Quest {
        /// Quest 子命令动作
        #[command(subcommand)]
        action: QuestAction,
    },
    /// 配置管理(初始化/查看/列出)
    Config {
        /// 配置子命令动作
        #[command(subcommand)]
        action: ConfigAction,
    },
    /// Wiki 查询(对应 `repo-wiki` crate 的语义检索)
    Wiki {
        /// 查询语句(自然语言)
        query: String,
    },
    /// 议会审议(对应 `parliament` crate,提交提案供多模型议会表决)
    Parliament {
        /// 提案内容(需审议的决策描述)
        proposal: String,
    },
}

/// Quest 子命令动作
#[derive(Subcommand, Debug)]
pub enum QuestAction {
    /// 列出所有 Quest(含进行中/已完成/已取消)
    List,
    /// 查看 Quest 详情
    Show {
        /// Quest ID
        id: String,
    },
    /// 取消 Quest(会触发检查点保存)
    Cancel {
        /// Quest ID
        id: String,
    },
    /// 为 Quest 创建检查点(对应 LHQP 长期持久化)
    Checkpoint {
        /// Quest ID
        id: String,
    },
}

/// Config 子命令动作
#[derive(Subcommand, Debug)]
pub enum ConfigAction {
    /// 生成默认 omega.yaml 到指定路径(默认 ~/.aether/omega.yaml)
    Init,
    /// 列出当前生效的配置项(键值对形式)
    List,
    /// 显示完整配置(JSON 格式,便于脚本消费)
    Show,
    /// 显示配置文件路径(实际加载的文件)
    Path,
}
