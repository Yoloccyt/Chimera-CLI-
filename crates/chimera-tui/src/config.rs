//! TUI 配置类型 — 主题与布局
//!
//! 对应架构层:L10 Interface
//! 对应创新点:无(用户交互入口)
//!
//! # 设计决策(WHY)
//! - `theme` 默认 Dark:终端应用常用深色主题,与多数终端配色兼容
//! - `main_panel_ratio` 默认 0.7:主面板占 70%,侧边栏占 30%,保证主内容可读性
//! - `log_panel_height` 默认 8:日志面板 8 行,足够显示最近日志不占用过多空间

use serde::{Deserialize, Serialize};

use crate::error::TuiError;

// ============================================================
// 主题枚举
// ============================================================

/// TUI 主题 — 颜色方案
///
/// WHY enum:主题是离散选择,非连续值,enum 语义清晰。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Theme {
    /// 深色主题(默认)
    Dark,
    /// 浅色主题
    Light,
}

impl Theme {
    /// 返回主题的人类可读名称
    pub fn as_str(&self) -> &'static str {
        match self {
            Theme::Dark => "dark",
            Theme::Light => "light",
        }
    }
}

impl std::fmt::Display for Theme {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ============================================================
// TUI 配置
// ============================================================

/// TUI 配置 — 主题与布局参数
///
/// 所有字段均有合理默认值,可通过 `Default::default()` 快速构造。
/// 构造 `TuiApp` 时会调用 `validate()` 校验配置合法性。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuiConfig {
    /// 主题(颜色方案)
    pub theme: Theme,
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
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            // WHY Dark:终端应用常用深色主题,与多数终端配色兼容
            theme: Theme::Dark,
            // WHY 0.7:主面板 70%,侧边栏 30%,保证主内容可读性
            main_panel_ratio: 0.7,
            // WHY 8:日志面板 8 行,足够显示最近日志不占用过多空间
            log_panel_height: 8,
            enable_mouse: true,
            // WHY 60:60 FPS,流畅渲染且不过度消耗 CPU
            frame_rate: 60,
            // P2.4 默认值见字段文档
            tick_interval_ms: 250,
            snapshot_interval_s: 30,
            max_event_history: 256,
            max_snapshots: 100,
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
        if self.main_panel_ratio.is_nan() || !(0.0..=1.0).contains(&self.main_panel_ratio) {
            return Err(TuiError::ConfigError {
                detail: format!(
                    "main_panel_ratio must be in [0.0, 1.0], got {}",
                    self.main_panel_ratio
                ),
            });
        }
        if self.main_panel_ratio == 0.0 || self.main_panel_ratio == 1.0 {
            return Err(TuiError::ConfigError {
                detail: format!(
                    "main_panel_ratio must be in (0.0, 1.0) exclusive, got {} (0 or 1 leaves no room for sidebar)",
                    self.main_panel_ratio
                ),
            });
        }
        if self.log_panel_height < 3 {
            return Err(TuiError::ConfigError {
                detail: format!(
                    "log_panel_height must be >= 3 (border + 1 line content), got {}",
                    self.log_panel_height
                ),
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
        // P2.4 新增字段默认值
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

    // === P2.4 新增字段校验测试 ===

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
    fn test_theme_as_str() {
        assert_eq!(Theme::Dark.as_str(), "dark");
        assert_eq!(Theme::Light.as_str(), "light");
    }

    #[test]
    fn test_theme_display() {
        assert_eq!(Theme::Dark.to_string(), "dark");
    }

    #[test]
    fn test_theme_serde_roundtrip() {
        let theme = Theme::Light;
        let json = serde_json::to_string(&theme).unwrap();
        let restored: Theme = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, theme);
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let cfg = TuiConfig::default();
        let json = serde_json::to_string(&cfg).unwrap();
        let restored: TuiConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.theme, cfg.theme);
        assert!((restored.main_panel_ratio - cfg.main_panel_ratio).abs() < 1e-6);
    }
}
