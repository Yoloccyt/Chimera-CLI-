//! TuiConfig v1.8-omega 扩展字段测试(Task 1.4 — TDD-RED → GREEN)
//!
//! ## 覆盖范围
//! - 5 个新增字段的默认值契约
//! - 旧配置文件(YAML 仅含 4 个旧字段)的向后兼容加载
//! - 序列化往返一致性(Priority 排序模式)
//!
//! ## 设计依据(spec.md §五「配置与持久化」+ §MODIFIED Requirements)
//! - `enable_trend_charts: bool`(默认 false — 不破坏既有 resource_monitor 面板断言)
//! - `metrics_sample_interval_ms: u64`(默认 1000 — 1Hz 采样)
//! - `metrics_history_retention_days: u32`(默认 7 — 一周历史)
//! - `task_manager_default_sort: SortMode`(默认 Priority — 运维优先关注高优先级任务)
//! - `sysinfo_refresh_interval_ms: u64`(默认 5000 — 5s 刷新,平衡实时性与 CPU)
//!
//! ## 兼容性契约
//! 所有 5 字段均 `#[serde(default)]`,旧 YAML(只含 theme/colors/main_panel_ratio/tick_interval_ms)
//! 加载时 5 字段自动回退到默认值,无需任何迁移逻辑。
//!
//! ## TDD 阶段
//! 本文件首次提交时为 RED:由于 `TuiConfig` 尚未包含新字段,所有引用
//! `enable_trend_charts` / `metrics_sample_interval_ms` 等的测试将编译失败
//! (E0609 找不到字段);也包含断言默认值的测试,实现完成后转为 GREEN。

use chimera_tui::{SortMode, TuiConfig};

// ============================================================
// 1) 默认值契约 — 5 字段默认值
// ============================================================

/// `enable_trend_charts` 默认 false,避免破坏既有 resource_monitor 面板断言
/// (spec §MODIFIED Requirements 迁移路径)
#[test]
fn test_enable_trend_charts_defaults_to_false() {
    let cfg = TuiConfig::default();
    assert!(
        !cfg.enable_trend_charts,
        "enable_trend_charts should default to false (opt-in for trend chart rendering)"
    );
}

/// `metrics_sample_interval_ms` 默认 1000ms(1Hz 采样,符合 sparkline 5 分钟窗口 300 点)
#[test]
fn test_metrics_sample_interval_ms_defaults_to_1000() {
    let cfg = TuiConfig::default();
    assert_eq!(
        cfg.metrics_sample_interval_ms, 1000,
        "metrics_sample_interval_ms should default to 1000 (1Hz sampling for 5min window)"
    );
}

/// `metrics_history_retention_days` 默认 7 天(spec §Requirement "监控历史持久化")
#[test]
fn test_metrics_history_retention_days_defaults_to_7() {
    let cfg = TuiConfig::default();
    assert_eq!(
        cfg.metrics_history_retention_days, 7,
        "metrics_history_retention_days should default to 7 (one week of history)"
    );
}

/// `task_manager_default_sort` 默认 Priority(spec §Requirement "任务管理面板")
#[test]
fn test_task_manager_default_sort_defaults_to_priority() {
    let cfg = TuiConfig::default();
    assert_eq!(
        cfg.task_manager_default_sort,
        SortMode::Priority,
        "task_manager_default_sort should default to Priority (ops focus on high-priority quests)"
    );
}

/// `sysinfo_refresh_interval_ms` 默认 5000ms(spec §Scenario "系统信息面板启动加载":5s 刷新)
#[test]
fn test_sysinfo_refresh_interval_ms_defaults_to_5000() {
    let cfg = TuiConfig::default();
    assert_eq!(
        cfg.sysinfo_refresh_interval_ms, 5000,
        "sysinfo_refresh_interval_ms should default to 5000 (5s refresh, balance liveness and CPU)"
    );
}

// ============================================================
// 2) 向后兼容契约 — 旧 YAML 无需迁移即可加载
// ============================================================

/// 关键:旧配置文件(YAML 只含 4 个旧字段)加载后,5 个新字段自动回退到默认值
///
/// WHY 这是"长期主义"硬约束:既有用户的 `~/.chimera/tui.yaml` 不应因 TUI 升级
/// 而需要任何手动迁移;`#[serde(default)]` 标注必须保证字段缺失时静默回退。
#[test]
fn test_old_config_file_loads_with_defaults() {
    use std::io::Write;

    let mut temp_path = std::env::temp_dir();
    temp_path.push("chimera_tui_extension_old_config.yaml");

    // 清理可能残留的旧文件
    let _ = std::fs::remove_file(&temp_path);

    // 构造"v1.7 时代"的配置文件 — 仅含 4 个旧字段,不含任何 v1.8 新字段
    let old_yaml = r#"theme: HighContrast
colors: {}
main_panel_ratio: 0.65
tick_interval_ms: 300
"#;
    {
        let mut f = std::fs::File::create(&temp_path).expect("create old config file");
        f.write_all(old_yaml.as_bytes())
            .expect("write old config file");
    }

    // 加载旧配置(应成功,无任何迁移)
    let loaded = TuiConfig::load_from_file(&temp_path)
        .expect("load_from_file should succeed for old config");

    // 旧字段被加载(用户设置的值)
    assert_eq!(loaded.theme, chimera_tui::Theme::HighContrast);
    assert!(
        (loaded.main_panel_ratio - 0.65).abs() < 1e-5,
        "main_panel_ratio should be loaded from old config (0.65)"
    );
    assert_eq!(
        loaded.tick_interval_ms, 300,
        "tick_interval_ms should be loaded from old config (300)"
    );

    // 5 个新字段全部回退到默认值 — 这就是"零迁移"的契约
    assert!(!loaded.enable_trend_charts);
    assert_eq!(loaded.metrics_sample_interval_ms, 1000);
    assert_eq!(loaded.metrics_history_retention_days, 7);
    assert_eq!(loaded.task_manager_default_sort, SortMode::Priority);
    assert_eq!(loaded.sysinfo_refresh_interval_ms, 5000);

    // 清理
    let _ = std::fs::remove_file(&temp_path);
}
