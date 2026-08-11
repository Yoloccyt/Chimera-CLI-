//! TUI Chat 面板 — 交互式 Agent 对话历史(M3b)
//!
//! 对应架构层:L10 Interface
//! 对应 PanelId:Chat
//!
//! # 设计决策(WHY)
//! - **纯展示面板**:对话历史由 `ChatSync`(DataPipeline)单一拥有,经 `DataSnapshot`
//!   同步到 `TuiState::chat_messages`;本面板只读渲染,不持有历史(与其余数据面板一致)。
//! - **输入不在面板内**:文本输入经全局 Insert 模式在底部 `command_palette` 输入行完成
//!   (`> {buffer}`),Enter 提交由 `TuiApp` 发布 `TuiChatSubmitted`;面板只呈现历史 +
//!   会话状态指示器,避免重复输入源。
//! - **自动贴底**:默认跟随最新消息(`follow`);用户上滚(↑/k)接管,`G` 恢复贴底,
//!   借鉴 EventStream 的流式输出 UX。

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::panels::Panel;
use crate::types::{ChatMessage, ChatRole, PanelId, TuiCommand, TuiState};
use event_bus::ChatStatus;

/// Chat 面板 — 渲染对话历史 + 会话状态指示器
#[derive(Debug, Clone, PartialEq)]
pub struct ChatPanel {
    /// 渲染起始行偏移(仅在非贴底时由用户滚动控制)
    scroll_offset: usize,
    /// 是否自动跟随最新消息(贴底);上滚后置 false,`G` 恢复 true
    follow: bool,
}

impl Default for ChatPanel {
    fn default() -> Self {
        Self {
            scroll_offset: 0,
            // WHY 默认贴底:对话是"最新在底"的追加流,启动即跟随最新回答。
            follow: true,
        }
    }
}

impl ChatPanel {
    /// 创建新的 Chat 面板
    pub fn new() -> Self {
        Self::default()
    }

    /// 返回当前滚动偏移(测试用,与其他面板一致)
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }

    /// 会话状态 → 本地化标签(状态指示器文本)
    fn status_label(status: ChatStatus) -> &'static str {
        match status {
            ChatStatus::Thinking => crate::t!("chat.status.thinking"),
            ChatStatus::ToolExecuting => crate::t!("chat.status.tool"),
            ChatStatus::Idle => crate::t!("chat.status.idle"),
        }
    }

    /// 单条消息的角色前缀标记与配色
    ///
    /// WHY 硬编码配色:与 LogPanel 一致的直接配色策略,User/Assistant 视觉区分即可,
    /// 主题化留后续统一处理。
    fn role_style(role: ChatRole) -> (&'static str, Style) {
        match role {
            ChatRole::User => ("▸ You  ", Style::default().fg(Color::Cyan)),
            ChatRole::Assistant => ("◂ AI   ", Style::default().fg(Color::Green)),
        }
    }

    /// 将对话历史构建为渲染行:每条消息以角色前缀起始,多行内容按 `\n` 拆分。
    ///
    /// WHY 独立方法:渲染行构建与滚动/边框解耦,便于单元测试直接断言文本。
    pub fn content_lines(messages: &[ChatMessage]) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        for msg in messages {
            let (prefix, style) = Self::role_style(msg.role);
            // 空内容(如刚起新的 Assistant 流式条,尚无 token)仍占一行显示前缀,
            // 让用户看到"AI 正在回答"的占位行。
            let mut segments = msg.content.split('\n');
            let first = segments.next().unwrap_or("");
            lines.push(Line::from(vec![
                Span::styled(prefix, style.add_modifier(Modifier::BOLD)),
                Span::styled(first.to_string(), Style::default()),
            ]));
            // 后续行缩进对齐前缀宽度,保持可读性
            for cont in segments {
                lines.push(Line::from(vec![
                    Span::raw("       "),
                    Span::styled(cont.to_string(), Style::default()),
                ]));
            }
        }
        lines
    }
}

impl Panel for ChatPanel {
    fn id(&self) -> PanelId {
        PanelId::Chat
    }

    fn title(&self) -> Line<'static> {
        Line::from(crate::t!("panel.border.chat"))
    }

    fn render(&mut self, state: &TuiState, area: Rect, buf: &mut Buffer) {
        // 边框标题内嵌会话状态指示器,如 " Chat  [Idle]"
        let title = format!(
            "{} [{}]",
            crate::t!("panel.border.chat"),
            Self::status_label(state.chat_status)
        );
        let block = Block::default()
            .borders(Borders::ALL)
            .title(Line::from(title));
        let inner = block.inner(area);
        block.render(area, buf);

        let lines = Self::content_lines(&state.chat_messages);
        let visible = inner.height as usize;
        let total = lines.len();
        // 贴底跟随:follow=true 时滚到底部;否则钳制用户偏移到有效范围。
        let max_offset = total.saturating_sub(visible);
        if self.follow {
            self.scroll_offset = max_offset;
        } else {
            self.scroll_offset = self.scroll_offset.min(max_offset);
        }

        let paragraph = Paragraph::new(Text::from(lines)).scroll((self.scroll_offset as u16, 0));
        paragraph.render(inner, buf);
    }

    fn handle_key(&mut self, key: KeyEvent, _state: &mut TuiState) -> Option<TuiCommand> {
        // WHY 仅处理面板级滚动键:全局键(q/Tab/g/G/数字 等)由 InputRouter 拦截;
        // Insert 模式下字符进输入缓冲不会到达此处。故此处只需 ↑/↓/j/k 行滚动。
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.follow = false;
                self.scroll_offset = self.scroll_offset.saturating_sub(1);
                None
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.follow = false;
                self.scroll_offset = self.scroll_offset.saturating_add(1);
                None
            }
            _ => None,
        }
    }

    fn scroll_to_top(&mut self, _state: &mut TuiState) {
        // gg:跳到最早消息,退出贴底跟随
        self.follow = false;
        self.scroll_offset = 0;
    }

    fn scroll_to_bottom(&mut self, _state: &mut TuiState) {
        // G:恢复贴底跟随最新消息(实际偏移在 render 时按内容高度计算)
        self.follow = true;
    }

    fn shortcuts(&self) -> Vec<(&'static str, &'static str)> {
        vec![
            ("i", crate::t!("shortcut.input_message")),
            ("Enter", crate::t!("shortcut.send")),
            ("Esc", crate::t!("shortcut.exit_input")),
            ("↑/↓ j/k", crate::t!("shortcut.scroll")),
            ("g g / G", crate::t!("shortcut.top_bottom")),
        ]
    }
}
