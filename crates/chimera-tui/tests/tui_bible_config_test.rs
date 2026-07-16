//! TuiBible 配置加载器集成测试(Task 3.2 — TDD-RED→GREEN)
//!
//! 验证 Figment 4 源合并(默认 < `~/.chimera/tui_bible.yaml` < env `CHIMERA_BIBLE_*` < CLI):
//! 1. 默认配置加载(无文件无 env)
//! 2. 用户 YAML 覆盖默认
//! 3. 环境变量 `CHIMERA_BIBLE_*` 覆盖 YAML
//!
//! WHY 隔离:每个测试用 `tempfile::TempDir` 创建独立 HOME 目录,
//! 通过 `set_var`/`remove_var` 隔离环境变量,避免相互污染与系统污染。
//! 并通过 `static ENV_LOCK` 互斥锁串行化所有测试,避免并行测试间
//! 共享 std::env 全局状态导致的环境变量竞态。

use chimera_tui::config::tui_bible::TuiBible;
use std::sync::Mutex;
use tempfile::TempDir;

/// 串行化所有 std::env 操作的全局互斥锁
///
/// WHY:std::env::set_var/remove_var 是进程级全局状态,cargo test 默认并行
/// 运行测试用例会导致环境变量竞态(测试 A 还在用 env 时,测试 B 已清空)。
/// 通过 Mutex 让所有 TuiBible 相关测试串行执行,避免竞态。
static ENV_LOCK: Mutex<()> = Mutex::new(());

// ============================================================
// 工具函数:在测试作用域内隔离 HOME 与 CHIMERA_BIBLE_* 环境变量
// ============================================================

/// 临时设置 HOME 指向给定目录(覆盖 `~/.chimera/tui_bible.yaml` 路径解析),
/// 并清理所有 `CHIMERA_BIBLE_*` 环境变量,确保测试间无相互污染。
///
/// WHY unsafe 注释:std::env::set_var 在多线程测试中不安全(race),
/// 但本测试通过 `ENV_LOCK` 串行化所有调用,规避了竞态。
fn isolate_env(home: &std::path::Path) {
    // 先清理可能影响测试的 CHIMERA_BIBLE_* env,避免上次测试残留
    for (key, _) in std::env::vars() {
        if key.starts_with("CHIMERA_BIBLE_") {
            std::env::remove_var(&key);
        }
    }
    std::env::set_var("HOME", home);
    std::env::set_var("USERPROFILE", home);
}

/// 恢复 HOME 与 USERPROFILE 到调用前值,清理 CHIMERA_BIBLE_*。
///
/// WHY 单独 restore 函数:测试间需要完全隔离的环境,避免一个测试设置
/// 的 HOME 污染另一个测试的文件路径解析。
fn restore_env(original_home: Option<String>, original_userprofile: Option<String>) {
    match original_home {
        Some(v) => std::env::set_var("HOME", v),
        None => std::env::remove_var("HOME"),
    }
    match original_userprofile {
        Some(v) => std::env::set_var("USERPROFILE", v),
        None => std::env::remove_var("USERPROFILE"),
    }
    for (key, _) in std::env::vars() {
        if key.starts_with("CHIMERA_BIBLE_") {
            std::env::remove_var(&key);
        }
    }
}

// ============================================================
// 测试 1:加载默认 bible(无文件无 env → 全部使用 Default::default())
// ============================================================

#[test]
fn test_load_default_bible() {
    // 加锁串行化,避免并行测试污染 std::env 全局状态
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    // 隔离环境:指向一个空临时目录,确保不读取到真实 ~/.chimera/tui_bible.yaml
    let temp = TempDir::new().expect("tempdir");
    let original_home = std::env::var("HOME").ok();
    let original_userprofile = std::env::var("USERPROFILE").ok();
    isolate_env(temp.path());

    let result = TuiBible::load();

    // 不管什么结果,环境要恢复
    restore_env(original_home, original_userprofile);

    // 加载应成功(无文件无 env 时使用默认值)
    let bible = result.expect("TuiBible::load should succeed with default values");
    // 默认主题:沿用 Theme::Dark(TuiConfig 默认)
    assert_eq!(bible.theme, chimera_tui::Theme::Dark);
    // 默认布局:沿用 LayoutMode::DualPane
    assert_eq!(bible.layout.mode, chimera_tui::LayoutMode::DualPane);
    // 默认 main_panel_ratio 与 TuiConfig 一致(0.7)
    assert!((bible.layout.main_panel_ratio - 0.7).abs() < 1e-6);
    // 默认 key_bindings 至少包含 "quit" 绑定
    assert!(
        bible.key_bindings.contains_key("quit"),
        "default key_bindings should contain 'quit', got keys: {:?}",
        bible.key_bindings.keys().collect::<Vec<_>>()
    );
    // 默认 thresholds 非空(至少包含 CPU/内存告警阈值)
    assert!(
        !bible.thresholds.is_empty(),
        "default thresholds should not be empty"
    );
}

// ============================================================
// 测试 2:用户 YAML 覆盖默认(写入 ~/.chimera/tui_bible.yaml → 加载后字段值变化)
// ============================================================

#[test]
fn test_load_user_yaml_overrides() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = TempDir::new().expect("tempdir");
    let original_home = std::env::var("HOME").ok();
    let original_userprofile = std::env::var("USERPROFILE").ok();
    isolate_env(temp.path());

    // 在临时 HOME 下创建 .chimera/tui_bible.yaml
    let chimera_dir = temp.path().join(".chimera");
    std::fs::create_dir_all(&chimera_dir).expect("create .chimera");
    let yaml_path = chimera_dir.join("tui_bible.yaml");
    let yaml_content = r#"
theme: Light
color_scheme:
  accent: BrightBlue
  warning: BrightYellow
key_bindings:
  quit:
    action: quit
    key: "q"
    description: "退出 TUI"
  next_panel:
    action: next_panel
    key: "j"
thresholds:
  cpu_warning: 0.75
  cpu_critical: 0.95
  memory_warning: 0.80
layout:
  mode: TriplePane
  main_panel_ratio: 0.6
  log_panel_height: 10
  sidebar_width: 30
"#;
    std::fs::write(&yaml_path, yaml_content).expect("write yaml");

    let result = TuiBible::load();

    restore_env(original_home, original_userprofile);

    let bible = result.expect("load should succeed with user yaml");
    // 主题被 YAML 覆盖
    assert_eq!(bible.theme, chimera_tui::Theme::Light);
    // ColorScheme 字段被 YAML 覆盖
    assert_eq!(
        bible.color_scheme.accent,
        Some(chimera_tui::ColorKind::BrightBlue)
    );
    assert_eq!(
        bible.color_scheme.warning,
        Some(chimera_tui::ColorKind::BrightYellow)
    );
    // key_bindings:YAML 定义的 quit 应覆盖默认的 quit
    // (Figment 浅合并语义:同名 key 覆盖,不同名 key 保留)
    assert_eq!(bible.key_bindings.get("quit").unwrap().key, "q");
    assert_eq!(bible.key_bindings.get("next_panel").unwrap().key, "j");
    // YAML 覆盖的 quit 描述应为用户值
    assert_eq!(
        bible.key_bindings.get("quit").unwrap().description,
        Some("退出 TUI".to_string())
    );
    // thresholds:浅合并(同名 key 覆盖,不同名 key 保留)
    assert!((bible.thresholds.get("cpu_warning").unwrap() - 0.75).abs() < 1e-6);
    assert!((bible.thresholds.get("cpu_critical").unwrap() - 0.95).abs() < 1e-6);
    // memory_critical 是默认阈值,YAML 未覆盖,应保留默认值 0.95
    assert!(
        (bible.thresholds.get("memory_critical").unwrap() - 0.95).abs() < 1e-6,
        "memory_critical should retain default 0.95"
    );
    // layout 应被 YAML 完全覆盖(单值字段,非 HashMap)
    assert_eq!(bible.layout.mode, chimera_tui::LayoutMode::TriplePane);
    assert!((bible.layout.main_panel_ratio - 0.6).abs() < 1e-6);
    assert_eq!(bible.layout.log_panel_height, 10);
    assert_eq!(bible.layout.sidebar_width, 30);
}

// ============================================================
// 测试 3:环境变量 CHIMERA_BIBLE_* 覆盖 YAML
// ============================================================
//
// Figment Env provider 解析规则(参考 chimera-cli/src/config.rs):
// - 前缀 CHIMERA_BIBLE_
// - 嵌套字段用 __ 分隔(例如 CHIMERA_BIBLE_THEME=Light)
// - 点号路径也常见,但本项目统一用 __
// - 标量字段(string/int/float/bool)直接读取
// - 结构体/HashMap 等复杂类型 env provider 无法直接表达,
//   所以本测试只覆盖标量字段(theme / main_panel_ratio)的 env override。

#[test]
fn test_env_var_chimera_bible_override() {
    let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let temp = TempDir::new().expect("tempdir");
    let original_home = std::env::var("HOME").ok();
    let original_userprofile = std::env::var("USERPROFILE").ok();
    isolate_env(temp.path());

    // 先写一个 YAML,设置 theme=Light
    let chimera_dir = temp.path().join(".chimera");
    std::fs::create_dir_all(&chimera_dir).expect("create .chimera");
    let yaml_path = chimera_dir.join("tui_bible.yaml");
    let yaml_content = r#"
theme: Light
layout:
  mode: DualPane
  main_panel_ratio: 0.5
  log_panel_height: 8
  sidebar_width: 25
"#;
    std::fs::write(&yaml_path, yaml_content).expect("write yaml");

    // 然后通过 env 覆盖为 HighContrast 与 0.8
    std::env::set_var("CHIMERA_BIBLE_THEME", "HighContrast");
    std::env::set_var("CHIMERA_BIBLE_LAYOUT__MAIN_PANEL_RATIO", "0.8");

    let result = TuiBible::load();

    restore_env(original_home, original_userprofile);

    let bible = result.expect("load should succeed with env override");
    // env 覆盖 YAML:theme 应该是 HighContrast
    assert_eq!(bible.theme, chimera_tui::Theme::HighContrast);
    // env 覆盖 YAML:layout.main_panel_ratio 应该是 0.8
    assert!(
        (bible.layout.main_panel_ratio - 0.8).abs() < 1e-6,
        "env override should set main_panel_ratio to 0.8, got {}",
        bible.layout.main_panel_ratio
    );
    // 未被 env 覆盖的字段仍来自 YAML(layout.mode)
    assert_eq!(bible.layout.mode, chimera_tui::LayoutMode::DualPane);
}
