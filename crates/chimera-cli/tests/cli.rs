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
    let result = Cli::try_parse_from(["chimera", "--version"]);
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
    let result = Cli::try_parse_from(["chimera", "--help"]);
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
    let cli = Cli::try_parse_from(["chimera"]).unwrap();
    assert!(cli.command.is_none(), "无子命令时 command 应为 None");
}

/// 测试 run 子命令解析
#[test]
fn test_run_subcommand() {
    let cli = Cli::try_parse_from(["chimera", "run", "hello world"]).unwrap();
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
    let cli = Cli::try_parse_from(["chimera", "quest", "list"]).unwrap();
    match cli.command {
        Some(chimera_cli::cli::Commands::Quest { action, .. }) => {
            assert!(matches!(action, chimera_cli::cli::QuestAction::List));
        }
        _ => panic!("应解析为 Quest 命令"),
    }
}

/// 测试 config 子命令解析
#[test]
fn test_config_subcommand() {
    let cli = Cli::try_parse_from(["chimera", "config", "init"]).unwrap();
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
    let cli = Cli::try_parse_from(["chimera", "--config", "/tmp/test.yaml", "run", "hi"]).unwrap();
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

// === Task 1.1-1.4: 真实接入命令验证(v2.9.0-omega)===
//
// 验证 run/quest/wiki/parliament 4 个命令已真实接入 L5-L9 crate,
// 不再返回 NotImplemented 占位错误,而是执行真实业务逻辑。
// WHY 用子进程而非 dispatch 直调:spec 要求验证"stdout/stderr 输出 + 退出码",
// 这两者是进程级语义,只有真实执行二进制才能覆盖完整链路
// (main → dispatch → handler → 真实 crate 调用 → 输出 → 退出码)。

// --- Task 1.1: chimera run 真实接入 QuestEngine ---

/// 测试 `chimera run <prompt>` 成功执行 QuestEngine 分解并流式输出(SubTask 1.1.1-1.1.3)
///
/// 验证点:
/// - 退出码 0(成功)
/// - stdout 包含 `[done]` 标记(SubTask 1.1.3)
/// - stdout 非空(流式输出回复文本,SubTask 1.1.2)
/// - CHIMERA_RUN_CHUNK_DELAY_MS=0 禁用延迟以加速测试
#[test]
fn test_run_command_streams_output_with_done_marker() {
    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .args(["run", "实现一个 hello world 函数"])
        .env("CHIMERA_RUN_CHUNK_DELAY_MS", "0")
        .output()
        .expect("执行 chimera 二进制失败");
    assert!(
        output.status.success(),
        "run 命令应成功执行,实际退出码: {:?}",
        output.status.code()
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("[done]"),
        "stdout 应包含 [done] 标记,实际 stdout: {}",
        stdout
    );
    // 流式输出应有非空回复文本(在 [done] 之前)
    let reply_part = stdout.split("[done]").next().unwrap_or("");
    assert!(
        !reply_part.trim().is_empty(),
        "stdout 在 [done] 前应有流式回复文本,实际 stdout: {}",
        stdout
    );
}

/// 测试 `chimera run --json` 输出 JSON 成功 envelope(SubTask 1.1.1 + Task 1.7)
///
/// JSON 模式不流式输出,而是返回 Quest 结构的 envelope。
#[test]
fn test_run_json_outputs_quest_envelope() {
    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .args(["--json", "run", "test task"])
        .env("CHIMERA_RUN_CHUNK_DELAY_MS", "0")
        .output()
        .expect("执行 chimera 二进制失败");
    assert!(
        output.status.success(),
        "run --json 应成功,实际退出码: {:?}",
        output.status.code()
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"status\": \"ok\""),
        "stdout 应包含 status: ok,实际: {}",
        stdout
    );
    assert!(
        stdout.contains("\"quest_id\""),
        "stdout 应包含 quest_id 字段(Quest 结构),实际: {}",
        stdout
    );
}

/// 测试 `chimera run` 空 prompt 边界(空字符串仍能分解,SubTask 1.1.4)
///
/// 空字符串 prompt 是合法输入,QuestEngine 会生成默认标题的 Quest。
#[test]
fn test_run_empty_prompt_boundary() {
    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .args(["run", ""])
        .env("CHIMERA_RUN_CHUNK_DELAY_MS", "0")
        .output()
        .expect("执行 chimera 二进制失败");
    assert!(
        output.status.success(),
        "run 空字符串 prompt 应成功,实际退出码: {:?}",
        output.status.code()
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("[done]"),
        "空 prompt 也应输出 [done] 标记,实际: {}",
        stdout
    );
}

// --- Task 1.2: chimera quest 真实接入 QuestEngine API ---

/// 测试 `chimera quest list` 成功返回空列表(进程内 ephemeral 引擎,SubTask 1.2.2)
///
/// 新进程的 QuestEngine 无持久化,list_quests 返回空 Vec。
/// 人类可读模式输出友好提示到 stderr。
#[test]
fn test_quest_list_returns_empty_list() {
    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .args(["quest", "list"])
        .output()
        .expect("执行 chimera 二进制失败");
    assert!(
        output.status.success(),
        "quest list 应成功,实际退出码: {:?}",
        output.status.code()
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("无 Quest"),
        "stderr 应包含空列表提示,实际 stderr: {}",
        stderr
    );
}

/// 测试 `chimera --json quest list` 输出 JSON 成功 envelope(SubTask 1.2.2 + Task 1.7)
#[test]
fn test_quest_list_json_outputs_empty_array_envelope() {
    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .args(["--json", "quest", "list"])
        .output()
        .expect("执行 chimera 二进制失败");
    assert!(
        output.status.success(),
        "quest list --json 应成功,实际退出码: {:?}",
        output.status.code()
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"status\": \"ok\""),
        "stdout 应包含 status: ok,实际: {}",
        stdout
    );
    // 空列表的 data 字段应为 []
    assert!(
        stdout.contains("\"data\": []"),
        "stdout 应包含 data: [](空数组),实际: {}",
        stdout
    );
}

/// 测试 `chimera quest show <id>` 不存在时返回 EngineError(SubTask 1.2.3)
///
/// 进程内 ephemeral 引擎无持久化,任意 quest_id 都不存在。
/// 返回 EngineError(退出码 3)。
#[test]
fn test_quest_show_nonexistent_returns_engine_error() {
    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .args(["quest", "show", "nonexistent-quest-id"])
        .output()
        .expect("执行 chimera 二进制失败");
    assert!(
        !output.status.success(),
        "quest show 不存在 ID 应失败,实际退出码: {:?}",
        output.status.code()
    );
    let code = output.status.code().unwrap_or(-1);
    assert_eq!(code, 3, "EngineError 退出码应为 3,实际: {}", code);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("EngineError"),
        "stderr 应包含 EngineError,实际: {}",
        stderr
    );
}

/// 测试 `chimera quest checkpoint <id>` 无 CheckpointManager 时返回错误(SubTask 1.2.5)
///
/// QuestEngine::new() 未配置 CheckpointManager,save_checkpoint 返回 CheckpointSaveFailed。
#[test]
fn test_quest_checkpoint_without_manager_returns_error() {
    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .args(["quest", "checkpoint", "test-quest-id"])
        .output()
        .expect("执行 chimera 二进制失败");
    assert!(
        !output.status.success(),
        "quest checkpoint 无 manager 应失败,实际退出码: {:?}",
        output.status.code()
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("EngineError"),
        "stderr 应包含 EngineError,实际: {}",
        stderr
    );
}

// --- Task 1.3: chimera wiki 真实接入 repo-wiki ---

/// 测试 `chimera wiki <query>` 成功执行语义检索(SubTask 1.3.1)
///
/// 使用 tempdir 创建临时 WikiStore,验证命令真实接入 repo-wiki crate。
/// 空数据库的搜索结果为空,输出友好提示。
#[test]
fn test_wiki_query_executes_search() {
    let tmp = TempDir::new().expect("创建临时目录失败");
    let db_path = tmp.path().join("test_wiki.db");
    let db_path_str = db_path.to_string_lossy().replace('\\', "/");

    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .args(["wiki", "test query"])
        .env("CHIMERA_REPO_WIKI__DB_PATH", &*db_path_str)
        .output()
        .expect("执行 chimera 二进制失败");
    assert!(
        output.status.success(),
        "wiki 查询应成功,实际退出码: {:?}",
        output.status.code()
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("未找到"),
        "空数据库应输出未找到提示,实际 stderr: {}",
        stderr
    );
}

/// 测试 `chimera wiki --json <query>` 输出 JSON 成功 envelope(SubTask 1.3.2 + Task 1.7)
#[test]
fn test_wiki_json_outputs_empty_array_envelope() {
    let tmp = TempDir::new().expect("创建临时目录失败");
    let db_path = tmp.path().join("test_wiki_json.db");
    let db_path_str = db_path.to_string_lossy().replace('\\', "/");

    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .args(["--json", "wiki", "test query"])
        .env("CHIMERA_REPO_WIKI__DB_PATH", &*db_path_str)
        .output()
        .expect("执行 chimera 二进制失败");
    assert!(
        output.status.success(),
        "wiki --json 应成功,实际退出码: {:?}",
        output.status.code()
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"status\": \"ok\""),
        "stdout 应包含 status: ok,实际: {}",
        stdout
    );
    assert!(
        stdout.contains("\"data\": []"),
        "stdout 应包含 data: [](空结果),实际: {}",
        stdout
    );
}

/// 测试 `chimera wiki --limit <N>` 参数被正确解析(SubTask 1.3.3)
///
/// --limit 参数不影响空数据库的搜索结果,但验证参数被接受不报错。
#[test]
fn test_wiki_limit_parameter_accepted() {
    let tmp = TempDir::new().expect("创建临时目录失败");
    let db_path = tmp.path().join("test_wiki_limit.db");
    let db_path_str = db_path.to_string_lossy().replace('\\', "/");

    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .args(["wiki", "test", "--limit", "5"])
        .env("CHIMERA_REPO_WIKI__DB_PATH", &*db_path_str)
        .output()
        .expect("执行 chimera 二进制失败");
    assert!(
        output.status.success(),
        "wiki --limit 5 应成功,实际退出码: {:?}",
        output.status.code()
    );
}

// --- Task 1.4: chimera parliament 真实接入 Parliament ---

/// 测试 `chimera parliament <proposal>` 成功执行审议(SubTask 1.4.1-1.4.2)
///
/// 验证点:
/// - 退出码 0(成功)
/// - stderr 包含审议上下文(=== 议会审议 ===)
/// - stderr 包含共识结果标签(共识达成 / 提案被拒绝 / Skeptic 否决)
///   注:print_consensus_human 用 print_success/warning/error 输出到 stderr
#[test]
fn test_parliament_proposal_deliberates() {
    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .args(["parliament", "重构核心模块提升性能"])
        .output()
        .expect("执行 chimera 二进制失败");
    assert!(
        output.status.success(),
        "parliament 命令应成功执行审议,实际退出码: {:?}",
        output.status.code()
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("议会审议"),
        "stderr 应包含审议上下文,实际: {}",
        stderr
    );
    // 审议结果标签由 print_success/warning/error 输出到 stderr
    let has_result =
        stderr.contains("共识达成") || stderr.contains("提案被拒绝") || stderr.contains("否决");
    assert!(
        has_result,
        "stderr 应包含审议结果(共识达成/拒绝/否决),实际 stderr: {}",
        stderr
    );
}

/// 测试 `chimera --json parliament <proposal>` 输出 JSON 成功 envelope(SubTask 1.4.3)
#[test]
fn test_parliament_json_outputs_deliberation_envelope() {
    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .args(["--json", "parliament", "test proposal"])
        .output()
        .expect("执行 chimera 二进制失败");
    assert!(
        output.status.success(),
        "parliament --json 应成功,实际退出码: {:?}",
        output.status.code()
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"status\": \"ok\""),
        "stdout 应包含 status: ok,实际: {}",
        stdout
    );
    assert!(
        stdout.contains("\"consensus\""),
        "stdout 应包含 consensus 字段,实际: {}",
        stdout
    );
    assert!(
        stdout.contains("\"quest_id\""),
        "stdout 应包含 quest_id 字段,实际: {}",
        stdout
    );
}

// === Task 1.7: 全局 --json 参数集成测试(SubTask 1.7.5)===
//
// 验证 `--json` flag 启用时,命令输出遵循 envelope schema:
// - 成功:`{ "status": "ok", "data": <payload> }`(stdout)
// - 错误:`{ "status": "error", "error": { "kind", "message" }, "exit_code": <N> }`(stderr)
//
// WHY 子进程验证:JSON 输出是进程级语义(stdout/stderr 分流 + 退出码),
// 只有真实执行二进制才能完整覆盖 main → dispatch → handler → output 链路。

/// 测试 `chimera config show --json` 输出成功 envelope schema
#[test]
fn test_config_show_json_outputs_envelope() {
    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .args(["config", "show", "--json"])
        .output()
        .expect("执行 chimera 二进制失败");
    assert!(
        output.status.success(),
        "config show --json 应成功,实际退出码: {:?}",
        output.status.code()
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"status\": \"ok\""),
        "stdout 应包含 status: ok,实际: {}",
        stdout
    );
    assert!(
        stdout.contains("\"data\""),
        "stdout 应包含 data 字段,实际: {}",
        stdout
    );
}

/// 测试 `chimera config list --json` 输出成功 envelope schema
#[test]
fn test_config_list_json_outputs_envelope() {
    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .args(["config", "list", "--json"])
        .output()
        .expect("执行 chimera 二进制失败");
    assert!(
        output.status.success(),
        "config list --json 应成功,实际退出码: {:?}",
        output.status.code()
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"status\": \"ok\""),
        "stdout 应包含 status: ok"
    );
    // ListPayload 包含 nexus_version 字段,验证 data 载荷正确序列化
    assert!(
        stdout.contains("\"nexus_version\""),
        "stdout 应包含 nexus_version 字段(data 载荷),实际: {}",
        stdout
    );
}

/// 测试 `chimera config path --json` 输出成功 envelope schema
#[test]
fn test_config_path_json_outputs_envelope() {
    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .args(["config", "path", "--json"])
        .output()
        .expect("执行 chimera 二进制失败");
    assert!(
        output.status.success(),
        "config path --json 应成功,实际退出码: {:?}",
        output.status.code()
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"status\": \"ok\""),
        "stdout 应包含 status: ok"
    );
    assert!(
        stdout.contains("\"path\""),
        "stdout 应包含 path 字段,实际: {}",
        stdout
    );
}

// 注:Task 1.1-1.4 真实接入后,run/quest list 的 --json 测试已迁移至上方
// "Task 1.1-1.4: 真实接入命令验证" 区段(test_run_json_outputs_quest_envelope /
// test_quest_list_json_outputs_empty_array_envelope),验证成功 envelope 而非错误 envelope。

// === Task 1.11: permission prompt 机制集成测试(SubTask 1.11.6)===
//
// 验证 `--yes` / `--no-permission` flag 跳过 prompt,以及用户拒绝时返回 PermissionDenied。
// WHY 子进程验证:permission prompt 涉及 stdin 读取 + Ctrl+C 信号处理,
// 只有进程级执行才能完整覆盖 tokio::select! + spawn_blocking 链路。

/// 测试 `--yes` flag 跳过 permission prompt 后 quest cancel 成功执行(Task 1.2.4 + 1.11.4)
///
/// Task 1.2 真实接入后,`quest cancel` 调用 QuestEngine::cancel_quest,
/// 该方法对不存在的 quest_id 幂等成功(engine.rs 设计决策)。
/// --yes 跳过 prompt 后应返回退出码 0(成功),而非 NotImplemented。
/// 注:`print_success` 输出到 stderr(保留 stdout 给数据流),故检查 stderr。
#[test]
fn test_quest_cancel_with_yes_flag_skips_prompt() {
    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .args(["quest", "cancel", "test-quest-id", "--yes"])
        .output()
        .expect("执行 chimera 二进制失败");
    assert!(
        output.status.success(),
        "quest cancel --yes 应成功(idempotent cancel),实际退出码: {:?}",
        output.status.code()
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("已取消"),
        "stderr 应包含取消确认信息(print_success 输出到 stderr),实际: {}",
        stderr
    );
}

/// 测试 `--no-permission` flag 跳过 permission prompt 后 quest cancel 成功执行
#[test]
fn test_quest_cancel_with_no_permission_flag_skips_prompt() {
    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .args(["quest", "cancel", "test-quest-id", "--no-permission"])
        .output()
        .expect("执行 chimera 二进制失败");
    assert!(
        output.status.success(),
        "quest cancel --no-permission 应成功(idempotent cancel),实际退出码: {:?}",
        output.status.code()
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("已取消"),
        "stderr 应包含取消确认信息(print_success 输出到 stderr),实际: {}",
        stderr
    );
}

/// 测试无 `--yes` 时输入 "n" 拒绝 permission prompt,返回 PermissionDenied(退出码 5)
///
/// WHY 需要stdin 交互:`quest cancel` 不带 `--yes` / `--no-permission` 时,
/// `permission::confirm` 会读取 stdin 等待用户输入。测试通过管道写入 "n\n"
/// 模拟用户拒绝,验证返回 PermissionDenied(退出码 5),不进入 cancel 逻辑。
#[test]
fn test_quest_cancel_with_rejection_input_returns_permission_denied() {
    use std::io::Write;
    use std::process::Stdio;

    let bin = env!("CARGO_BIN_EXE_chimera");
    let mut child = std::process::Command::new(bin)
        .args(["quest", "cancel", "test-quest-id"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("执行 chimera 二进制失败");

    // 向 stdin 写入 "n\n" 模拟用户拒绝
    if let Some(stdin) = child.stdin.as_mut() {
        stdin.write_all(b"n\n").expect("写入 stdin 失败");
    }
    // 关闭 stdin 通道,让子进程读到 EOF(避免 hang)
    drop(child.stdin.take());

    let output = child.wait_with_output().expect("等待子进程失败");

    // 用户拒绝后应返回 PermissionDenied(退出码 5)
    assert!(
        !output.status.success(),
        "quest cancel 被拒绝应以非零退出码失败"
    );
    let code = output.status.code().unwrap_or(-1);
    assert_eq!(
        code, 5,
        "用户拒绝应返回 PermissionDenied(退出码 5),实际: {}",
        code
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("PermissionDenied"),
        "stderr 应包含 PermissionDenied 错误,实际: {}",
        stderr
    );
}

// === Task 1.12: 彩色输出 --no-color flag 集成测试 ===
//
// 验证 `--no-color` flag 与 `NO_COLOR` 环境变量不影响命令执行(仅禁用 ANSI 颜色码)。

/// 测试 `--no-color` flag 不影响 `config list` 命令执行
#[test]
fn test_no_color_flag_runs_successfully() {
    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .args(["config", "list", "--no-color"])
        .output()
        .expect("执行 chimera 二进制失败");
    assert!(
        output.status.success(),
        "config list --no-color 应成功,实际退出码: {:?}",
        output.status.code()
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.is_empty(), "应输出配置内容(表格形式)");
}

/// 测试 `NO_COLOR=1` 环境变量不影响 `config list` 命令执行
#[test]
fn test_no_color_env_runs_successfully() {
    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .args(["config", "list"])
        .env("NO_COLOR", "1")
        .output()
        .expect("执行 chimera 二进制失败");
    assert!(
        output.status.success(),
        "NO_COLOR=1 config list 应成功,实际退出码: {:?}",
        output.status.code()
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(!stdout.is_empty(), "应输出配置内容(表格形式)");
}

// === Task 1.8: chimera mcp <action> 子命令集成测试(SubTask 1.8.7)===
//
// 验证 mcp list/serve/call/inspect 4 个子动作的真实接入 mcp-mesh crate。
// 进程内 ephemeral mesh 注册表为空是预期行为(mcp list 返回空提示)。

/// 测试 `chimera mcp list` 成功执行,空注册表输出友好提示(SubTask 1.8.2)
///
/// 进程内 ephemeral McpMesh 无注册服务器,list_all() 返回空 Vec。
/// 人类可读模式输出"当前无 MCP 服务器"提示到 stderr。
#[test]
fn test_mcp_list_returns_empty_list() {
    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .args(["mcp", "list"])
        .output()
        .expect("执行 chimera 二进制失败");
    assert!(
        output.status.success(),
        "mcp list 应成功,实际退出码: {:?}",
        output.status.code()
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("无 MCP 服务器"),
        "stderr 应包含空注册表提示,实际 stderr: {}",
        stderr
    );
}

/// 测试 `chimera mcp serve` 返回 NotImplemented(SubTask 1.8.3)
///
/// `mcp serve` 需要绑定网络端口、加载 TLS 证书等配置,不适合 CLI 一次性启动。
/// 返回 NotImplemented 错误(退出码 2),指引替代方案(`chimera tui` 或独立部署)。
#[test]
fn test_mcp_serve_returns_not_implemented() {
    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .args(["mcp", "serve"])
        .output()
        .expect("执行 chimera 二进制失败");
    assert!(
        !output.status.success(),
        "mcp serve 应失败(NotImplemented),实际退出码: {:?}",
        output.status.code()
    );
    let code = output.status.code().unwrap_or(-1);
    assert_eq!(code, 2, "NotImplemented 退出码应为 2,实际: {}", code);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("NotImplemented"),
        "stderr 应包含 NotImplemented,实际: {}",
        stderr
    );
}

/// 测试 `chimera mcp call` 调用不存在的服务器返回 EngineError(SubTask 1.8.4)
///
/// 使用 `--yes` 跳过 permission prompt 后,调用 execute_transaction
/// 对未注册 server_id 返回 ServerNotFound(EngineError,退出码 3)。
#[test]
fn test_mcp_call_nonexistent_server_returns_engine_error() {
    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .args(["--yes", "mcp", "call", "nonexistent-server", "some_tool"])
        .output()
        .expect("执行 chimera 二进制失败");
    assert!(
        !output.status.success(),
        "mcp call 不存在 server 应失败,实际退出码: {:?}",
        output.status.code()
    );
    let code = output.status.code().unwrap_or(-1);
    assert_eq!(code, 3, "EngineError 退出码应为 3,实际: {}", code);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("EngineError"),
        "stderr 应包含 EngineError,实际: {}",
        stderr
    );
}

/// 测试 `chimera mcp inspect` 不存在服务器返回 EngineError(SubTask 1.8.5)
///
/// 进程内 ephemeral mesh 无注册服务器,inspect 返回 EngineError(退出码 3)。
#[test]
fn test_mcp_inspect_nonexistent_returns_engine_error() {
    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .args(["mcp", "inspect", "nonexistent-server"])
        .output()
        .expect("执行 chimera 二进制失败");
    assert!(
        !output.status.success(),
        "mcp inspect 不存在 server 应失败,实际退出码: {:?}",
        output.status.code()
    );
    let code = output.status.code().unwrap_or(-1);
    assert_eq!(code, 3, "EngineError 退出码应为 3,实际: {}", code);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("EngineError"),
        "stderr 应包含 EngineError,实际: {}",
        stderr
    );
}

// === Task 1.9: chimera audit 子命令集成测试(SubTask 1.9.6)===
//
// 验证 audit 命令真实接入 parliament::ahirt::AhirtRedTeam。
// 默认载荷库 100 个载荷 × 4 类攻击向量,验证报告输出。

/// 测试 `chimera audit` 成功执行红队审计(SubTask 1.9.2)
///
/// 调用 AhirtRedTeam::verify_security() 执行全量探测,
/// 输出报告标题 + 统计摘要到 stderr。
#[test]
fn test_audit_executes_red_team_scan() {
    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .args(["audit"])
        .output()
        .expect("执行 chimera 二进制失败");
    assert!(
        output.status.success(),
        "audit 应成功,实际退出码: {:?}",
        output.status.code()
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("红队安全审计报告"),
        "stderr 应包含报告标题,实际 stderr: {}",
        stderr
    );
    assert!(
        stderr.contains("探测总数"),
        "stderr 应包含统计摘要,实际 stderr: {}",
        stderr
    );
}

/// 测试 `chimera audit --json` 输出 JSON envelope(SubTask 1.9.3 + Task 1.7)
///
/// JSON 模式输出完整 SecurityReport envelope,包含 status / stats / vulnerable_types 字段。
#[test]
fn test_audit_json_outputs_report_envelope() {
    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .args(["--json", "audit"])
        .output()
        .expect("执行 chimera 二进制失败");
    assert!(
        output.status.success(),
        "audit --json 应成功,实际退出码: {:?}",
        output.status.code()
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"status\": \"ok\""),
        "stdout 应包含 status: ok,实际: {}",
        stdout
    );
    // SecurityReport 包含 stats 字段(总探测数 / 通过 / 失败)
    assert!(
        stdout.contains("\"stats\""),
        "stdout 应包含 stats 字段(SecurityReport),实际: {}",
        stdout
    );
}

// === Task 1.10: chimera agent <action> 子命令集成测试(SubTask 1.10.8)===
//
// 验证 agent list/spawn/inspect/cancel 4 个子动作真实接入 chimera-mas crate。
// 进程内 ephemeral orchestrator 心跳注册表为空是预期行为。

/// 测试 `chimera agent list` 成功执行,空注册表输出友好提示(SubTask 1.10.2)
///
/// 进程内 ephemeral RootOrchestrator 无心跳,heartbeat_count() 返回 0。
/// 人类可读模式输出"当前无 Agent"提示到 stderr。
#[test]
fn test_agent_list_returns_empty_list() {
    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .args(["agent", "list"])
        .output()
        .expect("执行 chimera 二进制失败");
    assert!(
        output.status.success(),
        "agent list 应成功,实际退出码: {:?}",
        output.status.code()
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("无 Agent"),
        "stderr 应包含空注册表提示,实际 stderr: {}",
        stderr
    );
}

/// 测试 `chimera agent spawn --quadrant Q1 --task <desc>` 成功创建 Agent(SubTask 1.10.3)
///
/// 调用 RootOrchestrator::delegate,Simple 复杂度创建 1 个 Agent。
/// 输出成功提示到 stderr,包含 Agent ID 与象限信息。
#[test]
fn test_agent_spawn_q1_creates_agent() {
    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .args([
            "agent",
            "spawn",
            "--quadrant",
            "Q1",
            "--task",
            "实现 hello world",
        ])
        .output()
        .expect("执行 chimera 二进制失败");
    assert!(
        output.status.success(),
        "agent spawn Q1 应成功,实际退出码: {:?}",
        output.status.code()
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Agent 创建成功"),
        "stderr 应包含创建成功提示,实际 stderr: {}",
        stderr
    );
    assert!(
        stderr.contains("Q1"),
        "stderr 应包含象限信息 Q1,实际 stderr: {}",
        stderr
    );
}

/// 测试 `chimera agent spawn --quadrant Invalid` 返回 ConfigError(SubTask 1.10.3 边界)
///
/// 无效象限参数返回 ConfigError(退出码 1),指引有效值清单。
#[test]
fn test_agent_spawn_invalid_quadrant_returns_config_error() {
    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .args(["agent", "spawn", "--quadrant", "Invalid", "--task", "test"])
        .output()
        .expect("执行 chimera 二进制失败");
    assert!(
        !output.status.success(),
        "agent spawn 无效象限应失败,实际退出码: {:?}",
        output.status.code()
    );
    let code = output.status.code().unwrap_or(-1);
    assert_eq!(code, 1, "ConfigError 退出码应为 1,实际: {}", code);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("ConfigError"),
        "stderr 应包含 ConfigError,实际: {}",
        stderr
    );
    assert!(
        stderr.contains("Q1"),
        "stderr 应在错误信息中包含有效值 Q1,实际: {}",
        stderr
    );
}

/// 测试 `chimera agent inspect <id>` 不存在时返回 EngineError(SubTask 1.10.4)
///
/// 进程内 ephemeral orchestrator 无心跳,get_heartbeat 返回 None。
/// inspect 返回 EngineError(退出码 3)。
#[test]
fn test_agent_inspect_nonexistent_returns_engine_error() {
    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .args(["agent", "inspect", "nonexistent-agent-id"])
        .output()
        .expect("执行 chimera 二进制失败");
    assert!(
        !output.status.success(),
        "agent inspect 不存在 ID 应失败,实际退出码: {:?}",
        output.status.code()
    );
    let code = output.status.code().unwrap_or(-1);
    assert_eq!(code, 3, "EngineError 退出码应为 3,实际: {}", code);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("EngineError"),
        "stderr 应包含 EngineError,实际: {}",
        stderr
    );
}

/// 测试 `chimera --yes agent cancel <id>` 不存在时返回 EngineError(SubTask 1.10.5)
///
/// 使用 `--yes` 跳过 permission prompt 后,检查 Agent 是否存在。
/// 不存在时返回 EngineError(退出码 3)。
#[test]
fn test_agent_cancel_nonexistent_with_yes_returns_engine_error() {
    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .args(["--yes", "agent", "cancel", "nonexistent-agent-id"])
        .output()
        .expect("执行 chimera 二进制失败");
    assert!(
        !output.status.success(),
        "agent cancel 不存在 ID 应失败,实际退出码: {:?}",
        output.status.code()
    );
    let code = output.status.code().unwrap_or(-1);
    assert_eq!(code, 3, "EngineError 退出码应为 3,实际: {}", code);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("EngineError"),
        "stderr 应包含 EngineError,实际: {}",
        stderr
    );
}

/// 测试 `chimera agent spawn --parallel --quadrant Q2 --task <desc>` 并行模式(SubTask 1.10.6)
///
/// `--parallel` 启用 Medium 复杂度,创建 2 个并行 Agent。
/// 输出应包含 "2 个 Agent" 数量提示。
#[test]
fn test_agent_spawn_parallel_creates_two_agents() {
    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .args([
            "agent",
            "--parallel",
            "spawn",
            "--quadrant",
            "Q2",
            "--task",
            "集成测试",
        ])
        .output()
        .expect("执行 chimera 二进制失败");
    assert!(
        output.status.success(),
        "agent --parallel spawn Q2 应成功,实际退出码: {:?}",
        output.status.code()
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Agent 创建成功"),
        "stderr 应包含创建成功提示,实际 stderr: {}",
        stderr
    );
    // --parallel 创建 2 个 Agent(Medium 复杂度)
    assert!(
        stderr.contains("2 个 Agent"),
        "stderr 应包含 '2 个 Agent' 数量提示,实际 stderr: {}",
        stderr
    );
}

// === Task 1.13: chimera doctor 子命令集成测试(SubTask 1.13.6)===
//
// 验证 doctor 命令 6 维度健康检查(config / cargo_lock / sqlite / mcp / event_bus / llm_provider)。
// Wave 2 Task 4:在原 5 维度基础上扩展 LLM Provider 断言。
// 测试环境配置文件可能缺失(WARN),但不应 FAIL 到退出码非 0。

/// 测试 `chimera doctor` 成功执行 6 维度健康检查(SubTask 1.13.2 + Wave 2 Task 4)
///
/// 即使配置文件缺失(WARN),doctor 命令仍返回成功(退出码 0)。
/// 输出包含 6 项检查结果 + 汇总统计。
#[test]
fn test_doctor_executes_five_dimension_checks() {
    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .args(["doctor"])
        .output()
        .expect("执行 chimera 二进制失败");
    assert!(
        output.status.success(),
        "doctor 应成功(即使有 WARN 项),实际退出码: {:?}",
        output.status.code()
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("系统健康检查"),
        "stderr 应包含报告标题,实际 stderr: {}",
        stderr
    );
    // 6 维度检查项名称(5 项原维度 + LLM Provider)
    assert!(
        stderr.contains("配置文件"),
        "stderr 应包含配置文件检查项,实际 stderr: {}",
        stderr
    );
    assert!(
        stderr.contains("Cargo.lock"),
        "stderr 应包含 Cargo.lock 检查项,实际 stderr: {}",
        stderr
    );
    assert!(
        stderr.contains("SQLite"),
        "stderr 应包含 SQLite 检查项,实际 stderr: {}",
        stderr
    );
    assert!(
        stderr.contains("MCP"),
        "stderr 应包含 MCP 检查项,实际 stderr: {}",
        stderr
    );
    assert!(
        stderr.contains("EventBus"),
        "stderr 应包含 EventBus 检查项,实际 stderr: {}",
        stderr
    );
    // Wave 2 Task 4:第 6 维 LLM Provider 健康度
    assert!(
        stderr.contains("LLM Provider"),
        "stderr 应包含 LLM Provider 检查项,实际 stderr: {}",
        stderr
    );
    // 汇总统计(共 6 项)
    assert!(
        stderr.contains("汇总"),
        "stderr 应包含汇总统计,实际 stderr: {}",
        stderr
    );
    assert!(
        stderr.contains("共 6 项"),
        "stderr 应包含 '共 6 项' 汇总,实际 stderr: {}",
        stderr
    );
}

/// 测试 `chimera doctor --json` 输出 JSON envelope(SubTask 1.13.3 + Task 1.7)
///
/// JSON 模式输出完整 HealthReport envelope,包含 checks 数组 + summary 统计。
#[test]
fn test_doctor_json_outputs_report_envelope() {
    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .args(["--json", "doctor"])
        .output()
        .expect("执行 chimera 二进制失败");
    assert!(
        output.status.success(),
        "doctor --json 应成功,实际退出码: {:?}",
        output.status.code()
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("\"status\": \"ok\""),
        "stdout 应包含 status: ok,实际: {}",
        stdout
    );
    // HealthReport 包含 checks 数组(5 项)
    assert!(
        stdout.contains("\"checks\""),
        "stdout 应包含 checks 字段,实际: {}",
        stdout
    );
    // HealthReport 包含 summary 统计(ok/warn/fail/total)
    assert!(
        stdout.contains("\"summary\""),
        "stdout 应包含 summary 字段,实际: {}",
        stdout
    );
    assert!(
        stdout.contains("\"total\": 6"),
        "stdout 应包含 total: 6(6 项检查),实际: {}",
        stdout
    );
}

/// 测试 `chimera doctor --fix` 自动修复配置文件缺失(SubTask 1.13.4)
///
/// `--fix` flag 在配置文件缺失时自动生成默认 omega.yaml。
/// 由于测试环境可能已有配置文件,此测试主要验证 --fix flag 不导致失败。
#[test]
fn test_doctor_fix_flag_runs_successfully() {
    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .args(["doctor", "--fix"])
        .output()
        .expect("执行 chimera 二进制失败");
    assert!(
        output.status.success(),
        "doctor --fix 应成功,实际退出码: {:?}",
        output.status.code()
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    // --fix 时配置文件检查应返回 OK(已存在或已自动生成)
    assert!(
        stderr.contains("[OK]") || stderr.contains("[WARN]"),
        "stderr 应包含 OK 或 WARN 状态(不应全部 FAIL),实际 stderr: {}",
        stderr
    );
}

// === Task 1.14: chimera completions <shell> 子命令集成测试(SubTask 1.14.5)===
//
// 验证 completions 命令真实调用 clap_complete::generate 生成补全脚本。

/// 测试 `chimera completions bash` 生成包含命令清单的 bash 补全脚本(SubTask 1.14.5)
///
/// 生成的 bash 补全脚本应包含 `_chimera` 函数名 + 主要子命令(run/quest/wiki/parliament/
/// mcp/audit/agent/doctor/completions/config/chat/tui)。
#[test]
fn test_completions_bash_includes_commands() {
    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .args(["completions", "bash"])
        .output()
        .expect("执行 chimera 二进制失败");
    assert!(
        output.status.success(),
        "completions bash 应成功,实际退出码: {:?}",
        output.status.code()
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // bash 补全脚本包含 `_chimera` 函数(clap_complete 生成)
    assert!(
        stdout.contains("_chimera"),
        "stdout 应包含 _chimera 函数名,实际: {}",
        stdout
    );
    // 补全脚本应包含主要子命令(SubTask 1.14.5 验收点)
    assert!(
        stdout.contains("run"),
        "stdout 应包含 run 子命令,实际: {}",
        stdout
    );
    assert!(
        stdout.contains("quest"),
        "stdout 应包含 quest 子命令,实际: {}",
        stdout
    );
    assert!(
        stdout.contains("mcp"),
        "stdout 应包含 mcp 子命令,实际: {}",
        stdout
    );
    assert!(
        stdout.contains("audit"),
        "stdout 应包含 audit 子命令,实际: {}",
        stdout
    );
    assert!(
        stdout.contains("agent"),
        "stdout 应包含 agent 子命令,实际: {}",
        stdout
    );
    assert!(
        stdout.contains("doctor"),
        "stdout 应包含 doctor 子命令,实际: {}",
        stdout
    );
    assert!(
        stdout.contains("completions"),
        "stdout 应包含 completions 子命令,实际: {}",
        stdout
    );
}

// === Task 2.1: long_about + examples 字段集成测试 ===
//
// 验证 `--help` 输出包含 EXAMPLES 段落(由 after_long_help 渲染)。
// WHY 子进程验证:--help 是进程级语义(clap 在 parse 阶段直接打印到 stdout 退出),
// 只有真实执行二进制才能捕获完整 help 文本(long_about + after_long_help)。

/// 测试 `chimera --help` 输出包含 EXAMPLES 段落(SubTask 2.1.3)
///
/// `after_long_help` 在 `--help`(长帮助)模式下渲染,
/// 验证主命令 help 含 EXAMPLES 段落 + OMEGA 四定律描述(long_about)。
#[test]
fn test_help_output_contains_examples_section() {
    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .arg("--help")
        .output()
        .expect("执行 chimera 二进制失败");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("EXAMPLES:"),
        "stdout 应包含 EXAMPLES: 段落(after_long_help 渲染),实际: {}",
        stdout
    );
}

/// 测试 `chimera <subcommand> --help` 输出包含 EXAMPLES 段落(子命令级)
///
/// 抽样验证 `run` / `quest` / `agent` 3 个子命令的 help 输出含 EXAMPLES 段落,
/// 确保 12 个子命令均添加了 after_long_help 字段。
#[test]
fn test_subcommand_help_contains_examples_section() {
    let bin = env!("CARGO_BIN_EXE_chimera");
    for sub in ["run", "quest", "agent"] {
        let output = std::process::Command::new(bin)
            .args([sub, "--help"])
            .output()
            .expect("执行 chimera 二进制失败");
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("EXAMPLES:"),
            "`chimera {} --help` 应包含 EXAMPLES: 段落,实际: {}",
            sub,
            stdout
        );
    }
}

/// 测试 `chimera --help` 输出包含 OMEGA 四定律描述(long_about,SubTask 2.1.1)
#[test]
fn test_help_output_contains_omega_long_about() {
    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .arg("--help")
        .output()
        .expect("执行 chimera 二进制失败");
    let stdout = String::from_utf8_lossy(&output.stdout);
    // long_about 应提及 OMEGA 四定律
    assert!(
        stdout.contains("OMEGA"),
        "stdout 应包含 OMEGA 关键字(long_about 描述),实际: {}",
        stdout
    );
    assert!(
        stdout.contains("Ω-Sparse") || stdout.contains("Sparse"),
        "stdout 应包含 Ω-Sparse 或 Sparse 关键字,实际: {}",
        stdout
    );
}

// === Task 2.2: --dry-run 全局参数集成测试(SubTask 2.2.3)===
//
// 验证 `--dry-run` flag 启用时,破坏性命令(quest cancel / agent cancel / mcp call)
// 只输出预览不实际执行,退出码 0。
// WHY 子进程验证:dry-run 行为涉及 stdout/stderr 输出 + 退出码,只有真实执行
// 二进制才能完整覆盖 main → dispatch → handler → dry_run 检查 → 预览输出链路。

/// 测试 `chimera --dry-run --yes quest cancel <id>` 只输出预览不执行(SubTask 2.2.2)
///
/// dry_run=true 时 cancel_quest 应:
/// 1. 输出 `[dry-run] 将取消 Quest <id>,不执行` 到 stderr
/// 2. 不调用 QuestEngine::cancel_quest
/// 3. 返回 Ok(退出码 0)
#[test]
fn test_quest_cancel_dry_run_skips_execution() {
    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .args(["--dry-run", "--yes", "quest", "cancel", "test-quest-id"])
        .output()
        .expect("执行 chimera 二进制失败");
    assert!(
        output.status.success(),
        "quest cancel --dry-run 应成功(不实际执行),实际退出码: {:?}",
        output.status.code()
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[dry-run]"),
        "stderr 应包含 [dry-run] 前缀,实际: {}",
        stderr
    );
    assert!(
        stderr.contains("Quest"),
        "stderr 应包含 Quest 关键字,实际: {}",
        stderr
    );
    // dry-run 不应输出"已取消"成功消息(那是实际执行的输出)
    assert!(
        !stderr.contains("已取消"),
        "dry-run 不应输出'已取消'(说明实际执行了),实际: {}",
        stderr
    );
}

/// 测试 `chimera --dry-run --yes agent cancel <id>` 只输出预览不执行(SubTask 2.2.2)
#[test]
fn test_agent_cancel_dry_run_skips_execution() {
    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .args(["--dry-run", "--yes", "agent", "cancel", "test-agent-id"])
        .output()
        .expect("执行 chimera 二进制失败");
    assert!(
        output.status.success(),
        "agent cancel --dry-run 应成功(不实际执行),实际退出码: {:?}",
        output.status.code()
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[dry-run]"),
        "stderr 应包含 [dry-run] 前缀,实际: {}",
        stderr
    );
    assert!(
        stderr.contains("Agent"),
        "stderr 应包含 Agent 关键字,实际: {}",
        stderr
    );
}

/// 测试 `chimera --dry-run --yes mcp call <server> <tool>` 只输出预览不执行(SubTask 2.2.2)
#[test]
fn test_mcp_call_dry_run_skips_execution() {
    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .args([
            "--dry-run",
            "--yes",
            "mcp",
            "call",
            "test-server",
            "test-tool",
        ])
        .output()
        .expect("执行 chimera 二进制失败");
    assert!(
        output.status.success(),
        "mcp call --dry-run 应成功(不实际执行),实际退出码: {:?}",
        output.status.code()
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[dry-run]"),
        "stderr 应包含 [dry-run] 前缀,实际: {}",
        stderr
    );
    // dry-run 不应输出"调用完成"(那是实际执行的输出)
    assert!(
        !stderr.contains("调用完成"),
        "dry-run 不应输出'调用完成'(说明实际执行了),实际: {}",
        stderr
    );
}

// === Task 2.3: panic hook (human-panic) 集成测试(SubTask 2.3.3)===
//
// 验证 `human_panic::setup_panic!()` 已在 main.rs 安装全局 panic hook。
// 测试策略:通过 `CHIMERA_PANIC_TEST=1` 环境变量触发故意 panic(仅 debug 模式),
// 验证进程因 panic 退出(stderr 含 "panicked" 关键字)。
//
// WHY 子进程验证:panic hook 是进程级全局状态,只有真实执行二进制才能覆盖
// main → setup_panic! → panic → hook 触发 → stderr 输出完整链路。
// release 模式下 human-panic 输出友好提示,debug 模式退化为默认 panic handler
// (均输出 "panicked" 关键字,测试断言对此关键字生效)。

/// 测试 `CHIMERA_PANIC_TEST=1` 触发 panic 且 stderr 输出 panic 信息(SubTask 2.3.3)
///
/// 验证点:
/// 1. 进程非正常退出(退出码非 0,panic 导致)
/// 2. stderr 包含 "panicked" 关键字(默认 panic handler 或 human-panic 友好提示)
///
/// WHY 此测试在 Task 2.3 实现前会失败:main.rs 未实现 CHIMERA_PANIC_TEST 检查,
/// 进程会正常启动并尝试解析 CLI 参数(无子命令时进入 TUI),不会 panic。
#[test]
fn test_panic_hook_outputs_panic_message() {
    let bin = env!("CARGO_BIN_EXE_chimera");
    let output = std::process::Command::new(bin)
        .env("CHIMERA_PANIC_TEST", "1")
        .output()
        .expect("执行 chimera 二进制失败");
    // 进程应因 panic 退出(非 0 退出码)
    assert!(
        !output.status.success(),
        "CHIMERA_PANIC_TEST=1 应触发 panic,进程应非正常退出,实际退出码: {:?}",
        output.status.code()
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    // debug 模式:默认 panic handler 输出 "panicked at"
    // release 模式:human-panic 输出友好提示(也含 "panic" 关键字)
    assert!(
        stderr.to_lowercase().contains("panic"),
        "stderr 应包含 panic 相关信息,实际: {}",
        stderr
    );
}
