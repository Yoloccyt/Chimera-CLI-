//! `chimera` 二进制入口 — NEXUS-OMEGA CLI 主程序
//!
//! 启动流程(目标 < 200ms):
//! 1. Task 2.3:安装 human-panic hook(release 模式输出友好提示)
//! 2. Clap 解析命令行参数(同步,极快)
//! 3. 初始化颜色模式(Task 1.12:`--no-color` flag)
//! 4. 初始化 tracing 日志订阅器
//! 5. Figment 加载合并配置(默认 > file > env)
//! 6. 分发到对应子命令(无子命令时默认启动 TUI)
//!
//! 注意:main 中不做重活(如数据库连接、模型加载),这些延迟到子命令内部按需初始化,
//! 确保 `chimera --version` 等快速命令的响应时间。

// WHY: 与 lib.rs 保持一致,禁止 main 入口引入 unsafe。
// `#![forbid(unsafe_code)]` 是项目铁律,所有 crate 必须声明(见 AETHER_NEXUS_OMEGA_ULTIMATE.md §6 红线)。
#![forbid(unsafe_code)]

use clap::Parser;
use tracing_subscriber::EnvFilter;

use chimera_cli::{banner, cli::Cli, commands, config, output, ChimeraCliError};

/// 程序入口
///
/// 返回 `std::process::ExitCode` 而非 `anyhow::Result<()>`,
/// 以支持结构化退出码矩阵(ADR-060):不同错误类别返回不同退出码,
/// 便于 shell 脚本/CI 通过 `$?` 程序化区分错误类型。
///
/// 退出码矩阵:
/// - 0 = success
/// - 1 = user_error(ConfigError)
/// - 2 = not_implemented(NotImplemented)
/// - 3 = system_error(EngineError / IoError / 未知错误)
/// - 4 = user_cancelled(UserCancelled)
/// - 5 = permission_denied(PermissionDenied)
/// - 6 = timeout(Timeout)
///
/// `--legacy-exit-code` flag 可兼容 v2.8.0 行为(所有错误统一返回 1)。
///
/// # Task 1.7 `--json` 错误输出
///
/// `cli.json=true` 时,错误输出走结构化 envelope schema(而非 Debug 格式):
/// ```json
/// { "status": "error", "error": { "kind": "NotImplemented", "message": "..." }, "exit_code": 2 }
/// ```
/// 详见 `output::print_json_error`。成功输出的 JSON 格式化由各命令 handler 调用
/// `output::print_json` 完成,main 只负责错误路径。
#[tokio::main]
async fn main() -> std::process::ExitCode {
    // Task 2.3:安装 human-panic hook(必须在所有可能 panic 的代码之前)
    //
    // WHY 宏而非函数调用:`setup_panic!()` 是声明式宏,在编译期展开为 panic hook 注册代码,
    // 通过 `std::panic::set_hook` 安装全局 hook。宏在 debug 构建中展开为 no-op,
    // 保留默认 panic handler(含完整 backtrace 便于开发调试);release 构建中展开为
    // human-panic 的友好提示(含"请将以下信息提交给开发者"+ 简化 backtrace)。
    //
    // WHY main 函数第一行:panic hook 是进程级全局状态,必须在任何可能 panic 的代码之前安装。
    // 宏展开为 block expression,只能在函数体内调用(非 item context)。
    human_panic::setup_panic!();

    // Task 2.3:测试专用 panic 触发入口(仅 CHIMERA_PANIC_TEST=1 时生效)
    //
    // WHY 环境变量而非 CLI flag:测试专用入口不应暴露给用户;环境变量不会污染 --help 输出;
    // 子进程测试通过 `.env("CHIMERA_PANIC_TEST", "1")` 设置最简洁。
    //
    // WHY 在 Cli::parse() 之前:确保测试能可靠触发 panic,不依赖 CLI 参数解析
    // (避免无子命令时进入 TUI 模式阻塞测试进程)。
    if std::env::var("CHIMERA_PANIC_TEST")
        .ok()
        .filter(|v| v == "1")
        .is_some()
    {
        panic!("CHIMERA_PANIC_TEST=1:故意触发 panic 以验证 panic hook 安装");
    }

    // 1. 解析 CLI(包含 --version/--help 的快速退出,不进入后续流程)
    let cli = Cli::parse();

    // 2. Task 1.12:初始化全局颜色模式(`--no-color` flag 或 `NO_COLOR` 环境变量)
    //    WHY 在 tracing 之前:颜色模式影响后续所有 output helper,且不依赖日志
    output::init_color_mode(cli.no_color);

    // 2.5 输出启动 banner(品牌 ASCII art),除非用户传入 `--no-banner`
    //     WHY 在 init_color_mode 之后、tracing 之前:banner 是面向用户的视觉入口,
    //     应尽早显示;同时先初始化颜色模式便于 banner 后续可着色(当前为纯文本,
    //     保留扩展点)。`--no-banner` 守护确保 CI 截屏 / 自动化断言不被彩条干扰。
    if !cli.no_banner {
        banner::print();
    }

    // 3. 初始化日志:verbose 时用 debug,否则用 info
    //    EnvFilter 允许 RUST_LOG 环境变量覆盖,提供运行时调试灵活性
    let default_level = if cli.verbose { "debug" } else { "info" };
    let env_filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default_level));
    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .with_target(false)
        .compact()
        .init();

    // 4. 分发命令并根据结果映射退出码
    match run(&cli).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            // Task 1.7:`--json` 模式下输出结构化错误 envelope 到 stderr
            // WHY 单一出口:无论 ChimeraCliError 还是裸 anyhow::Error,都通过
            // print_json_error 输出统一 schema,便于脚本 `jq .error.kind` 程序化消费
            if cli.json {
                if let Some(ce) = e.downcast_ref::<ChimeraCliError>() {
                    output::print_json_error(ce.kind(), &ce.message(), ce.exit_code_value());
                } else {
                    // 非 ChimeraCliError 的 anyhow::Error 归为 system_error(3),
                    // kind 用 "SystemError" 标识(对应 ADR-060 矩阵的 system_error 语义)
                    output::print_json_error("SystemError", &e.to_string(), 3);
                }
            } else {
                // 人类可读模式:错误输出到 stderr(含 Debug 格式,保留错误链与变体前缀,便于 grep)
                eprintln!("Error: {:?}", e);
            }

            // --legacy-exit-code:兼容 v2.8.0 行为,所有错误统一返回 1
            if cli.legacy_exit_code {
                return std::process::ExitCode::from(1);
            }

            // 尝试 downcast 到 ChimeraCliError 以获取结构化退出码
            // WHY downcast:dispatch 内部错误链可能包含 ChimeraCliError(经 anyhow
            // From 转换),downcast_ref 可无损还原具体类型以映射退出码。
            // 非 ChimeraCliError 的错误(如配置加载的裸 anyhow::Error)归为
            // system_error(3),因其属于"下游引擎或 IO 故障"语义。
            if let Some(ce) = e.downcast_ref::<ChimeraCliError>() {
                ce.exit_code()
            } else {
                std::process::ExitCode::from(3)
            }
        }
    }
}

/// 核心运行逻辑(配置加载 + 命令分发)
///
/// 抽取为独立函数使 `main` 能专注于退出码映射,
/// 同时保持配置加载的错误能被 downcast 识别为 `ConfigError`。
async fn run(cli: &Cli) -> anyhow::Result<()> {
    // 加载配置(优先级:CLI --config > 默认路径 > env > defaults)
    //    配置文件不存在不报错,仅用默认值(对齐 §6 红线:避免暴力加载)
    let cfg = config::load(cli.config.clone()).map_err(|e| {
        tracing::error!(error = %e, "配置加载失败");
        // 包装为 ChimeraCliError::ConfigError,确保退出码为 1(user_error) 而非 3(system_error)
        ChimeraCliError::ConfigError(e.to_string())
    })?;

    tracing::debug!(?cfg.nexus.version, "配置加载完成");

    // 分发命令(无子命令时打印帮助)
    commands::dispatch(cli, &cfg).await
}
