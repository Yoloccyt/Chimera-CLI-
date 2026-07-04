//! CLI 参数解析属性测试 — 验证 Clap 解析的不变量
//!
//! 对应 E-MINOR-1:补充 chimera-cli proptest,验证 CLI 参数解析的不变量。
//!
//! # 验证的不变量
//! 1. --version 总触发 DisplayVersion(任意前置全局参数组合下)
//! 2. 任意子命令 --help 不 panic,返回 DisplayHelp
//! 3. 任意无效参数返回真正的错误(非 DisplayVersion/DisplayHelp)
//! 4. --config <path> 全局参数在任意合法路径下被原样保留
//! 5. run <prompt> 在任意合法 prompt 下被原样保留
//!
//! # 策略
//! - 生成随机全局参数组合(--config、-v),验证 --version 不变量
//! - 生成随机子命令,验证 --help 不 panic
//! - 生成保证无效的 flag(--invalid-* 前缀),验证错误返回
//! - 生成随机合法路径/prompt,验证原样保留
//!
//! # WHY:proptest 的价值
//! 传统单元测试只能覆盖少量手工构造的输入,容易遗漏边界情况。
//! proptest 通过随机生成大量输入,能发现手工测试难以预见的解析异常
//! (如特殊字符路径、极端长 prompt)。对于 CLI 这种"输入空间无限"的
//! 场景,属性测试是保障解析鲁棒性的高性价比手段,能在 CI 阶段提前
//! 暴露潜在的 panic 与解析漂移问题。

#![forbid(unsafe_code)]

use std::path::PathBuf;

use clap::Parser;
use proptest::prelude::*;

use chimera_cli::cli::{Cli, Commands};

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// 不变量 1:--version 总触发 DisplayVersion
    ///
    /// 在任意 --config <path> 与 -v 全局参数组合下,--version 总是
    /// 触发 Clap 的 DisplayVersion 退出,不进入主流程(符合 §6 红线:
    /// 避免暴力加载,--version 应快速退出)。
    #[test]
    fn prop_test_version_always_outputs(
        verbose in prop::bool::ANY,
        config_path in "[a-z][a-z0-9_./-]{0,29}\\.yaml",
    ) {
        let mut args: Vec<String> = vec!["aether".to_string()];
        if verbose {
            args.push("-v".to_string());
        }
        args.push("--config".to_string());
        args.push(config_path);
        args.push("--version".to_string());

        let result = Cli::try_parse_from(args);
        prop_assert!(result.is_err(), "--version 应触发 Clap 退出");
        let err = result.unwrap_err();
        prop_assert_eq!(
            err.kind(),
            clap::error::ErrorKind::DisplayVersion,
            "任意全局参数组合下 --version 应返回 DisplayVersion"
        );
    }

    /// 不变量 2:任意子命令 --help 不 panic,返回 DisplayHelp
    ///
    /// 对任意一级子命令,--help 应稳定返回 DisplayHelp,不应 panic
    /// 或返回其他错误类型。覆盖所有已注册子命令,防止新增子命令时
    /// help 行为回归。
    #[test]
    fn prop_test_help_never_panics(
        subcommand in prop::sample::select(vec![
            "run", "tui", "quest", "config", "wiki", "parliament",
        ]),
    ) {
        let args: Vec<String> = vec![
            "aether".to_string(),
            subcommand.to_string(),
            "--help".to_string(),
        ];

        let result = Cli::try_parse_from(args);
        prop_assert!(result.is_err(), "--help 应触发 Clap 退出");
        let err = result.unwrap_err();
        prop_assert_eq!(
            err.kind(),
            clap::error::ErrorKind::DisplayHelp,
            "子命令 {} --help 应返回 DisplayHelp",
            subcommand,
        );
    }

    /// 不变量 3:任意无效参数返回真正的错误
    ///
    /// 对 --invalid-* 前缀的未知 flag,Clap 应返回错误,且错误类型
    /// 既非 DisplayVersion 也非 DisplayHelp(即真正的解析错误,
    /// 如 UnknownArgument),确保无效输入不会误触退出流程。
    #[test]
    fn prop_test_invalid_args_return_error(
        suffix in "[a-z]{1,10}",
    ) {
        // WHY:用 --invalid- 前缀保证生成的 flag 绝非已知参数
        // (如 --version/--help/--config),从而稳定触发 UnknownArgument
        let invalid_flag = format!("--invalid-{suffix}");
        let args: Vec<String> = vec!["aether".to_string(), invalid_flag.clone()];

        let result = Cli::try_parse_from(args);
        prop_assert!(result.is_err(), "无效参数 {} 应触发错误", invalid_flag);
        let err = result.unwrap_err();
        let kind = err.kind();
        prop_assert!(
            kind != clap::error::ErrorKind::DisplayVersion
                && kind != clap::error::ErrorKind::DisplayHelp,
            "无效参数应返回真正的错误,而非 DisplayVersion/DisplayHelp",
        );
    }

    /// 不变量 4:--config <path> 在任意合法路径下被原样保留
    ///
    /// 全局参数 --config 接受任意路径字符串,Cli.config 应原样保留
    /// (不做规范化、不报错),确保用户自定义配置路径不被意外篡改。
    #[test]
    fn prop_test_global_config_arg_preserved(
        path in "[a-z][a-z0-9_./-]{0,39}",
    ) {
        let args: Vec<String> = vec![
            "aether".to_string(),
            "--config".to_string(),
            path.clone(),
            "run".to_string(),
            "test".to_string(),
        ];

        let cli = Cli::try_parse_from(args)
            .expect("--config <合法路径> run <prompt> 应解析成功");
        prop_assert_eq!(cli.config, Some(PathBuf::from(path)));
    }

    /// 不变量 5:run <prompt> 在任意合法 prompt 下被原样保留
    ///
    /// run 子命令的 prompt 参数应原样保留,不做转义或修改,
    /// 确保用户意图的原始文本完整传递给下游 NMC 编码。
    #[test]
    fn prop_test_run_prompt_preserved(
        input_prompt in "[a-zA-Z][a-zA-Z0-9 .,!?]{0,49}",
    ) {
        let args: Vec<String> = vec![
            "aether".to_string(),
            "run".to_string(),
            input_prompt.clone(),
        ];

        let cli = Cli::try_parse_from(args)
            .expect("run <合法 prompt> 应解析成功");
        prop_assert!(
            matches!(cli.command, Some(Commands::Run { ref prompt }) if *prompt == input_prompt),
            "run <prompt> 应原样保留 prompt",
        );
    }
}
