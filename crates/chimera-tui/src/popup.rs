//! TUI 弹窗栈 — 通知、详情与确认弹窗的管理
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - `PopupStack` 用 Vec 实现 LIFO:支持多层弹窗叠加,后打开的弹窗优先显示,
//!   关闭后自动回到下层弹窗。
//! - `PopupKind` 区分通知/详情/确认三种语义,M3 完成确认弹窗渲染并支持
//!   详情弹窗滚动,为 M4 的用户确认流程预留扩展点。

use event_bus::NexusEvent;
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
    /// 帮助浮层 — 全局 `?` 键触发的快捷键速查表
    ///
    /// WHY 与 Help 面板区分:Help 面板是持久导航面板(会改变当前焦点),
    /// 而 Help overlay 是临时浮层,不切换当前面板,Esc 关闭,
    /// 用于在任何面板中快速查看快捷键而不丢失上下文。
    HelpOverlay {
        /// 快捷键表:按键描述 → 功能说明
        entries: Vec<(String, String)>,
        /// 垂直滚动偏移(行数)
        scroll: u16,
    },
    /// 事件详情弹窗 — 显示 MessagePack 解码后的 JSON 载荷、语法高亮与上下游事件 ID
    ///
    /// WHY P3.1:EventStream/Parliament/Log 面板按 Enter 后需要查看事件完整结构,
    /// 包括原始 MessagePack bytes、JSON 高亮展示与相关事件 ID 链,便于调试。
    EventDetail {
        /// 弹窗标题
        title: String,
        /// 事件类型名
        event_type: String,
        /// 解码后的 JSON 字符串(渲染时再做语法高亮)
        payload_decoded: String,
        /// 原始 MessagePack 字节(保留用于原始查看)
        payload_raw: Vec<u8>,
        /// 从载荷中提取的以 `_id` 结尾的相关事件 ID 列表
        related_event_ids: Vec<String>,
        /// 垂直滚动偏移(行数)
        scroll: u16,
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

    /// 创建默认快捷键帮助浮层
    ///
    /// WHY P3.2:将帮助表集中维护在弹窗类型内部,避免 app.rs 与各面板重复
    /// 定义。新增全局快捷键时只需修改此处即可同步到所有触发点。
    pub fn help_overlay() -> Self {
        Self::help_overlay_with_context(&[])
    }

    /// 创建带面板快捷键的上下文感知帮助浮层
    ///
    /// 在全局快捷键之后追加"面板快捷键"章节,展示当前焦点面板的 `shortcuts()` 返回值。
    /// 若 `panel_shortcuts` 为空,则不显示面板快捷键章节。
    pub fn help_overlay_with_context(panel_shortcuts: &[(&'static str, &'static str)]) -> Self {
        let mut entries = vec![
            ("q / Esc".into(), "退出应用".into()),
            ("Tab / Shift+Tab".into(), "切换下/上一个面板".into()),
            ("1-9".into(), "跳转到对应序号的面板".into()),
            (":".into(), "打开命令面板".into()),
            ("/".into(), "打开搜索过滤器".into()),
            ("?".into(), "显示本帮助浮层".into()),
            ("j / k".into(), "向下/向上滚动列表或弹窗".into()),
            ("Enter".into(), "查看选中项详情或确认操作".into()),
            ("Ctrl+↑ / Ctrl+↓".into(), "调整主面板显示比例".into()),
        ];

        // 追加面板快捷键章节(如有)
        if !panel_shortcuts.is_empty() {
            entries.push(("── 面板快捷键 ──".into(), String::new()));
            for (key, desc) in panel_shortcuts {
                entries.push((key.to_string(), desc.to_string()));
            }
        }

        Self::HelpOverlay { entries, scroll: 0 }
    }

    /// 从 `NexusEvent` 构造事件详情弹窗
    ///
    /// WHY P3.1:将 MessagePack 编码/解码、JSON 高亮准备与相关 ID 提取
    /// 集中在弹窗类型内部,避免 EventStream/Parliament/Log 三个面板重复实现。
    /// 解码失败时降级为 hex 展示,并在标题标注 `(raw hex)`。
    pub fn event_detail(event: &NexusEvent) -> Self {
        let event_type = event.type_name().to_string();
        let base_title = format!("{event_type} Detail");

        // WHY 先序列化为 JSON Value 再转 MessagePack:
        // NexusEvent 含 DateTime/Uuid 等类型,`rmp_serde::to_vec(event)` 会将其编码为
        // MessagePack 扩展类型,随后 `from_slice::<serde_json::Value>` 无法表示扩展类型,
        // 导致解码失败。先把事件转成 JSON Value(基本类型)后再做 MessagePack 往返,
        // 既保留 "MessagePack 解码为 JSON" 的语义,又避免扩展类型问题。
        let json_value = match serde_json::to_value(event) {
            Ok(v) => v,
            Err(_) => {
                return Self::EventDetail {
                    title: format!("{base_title} (raw hex)"),
                    event_type,
                    payload_decoded: "(encoding failed)".to_string(),
                    payload_raw: Vec::new(),
                    related_event_ids: Vec::new(),
                    scroll: 0,
                };
            }
        };

        let payload_raw = match rmp_serde::to_vec(&json_value) {
            Ok(bytes) => bytes,
            Err(_) => {
                return Self::EventDetail {
                    title: format!("{base_title} (raw hex)"),
                    event_type,
                    payload_decoded: "(encoding failed)".to_string(),
                    payload_raw: Vec::new(),
                    related_event_ids: Vec::new(),
                    scroll: 0,
                };
            }
        };

        match rmp_serde::from_slice::<serde_json::Value>(&payload_raw) {
            Ok(value) => {
                let related_event_ids = extract_related_ids(&value);
                let payload_decoded = serde_json::to_string_pretty(&value)
                    .unwrap_or_else(|_| hex_string(&payload_raw));
                Self::EventDetail {
                    title: base_title,
                    event_type,
                    payload_decoded,
                    payload_raw,
                    related_event_ids,
                    scroll: 0,
                }
            }
            Err(_) => Self::EventDetail {
                title: format!("{base_title} (raw hex)"),
                event_type,
                payload_decoded: hex_string(&payload_raw),
                payload_raw,
                related_event_ids: Vec::new(),
                scroll: 0,
            },
        }
    }
}

/// 将字节序列格式化为十六进制字符串(每字节两位,空格分隔)
fn hex_string(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// 从 JSON Value 中提取所有 key 以 `_id` 结尾的字符串值,作为相关事件 ID 链
///
/// WHY:EventMetadata 与各事件变体中的 ID 字段命名一致(如 `event_id`、
/// `quest_id`、`proposal_id`、`checkpoint_id`)，统一按后缀提取避免为每个
/// 变体手写匹配,同时保持去重与顺序稳定。
fn extract_related_ids(value: &serde_json::Value) -> Vec<String> {
    let mut ids = Vec::new();
    collect_related_ids(value, &mut ids);
    ids.dedup();
    ids
}

fn collect_related_ids(value: &serde_json::Value, ids: &mut Vec<String>) {
    match value {
        serde_json::Value::Object(map) => {
            for (k, v) in map {
                if k.ends_with("_id") {
                    if let serde_json::Value::String(s) = v {
                        ids.push(s.clone());
                    }
                }
                collect_related_ids(v, ids);
            }
        }
        serde_json::Value::Array(arr) => {
            for v in arr {
                collect_related_ids(v, ids);
            }
        }
        _ => {}
    }
}

/// 将 JSON 字符串渲染为带语法高亮的 ratatui 行列表
///
/// 颜色约定:字符串绿色、数字青色、布尔黄色、null 灰色、key 默认白色。
/// 解析失败时原样返回单行,确保渲染不 panic。
fn highlight_json(json: &str) -> Vec<Line<'static>> {
    match serde_json::from_str::<serde_json::Value>(json) {
        Ok(value) => value_to_lines(&value, 0),
        Err(_) => vec![Line::from(json.to_string())],
    }
}

fn value_to_lines(value: &serde_json::Value, indent: usize) -> Vec<Line<'static>> {
    match value {
        serde_json::Value::Null => vec![Line::from(Span::styled("null", Color::Gray))],
        serde_json::Value::Bool(b) => {
            vec![Line::from(Span::styled(b.to_string(), Color::Yellow))]
        }
        serde_json::Value::Number(n) => {
            vec![Line::from(Span::styled(n.to_string(), Color::Cyan))]
        }
        serde_json::Value::String(s) => {
            vec![Line::from(Span::styled(format!("\"{s}\""), Color::Green))]
        }
        serde_json::Value::Array(arr) if arr.is_empty() => vec![Line::from("[]")],
        serde_json::Value::Array(arr) => {
            let mut lines = vec![Line::from("[")];
            for (i, v) in arr.iter().enumerate() {
                let mut item_lines = value_to_lines(v, indent + 2);
                let is_last = i == arr.len() - 1;
                if let Some(last) = item_lines.last_mut() {
                    let mut spans = last.spans.clone();
                    spans.push(Span::raw(if is_last { "" } else { "," }));
                    *last = Line::from(spans);
                }
                lines.extend(item_lines);
            }
            lines.push(Line::from(format!("{}}}]", " ".repeat(indent))));
            lines
        }
        serde_json::Value::Object(map) if map.is_empty() => vec![Line::from("{}")],
        serde_json::Value::Object(map) => {
            let mut lines = vec![Line::from("{")];
            let entries: Vec<_> = map.iter().collect();
            for (i, (k, v)) in entries.iter().enumerate() {
                let key_prefix = format!("{}\"{k}\": ", " ".repeat(indent + 2));
                let is_last = i == entries.len() - 1;
                let mut value_lines = value_to_lines(v, indent + 2);
                if value_lines.len() == 1 {
                    let mut spans = vec![Span::raw(key_prefix)];
                    spans.extend(value_lines.into_iter().next().unwrap().spans);
                    spans.push(Span::raw(if is_last { "" } else { "," }));
                    lines.push(Line::from(spans));
                } else {
                    lines.push(Line::from(key_prefix));
                    if let Some(last) = value_lines.last_mut() {
                        let mut spans = last.spans.clone();
                        spans.push(Span::raw(if is_last { "" } else { "," }));
                        *last = Line::from(spans);
                    }
                    lines.extend(value_lines);
                }
            }
            lines.push(Line::from(format!("{}}}", " ".repeat(indent))));
            lines
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

    /// 滚动当前详情弹窗或帮助浮层
    ///
    /// `delta` 为正向下滚动,为负向上滚动。非可滚动弹窗被忽略。
    pub fn scroll_current(&mut self, delta: i16) {
        let scroll = match self.current_mut() {
            Some(PopupKind::Detail { scroll, .. }) => Some(scroll),
            Some(PopupKind::HelpOverlay { scroll, .. }) => Some(scroll),
            Some(PopupKind::EventDetail { scroll, .. }) => Some(scroll),
            _ => None,
        };
        if let Some(scroll) = scroll {
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
            PopupKind::HelpOverlay { entries, scroll } => {
                // WHY 使用深色背景 + 标题栏与 Notification 区分:
                // Help overlay 是覆盖在当前面板上的临时上下文帮助,
                // 需要视觉上明显区别于普通通知,同时不抢夺当前面板焦点的注意力。
                let block = Block::default()
                    .borders(Borders::ALL)
                    .title(" Help (?/Esc) ")
                    .style(Style::default().bg(Color::Rgb(30, 30, 46)));
                let lines: Vec<Line> = entries
                    .iter()
                    .map(|(key, desc)| {
                        Line::from(vec![
                            Span::styled(
                                format!("{key:20}"),
                                Style::default()
                                    .fg(Color::Cyan)
                                    .add_modifier(Modifier::BOLD),
                            ),
                            Span::raw(desc),
                        ])
                    })
                    .collect();
                let paragraph = Paragraph::new(Text::from(lines))
                    .block(block)
                    .wrap(Wrap { trim: true })
                    .scroll((*scroll, 0));
                paragraph.render(popup_area, buf);
            }
            PopupKind::EventDetail {
                title,
                event_type,
                payload_decoded,
                related_event_ids,
                scroll,
                ..
            } => {
                let block = Block::default()
                    .borders(Borders::ALL)
                    .title(format!(" {title} "));

                let mut lines = vec![
                    Line::from(vec![
                        Span::raw("Type: "),
                        Span::styled(event_type.clone(), Color::Cyan),
                    ]),
                    Line::from(""),
                ];
                lines.extend(highlight_json(payload_decoded));
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Related IDs:",
                    Style::default().add_modifier(Modifier::BOLD),
                )));
                if related_event_ids.is_empty() {
                    lines.push(Line::from("  (none)"));
                } else {
                    for id in related_event_ids {
                        lines.push(Line::from(format!("  • {id}")));
                    }
                }

                let paragraph = Paragraph::new(Text::from(lines))
                    .block(block)
                    .wrap(Wrap { trim: true })
                    .scroll((*scroll, 0));
                paragraph.render(popup_area, buf);
            }
        }
    }
}

/// 只读辅助方法,避免测试/调用方直接解构 enum
#[cfg(test)]
impl PopupKind {
    /// 返回 Detail/EventDetail 弹窗的滚动偏移;非可滚动详情弹窗返回 None
    pub(crate) fn detail_scroll(&self) -> Option<u16> {
        match self {
            PopupKind::Detail { scroll, .. } => Some(*scroll),
            PopupKind::EventDetail { scroll, .. } => Some(*scroll),
            _ => None,
        }
    }

    /// 当前弹窗是否为 HelpOverlay
    pub(crate) fn is_help_overlay(&self) -> bool {
        matches!(self, PopupKind::HelpOverlay { .. })
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
