//! 集成测试 — CLI 解析与配置加载
//!
//! 覆盖验收标准:
//! - `--version` 输出正确
//! - `config init` 生成 omega.yaml
//! - 配置文件可被 Figment 加载
//! - 默认配置非空

use std::path::PathBuf;

use clap::Parser;
use tempfile::TempDir;

use chimera_cli::cli::Cli;
use chimera_cli::config;

/// 测试 `--version` 触发 DisplayVersion(不进入主流程)
///
/// Clap 在遇到 --version 时返回特殊错误,kind 为 DisplayVersion,
/// 这是设计行为(快速退出,不加载配置)。
#[test]
fn test_version_command() {
    let result = Cli::try_parse_from(["aether", "--version"]);
    assert!(result.is_err(), "--version 应触发 Clap 退出");
    let err = result.unwrap_err();
    use clap::error::ErrorKind;
    assert_eq!(
        err.kind(),
        ErrorKind::DisplayVersion,
        "错误类型应为 DisplayVersion"
    );
}

/// 测试 `--help` 触发 DisplayHelp
#[test]
fn test_help_command() {
    let result = Cli::try_parse_from(["aether", "--help"]);
    assert!(result.is_err(), "--help 应触发 Clap 退出");
    let err = result.unwrap_err();
    use clap::error::ErrorKind;
    assert_eq!(
        err.kind(),
        ErrorKind::DisplayHelp,
        "错误类型应为 DisplayHelp"
    );
}

/// 测试无子命令时 command 为 None(不执行重活)
#[test]
fn test_no_subcommand() {
    let cli = Cli::try_parse_from(["aether"]).unwrap();
    assert!(cli.command.is_none(), "无子命令时 command 应为 None");
}

/// 测试 run 子命令解析
#[test]
fn test_run_subcommand() {
    let cli = Cli::try_parse_from(["aether", "run", "hello world"]).unwrap();
    match cli.command {
        Some(chimera_cli::cli::Commands::Run { prompt }) => {
            assert_eq!(prompt, "hello world");
        }
        _ => panic!("应解析为 Run 命令"),
    }
}

/// 测试 quest 子命令解析
#[test]
fn test_quest_subcommand() {
    let cli = Cli::try_parse_from(["aether", "quest", "list"]).unwrap();
    match cli.command {
        Some(chimera_cli::cli::Commands::Quest { action }) => {
            assert!(matches!(action, chimera_cli::cli::QuestAction::List));
        }
        _ => panic!("应解析为 Quest 命令"),
    }
}

/// 测试 config 子命令解析
#[test]
fn test_config_subcommand() {
    let cli = Cli::try_parse_from(["aether", "config", "init"]).unwrap();
    match cli.command {
        Some(chimera_cli::cli::Commands::Config { action }) => {
            assert!(matches!(action, chimera_cli::cli::ConfigAction::Init));
        }
        _ => panic!("应解析为 Config 命令"),
    }
}

/// 测试 `--config` 全局参数解析
#[test]
fn test_config_global_arg() {
    let cli = Cli::try_parse_from(["aether", "--config", "/tmp/test.yaml", "run", "hi"]).unwrap();
    assert_eq!(cli.config, Some(PathBuf::from("/tmp/test.yaml")));
}

/// 测试默认配置非空(对齐验收标准)
#[test]
fn test_default_config() {
    let cfg = config::default_config();
    assert!(!cfg.nexus.version.is_empty(), "version 不应为空");
    assert!(cfg.quest.auto_decompose, "auto_decompose 默认应为 true");
    assert_eq!(cfg.quest.max_tasks_per_quest, 20);
    assert_eq!(cfg.thinking_toggle.default_mode, "Auto");
    assert_eq!(cfg.model_router.strategy, "Auto");
    assert!(!cfg.model_router.providers.is_empty(), "providers 不应为空");
    assert_eq!(cfg.seccore.sandbox, "gvisor");
    assert_eq!(cfg.seccore.command_interpolation, "forbidden");
    assert!(cfg.evolution.enabled);
}

/// 测试 `config init` 生成 omega.yaml(对齐验收标准)
#[test]
fn test_config_init() {
    let tmp = TempDir::new().expect("创建临时目录失败");
    let config_path = tmp.path().join("omega.yaml");

    config::init_config_file(&config_path).expect("生成配置文件失败");

    // 验证文件存在
    assert!(config_path.exists(), "配置文件应已生成");

    // 验证文件非空
    let content = std::fs::read_to_string(&config_path).expect("读取配置文件失败");
    assert!(!content.is_empty(), "配置文件内容不应为空");

    // 验证包含关键章节(对齐 §10.2 模板)
    assert!(content.contains("nexus:"), "应包含 nexus 章节");
    assert!(content.contains("quest:"), "应包含 quest 章节");
    assert!(
        content.contains("model_router:"),
        "应包含 model_router 章节"
    );
    assert!(content.contains("seccore:"), "应包含 seccore 章节");
    assert!(content.contains("monitoring:"), "应包含 monitoring 章节");
}

/// 测试加载配置文件(对齐验收标准)
///
/// 先生成默认配置,再用 Figment 加载,验证字段正确反序列化。
#[test]
fn test_config_load() {
    let tmp = TempDir::new().expect("创建临时目录失败");
    let config_path = tmp.path().join("omega.yaml");

    // 1. 生成配置文件
    config::init_config_file(&config_path).expect("生成配置文件失败");

    // 2. 加载配置(指定路径)
    // 注:load 是 config 模块级函数,非 ChimeraConfig 关联函数。
    let cfg = config::load(Some(config_path.clone())).expect("加载配置失败");

    // 3. 验证关键字段
    assert_eq!(cfg.nexus.version, "1.0.0-omega");
    assert!(cfg.quest.auto_decompose);
    assert_eq!(cfg.quest.max_tasks_per_quest, 20);
    assert_eq!(cfg.thinking_toggle.default_mode, "Auto");
    assert_eq!(cfg.model_router.strategy, "Auto");
    assert_eq!(cfg.model_router.budget.daily_usd, 50.0);
    assert_eq!(cfg.seccore.sandbox, "gvisor");
    assert!(cfg.seccore.seccomp, "seccomp 默认应启用");
    assert!(cfg.evolution.enabled);
    assert!(cfg.monitoring.prometheus.enabled);

    // 4. 验证 providers 数量(§10.2 模板有 5 个)
    assert_eq!(cfg.model_router.providers.len(), 5, "应有 5 个模型提供商");
}

/// 测试加载不存在的配置文件时回退到默认值(不报错)
#[test]
fn test_config_load_missing_file_uses_defaults() {
    let tmp = TempDir::new().expect("创建临时目录失败");
    let missing_path = tmp.path().join("nonexistent.yaml");

    // 文件不存在时应回退到默认值,不报错
    let cfg = config::load(Some(missing_path)).expect("缺失文件应回退默认值");
    assert!(!cfg.nexus.version.is_empty(), "默认 version 不应为空");
}

/// 测试默认配置路径函数返回非空路径
#[test]
fn test_default_config_path() {
    let path = config::default_config_path();
    assert!(
        path.to_string_lossy().contains("omega.yaml"),
        "路径应包含 omega.yaml"
    );
}

/// 测试 omega.yaml 模板包含所有必要章节
#[test]
fn test_omega_yaml_template_completeness() {
    let tmp = TempDir::new().expect("创建临时目录失败");
    let config_path = tmp.path().join("omega.yaml");
    config::init_config_file(&config_path).expect("生成配置文件失败");

    let content = std::fs::read_to_string(&config_path).expect("读取配置文件失败");

    // 验证所有顶层章节存在(对齐 §10.2)
    let required_sections = [
        "nexus:",
        "quest:",
        "thinking_toggle:",
        "repo_wiki:",
        "model_router:",
        "osa:",
        "kvbsr:",
        "pvl:",
        "mtpe:",
        "gqep:",
        "seccore:",
        "mcp:",
        "evolution:",
        "monitoring:",
    ];
    for section in &required_sections {
        assert!(content.contains(section), "模板应包含章节: {}", section);
    }
}
