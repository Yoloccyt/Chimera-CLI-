//! P4.3 可调 tick 暴露集成测试
//!
//! 验证 TuiConfig.tick_interval_ms 正确桥接到 DataSourceConfig

use chimera_tui::config::TuiConfig;
use chimera_tui::data::DataSourceConfig;

/// 验证 TuiConfig.tick_interval_ms 默认值
#[test]
fn tui_config_default_tick_interval() {
    let config = TuiConfig::default();
    assert_eq!(config.tick_interval_ms, 250);
}

/// 验证 TuiConfig 校验 tick_interval_ms 范围
#[test]
fn tui_config_validate_tick_interval_range() {
    // 正常值通过
    for ms in [100u16, 200, 500, 1000] {
        let config = TuiConfig {
            tick_interval_ms: ms,
            ..Default::default()
        };
        assert!(config.validate().is_ok());
    }

    // 过低
    let config = TuiConfig {
        tick_interval_ms: 50,
        ..Default::default()
    };
    assert!(config.validate().is_err());

    // 过高
    let config = TuiConfig {
        tick_interval_ms: 2000,
        ..Default::default()
    };
    assert!(config.validate().is_err());
}

/// 验证 DataSourceConfig::from_tui_config 桥接
#[test]
fn data_source_config_from_tui_config() {
    let tui_config = TuiConfig {
        tick_interval_ms: 500,
        ..Default::default()
    };
    let ds_config = DataSourceConfig::from_tui_config(&tui_config);
    assert_eq!(ds_config.tick_interval_ms, 500);
}

/// 验证不同 tick_interval_ms 值的桥接
#[test]
fn data_source_config_from_tui_config_various() {
    for ms in [100u16, 200, 500, 1000] {
        let tui_config = TuiConfig {
            tick_interval_ms: ms,
            ..Default::default()
        };
        assert!(tui_config.validate().is_ok());
        let ds_config = DataSourceConfig::from_tui_config(&tui_config);
        assert_eq!(ds_config.tick_interval_ms, ms as u64);
    }
}

/// 验证 TuiApp 构造时 tick_interval_ms 从 TuiConfig 传入
#[test]
fn tui_app_uses_configured_tick_interval() {
    let config = TuiConfig {
        tick_interval_ms: 500,
        ..Default::default()
    };
    let app = chimera_tui::TuiApp::new(config).unwrap();
    assert_eq!(app.config().tick_interval_ms, 500);
}
