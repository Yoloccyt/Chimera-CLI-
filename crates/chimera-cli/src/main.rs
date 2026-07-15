//! `chimera` 二进制入口 — NEXUS-OMEGA CLI 主程序
//!
//! 启动流程(目标 < 200ms):
//! 1. Clap 解析命令行参数(同步,极快)
//! 2. 初始化 tracing 日志订阅器
//! 3. Figment 加载合并配置(默认 > file > env)
//! 4. 分发到对应子命令(无子命令时默认启动 TUI)
//!
//! 注意:main 中不做重活(如数据库连接、模型加载),这些延迟到子命令内部按需初始化,
//! 确保 `chimera --version` 等快速命令的响应时间。

// WHY: 与 lib.rs 保持一致,禁止 main 入口引入 unsafe。
// `#![forbid(unsafe_code)]` 是项目铁律,所有 crate 必须声明(见 AETHER_NEXUS_OMEGA_ULTIMATE.md §6 红线)。
#![forbid(unsafe_code)]

use clap::Parser;
use tracing_subscriber::EnvFilter;

use chimera_cli::{cli::Cli, commands, config};

/// 程序入口
///
/// 使用 `#[tokio::main]` 将同步 main 转为 async,默认多线程运行时。
/// 错误向上传播为 `anyhow::Error`,由 main 返回时自动打印。
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. 解析 CLI(包含 --version/--help 的快速退出,不进入后续流程)
    let cli = Cli::parse();

    // 2. 初始化日志:verbose 时用 debug,否则用 info
    //    EnvFilter 允许 RUST_LOG 环境变量覆盖,提供运行时调试灵活性
    let default_level = if cli.verbose { "debug" } else { "info" };
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .compact()
        .init();

    // 3. 加载配置(优先级:CLI --config > 默认路径 > env > defaults)
    //    配置文件不存在不报错,仅用默认值(对齐 §6 红线:避免暴力加载)
    let config = config::load(cli.config.clone()).map_err(|e| {
        tracing::error!(error = %e, "配置加载失败");
        e
    })?;

    tracing::debug!(?config.nexus.version, "配置加载完成");

    // 4. 分发命令(无子命令时打印帮助)
    commands::dispatch(&cli, &config).await
}
