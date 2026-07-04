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
