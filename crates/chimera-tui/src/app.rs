//! TUI 应用核心 — 事件循环、渲染与状态管理
//!
//! 对应架构层:L10 Interface
//! 对应创新点:无(用户交互入口)
//!
//! # 设计决策(WHY)
//! - `state` 与 `config` 独立:状态可变,配置只读,分离便于测试
//! - `render` 接收 `&mut Frame`:与 ratatui 的 draw 闭包签名对齐,
//!   支持 TestBackend 内存渲染测试(无需真实终端)
//! - `run` 用 `no_run` 标注:涉及真实终端 IO,测试不调用,仅保证编译

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Tabs};
use ratatui::{Frame, Terminal};
use std::io::{self, Stdout};
use std::time::Duration;

use crate::config::Theme;
use crate::config::TuiConfig;
use crate::error::TuiError;
use crate::types::{PanelKind, TuiState};

/// TUI 应用 — Chimera 终端用户界面核心
///
/// 维护配置与状态,提供:
/// - 终端事件循环(键盘事件处理)
/// - 多面板渲染(基于 ratatui)
/// - 状态管理(面板切换、退出)
///
/// # 线程安全
/// TuiApp 为单线程设计(终端 IO 不支持多线程),`run` 方法独占终端。
pub struct TuiApp {
    /// TUI 配置(只读,构造后不变)
    config: TuiConfig,
    /// 应用状态(可变,事件循环中更新)
    state: TuiState,
}

impl TuiApp {
    /// 创建新的 TUI 应用
    ///
    /// # 错误
    /// - `ConfigError`:配置校验失败
    pub fn new(config: TuiConfig) -> Result<Self, TuiError> {
        config.validate()?;
        Ok(Self {
            config,
            state: TuiState::new(),
        })
    }

    /// 返回配置引用
    pub fn config(&self) -> &TuiConfig {
        &self.config
    }

    /// 返回状态引用
    pub fn state(&self) -> &TuiState {
        &self.state
    }

    /// 返回状态可变引用(测试与外部控制用)
    pub fn state_mut(&mut self) -> &mut TuiState {
        &mut self.state
    }

    /// 切换到下一个面板
    pub fn switch_panel_next(&mut self) {
        self.state.switch_next();
    }

    /// 切换到上一个面板
    pub fn switch_panel_prev(&mut self) {
        self.state.switch_prev();
    }

    /// 退出应用
    pub fn quit(&mut self) {
        self.state.quit();
    }

    /// 处理键盘事件
    ///
    /// WHY 独立方法:将事件处理与终端读取分离,便于单元测试
    /// (测试时直接构造 KeyEvent 调用此方法,无需真实终端)
    pub fn handle_key_event(&mut self, key: KeyEvent) {
        // WHY 检查 KeyEventKind:crossterm 在 Windows 上会触发 Release 事件,
        // 只处理 Press 避免重复响应(平台兼容性)
        if key.kind != KeyEventKind::Press {
            return;
        }
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit(),
            KeyCode::Tab => self.switch_panel_next(),
            KeyCode::BackTab => self.switch_panel_prev(),
            KeyCode::Char('1') => self.state.switch_to(PanelKind::Quest),
            KeyCode::Char('2') => self.state.switch_to(PanelKind::Parliament),
            KeyCode::Char('3') => self.state.switch_to(PanelKind::Budget),
            KeyCode::Char('4') => self.state.switch_to(PanelKind::Log),
            KeyCode::Char('5') => self.state.switch_to(PanelKind::Help),
            KeyCode::Char(c) if c.is_ascii_graphic() || c == ' ' => {
                self.state.append_input(c);
            }
            KeyCode::Backspace => {
                self.state.input_buffer.pop();
            }
            _ => {}
        }
    }

    /// 渲染 UI 到 Frame
    ///
    /// WHY 接收 &mut Frame:与 ratatui 的 draw 闭包签名对齐,
    /// 支持 TestBackend 内存渲染测试(无需真实终端)。
    ///
    /// # 布局
    /// - 顶部:面板标签栏(1 行)
    /// - 中部:主面板内容(自适应高度)
    /// - 底部:日志面板(log_panel_height 行)
    /// - 最底:状态栏(1 行)
    pub fn render(&self, frame: &mut Frame<'_>) {
        let area = frame.area();

        // 垂直分割:标签栏 + 主面板 + 日志面板 + 状态栏
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // 标签栏(含边框)
                Constraint::Min(1),    // 主面板
                Constraint::Length(self.config.log_panel_height),
                Constraint::Length(1), // 状态栏
            ])
            .split(area);

        // 渲染标签栏
        self.render_tabs(frame, chunks[0]);

        // 渲染主面板
        self.render_main_panel(frame, chunks[1]);

        // 渲染日志面板
        self.render_log_panel(frame, chunks[2]);

        // 渲染状态栏
        self.render_status_bar(frame, chunks[3]);
    }

    /// 渲染面板标签栏
    fn render_tabs(&self, frame: &mut Frame<'_>, area: ratatui::layout::Rect) {
        let titles: Vec<Line> = [
            PanelKind::Quest,
            PanelKind::Parliament,
            PanelKind::Budget,
            PanelKind::Log,
            PanelKind::Help,
        ]
        .iter()
        .map(|p| Line::from(format!(" {} ", p.as_str())))
        .collect();

        let selected = match self.state.current_panel {
            PanelKind::Quest => 0,
            PanelKind::Parliament => 1,
            PanelKind::Budget => 2,
            PanelKind::Log => 3,
            PanelKind::Help => 4,
        };

        let tabs = Tabs::new(titles)
            .select(selected)
            .style(Style::default().fg(Color::Gray))
            .highlight_style(
                Style::default()
                    .fg(self.theme_fg())
                    .add_modifier(Modifier::BOLD),
            )
            .block(Block::default().borders(Borders::ALL).title(" Panels "));

        frame.render_widget(tabs, area);
    }

    /// 渲染主面板(当前激活面板的内容)
    fn render_main_panel(&self, frame: &mut Frame<'_>, area: ratatui::layout::Rect) {
        let content = self.panel_content();
        let block = Block::default()
            .borders(Borders::ALL)
            .title(self.state.current_panel.title())
            .style(Style::default().fg(self.theme_fg()));

        let paragraph = Paragraph::new(content).block(block);
        frame.render_widget(paragraph, area);
    }

    /// 渲染日志面板
    fn render_log_panel(&self, frame: &mut Frame<'_>, area: ratatui::layout::Rect) {
        let log_content = " [System] Chimera TUI initialized\n [Event] Waiting for events...";
        let block = Block::default()
            .borders(Borders::ALL)
            .title(" System Log ")
            .style(Style::default().fg(Color::DarkGray));

        let paragraph = Paragraph::new(log_content).block(block);
        frame.render_widget(paragraph, area);
    }

    /// 渲染状态栏
    fn render_status_bar(&self, frame: &mut Frame<'_>, area: ratatui::layout::Rect) {
        let status = format!(
            " Panel: {} | Running: {} | Frame: {} | Input: {} ",
            self.state.current_panel.as_str(),
            self.state.running,
            self.state.frame_count,
            self.state.input_buffer
        );

        let span = Span::styled(
            status,
            Style::default()
                .fg(Color::Black)
                .bg(self.theme_accent())
                .add_modifier(Modifier::BOLD),
        );
        let line = Line::from(span);
        let paragraph = Paragraph::new(line);
        frame.render_widget(paragraph, area);
    }

    /// 返回当前面板的占位内容
    fn panel_content(&self) -> String {
        match self.state.current_panel {
            PanelKind::Quest => {
                "Quest Tasks\n─────────────\n[1] Initialize workspace\n[2] Build L1 infrastructure\n[3] Implement event-bus\n\nPress Tab to switch panels, 'q' to quit."
                    .to_string()
            }
            PanelKind::Parliament => {
                "Parliament\n─────────────\nVisionary:  vote FOR\nSkeptic:     vote AGAINST\nPragmatist:  vote FOR\n\nConsensus: REACHED"
                    .to_string()
            }
            PanelKind::Budget => {
                format!(
                    "Budget\n─────────────\nCurrent Tier: L3 (Abundant)\nConsumption:  0 / {}\nUtilization:  0.0%\n\nStatus: OK",
                    1_000_000
                )
            }
            PanelKind::Log => {
                "System Log\n─────────────\n[INFO]  System initialized\n[DEBUG] Event bus subscribed\n[WARN]  No critical events\n[ERROR] (none)"
                    .to_string()
            }
            PanelKind::Help => {
                "Help\n─────────────\nTab     - Next panel\nShift+Tab - Previous panel\n1-5     - Jump to panel\nq / Esc - Quit\n\nChimera CLI NEXUS-OMEGA"
                    .to_string()
            }
        }
    }

    /// 返回主题前景色
    fn theme_fg(&self) -> Color {
        match self.config.theme {
            Theme::Dark => Color::White,
            Theme::Light => Color::Black,
        }
    }

    /// 返回主题强调色
    fn theme_accent(&self) -> Color {
        match self.config.theme {
            Theme::Dark => Color::Cyan,
            Theme::Light => Color::Blue,
        }
    }

    /// 启动 TUI 事件循环
    ///
    /// 此方法接管终端:进入 raw mode、alternate screen,读取键盘事件,
    /// 渲染 UI,直到用户退出(q/Esc)。退出后恢复终端状态。
    ///
    /// # 错误
    /// - `TerminalInit`:终端初始化失败(如非 TTY 环境)
    /// - `EventRead`:事件读取失败
    /// - `Render`:渲染失败
    /// - `TerminalRestore`:终端恢复失败
    ///
    /// # Panics
    /// 此方法不主动 panic,但 crossterm 内部若遇致命错误可能返回 io::Error。
    pub fn run(&mut self) -> Result<(), TuiError> {
        // 步骤 1:启用 raw mode 与 alternate screen
        enable_raw_mode().map_err(|e| TuiError::TerminalInit(e.to_string()))?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)
            .map_err(|e| TuiError::TerminalInit(e.to_string()))?;

        // 步骤 2:创建终端
        let backend = CrosstermBackend::new(stdout);
        let mut terminal =
            Terminal::new(backend).map_err(|e| TuiError::TerminalInit(e.to_string()))?;

        // 步骤 3:事件循环
        // WHY 用 result 变量:确保终端恢复在 return 前执行,即使事件循环出错
        let result = self.event_loop(&mut terminal);

        // 步骤 4:恢复终端(无论事件循环成功与否)
        // WHY 恢复在 result 返回前:确保终端状态不残留,即使出错也要恢复
        disable_raw_mode().map_err(|e| TuiError::TerminalRestore(e.to_string()))?;
        execute!(terminal.backend_mut(), LeaveAlternateScreen)
            .map_err(|e| TuiError::TerminalRestore(e.to_string()))?;

        result
    }

    /// 事件循环内部实现
    ///
    /// WHY 独立方法:将循环逻辑与终端初始化/恢复分离,职责单一
    fn event_loop(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<Stdout>>,
    ) -> Result<(), TuiError> {
        while self.state.running {
            // 渲染当前帧
            terminal
                .draw(|f| self.render(f))
                .map_err(|e| TuiError::Render(e.to_string()))?;
            self.state.tick_frame();

            // 轮询事件(100ms 超时,避免阻塞渲染)
            if !event::poll(Duration::from_millis(100))
                .map_err(|e| TuiError::EventRead(e.to_string()))?
            {
                continue;
            }

            // 读取并处理事件
            let event = event::read().map_err(|e| TuiError::EventRead(e.to_string()))?;
            if let Event::Key(key) = event {
                self.handle_key_event(key);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::backend::TestBackend;

    fn make_app() -> TuiApp {
        TuiApp::new(TuiConfig::default()).unwrap()
    }

    // ============================================================
    // 应用初始化测试
    // ============================================================

    #[test]
    fn test_app_new() {
        let app = make_app();
        assert_eq!(app.state().current_panel, PanelKind::Quest);
        assert!(app.state().running);
        assert_eq!(app.config().theme, Theme::Dark);
    }

    #[test]
    fn test_app_invalid_config_rejected() {
        let config = TuiConfig {
            main_panel_ratio: 0.0,
            ..Default::default()
        };
        assert!(TuiApp::new(config).is_err());
    }

    // ============================================================
    // 面板切换测试
    // ============================================================

    #[test]
    fn test_switch_panel_next() {
        let mut app = make_app();
        assert_eq!(app.state().current_panel, PanelKind::Quest);
        app.switch_panel_next();
        assert_eq!(app.state().current_panel, PanelKind::Parliament);
        app.switch_panel_next();
        assert_eq!(app.state().current_panel, PanelKind::Budget);
    }

    #[test]
    fn test_switch_panel_prev() {
        let mut app = make_app();
        app.switch_panel_prev();
        assert_eq!(app.state().current_panel, PanelKind::Help);
    }

    #[test]
    fn test_quit() {
        let mut app = make_app();
        assert!(app.state().running);
        app.quit();
        assert!(!app.state().running);
    }

    // ============================================================
    // 键盘事件处理测试
    // ============================================================

    #[test]
    fn test_handle_key_q_quits() {
        let mut app = make_app();
        // WHY KeyEvent::new(code, modifiers):crossterm 0.28 的 2 参数构造,
        // kind 默认为 Press,state 默认为 NONE
        app.handle_key_event(KeyEvent::new(KeyCode::Char('q'), event::KeyModifiers::NONE));
        assert!(!app.state().running);
    }

    #[test]
    fn test_handle_key_esc_quits() {
        let mut app = make_app();
        app.handle_key_event(KeyEvent::new(KeyCode::Esc, event::KeyModifiers::NONE));
        assert!(!app.state().running);
    }

    #[test]
    fn test_handle_key_tab_switches_panel() {
        let mut app = make_app();
        app.handle_key_event(KeyEvent::new(KeyCode::Tab, event::KeyModifiers::NONE));
        assert_eq!(app.state().current_panel, PanelKind::Parliament);
    }

    #[test]
    fn test_handle_key_number_jumps_to_panel() {
        let mut app = make_app();
        app.handle_key_event(KeyEvent::new(KeyCode::Char('3'), event::KeyModifiers::NONE));
        assert_eq!(app.state().current_panel, PanelKind::Budget);
    }

    #[test]
    fn test_handle_key_release_ignored() {
        // WHY Windows 兼容:Release 事件应被忽略
        // 用 new_with_kind 显式指定 Release,验证 handle_key_event 的 kind 过滤
        let mut app = make_app();
        app.handle_key_event(KeyEvent::new_with_kind(
            KeyCode::Char('q'),
            event::KeyModifiers::NONE,
            event::KeyEventKind::Release,
        ));
        assert!(app.state().running, "Release event should be ignored");
    }

    #[test]
    fn test_handle_key_input_buffer() {
        let mut app = make_app();
        for c in ['h', 'e', 'l', 'l', 'o'] {
            app.handle_key_event(KeyEvent::new(KeyCode::Char(c), event::KeyModifiers::NONE));
        }
        assert_eq!(app.state().input_buffer, "hello");
    }

    #[test]
    fn test_handle_key_backspace() {
        let mut app = make_app();
        app.state.input_buffer = "abc".to_string();
        app.handle_key_event(KeyEvent::new(KeyCode::Backspace, event::KeyModifiers::NONE));
        assert_eq!(app.state().input_buffer, "ab");
    }

    // ============================================================
    // 渲染测试(使用 TestBackend,无需真实终端)
    // ============================================================

    #[test]
    fn test_render_produces_output() {
        let app = make_app();
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();

        // 验证渲染后缓冲区非空(TestBackend 会填充缓冲区)
        let buffer = terminal.backend().buffer();
        // 检查状态栏包含 "Panel:" 文本
        let content: String = buffer
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(
            content.contains("Panel:") || content.contains("Quest"),
            "rendered output should contain panel info"
        );
    }

    #[test]
    fn test_render_switches_panel_content() {
        let mut app = make_app();
        app.switch_panel_next(); // Quest → Parliament

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();

        let buffer = terminal.backend().buffer();
        let content: String = buffer
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(
            content.contains("Parliament"),
            "rendered output should contain Parliament panel"
        );
    }

    #[test]
    fn test_render_help_panel() {
        let mut app = make_app();
        app.state.switch_to(PanelKind::Help);

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();

        let buffer = terminal.backend().buffer();
        let content: String = buffer
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(
            content.contains("Help") || content.contains("Quit"),
            "rendered output should contain Help panel content"
        );
    }

    // ============================================================
    // 主题颜色测试
    // ============================================================

    #[test]
    fn test_theme_fg_dark() {
        let app = make_app();
        assert_eq!(app.theme_fg(), Color::White);
    }

    #[test]
    fn test_theme_fg_light() {
        let app = TuiApp::new(TuiConfig {
            theme: Theme::Light,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(app.theme_fg(), Color::Black);
        assert_eq!(app.theme_accent(), Color::Blue);
    }

    #[test]
    fn test_theme_accent_dark() {
        let app = make_app();
        assert_eq!(app.theme_accent(), Color::Cyan);
    }

    // ============================================================
    // 面板内容测试
    // ============================================================

    #[test]
    fn test_panel_content_quest() {
        let app = make_app();
        let content = app.panel_content();
        assert!(content.contains("Quest"));
        assert!(content.contains("Tab"));
    }

    #[test]
    fn test_panel_content_budget() {
        let mut app = make_app();
        app.state.switch_to(PanelKind::Budget);
        let content = app.panel_content();
        assert!(content.contains("Budget"));
        assert!(content.contains("L3"));
    }
}
