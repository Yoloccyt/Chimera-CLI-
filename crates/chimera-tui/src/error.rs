//! TUI 错误类型 — 库层错误用 thiserror enum(§4.1)
//!
//! 对应架构层:L10 Interface
//! 对应创新点:无(用户交互入口)
//!
//! WHY thiserror:库层错误用自定义 enum(§4.1),应用层才用 anyhow。
//! 终端 IO 错误来自 crossterm,渲染错误来自 ratatui,统一包装为本枚举。

use thiserror::Error;

/// TUI 错误类型
///
/// WHY:TUI 涉及终端初始化、事件读取、屏幕渲染等多个可能失败的环节,
/// 需要统一的错误类型便于调用方处理。
#[derive(Debug, Error)]
pub enum TuiError {
    /// 终端初始化失败 — crossterm 后端错误
    ///
    /// WHY:终端初始化(进入 raw mode、alternate screen)可能失败,
    /// 如非 TTY 环境,携带原始错误便于定位
    #[error("terminal initialization failed: {0}")]
    TerminalInit(String),

    /// 终端事件读取失败 — crossterm 事件循环错误
    #[error("terminal event read failed: {0}")]
    EventRead(String),

    /// 屏幕渲染失败 — ratatui 绘制错误
    #[error("screen render failed: {0}")]
    Render(String),

    /// 终端恢复失败 — 退出时恢复原始终端状态失败
    ///
    /// WHY:退出 TUI 时需恢复 raw mode 与主屏幕,失败可能导致终端残留异常状态,
    /// 必须向调用方报告以便手动恢复
    #[error("terminal restore failed: {0}")]
    TerminalRestore(String),

    /// 配置错误 — 配置项非法(如布局比例为 0 等)
    #[error("config error: {detail}")]
    ConfigError {
        /// 配置错误详情
        detail: String,
    },
}

impl From<std::io::Error> for TuiError {
    /// 将 std::io::Error 转换为 TuiError
    ///
    /// WHY:crossterm 与 ratatui 的底层 IO 操作返回 std::io::Error,
    /// 统一转换为 TerminalInit 便于调用方处理
    fn from(e: std::io::Error) -> Self {
        TuiError::TerminalInit(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_terminal_init_display() {
        let err = TuiError::TerminalInit("not a tty".into());
        assert!(err.to_string().contains("not a tty"));
        assert!(err.to_string().contains("initialization"));
    }

    #[test]
    fn test_event_read_display() {
        let err = TuiError::EventRead("poll failed".into());
        assert!(err.to_string().contains("poll failed"));
        assert!(err.to_string().contains("event read"));
    }

    #[test]
    fn test_render_display() {
        let err = TuiError::Render("buffer overflow".into());
        assert!(err.to_string().contains("buffer overflow"));
        assert!(err.to_string().contains("render"));
    }

    #[test]
    fn test_terminal_restore_display() {
        let err = TuiError::TerminalRestore("disable raw mode failed".into());
        assert!(err.to_string().contains("disable raw mode"));
        assert!(err.to_string().contains("restore"));
    }

    #[test]
    fn test_config_error_display() {
        let err = TuiError::ConfigError {
            detail: "invalid layout".into(),
        };
        assert!(err.to_string().contains("invalid layout"));
    }

    #[test]
    fn test_from_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        let tui_err: TuiError = io_err.into();
        assert!(matches!(tui_err, TuiError::TerminalInit(_)));
        assert!(tui_err.to_string().contains("missing"));
    }
}
