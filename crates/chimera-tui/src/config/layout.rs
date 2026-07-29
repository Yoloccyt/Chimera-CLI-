//! 布局配置 — TUI 面板布局参数与约束
//!
//! 布局相关字段(`main_panel_ratio`、`log_panel_height`)内嵌于
//! [`TuiConfig`](super::TuiConfig),本模块提供布局维度的默认值常量
//! 与校验辅助函数。
//!
//! 对应架构层:L10 Interface

/// 主面板默认占比(70% 主面板,30% 侧边栏)
///
/// WHY 0.7:主面板占 70%,侧边栏占 30%,保证主内容可读性
pub const DEFAULT_MAIN_PANEL_RATIO: f32 = 0.7;

/// 日志面板默认高度(行数)
///
/// WHY 8:日志面板 8 行,足够显示最近日志不占用过多空间
pub const DEFAULT_LOG_PANEL_HEIGHT: u16 = 8;

/// 日志面板最小允许高度(边框 + 至少 1 行内容)
pub const MIN_LOG_PANEL_HEIGHT: u16 = 3;

/// 校验主面板占比是否合法
///
/// 规则: `ratio` ∈ (0.0, 1.0) 开区间(不能为 0 或 1,需留侧边栏空间)
pub fn validate_main_panel_ratio(ratio: f32) -> Result<(), &'static str> {
    if ratio.is_nan() || !(0.0..=1.0).contains(&ratio) {
        return Err("main_panel_ratio must be in [0.0, 1.0]");
    }
    if ratio == 0.0 || ratio == 1.0 {
        return Err(
            "main_panel_ratio must be in (0.0, 1.0) exclusive (0 or 1 leaves no room for sidebar)",
        );
    }
    Ok(())
}

/// 校验日志面板高度是否合法
///
/// 规则: `height` >= [`MIN_LOG_PANEL_HEIGHT`]
pub fn validate_log_panel_height(height: u16) -> Result<(), &'static str> {
    if height < MIN_LOG_PANEL_HEIGHT {
        return Err("log_panel_height must be >= 3 (border + 1 line content)");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_ratio_ok() {
        assert!(validate_main_panel_ratio(0.5).is_ok());
        assert!(validate_main_panel_ratio(DEFAULT_MAIN_PANEL_RATIO).is_ok());
    }

    #[test]
    fn test_validate_ratio_nan() {
        assert!(validate_main_panel_ratio(f32::NAN).is_err());
    }

    #[test]
    fn test_validate_ratio_out_of_range() {
        assert!(validate_main_panel_ratio(1.5).is_err());
        assert!(validate_main_panel_ratio(-0.1).is_err());
    }

    #[test]
    fn test_validate_ratio_zero_and_one() {
        assert!(validate_main_panel_ratio(0.0).is_err());
        assert!(validate_main_panel_ratio(1.0).is_err());
    }

    #[test]
    fn test_validate_log_height_ok() {
        assert!(validate_log_panel_height(DEFAULT_LOG_PANEL_HEIGHT).is_ok());
        assert!(validate_log_panel_height(MIN_LOG_PANEL_HEIGHT).is_ok());
    }

    #[test]
    fn test_validate_log_height_too_small() {
        assert!(validate_log_panel_height(2).is_err());
        assert!(validate_log_panel_height(0).is_err());
    }
}
