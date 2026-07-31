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

use crate::command_palette::CommandPalette;
use crate::config::TuiConfig;
use crate::data::{StubDataSource, TuiDataSource};
use crate::error::TuiError;
use crate::focus::FocusManager;
use crate::panels::{
    BudgetPanel, ChatPanel, ChtcPanel, ClvVectorPanel, DagVizPanel, DecayPanel, EventStreamPanel,
    HealthPanel, HelpPanel, LogPanel, McpNodesPanel, MemoryPanel, MetricsDashboardPanel,
    OsaSparsePanel, Panel, ParliamentPanel, PvlScorePanel, QuestPanel, ResourceMonitorPanel,
    RouterPanel, SecurityPanel, SelfAssessmentPanel, TaskManagerPanel,
};
use crate::types::{PanelId, TuiState};
use event_bus::EventBus;

// 子模块声明(Task 1.15 拆分:新增 chat_session / fps_counter / pane_manager)
pub(crate) mod chat_session;
pub(crate) mod event_loop;
pub(crate) mod fps_counter;
pub(crate) mod mouse;
pub(crate) mod pane_manager;
pub(crate) mod render;
pub(crate) mod state;

// 重导出新结构与常量,供子模块通过 `super::` 统一访问(Task 1.15 拆分后单一来源)
// Task 1.15.4:`Rect` / `Instant` / `VecDeque` / `CommandPaletteModel` 已下沉到
// 各自归属的子模块(pane_manager / fps_counter / chat_session),mod.rs 不再直接使用。
// 常量(FPS_DISPLAY_MAX/FPS_WINDOW_SIZE/RATIO_*)仅在 #[cfg(test)] 内联测试中使用,
// 非测试构建下 re-export 无消费者,显式 allow 避免 unused_imports 噪音。
#[allow(unused_imports)]
pub(crate) use chat_session::ChatSession;
#[allow(unused_imports)]
pub(crate) use fps_counter::{FpsCounter, FPS_DISPLAY_MAX, FPS_WINDOW_SIZE};
#[allow(unused_imports)]
pub(crate) use pane_manager::{PaneManager, RATIO_MAX, RATIO_MIN, RATIO_STEP};

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
///
/// # Task 1.15 拆分后字段布局(18 → 10 字段)
/// 原 18 个字段按职责聚合到 3 个子结构体:
/// - `pane_manager: PaneManager` — 7 个窗格/布局相关字段(main_panel_ratio /
///   companion_visible / prev_panel / bound_companion / active_pane /
///   last_focused / last_area)
/// - `fps_counter: FpsCounter` — 2 个 FPS 相关字段(last_frame_time / frame_times)
/// - `chat_session: ChatSession` — 2 个会话字段(chat_session_id / palette)
///
/// WHY 聚合:单一职责 + 字段数降至 ≤10(spec 1.15.4)+ 后续扩展不膨胀 TuiApp。
pub struct TuiApp {
    /// TUI 配置(只读,构造后不变)
    config: TuiConfig,
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
    /// 命令面板(`:` 命令栏 + `/` 搜索 + 历史回溯)
    command_palette: CommandPalette,
    /// 窗格管理器 — 持有面板布局、伴随面板、活跃窗格与渲染区域等视图状态
    ///
    /// WHY 集中:7 个相关字段聚合,单一职责(窗格状态),便于后续扩展 PaneMode。
    pub(crate) pane_manager: PaneManager,
    /// FPS 计数器 — 帧时间移动平均与 FPS 计算
    ///
    /// WHY 集中:`last_frame_time` + `frame_times` 聚合,便于扩展 P95/P99 帧时间。
    pub(crate) fps_counter: FpsCounter,
    /// Chat 会话 — 持有会话标识与命令面板 overlay 状态
    ///
    /// WHY 集中:`chat_session_id` + `palette` 聚合,便于扩展多会话/命令历史。
    pub(crate) chat_session: ChatSession,
    /// 可选的事件总线引用,用于发布控制请求事件(M4 双向控制)
    ///
    /// WHY Option:测试与普通启动场景可能不需要 EventBus,避免强制依赖。
    event_bus: Option<EventBus>,
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
        // v2.9.0-omega:注册 22 个面板(23 PanelId 枚举,2 个未注册)。
        // 未注册 PanelId 原因:
        // - Timeline:P7 历史回放引擎(v1.8+) 接口占位,无对应 Panel 实现
        // - Sysinfo:数据由 ResourceMonitorPanel 承载,无需独立面板
        // FocusManager 循环顺序:Quest → Parliament → ... → OsaSparse → PvlScore → TaskManager → Quest(22 面板循环)。
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
            // Task 3.6:L10 → L6 向下依赖,OSA 稀疏度可视化面板
            // 展示 Ω-Sparse 五维掩码(Routing/Context/Memory/Audit/Budget),数据来源 osa_coordinator::five_dimension_masks()
            Box::new(OsaSparsePanel::new()),
            // M3b:Chat 面板(交互式 Agent 对话);追加到循环末尾,不改现有 Tab 次序
            Box::new(ChatPanel::new()),
            // polish-v2.7 P1-5:自评仪表盘面板(五维度 Harness 自我评估,ADR-049);
            // 追加到循环末尾,数据从 latest_events 派生,零管道侵入
            Box::new(SelfAssessmentPanel::new()),
            // closure Stage B-10:DAG 可视化面板(Quest 任务 DAG 层级树);
            // 追加到循环末尾,数据从 quest_list 派生,零管道侵入
            Box::new(DagVizPanel::new()),
            // Task 3.7:L10 → L7 向下依赖,PVL 过程评分面板
            // 展示九维度过程评分(快手 KAT,ADR-049),数据来源 pvl_layer::pvl_score()
            Box::new(PvlScorePanel::new()),
            // Task 3.9:L10 → L9 向下依赖,任务管理面板
            // 展示 Quest CRUD 控制台 + 四象限稳定分工(ADR-027),数据来源 chimera_mas::quadrant_status()
            Box::new(TaskManagerPanel::new()),
        ];
        let panel_ids: Vec<PanelId> = panels.iter().map(|p| p.id()).collect();
        let focus_manager = FocusManager::new(panel_ids);
        let state = if config.persist_state {
            TuiState::load_from_file(&config.state_file_path)
        } else {
            TuiState::new()
        };

        // Task 1.15.4:先取出 main_panel_ratio,避免 config 在结构体字面量中被 move 后再使用
        let main_panel_ratio = config.main_panel_ratio;
        Ok(Self {
            config,
            state,
            data_source,
            panels,
            focus_manager,
            command_palette: CommandPalette::new(),
            // Task 1.15.4:7 个窗格字段聚合到 PaneManager(比例从 config 初始化)
            pane_manager: PaneManager::new(main_panel_ratio),
            // Task 1.15.4:2 个 FPS 字段聚合到 FpsCounter(以当前时间为起点)
            fps_counter: FpsCounter::new(),
            // Task 1.15.4:chat_session_id + palette 聚合到 ChatSession
            // M3b:会话 id 用 uuid v7(时间有序),整个 TuiApp 生命周期复用
            chat_session: ChatSession::new(),
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
    ///
    /// Task 1.15.4:委托到 `pane_manager.main_panel_ratio`,保持外部 API 不变。
    pub fn main_panel_ratio(&self) -> f32 {
        self.pane_manager.main_panel_ratio
    }

    /// 返回当前焦点面板
    ///
    /// WHY M1 清理项 #2:`FocusManager` 是当前面板的唯一来源,
    /// 避免与 `TuiState.current_panel` 双来源不一致。
    pub fn current_panel(&self) -> PanelId {
        self.focus_manager.focused()
    }

    /// v2.9.0-omega Task 2.6:判断窄视口下是否应折叠伴随面板(响应式布局)
    ///
    /// 委托到 `PaneManager::should_collapse_companion`,集成测试与 CLI 入口
    /// 经此公开方法访问,避免暴露 `pane_manager` 字段(`pub(crate)`)。
    pub fn should_collapse_companion(&self, terminal_width: u16) -> bool {
        self.pane_manager
            .should_collapse_companion(terminal_width, self.config.responsive_collapse_threshold)
    }
}

#[cfg(test)]
mod tests;
