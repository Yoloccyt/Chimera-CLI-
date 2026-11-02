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
//! - **自动贴底**:默认跟随最新消息(`follow`);用户上滚(↑/k)接管,`G` 或贴底
//!   状态下的 ↓/j/滚轮向下保持/恢复贴底(F-26,与 EventStream 流式输出 UX 一致)。
//! - **预折行**:超宽消息在 `content_lines` 阶段按面板显示宽度折行(F-25),
//!   行数即渲染行数,贴底/滚动边界与之同源。

use crossterm::event::{KeyCode, KeyEvent, MouseEvent, MouseEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

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
    /// 最近一次 render 计算的贴底偏移(折行后总行数 − 可视行数)
    ///
    /// WHY 渲染期缓存:handle_key/handle_mouse 无 width 参数,无法按折行后行数
    /// 自算 max_offset;沿用 EventStreamPanel filter_cache 的「渲染期计算、
    /// 键处理期读取」先例(F-26 贴底语义依赖该判定)。
    last_max_offset: Option<usize>,
}

impl Default for ChatPanel {
    fn default() -> Self {
        Self {
            scroll_offset: 0,
            // WHY 默认贴底:对话是"最新在底"的追加流,启动即跟随最新回答。
            follow: true,
            last_max_offset: None,
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

    /// 将对话历史构建为渲染行:每条消息以角色前缀起始,多行内容按 `\n` 拆分,
    /// 超出面板宽度的行按显示宽度预折行(F-25)。
    ///
    /// WHY 预折行而非 `Paragraph::wrap`:max_offset(贴底/滚动边界)必须与实际
    /// 渲染行数同源——预折行后 `lines.len()` 即渲染行数,滚动语义不依赖
    /// ratatui WordWrapper 的内部折行行为。宽度经 unicode-width 计算,与
    /// ratatui 列宽同源(该依赖已在 Cargo.toml,零新增)。
    ///
    /// WHY 独立方法:渲染行构建与滚动/边框解耦,便于单元测试直接断言文本。
    pub fn content_lines(messages: &[ChatMessage], width: u16) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        for msg in messages {
            let (prefix, style) = Self::role_style(msg.role);
            // 折行可用宽 = 面板内容宽 − 角色前缀宽;续行缩进前缀与角色前缀等宽,
            // 故首行/续行共用同一折行宽度。
            let usable = (width as usize).saturating_sub(prefix.width());
            // 空内容(如刚起新的 Assistant 流式条,尚无 token)仍占一行显示前缀,
            // 让用户看到"AI 正在回答"的占位行。
            let mut segments = msg.content.split('\n');
            let first = segments.next().unwrap_or("");
            for (i, part) in Self::wrap_by_display_width(first, usable)
                .into_iter()
                .enumerate()
            {
                if i == 0 {
                    lines.push(Line::from(vec![
                        Span::styled(prefix, style.add_modifier(Modifier::BOLD)),
                        Span::styled(part, Style::default()),
                    ]));
                } else {
                    lines.push(Line::from(vec![
                        Span::raw("       "),
                        Span::styled(part, Style::default()),
                    ]));
                }
            }
            // 后续逻辑行缩进对齐前缀宽度,保持可读性
            for cont in segments {
                for part in Self::wrap_by_display_width(cont, usable) {
                    lines.push(Line::from(vec![
                        Span::raw("       "),
                        Span::styled(part, Style::default()),
                    ]));
                }
            }
        }
        lines
    }

    /// 按显示宽度把单行文本贪心折成多段(字符级断行;零宽字符不触发断行)
    ///
    /// WHY 字符级而非按词断行:中文(项目主用语言)无空格分词,逐字断行即正确
    /// 排版;按词断行对 CJK 退化为整段硬断且需复刻 WordWrapper 语义,得不偿失。
    /// 控制字符按 0 宽计(与 unicode-width 的 UnicodeWidthStr 口径一致)。
    fn wrap_by_display_width(text: &str, usable: usize) -> Vec<String> {
        // 极端窄面板无可用宽度时不折,交由渲染层截断兜底
        if usable == 0 {
            return vec![text.to_string()];
        }
        let mut parts: Vec<String> = Vec::new();
        let mut cur = String::new();
        let mut cur_w: usize = 0;
        for ch in text.chars() {
            let w = ch.width().unwrap_or(0);
            // WHY 限 cur 非空:单个超宽字符(极端窄面板)独占一行,不产生空段
            if w > 0 && !cur.is_empty() && cur_w + w > usable {
                parts.push(std::mem::take(&mut cur));
                cur_w = 0;
            }
            cur.push(ch);
            cur_w += w;
        }
        parts.push(cur);
        parts
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
        // PS-2 U-4:退化尺寸统一早退(共享最低线,见 crate::panels::MIN_PANEL_W/H)
        if crate::panels::degenerate(area) {
            crate::panels::render_too_small(area, buf);
            return;
        }

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

        let lines = Self::content_lines(&state.chat_messages, inner.width);
        let visible = inner.height as usize;
        let total = lines.len();
        // 贴底跟随:follow=true 时滚到底部;否则钳制用户偏移到有效范围。
        let max_offset = total.saturating_sub(visible);
        if self.follow {
            self.scroll_offset = max_offset;
        } else {
            self.scroll_offset = self.scroll_offset.min(max_offset);
        }
        // F-26:缓存贴底偏移,供键/滚轮处理判定"视觉底部"
        // (handle_key/handle_mouse 无 width 参数,无法自行按折行后行数计算)
        self.last_max_offset = Some(max_offset);

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
                // F-26:视觉底部按 ↓/j 保持贴底跟随(对照 EventStream 底部按 Down
                // 恢复 auto_scroll 的语义);修复前无条件解除 follow,新回复到达后
                // 视口滞留原地、新消息在视口外且无未读提示。非底部才解除跟随。
                match self.last_max_offset {
                    Some(max) if self.scroll_offset < max => {
                        self.follow = false;
                        self.scroll_offset = self.scroll_offset.saturating_add(1);
                    }
                    // 已在底部(或尚未渲染过、默认贴底):保持/恢复跟随,
                    // 偏移交由 render 按内容高度钳制
                    _ => self.follow = true,
                }
                None
            }
            _ => None,
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent, _state: &mut TuiState) -> Option<TuiCommand> {
        // WHY:mouse.rs 的 Chat 视图滚轮直取本面板,但 trait 默认实现是静默 None,
        // 滚轮此前无任何滚动语义(F-03 守护测试改真断言的前置)。语义与 j/k 键
        // 一致——滚动接管退出贴底,但贴底状态下向下滚动保持跟随(F-26,与 Down/j
        // 同语义);越界由 render 按内容高度钳制。
        match mouse.kind {
            MouseEventKind::ScrollUp => {
                self.follow = false;
                self.scroll_offset = self.scroll_offset.saturating_sub(1);
                None
            }
            MouseEventKind::ScrollDown => {
                // F-26:与 Down/j 同语义,贴底滚动不解除跟随(滚轮同病同修)
                match self.last_max_offset {
                    Some(max) if self.scroll_offset < max => {
                        self.follow = false;
                        self.scroll_offset = self.scroll_offset.saturating_add(1);
                    }
                    _ => self.follow = true,
                }
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
