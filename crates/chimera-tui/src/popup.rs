//! TUI 弹窗栈 — 通知、详情与确认弹窗的管理
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - `PopupStack` 用 Vec 实现 LIFO:支持多层弹窗叠加,后打开的弹窗优先显示,
//!   关闭后自动回到下层弹窗。
//! - `PopupKind` 区分通知/详情/确认三种语义,M1 实现通知与详情渲染,
//!   确认为占位,为 M3/M4 的用户确认流程预留扩展点。

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget, Wrap};
use serde::{Deserialize, Serialize};

/// 弹窗严重级别 — 控制通知与状态消息的颜色
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Severity {
    /// 普通信息
    Info,
    /// 警告
    Warning,
    /// 错误
    Error,
}

impl Severity {
    /// 返回对应的前景色
    pub fn color(&self) -> Color {
        match self {
            Severity::Info => Color::Cyan,
            Severity::Warning => Color::Yellow,
            Severity::Error => Color::Red,
        }
    }
}

/// 弹窗类型
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PopupKind {
    /// 通知弹窗 — 简短消息,按严重级别着色
    Notification {
        /// 通知内容
        message: String,
        /// 通知级别
        severity: Severity,
    },
    /// 详情弹窗 — 标题 + 多行内容
    Detail {
        /// 弹窗标题
        title: String,
        /// 详情内容
        content: String,
    },
    /// 确认弹窗 — M1 占位,包含提示与确认后执行的命令字符串
    Confirm {
        /// 提示文本
        prompt: String,
        /// 确认后执行的命令字符串(如 "quit")
        on_confirm: String,
    },
}

/// 弹窗栈 — LIFO 管理当前显示的弹窗
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct PopupStack {
    stack: Vec<PopupKind>,
}

impl PopupStack {
    /// 创建空弹窗栈
    pub fn new() -> Self {
        Self::default()
    }

    /// 压入一个弹窗
    pub fn push(&mut self, popup: PopupKind) {
        self.stack.push(popup);
    }

    /// 弹出栈顶弹窗,返回被弹出的弹窗
    pub fn pop(&mut self) -> Option<PopupKind> {
        self.stack.pop()
    }

    /// 查看栈顶弹窗
    pub fn current(&self) -> Option<&PopupKind> {
        self.stack.last()
    }

    /// 弹窗栈是否为空
    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }

    /// 清空所有弹窗
    pub fn clear(&mut self) {
        self.stack.clear();
    }

    /// 渲染当前弹窗到缓冲区
    ///
    /// WHY 接收 `area`:调用者传入整个终端区域,弹窗自行居中计算。
    /// M1 仅实现通知与详情弹窗;确认弹窗渲染为占位提示。
    pub fn render(&self, area: Rect, buf: &mut Buffer) {
        let Some(popup) = self.current() else {
            return;
        };

        // 居中弹窗:宽度最小 40,最大为终端宽度的 80%;高度最小 5,最大为终端高度的 80%。
        let width = (area.width as f32 * 0.8).max(40.0).min(area.width as f32) as u16;
        let height = (area.height as f32 * 0.8).max(5.0).min(area.height as f32) as u16;
        let popup_area = Rect {
            x: area.x + (area.width.saturating_sub(width)) / 2,
            y: area.y + (area.height.saturating_sub(height)) / 3,
            width,
            height,
        };

        // 先清空弹窗区域背景,避免与下层内容重叠。
        Clear.render(popup_area, buf);

        match popup {
            PopupKind::Notification { message, severity } => {
                let block = Block::default()
                    .borders(Borders::ALL)
                    .title(" Notification ")
                    .border_style(Style::default().fg(severity.color()));
                let text = Text::from(message.as_str());
                let paragraph = Paragraph::new(text)
                    .block(block)
                    .alignment(Alignment::Center)
                    .wrap(Wrap { trim: true });
                paragraph.render(popup_area, buf);
            }
            PopupKind::Detail { title, content } => {
                let block = Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" {title} "));
                let paragraph = Paragraph::new(content.as_str())
                    .block(block)
                    .wrap(Wrap { trim: true });
                paragraph.render(popup_area, buf);
            }
            PopupKind::Confirm { prompt, .. } => {
                // M1 占位:仅渲染提示,不处理确认回调。
                let block = Block::default()
                    .borders(Borders::ALL)
                    .title(" Confirm (M1 stub) ");
                let lines = vec![
                    Line::from(prompt.as_str()),
                    Line::from(""),
                    Line::from("Press Enter to confirm, Esc to cancel."),
                ];
                let paragraph = Paragraph::new(Text::from(lines))
                    .block(block)
                    .wrap(Wrap { trim: true });
                paragraph.render(popup_area, buf);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_popup_stack_push_pop() {
        let mut stack = PopupStack::new();
        assert!(stack.is_empty());

        stack.push(PopupKind::Notification {
            message: "hello".into(),
            severity: Severity::Info,
        });
        assert!(!stack.is_empty());
        assert_eq!(
            stack.current(),
            Some(&PopupKind::Notification {
                message: "hello".into(),
                severity: Severity::Info,
            })
        );

        let popped = stack.pop();
        assert_eq!(
            popped,
            Some(PopupKind::Notification {
                message: "hello".into(),
                severity: Severity::Info,
            })
        );
        assert!(stack.is_empty());
    }

    #[test]
    fn test_popup_stack_current_returns_last() {
        let mut stack = PopupStack::new();
        stack.push(PopupKind::Detail {
            title: "first".into(),
            content: "content".into(),
        });
        stack.push(PopupKind::Notification {
            message: "second".into(),
            severity: Severity::Warning,
        });

        assert_eq!(
            stack.current(),
            Some(&PopupKind::Notification {
                message: "second".into(),
                severity: Severity::Warning,
            })
        );
    }

    #[test]
    fn test_popup_stack_clear() {
        let mut stack = PopupStack::new();
        stack.push(PopupKind::Notification {
            message: "x".into(),
            severity: Severity::Error,
        });
        stack.clear();
        assert!(stack.is_empty());
        assert_eq!(stack.pop(), None);
    }

    #[test]
    fn test_severity_color() {
        assert_eq!(Severity::Info.color(), Color::Cyan);
        assert_eq!(Severity::Warning.color(), Color::Yellow);
        assert_eq!(Severity::Error.color(), Color::Red);
    }
}
