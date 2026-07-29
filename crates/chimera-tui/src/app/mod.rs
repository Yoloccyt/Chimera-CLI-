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

use ratatui::layout::Rect;
use std::collections::VecDeque;
use std::time::Instant;

use crate::command_palette::{CommandPalette, CommandPaletteModel};
use crate::config::TuiConfig;
use crate::data::{StubDataSource, TuiDataSource};
use crate::error::TuiError;
use crate::focus::FocusManager;
use crate::panels::{
    BudgetPanel, ChatPanel, ChtcPanel, ClvVectorPanel, DagVizPanel, DecayPanel, EventStreamPanel,
    HealthPanel, HelpPanel, LogPanel, McpNodesPanel, MemoryPanel, MetricsDashboardPanel, Panel,
    ParliamentPanel, QuestPanel, ResourceMonitorPanel, RouterPanel, SecurityPanel,
    SelfAssessmentPanel,
};
use crate::types::{PanelId, TuiState};
use event_bus::EventBus;

// 子模块声明
pub(crate) mod event_loop;
pub(crate) mod mouse;
pub(crate) mod render;
pub(crate) mod state;

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
    /// 创建 TUI 应用实例,使用默认桩数据源(生产环境应改用 `with_data_source`)。
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
            // polish-v2.7 P1-5:自评仪表盘面板(五维度 Harness 自我评估,ADR-049);
            // 追加到循环末尾,数据从 latest_events 派生,零管道侵入
            Box::new(SelfAssessmentPanel::new()),
            // closure Stage B-10:DAG 可视化面板(Quest 任务 DAG 层级树);
            // 追加到循环末尾,数据从 quest_list 派生,零管道侵入
            Box::new(DagVizPanel::new()),
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
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use crossterm::event::{self, KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
    use ratatui::style::Color;
    use ratatui::Terminal;

    use super::event_loop::{ratio_preset_next, tick_preset_next};
    use super::*;
    use crate::config::Theme;
    use crate::data::{BudgetMetrics, DataSnapshot, DataSourceConfig, TuiDataSource};
    use crate::popup::PopupKind;
    use crate::types::{InputMode, LayoutMode};
    use event_bus::{EventMetadata, NexusEvent};
    use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};
    use ratatui::backend::TestBackend;

    use crate::popup::Severity;

    fn make_app() -> Result<TuiApp, TuiError> {
        TuiApp::new(TuiConfig::default())
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
    fn test_app_new() -> Result<(), Box<dyn std::error::Error>> {
        let app = make_app()?;
        assert_eq!(app.current_panel(), PanelId::Quest);
        assert!(app.state().running);
        assert_eq!(app.config().theme, Theme::Dark);
        Ok(())
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
    fn test_switch_panel_next() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
        assert_eq!(app.current_panel(), PanelId::Quest);
        app.switch_panel_next();
        assert_eq!(app.current_panel(), PanelId::Parliament);
        app.switch_panel_next();
        assert_eq!(app.current_panel(), PanelId::Budget);
        app.switch_panel_next();
        assert_eq!(app.current_panel(), PanelId::Memory);
        Ok(())
    }

    #[test]
    fn test_switch_panel_prev() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
        app.switch_panel_prev();
        // closure Stage B-10:FocusManager 现注册 19 面板(DagViz 追加到末尾);
        // Quest 的上一个 = 列表末尾的 DagViz 面板。
        assert_eq!(app.current_panel(), PanelId::DagViz);
        Ok(())
    }

    #[test]
    fn test_switch_panel_to() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
        app.switch_panel_to(PanelId::Budget);
        assert_eq!(app.current_panel(), PanelId::Budget);
        Ok(())
    }

    #[test]
    fn test_quit() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
        assert!(app.state().running);
        app.quit();
        assert!(!app.state().running);
        Ok(())
    }

    // ============================================================
    // 键盘事件处理测试
    // ============================================================

    #[test]
    fn test_handle_key_q_quits() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
        app.handle_key_event(KeyEvent::new(KeyCode::Char('q'), event::KeyModifiers::NONE));
        assert!(!app.state().running);
        Ok(())
    }

    #[test]
    fn test_handle_key_esc_quits() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
        app.handle_key_event(KeyEvent::new(KeyCode::Esc, event::KeyModifiers::NONE));
        assert!(!app.state().running);
        Ok(())
    }

    #[test]
    fn test_handle_key_tab_switches_panel() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
        app.handle_key_event(KeyEvent::new(KeyCode::Tab, event::KeyModifiers::NONE));
        assert_eq!(app.current_panel(), PanelId::Parliament);
        Ok(())
    }

    #[test]
    fn test_handle_key_number_jumps_to_panel() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
        app.handle_key_event(KeyEvent::new(KeyCode::Char('3'), event::KeyModifiers::NONE));
        assert_eq!(app.current_panel(), PanelId::Budget);
        Ok(())
    }

    #[test]
    fn test_handle_key_new_panels() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
        app.handle_key_event(KeyEvent::new(KeyCode::Char('4'), event::KeyModifiers::NONE));
        assert_eq!(app.current_panel(), PanelId::Memory);

        app.handle_key_event(KeyEvent::new(KeyCode::Char('5'), event::KeyModifiers::NONE));
        assert_eq!(app.current_panel(), PanelId::Security);

        app.handle_key_event(KeyEvent::new(KeyCode::Char('6'), event::KeyModifiers::NONE));
        assert_eq!(app.current_panel(), PanelId::Health);
        Ok(())
    }

    #[test]
    fn test_handle_key_9_jumps_to_decay() -> Result<(), Box<dyn std::error::Error>> {
        // P2 TUI v1.7-omega:数字键 9 跳转到 Decay 面板(P0 Note 第 1 节)
        let mut app = make_app()?;
        app.handle_key_event(KeyEvent::new(KeyCode::Char('9'), event::KeyModifiers::NONE));
        assert_eq!(app.current_panel(), PanelId::Decay);
        Ok(())
    }

    #[test]
    fn test_handle_key_f_keys_jump_to_panel() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
        app.handle_key_event(KeyEvent::new(KeyCode::F(2), event::KeyModifiers::NONE));
        assert_eq!(app.current_panel(), PanelId::Parliament);
        Ok(())
    }

    #[test]
    fn test_handle_key_f_keys_new_panels() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;

        app.handle_key_event(KeyEvent::new(KeyCode::F(6), event::KeyModifiers::NONE));
        assert_eq!(app.current_panel(), PanelId::Memory);

        app.handle_key_event(KeyEvent::new(KeyCode::F(7), event::KeyModifiers::NONE));
        assert_eq!(app.current_panel(), PanelId::Security);

        app.handle_key_event(KeyEvent::new(KeyCode::F(8), event::KeyModifiers::NONE));
        assert_eq!(app.current_panel(), PanelId::Health);
        Ok(())
    }

    #[test]
    fn test_handle_key_release_ignored() -> Result<(), Box<dyn std::error::Error>> {
        // WHY Windows 兼容:Release 事件应被忽略
        // 用 new_with_kind 显式指定 Release,验证 handle_key_event 的 kind 过滤
        let mut app = make_app()?;
        app.handle_key_event(KeyEvent::new_with_kind(
            KeyCode::Char('q'),
            event::KeyModifiers::NONE,
            event::KeyEventKind::Release,
        ));
        assert!(app.state().running, "Release event should be ignored");
        Ok(())
    }

    #[test]
    fn test_handle_key_command_mode() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
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
        Ok(())
    }

    #[test]
    fn test_handle_key_search_mode_sets_filter() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
        app.handle_key_event(KeyEvent::new(KeyCode::Char('/'), event::KeyModifiers::NONE));
        assert_eq!(app.state().input_mode, InputMode::Search);

        for c in "Error".chars() {
            app.handle_key_event(KeyEvent::new(KeyCode::Char(c), event::KeyModifiers::NONE));
        }
        assert_eq!(app.state().input_buffer, "Error");

        app.handle_key_event(KeyEvent::new(KeyCode::Enter, event::KeyModifiers::NONE));
        assert_eq!(app.state().input_mode, InputMode::Normal);
        assert_eq!(app.state().filter_keyword, Some("error".into()));
        Ok(())
    }

    #[test]
    fn test_handle_key_esc_cancels_command_mode() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
        app.handle_key_event(KeyEvent::new(KeyCode::Char(':'), event::KeyModifiers::NONE));
        for c in "quit".chars() {
            app.handle_key_event(KeyEvent::new(KeyCode::Char(c), event::KeyModifiers::NONE));
        }
        app.handle_key_event(KeyEvent::new(KeyCode::Esc, event::KeyModifiers::NONE));
        assert_eq!(app.state().input_mode, InputMode::Normal);
        assert!(app.state().input_buffer.is_empty());
        assert!(app.state().running);
        Ok(())
    }

    #[test]
    fn test_handle_key_question_mark_shows_help_overlay() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut app = make_app()?;
        app.handle_key_event(KeyEvent::new(KeyCode::Char('?'), event::KeyModifiers::NONE));
        assert!(!app.state.popup_stack.is_empty());
        assert!(
            app.state
                .popup_stack
                .current()
                .ok_or("expected current popup")?
                .is_help_overlay(),
            "'?' should open Help overlay instead of switching to Help panel"
        );
        // P3.2:不切换当前面板,焦点仍保持在 Quest
        assert_eq!(app.current_panel(), PanelId::Quest);
        Ok(())
    }

    #[test]
    fn test_handle_key_ctrl_up_increases_ratio() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
        let before = app.main_panel_ratio;
        app.handle_key_event(KeyEvent::new(KeyCode::Up, event::KeyModifiers::CONTROL));
        assert!(app.main_panel_ratio > before);
        Ok(())
    }

    #[test]
    fn test_handle_key_ctrl_down_decreases_ratio() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
        let before = app.main_panel_ratio;
        app.handle_key_event(KeyEvent::new(KeyCode::Down, event::KeyModifiers::CONTROL));
        assert!(app.main_panel_ratio < before);
        Ok(())
    }

    #[test]
    fn test_main_panel_ratio_bounds() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
        for _ in 0..100 {
            app.adjust_main_panel_ratio(true);
        }
        assert!((app.main_panel_ratio - RATIO_MAX).abs() < f32::EPSILON);

        for _ in 0..100 {
            app.adjust_main_panel_ratio(false);
        }
        assert!((app.main_panel_ratio - RATIO_MIN).abs() < f32::EPSILON);
        Ok(())
    }

    // ============================================================
    // 弹窗测试
    // ============================================================

    #[test]
    fn test_popup_esc_closes() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
        app.state.popup_stack.push(PopupKind::Notification {
            message: "test".into(),
            severity: crate::popup::Severity::Info,
        });
        assert!(!app.state.popup_stack.is_empty());

        app.handle_key_event(KeyEvent::new(KeyCode::Esc, event::KeyModifiers::NONE));
        assert!(app.state.popup_stack.is_empty());
        Ok(())
    }

    #[test]
    fn test_detail_popup_scroll() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
        app.state.popup_stack.push(PopupKind::Detail {
            title: "Detail".into(),
            content: "line1\nline2\nline3".into(),
            scroll: 0,
        });

        app.handle_key_event(KeyEvent::new(KeyCode::Down, event::KeyModifiers::NONE));
        assert_eq!(
            app.state
                .popup_stack
                .current()
                .ok_or("expected current popup")?
                .detail_scroll(),
            Some(1)
        );
        Ok(())
    }

    #[test]
    fn test_confirm_popup_yes_quits() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
        app.state.popup_stack.push(PopupKind::Confirm {
            prompt: "Quit?".into(),
            on_confirm: "quit".into(),
            confirmed: true,
        });

        app.handle_key_event(KeyEvent::new(KeyCode::Enter, event::KeyModifiers::NONE));
        assert!(app.state.popup_stack.is_empty());
        assert!(!app.state.running);
        Ok(())
    }

    #[test]
    fn test_confirm_popup_no_dismisses() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
        app.state.popup_stack.push(PopupKind::Confirm {
            prompt: "Quit?".into(),
            on_confirm: "quit".into(),
            confirmed: false,
        });

        app.handle_key_event(KeyEvent::new(KeyCode::Enter, event::KeyModifiers::NONE));
        assert!(app.state.popup_stack.is_empty());
        assert!(app.state.running);
        Ok(())
    }

    // ============================================================
    // 渲染测试(使用 TestBackend,无需真实终端)
    // ============================================================

    #[test]
    fn test_render_produces_output() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend)?;
        terminal.draw(|f| app.render(f))?;

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
        Ok(())
    }

    #[test]
    fn test_render_switches_panel_content() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
        app.switch_panel_next(); // Quest → Parliament

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend)?;
        terminal.draw(|f| app.render(f))?;

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
        Ok(())
    }

    #[test]
    fn test_render_memory_panel() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
        app.switch_panel_to(PanelId::Memory);

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend)?;
        terminal.draw(|f| app.render(f))?;

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
        Ok(())
    }

    #[test]
    fn test_render_security_panel() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
        app.switch_panel_to(PanelId::Security);

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend)?;
        terminal.draw(|f| app.render(f))?;

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
        Ok(())
    }

    #[test]
    fn test_render_health_panel() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
        app.switch_panel_to(PanelId::Health);

        let backend = TestBackend::new(80, 24);
        let _locale_guard = crate::i18n::locale_test_guard();
        // i18n:面板文案随 locale 切换;固定英文捕获后复位,断言 ASCII 文案。
        crate::i18n::set_locale(crate::i18n::Locale::En);
        let mut terminal = Terminal::new(backend)?;
        terminal.draw(|f| app.render(f))?;

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
        Ok(())
    }

    #[test]
    fn test_render_help_panel() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
        app.switch_panel_to(PanelId::Help);

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend)?;
        terminal.draw(|f| app.render(f))?;

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
        Ok(())
    }

    // ============================================================
    // 主题颜色测试
    // ============================================================

    #[test]
    fn test_theme_fg_dark() -> Result<(), Box<dyn std::error::Error>> {
        let app = make_app()?;
        assert_eq!(app.theme_fg(), Color::White);
        Ok(())
    }

    #[test]
    fn test_theme_fg_light() -> Result<(), Box<dyn std::error::Error>> {
        let app = TuiApp::new(TuiConfig {
            theme: Theme::Light,
            ..Default::default()
        })?;
        assert_eq!(app.theme_fg(), Color::Black);
        assert_eq!(app.theme_accent(), Color::Blue);
        Ok(())
    }

    #[test]
    fn test_theme_accent_dark() -> Result<(), Box<dyn std::error::Error>> {
        let app = make_app()?;
        assert_eq!(app.theme_accent(), Color::Cyan);
        Ok(())
    }

    // ============================================================
    // P6.1/P6.2 handle_global_key 主题/布局切换测试
    // ============================================================

    /// P6.1.1 TDD-RED:按 `t` 键,主题从 Dark → Light
    #[test]
    fn test_handle_key_t_switches_theme_dark_to_light() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
        assert_eq!(app.config().theme, Theme::Dark);
        app.handle_key_event(KeyEvent::new(KeyCode::Char('t'), event::KeyModifiers::NONE));
        assert_eq!(app.config().theme, Theme::Light);
        Ok(())
    }

    /// P6.1.1 TDD-RED:按 `t` 键 3 次,主题循环回到 Dark
    #[test]
    fn test_handle_key_t_cycles_through_all_themes() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
        // Dark → Light
        app.handle_key_event(KeyEvent::new(KeyCode::Char('t'), event::KeyModifiers::NONE));
        assert_eq!(app.config().theme, Theme::Light);
        // Light → HighContrast
        app.handle_key_event(KeyEvent::new(KeyCode::Char('t'), event::KeyModifiers::NONE));
        assert_eq!(app.config().theme, Theme::HighContrast);
        // HighContrast → Dark
        app.handle_key_event(KeyEvent::new(KeyCode::Char('t'), event::KeyModifiers::NONE));
        assert_eq!(app.config().theme, Theme::Dark);
        Ok(())
    }

    /// P6.1.1 TDD-RED:按 `t` 键后,所有面板被标记 dirty(立即重绘)
    #[test]
    fn test_handle_key_t_marks_all_panels_dirty() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
        // 初始无 dirty 面板
        assert!(app.state().dirty_panels.is_empty());
        app.handle_key_event(KeyEvent::new(KeyCode::Char('t'), event::KeyModifiers::NONE));
        // 所有已注册面板都应被标记 dirty
        assert!(!app.state().dirty_panels.is_empty());
        // 验证至少 Quest 与 Parliament 被标记(代表性断言)
        assert!(app.state().dirty_panels.contains(&PanelId::Quest));
        assert!(app.state().dirty_panels.contains(&PanelId::Parliament));
        Ok(())
    }

    /// P6.1:按 `t` 键后,status_message 显示新主题名
    #[test]
    fn test_handle_key_t_sets_status_message() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
        app.handle_key_event(KeyEvent::new(KeyCode::Char('t'), event::KeyModifiers::NONE));
        let (msg, severity) = app
            .state()
            .status_message
            .clone()
            .ok_or("status_message should be set")?;
        // status_message 标签已 i18n 化(见 tests/i18n_chrome_test.rs);
        // 此处只断言 locale 无关的主题值,避免并行测试切换 locale 造成拖动。
        assert!(
            msg.contains("light"),
            "status_message should contain 'light', got: {msg}"
        );
        assert_eq!(severity, Severity::Info);
        Ok(())
    }

    /// P6.2:按 `l` 键,布局从 DualPane → TriplePane
    #[test]
    fn test_handle_key_l_switches_layout_dual_to_triple() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut app = make_app()?;
        assert_eq!(app.state().layout_mode, LayoutMode::DualPane);
        app.handle_key_event(KeyEvent::new(KeyCode::Char('l'), event::KeyModifiers::NONE));
        assert_eq!(app.state().layout_mode, LayoutMode::TriplePane);
        Ok(())
    }

    /// M3d:按 `l` 键 4 次,布局循环回到 DualPane(纳入 VimSplit)
    #[test]
    fn test_handle_key_l_cycles_through_all_layouts() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
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
        Ok(())
    }

    /// P6.2:按 `l` 键后,status_message 显示新布局名
    #[test]
    fn test_handle_key_l_sets_status_message() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
        app.handle_key_event(KeyEvent::new(KeyCode::Char('l'), event::KeyModifiers::NONE));
        let (msg, severity) = app
            .state()
            .status_message
            .clone()
            .ok_or("status_message should be set")?;
        // status_message 标签已 i18n 化(见 tests/i18n_chrome_test.rs);
        // 此处只断言 locale 无关的布局值。
        assert!(
            msg.contains("triple"),
            "status_message should contain 'triple', got: {msg}"
        );
        assert_eq!(severity, Severity::Info);
        Ok(())
    }

    /// P6.2:SinglePane 布局下 render 不崩溃(专注模式跳过 tabs/status_bar)
    #[test]
    fn test_render_single_pane_layout_no_panic() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
        // 切换到 SinglePane(按 `l` 三次:Dual → Triple → VimSplit → Single,M3d 4 循环)
        app.handle_key_event(KeyEvent::new(KeyCode::Char('l'), event::KeyModifiers::NONE));
        app.handle_key_event(KeyEvent::new(KeyCode::Char('l'), event::KeyModifiers::NONE));
        app.handle_key_event(KeyEvent::new(KeyCode::Char('l'), event::KeyModifiers::NONE));
        assert_eq!(app.state().layout_mode, LayoutMode::SinglePane);

        // 渲染不应 panic(SinglePane 跳过 tabs 和 status_bar)
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend)?;
        terminal.draw(|f| app.render(f))?;
        Ok(())
    }

    /// P6.2:TriplePane 布局下 render 不崩溃
    #[test]
    fn test_render_triple_pane_layout_no_panic() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
        // 切换到 TriplePane
        app.handle_key_event(KeyEvent::new(KeyCode::Char('l'), event::KeyModifiers::NONE));
        assert_eq!(app.state().layout_mode, LayoutMode::TriplePane);

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend)?;
        terminal.draw(|f| app.render(f))?;
        Ok(())
    }

    /// P0 交互链 Phase 2:panel.drill_down 派发进入 Focus 全屏(SinglePane)
    #[test]
    fn dispatch_drill_down_enters_focus_layout() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
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
        Ok(())
    }

    /// 入口三:bare `a` 唤出焦点面板的非空上下文动作菜单(端到端:键→路由→打开)
    #[test]
    fn key_a_opens_panel_action_menu() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
        app.handle_key_event(KeyEvent::new(KeyCode::Char('a'), event::KeyModifiers::NONE));
        let is_menu = matches!(
            app.state().popup_stack.current(),
            Some(PopupKind::ActionMenu { entries, .. }) if !entries.is_empty()
        );
        assert!(is_menu, "bare `a` 应唤出非空面板动作菜单");
        Ok(())
    }

    /// 入口三:菜单 Enter 派发选中动作(用本地 arm drill_down 断言,不依赖 cli 异步)
    #[test]
    fn action_menu_enter_dispatches_selected_local_action() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut app = make_app()?;
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
        Ok(())
    }

    /// M3 monitor.pause_sampling:派发切换冻结标志(幂等切换)
    #[test]
    fn dispatch_monitor_pause_toggles_freeze_flag() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
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
        Ok(())
    }

    /// M3 monitor.time_window:派发循环时间窗(默认 Long → Short)
    #[test]
    fn dispatch_monitor_time_window_cycles() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
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
        Ok(())
    }

    /// M3 viz.switch_dimension:ClvVector 焦点切换热图值域自适应
    #[test]
    fn dispatch_viz_switch_dimension_clv_toggles_autoscale(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
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
        Ok(())
    }

    /// M3 viz.switch_dimension:OsaSparse 焦点无可切维度→诚实反馈且不误改 CLV 值域
    #[test]
    fn dispatch_viz_switch_dimension_osa_honest_no_toggle() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut app = make_app()?;
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
        let (msg, _) = app.state().status_message.clone().ok_or("应给诚实反馈")?;
        assert!(
            msg.contains("暂无可切换维度"),
            "OsaSparse 应给诚实反馈,got: {msg}"
        );
        Ok(())
    }

    /// M3 monitor.pause_sampling:暂停时 update() 冻结 sys_metrics(不被快照覆盖)
    #[test]
    fn update_freezes_sys_metrics_when_monitor_paused() -> Result<(), Box<dyn std::error::Error>> {
        // Mock 数据源固定返回 global_usage=10 的 sys_metrics
        let mut snap = DataSnapshot::default();
        snap.sys_metrics.cpu.global_usage = 10.0;
        let mut app =
            TuiApp::with_data_source(TuiConfig::default(), Box::new(MockDataSource::new(snap)))?;

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
        Ok(())
    }

    /// M4 view.apply_saved:apply_view_fields 仅拷贝视图偏好,不碰运行时字段
    #[test]
    fn apply_view_fields_copies_view_prefs_only() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
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
        Ok(())
    }

    /// M4 view.apply_saved:无持久化文件时给出诚实反馈(不静默/不伪造)
    #[test]
    fn dispatch_view_apply_saved_no_file_gives_honest_status(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
        // 用确定不存在的路径,保证测试确定性(不依赖真实文件)
        app.config.state_file_path =
            std::path::PathBuf::from("nonexistent_dir_xyz/no_such_view.yaml");
        app.dispatch_action(
            "view.apply_saved",
            "{}".to_string(),
            event_bus::ActionSource::Palette,
        );
        let (msg, _) = app.state().status_message.clone().ok_or("应给反馈")?;
        assert!(msg.contains("无已保存"), "无文件应给诚实反馈,got: {msg}");
        Ok(())
    }

    /// M4 config.edit:派发打开非空配置菜单
    #[test]
    fn dispatch_config_edit_opens_config_menu() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
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
        Ok(())
    }

    /// M4 config.edit:菜单 Enter 就地循环选中项(默认 selected=0=主题)且菜单常驻
    #[test]
    fn config_menu_enter_cycles_selected_theme() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
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
        Ok(())
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
    fn dispatch_quest_jump_single_quest_filters_eventstream(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
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
        Ok(())
    }

    /// Phase 3 quest.jump:无 Quest → 切事件流 + 诚实反馈(不臆测目标)
    #[test]
    fn dispatch_quest_jump_empty_switches_eventstream_honest(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
        app.state.quest_list = vec![];
        app.dispatch_action(
            "quest.jump",
            "{}".to_string(),
            event_bus::ActionSource::Palette,
        );
        assert_eq!(app.current_panel(), PanelId::EventStream);
        let (msg, _) = app.state().status_message.clone().ok_or("应给反馈")?;
        assert!(msg.contains("无 Quest"), "空列表应诚实提示,got: {msg}");
        Ok(())
    }

    /// Phase 3 quest.jump:多 Quest 无选中 → 切事件流 + 提示精确跳转(不臆测目标)
    #[test]
    fn dispatch_quest_jump_multi_switches_eventstream_hint(
    ) -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
        app.state.quest_list = vec![sample_quest("q1", "First"), sample_quest("q2", "Second")];
        // §1.3b:焦点切非 Quest 面板(无选中上下文)以测多 Quest 回退路径
        app.switch_panel_to(PanelId::Budget);
        app.dispatch_action(
            "quest.jump",
            "{}".to_string(),
            event_bus::ActionSource::Palette,
        );
        assert_eq!(app.current_panel(), PanelId::EventStream);
        let (msg, _) = app.state().status_message.clone().ok_or("应给反馈")?;
        assert!(
            msg.contains("多 Quest"),
            "多 Quest 无选中上下文应提示精确跳转,got: {msg}"
        );
        Ok(())
    }

    /// §1.3b:焦点 Quest 面板有选中项时 quest.jump 精确跳转(不走多 Quest 回退)
    #[test]
    fn dispatch_quest_jump_precise_uses_focused_selection() -> Result<(), Box<dyn std::error::Error>>
    {
        let mut app = make_app()?;
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
        Ok(())
    }

    /// §1.3b:enrich_payload_with_focused_quest 三态(注入 / 不覆盖 / 透传)
    #[test]
    fn enrich_payload_with_focused_quest_three_states() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
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
        Ok(())
    }

    // ============================================================
    // 数据接入测试
    // ============================================================

    #[test]
    fn test_with_data_source_accepts_custom_source() -> Result<(), Box<dyn std::error::Error>> {
        let app = TuiApp::with_data_source(
            TuiConfig::default(),
            Box::new(MockDataSource::new(DataSnapshot::default())),
        )?;
        assert!(app.state().quest_list.is_empty());
        assert_eq!(app.state().budget.current_tier, "High");
        Ok(())
    }

    #[test]
    fn test_update_pulls_snapshot_into_state() -> Result<(), Box<dyn std::error::Error>> {
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
        )?;
        app.update();

        assert_eq!(app.state().quest_list.len(), 1);
        assert_eq!(app.state().quest_list[0].title, "Data Driven Quest");
        assert_eq!(app.state().budget.current_tier, "Critical");
        assert_eq!(app.state().latest_events.len(), 1);
        Ok(())
    }

    #[test]
    fn test_update_sets_status_message_on_error() -> Result<(), Box<dyn std::error::Error>> {
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

        let mut app = TuiApp::with_data_source(TuiConfig::default(), Box::new(FailingDataSource))?;
        app.update();

        assert!(
            app.state().status_message.is_some(),
            "data source failure should set status message"
        );
        let (msg, severity) = app
            .state()
            .status_message
            .as_ref()
            .ok_or("expected status message")?;
        assert!(msg.contains("forced failure"));
        assert_eq!(*severity, Severity::Warning);
        Ok(())
    }

    #[test]
    fn test_quest_panel_renders_real_quest_data() -> Result<(), Box<dyn std::error::Error>> {
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
        )?;
        app.update();

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend)?;
        terminal.draw(|f| app.render(f))?;

        let buffer = terminal.backend().buffer();
        let content: String = buffer
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(content.contains("First Quest"));
        assert!(content.contains("Second Quest"));
        Ok(())
    }

    #[test]
    fn test_budget_panel_content_uses_state() -> Result<(), Box<dyn std::error::Error>> {
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
        )?;
        app.update();
        app.switch_panel_to(PanelId::Budget);

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend)?;
        terminal.draw(|f| app.render(f))?;

        let buffer = terminal.backend().buffer();
        let content: String = buffer
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(content.contains("Medium"));
        assert!(content.contains("800.0"));
        assert!(content.contains("OK"));
        Ok(())
    }

    #[test]
    fn test_log_panel_content_uses_state() -> Result<(), Box<dyn std::error::Error>> {
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
        )?;
        app.update();
        app.switch_panel_to(PanelId::Log);

        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend)?;
        terminal.draw(|f| app.render(f))?;

        let buffer = terminal.backend().buffer();
        let content: String = buffer
            .content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        assert!(content.contains("System Log"));
        assert!(content.contains("CacheHit"));
        Ok(())
    }

    // ============================================================
    // 鼠标事件测试
    // ============================================================

    #[test]
    fn test_mouse_scroll_in_main_panel() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
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
        let mut terminal = Terminal::new(backend)?;
        terminal.draw(|f| app.render(f))?;

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
        Ok(())
    }

    #[test]
    fn test_mouse_tab_click_switches_panel() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
        // 先渲染以设置 last_area
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend)?;
        terminal.draw(|f| app.render(f))?;

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
        Ok(())
    }

    #[test]
    fn test_mouse_command_bar_click_focuses() -> Result<(), Box<dyn std::error::Error>> {
        let mut app = make_app()?;
        // 先渲染以设置 last_area
        let backend = TestBackend::new(80, 24);
        let mut terminal = Terminal::new(backend)?;
        terminal.draw(|f| app.render(f))?;

        app.handle_mouse_event(MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 10,
            row: 20,
            modifiers: event::KeyModifiers::NONE,
        });
        assert_eq!(app.state().input_mode, InputMode::Command);
        Ok(())
    }
}
