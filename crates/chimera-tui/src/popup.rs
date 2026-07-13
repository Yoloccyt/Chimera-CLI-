//! TUI 弹窗栈 — 通知、详情与确认弹窗的管理
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - `PopupStack` 用 Vec 实现 LIFO:支持多层弹窗叠加,后打开的弹窗优先显示,
//!   关闭后自动回到下层弹窗。
//! - `PopupKind` 区分通知/详情/确认三种语义,M3 完成确认弹窗渲染并支持
//!   详情弹窗滚动,为 M4 的用户确认流程预留扩展点。

use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
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
        /// 垂直滚动偏移(行数)
        scroll: u16,
    },
    /// 确认弹窗 — 包含提示、Yes/No 选项与确认后执行的命令字符串
    Confirm {
        /// 提示文本
        prompt: String,
        /// 确认后执行的命令字符串(如 "quit")
        on_confirm: String,
        /// 当前是否选中 Yes(true) 或 No(false)
        confirmed: bool,
    },
}

impl PopupKind {
    /// 创建控制命令确认弹窗
    ///
    /// WHY M4:将动作描述与目标组合为提示文本,同时保留机器可解析的
    /// `on_confirm` 字符串,供 `TuiApp::apply_confirm_command` 解码执行。
    /// 默认选中 Yes,减少操作员每次控制命令都需多按一次键的摩擦。
    pub fn control_confirm(action: &str, target: &str, on_confirm: impl Into<String>) -> Self {
        Self::Confirm {
            prompt: format!("{action} {target}?"),
            on_confirm: on_confirm.into(),
            confirmed: true,
        }
    }
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

    /// 查看栈顶弹窗可变引用,用于更新滚动状态等
    pub fn current_mut(&mut self) -> Option<&mut PopupKind> {
        self.stack.last_mut()
    }

    /// 弹窗栈是否为空
    pub fn is_empty(&self) -> bool {
        self.stack.is_empty()
    }

    /// 清空所有弹窗
    pub fn clear(&mut self) {
        self.stack.clear();
    }

    /// 滚动当前详情弹窗
    ///
    /// `delta` 为正向下滚动,为负向上滚动。非详情弹窗被忽略。
    pub fn scroll_current(&mut self, delta: i16) {
        if let Some(PopupKind::Detail { scroll, .. }) = self.current_mut() {
            let new = *scroll as i32 + delta as i32;
            *scroll = new.clamp(0, u16::MAX as i32) as u16;
        }
    }

    /// 切换当前确认弹窗的选中项(Yes/No)
    pub fn toggle_confirm(&mut self) {
        if let Some(PopupKind::Confirm { confirmed, .. }) = self.current_mut() {
            *confirmed = !*confirmed;
        }
    }

    /// 渲染当前弹窗到缓冲区
    ///
    /// WHY 接收 `area`:调用者传入整个终端区域,弹窗自行居中计算。
    /// M3 实现通知、详情与确认弹窗渲染;详情弹窗支持滚动。
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
            PopupKind::Detail {
                title,
                content,
                scroll,
            } => {
                let block = Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" {title} "));
                let paragraph = Paragraph::new(content.as_str())
                    .block(block)
                    .wrap(Wrap { trim: true })
                    .scroll((*scroll, 0));
                paragraph.render(popup_area, buf);
            }
            PopupKind::Confirm {
                prompt, confirmed, ..
            } => {
                let block = Block::default().borders(Borders::ALL).title(" Confirm ");
                let yes_style = if *confirmed {
                    Style::default()
                        .fg(Color::Green)
                        .add_modifier(Modifier::BOLD | Modifier::REVERSED)
                } else {
                    Style::default().fg(Color::Green)
                };
                let no_style = if *confirmed {
                    Style::default().fg(Color::Red)
                } else {
                    Style::default()
                        .fg(Color::Red)
                        .add_modifier(Modifier::BOLD | Modifier::REVERSED)
                };
                let lines = vec![
                    Line::from(prompt.as_str()),
                    Line::from(""),
                    Line::from(vec![
                        Span::styled("[Yes]", yes_style),
                        Span::raw("   "),
                        Span::styled("[No]", no_style),
                    ]),
                    Line::from(""),
                    Line::from("Left/Right to select, Enter to confirm, Esc to cancel."),
                ];
                let paragraph = Paragraph::new(Text::from(lines))
                    .block(block)
                    .wrap(Wrap { trim: true });
                paragraph.render(popup_area, buf);
            }
        }
    }
}

/// 只读辅助方法,避免测试/调用方直接解构 enum
#[cfg(test)]
impl PopupKind {
    /// 返回 Detail 弹窗的滚动偏移;非 Detail 返回 None
    pub(crate) fn detail_scroll(&self) -> Option<u16> {
        match self {
            PopupKind::Detail { scroll, .. } => Some(*scroll),
            _ => None,
        }
    }

    fn is_confirmed(&self) -> bool {
        match self {
            PopupKind::Confirm { confirmed, .. } => *confirmed,
            _ => false,
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
            scroll: 0,
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

    #[test]
    fn test_detail_scroll() {
        let mut stack = PopupStack::new();
        stack.push(PopupKind::Detail {
            title: "detail".into(),
            content: "line1\nline2\nline3".into(),
            scroll: 0,
        });

        stack.scroll_current(1);
        assert_eq!(stack.current().unwrap().detail_scroll(), Some(1));

        stack.scroll_current(-3);
        assert_eq!(stack.current().unwrap().detail_scroll(), Some(0));

        stack.scroll_current(10);
        assert_eq!(stack.current().unwrap().detail_scroll(), Some(10));
    }

    #[test]
    fn test_confirm_toggle() {
        let mut stack = PopupStack::new();
        stack.push(PopupKind::Confirm {
            prompt: "quit?".into(),
            on_confirm: "quit".into(),
            confirmed: true,
        });

        assert!(stack.current().unwrap().is_confirmed());
        stack.toggle_confirm();
        assert!(!stack.current().unwrap().is_confirmed());
        stack.toggle_confirm();
        assert!(stack.current().unwrap().is_confirmed());
    }
}
