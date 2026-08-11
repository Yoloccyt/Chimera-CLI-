//! TuiBible 接线闭环集成测试 — Concord 重构 T1.5(P4①)
//!
//! 对应架构层:L10 Interface(`chimera-tui`)
//!
//! 验证 `chimera-cli/commands/tui.rs` 接线链路的可测部分:
//! 四源 Figment 加载 → `TuiConfig::apply_bible_overrides` → `validate()`。
//! CLI 侧的"损坏文件回退默认 + warn"语义以分支复刻方式覆盖(进程隔离,
//! 不依赖真实 HOME)。
//!
//! WHY 独立文件:既有 `tui_bible_config_test.rs` 为 GBK 编码历史文件,
//! 新增 UTF-8 测试避免编码混写;env 隔离用本文件自有互斥锁(测试二进制
//! 为独立进程,与既有文件的 ENV_LOCK 无跨进程竞争)。

#![forbid(unsafe_code)]

use chimera_tui::config::tui_bible::TuiBible;
use chimera_tui::{Theme, TuiConfig};
use std::sync::Mutex;
use tempfile::TempDir;

/// 本文件内 env 变更互斥锁(std::env 为进程级全局状态,串行化防竞态)
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// 将 HOME/USERPROFILE 指向临时目录并清洗 CHIMERA_BIBLE_* 环境变量
fn isolate_env(home: &std::path::Path) {
    for (key, _) in std::env::vars() {
        if key.starts_with("CHIMERA_BIBLE_") {
            std::env::remove_var(&key);
        }
    }
    std::env::set_var("HOME", home);
    std::env::set_var("USERPROFILE", home);
}

/// 恢复 HOME/USERPROFILE 至测前值并清洗 CHIMERA_BIBLE_*
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

/// 主路径:文件源 bible 经 apply_bible_overrides 合并进 TuiConfig 且通过 validate
#[test]
fn bible_file_overrides_apply_to_tui_config() {
    let _guard = ENV_LOCK.lock().unwrap();
    let tmp = TempDir::new().expect("tempdir");
    let original_home = std::env::var("HOME").ok();
    let original_userprofile = std::env::var("USERPROFILE").ok();
    isolate_env(tmp.path());

    std::fs::create_dir_all(tmp.path().join(".chimera")).expect("create .chimera");
    std::fs::write(
        tmp.path().join(".chimera").join("tui_bible.yaml"),
        "theme: Light\nlayout:\n  mode: DualPane\n  main_panel_ratio: 0.5\n  log_panel_height: 10\n  sidebar_width: 24\n",
    )
    .expect("write yaml");

    let result = TuiBible::load();
    restore_env(original_home, original_userprofile);

    let bible = result.expect("合法 bible 文件应加载成功");
    let mut cfg = TuiConfig {
        default_view_mode: chimera_tui::ViewMode::Dashboard,
        persist_state: false,
        ..Default::default()
    };
    cfg.apply_bible_overrides(&bible);

    // 四字段合并生效
    assert_eq!(cfg.theme, Theme::Light, "theme 应被 bible 覆盖");
    assert!(
        (cfg.main_panel_ratio - 0.5).abs() < 1e-6,
        "ratio 应被 bible 覆盖"
    );
    assert_eq!(cfg.log_panel_height, 10, "log_panel_height 应被 bible 覆盖");
    // 合并后配置仍通过校验(接线路径在 validate 之前应用)
    cfg.validate().expect("合并后的配置应通过 validate");
}

/// 边界路径:默认 bible 应用到默认配置不改变关键字段(幂等语义)
#[test]
fn default_bible_keeps_default_config_stable() {
    let _guard = ENV_LOCK.lock().unwrap();
    let tmp = TempDir::new().expect("tempdir");
    let original_home = std::env::var("HOME").ok();
    let original_userprofile = std::env::var("USERPROFILE").ok();
    isolate_env(tmp.path());
    let result = TuiBible::load();
    restore_env(original_home, original_userprofile);

    let bible = result.expect("无文件无 env 时应回退默认 bible");
    let before = TuiConfig {
        default_view_mode: chimera_tui::ViewMode::Dashboard,
        persist_state: false,
        ..Default::default()
    };
    let mut after = TuiConfig {
        default_view_mode: chimera_tui::ViewMode::Dashboard,
        persist_state: false,
        ..Default::default()
    };
    after.apply_bible_overrides(&bible);

    // 默认 bible 与默认 TuiConfig 语义一致(见 TuiBible::default WHY),四字段不变
    assert!(matches!(after.theme, Theme::Dark));
    assert!((after.main_panel_ratio - before.main_panel_ratio).abs() < 1e-6);
    assert_eq!(after.log_panel_height, before.log_panel_height);
}

/// 异常路径:bible 文件损坏 → load 报错;复刻 CLI 回退分支,默认配置不受影响
#[test]
fn corrupted_bible_falls_back_to_defaults_like_cli() {
    let _guard = ENV_LOCK.lock().unwrap();
    let tmp = TempDir::new().expect("tempdir");
    let original_home = std::env::var("HOME").ok();
    let original_userprofile = std::env::var("USERPROFILE").ok();
    isolate_env(tmp.path());

    std::fs::create_dir_all(tmp.path().join(".chimera")).expect("create .chimera");
    // 非法 YAML(键值结构残缺)触发解析错误
    std::fs::write(
        tmp.path().join(".chimera").join("tui_bible.yaml"),
        "theme: [unclosed\n  - broken",
    )
    .expect("write corrupted yaml");

    let result = TuiBible::load();
    restore_env(original_home, original_userprofile);

    // 损坏文件必须显式报错(不静默),CLI 据此走 warn + 回退分支
    assert!(result.is_err(), "损坏 bible 应返回 Err 而非静默");

    // 复刻 commands/tui.rs 的回退语义:Err 时保持既有配置,启动不阻断
    let mut cfg = TuiConfig {
        default_view_mode: chimera_tui::ViewMode::Dashboard,
        persist_state: false,
        ..Default::default()
    };
    if let Ok(bible) = result {
        cfg.apply_bible_overrides(&bible);
    }
    cfg.validate().expect("回退后的默认配置应通过 validate");
    assert!(matches!(cfg.theme, Theme::Dark), "回退后应保持默认主题");
}

/// 异常路径:bible 引入越界 ratio → apply 后被 validate 拦截(错误路径单一)
#[test]
fn out_of_range_ratio_is_blocked_by_validate() {
    let bible = {
        let mut b = TuiBible::default();
        b.layout.main_panel_ratio = 1.5; // 越界(合法区间 0.0-1.0)
        b
    };
    let mut cfg = TuiConfig {
        default_view_mode: chimera_tui::ViewMode::Dashboard,
        persist_state: false,
        ..Default::default()
    };
    cfg.apply_bible_overrides(&bible);
    assert!(
        cfg.validate().is_err(),
        "bible 引入的越界 ratio 必须被 validate 拦截"
    );
}
