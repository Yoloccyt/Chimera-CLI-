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
//! - M1 引入 `Panel` trait + `FocusManager` + `CommandPalette` + `PopupStack`:
//!   将原本硬编码在 `app.rs` 中的面板切换/渲染/输入逻辑拆分为可扩展架构,
//!   为 M2/M3/M4 的新面板与控制功能提供插拔点。
//! - M2 迁移 Parliament/Log/Help 到独立模块,并新增 Memory/Security/Health 面板。
//! - M2 清理 `TuiState.current_panel` 双来源:当前面板以 `FocusManager` 为准,
//!   `TuiApp::current_panel()` 对外暴露。
//! - M3 增加鼠标支持、可调整主面板比例、弹窗滚动与确认弹窗处理。

use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, MouseButton, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Tabs};
use ratatui::{Frame, Terminal};
use std::io::{self, Stdout};
use std::time::Duration;

use crate::command_palette::CommandPalette;
use crate::config::Theme;
use crate::config::TuiConfig;
use crate::data::{StubDataSource, TuiDataSource};
use crate::error::TuiError;
use crate::focus::FocusManager;
use crate::panels::{
    BudgetPanel, ChtcPanel, DecayPanel, EventStreamPanel, HealthPanel, HelpPanel, LogPanel,
    McpNodesPanel, MemoryPanel, Panel, ParliamentPanel, QuestPanel, RouterPanel, SecurityPanel,
};
use crate::popup::{PopupKind, Severity};
use crate::types::{InputMode, PanelId, TuiCommand, TuiState};
use event_bus::{EventBus, EventMetadata, NexusEvent, VoteValue};

/// 主面板比例调整步长
const RATIO_STEP: f32 = 0.05;
/// 主面板比例最小值
const RATIO_MIN: f32 = 0.3;
/// 主面板比例最大值
const RATIO_MAX: f32 = 0.9;

/// TUI 应用 — Chimera 终端用户界面核心
///
/// 维护配置与状态,提供:
/// - 终端事件循环(键盘/鼠标事件处理)
/// - 多面板渲染(基于 ratatui 与 `Panel` trait)
/// - 状态管理(面板切换、退出、命令面板、弹窗栈)
///
/// # 线程安全
/// TuiApp 为单线程设计(终端 IO 不支持多线程),`run` 方法独占终端。
pub struct TuiApp {
    /// TUI 配置(只读,构造后不变)
    config: TuiConfig,
    /// 当前会话的主面板比例(从配置初始化,不持久化到文件)
    main_panel_ratio: f32,
    /// 应用状态(可变,事件循环中更新)
    state: TuiState,
    /// 数据源(抽象,支持内存桩、事件管道或测试替身)
    ///
    /// WHY `Box<dyn>`:TUI 主循环不需要知道数据来自 event-bus 还是测试桩;
    /// trait object 避免在 `TuiApp` 上引入泛型,简化 CLI 入口的实例化。
    data_source: Box<dyn TuiDataSource>,
    /// 面板集合
    ///
    /// WHY `Box<dyn Panel>`:M1 用 trait object 实现面板插件化,
    /// 新增面板只需加入此向量,无需修改事件循环。
    panels: Vec<Box<dyn Panel>>,
    /// 焦点管理器
    focus_manager: FocusManager,
    /// 命令面板
    command_palette: CommandPalette,
    /// 上一帧的焦点面板,用于避免每帧重复调用 `focus(true/false)`
    ///
    /// WHY M1 清理项 #5:仅在实际变化时通知面板焦点变化,减少无效回调。
    last_focused: Option<PanelId>,
    /// 最后一帧的终端区域,用于鼠标事件命中测试
    last_area: Rect,
    /// 可选的事件总线引用,用于发布控制请求事件(M4 双向控制)
    ///
    /// WHY Option:测试与普通启动场景可能不需要 EventBus,避免强制依赖。
    event_bus: Option<EventBus>,
}

impl TuiApp {
    /// 创建新的 TUI 应用
    ///
    /// 默认使用内存桩数据源，返回空 `DataSnapshot`，无需 event-bus 连接即可启动。
    ///
    /// # 错误
    /// - `ConfigError`:配置校验失败
    pub fn new(config: TuiConfig) -> Result<Self, TuiError> {
        Self::with_data_source(config, Box::new(StubDataSource::new()))
    }

    /// 使用指定数据源创建 TUI 应用
    ///
    /// 生产环境通常传入 `DataPipeline`，测试可传入自定义桩实现。
    ///
    /// # 错误
    /// - `ConfigError`:配置校验失败
    pub fn with_data_source(
        config: TuiConfig,
        data_source: Box<dyn TuiDataSource>,
    ) -> Result<Self, TuiError> {
        config.validate()?;
        // P2 TUI v1.7-omega:注册 13 个面板(8 原始 + 5 新增监控面板)。
        // WHY 不含 Timeline:Timeline 面板由 P7 历史回放引擎(v1.8+)实现,
        // 当前 PanelId::Timeline 已定义但无对应 Panel 实现,故不注册。
        // FocusManager 循环顺序:Quest → Parliament → ... → Help → Decay
        // → EventStream → Router → McpNodes → Chtc → Quest(13 面板循环)。
        let panels: Vec<Box<dyn Panel>> = vec![
            Box::new(QuestPanel::new()),
            Box::new(ParliamentPanel::new()),
            Box::new(BudgetPanel::new()),
            Box::new(MemoryPanel::new()),
            Box::new(SecurityPanel::new()),
            Box::new(HealthPanel::new()),
            Box::new(LogPanel::new()),
            Box::new(HelpPanel::new()),
            // P2 新增监控面板(占位实现,后续 Task 填充具体渲染逻辑)
            Box::new(DecayPanel::new()),
            Box::new(EventStreamPanel::new()),
            Box::new(RouterPanel::new()),
            Box::new(McpNodesPanel::new()),
            Box::new(ChtcPanel::new()),
        ];
        let panel_ids: Vec<PanelId> = panels.iter().map(|p| p.id()).collect();
        let focus_manager = FocusManager::new(panel_ids);
        let state = TuiState::new();
        let main_panel_ratio = config.main_panel_ratio;

        Ok(Self {
            config,
            main_panel_ratio,
            state,
            data_source,
            panels,
            focus_manager,
            command_palette: CommandPalette::new(),
            last_focused: None,
            last_area: Rect::default(),
            event_bus: None,
        })
    }

    /// 将 EventBus 绑定到已有 TUI 应用
    ///
    /// WHY M4:CLI 在创建 TUI 后注入生产 EventBus,使 TUI 获得双向控制能力。
    pub fn with_event_bus(mut app: Self, bus: EventBus) -> Self {
        app.event_bus = Some(bus);
        app
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

    /// 返回当前主面板比例(会话级,不持久化)
    pub fn main_panel_ratio(&self) -> f32 {
        self.main_panel_ratio
    }

    /// 返回当前焦点面板
    ///
    /// WHY M1 清理项 #2:`FocusManager` 是当前面板的唯一来源,
    /// 避免与 `TuiState.current_panel` 双来源不一致。
    pub fn current_panel(&self) -> PanelId {
        self.focus_manager.focused()
    }

    /// 从数据源拉取最新快照并更新状态
    ///
    /// WHY 独立方法:将数据刷新与事件循环解耦，便于单元测试直接调用验证，
    /// 也允许未来在渲染之外的时刻(如收到特定按键)手动刷新。
    pub fn update(&mut self) {
        match self.data_source.snapshot() {
            Ok(snapshot) => {
                self.state.quest_list = snapshot.quest_list;
                self.state.budget = snapshot.budget_metrics;
                self.state.memory_metrics = snapshot.memory_metrics;
                self.state.security_state = snapshot.security_state;
                self.state.health_metrics = snapshot.health_metrics;
                self.state.budget_history = snapshot.budget_history;
                self.state.memory_history = snapshot.memory_history;
                self.state.event_rate_history = snapshot.event_rate_history;
                self.state.latest_events = snapshot.latest_events;
                // P2 新增字段同步:DataSnapshot → TuiState
                self.state.decay_metrics = snapshot.decay_metrics;
                self.state.router_metrics = snapshot.router_metrics;
                self.state.mcp_nodes = snapshot.mcp_nodes;
                self.state.chtc_state = snapshot.chtc_state;
                self.state.decay_history = snapshot.decay_history;
            }
            Err(e) => {
                // M1 清理项 #4:数据源失败时向用户展示状态栏警告,而非静默忽略。
                self.state.status_message =
                    Some((format!("data source unavailable: {e}"), Severity::Warning));
            }
        }
    }

    /// 切换到下一个面板
    pub fn switch_panel_next(&mut self) {
        self.focus_manager.next();
    }

    /// 切换到上一个面板
    pub fn switch_panel_prev(&mut self) {
        self.focus_manager.prev();
    }

    /// 切换到指定面板
    pub fn switch_panel_to(&mut self, panel: PanelId) {
        self.focus_manager.jump_to(panel);
    }

    /// 退出应用
    pub fn quit(&mut self) {
        self.state.quit();
    }

    /// 查找面板索引
    fn panel_index(&self, id: PanelId) -> Option<usize> {
        self.panels.iter().position(|p| p.id() == id)
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

        // 弹窗激活时:优先处理弹窗级交互
        if !self.state.popup_stack.is_empty() {
            self.handle_popup_key(key);
            return;
        }

        // 命令/搜索模式:委托给命令面板
        if self.state.input_mode != InputMode::Normal {
            if let Some(cmd) = self.command_palette.handle_key(key, &mut self.state) {
                self.apply_command(cmd);
            }
            return;
        }

        // 普通模式:全局导航快捷键
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.quit(),
            KeyCode::Tab => self.switch_panel_next(),
            KeyCode::BackTab => self.switch_panel_prev(),
            KeyCode::Char('1') => self.switch_panel_to(PanelId::Quest),
            KeyCode::Char('2') => self.switch_panel_to(PanelId::Parliament),
            KeyCode::Char('3') => self.switch_panel_to(PanelId::Budget),
            KeyCode::Char('4') => self.switch_panel_to(PanelId::Memory),
            KeyCode::Char('5') => self.switch_panel_to(PanelId::Security),
            KeyCode::Char('6') => self.switch_panel_to(PanelId::Health),
            KeyCode::Char('7') => self.switch_panel_to(PanelId::Log),
            KeyCode::Char('8') => self.switch_panel_to(PanelId::Help),
            // P2 TUI v1.7-omega:数字键 9 跳转到 Decay 面板(P0 Note 第 1 节)
            // WHY 数字键仅映射前 9 个面板:超过 9 的面板(EventStream/Router/
            // McpNodes/Chtc/Timeline)由 P3.3 的 `g` 前缀 + 数字键映射。
            KeyCode::Char('9') => self.switch_panel_to(PanelId::Decay),
            KeyCode::Char(':') => {
                self.state.input_mode = InputMode::Command;
                self.state.input_buffer.clear();
            }
            KeyCode::Char('/') => {
                self.state.input_mode = InputMode::Search;
                self.state.input_buffer.clear();
            }
            KeyCode::F(1) => self.switch_panel_to(PanelId::Quest),
            KeyCode::F(2) => self.switch_panel_to(PanelId::Parliament),
            KeyCode::F(3) => self.switch_panel_to(PanelId::Budget),
            KeyCode::F(6) => self.switch_panel_to(PanelId::Memory),
            KeyCode::F(7) => self.switch_panel_to(PanelId::Security),
            KeyCode::F(8) => self.switch_panel_to(PanelId::Health),
            KeyCode::Up | KeyCode::Down if key.modifiers.contains(event::KeyModifiers::CONTROL) => {
                self.adjust_main_panel_ratio(key.code == KeyCode::Up);
            }
            _ => {
                // 其他按键委托给当前焦点面板
                let focused = self.focus_manager.focused();
                if let Some(idx) = self.panel_index(focused) {
                    if let Some(cmd) = self.panels[idx].handle_key(key, &mut self.state) {
                        self.apply_command(cmd);
                    }
                }
            }
        }
    }

    /// 处理弹窗激活时的键盘事件
    fn handle_popup_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.state.popup_stack.pop();
            }
            KeyCode::Enter => {
                // 确认弹窗且选中 Yes 时执行关联命令
                if let Some(PopupKind::Confirm {
                    on_confirm,
                    confirmed,
                    ..
                }) = self.state.popup_stack.current()
                {
                    if *confirmed {
                        let cmd = on_confirm.clone();
                        self.state.popup_stack.pop();
                        self.apply_confirm_command(&cmd);
                    } else {
                        self.state.popup_stack.pop();
                    }
                } else {
                    self.state.popup_stack.pop();
                }
            }
            KeyCode::Up => {
                self.state.popup_stack.scroll_current(-1);
            }
            KeyCode::Down => {
                self.state.popup_stack.scroll_current(1);
            }
            KeyCode::Left | KeyCode::Right => {
                self.state.popup_stack.toggle_confirm();
            }
            _ => {}
        }
    }

    /// 根据确认弹窗的命令字符串执行动作
    fn apply_confirm_command(&mut self, cmd: &str) {
        if cmd == "quit" {
            self.quit();
        } else if let Some(quest_id) = cmd.strip_prefix("pause:") {
            self.publish_pause(quest_id);
        } else if let Some(quest_id) = cmd.strip_prefix("resume:") {
            self.publish_resume(quest_id);
        } else if let Some(rest) = cmd.strip_prefix("vote:") {
            let mut parts = rest.splitn(2, ':');
            let vote_str = parts.next().unwrap_or("");
            let proposal_id = parts.next().unwrap_or("");
            if let Some(vote) = parse_vote_value(vote_str) {
                self.publish_vote(proposal_id, vote);
            } else {
                self.state.set_status(
                    format!("invalid vote in confirm command: {cmd}"),
                    Severity::Error,
                );
            }
        }
    }

    /// 发布 Quest 暂停请求
    fn publish_pause(&mut self, quest_id: &str) {
        self.publish_control_event(NexusEvent::QuestPauseRequested {
            metadata: EventMetadata::new("chimera-tui"),
            quest_id: quest_id.to_string(),
            requested_by: "operator".to_string(),
        });
    }

    /// 发布 Quest 恢复请求
    fn publish_resume(&mut self, quest_id: &str) {
        self.publish_control_event(NexusEvent::QuestResumeRequested {
            metadata: EventMetadata::new("chimera-tui"),
            quest_id: quest_id.to_string(),
            requested_by: "operator".to_string(),
        });
    }

    /// 发布投票请求
    fn publish_vote(&mut self, proposal_id: &str, vote: VoteValue) {
        self.publish_control_event(NexusEvent::VoteCastRequested {
            metadata: EventMetadata::new("chimera-tui"),
            proposal_id: proposal_id.to_string(),
            voter: "operator".to_string(),
            vote,
        });
    }

    /// 发布状态刷新请求
    fn publish_refresh(&mut self) {
        self.publish_control_event(NexusEvent::RefreshStateRequested {
            metadata: EventMetadata::new("chimera-tui"),
            requested_by: "operator".to_string(),
        });
    }

    /// 通用控制事件发布,处理 EventBus 不可用或发布失败
    ///
    /// WHY:所有 M4 控制请求走同一入口,统一设置状态栏反馈,
    /// 避免每个命令重复 error/success 处理逻辑。
    fn publish_control_event(&mut self, event: NexusEvent) {
        let type_name = event.type_name();
        match &self.event_bus {
            Some(bus) => match bus.publish_blocking(event) {
                Ok(()) => {
                    let msg = format!("{type_name} request published");
                    self.state.set_status(msg, Severity::Info);
                }
                Err(e) => {
                    self.state.set_status(
                        format!("failed to publish {type_name}: {e}"),
                        Severity::Error,
                    );
                }
            },
            None => {
                self.state
                    .set_status("event bus not available", Severity::Error);
            }
        }
    }

    /// 调整主面板比例
    ///
    /// `increase` 为 true 时增大比例,否则减小。限制在 [RATIO_MIN, RATIO_MAX]。
    fn adjust_main_panel_ratio(&mut self, increase: bool) {
        let delta = if increase { RATIO_STEP } else { -RATIO_STEP };
        self.main_panel_ratio = (self.main_panel_ratio + delta).clamp(RATIO_MIN, RATIO_MAX);
    }

    /// 执行高层命令
    fn apply_command(&mut self, cmd: TuiCommand) {
        match cmd {
            TuiCommand::Quit => self.quit(),
            TuiCommand::SwitchPanel(id) => self.switch_panel_to(id),
            TuiCommand::ShowHelp => self.switch_panel_to(PanelId::Help),
            TuiCommand::OpenPopup(kind) => self.state.popup_stack.push(kind),
            // M4:破坏性控制命令先弹出确认框,由操作员二次确认后再发布事件
            TuiCommand::RequestQuestPause(quest_id) => {
                self.state.popup_stack.push(PopupKind::control_confirm(
                    "Pause quest",
                    &quest_id,
                    format!("pause:{quest_id}"),
                ));
            }
            TuiCommand::RequestQuestResume(quest_id) => {
                self.state.popup_stack.push(PopupKind::control_confirm(
                    "Resume quest",
                    &quest_id,
                    format!("resume:{quest_id}"),
                ));
            }
            TuiCommand::RequestVote { proposal_id, vote } => {
                let vote_str = vote.as_str();
                self.state.popup_stack.push(PopupKind::control_confirm(
                    &format!("Vote {vote_str} on proposal"),
                    &proposal_id,
                    format!("vote:{vote_str}:{proposal_id}"),
                ));
            }
            // M4:非破坏性刷新直接发布事件
            TuiCommand::RequestRefresh => self.publish_refresh(),
        }
    }

    /// 渲染 UI 到 Frame
    ///
    /// WHY 接收 &mut Frame:与 ratatui 的 draw 闭包签名对齐,
    /// 支持 TestBackend 内存渲染测试(无需真实终端)。
    ///
    /// # 布局
    /// - 顶部:面板标签栏(1 行,含边框)
    /// - 中部:主面板(占 `main_panel_ratio`)
    /// - 底部:命令面板(激活时)或状态栏(普通模式)
    /// - 最上层:弹窗叠加
    pub fn render(&mut self, frame: &mut Frame<'_>) {
        let area = frame.area();
        self.last_area = area;
        let chunks = self.layout(area);

        self.render_tabs(frame, chunks[0]);
        self.render_main_panel(frame, chunks[1]);

        if self.state.input_mode != InputMode::Normal {
            self.command_palette
                .render(&self.state, chunks[2], frame.buffer_mut());
        } else {
            self.render_status_bar(frame, chunks[2]);
        }

        // 弹窗叠加在最上层
        if !self.state.popup_stack.is_empty() {
            self.state.popup_stack.render(area, frame.buffer_mut());
        }
    }

    /// 计算当前布局,返回 [tabs, main, bottom] 三个区域
    ///
    /// WHY 独立方法:事件处理中需要知道各区域位置以响应鼠标点击,
    /// 与渲染复用同一套布局逻辑。
    fn layout(&self, area: Rect) -> [Rect; 3] {
        let tab_and_rest = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(3), Constraint::Min(0)])
            .split(area);

        let main_and_bottom = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage((self.main_panel_ratio * 100.0) as u16),
                Constraint::Min(3),
            ])
            .split(tab_and_rest[1]);

        [tab_and_rest[0], main_and_bottom[0], main_and_bottom[1]]
    }

    /// 渲染面板标签栏
    fn render_tabs(&self, frame: &mut Frame<'_>, area: ratatui::layout::Rect) {
        let titles: Vec<Line> = self
            .focus_manager
            .panels()
            .iter()
            .map(|&p| Line::from(format!(" {} ", p.as_str())))
            .collect();

        let focused = self.focus_manager.focused();
        let selected = self
            .focus_manager
            .panels()
            .iter()
            .position(|&p| p == focused)
            .unwrap_or(0);

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
    fn render_main_panel(&mut self, frame: &mut Frame<'_>, area: ratatui::layout::Rect) {
        let focused = self.focus_manager.focused();
        let focused_idx = self.panel_index(focused);

        // M1 清理项 #5:仅当焦点面板变化时才调用 focus 回调。
        if self.last_focused != Some(focused) {
            if let Some(idx) = focused_idx {
                self.panels[idx].focus(true);
            }
            for (i, panel) in self.panels.iter_mut().enumerate() {
                if Some(i) != focused_idx {
                    panel.focus(false);
                }
            }
            self.last_focused = Some(focused);
        }

        if let Some(idx) = focused_idx {
            self.panels[idx].render(&self.state, area, frame.buffer_mut());
        }
    }

    /// 渲染状态栏
    fn render_status_bar(&self, frame: &mut Frame<'_>, area: ratatui::layout::Rect) {
        let (status, fg) = match &self.state.status_message {
            Some((msg, severity)) => (
                format!(
                    " Panel: {} | Running: {} | Frame: {} | {} ",
                    self.current_panel().as_str(),
                    self.state.running,
                    self.state.frame_count,
                    msg
                ),
                severity.color(),
            ),
            None => (
                format!(
                    " Panel: {} | Running: {} | Frame: {} | Ratio: {:.0}% ",
                    self.current_panel().as_str(),
                    self.state.running,
                    self.state.frame_count,
                    self.main_panel_ratio * 100.0
                ),
                Color::Black,
            ),
        };

        let span = Span::styled(
            status,
            Style::default()
                .fg(fg)
                .bg(self.theme_accent())
                .add_modifier(Modifier::BOLD),
        );
        let line = Line::from(span);
        let paragraph = Paragraph::new(line);
        frame.render_widget(paragraph, area);
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

        // M3:按配置启用鼠标捕获
        if self.config.enable_mouse {
            execute!(stdout, event::EnableMouseCapture)
                .map_err(|e| TuiError::TerminalInit(e.to_string()))?;
        }

        // 步骤 2:创建终端
        let backend = CrosstermBackend::new(stdout);
        let mut terminal =
            Terminal::new(backend).map_err(|e| TuiError::TerminalInit(e.to_string()))?;

        // 步骤 3:事件循环
        // WHY 用 result 变量:确保终端恢复在 return 前执行,即使事件循环出错
        let result = self.event_loop(&mut terminal);

        // 步骤 4:恢复终端(无论事件循环成功与否)
        // WHY 恢复在 result 返回前:确保终端状态不残留,即使出错也要恢复
        let stdout = terminal.backend_mut();
        if self.config.enable_mouse {
            let _ = execute!(stdout, event::DisableMouseCapture);
        }
        disable_raw_mode().map_err(|e| TuiError::TerminalRestore(e.to_string()))?;
        execute!(stdout, LeaveAlternateScreen)
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
            // 在渲染前从数据源刷新状态，确保面板显示最新快照。
            // 数据源实现内部处理去重与缓存，此调用为 O(1) 非阻塞。
            self.update();

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
            match event {
                Event::Key(key) => self.handle_key_event(key),
                Event::Mouse(mouse) => self.handle_mouse_event(mouse),
                _ => {}
            }
        }
        Ok(())
    }

    /// 处理鼠标事件
    ///
    /// M3 实现:标签栏切换、命令栏聚焦、弹窗/面板滚轮滚动。
    pub fn handle_mouse_event(&mut self, mouse: MouseEvent) {
        let area = self.last_area;
        if area.width == 0 || area.height == 0 {
            return;
        }
        let chunks = self.layout(area);

        match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => {
                if is_inside(mouse.column, mouse.row, chunks[0]) {
                    self.handle_tab_click(mouse.column, chunks[0].width);
                } else if is_inside(mouse.column, mouse.row, chunks[2]) {
                    self.state.input_mode = InputMode::Command;
                    self.state.input_buffer.clear();
                }
                // 主面板点击已在焦点上,无需额外处理
            }
            MouseEventKind::ScrollUp | MouseEventKind::ScrollDown => {
                if !self.state.popup_stack.is_empty() {
                    let delta = if mouse.kind == MouseEventKind::ScrollUp {
                        -1
                    } else {
                        1
                    };
                    self.state.popup_stack.scroll_current(delta);
                } else if is_inside(mouse.column, mouse.row, chunks[1]) {
                    let focused = self.focus_manager.focused();
                    if let Some(idx) = self.panel_index(focused) {
                        if let Some(cmd) = self.panels[idx].handle_mouse(mouse, &mut self.state) {
                            self.apply_command(cmd);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// 处理标签栏点击,切换到对应面板
    fn handle_tab_click(&mut self, column: u16, tab_area_width: u16) {
        let panel_count = self.focus_manager.panels().len() as u16;
        if panel_count == 0 || tab_area_width == 0 {
            return;
        }
        let tab_width = tab_area_width / panel_count;
        let index = (column / tab_width) as usize;
        if let Some(&panel) = self.focus_manager.panels().get(index) {
            self.switch_panel_to(panel);
        }
    }
}

/// 判断坐标是否落在指定区域内
fn is_inside(column: u16, row: u16, area: Rect) -> bool {
    column >= area.x && column < area.x + area.width && row >= area.y && row < area.y + area.height
}

/// 将 vote 字符串解析为 VoteValue
///
/// WHY:确认弹窗的 `on_confirm` 只能传递字符串,解码时需要与
/// CommandPalette 编码时使用的 `yes|no|abstain` 保持一致。
/// 委托给 `VoteValue::from_str` 以保证唯一真实来源。
fn parse_vote_value(s: &str) -> Option<VoteValue> {
    s.parse().ok()
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;
    use crate::data::{BudgetMetrics, DataSnapshot, DataSourceConfig, TuiDataSource};
    use crate::popup::PopupKind;
    use event_bus::{EventMetadata, NexusEvent};
    use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};
    use ratatui::backend::TestBackend;

    fn make_app() -> TuiApp {
        TuiApp::new(TuiConfig::default()).unwrap()
    }

    /// 构造一个简单 Quest，用于数据驱动面板测试
    fn sample_quest(id: &str, title: &str) -> Quest {
        Quest {
            quest_id: id.into(),
            title: title.into(),
            tasks: vec![Task {
                task_id: format!("{id}-t1"),
                description: "test task".into(),
                status: TaskStatus::Pending,
                dependencies: vec![],
            }],
            thinking_mode: ThinkingMode::Standard,
            checkpoint_id: None,
        }
    }

    /// 测试替身数据源 — 返回预设快照
    #[derive(Debug)]
    struct MockDataSource {
        snapshot: DataSnapshot,
        config: DataSourceConfig,
    }

    impl MockDataSource {
        fn new(snapshot: DataSnapshot) -> Self {
            Self {
                snapshot,
                config: DataSourceConfig::default(),
            }
        }
    }

    impl TuiDataSource for MockDataSource {
        fn snapshot(&self) -> Result<DataSnapshot, TuiError> {
            Ok(self.snapshot.clone())
        }

        fn config(&self) -> &DataSourceConfig {
            &self.config
        }
    }

    // ============================================================
    // 应用初始化测试
    // ============================================================

    #[test]
    fn test_app_new() {
        let app = make_app();
        assert_eq!(app.current_panel(), PanelId::Quest);
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
        assert_eq!(app.current_panel(), PanelId::Quest);
        app.switch_panel_next();
        assert_eq!(app.current_panel(), PanelId::Parliament);
        app.switch_panel_next();
        assert_eq!(app.current_panel(), PanelId::Budget);
        app.switch_panel_next();
        assert_eq!(app.current_panel(), PanelId::Memory);
    }

    #[test]
    fn test_switch_panel_prev() {
        let mut app = make_app();
        app.switch_panel_prev();
        // P2 TUI v1.7-omega:FocusManager 现注册 13 面板(8 原始 + 5 新增,
        // 不含 Timeline),Quest.prev() 跳到列表末尾的 Chtc 面板。
        assert_eq!(app.current_panel(), PanelId::Chtc);
    }

    #[test]
    fn test_switch_panel_to() {
        let mut app = make_app();
        app.switch_panel_to(PanelId::Budget);
        assert_eq!(app.current_panel(), PanelId::Budget);
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
        assert_eq!(app.current_panel(), PanelId::Parliament);
    }

    #[test]
    fn test_handle_key_number_jumps_to_panel() {
        let mut app = make_app();
        app.handle_key_event(KeyEvent::new(KeyCode::Char('3'), event::KeyModifiers::NONE));
        assert_eq!(app.current_panel(), PanelId::Budget);
    }

    #[test]
    fn test_handle_key_new_panels() {
        let mut app = make_app();
        app.handle_key_event(KeyEvent::new(KeyCode::Char('4'), event::KeyModifiers::NONE));
        assert_eq!(app.current_panel(), PanelId::Memory);

        app.handle_key_event(KeyEvent::new(KeyCode::Char('5'), event::KeyModifiers::NONE));
        assert_eq!(app.current_panel(), PanelId::Security);

        app.handle_key_event(KeyEvent::new(KeyCode::Char('6'), event::KeyModifiers::NONE));
        assert_eq!(app.current_panel(), PanelId::Health);
    }

    #[test]
    fn test_handle_key_9_jumps_to_decay() {
        // P2 TUI v1.7-omega:数字键 9 跳转到 Decay 面板(P0 Note 第 1 节)
        let mut app = make_app();
        app.handle_key_event(KeyEvent::new(KeyCode::Char('9'), event::KeyModifiers::NONE));
        assert_eq!(app.current_panel(), PanelId::Decay);
    }

    #[test]
    fn test_handle_key_f_keys_jump_to_panel() {
        let mut app = make_app();
        app.handle_key_event(KeyEvent::new(KeyCode::F(2), event::KeyModifiers::NONE));
        assert_eq!(app.current_panel(), PanelId::Parliament);
    }

    #[test]
    fn test_handle_key_f_keys_new_panels() {
        let mut app = make_app();

        app.handle_key_event(KeyEvent::new(KeyCode::F(6), event::KeyModifiers::NONE));
        assert_eq!(app.current_panel(), PanelId::Memory);

        app.handle_key_event(KeyEvent::new(KeyCode::F(7), event::KeyModifiers::NONE));
        assert_eq!(app.current_panel(), PanelId::Security);

        app.handle_key_event(KeyEvent::new(KeyCode::F(8), event::KeyModifiers::NONE));
        assert_eq!(app.current_panel(), PanelId::Health);
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
    fn test_handle_key_command_mode() {
        let mut app = make_app();
        app.handle_key_event(KeyEvent::new(KeyCode::Char(':'), event::KeyModifiers::NONE));
        assert_eq!(app.state().input_mode, InputMode::Command);

        // 输入命令
        for c in "budget".chars() {
            app.handle_key_event(KeyEvent::new(KeyCode::Char(c), event::KeyModifiers::NONE));
        }
        assert_eq!(app.state().input_buffer, "budget");

        // 提交
        app.handle_key_event(KeyEvent::new(KeyCode::Enter, event::KeyModifiers::NONE));
        assert_eq!(app.current_panel(), PanelId::Budget);
        assert_eq!(app.state().input_mode, InputMode::Normal);
    }

    #[test]
    fn test_handle_key_search_mode_sets_filter() {
        let mut app = make_app();
        app.handle_key_event(KeyEvent::new(KeyCode::Char('/'), event::KeyModifiers::NONE));
        assert_eq!(app.state().input_mode, InputMode::Search);

        for c in "Error".chars() {
            app.handle_key_event(KeyEvent::new(KeyCode::Char(c), event::KeyModifiers::NONE));
        }
        assert_eq!(app.state().input_buffer, "Error");

        app.handle_key_event(KeyEvent::new(KeyCode::Enter, event::KeyModifiers::NONE));
        assert_eq!(app.state().input_mode, InputMode::Normal);
        assert_eq!(app.state().filter_keyword, Some("error".into()));
    }

    #[test]
    fn test_handle_key_esc_cancels_command_mode() {
        let mut app = make_app();
        app.handle_key_event(KeyEvent::new(KeyCode::Char(':'), event::KeyModifiers::NONE));
        for c in "quit".chars() {
            app.handle_key_event(KeyEvent::new(KeyCode::Char(c), event::KeyModifiers::NONE));
        }
        app.handle_key_event(KeyEvent::new(KeyCode::Esc, event::KeyModifiers::NONE));
        assert_eq!(app.state().input_mode, InputMode::Normal);
        assert!(app.state().input_buffer.is_empty());
        assert!(app.state().running);
    }

    #[test]
    fn test_handle_key_question_mark_shows_help() {
        let mut app = make_app();
        app.handle_key_event(KeyEvent::new(KeyCode::Char('?'), event::KeyModifiers::NONE));
        assert_eq!(app.current_panel(), PanelId::Help);
    }

    #[test]
    fn test_handle_key_ctrl_up_increases_ratio() {
        let mut app = make_app();
        let before = app.main_panel_ratio;
        app.handle_key_event(KeyEvent::new(KeyCode::Up, event::KeyModifiers::CONTROL));
        assert!(app.main_panel_ratio > before);
    }

    #[test]
    fn test_handle_key_ctrl_down_decreases_ratio() {
        let mut app = make_app();
        let before = app.main_panel_ratio;
        app.handle_key_event(KeyEvent::new(KeyCode::Down, event::KeyModifiers::CONTROL));
        assert!(app.main_panel_ratio < before);
    }

    #[test]
    fn test_main_panel_ratio_bounds() {
        let mut app = make_app();
        for _ in 0..100 {
            app.adjust_main_panel_ratio(true);
        }
        assert!((app.main_panel_ratio - RATIO_MAX).abs() < f32::EPSILON);

        for _ in 0..100 {
            app.adjust_main_panel_ratio(false);
        }
        assert!((app.main_panel_ratio - RATIO_MIN).abs() < f32::EPSILON);
    }

    // ============================================================
    // 弹窗测试
    // ============================================================

    #[test]
    fn test_popup_esc_closes() {
        let mut app = make_app();
        app.state.popup_stack.push(PopupKind::Notification {
            message: "test".into(),
            severity: crate::popup::Severity::Info,
        });
        assert!(!app.state.popup_stack.is_empty());

        app.handle_key_event(KeyEvent::new(KeyCode::Esc, event::KeyModifiers::NONE));
        assert!(app.state.popup_stack.is_empty());
    }

    #[test]
    fn test_detail_popup_scroll() {
        let mut app = make_app();
        app.state.popup_stack.push(PopupKind::Detail {
            title: "Detail".into(),
            content: "line1\nline2\nline3".into(),
            scroll: 0,
        });

        app.handle_key_event(KeyEvent::new(KeyCode::Down, event::KeyModifiers::NONE));
        assert_eq!(
            app.state.popup_stack.current().unwrap().detail_scroll(),
            Some(1)
        );
    }

    #[test]
    fn test_confirm_popup_yes_quits() {
        let mut app = make_app();
        app.state.popup_stack.push(PopupKind::Confirm {
            prompt: "Quit?".into(),
            on_confirm: "quit".into(),
            confirmed: true,
        });

        app.handle_key_event(KeyEvent::new(KeyCode::Enter, event::KeyModifiers::NONE));
        assert!(app.state.popup_stack.is_empty());
        assert!(!app.state.running);
    }

    #[test]
    fn test_confirm_popup_no_dismisses() {
        let mut app = make_app();
        app.state.popup_stack.push(PopupKind::Confirm {
            prompt: "Quit?".into(),
            on_confirm: "quit".into(),
            confirmed: false,
        });

        app.handle_key_event(KeyEvent::new(KeyCode::Enter, event::KeyModifiers::NONE));
        assert!(app.state.popup_stack.is_empty());
        assert!(app.state.running);
    }

    // ============================================================
    // 渲染测试(使用 TestBackend,无需真实终端)
    // ============================================================

    #[test]
    fn test_render_produces_output() {
        let mut app = make_app();
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
    fn test_render_memory_panel() {
        let mut app = make_app();
        app.switch_panel_to(PanelId::Memory);

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
            content.contains("Memory") || content.contains("Cache Hit Rate"),
            "rendered output should contain Memory panel"
        );
    }

    #[test]
    fn test_render_security_panel() {
        let mut app = make_app();
        app.switch_panel_to(PanelId::Security);

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
            content.contains("Security") || content.contains("VETO"),
            "rendered output should contain Security panel"
        );
    }

    #[test]
    fn test_render_health_panel() {
        let mut app = make_app();
        app.switch_panel_to(PanelId::Health);

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
            content.contains("Health") || content.contains("Events/sec"),
            "rendered output should contain Health panel"
        );
    }

    #[test]
    fn test_render_help_panel() {
        let mut app = make_app();
        app.switch_panel_to(PanelId::Help);

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
    // 数据接入测试
    // ============================================================

    #[test]
    fn test_with_data_source_accepts_custom_source() {
        let app = TuiApp::with_data_source(
            TuiConfig::default(),
            Box::new(MockDataSource::new(DataSnapshot::default())),
        )
        .unwrap();
        assert!(app.state().quest_list.is_empty());
        assert_eq!(app.state().budget.current_tier, "High");
    }

    #[test]
    fn test_update_pulls_snapshot_into_state() {
        let snapshot = DataSnapshot {
            quest_list: vec![sample_quest("q1", "Data Driven Quest")],
            budget_metrics: BudgetMetrics {
                current_tier: "Critical".into(),
                utilization_rate: 0.95,
                ..Default::default()
            },
            latest_events: VecDeque::from([NexusEvent::CacheHit {
                metadata: EventMetadata::new("test"),
                cache_key: "k1".into(),
            }]),
            ..Default::default()
        };

        let mut app = TuiApp::with_data_source(
            TuiConfig::default(),
            Box::new(MockDataSource::new(snapshot)),
        )
        .unwrap();
        app.update();

        assert_eq!(app.state().quest_list.len(), 1);
        assert_eq!(app.state().quest_list[0].title, "Data Driven Quest");
        assert_eq!(app.state().budget.current_tier, "Critical");
        assert_eq!(app.state().latest_events.len(), 1);
    }

    #[test]
    fn test_update_sets_status_message_on_error() {
        /// 总是返回错误的数据源
        #[derive(Debug)]
        struct FailingDataSource;

        impl TuiDataSource for FailingDataSource {
            fn snapshot(&self) -> Result<DataSnapshot, TuiError> {
                Err(TuiError::DataSource("forced failure".into()))
            }

            fn config(&self) -> &DataSourceConfig {
                static CONFIG: std::sync::OnceLock<DataSourceConfig> = std::sync::OnceLock::new();
                CONFIG.get_or_init(DataSourceConfig::default)
            }
        }

        let mut app =
            TuiApp::with_data_source(TuiConfig::default(), Box::new(FailingDataSource)).unwrap();
        app.update();

        assert!(
            app.state().status_message.is_some(),
            "data source failure should set status message"
        );
        let (msg, severity) = app.state().status_message.as_ref().unwrap();
        assert!(msg.contains("forced failure"));
        assert_eq!(*severity, Severity::Warning);
    }

    #[test]
    fn test_quest_panel_renders_real_quest_data() {
        let snapshot = DataSnapshot {
            quest_list: vec![
                sample_quest("q1", "First Quest"),
                sample_quest("q2", "Second Quest"),
            ],
            ..Default::default()
        };

        let mut app = TuiApp::with_data_source(
            TuiConfig::default(),
            Box::new(MockDataSource::new(snapshot)),
        )
        .unwrap();
        app.update();

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();

        let buffer = terminal.backend().buffer();
        let content: String = buffer
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(content.contains("First Quest"));
        assert!(content.contains("Second Quest"));
    }

    #[test]
    fn test_budget_panel_content_uses_state() {
        let snapshot = DataSnapshot {
            budget_metrics: BudgetMetrics {
                total_consumption: 800.0,
                remaining_budget: 200.0,
                utilization_rate: 0.8,
                current_tier: "Medium".into(),
                coefficient: 0.8,
                is_exceeded: false,
                alert: None,
            },
            ..Default::default()
        };

        let mut app = TuiApp::with_data_source(
            TuiConfig::default(),
            Box::new(MockDataSource::new(snapshot)),
        )
        .unwrap();
        app.update();
        app.switch_panel_to(PanelId::Budget);

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();

        let buffer = terminal.backend().buffer();
        let content: String = buffer
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(content.contains("Medium"));
        assert!(content.contains("800.0"));
        assert!(content.contains("OK"));
    }

    #[test]
    fn test_log_panel_content_uses_state() {
        let snapshot = DataSnapshot {
            latest_events: VecDeque::from([NexusEvent::CacheHit {
                metadata: EventMetadata::new("scc-cache"),
                cache_key: "k1".into(),
            }]),
            ..Default::default()
        };

        let mut app = TuiApp::with_data_source(
            TuiConfig::default(),
            Box::new(MockDataSource::new(snapshot)),
        )
        .unwrap();
        app.update();
        app.switch_panel_to(PanelId::Log);

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();

        let buffer = terminal.backend().buffer();
        let content: String = buffer
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(content.contains("System Log"));
        assert!(content.contains("CacheHit"));
    }

    // ============================================================
    // 鼠标事件测试
    // ============================================================

    #[test]
    fn test_mouse_scroll_in_main_panel() {
        let mut app = make_app();
        app.switch_panel_to(PanelId::Log);
        let state = app.state_mut();
        state.latest_events = VecDeque::from([
            NexusEvent::CacheHit {
                metadata: EventMetadata::new("scc-cache"),
                cache_key: "k1".into(),
            },
            NexusEvent::CacheMiss {
                metadata: EventMetadata::new("scc-cache"),
                cache_key: "k2".into(),
            },
        ]);

        // 先渲染以设置 last_area
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();

        // 在主面板区域(80x24 默认布局)滚动
        app.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 10,
            row: 10,
            modifiers: event::KeyModifiers::NONE,
        });

        // 滚动 Down 在 Log 面板中选择下一条事件
        // 由于 selected 初始为 0,ScrollDown 应使其变为 1
        // 但面板状态无法直接从 app 访问,这里只验证不 panic
    }

    #[test]
    fn test_mouse_tab_click_switches_panel() {
        let mut app = make_app();
        // 先渲染以设置 last_area
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();

        // P2 TUI v1.7-omega:标签栏宽度 80,13 个面板(8 原始 + 5 新增),
        // 每个标签约 6 列。点击第 2 个标签(Parliament)需落在 column 6-11 范围内。
        // WHY column=8:避开标签边界(6/12),确保命中 Parliament 标签内部。
        app.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 8,
            row: 1,
            modifiers: event::KeyModifiers::NONE,
        });
        assert_eq!(app.current_panel(), PanelId::Parliament);
    }

    #[test]
    fn test_mouse_command_bar_click_focuses() {
        let mut app = make_app();
        // 先渲染以设置 last_area
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();

        app.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 10,
            row: 20,
            modifiers: event::KeyModifiers::NONE,
        });
        assert_eq!(app.state().input_mode, InputMode::Command);
    }
}
