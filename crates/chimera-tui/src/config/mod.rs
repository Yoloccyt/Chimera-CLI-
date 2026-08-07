//! TUI 配置类型 — 主题与布局
//!
//! 对应架构层:L10 Interface
//! 对应创新点:无(用户交互入口)
//!
//! # 设计决策(WHY)
//! - `theme` 默认 Dark:终端应用常用深色主题,与多数终端配色兼容
//! - `main_panel_ratio` 默认 0.7:主面板占 70%,侧边栏占 30%,保证主内容可读性
//! - `log_panel_height` 默认 8:日志面板 8 行,足够显示最近日志不占用过多空间
//!
//! # 模块组织
//! - `theme`:主题枚举、颜色种类、颜色方案覆盖
//! - `layout`:布局参数常量与校验辅助函数
//! - `persistence`:配置持久化(保存/加载 YAML)
//! - `tui_bible`:基于 Figment 多源合并的"设计手册"配置加载器
//!   (Task 3.2,v1.8-omega),通过 `TuiBible::load()` 提供 4 源合并:
//!   默认 < `~/.chimera/tui_bible.yaml` < env `CHIMERA_BIBLE_*` < CLI 参数

pub mod layout;
pub mod persistence;
pub mod theme;
pub mod tui_bible;

// Re-export 子模块公开类型,保持公开 API 不变
pub use layout::{DEFAULT_LOG_PANEL_HEIGHT, DEFAULT_MAIN_PANEL_RATIO, MIN_LOG_PANEL_HEIGHT};
pub use theme::{ColorKind, ColorScheme, Theme, ThemeColors};

use serde::{Deserialize, Serialize};

use crate::error::TuiError;
use crate::types::SortMode;

// ============================================================
// TUI 配置
// ============================================================

/// TUI 配置 — 主题与布局参数
///
/// 所有字段均有合理默认值,可通过 `Default::default()` 快速构造。
/// 构造 `TuiApp` 时会调用 `validate()` 校验配置合法性。
///
/// WHY `#[serde(default)]`:配置文件(`~/.aether/omega.yaml`)只需提供
/// 用户想覆盖的字段,其余字段回退到 `TuiConfig::default()` 的预设值。
/// 这与 Figment 四源合并(CLAUDE.md §4)一致 — 内置默认 → 配置文件 → 环境变量 → CLI。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct TuiConfig {
    /// 主题(颜色方案)
    pub theme: Theme,
    /// 颜色方案覆盖(P6.3)— 用户对主题颜色的细粒度定制
    ///
    /// WHY 默认全 None:`ColorScheme::default()` 不覆盖任何颜色,
    /// 完全沿用 `theme` 预设。用户在配置文件 `tui.colors` 节设置
    /// 某字段才会生效。渲染层通过 `colors.resolve(theme)` 获取最终颜色。
    pub colors: ColorScheme,
    /// 主面板占比(范围 0.0-1.0,表示主面板占水平方向的比例)
    pub main_panel_ratio: f32,
    /// 日志面板高度(行数)
    pub log_panel_height: u16,
    /// 是否启用鼠标支持
    pub enable_mouse: bool,
    /// 刷新率(帧/秒)
    pub frame_rate: u16,
    /// tick 间隔(毫秒),控制 DataPipeline 快照频率(P4.3 性能优化)
    ///
    /// WHY 250ms 默认:平衡响应性与 CPU 开销,4 Hz 更新足够面板展示
    /// 实时指标;过低(如 50ms)会导致 event-bus 频繁加锁,
    /// 过高(如 1000ms)会让操作员感觉面板"卡顿"。
    pub tick_interval_ms: u16,
    /// 快照间隔(秒),P7 历史回放用(P7 接口占位,v1.8+ 实现)
    ///
    /// WHY 30s 默认:历史回放粒度,过细会占用大量内存,过粗无法回看细节。
    pub snapshot_interval_s: u16,
    /// 事件流最大保留条数(P2.2 EventStream 面板需要万级)
    ///
    /// WHY 256 默认:与现有 `DataSourceConfig::max_event_history` 默认值
    /// 保持一致;P2.2 EventStream 实现万级虚拟滚动时,可上调至 10000+。
    pub max_event_history: usize,
    /// 快照最大保留数(P7 接口占位,v1.8+ 实现)
    ///
    /// WHY 100 默认:30s × 100 = 50 分钟历史回放窗口,覆盖典型调试场景。
    pub max_snapshots: usize,
    // === v1.8-omega: 监控/任务/系统信息扩展字段(Task 1.4) ===
    /// 是否启用实时趋势图(默认 false — 不破坏既有 resource_monitor 面板断言)
    ///
    /// WHY 默认 false:spec §MODIFIED Requirements 迁移路径明确指出"默认关闭,
    /// 需用户显式开启以避免破坏既有 `resource_monitor_panel_test.rs` 断言"。
    /// 用户在配置文件中设置 `enable_trend_charts: true` 后,ResourceMonitorPanel
    /// 才渲染 sparkline 趋势图 + 阈值告警颜色。
    pub enable_trend_charts: bool,
    /// 指标采样间隔(毫秒,默认 1000ms = 1Hz)
    ///
    /// 控制 ResourceMonitorPanel 趋势图的采样频率,1Hz 与 5 分钟窗口 300 点对齐。
    /// WHY 1000ms:平衡实时性与存储开销;过低(<500ms)导致 CPU/IO 压力,
    /// 过高(>5000ms)丢失细节。validate() 限制 [100, 60000]。
    pub metrics_sample_interval_ms: u64,
    /// 指标历史保留天数(默认 7 天)
    ///
    /// 控制 metrics_history.sqlite 的数据保留期,过期数据由后台清理任务删除。
    /// WHY 7 天:一周历史覆盖典型运维诊断周期(周末复盘 + 工作日回溯)。
    pub metrics_history_retention_days: u32,
    /// 任务管理面板默认排序模式(默认 Priority)
    ///
    /// 决定 TaskManagerPanel 启动时的 Quest 列表排序方式,
    /// 用户可在面板内通过快捷键循环切换(SortMode::next())。
    pub task_manager_default_sort: SortMode,
    /// 系统信息刷新间隔(毫秒,默认 5000ms = 5s)
    ///
    /// 控制 SysinfoPanel 进程信息(PID/RSS/线程数/文件句柄数)的刷新频率,
    /// 主机信息(OS/CPU/内存)仅在面板首次打开时采集一次。
    /// WHY 5000ms:5s 刷新足够展示进程变化趋势,避免 sysinfo 调用过于频繁
    /// 导致 CPU 占用(spec §Scenario "系统信息面板启动加载")。
    pub sysinfo_refresh_interval_ms: u64,
    /// 是否启用视图状态持久化（默认 true）
    ///
    /// WHY 默认 true:退出时保存布局模式/过滤器等用户偏好,
    /// 下次启动自动恢复,减少用户重复操作。用户可通过配置文件关闭。
    pub persist_state: bool,
    /// 状态文件路径（默认 ~/.chimera/tui_state.yaml）
    pub state_file_path: std::path::PathBuf,
    /// v2.9.0-omega Task 2.6:响应式折叠阈值(终端宽度低于此值时自动隐藏伴随面板)
    ///
    /// WHY 默认 100:终端宽度 < 100 列时,主面板 + 伴随面板(30 字符)会挤压主内容
    /// 至 70 列以下,可读性下降。折叠伴随面板让主面板独占宽度,符合响应式设计原则。
    /// 用户可在配置文件 `tui.responsive_collapse_threshold` 调整(设为 0 禁用折叠)。
    pub responsive_collapse_threshold: u16,
    /// 退出确认(默认 false):开启后 Normal 模式按 q/Esc 先弹确认框,
    /// 左/右键切到 Yes 后 Enter 才真正退出,防误触(§4.3 退出安全)。
    ///
    /// WHY 默认 false:保持既有 `q`/Esc 立即退出行为零回归(含 m3a 契约),
    /// 需要误触保护的用户在配置文件中显式开启。
    pub quit_requires_confirm: bool,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            // WHY Dark:终端应用常用深色主题,与多数终端配色兼容
            theme: Theme::Dark,
            // P6.3:默认不覆盖任何颜色,完全使用 Dark 主题预设
            colors: ColorScheme::default(),
            // WHY 0.7:主面板 70%,侧边栏 30%,保证主内容可读性
            main_panel_ratio: DEFAULT_MAIN_PANEL_RATIO,
            // WHY 8:日志面板 8 行,足够显示最近日志不占用过多空间
            log_panel_height: DEFAULT_LOG_PANEL_HEIGHT,
            enable_mouse: true,
            // WHY 60:60 FPS,流畅渲染且不过度消耗 CPU
            frame_rate: 60,
            // P2.4 默认值见字段文档
            tick_interval_ms: 250,
            snapshot_interval_s: 30,
            max_event_history: 256,
            max_snapshots: 100,
            // === v1.8-omega 扩展字段默认值(Task 1.4) ===
            // 与 spec §Requirement / §MODIFIED Requirements 对齐
            enable_trend_charts: false,
            metrics_sample_interval_ms: 1000,
            metrics_history_retention_days: 7,
            task_manager_default_sort: SortMode::default(), // = Priority
            sysinfo_refresh_interval_ms: 5000,
            persist_state: true,
            state_file_path: Self::default_state_path(),
            // v2.9.0-omega Task 2.6:响应式折叠阈值默认 100 列
            responsive_collapse_threshold: 100,
            quit_requires_confirm: false,
        }
    }
}

impl TuiConfig {
    /// 校验配置合法性
    ///
    /// WHY:在构造 TuiApp 时调用,提前暴露配置错误。
    ///
    /// # 校验规则
    /// - `main_panel_ratio` ∈ (0.0, 1.0)(不能为 0 或 1,需留侧边栏空间)
    /// - `log_panel_height` >= 3(至少 3 行:边框 + 1 行内容)
    /// - `frame_rate` >= 1
    /// - `tick_interval_ms` ∈ [100, 1000](过短导致 CPU 占用高,过长面板卡顿)
    /// - `snapshot_interval_s` >= 1(P7 历史回放最小粒度)
    /// - `max_event_history` >= 64(EventStream 面板最小可用容量)
    /// - `max_snapshots` >= 10(P7 历史回放最小回看窗口)
    pub fn validate(&self) -> Result<(), TuiError> {
        if let Err(msg) = layout::validate_main_panel_ratio(self.main_panel_ratio) {
            return Err(TuiError::ConfigError {
                detail: format!("{} , got {}", msg, self.main_panel_ratio),
            });
        }
        if let Err(msg) = layout::validate_log_panel_height(self.log_panel_height) {
            return Err(TuiError::ConfigError {
                detail: format!("{} , got {}", msg, self.log_panel_height),
            });
        }
        if self.frame_rate == 0 {
            return Err(TuiError::ConfigError {
                detail: "frame_rate must be >= 1".into(),
            });
        }
        // P2.4 新增校验
        if !(100..=1000).contains(&self.tick_interval_ms) {
            return Err(TuiError::ConfigError {
                detail: format!(
                    "tick_interval_ms must be in [100, 1000], got {} (too low: CPU overhead; too high: panel feels frozen)",
                    self.tick_interval_ms
                ),
            });
        }
        if self.snapshot_interval_s < 1 {
            return Err(TuiError::ConfigError {
                detail: format!(
                    "snapshot_interval_s must be >= 1, got {}",
                    self.snapshot_interval_s
                ),
            });
        }
        if self.max_event_history < 64 {
            return Err(TuiError::ConfigError {
                detail: format!(
                    "max_event_history must be >= 64 (EventStream panel minimum), got {}",
                    self.max_event_history
                ),
            });
        }
        if self.max_snapshots < 10 {
            return Err(TuiError::ConfigError {
                detail: format!(
                    "max_snapshots must be >= 10 (P7 history replay minimum), got {}",
                    self.max_snapshots
                ),
            });
        }
        // === v1.8-omega 扩展字段校验(Task 1.4 REFACTOR) ===
        if !(100..=60_000).contains(&self.metrics_sample_interval_ms) {
            return Err(TuiError::ConfigError {
                detail: format!(
                    "metrics_sample_interval_ms must be in [100, 60000], got {} (too low: CPU/IO pressure; too high: loses detail)",
                    self.metrics_sample_interval_ms
                ),
            });
        }
        if self.metrics_history_retention_days < 1 {
            return Err(TuiError::ConfigError {
                detail: format!(
                    "metrics_history_retention_days must be >= 1, got {} (0 days would immediately purge all history)",
                    self.metrics_history_retention_days
                ),
            });
        }
        if self.sysinfo_refresh_interval_ms < 100 {
            return Err(TuiError::ConfigError {
                detail: format!(
                    "sysinfo_refresh_interval_ms must be >= 100, got {} (too low: sysinfo refresh is heavy)",
                    self.sysinfo_refresh_interval_ms
                ),
            });
        }
        // SortMode 是 enum,无范围可言,无需校验(serde 反序列化已保证有效性)
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_values() {
        let cfg = TuiConfig::default();
        assert_eq!(cfg.theme, Theme::Dark);
        assert!((cfg.main_panel_ratio - 0.7).abs() < 1e-6);
        assert_eq!(cfg.log_panel_height, 8);
        assert!(cfg.enable_mouse);
        assert_eq!(cfg.frame_rate, 60);
        assert_eq!(cfg.tick_interval_ms, 250);
        assert_eq!(cfg.snapshot_interval_s, 30);
        assert_eq!(cfg.max_event_history, 256);
        assert_eq!(cfg.max_snapshots, 100);
    }

    #[test]
    fn test_validate_ok() {
        let cfg = TuiConfig::default();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn test_validate_ratio_out_of_range() {
        let cfg = TuiConfig {
            main_panel_ratio: 1.5,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_ratio_zero() {
        let cfg = TuiConfig {
            main_panel_ratio: 0.0,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_ratio_one() {
        let cfg = TuiConfig {
            main_panel_ratio: 1.0,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_log_height_too_small() {
        let cfg = TuiConfig {
            log_panel_height: 2,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_zero_frame_rate() {
        let cfg = TuiConfig {
            frame_rate: 0,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_tick_interval_too_low() {
        let cfg = TuiConfig {
            tick_interval_ms: 50,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_tick_interval_too_high() {
        let cfg = TuiConfig {
            tick_interval_ms: 2000,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_snapshot_interval_zero() {
        let cfg = TuiConfig {
            snapshot_interval_s: 0,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_max_event_history_too_small() {
        let cfg = TuiConfig {
            max_event_history: 32,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_max_snapshots_too_small() {
        let cfg = TuiConfig {
            max_snapshots: 5,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn quit_requires_confirm_defaults_false_and_serde_compat(
    ) -> Result<(), Box<dyn std::error::Error>> {
        // 默认关闭(保持 q/Esc 立即退出零回归);旧配置文件缺省该字段时回退 false
        let cfg = TuiConfig::default();
        assert!(!cfg.quit_requires_confirm);
        let json = r#"{"theme": "Dark"}"#;
        let restored: TuiConfig = serde_json::from_str(json)?;
        assert!(!restored.quit_requires_confirm);
        let enabled: TuiConfig = serde_json::from_str(r#"{"quit_requires_confirm": true}"#)?;
        assert!(enabled.quit_requires_confirm);
        Ok(())
    }

    #[test]
    fn test_config_serde_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
        let cfg = TuiConfig::default();
        let json = serde_json::to_string(&cfg)?;
        let restored: TuiConfig = serde_json::from_str(&json)?;
        assert_eq!(restored.theme, cfg.theme);
        assert!((restored.main_panel_ratio - cfg.main_panel_ratio).abs() < 1e-6);
        Ok(())
    }

    #[test]
    fn test_config_with_colors_serde_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
        let cfg = TuiConfig {
            colors: ColorScheme {
                accent: Some(ColorKind::BrightBlue),
                ..Default::default()
            },
            ..Default::default()
        };
        let json = serde_json::to_string(&cfg)?;
        let restored: TuiConfig = serde_json::from_str(&json)?;
        assert_eq!(restored.theme, cfg.theme);
        assert_eq!(restored.colors, cfg.colors);
        assert_eq!(restored.colors.accent, Some(ColorKind::BrightBlue));
        Ok(())
    }

    #[test]
    fn test_config_json_colors_override_from_string() -> Result<(), Box<dyn std::error::Error>> {
        let json = r#"{
            "theme": "Dark",
            "colors": {
                "accent": "BrightBlue",
                "warning": "BrightYellow"
            }
        }"#;
        let cfg: TuiConfig = serde_json::from_str(json)?;
        assert_eq!(cfg.theme, Theme::Dark);
        assert_eq!(cfg.colors.accent, Some(ColorKind::BrightBlue));
        assert_eq!(cfg.colors.warning, Some(ColorKind::BrightYellow));
        assert!(cfg.colors.foreground.is_none());
        assert!(cfg.colors.background.is_none());
        let resolved = cfg.colors.resolve(cfg.theme);
        assert_eq!(resolved.accent, ColorKind::BrightBlue);
        assert_eq!(resolved.foreground, ColorKind::White);
        Ok(())
    }

    #[test]
    fn test_config_colors_field_default_when_absent() -> Result<(), Box<dyn std::error::Error>> {
        let json = r#"{"theme": "Light"}"#;
        let cfg: TuiConfig = serde_json::from_str(json)?;
        assert_eq!(cfg.theme, Theme::Light);
        assert_eq!(cfg.colors, ColorScheme::default());
        Ok(())
    }
}
