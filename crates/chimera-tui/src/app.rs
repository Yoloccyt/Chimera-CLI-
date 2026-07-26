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
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Tabs};
use ratatui::{Frame, Terminal};
use std::collections::VecDeque;
use std::io::{self, Stdout};
use std::time::{Duration, Instant};

use crate::command_palette::{CommandPalette, CommandPaletteModel};
use crate::config::Theme;
use crate::config::TuiConfig;
use crate::data::{DataSnapshot, ExportFormat, StubDataSource, TuiDataSource};
use crate::error::TuiError;
use crate::focus::FocusManager;
use crate::input::{InputRouter, PaneDir, RouteTarget, RouterMode};
use crate::panels::{
    BudgetPanel, ChatPanel, ChtcPanel, ClvVectorPanel, DecayPanel, EventStreamPanel, HealthPanel,
    HelpPanel, LogPanel, McpNodesPanel, MemoryPanel, MetricsDashboardPanel, Panel, ParliamentPanel,
    QuestPanel, ResourceMonitorPanel, RouterPanel, SecurityPanel,
};
use crate::popup::{PopupKind, Severity};
use crate::types::{InputMode, LayoutMode, PanelId, TuiCommand, TuiState};
use event_bus::{ActionSource, EventBus, EventMetadata, NexusEvent, VoteValue};

/// 主面板比例调整步长
const RATIO_STEP: f32 = 0.05;
/// 主面板比例最小值
const RATIO_MIN: f32 = 0.3;
/// 主面板比例最大值
const RATIO_MAX: f32 = 0.9;
/// FPS 移动平均窗口大小(最近 N 帧)
///
/// WHY 60 帧:对应 60fps 下约 1 秒的窗口,既能平滑单帧抖动
/// (避免状态栏数字频繁跳动),又能对真实帧率变化保持灵敏。
const FPS_WINDOW_SIZE: usize = 60;
/// FPS 显示上限,防止瞬时帧(如调试器步进后首帧)产生超大数字撑破状态栏宽度
///
/// WHY 999:三位数可保证 `FPS: <n>` 文本宽度稳定,配合 80 列状态栏约束。
const FPS_DISPLAY_MAX: u16 = 999;
/// 伴随面板宽度(字符),与引擎 Chat 模式 CHAT_CONTEXT_WIDTH 对齐(M2 增量3)
const COMPANION_WIDTH: u16 = 30;
/// 触发伴随面板并排的最小视口宽度(低于此不切分,避免主区被挤压)
const COMPANION_MIN_WIDTH: u16 = 60;
/// IDE 三窗格模式左侧栏宽度(字符),与引擎 presets IDE_SIDEBAR_WIDTH 对齐(M3d)
const IDE_SIDEBAR_WIDTH: u16 = 20;
/// IDE 三窗格模式右侧 context 栏宽度(字符),与引擎 presets IDE_CONTEXT_WIDTH 对齐(M3d)
const IDE_CONTEXT_WIDTH: u16 = 28;

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
    /// 统一命令面板 overlay 状态(M2.2,用户北极星)
    ///
    /// WHY `Option` 表达开关:`Some` = 面板已打开(键盘路由与渲染都据此分流),
    /// `None` = 关闭。模型自持 `ActionRegistry` 副本,复用同一实例避免每次
    /// 打开都重建注册表;候选项经 `codegen::palette_entries` 与斜杠命令/帮助同源。
    palette: Option<CommandPaletteModel>,
    /// 伴随面板可见性(M2 增量3 Stage 1,opt-in,默认关闭)
    ///
    /// WHY 默认关闭:开启时主区右侧并排渲染伴随面板;关闭时 `render_main_panel`
    /// 行为与现状逐字节一致,保证既有 render/layout 测试零回归。
    companion_visible: bool,
    /// 伴随面板目标 = 最近使用的面板(切换焦点时记录切换前的面板)
    prev_panel: Option<PanelId>,
    /// 显式绑定的伴随面板(M2 增量3 Stage 2,None = 回退 Stage1 自动"最近使用")
    ///
    /// WHY 与 prev_panel 分离:`]` 循环绑定写入本字段并优先于自动逻辑;
    /// 未绑定时保持 Stage1 行为,零回归。
    bound_companion: Option<PanelId>,
    /// 活跃窗格索引(M3d 多窗格,0 = 主窗格,默认主区活跃)
    ///
    /// WHY 从 bool 升级为索引:M3d 把"主+单一伴随"2 窗格泛化为 PaneMode 驱动的
    /// 多窗格(Chat 2 / VimSplit 2 / IDE 3),`active_pane` 是 `pane_panels()` 循环序
    /// 的下标——面板级键路由到该窗格面板、渲染时高亮其边框。`w` 键环形递增,
    /// 主区焦点变化 / 窗格数收缩时复位或钳制回 0。2 窗格时 1 = 伴随,语义等价 Stage 2。
    active_pane: usize,
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
    /// 上一帧的渲染时间戳(P4.4 FPS 计算)
    last_frame_time: Instant,
    /// 最近 N 帧的耗时(毫秒),用于 FPS 移动平均(P4.4)
    frame_times: VecDeque<f64>,
    /// 当前 Chat 会话标识(M3b,uuid v7 时间有序,构造时生成)
    ///
    /// WHY TuiApp 持有:Submit 时随 `TuiChatSubmitted` 发布,供 M3c 编排器多轮关联;
    /// 单会话生命周期与 TuiApp 一致(`/clear` 重置留后续)。
    chat_session_id: String,
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
        // P8 TUI v1.7-omega:注册 15 个面板(8 原始 + 7 新增面板)。
        // WHY 不含 Timeline:Timeline 面板由 P7 历史回放引擎(v1.8+)实现,
        // 当前 PanelId::Timeline 已定义但无对应 Panel 实现,故不注册。
        // FocusManager 循环顺序:Quest → Parliament → ... → Chtc → ClvVector
        // → ResourceMonitor → MetricsDashboard → Quest(16 面板循环)。
        // WHY MetricsDashboard 加入主循环:与 ResourceMonitorPanel 同属
        // 监控类展示面板,默认进入主循环便于用户 Tab 键直接访问。
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
            // P8 系统资源监控面板:CLV 向量可视化 + 实时资源指标
            Box::new(ClvVectorPanel::new()),
            Box::new(ResourceMonitorPanel::new()),
            // P9 指标仪表盘面板(Task 2.2):5×2 网格 + 可绑定数据源
            Box::new(MetricsDashboardPanel::new()),
            // M3b:Chat 面板(交互式 Agent 对话);追加到循环末尾,不改现有 Tab 次序
            Box::new(ChatPanel::new()),
        ];
        let panel_ids: Vec<PanelId> = panels.iter().map(|p| p.id()).collect();
        let focus_manager = FocusManager::new(panel_ids);
        let state = if config.persist_state {
            TuiState::load_from_file(&config.state_file_path)
        } else {
            TuiState::new()
        };
        let main_panel_ratio = config.main_panel_ratio;

        Ok(Self {
            config,
            main_panel_ratio,
            state,
            data_source,
            panels,
            focus_manager,
            command_palette: CommandPalette::new(),
            // M2.2:命令面板 overlay 初始关闭,首次 Ctrl+P 时惰性构建模型
            palette: None,
            // M2 增量3:伴随面板默认关闭(opt-in),prev_panel 首次切换面板后填充
            companion_visible: false,
            prev_panel: None,
            // M2 增量3 Stage 2:未绑定伴随;M3d:活跃窗格默认主区(索引 0)
            bound_companion: None,
            active_pane: 0,
            last_focused: None,
            last_area: Rect::default(),
            event_bus: None,
            last_frame_time: Instant::now(),
            frame_times: VecDeque::with_capacity(FPS_WINDOW_SIZE),
            // M3b:会话 id 用 uuid v7(时间有序),整个 TuiApp 生命周期复用
            chat_session_id: uuid::Uuid::now_v7().to_string(),
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
    ///
    /// # P4.1 增量渲染
    /// 在赋值前比较新旧快照中各面板绑定的字段,若发生变化则通过
    /// `TuiState::mark_dirty` 标记对应面板。由于 `PartialEq` 已经在
    /// 各 `*Metrics` / `*State` 上派生,比较为 O(字段大小) 的结构化
    /// 相等比较,不引入额外哈希/序列化开销。
    pub fn update(&mut self) {
        match self.data_source.snapshot() {
            Ok(snapshot) => {
                // P4.1:在覆盖状态前检测哪些面板数据发生变化,先打 dirty 标记
                self.mark_dirty_panels_from_snapshot(&snapshot);

                self.state.quest_list = snapshot.quest_list;
                self.state.paused_quest_count = snapshot.paused_quest_count;
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
                // P8 ResourceMonitor 面板字段同步:DataSnapshot → TuiState
                // M3 monitor.pause_sampling:暂停时跳过覆盖,保留冻结快照供检视(UI 本地冻结)
                if !self.state.monitor_paused {
                    self.state.sys_metrics = snapshot.sys_metrics.clone();
                    self.state.sys_metrics_history = snapshot.sys_metrics_history.clone();
                }
                // Task 6:同步 tick 模式,供状态栏展示
                self.state.tick_mode = snapshot.tick_mode;
                // M3b:同步对话历史与状态到 TuiState(供 Chat 面板渲染)
                self.state.chat_messages = snapshot.chat_messages;
                self.state.chat_status = snapshot.chat_status;
                // P0 交互链:新 Action 终态反馈(seq 递增)时上屏 status_message,
                // 比对 seq 只上屏一次;错误用 Error 级,成功用 Info 级。
                if snapshot.action_feedback_seq > self.state.last_action_feedback_seq {
                    if let Some((msg, is_error)) = &snapshot.action_feedback {
                        let severity = if *is_error {
                            Severity::Error
                        } else {
                            Severity::Info
                        };
                        self.state.status_message = Some((msg.clone(), severity));
                    }
                    self.state.last_action_feedback_seq = snapshot.action_feedback_seq;
                }
                // P1-W2.2:同步 Critical 旁路通道丢弃计数(EventStream 面板告警显示)
                self.state.critical_event_dropped_count = snapshot.critical_event_dropped_count;
            }
            Err(e) => {
                // M1 清理项 #4:数据源失败时向用户展示状态栏警告,而非静默忽略。
                self.state.status_message =
                    Some((format!("data source unavailable: {e}"), Severity::Warning));
            }
        }
    }

    /// 比较当前 `TuiState` 与新 `DataSnapshot` 中各面板绑定的字段,
    /// 对发生变化的字段调用 `mark_dirty`。
    ///
    /// WHY 独立方法:集中维护"字段 → PanelId"映射,避免 `update` 方法
    /// 臃肿;同时便于测试针对单个字段的变化进行断言。
    ///
    /// # 字段 → 面板映射
    /// - `quest_list` → Quest + Health(Active Quests 从 quest_list.len() 派生)
    /// - `paused_quest_count` → Health(Paused Quests 指标)
    /// - `budget_metrics` / `budget_history` → Budget
    /// - `memory_metrics` / `memory_history` → Memory
    /// - `security_state` → Security
    /// - `health_metrics` / `event_rate_history` → Health
    /// - `latest_events` → Parliament + Log + EventStream(三者共享事件流)
    /// - `decay_metrics` / `decay_history` → Decay
    /// - `router_metrics` → Router
    /// - `mcp_nodes` → McpNodes
    /// - `chtc_state` → Chtc
    fn mark_dirty_panels_from_snapshot(&mut self, snapshot: &DataSnapshot) {
        // WHY 使用 `!=` 而非哈希比较:所有 metrics 类型都已 `PartialEq`,
        // 结构化比较更易读,且无需额外引入哈希依赖。
        if self.state.quest_list != snapshot.quest_list {
            self.state.mark_dirty(PanelId::Quest);
            // quest_list 变化也影响 Health 面板的 Active Quests 指标
            self.state.mark_dirty(PanelId::Health);
        }
        if self.state.budget != snapshot.budget_metrics
            || self.state.budget_history != snapshot.budget_history
        {
            self.state.mark_dirty(PanelId::Budget);
        }
        if self.state.memory_metrics != snapshot.memory_metrics
            || self.state.memory_history != snapshot.memory_history
        {
            self.state.mark_dirty(PanelId::Memory);
        }
        if self.state.security_state != snapshot.security_state {
            self.state.mark_dirty(PanelId::Security);
        }
        if self.state.health_metrics != snapshot.health_metrics
            || self.state.event_rate_history != snapshot.event_rate_history
            || self.state.paused_quest_count != snapshot.paused_quest_count
        {
            self.state.mark_dirty(PanelId::Health);
        }
        // WHY latest_events 同时驱动 Parliament / Log / EventStream 三面板,
        // 任一变化都需标记这三个面板,避免事件流面板错过新事件。
        if self.state.latest_events != snapshot.latest_events {
            self.state.mark_dirty(PanelId::Parliament);
            self.state.mark_dirty(PanelId::Log);
            self.state.mark_dirty(PanelId::EventStream);
        }
        if self.state.decay_metrics != snapshot.decay_metrics
            || self.state.decay_history != snapshot.decay_history
        {
            self.state.mark_dirty(PanelId::Decay);
        }
        if self.state.router_metrics != snapshot.router_metrics {
            self.state.mark_dirty(PanelId::Router);
        }
        if self.state.mcp_nodes != snapshot.mcp_nodes {
            self.state.mark_dirty(PanelId::McpNodes);
        }
        if self.state.chtc_state != snapshot.chtc_state {
            self.state.mark_dirty(PanelId::Chtc);
        }
        // P8:系统资源指标变化时标记 ResourceMonitor 面板 dirty,
        // 同时标记 Health 面板(Health 面板也展示系统资源摘要)
        // M3 monitor.pause_sampling:暂停时冻结显示,不因快照变化重标 dirty(避免每 tick 重绘冻结数据)
        if !self.state.monitor_paused
            && (self.state.sys_metrics != snapshot.sys_metrics
                || self.state.sys_metrics_history != snapshot.sys_metrics_history)
        {
            self.state.mark_dirty(PanelId::ResourceMonitor);
            self.state.mark_dirty(PanelId::Health);
        }
        // M3b:对话历史或状态变化时标记 Chat 面板重绘
        if self.state.chat_messages != snapshot.chat_messages
            || self.state.chat_status != snapshot.chat_status
        {
            self.state.mark_dirty(PanelId::Chat);
        }
        // P1-W2.2:Critical 丢弃计数变化时标记 EventStream 面板重绘(顶部告警行)
        if self.state.critical_event_dropped_count != snapshot.critical_event_dropped_count {
            self.state.mark_dirty(PanelId::EventStream);
        }
    }

    /// 切换到下一个面板
    pub fn switch_panel_next(&mut self) {
        let before = self.focus_manager.focused();
        self.focus_manager.next();
        self.record_prev_panel(before);
    }

    /// 切换到上一个面板
    pub fn switch_panel_prev(&mut self) {
        let before = self.focus_manager.focused();
        self.focus_manager.prev();
        self.record_prev_panel(before);
    }

    /// 切换到指定面板
    pub fn switch_panel_to(&mut self, panel: PanelId) {
        let before = self.focus_manager.focused();
        self.focus_manager.jump_to(panel);
        self.record_prev_panel(before);
    }

    /// 记录切换前的焦点面板为伴随面板目标(仅当焦点确实变化时)
    ///
    /// WHY 仅在变化时记录:重复切到同一面板不应把伴随目标覆盖为自身,
    /// 保证 `companion_target` 始终指向"上一个不同面板"。
    fn record_prev_panel(&mut self, before: PanelId) {
        if self.focus_manager.focused() != before {
            self.prev_panel = Some(before);
            // Stage 2/M3d:主区焦点变化时复位活跃窗格回主区,避免焦点滞留旧次窗格。
            self.active_pane = 0;
        }
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

        // M2.2:统一命令面板打开时,键盘事件全部路由给面板(模糊检索/导航/执行),
        // 与 InputRouter 的 Command 模式语义一致(Esc 关闭 / ↑↓ 选择 / Enter 执行)。
        if self.palette.is_some() {
            self.handle_palette_key(key);
            return;
        }

        // 命令/搜索模式:委托给命令面板(`:` 带参命令 / `/` 关键字过滤)
        if matches!(
            self.state.input_mode,
            InputMode::Command | InputMode::Search
        ) {
            if let Some(cmd) = self.command_palette.handle_key(key, &mut self.state) {
                self.apply_command(cmd);
            }
            return;
        }

        // Insert 模式(M3a):原始文本输入,经 InputRouter 的 Insert 表路由
        if self.state.input_mode == InputMode::Insert {
            self.handle_insert_key(key);
            return;
        }

        // 普通模式:经 InputRouter 计算路由目标,再由 apply_route_target 执行。
        // WHY 经路由器:按键归属(全局键/模式切换/面板跳转/焦点面板)的单一事实源在
        // InputRouter,app 只负责"执行意图",与"面板表达意图、App 执行"设计一致。
        // g / Ctrl+W 前缀为瞬态:置位后下一键在对应模式解析,解析后立即复位。
        let mode = if self.state.g_prefix {
            RouterMode::GPrefix
        } else if self.state.w_prefix {
            RouterMode::WPrefix
        } else {
            RouterMode::Normal
        };
        let mut target = InputRouter::route(mode, key);
        // 消费瞬态前缀态(在 apply 前复位,保证本键若为 EnterMode(前缀) 的置位不被清)。
        self.state.g_prefix = false;
        self.state.w_prefix = false;
        // g 前缀遇非预期次键:退出前缀态后,该键按面板级委托处理(与旧 handle_global_key
        // `_ => return false` 行为一致——不重新触发 Normal 全局键,避免 `gq` 误退出)。
        // WPrefix 遇非预期次键(ExitMode)直接取消,不回退委托(Ctrl+W 后随机键 = 取消)。
        if mode == RouterMode::GPrefix && target == RouteTarget::ExitMode {
            target = RouteTarget::FocusPanel;
        }
        self.apply_route_target(target, key);
    }

    /// 执行 InputRouter 计算出的路由目标(Normal/GPrefix 上下文)
    ///
    /// WHY 集中执行:路由器只表达"按键归属意图",具体副作用(退出/切面板/滚动/
    /// 主题/比例/派发动作/进入模式/面板委托)在此统一落地,与"面板表达意图、
    /// App 执行"设计一致。Insert/Command 模式专属目标(InsertChar/Palette*/Submit 等)
    /// 不会由 Normal/GPrefix 路由产生,此处按无操作兜底以保证穷尽匹配。
    fn apply_route_target(&mut self, target: RouteTarget, key: KeyEvent) {
        match target {
            RouteTarget::Quit => self.quit(),
            RouteTarget::PanelJump(id) => self.switch_panel_to(id),
            RouteTarget::FocusCycle { forward } => {
                if forward {
                    self.switch_panel_next();
                } else {
                    self.switch_panel_prev();
                }
            }
            RouteTarget::ScrollTop => {
                let focused = self.focus_manager.focused();
                if let Some(idx) = self.panel_index(focused) {
                    self.panels[idx].scroll_to_top(&mut self.state);
                }
            }
            RouteTarget::ScrollBottom => {
                let focused = self.focus_manager.focused();
                if let Some(idx) = self.panel_index(focused) {
                    self.panels[idx].scroll_to_bottom(&mut self.state);
                }
            }
            RouteTarget::ThemeCycle => self.cycle_theme_action(),
            RouteTarget::RatioAdjust { increase } => self.adjust_main_panel_ratio(increase),
            // Action 支持的全局键统一经派发桥接(locale/layout/companion/help/export);
            // 均在 dispatch_action 有本地 arm,不会回退发事件。
            RouteTarget::GlobalAction(action_id) => {
                self.dispatch_action(action_id, "{}".to_string(), ActionSource::Panel);
            }
            // 模式入口:`:` 命令栏 / `/` 搜索 / Ctrl+P palette / `i` Insert / `g` 前缀
            RouteTarget::EnterCommandBar => {
                self.state.input_mode = InputMode::Command;
                self.state.input_buffer.clear();
            }
            RouteTarget::EnterSearch => {
                self.state.input_mode = InputMode::Search;
                self.state.input_buffer.clear();
            }
            RouteTarget::OpenPalette => self.open_palette(),
            RouteTarget::OpenActionMenu => self.open_action_menu(),
            RouteTarget::EnterMode(RouterMode::Insert) => {
                self.state.input_mode = InputMode::Insert;
                self.state.input_buffer.clear();
            }
            RouteTarget::EnterMode(RouterMode::GPrefix) => {
                self.state.g_prefix = true;
            }
            RouteTarget::EnterMode(RouterMode::WPrefix) => {
                self.state.w_prefix = true;
            }
            // Ctrl+W 前缀方向导航:按窗格几何切换活跃窗格(h/l 左右,j/k 上下)
            RouteTarget::FocusPaneDir(dir) => self.focus_pane_dir(dir),
            // 交由当前活跃窗格(Stage 2 伴随焦点感知)处理
            RouteTarget::FocusPanel => self.delegate_key_to_active_panel(key),
            // 以下目标不由 Normal/GPrefix 路由产生(Insert/Command 模式专属或已在上游处理),
            // 兜底无操作以保证穷尽匹配。
            RouteTarget::EnterMode(RouterMode::Normal | RouterMode::Command)
            | RouteTarget::ExitMode
            | RouteTarget::InsertChar(_)
            | RouteTarget::PaletteInput(_)
            | RouteTarget::PaletteMove { .. }
            | RouteTarget::Backspace
            | RouteTarget::Submit
            | RouteTarget::Ignored => {}
        }
    }

    /// 处理 Insert 模式按键(M3a):经 InputRouter 的 Insert 表路由到输入缓冲操作
    ///
    /// WHY 独立方法:Insert 是原始文本输入,与 Normal 的按键归属语义不同
    /// (字符进缓冲、Enter 提交、Esc 退出),单独处理避免与 apply_route_target 混杂。
    /// M3a 阶段 Submit 为占位(不发事件),M3b 接入 Chat 面板后改为发 TuiChatSubmitted。
    fn handle_insert_key(&mut self, key: KeyEvent) {
        match InputRouter::route(RouterMode::Insert, key) {
            RouteTarget::InsertChar(c) => self.state.input_buffer.push(c),
            RouteTarget::Backspace => {
                self.state.input_buffer.pop();
            }
            RouteTarget::ExitMode => {
                self.state.input_mode = InputMode::Normal;
                self.state.input_buffer.clear();
            }
            // Insert 下仍允许极少数全局键(如 Ctrl+L 中英切换),经派发桥接
            RouteTarget::GlobalAction(action_id) => {
                self.dispatch_action(action_id, "{}".to_string(), ActionSource::Chat);
            }
            RouteTarget::Submit => {
                // M3b:非空输入发布 TuiChatSubmitted(经 EventBus 回环由 ChatSync 追加用户消息),
                // 自动切到 Chat 面板;保持 Insert 模式形成 chat REPL(Esc 退出)。
                let text = self.state.input_buffer.trim().to_string();
                if !text.is_empty() {
                    // 以 `/` 开头视为斜杠命令,提取命令名(首个空白前的词)
                    let slash_command = text
                        .strip_prefix('/')
                        .map(|rest| rest.split_whitespace().next().unwrap_or("").to_string());
                    self.publish_control_event(NexusEvent::TuiChatSubmitted {
                        metadata: EventMetadata::new("chimera-tui"),
                        session_id: self.chat_session_id.clone(),
                        query: text,
                        slash_command,
                    });
                    self.switch_panel_to(PanelId::Chat);
                }
                self.state.input_buffer.clear();
            }
            // 其余(Ignored 等)在 Insert 下无操作
            _ => {}
        }
    }

    /// 循环切换主题 Dark → Light → HighContrast → Dark(P6.1,原 `t` 全局键)
    ///
    /// WHY 提取为方法:M3a 将 `t` 键路由为 `RouteTarget::ThemeCycle`,执行逻辑集中于此,
    /// 与 `cycle_layout_action` 等其他 *_action 方法风格一致(DRY)。切换后标记所有面板
    /// dirty 触发重绘以应用新配色。
    fn cycle_theme_action(&mut self) {
        let new_theme = self.config.theme.next();
        self.config.theme = new_theme;
        // 标记所有已注册面板为 dirty,确保下一帧重绘
        for panel_id in self.focus_manager.panels() {
            self.state.mark_dirty(*panel_id);
        }
        self.state.status_message = Some((
            format!("{}: {}", crate::t!("status.theme"), new_theme.as_str()),
            Severity::Info,
        ));
    }

    /// 将按键委托给当前活跃窗格处理(Stage 2 伴随焦点感知)
    ///
    /// WHY 活跃窗格优先:面板级键路由到 `active_pane` 指向的窗格面板(M3d 多窗格);
    /// 单窗格时即主焦点面板。路由器返回 `FocusPanel`(非全局键)时经此委托。
    fn delegate_key_to_active_panel(&mut self, key: KeyEvent) {
        // M3d:路由到当前活跃窗格对应的面板(active_pane 是 pane_panels 循环序下标);
        // 越界(窗格数收缩未及钳制)兜底回主焦点面板。
        let panes = self.pane_panels();
        let target = panes
            .get(self.active_pane)
            .copied()
            .unwrap_or_else(|| self.focus_manager.focused());
        if let Some(idx) = self.panel_index(target) {
            if let Some(cmd) = self.panels[idx].handle_key(key, &mut self.state) {
                self.apply_command(cmd);
            }
        }
    }

    /// 打开统一命令面板(M2.2)
    ///
    /// WHY 复用既有模型:若已存在(之前打开过)则仅 `open()` 复位 query/选择,
    /// 保留其 `ActionRegistry` 副本;首次打开才用内建六域注册表构造,
    /// 避免每次打开都重建注册表(约 21 条描述)。
    fn open_palette(&mut self) {
        let mut model = self
            .palette
            .take()
            .unwrap_or_else(CommandPaletteModel::with_builtin_domains);
        model.open();
        self.palette = Some(model);
    }

    /// 打开焦点面板的上下文动作菜单(§4.5 入口三:面板动作)
    ///
    /// WHY 从 Registry 组装:动作集由 `panel_context_actions(焦点面板)` 精选,
    /// 展示标题经 `ActionRegistry` + i18n 解析(与命令面板/斜杠同源,locale 感知)。
    /// 选中经 `DispatchAction{source:Panel}` 统一派发,复用三入口执行/反馈管线。
    fn open_action_menu(&mut self) {
        let focused = self.focus_manager.focused();
        let ids = crate::actions::panel_context_actions(focused);
        let registry = crate::actions::ActionRegistry::with_builtin_domains();
        let entries: Vec<(String, String)> = ids
            .into_iter()
            .map(|id| {
                let title = registry
                    .get(id)
                    .map(|d| crate::i18n::tr(d.title_key).to_string())
                    .unwrap_or_else(|| id.to_string());
                (id.to_string(), title)
            })
            .collect();
        self.state
            .popup_stack
            .push(PopupKind::action_menu(focused.as_str(), entries));
    }

    /// 命令面板打开时的键盘处理(M2.2)
    ///
    /// 语义对齐 `InputRouter` 的 Command 模式:Esc 关闭 / ↑↓ 选择 / Enter 执行
    /// 选中动作(经 `DispatchAction` 统一派发,source=Palette)/ 退格 / 字符过滤。
    ///
    /// WHY 逐分支分别借用 `self.palette`:Esc/Enter 需写 `self.palette = None`,
    /// 而导航键需 `&mut` 模型;分开借用避免在同一作用域同时持有可变
    /// 引用与重赋值的借用冲突。
    fn handle_palette_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                self.palette = None;
            }
            KeyCode::Enter => {
                // 先取选中动作 id(&'static str,不借用模型),关闭面板后统一派发。
                let action_id = self
                    .palette
                    .as_ref()
                    .and_then(|m| m.selected_action())
                    .map(str::to_string);
                self.palette = None;
                if let Some(action_id) = action_id {
                    self.apply_command(TuiCommand::DispatchAction {
                        action_id,
                        payload: "{}".to_string(),
                        source: ActionSource::Palette,
                    });
                }
            }
            KeyCode::Up => {
                if let Some(m) = self.palette.as_mut() {
                    m.move_selection(false);
                }
            }
            KeyCode::Down => {
                if let Some(m) = self.palette.as_mut() {
                    m.move_selection(true);
                }
            }
            KeyCode::Backspace => {
                if let Some(m) = self.palette.as_mut() {
                    m.on_backspace();
                }
            }
            // 排除 Ctrl 组合:仅纯字符进入检索缓冲,Ctrl+X 类快捷键在面板内忽略。
            KeyCode::Char(c) if !key.modifiers.contains(event::KeyModifiers::CONTROL) => {
                if let Some(m) = self.palette.as_mut() {
                    m.on_input(c);
                }
            }
            _ => {}
        }
    }

    /// 命令面板是否打开(测试与外部查询用)
    pub fn palette_is_open(&self) -> bool {
        self.palette.is_some()
    }

    /// 伴随面板是否可见(测试与外部查询用)
    pub fn companion_visible(&self) -> bool {
        self.companion_visible
    }

    /// 活跃窗格是否为伴随(次)窗格(测试与外部查询用)
    ///
    /// WHY 保留 bool 语义:M3d 后活跃窗格为索引,此访问器映射"活跃窗格是否为第 2 窗格"
    /// (`active_pane == 1`,2 窗格时即 companion),保持 Stage 2 测试与外部契约等价。
    pub fn companion_focused(&self) -> bool {
        self.active_pane == 1
    }

    /// 当前伴随面板目标(测试与外部查询用;不含可见性判断)
    pub fn companion_panel(&self) -> Option<PanelId> {
        self.companion_target()
    }

    /// 当前活跃窗格索引(M3d,0 = 主窗格;测试与外部查询用)
    pub fn active_pane(&self) -> usize {
        self.active_pane
    }

    /// 当前可见窗格数(M3d,由 PaneMode + companion_visible 决定;测试与外部查询用)
    pub fn pane_count(&self) -> usize {
        self.pane_panels().len()
    }

    /// M3d:窗格数变化(切布局 / 开关伴随)后钳制活跃窗格索引,越界则复位主区
    ///
    /// WHY 复位而非饱和:切布局 / 开关伴随是显式的视图上下文切换,回到主窗格(0)
    /// 符合直觉且避免焦点滞留已消失的次窗格;`get(active).unwrap_or` 兜底不 panic。
    fn clamp_active_pane(&mut self) {
        if self.active_pane >= self.pane_panels().len() {
            self.active_pane = 0;
        }
    }

    /// 计算伴随面板目标:显式绑定优先,否则最近使用面板(且非当前焦点),
    /// 无历史则回退到焦点顺序中首个非焦点面板
    ///
    /// WHY 回退链:保证伴随面板开启时总有内容可显示,且永不等于主区面板。
    fn companion_target(&self) -> Option<PanelId> {
        let focused = self.focus_manager.focused();
        // Stage 2:显式绑定优先(且不等于主区面板)
        if let Some(bound) = self.bound_companion {
            if bound != focused {
                return Some(bound);
            }
        }
        if let Some(prev) = self.prev_panel {
            if prev != focused {
                return Some(prev);
            }
        }
        self.focus_manager
            .panels()
            .iter()
            .copied()
            .find(|&p| p != focused)
    }

    /// M3d:IDE 侧栏(第三窗格)目标 —— 首个既非主焦点、也非 context 的面板
    ///
    /// WHY 确定性:三窗格 IDE 需要一个稳定的第三面板;取焦点面板顺序中首个
    /// 不等于主焦点与 context 的面板,面板不足 3 个时返回 None(退化为 2 窗格)。
    fn third_pane_target(&self) -> Option<PanelId> {
        let focused = self.focus_manager.focused();
        let context = self.companion_target();
        self.focus_manager
            .panels()
            .iter()
            .copied()
            .find(|&p| p != focused && Some(p) != context)
    }

    /// M3d:是否应显示次窗格(非 Focus 单窗格)—— pane_panels 与 pane_rects 共用判定
    ///
    /// WHY 抽取:窗格计数(路由)与区域切分(渲染)必须用同一规则判断"是否多窗格",
    /// 否则会出现路由认为 2 窗格、渲染只画 1 窗格的错位。Chat 尊重 companion_visible
    /// opt-in(Stage 2 语义);VimSplit/IDE 为内在多窗格。
    fn wants_multi_pane(&self) -> bool {
        use crate::engine::layout::PaneMode;
        match self.state.layout_mode.to_pane_mode() {
            PaneMode::Focus => false,
            PaneMode::Chat => self.companion_visible,
            PaneMode::VimSplit | PaneMode::Ide => true,
        }
    }

    /// M3d:当前 PaneMode 下可见窗格的面板列表(循环序 [主, (context), (侧栏)])
    ///
    /// WHY 不含 rect:此列表供键盘路由(`delegate_key_to_active_panel`)与窗格计数
    /// (`focus_pane_action` 的 `w` 循环)使用,与几何无关;渲染另经 `pane_rects` 取区域。
    /// - Focus:[主]
    /// - Chat:companion_visible ? [主, context] : [主](Stage 2 opt-in 保留)
    /// - VimSplit:[主, context](左右等分)
    /// - Ide:[主, context, 侧栏](三窗格;目标不足时省略对应窗格,列表无空洞)
    fn pane_panels(&self) -> Vec<PanelId> {
        use crate::engine::layout::PaneMode;
        let main = self.focus_manager.focused();
        let mut panes = vec![main];
        if !self.wants_multi_pane() {
            return panes;
        }
        if let Some(context) = self.companion_target() {
            panes.push(context);
        }
        // IDE 三窗格:在 context 之外再追加侧栏(第三面板)
        if self.state.layout_mode.to_pane_mode() == PaneMode::Ide {
            if let Some(sidebar) = self.third_pane_target() {
                panes.push(sidebar);
            }
        }
        panes
    }

    /// M3d:当前 PaneMode 下各可见窗格的区域(循环序对齐 `pane_panels`)
    ///
    /// WHY 复用引擎 split:用自研 `engine::layout::split` 按 PaneMode 切分主内容区的列
    /// (外层 tabs/status 已由 `layout()` 处理)。返回顺序与 `pane_panels` 一致
    /// ([主, context, 侧栏]);单窗格模式 / 窄视口 / 无次窗格目标退化为 `vec![area]`
    /// (与既有 companion_split 逐字节等价,零回归)。
    fn pane_rects(&self, area: ratatui::layout::Rect) -> Vec<ratatui::layout::Rect> {
        use crate::engine::layout::{Constraint, Direction, PaneMode};
        if !self.wants_multi_pane()
            || area.width < COMPANION_MIN_WIDTH
            || self.companion_target().is_none()
        {
            return vec![area];
        }
        let eng_area = crate::engine::from_ratatui_rect(area);
        let to = crate::engine::to_ratatui_rect;
        match self.state.layout_mode.to_pane_mode() {
            // Chat:主(Flex)+ 右 context(固定宽)。循环序 [主, context]。
            PaneMode::Chat => {
                let cols = crate::engine::layout::split(
                    eng_area,
                    Direction::Horizontal,
                    &[Constraint::Flex(1), Constraint::Fixed(COMPANION_WIDTH)],
                );
                vec![to(cols[0]), to(cols[1])]
            }
            // VimSplit:左右等分(Flex1 + Flex1)。循环序 [主, context]。
            PaneMode::VimSplit => {
                let cols = crate::engine::layout::split(
                    eng_area,
                    Direction::Horizontal,
                    &[Constraint::Flex(1), Constraint::Flex(1)],
                );
                vec![to(cols[0]), to(cols[1])]
            }
            // IDE:三栏 [侧栏(左,固定) | 主(中,Flex) | context(右,固定)]。
            // 位置左中右,循环序返回 [主(中), context(右), 侧栏(左)]——各窗格自带
            // 区域,渲染位置与循环索引解耦,保证 active_pane=0 恒为主区。
            PaneMode::Ide => {
                let cols = crate::engine::layout::split(
                    eng_area,
                    Direction::Horizontal,
                    &[
                        Constraint::Fixed(IDE_SIDEBAR_WIDTH),
                        Constraint::Flex(1),
                        Constraint::Fixed(IDE_CONTEXT_WIDTH),
                    ],
                );
                vec![to(cols[1]), to(cols[2]), to(cols[0])]
            }
            // Focus 已在上方 wants_multi_pane=false 提前返回,此处兜底整块。
            PaneMode::Focus => vec![area],
        }
    }

    /// 切换伴随面板可见性(M2 增量3,供 `\` 键与命令面板 `view.toggle_companion` 共用)
    fn toggle_companion_action(&mut self) {
        self.companion_visible = !self.companion_visible;
        // M3d:关闭伴随会使 Chat 窗格数 2→1,钳制活跃窗格避免停留已消失的 context。
        self.clamp_active_pane();
        let state_label = if self.companion_visible { "on" } else { "off" };
        self.state.status_message = Some((
            format!(
                "{}: {}",
                crate::t!("action.view.toggle_companion"),
                state_label
            ),
            Severity::Info,
        ));
    }

    /// 循环绑定伴随面板到下一个非焦点面板(M2 增量3 Stage 2,供 `]` 与命令面板共用)
    ///
    /// 以焦点面板顺序从当前伴随目标起环形查找下一个 `!= 主焦点` 的面板写入
    /// `bound_companion`,并置 `companion_visible = true`(循环即意图显示)。
    fn cycle_companion_action(&mut self) {
        let focused = self.focus_manager.focused();
        // WHY 克隆为 Vec:既将读面板顺序又要随后可变写 bound_companion,
        // 克隆断开 self 借用(约 19 个 PanelId,Copy,廉价)。
        let panels: Vec<PanelId> = self.focus_manager.panels().to_vec();
        if panels.len() < 2 {
            return;
        }
        // 起点:当前伴随目标位置(无则从主焦点位置起)
        let start = self
            .companion_target()
            .and_then(|c| panels.iter().position(|&p| p == c))
            .unwrap_or_else(|| panels.iter().position(|&p| p == focused).unwrap_or(0));
        let n = panels.len();
        let next = (1..=n)
            .map(|off| panels[(start + off) % n])
            .find(|&p| p != focused);
        if let Some(target) = next {
            self.bound_companion = Some(target);
            self.companion_visible = true;
            self.state.status_message = Some((
                format!(
                    "{}: {}",
                    crate::t!("action.view.cycle_companion"),
                    target.as_str()
                ),
                Severity::Info,
            ));
        }
    }

    /// 循环切换活跃窗格(M3d,供 `w` 与命令面板共用)
    ///
    /// 在可见窗格间环形循环(main → context → sidebar → main);单窗格
    /// (Focus / Chat 未开伴随 / 面板不足)无可切换窗格时 no-op 并提示。
    fn focus_pane_action(&mut self) {
        let n = self.pane_panels().len();
        if n <= 1 {
            self.state.status_message = Some((
                format!("{}: n/a", crate::t!("action.view.focus_pane")),
                Severity::Warning,
            ));
            return;
        }
        self.active_pane = (self.active_pane + 1) % n;
        // 窗格标签:0 = 主区,其余按序号提示(2 窗格时 1 = companion,语义等价 Stage 2)
        let pane = if self.active_pane == 0 {
            "main".to_string()
        } else {
            format!("pane {}", self.active_pane + 1)
        };
        self.state.status_message = Some((
            format!("{}: {}", crate::t!("action.view.focus_pane"), pane),
            Severity::Info,
        ));
    }

    /// 按方向切换活跃窗格(M3 后续 Ctrl+W h/j/k/l 方向导航)
    ///
    /// 用 `pane_rects` 的窗格矩形几何解析目标:在指定方向上取中心坐标最近的邻窗格。
    /// 当前预设布局均为横向列,故 h/l 生效;j/k(上下)当前无候选时 no-op。
    /// 单窗格(rects<=1)或该方向无邻居时 no-op 并提示。
    fn focus_pane_dir(&mut self, dir: PaneDir) {
        let main = self.layout(self.last_area)[1];
        let rects = self.pane_rects(main);
        if rects.len() <= 1 {
            self.state.status_message = Some((
                format!("{}: n/a", crate::t!("action.view.focus_pane")),
                Severity::Warning,
            ));
            return;
        }
        let cur = rects.get(self.active_pane).copied().unwrap_or(rects[0]);
        let cur_cx = cur.x as i32 + cur.width as i32 / 2;
        let cur_cy = cur.y as i32 + cur.height as i32 / 2;
        // 在指定方向上找中心坐标最近的窗格(水平比 x,垂直比 y)
        let mut best: Option<(usize, i32)> = None;
        for (idx, r) in rects.iter().enumerate() {
            if idx == self.active_pane {
                continue;
            }
            let cx = r.x as i32 + r.width as i32 / 2;
            let cy = r.y as i32 + r.height as i32 / 2;
            let dist = match dir {
                PaneDir::Left => (cx < cur_cx).then_some(cur_cx - cx),
                PaneDir::Right => (cx > cur_cx).then_some(cx - cur_cx),
                PaneDir::Up => (cy < cur_cy).then_some(cur_cy - cy),
                PaneDir::Down => (cy > cur_cy).then_some(cy - cur_cy),
            };
            if let Some(d) = dist {
                let better = match best {
                    None => true,
                    Some((_, bd)) => d < bd,
                };
                if better {
                    best = Some((idx, d));
                }
            }
        }
        match best {
            Some((idx, _)) => {
                self.active_pane = idx;
                self.state.status_message = Some((
                    format!("{}: pane {}", crate::t!("action.view.focus_pane"), idx + 1),
                    Severity::Info,
                ));
            }
            None => {
                self.state.status_message = Some((
                    format!("{}: n/a", crate::t!("action.view.focus_pane")),
                    Severity::Warning,
                ));
            }
        }
    }

    /// 切换界面语言(中英)并刷新(M2.1 提取,供 Ctrl+L 与命令面板共用)
    ///
    /// WHY 提取:Ctrl+L 快捷键与命令面板 `system.toggle_locale` 动作行为必须一致,
    /// 集中一处避免两条入口逻辑漂移(三入口统一派发)。
    fn toggle_locale_action(&mut self) {
        let locale = crate::i18n::toggle_locale();
        // 切换后全体面板文案需重绘,标记 dirty 触发下一帧重绘
        for panel_id in self.focus_manager.panels() {
            self.state.mark_dirty(*panel_id);
        }
        self.state.status_message = Some((
            format!("{}: {}", crate::t!("status.locale"), locale.short_label()),
            Severity::Info,
        ));
    }

    /// 循环切换布局模式(M2 提取,供 `l` 键与命令面板 `view.switch_layout` 共用)
    fn cycle_layout_action(&mut self) {
        let new_mode = self.state.layout_mode.next();
        self.state.layout_mode = new_mode;
        // M3d:切布局可能改变窗格数(如 IDE 3 → Focus 1),钳制活跃窗格回主区。
        self.clamp_active_pane();
        self.state.status_message = Some((
            format!("{}: {}", crate::t!("status.layout"), new_mode.as_str()),
            Severity::Info,
        ));
    }

    /// 打开帮助 overlay(M2 提取,供 `?` 键与命令面板 `system.open_help` 共用)
    ///
    /// 传入当前焦点面板的快捷键列表,帮助按上下文动态生成(§4.6 渐进披露)。
    fn open_help_action(&mut self) {
        let shortcuts = self
            .panels
            .iter()
            .find(|p| p.id() == self.focus_manager.focused())
            .map(|p| p.shortcuts())
            .unwrap_or_default();
        // M2 增量3:帮助浮层追加 Registry 驱动的命令清单(与命令面板 Ctrl+P 同源),
        // 随 locale 动态生成。构造成本低(约 21 条),`?` 为低频操作,按需构建即可。
        let registry = crate::actions::ActionRegistry::with_builtin_domains();
        let action_lines: Vec<(String, String)> =
            crate::actions::codegen::help_lines(&registry, None)
                .into_iter()
                .map(|line| (line.key, line.title))
                .collect();
        self.state
            .popup_stack
            .push(PopupKind::help_overlay_with_context_and_actions(
                &shortcuts,
                &action_lines,
            ));
    }

    /// 面板下钻:焦点面板进入 Focus 全屏(SinglePane)——§4.6 L3 三级信息层级下钻层
    ///
    /// WHY 复用 SinglePane:该布局即"当前面板全屏",语义等同下钻 Focus;`l` 键循环
    /// 布局可切回,无需额外返回栈。切布局后 `clamp_active_pane` 收敛活跃窗格到单窗格。
    fn drill_down_action(&mut self) {
        self.state.layout_mode = LayoutMode::SinglePane;
        self.clamp_active_pane();
        let focused = self.focus_manager.focused();
        self.state.status_message = Some((
            format!(
                "{}: {}",
                crate::t!("action.panel.drill_down"),
                focused.as_str()
            ),
            Severity::Info,
        ));
    }

    /// 切换监控采样暂停(monitor.pause_sampling;UI 本地冻结显示)
    ///
    /// WHY 标记 ResourceMonitor + Health dirty:两面板均展示 sys_metrics,
    /// 暂停/恢复切换需立即重绘(显/隐 PAUSED 标记 + 恢复时刷新最新数据)。
    fn toggle_monitor_pause(&mut self) {
        self.state.monitor_paused = !self.state.monitor_paused;
        self.state.mark_dirty(PanelId::ResourceMonitor);
        self.state.mark_dirty(PanelId::Health);
        let msg = if self.state.monitor_paused {
            "监控采样已暂停(显示冻结)"
        } else {
            "监控采样已恢复"
        };
        self.state.status_message = Some((msg.to_string(), Severity::Info));
    }

    /// 循环监控 sparkline 时间窗(monitor.time_window)
    fn cycle_monitor_window(&mut self) {
        self.state.monitor_window = self.state.monitor_window.next();
        self.state.mark_dirty(PanelId::ResourceMonitor);
        self.state.status_message = Some((
            format!("监控时间窗: 最近 {} 点", self.state.monitor_window.label()),
            Severity::Info,
        ));
    }

    /// 切换可视化维度(viz.switch_dimension)——按焦点面板分级
    ///
    /// WHY 分级:仅 ClvVector 有数据背书(8-block 热图值域 fixed↔autoscale);
    /// OsaSparse(仅平均稀疏度)/ MetricsDashboard(cell 默认未绑定)暂无可切维度,
    /// 给诚实反馈而非伪造(质量红线:不造无数据支撑的功能)。
    fn switch_viz_dimension(&mut self) {
        match self.focus_manager.focused() {
            PanelId::ClvVector => {
                self.state.clv_heatmap_autoscale = !self.state.clv_heatmap_autoscale;
                self.state.mark_dirty(PanelId::ClvVector);
                let mode = if self.state.clv_heatmap_autoscale {
                    "自适应(按实际值域)"
                } else {
                    "固定 [-1, 1]"
                };
                self.state.status_message = Some((format!("CLV 热图值域: {mode}"), Severity::Info));
            }
            other => {
                self.state.status_message = Some((
                    format!("{}: 当前面板暂无可切换维度", other.as_str()),
                    Severity::Info,
                ));
            }
        }
    }

    /// 应用磁盘持久化的视图(view.apply_saved)——重载 `state_file_path` 的视图字段
    ///
    /// WHY 重载而非命名视图:项目无命名视图概念,"saved view" 即退出时经
    /// `save_to_file` 存盘的单一视图快照;本动作让用户中途恢复上次保存的布局偏好。
    fn apply_saved_view_action(&mut self) {
        let path = self.config.state_file_path.clone();
        if !path.exists() {
            self.state.status_message = Some((
                "无已保存的视图(退出时自动保存布局)".to_string(),
                Severity::Info,
            ));
            return;
        }
        let saved = crate::types::TuiState::load_from_file(&path);
        self.apply_view_fields(&saved);
        // 布局可能变化(如 Triple→Single),钳制活跃窗格;全面板 dirty 确保重绘
        self.clamp_active_pane();
        for panel_id in self.focus_manager.panels() {
            self.state.mark_dirty(*panel_id);
        }
        self.state.status_message = Some(("已应用保存的视图".to_string(), Severity::Info));
    }

    /// 从另一 `TuiState` 拷贝视图字段(纯逻辑,便于测试)
    ///
    /// 仅拷贝视图偏好(布局/过滤/监控窗口/CLV 值域),不碰运行时/瞬态字段
    ///(running/events/popup/monitor_paused),避免恢复视图误改运行状态。
    fn apply_view_fields(&mut self, from: &crate::types::TuiState) {
        self.state.layout_mode = from.layout_mode;
        self.state.filter_keyword = from.filter_keyword.clone();
        self.state.filter_topic = from.filter_topic.clone();
        self.state.filter_level = from.filter_level.clone();
        self.state.monitor_window = from.monitor_window;
        self.state.clv_heatmap_autoscale = from.clv_heatmap_autoscale;
    }

    /// 跳转到焦点/单一 Quest 的事件流(quest.jump;TUI 本地,复用 JumpToEventStream)
    ///
    /// WHY TUI 本地:quest.jump 是"切到事件流并按 Quest 过滤"的视图导航,无需引擎;
    /// 与 QuestPanel Enter 同用 `JumpToEventStream`。命令面板/菜单无选中上下文时,
    /// 单 Quest 直接过滤,0/多 Quest 诚实切换(不臆测目标,提示经 Quest 面板精确跳转)。
    fn jump_to_quest_events_action(&mut self) {
        // §1.3b:焦点面板(Quest)有选中项时精确跳转其事件流
        if let Some(quest_id) = self.focused_selected_context_id() {
            self.apply_command(TuiCommand::JumpToEventStream { quest_id });
            return;
        }
        // 无选中上下文(焦点非列表面板):回退 quest_list 数量启发
        match self.state.quest_list.len() {
            1 => {
                let quest_id = self.state.quest_list[0].quest_id.clone();
                self.apply_command(TuiCommand::JumpToEventStream { quest_id });
            }
            0 => {
                self.switch_panel_to(PanelId::EventStream);
                self.state
                    .set_status("已切换事件流(当前无 Quest)", Severity::Info);
            }
            _ => {
                self.switch_panel_to(PanelId::EventStream);
                self.state.set_status(
                    "已切换事件流(多 Quest;Quest 面板选中后 Enter 可精确跳转)",
                    Severity::Info,
                );
            }
        }
    }

    /// 返回焦点面板当前选中项的上下文 id(§1.3b,供 quest.* 精确定位)
    ///
    /// 经 `Panel::selected_context_id` 泛化获取,不判具体面板类型;
    /// 焦点面板未覆写(展示型)或无选中项时返回 None。
    fn focused_selected_context_id(&self) -> Option<String> {
        let focused = self.focus_manager.focused();
        self.panel_index(focused)
            .and_then(|idx| self.panels[idx].selected_context_id(&self.state))
    }

    /// 若焦点面板有选中 Quest 且 payload 未含 quest_id,注入之(§1.3b 精确定位)
    ///
    /// WHY 单点富化:三入口派发 quest.pause/resume/cancel 均经 dispatch_action,
    /// 此处统一注入使"选中某 Quest 后经菜单/命令面板操作它"精确生效;
    /// cli `resolve_quest_id` 优先用 payload.quest_id,注入即命中,消除多 Quest 歧义。
    fn enrich_payload_with_focused_quest(&self, payload: String) -> String {
        let Some(quest_id) = self.focused_selected_context_id() else {
            return payload;
        };
        // 解析为 JSON 对象;已含 quest_id 则尊重显式指定不覆盖
        let mut obj = serde_json::from_str::<serde_json::Value>(&payload)
            .ok()
            .and_then(|v| v.as_object().cloned())
            .unwrap_or_default();
        if obj.contains_key("quest_id") {
            return payload;
        }
        obj.insert("quest_id".to_string(), serde_json::Value::String(quest_id));
        serde_json::to_string(&obj).unwrap_or(payload)
    }

    /// 统一派发 action_id 为具体行为(M2 增量2:三入口统一派发桥接)
    ///
    /// WHY 桥接而非仅发事件:命令面板 Enter 需产生"真实效果",但既有可用路径分两类——
    /// - **本地即时效果**(无参数):切换语言/布局、打开帮助,直接调用既有本地方法;
    /// - **需编排器消费的动作**(agent.chat/quest.*/task.* 等,多含参数):当前无本地
    ///   通路,回退发布 `TuiActionRequested`,交 chimera-cli QueryLoop 编排(M3 落地)。
    ///
    /// 面板上下文动作仍走各自的 `TuiCommand` 变体(带 quest_id 等参数),不经此桥接;
    /// 本方法只服务"无参数、来源为命令面板/斜杠"的统一入口。
    fn dispatch_action(&mut self, action_id: &str, payload: String, source: ActionSource) {
        // §1.3b:quest.pause/resume/cancel 若焦点面板有选中 Quest,注入 quest_id 精确定位;
        // 其余动作(agent.chat/quest.start 需 query、task.* 已推迟)payload 原样透传。
        let payload = if matches!(action_id, "quest.pause" | "quest.resume" | "quest.cancel") {
            self.enrich_payload_with_focused_quest(payload)
        } else {
            payload
        };
        match action_id {
            // —— 本地即时效果(无参数,已有实现路径)——
            "system.toggle_locale" => self.toggle_locale_action(),
            "view.switch_layout" => self.cycle_layout_action(),
            "view.toggle_companion" => self.toggle_companion_action(),
            "view.cycle_companion" => self.cycle_companion_action(),
            "view.focus_pane" => self.focus_pane_action(),
            "system.open_help" => self.open_help_action(),
            // export.run:本地弹出导出格式选择框(原 `E` 键行为)——必须本地 arm,
            // 否则经 router 路由的 `E`/Ctrl+E 会回退发事件而非本地导出(避免行为回归)。
            "export.run" => self.handle_export_command(),
            // —— Phase 2 UI 本地域(UI 态,不绕道 cli,§2.2 依赖铁律)——
            // 面板下钻:焦点面板进入 Focus 全屏(SinglePane),`l` 键切回(§4.6 L3 下钻层级)。
            "panel.drill_down" => self.drill_down_action(),
            // —— UI 本地态视图/配置动作(M3/M4 已落地,不绕 cli,§2.2 依赖铁律)——
            // view.apply_saved 重载持久化视图;monitor/viz 视图控制;config.edit 配置速调菜单。
            "view.apply_saved" => self.apply_saved_view_action(),
            // M3 三大核心功能:monitor/viz 视图控制(UI 本地态,不绕 cli)
            "monitor.pause_sampling" => self.toggle_monitor_pause(),
            "monitor.time_window" => self.cycle_monitor_window(),
            "viz.switch_dimension" => self.switch_viz_dimension(),
            "config.edit" => self.open_config_menu(),
            // quest.jump:TUI 本地导航(切事件流 + 按 Quest 过滤),不经 cli(§4.6 跨面板联动)
            "quest.jump" => self.jump_to_quest_events_action(),
            // —— 编排域(quest.*/task.*/agent.chat):发布 TuiActionRequested,由 chimera-cli
            // Action 编排器消费并回发 Completed/Failed(P0 已接线,反馈经 ActionFeedbackSync 上屏)——
            _ => {
                self.publish_control_event(NexusEvent::TuiActionRequested {
                    metadata: EventMetadata::new("chimera-tui"),
                    action_id: action_id.to_string(),
                    payload,
                    source,
                });
            }
        }
    }

    /// 处理弹窗激活时的键盘事件
    fn handle_popup_key(&mut self, key: KeyEvent) {
        // ActionMenu 有独立选择/执行语义(↑↓ 移选、Enter 派发选中动作),
        // 与滚动型弹窗(Detail/Help)分流,避免 Up/Down 被 scroll 语义占用。
        if matches!(
            self.state.popup_stack.current(),
            Some(PopupKind::ActionMenu { .. })
        ) {
            self.handle_action_menu_key(key);
            return;
        }
        // ConfigMenu 就地循环编辑语义(↑↓ 移选、Enter 循环当前项、菜单常驻),同样分流
        if matches!(
            self.state.popup_stack.current(),
            Some(PopupKind::ConfigMenu { .. })
        ) {
            self.handle_config_menu_key(key);
            return;
        }
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

    /// 动作菜单弹窗的键盘处理(§4.5 入口三:面板动作)
    ///
    /// ↑↓/kj 移动选中项,Enter 派发选中动作(经 `DispatchAction`,source=Panel,
    /// 复用三入口统一派发与反馈管线),Esc/q 关闭。
    fn handle_action_menu_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.state.popup_stack.pop();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.state.popup_stack.move_action_menu_selection(false);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.state.popup_stack.move_action_menu_selection(true);
            }
            KeyCode::Enter => {
                // 取选中 action_id,关闭菜单后经统一派发桥接执行(source=Panel)
                let action_id = self.state.popup_stack.action_menu_selected_id();
                self.state.popup_stack.pop();
                if let Some(action_id) = action_id {
                    self.apply_command(TuiCommand::DispatchAction {
                        action_id,
                        payload: "{}".to_string(),
                        source: ActionSource::Panel,
                    });
                }
            }
            _ => {}
        }
    }

    /// 打开配置速调菜单(config.edit)—— 列出运行时可调配置项,就地循环编辑
    fn open_config_menu(&mut self) {
        let entries = self.config_menu_entries();
        self.state.popup_stack.push(PopupKind::config_menu(entries));
    }

    /// 组装配置菜单条目(固定顺序 [主题, 占比, Tick],值取自当前 config)
    ///
    /// WHY 顺序固定:`cycle_config_item` 按下标循环对应项,顺序须与本函数一致。
    fn config_menu_entries(&self) -> Vec<(String, String)> {
        vec![
            (
                crate::t!("status.theme").to_string(),
                self.config.theme.as_str().to_string(),
            ),
            (
                crate::t!("status.ratio").to_string(),
                format!("{:.0}%", self.main_panel_ratio * 100.0),
            ),
            (
                crate::t!("status.tick").to_string(),
                format!("{}ms (重启生效)", self.config.tick_interval_ms),
            ),
        ]
    }

    /// 配置菜单键盘处理(§4.5 收尾)——↑↓/kj 移选,Enter 就地循环选中项,Esc/q 关
    fn handle_config_menu_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.state.popup_stack.pop();
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.state.popup_stack.move_config_menu_selection(false);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.state.popup_stack.move_config_menu_selection(true);
            }
            KeyCode::Enter => {
                if let Some(idx) = self.state.popup_stack.config_menu_selected() {
                    self.cycle_config_item(idx);
                    // 循环后刷新条目显示当前值,菜单常驻以便连续编辑
                    let entries = self.config_menu_entries();
                    self.state.popup_stack.set_config_menu_entries(entries);
                }
            }
            _ => {}
        }
    }

    /// 循环指定配置项(0=主题即时生效,1=占比即时生效,2=tick 重启生效)
    ///
    /// WHY 仅这 3 项:核验确认仅 theme/main_panel_ratio/tick_interval_ms 可运行时安全修改;
    /// 下标顺序须与 `config_menu_entries` 一致。
    fn cycle_config_item(&mut self, idx: usize) {
        match idx {
            0 => {
                self.config.theme = self.config.theme.next();
                // 主题即时生效:全面板 mark_dirty 触发下一帧重绘
                for panel_id in self.focus_manager.panels() {
                    self.state.mark_dirty(*panel_id);
                }
            }
            1 => self.main_panel_ratio = ratio_preset_next(self.main_panel_ratio),
            2 => self.config.tick_interval_ms = tick_preset_next(self.config.tick_interval_ms),
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
        } else if let Some(quest_id) = cmd.strip_prefix("cancel:") {
            self.publish_cancel_request(quest_id);
        } else if let Some(ids_str) = cmd.strip_prefix("batch_pause:") {
            // 批量暂停:遍历逗号分隔的 quest_id 列表,逐个发布暂停请求
            for quest_id in ids_str.split(',') {
                self.publish_pause(quest_id);
            }
        } else if let Some(ids_str) = cmd.strip_prefix("batch_resume:") {
            // 批量恢复:遍历逗号分隔的 quest_id 列表,逐个发布恢复请求
            for quest_id in ids_str.split(',') {
                self.publish_resume(quest_id);
            }
        } else if let Some(ids_str) = cmd.strip_prefix("batch_terminate:") {
            // 批量终止:遍历逗号分隔的 quest_id 列表,逐个发布终止请求
            for quest_id in ids_str.split(',') {
                self.publish_terminate(quest_id);
            }
        } else if let Some(ids_str) = cmd.strip_prefix("batch_cancel:") {
            // 批量取消:遍历逗号分隔的 quest_id 列表,逐个发布取消请求
            // WHY 逐条发布而非单事件:event-bus 的 QuestCancelRequested 事件
            // 语义为单个 Quest 的取消,保持契约一致。
            for quest_id in ids_str.split(',') {
                self.publish_cancel_request(quest_id);
            }
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
        } else if let Some(format_str) = cmd.strip_prefix("export:") {
            let format = if format_str.contains("csv") {
                ExportFormat::Csv
            } else {
                ExportFormat::Json
            };
            self.perform_export(format);
        }
    }

    /// 处理导出命令 — 弹出格式选择弹窗
    fn handle_export_command(&mut self) {
        self.state.popup_stack.push(PopupKind::Confirm {
            prompt: "Export as CSV?".into(),
            on_confirm: "export:csv".into(),
            confirmed: true,
        });
    }

    /// 执行导出操作
    fn perform_export(&mut self, format: ExportFormat) {
        let data_source = &self.data_source;
        match data_source.snapshot() {
            Ok(snapshot) => {
                let now = chrono::Utc::now().format("%Y%m%d_%H%M%S").to_string();
                let home = std::env::var("USERPROFILE")
                    .or_else(|_| std::env::var("HOME"))
                    .unwrap_or_else(|_| ".".to_string());
                let export_dir = std::path::PathBuf::from(home)
                    .join(".chimera")
                    .join("exports");
                let filename = format!("quests_{}.{}", now, format.extension());
                let path = export_dir.join(&filename);
                match snapshot.export_quests_to(format, &path) {
                    Ok(()) => {
                        self.state.status_message =
                            Some((format!("Exported: {}", path.display()), Severity::Info));
                    }
                    Err(e) => {
                        self.state.status_message =
                            Some((format!("Export failed: {e}"), Severity::Error));
                    }
                }
            }
            Err(e) => {
                self.state.status_message = Some((format!("Cannot export: {e}"), Severity::Error));
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

    /// 发布 Quest 取消请求(M4 扩展)
    ///
    /// WHY 与 pause/resume 同构:复用 EventMetadata + requested_by 模式,
    /// 由 quest-engine 消费后发布 QuestCancelled 状态变更事件。
    fn publish_cancel_request(&mut self, quest_id: &str) {
        self.publish_control_event(NexusEvent::QuestCancelRequested {
            metadata: EventMetadata::new("chimera-tui"),
            quest_id: quest_id.to_string(),
            requested_by: "operator".to_string(),
        });
    }

    /// 发布 Quest 终止请求(Task 7.2 批量操作)
    ///
    /// WHY 与 pause/resume/cancel 同构:复用 EventMetadata + requested_by 模式,
    /// 由 quest-engine 消费后终止 Quest 执行。
    fn publish_terminate(&mut self, quest_id: &str) {
        self.publish_control_event(NexusEvent::QuestCancelRequested {
            metadata: EventMetadata::new("chimera-tui"),
            quest_id: quest_id.to_string(),
            requested_by: "operator".to_string(),
        });
    }

    /// 发布 Quest 优先级变更请求(M4 扩展)
    ///
    /// WHY new_priority 由面板边界检查后传入:app 层不再重复校验,
    /// 保持职责单一(面板=输入校验,app=事件发布)。
    fn publish_priority_change(&mut self, quest_id: &str, new_priority: u8) {
        self.publish_control_event(NexusEvent::QuestPriorityChanged {
            metadata: EventMetadata::new("chimera-tui"),
            quest_id: quest_id.to_string(),
            new_priority,
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

    /// 更新 FPS 移动平均(P4.4)
    ///
    /// WHY 使用移动平均:单帧耗时受 OS 调度、事件循环等待、IO 等影响波动较大,
    /// 直接显示瞬时 FPS 会让状态栏数字频繁跳动、难以阅读。固定窗口移动平均
    /// 平滑短时抖动,同时对真实帧率下降仍保持灵敏响应。
    ///
    /// WHY `VecDeque<f64>` + O(1) push/pop:窗口大小固定为 `FPS_WINDOW_SIZE`,
    /// 不需要环形缓冲区等更复杂结构,`VecDeque` 已能满足需求且语义直观。
    fn update_fps(&mut self, delta: Duration) {
        let frame_time_ms = delta.as_secs_f64() * 1000.0;
        self.frame_times.push_back(frame_time_ms);
        while self.frame_times.len() > FPS_WINDOW_SIZE {
            self.frame_times.pop_front();
        }
        if self.frame_times.is_empty() {
            self.state.fps = 0;
            return;
        }
        let avg_ms = self.frame_times.iter().sum::<f64>() / self.frame_times.len() as f64;
        // avg_ms 为 0 仅在两帧几乎同时渲染(如调试步进)时发生,避免除零,
        // 将 FPS 记为显示上限。
        self.state.fps = if avg_ms > 0.0 {
            ((1000.0 / avg_ms).round() as u16).min(FPS_DISPLAY_MAX)
        } else {
            FPS_DISPLAY_MAX
        };
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
            // M4 扩展:cancel 是破坏性操作,与 pause/resume 一致走确认流程
            TuiCommand::RequestQuestCancel(quest_id) => {
                self.state.popup_stack.push(PopupKind::control_confirm(
                    "Cancel quest",
                    &quest_id,
                    format!("cancel:{quest_id}"),
                ));
            }
            // M4 扩展:priority 调整是非破坏性操作,直接发布事件
            // WHY 不走确认流程:优先级 +/- 可逆,二次确认会增加操作摩擦
            TuiCommand::RequestQuestPriorityChange {
                quest_id,
                new_priority,
            } => {
                self.publish_priority_change(&quest_id, new_priority);
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
            // P4.3:运行时调整 tick 间隔(更新配置,下次启动 DataPipeline 时生效)
            TuiCommand::SetTickInterval(ms) => {
                self.config.tick_interval_ms = ms;
                self.state.status_message = Some((
                    format!("Tick interval set to {}ms (restart to apply)", ms),
                    crate::popup::Severity::Info,
                ));
            }
            // P5 跨面板联动:Quest→EventStream 跳转,原子完成 filter 设置 + 面板切换
            //
            // WHY 先设置 filter 再切换:确保 EventStream 面板首次渲染时
            // 即应用筛选,避免一帧全量事件闪烁后再被过滤的视觉抖动。
            // filter_keyword 复用现有 EventStream 的关键字过滤逻辑
            // (event_matches_keyword),quest_id 作为关键字可匹配事件 JSON
            // 载荷中包含该 quest_id 的所有事件(如 QuestCreated/QuestProgressUpdated 等)。
            TuiCommand::JumpToEventStream { quest_id } => {
                self.state.filter_keyword = Some(quest_id.clone());
                self.switch_panel_to(PanelId::EventStream);
                self.state.set_status(
                    format!("Jumped to EventStream, filter: {quest_id}"),
                    Severity::Info,
                );
            }
            // M3-2 TaskManagerPanel:Quest 控制命令的桥接层
            //
            // WHY 在此桥接:TaskManagerPanel 是 L10 用户面,使用 0-10 优先级范围
            // (spec 用户面友好);底层 event-bus 沿用 0-255 内部范围(Quest.priority)。
            // 桥接公式:`priority_255 = priority_10 * 25` (0→0, 10→250),
            // 保留两端极值,中段略稀疏,符合"用户面稀疏、内部精细"的直觉。
            //
            // WHY Pause/Resume/Terminate 走确认弹窗:沿用既有
            // `RequestQuestPause` 的二次确认 UX(Week 7 M4 教训);
            // 破坏性动作(Terminate)与 Pause/Resume 一致走 Confirm,避免误触。
            // SetPriority 直接发布(非破坏性,可逆),与既有
            // `RequestQuestPriorityChange` 行为对齐。
            TuiCommand::QuestControl { id, action } => {
                use crate::types::QuestAction;
                match action {
                    QuestAction::Pause => {
                        self.state.popup_stack.push(PopupKind::control_confirm(
                            "Pause quest",
                            &id,
                            format!("pause:{id}"),
                        ));
                    }
                    QuestAction::Resume => {
                        self.state.popup_stack.push(PopupKind::control_confirm(
                            "Resume quest",
                            &id,
                            format!("resume:{id}"),
                        ));
                    }
                    QuestAction::Terminate => {
                        self.state.popup_stack.push(PopupKind::control_confirm(
                            "Terminate quest",
                            &id,
                            format!("terminate:{id}"),
                        ));
                    }
                    QuestAction::SetPriority(level) => {
                        // 用户面 [0, 10] → 内部 [0, 250] 线性映射
                        // 边界已在 TaskManagerPanel 钳制,此处仍做 defensive saturate
                        // 防止未来调用方传入越界值
                        let level = level.min(10);
                        let new_priority = (level as u16 * 25).min(u8::MAX as u16) as u8;
                        self.publish_priority_change(&id, new_priority);
                    }
                }
            }
            TuiCommand::Export => {
                self.handle_export_command();
            }
            TuiCommand::DispatchAction {
                action_id,
                payload,
                source,
            } => {
                // v3.1(ADR-029)M2 增量2:三入口统一派发桥接 —— 本地即时动作直接执行,
                // 其余回退发布 TuiActionRequested,经 EventBus 交 chimera-cli 编排(M3)。
                self.dispatch_action(&action_id, payload, source);
            }
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
    ///
    /// # P4.1 增量渲染说明
    /// ratatui 的 Frame 每帧会用空白缓冲区覆盖前帧内容,因此面板渲染
    /// 本身必须每帧执行(否则对应区域会被清空)。`dirty_panels` 标记
    /// 并不跳过渲染,而是为面板内部提供"数据是否变化"的可观测信号:
    /// 面板实现可以选择在数据未变时复用上次构建的 `Text` / `Span`。
    /// 渲染结束后调用 `clear_dirty` 重置集合,保证下一帧的脏标记
    /// - 最上层:弹窗叠加
    pub fn render(&mut self, frame: &mut Frame<'_>) {
        // P4.4 FPS 统计:测量上一帧到本帧的真实耗时。
        // WHY 放在 render 开头:捕获两次渲染间的完整间隔(含事件处理与等待),
        // 这是用户实际感知到的帧率,比仅测量绘制耗时更具代表性。
        let now = Instant::now();
        let delta = now.duration_since(self.last_frame_time);
        self.last_frame_time = now;
        self.update_fps(delta);

        let area = frame.area();
        self.last_area = area;
        let chunks = self.layout(area);

        // P6.2:SinglePane 专注模式不渲染 tabs(全屏当前面板)
        // WHY 跳过渲染:SinglePane 的 layout 返回 chunks[0] = Rect::default()(空),
        // 在空 Rect 上渲染 Tabs widget 虽不 panic 但浪费 CPU,显式跳过更高效。
        if self.state.layout_mode != LayoutMode::SinglePane {
            self.render_tabs(frame, chunks[0]);
        }
        self.render_main_panel(frame, chunks[1]);

        // P6.2:SinglePane 专注模式不渲染 status_bar(全屏当前面板)
        // 但命令/搜索模式仍需渲染 command_palette(用户输入需可见)
        if self.state.input_mode != InputMode::Normal {
            self.command_palette
                .render(&self.state, chunks[2], frame.buffer_mut());
        } else if self.state.layout_mode == LayoutMode::SinglePane {
            // SinglePane:只渲染 hint_bar(快捷提示),不渲染 status_bar
            self.render_hint_bar(frame, chunks[2]);
        } else {
            // DualPane/TriplePane:将 bottom 区域拆分为 status_bar + hint_bar 双行
            let bottom_split = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(1), Constraint::Length(1)])
                .split(chunks[2]);
            self.render_status_bar(frame, bottom_split[0]);
            self.render_hint_bar(frame, bottom_split[1]);
        }

        // 弹窗叠加在最上层
        if !self.state.popup_stack.is_empty() {
            self.state.popup_stack.render(area, frame.buffer_mut());
        }

        // M2.2:统一命令面板作为居中 overlay,渲染在最上层(高于面板与状态栏)
        if self.palette.is_some() {
            self.render_palette(frame, area);
        }

        // P4.1:本帧渲染完成,重置 dirty 集合。下一帧的 `update` 会基于
        // 新一轮快照比较重新填充。
        self.state.clear_dirty();
    }

    /// 渲染统一命令面板 overlay(M2.2,用户北极星)
    ///
    /// WHY 复用自研布局引擎的 `centered_overlay`(M1.4):将 M1 的布局原语接线到
    /// M2 的实际渲染,overlay 尺寸为视口的 60%×60% 居中;标题/提示取自 i18n,
    /// 随 `Ctrl+L` 实时切换语言。渲染仍走 ratatui widget(引擎渲染路径切换属
    /// 后续 `v3-engine` 里程碑),此处只用引擎做几何计算。
    fn render_palette(&self, frame: &mut Frame<'_>, area: Rect) {
        let Some(model) = self.palette.as_ref() else {
            return;
        };
        // 用自研布局引擎计算居中 overlay 区域(engine::Rect → ratatui::Rect)
        let eng_overlay =
            crate::engine::layout::centered_overlay(crate::engine::from_ratatui_rect(area), 60, 60);
        let overlay = crate::engine::to_ratatui_rect(eng_overlay);
        // 视口过小(边框 + 三行内容至少需 3 行 4 列):放弃渲染,避免挤压
        if overlay.width < 4 || overlay.height < 3 {
            return;
        }

        // 先清空 overlay 区域,确保面板浮于底层内容之上
        frame.render_widget(Clear, overlay);

        // 外框:标题取自 i18n(随 Ctrl+L 实时切换)
        let block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" {} ", crate::t!("palette.title")));
        let inner = block.inner(overlay);
        frame.render_widget(block, overlay);

        // 内部纵向切分:查询行(1)+ 候选列表(其余)+ 提示行(1)
        let rows = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(1),
                Constraint::Length(1),
            ])
            .split(inner);

        // 查询行:`> <query>`
        let query_line = Paragraph::new(format!("> {}", model.query()))
            .style(Style::default().fg(Color::Yellow));
        frame.render_widget(query_line, rows[0]);

        // 候选列表:滚动窗口保证选中项始终可见,选中项高亮 + ▶ 标记
        let list_area = rows[1];
        let visible = list_area.height as usize;
        let sel = model.selected_index();
        let entries = model.entries();
        // WHY 滚动偏移:当选中项超出可视高度时,下滑窗口使其贴底可见
        let offset = if visible > 0 && sel >= visible {
            sel + 1 - visible
        } else {
            0
        };
        let items: Vec<ListItem> = entries
            .iter()
            .enumerate()
            .skip(offset)
            .take(visible)
            .map(|(i, e)| {
                let marker = if i == sel { "▶ " } else { "  " };
                let text = format!("{marker}{}  —  {}", e.title, e.subtitle);
                let style = if i == sel {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::White)
                };
                ListItem::new(Line::from(Span::styled(text, style)))
            })
            .collect();
        frame.render_widget(List::new(items), list_area);

        // 提示行:操作说明(随 locale 切换)
        let hint = Paragraph::new(crate::t!("palette.hint"))
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center);
        frame.render_widget(hint, rows[2]);
    }

    /// 计算当前布局,返回 [tabs, main, bottom] 三个区域
    ///
    /// WHY 独立方法:事件处理中需要知道各区域位置以响应鼠标点击,
    /// 与渲染复用同一套布局逻辑。
    ///
    /// # P6.2 布局模式
    /// - `SinglePane`:当前面板全屏,无 tabs,无 bottom(专注模式)
    /// - `DualPane`:默认布局(tabs 3 行 + main ratio% + bottom 剩余)
    /// - `TriplePane`:main 更小(70% × ratio),bottom 更大(预留 log_panel)
    fn layout(&self, area: Rect) -> [Rect; 3] {
        match self.state.layout_mode {
            // P6.2 SinglePane:当前面板全屏,无 tabs,无 bottom
            // 返回 [空, 全屏, 空],render 时跳过 tabs/status_bar 渲染
            LayoutMode::SinglePane => [Rect::default(), area, Rect::default()],
            // P6.2 DualPane / M3d VimSplit:tabs + main + bottom
            // WHY 合并:VimSplit 的外层结构与 DualPane 一致(tabs + main + bottom),
            // 其左右双分屏在 main 区内由 `pane_rects` 按 PaneMode 切分,不影响外层布局。
            LayoutMode::DualPane | LayoutMode::VimSplit => {
                let tab_and_rest = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(3), Constraint::Min(0)])
                    .split(area);

                let main_and_bottom = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Percentage((self.main_panel_ratio * 100.0) as u16),
                        // 双行状态栏(状态行 + 快捷提示行)至少占 4 行
                        Constraint::Min(4),
                    ])
                    .split(tab_and_rest[1]);

                [tab_and_rest[0], main_and_bottom[0], main_and_bottom[1]]
            }
            // P6.2 TriplePane:main 更小(70% × ratio),bottom 更大(预留 log_panel)
            // WHY 70%:DualPane 的 main 是 ratio(默认 70%),TriplePane 的 main 是
            // ratio × 0.7(默认约 49%),留出更多空间给 bottom(log_panel + status_bar)
            LayoutMode::TriplePane => {
                let tab_and_rest = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([Constraint::Length(3), Constraint::Min(0)])
                    .split(area);

                // TriplePane 的 main 占 ratio × 0.7,bottom 占剩余(更大)
                let triple_ratio = (self.main_panel_ratio * 0.7 * 100.0) as u16;
                let main_and_bottom = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Percentage(triple_ratio),
                        // 双行状态栏(状态行 + 快捷提示行)至少占 4 行
                        Constraint::Min(4),
                    ])
                    .split(tab_and_rest[1]);

                [tab_and_rest[0], main_and_bottom[0], main_and_bottom[1]]
            }
        }
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

        // M3d:按当前 PaneMode 计算可见窗格与区域,逐窗格渲染其面板。
        // 单窗格模式 / 窄视口下 panes/rects 均退化为主区一块(与既有 companion 行为等价)。
        let panes = self.pane_panels();
        let rects = self.pane_rects(area);
        for (panel_id, rect) in panes.iter().copied().zip(rects.iter().copied()) {
            if let Some(idx) = self.panel_index(panel_id) {
                self.panels[idx].render(&self.state, rect, frame.buffer_mut());
            }
        }

        // M3d:多窗格时(>1 区域)在活跃窗格边框叠加 accent 高亮,提示焦点所在。
        // WHY 仅多窗格:单窗格无需高亮(全屏即焦点),与既有单栏渲染逐字节一致。
        if rects.len() > 1 {
            let active_rect = rects.get(self.active_pane).copied().unwrap_or(rects[0]);
            let highlight = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(self.theme_accent()));
            frame.render_widget(highlight, active_rect);
        }
    }

    /// 渲染状态栏
    fn render_status_bar(&self, frame: &mut Frame<'_>, area: ratatui::layout::Rect) {
        let (status, fg) = match &self.state.status_message {
            Some((msg, severity)) => (
                format!(
                    " {}: {} | {}: {} | {}: {} | {} ",
                    crate::t!("status.panel"),
                    self.current_panel().as_str(),
                    crate::t!("status.tick"),
                    self.state.tick_mode.display(),
                    crate::t!("status.fps"),
                    self.state.fps,
                    msg
                ),
                severity.color(),
            ),
            None => (
                format!(
                    " {}: {} | {}: {} | {}: {} | {}: {} | {}: {:.0}% ",
                    crate::t!("status.panel"),
                    self.current_panel().as_str(),
                    crate::t!("status.tick"),
                    self.state.tick_mode.display(),
                    crate::t!("status.fps"),
                    self.state.fps,
                    crate::t!("status.frame"),
                    self.state.frame_count,
                    crate::t!("status.ratio"),
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

    /// 渲染键盘快捷提示栏
    ///
    /// WHY 独立方法:用户首次使用 TUI 时不知道有哪些快捷键,
    /// 底部提示栏可降低学习曲线,同时不会挤占状态信息空间。
    fn render_hint_bar(&self, frame: &mut Frame<'_>, area: ratatui::layout::Rect) {
        let span = Span::styled(crate::t!("hint.bar"), Style::default().fg(Color::DarkGray));
        let paragraph = Paragraph::new(Line::from(span)).alignment(Alignment::Right);
        frame.render_widget(paragraph, area);
    }

    /// 返回主题前景色
    fn theme_fg(&self) -> Color {
        match self.config.theme {
            Theme::Dark => Color::White,
            Theme::Light => Color::Black,
            // P6.1:HighContrast 前景为白色(纯黑白最大对比度)
            Theme::HighContrast => Color::White,
        }
    }

    /// 返回主题强调色
    fn theme_accent(&self) -> Color {
        match self.config.theme {
            Theme::Dark => Color::Cyan,
            Theme::Light => Color::Blue,
            // P6.1:HighContrast 强调色为亮黄(高饱和度,色盲友好)
            Theme::HighContrast => Color::LightYellow,
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

        // 步骤 4:退出时保存 TuiConfig 到 ~/.chimera/tui.yaml(最佳努力)
        // WHY 在终端恢复前保存:此时 self.config 仍持有运行时修改
        // (主题切换 `t`、tick 间隔调整),保存可持久化用户偏好。
        // WHY 同步 main_panel_ratio:运行时比例调整(Ctrl+Up/Down)更新的是
        // self.main_panel_ratio 而非 self.config.main_panel_ratio,
        // 保存前需同步回 config 以持久化用户调整。
        // WHY 保存失败不阻塞退出:配置持久化是最佳努力,失败仅记录警告。
        self.config.main_panel_ratio = self.main_panel_ratio;
        let config_path = TuiConfig::default_path();
        if let Err(e) = self.config.save_to_file(&config_path) {
            tracing::warn!(
                path = %config_path.display(),
                error = %e,
                "Failed to save TuiConfig on exit (non-blocking)"
            );
        }

        // 步骤 4.5:退出时保存 TuiState (最佳努力)
        if self.config.persist_state {
            if let Err(e) = self.state.save_to_file(&self.config.state_file_path) {
                tracing::warn!(
                    path = %self.config.state_file_path.display(),
                    error = %e,
                    "Failed to save TuiState on exit (non-blocking)"
                );
            }
        }

        // 步骤 5:恢复终端(无论事件循环成功与否)
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

/// 主面板占比预设循环:0.5 → 0.6 → 0.7 → 0.8 → 0.5(非预设值归入最近档后前进)
///
/// WHY 预设循环而非步进:config.edit 菜单单键循环需确定性档位;非预设值(经 Ctrl+方向
/// 步进产生)归入最近档再前进,保证循环闭合。返回值 ∈ [0.5, 0.8] 满足 validate 的 (0,1)。
fn ratio_preset_next(current: f32) -> f32 {
    const PRESETS: [f32; 4] = [0.5, 0.6, 0.7, 0.8];
    let nearest = PRESETS
        .iter()
        .enumerate()
        .min_by(|(_, a), (_, b)| (**a - current).abs().total_cmp(&(**b - current).abs()))
        .map(|(i, _)| i)
        .unwrap_or(0);
    PRESETS[(nearest + 1) % PRESETS.len()]
}

/// Tick 间隔(ms)预设循环:100 → 250 → 500 → 1000 → 100(非预设值归入最近档后前进)
///
/// 返回值 ∈ [100, 1000] 满足 TuiConfig::validate 的 tick_interval_ms 范围。
fn tick_preset_next(current: u16) -> u16 {
    const PRESETS: [u16; 4] = [100, 250, 500, 1000];
    let nearest = PRESETS
        .iter()
        .enumerate()
        .min_by_key(|(_, &p)| p.abs_diff(current))
        .map(|(i, _)| i)
        .unwrap_or(0);
    PRESETS[(nearest + 1) % PRESETS.len()]
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
            priority: 128,
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
        // M3b:FocusManager 现注册 17 面板(Chat 追加到末尾);
        // Quest 的上一个 = 列表末尾的 Chat 面板。
        assert_eq!(app.current_panel(), PanelId::Chat);
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
    fn test_handle_key_question_mark_shows_help_overlay() {
        let mut app = make_app();
        app.handle_key_event(KeyEvent::new(KeyCode::Char('?'), event::KeyModifiers::NONE));
        assert!(!app.state.popup_stack.is_empty());
        assert!(
            app.state.popup_stack.current().unwrap().is_help_overlay(),
            "'?' should open Help overlay instead of switching to Help panel"
        );
        // P3.2:不切换当前面板,焦点仍保持在 Quest
        assert_eq!(app.current_panel(), PanelId::Quest);
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
        let _locale_guard = crate::i18n::locale_test_guard();
        // i18n:面板文案随 locale 切换;固定英文捕获后复位,断言 ASCII 文案。
        crate::i18n::set_locale(crate::i18n::Locale::En);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();

        let buffer = terminal.backend().buffer();
        let content: String = buffer
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        crate::i18n::set_locale(crate::i18n::Locale::Zh);
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
    // P6.1/P6.2 handle_global_key 主题/布局切换测试
    // ============================================================

    /// P6.1.1 TDD-RED:按 `t` 键,主题从 Dark → Light
    #[test]
    fn test_handle_key_t_switches_theme_dark_to_light() {
        let mut app = make_app();
        assert_eq!(app.config().theme, Theme::Dark);
        app.handle_key_event(KeyEvent::new(KeyCode::Char('t'), event::KeyModifiers::NONE));
        assert_eq!(app.config().theme, Theme::Light);
    }

    /// P6.1.1 TDD-RED:按 `t` 键 3 次,主题循环回到 Dark
    #[test]
    fn test_handle_key_t_cycles_through_all_themes() {
        let mut app = make_app();
        // Dark → Light
        app.handle_key_event(KeyEvent::new(KeyCode::Char('t'), event::KeyModifiers::NONE));
        assert_eq!(app.config().theme, Theme::Light);
        // Light → HighContrast
        app.handle_key_event(KeyEvent::new(KeyCode::Char('t'), event::KeyModifiers::NONE));
        assert_eq!(app.config().theme, Theme::HighContrast);
        // HighContrast → Dark
        app.handle_key_event(KeyEvent::new(KeyCode::Char('t'), event::KeyModifiers::NONE));
        assert_eq!(app.config().theme, Theme::Dark);
    }

    /// P6.1.1 TDD-RED:按 `t` 键后,所有面板被标记 dirty(立即重绘)
    #[test]
    fn test_handle_key_t_marks_all_panels_dirty() {
        let mut app = make_app();
        // 初始无 dirty 面板
        assert!(app.state().dirty_panels.is_empty());
        app.handle_key_event(KeyEvent::new(KeyCode::Char('t'), event::KeyModifiers::NONE));
        // 所有已注册面板都应被标记 dirty
        assert!(!app.state().dirty_panels.is_empty());
        // 验证至少 Quest 与 Parliament 被标记(代表性断言)
        assert!(app.state().dirty_panels.contains(&PanelId::Quest));
        assert!(app.state().dirty_panels.contains(&PanelId::Parliament));
    }

    /// P6.1:按 `t` 键后,status_message 显示新主题名
    #[test]
    fn test_handle_key_t_sets_status_message() {
        let mut app = make_app();
        app.handle_key_event(KeyEvent::new(KeyCode::Char('t'), event::KeyModifiers::NONE));
        let (msg, severity) = app
            .state()
            .status_message
            .clone()
            .expect("status_message should be set");
        // status_message 标签已 i18n 化(见 tests/i18n_chrome_test.rs);
        // 此处只断言 locale 无关的主题值,避免并行测试切换 locale 造成拖动。
        assert!(
            msg.contains("light"),
            "status_message should contain 'light', got: {msg}"
        );
        assert_eq!(severity, Severity::Info);
    }

    /// P6.2:按 `l` 键,布局从 DualPane → TriplePane
    #[test]
    fn test_handle_key_l_switches_layout_dual_to_triple() {
        let mut app = make_app();
        assert_eq!(app.state().layout_mode, LayoutMode::DualPane);
        app.handle_key_event(KeyEvent::new(KeyCode::Char('l'), event::KeyModifiers::NONE));
        assert_eq!(app.state().layout_mode, LayoutMode::TriplePane);
    }

    /// M3d:按 `l` 键 4 次,布局循环回到 DualPane(纳入 VimSplit)
    #[test]
    fn test_handle_key_l_cycles_through_all_layouts() {
        let mut app = make_app();
        // DualPane → TriplePane
        app.handle_key_event(KeyEvent::new(KeyCode::Char('l'), event::KeyModifiers::NONE));
        assert_eq!(app.state().layout_mode, LayoutMode::TriplePane);
        // TriplePane → VimSplit
        app.handle_key_event(KeyEvent::new(KeyCode::Char('l'), event::KeyModifiers::NONE));
        assert_eq!(app.state().layout_mode, LayoutMode::VimSplit);
        // VimSplit → SinglePane
        app.handle_key_event(KeyEvent::new(KeyCode::Char('l'), event::KeyModifiers::NONE));
        assert_eq!(app.state().layout_mode, LayoutMode::SinglePane);
        // SinglePane → DualPane
        app.handle_key_event(KeyEvent::new(KeyCode::Char('l'), event::KeyModifiers::NONE));
        assert_eq!(app.state().layout_mode, LayoutMode::DualPane);
    }

    /// P6.2:按 `l` 键后,status_message 显示新布局名
    #[test]
    fn test_handle_key_l_sets_status_message() {
        let mut app = make_app();
        app.handle_key_event(KeyEvent::new(KeyCode::Char('l'), event::KeyModifiers::NONE));
        let (msg, severity) = app
            .state()
            .status_message
            .clone()
            .expect("status_message should be set");
        // status_message 标签已 i18n 化(见 tests/i18n_chrome_test.rs);
        // 此处只断言 locale 无关的布局值。
        assert!(
            msg.contains("triple"),
            "status_message should contain 'triple', got: {msg}"
        );
        assert_eq!(severity, Severity::Info);
    }

    /// P6.2:SinglePane 布局下 render 不崩溃(专注模式跳过 tabs/status_bar)
    #[test]
    fn test_render_single_pane_layout_no_panic() {
        let mut app = make_app();
        // 切换到 SinglePane(按 `l` 三次:Dual → Triple → VimSplit → Single,M3d 4 循环)
        app.handle_key_event(KeyEvent::new(KeyCode::Char('l'), event::KeyModifiers::NONE));
        app.handle_key_event(KeyEvent::new(KeyCode::Char('l'), event::KeyModifiers::NONE));
        app.handle_key_event(KeyEvent::new(KeyCode::Char('l'), event::KeyModifiers::NONE));
        assert_eq!(app.state().layout_mode, LayoutMode::SinglePane);

        // 渲染不应 panic(SinglePane 跳过 tabs 和 status_bar)
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();
    }

    /// P6.2:TriplePane 布局下 render 不崩溃
    #[test]
    fn test_render_triple_pane_layout_no_panic() {
        let mut app = make_app();
        // 切换到 TriplePane
        app.handle_key_event(KeyEvent::new(KeyCode::Char('l'), event::KeyModifiers::NONE));
        assert_eq!(app.state().layout_mode, LayoutMode::TriplePane);

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.draw(|f| app.render(f)).unwrap();
    }

    /// P0 交互链 Phase 2:panel.drill_down 派发进入 Focus 全屏(SinglePane)
    #[test]
    fn dispatch_drill_down_enters_focus_layout() {
        let mut app = make_app();
        assert_eq!(app.state().layout_mode, LayoutMode::DualPane);
        app.dispatch_action(
            "panel.drill_down",
            "{}".to_string(),
            event_bus::ActionSource::Palette,
        );
        assert_eq!(
            app.state().layout_mode,
            LayoutMode::SinglePane,
            "panel.drill_down 应进入 Focus 全屏(SinglePane)"
        );
    }

    /// 入口三:bare `a` 唤出焦点面板的非空上下文动作菜单(端到端:键→路由→打开)
    #[test]
    fn key_a_opens_panel_action_menu() {
        let mut app = make_app();
        app.handle_key_event(KeyEvent::new(KeyCode::Char('a'), event::KeyModifiers::NONE));
        let is_menu = matches!(
            app.state().popup_stack.current(),
            Some(PopupKind::ActionMenu { entries, .. }) if !entries.is_empty()
        );
        assert!(is_menu, "bare `a` 应唤出非空面板动作菜单");
    }

    /// 入口三:菜单 Enter 派发选中动作(用本地 arm drill_down 断言,不依赖 cli 异步)
    #[test]
    fn action_menu_enter_dispatches_selected_local_action() {
        let mut app = make_app();
        // 手工压入含本地 arm 动作(drill_down)的菜单;选中项即 drill_down
        app.state.popup_stack.push(PopupKind::action_menu(
            "Test",
            vec![("panel.drill_down".to_string(), "下钻".to_string())],
        ));
        assert_eq!(app.state().layout_mode, LayoutMode::DualPane);
        app.handle_key_event(KeyEvent::new(KeyCode::Enter, event::KeyModifiers::NONE));
        assert_eq!(
            app.state().layout_mode,
            LayoutMode::SinglePane,
            "菜单 Enter 应派发选中动作(drill_down → SinglePane)"
        );
        assert!(app.state().popup_stack.is_empty(), "派发后菜单应关闭");
    }

    /// M3 monitor.pause_sampling:派发切换冻结标志(幂等切换)
    #[test]
    fn dispatch_monitor_pause_toggles_freeze_flag() {
        let mut app = make_app();
        assert!(!app.state().monitor_paused);
        app.dispatch_action(
            "monitor.pause_sampling",
            "{}".to_string(),
            event_bus::ActionSource::Palette,
        );
        assert!(app.state().monitor_paused, "首次派发应暂停");
        app.dispatch_action(
            "monitor.pause_sampling",
            "{}".to_string(),
            event_bus::ActionSource::Palette,
        );
        assert!(!app.state().monitor_paused, "再次派发应恢复");
    }

    /// M3 monitor.time_window:派发循环时间窗(默认 Long → Short)
    #[test]
    fn dispatch_monitor_time_window_cycles() {
        let mut app = make_app();
        assert_eq!(
            app.state().monitor_window,
            crate::types::MonitorWindow::Long
        );
        app.dispatch_action(
            "monitor.time_window",
            "{}".to_string(),
            event_bus::ActionSource::Palette,
        );
        assert_eq!(
            app.state().monitor_window,
            crate::types::MonitorWindow::Short,
            "Long.next() 应为 Short"
        );
    }

    /// M3 viz.switch_dimension:ClvVector 焦点切换热图值域自适应
    #[test]
    fn dispatch_viz_switch_dimension_clv_toggles_autoscale() {
        let mut app = make_app();
        app.switch_panel_to(PanelId::ClvVector);
        assert!(!app.state().clv_heatmap_autoscale);
        app.dispatch_action(
            "viz.switch_dimension",
            "{}".to_string(),
            event_bus::ActionSource::Palette,
        );
        assert!(
            app.state().clv_heatmap_autoscale,
            "ClvVector 焦点应切换热图值域自适应"
        );
    }

    /// M3 viz.switch_dimension:OsaSparse 焦点无可切维度→诚实反馈且不误改 CLV 值域
    #[test]
    fn dispatch_viz_switch_dimension_osa_honest_no_toggle() {
        let mut app = make_app();
        app.switch_panel_to(PanelId::OsaSparse);
        app.dispatch_action(
            "viz.switch_dimension",
            "{}".to_string(),
            event_bus::ActionSource::Palette,
        );
        assert!(
            !app.state().clv_heatmap_autoscale,
            "OsaSparse 焦点不应改 CLV 值域"
        );
        let (msg, _) = app.state().status_message.clone().expect("应给诚实反馈");
        assert!(
            msg.contains("暂无可切换维度"),
            "OsaSparse 应给诚实反馈,got: {msg}"
        );
    }

    /// M3 monitor.pause_sampling:暂停时 update() 冻结 sys_metrics(不被快照覆盖)
    #[test]
    fn update_freezes_sys_metrics_when_monitor_paused() {
        // Mock 数据源固定返回 global_usage=10 的 sys_metrics
        let mut snap = DataSnapshot::default();
        snap.sys_metrics.cpu.global_usage = 10.0;
        let mut app =
            TuiApp::with_data_source(TuiConfig::default(), Box::new(MockDataSource::new(snap)))
                .unwrap();

        // 未暂停:update 刷新为 mock 值
        app.update();
        assert_eq!(app.state().sys_metrics.cpu.global_usage, 10.0);

        // 暂停后手工置可辨识冻结值,update 不应覆盖
        app.state.monitor_paused = true;
        app.state.sys_metrics.cpu.global_usage = 42.0;
        app.update();
        assert_eq!(
            app.state().sys_metrics.cpu.global_usage,
            42.0,
            "暂停时 sys_metrics 应冻结,不被 update 覆盖"
        );

        // 恢复后:update 重新刷新为 mock 值
        app.state.monitor_paused = false;
        app.update();
        assert_eq!(
            app.state().sys_metrics.cpu.global_usage,
            10.0,
            "恢复后 sys_metrics 应被 update 刷新"
        );
    }

    /// M4 view.apply_saved:apply_view_fields 仅拷贝视图偏好,不碰运行时字段
    #[test]
    fn apply_view_fields_copies_view_prefs_only() {
        let mut app = make_app();
        let mut saved = crate::types::TuiState::new();
        saved.layout_mode = LayoutMode::TriplePane;
        saved.filter_keyword = Some("q1".to_string());
        saved.monitor_window = crate::types::MonitorWindow::Short;
        saved.clv_heatmap_autoscale = true;
        saved.running = false; // 运行时字段,不应被拷贝
        app.apply_view_fields(&saved);
        assert_eq!(app.state().layout_mode, LayoutMode::TriplePane);
        assert_eq!(app.state().filter_keyword.as_deref(), Some("q1"));
        assert_eq!(
            app.state().monitor_window,
            crate::types::MonitorWindow::Short
        );
        assert!(app.state().clv_heatmap_autoscale);
        assert!(
            app.state().running,
            "running 是运行时字段,不应被视图应用覆盖"
        );
    }

    /// M4 view.apply_saved:无持久化文件时给出诚实反馈(不静默/不伪造)
    #[test]
    fn dispatch_view_apply_saved_no_file_gives_honest_status() {
        let mut app = make_app();
        // 用确定不存在的路径,保证测试确定性(不依赖真实文件)
        app.config.state_file_path =
            std::path::PathBuf::from("nonexistent_dir_xyz/no_such_view.yaml");
        app.dispatch_action(
            "view.apply_saved",
            "{}".to_string(),
            event_bus::ActionSource::Palette,
        );
        let (msg, _) = app.state().status_message.clone().expect("应给反馈");
        assert!(msg.contains("无已保存"), "无文件应给诚实反馈,got: {msg}");
    }

    /// M4 config.edit:派发打开非空配置菜单
    #[test]
    fn dispatch_config_edit_opens_config_menu() {
        let mut app = make_app();
        app.dispatch_action(
            "config.edit",
            "{}".to_string(),
            event_bus::ActionSource::Palette,
        );
        let is_menu = matches!(
            app.state().popup_stack.current(),
            Some(PopupKind::ConfigMenu { entries, .. }) if !entries.is_empty()
        );
        assert!(is_menu, "config.edit 应打开非空配置菜单");
    }

    /// M4 config.edit:菜单 Enter 就地循环选中项(默认 selected=0=主题)且菜单常驻
    #[test]
    fn config_menu_enter_cycles_selected_theme() {
        let mut app = make_app();
        app.open_config_menu();
        let before = app.config.theme.as_str();
        app.handle_key_event(KeyEvent::new(KeyCode::Enter, event::KeyModifiers::NONE));
        assert_ne!(
            app.config.theme.as_str(),
            before,
            "Enter 选中主题项应循环主题"
        );
        assert!(
            matches!(
                app.state().popup_stack.current(),
                Some(PopupKind::ConfigMenu { .. })
            ),
            "配置菜单 Enter 后应常驻(不关闭)"
        );
    }

    /// M4 config.edit:预设循环闭合 + 非预设值归最近档
    #[test]
    fn config_presets_cycle_closed_and_snap_nearest() {
        // ratio 0.7→0.8→0.5 闭合;0.72 归最近 0.7 → 0.8
        assert_eq!(ratio_preset_next(0.7), 0.8);
        assert_eq!(ratio_preset_next(0.8), 0.5);
        assert_eq!(ratio_preset_next(0.72), 0.8);
        // tick 250→500→1000→100 闭合;300 归最近 250 → 500
        assert_eq!(tick_preset_next(250), 500);
        assert_eq!(tick_preset_next(1000), 100);
        assert_eq!(tick_preset_next(300), 500);
    }

    /// Phase 3 quest.jump:单 Quest → 切事件流并按其 id 过滤(复用 JumpToEventStream)
    #[test]
    fn dispatch_quest_jump_single_quest_filters_eventstream() {
        let mut app = make_app();
        app.state.quest_list = vec![sample_quest("q1", "First")];
        app.dispatch_action(
            "quest.jump",
            "{}".to_string(),
            event_bus::ActionSource::Palette,
        );
        assert_eq!(app.current_panel(), PanelId::EventStream, "应切到事件流");
        assert_eq!(
            app.state().filter_keyword.as_deref(),
            Some("q1"),
            "单 Quest 应按其 id 过滤"
        );
    }

    /// Phase 3 quest.jump:无 Quest → 切事件流 + 诚实反馈(不臆测目标)
    #[test]
    fn dispatch_quest_jump_empty_switches_eventstream_honest() {
        let mut app = make_app();
        app.state.quest_list = vec![];
        app.dispatch_action(
            "quest.jump",
            "{}".to_string(),
            event_bus::ActionSource::Palette,
        );
        assert_eq!(app.current_panel(), PanelId::EventStream);
        let (msg, _) = app.state().status_message.clone().expect("应给反馈");
        assert!(msg.contains("无 Quest"), "空列表应诚实提示,got: {msg}");
    }

    /// Phase 3 quest.jump:多 Quest 无选中 → 切事件流 + 提示精确跳转(不臆测目标)
    #[test]
    fn dispatch_quest_jump_multi_switches_eventstream_hint() {
        let mut app = make_app();
        app.state.quest_list = vec![sample_quest("q1", "First"), sample_quest("q2", "Second")];
        // §1.3b:焦点切非 Quest 面板(无选中上下文)以测多 Quest 回退路径
        app.switch_panel_to(PanelId::Budget);
        app.dispatch_action(
            "quest.jump",
            "{}".to_string(),
            event_bus::ActionSource::Palette,
        );
        assert_eq!(app.current_panel(), PanelId::EventStream);
        let (msg, _) = app.state().status_message.clone().expect("应给反馈");
        assert!(
            msg.contains("多 Quest"),
            "多 Quest 无选中上下文应提示精确跳转,got: {msg}"
        );
    }

    /// §1.3b:焦点 Quest 面板有选中项时 quest.jump 精确跳转(不走多 Quest 回退)
    #[test]
    fn dispatch_quest_jump_precise_uses_focused_selection() {
        let mut app = make_app();
        // 默认焦点 = Quest 面板,selected=0 → 选中 q1
        app.state.quest_list = vec![sample_quest("q1", "First"), sample_quest("q2", "Second")];
        app.dispatch_action(
            "quest.jump",
            "{}".to_string(),
            event_bus::ActionSource::Palette,
        );
        assert_eq!(app.current_panel(), PanelId::EventStream);
        assert_eq!(
            app.state().filter_keyword.as_deref(),
            Some("q1"),
            "多 Quest 下焦点 Quest 选中项应精确过滤 q1"
        );
    }

    /// §1.3b:enrich_payload_with_focused_quest 三态(注入 / 不覆盖 / 透传)
    #[test]
    fn enrich_payload_with_focused_quest_three_states() {
        let mut app = make_app();
        app.state.quest_list = vec![sample_quest("q1", "First"), sample_quest("q2", "Second")];
        // 焦点 Quest(默认)+ 空 payload → 注入选中 quest_id
        let enriched = app.enrich_payload_with_focused_quest("{}".to_string());
        assert!(
            enriched.contains("q1"),
            "应注入焦点选中 quest_id,got: {enriched}"
        );
        // payload 已含 quest_id → 尊重不覆盖
        let explicit = app.enrich_payload_with_focused_quest(r#"{"quest_id":"qX"}"#.to_string());
        assert!(
            explicit.contains("qX") && !explicit.contains("q1"),
            "已含 quest_id 不应被覆盖,got: {explicit}"
        );
        // 焦点非 Quest 面板(无选中上下文)→ 透传
        app.switch_panel_to(PanelId::Budget);
        let passthrough = app.enrich_payload_with_focused_quest("{}".to_string());
        assert_eq!(passthrough, "{}", "焦点无选中上下文应透传原 payload");
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

        // M3b:标签栏宽度 80,17 个面板(含 Chat),每标签约 4 列。
        // WHY column=5:落在第 2 个标签(index 1 = Parliament)内——tab_width 为 4 或 5 时
        // 5/tab_width 均 = 1,避开边界且不受面板数微调影响。
        app.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 5,
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
