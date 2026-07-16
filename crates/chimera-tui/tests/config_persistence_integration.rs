//! TuiConfig 持久化集成测试(Task 16)
//!
//! 验证端到端流程: 保存 → 加载 → 持久化字段一致。
//! 与 config.rs 中的单元测试互补:单元测试覆盖 save/load 单方法行为,
//! 本测试覆盖"保存后重新加载"的完整往返场景。

use chimera_tui::{Theme, TuiConfig};

#[test]
fn test_config_persistence_save_and_load() {
    let mut temp_dir = std::env::temp_dir();
    temp_dir.push("chimera_tui_persistence_test.yaml");

    // 清理旧文件,避免残留干扰
    let _ = std::fs::remove_file(&temp_dir);

    // 创建自定义配置(覆盖全部 4 个持久化字段)
    let original = TuiConfig {
        theme: Theme::HighContrast,
        main_panel_ratio: 0.65,
        tick_interval_ms: 300,
        ..TuiConfig::default()
    };

    // 保存
    original
        .save_to_file(&temp_dir)
        .expect("save should succeed");

    // 加载
    let loaded = TuiConfig::load_from_file(&temp_dir).expect("load should succeed");

    // 验证持久化字段一致
    assert_eq!(loaded.theme, original.theme);
    assert!(
        (loaded.main_panel_ratio - original.main_panel_ratio).abs() < 1e-5,
        "main_panel_ratio should match after roundtrip"
    );
    assert_eq!(loaded.tick_interval_ms, original.tick_interval_ms);

    // 清理
    let _ = std::fs::remove_file(&temp_dir);
}

#[test]
fn test_config_persistence_default_path_format() {
    let path = TuiConfig::default_path();
    assert!(
        path.ends_with("tui.yaml") || path.ends_with("tui.yml"),
        "default path should end with tui.yaml, got: {:?}",
        path
    );
}

#[test]
fn test_config_persistence_nonexistent_returns_default() {
    // 文件不存在时 load_from_file 应返回默认配置,不报错
    let mut temp_dir = std::env::temp_dir();
    temp_dir.push("chimera_tui_nonexistent_integration.yaml");
    let _ = std::fs::remove_file(&temp_dir);

    let result = TuiConfig::load_from_file(&temp_dir);
    assert!(result.is_ok(), "nonexistent file should return Ok(default)");
    let loaded = result.unwrap();
    assert_eq!(loaded.theme, TuiConfig::default().theme);
    assert_eq!(
        loaded.tick_interval_ms,
        TuiConfig::default().tick_interval_ms
    );
}
