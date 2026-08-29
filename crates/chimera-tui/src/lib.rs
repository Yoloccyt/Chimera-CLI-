//! Chimera TUI — 基于 Ratatui 的多面板终端用户界面
//!
//! 对应架构层:L10 Interface
//! 对应创新点:无(用户交互入口)
//!
//! # 核心职责
//! - 多面板终端渲染(Quest / Parliament / Budget / Memory / Security / Health / Log / Help)
//! - 键盘事件处理(面板切换、退出、命令面板、搜索模式)
//! - 状态管理(运行状态、输入模式、弹窗栈)
//!
//! # 依赖方向(§2.2 依赖铁律)
//! Chimera TUI 是 L10 层,向下依赖 L1 的 event-bus。作为用户交互入口,
//! 不直接调用下层逻辑,通过 EventBus 订阅事件更新状态。
//!
//! # 技术选型(WHY)
//! - **ratatui 0.29**:Rust 生态最成熟的 TUI 框架,纯 Rust 实现契合
//!   `#![forbid(unsafe_code)]` 安全哲学;提供 Widget trait 组合式布局,
//!   支持 8 面板并行渲染(Quest/Parliament/Budget/Memory/Security/Health/Log/Help)。
//! - **crossterm 0.28**:跨平台终端后端(Windows/macOS/Linux),
//!   0.28 版本 KeyEvent API 变更为 `KeyEvent::new(code, modifiers)` 双参数,
//!   Release 事件需 `KeyEvent::new_with_kind(code, modifiers, KeyEventKind::Release)`。
//!   选 crossterm 而非 termion 因其 Windows 原生支持(项目主开发平台为 Windows)。
//!
//! # 快速示例
//! ```no_run
//! use chimera_tui::{TuiApp, TuiConfig};
//!
//! let mut app = TuiApp::new(TuiConfig::default()).unwrap();
//! app.run().unwrap(); // 启动 TUI 事件循环
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

pub mod app;
// === v3.1 交互式重构(ADR-029)新增模块 ===
// Action 统一层 / 组件系统 / 自研渲染引擎 / i18n / 输入路由
// M0-M1 默认编译供 CI 类型检查,M2 起经 v3-engine feature 切换实际渲染路径。
pub mod actions;
pub mod approval_mode;
pub mod chat_cards;
pub mod chat_mode;
pub mod command_palette;
pub mod components;
pub mod composer_history;
pub mod config;
pub mod data;
pub mod engine;
pub mod error;
pub mod focus;
pub mod i18n;
pub mod input;
pub mod mention;
pub mod mode_banner;
pub mod panels;
pub mod popup;
pub mod render;
pub mod rewind;
pub mod slash_surface;
pub mod subscriber;
pub mod types;
pub mod viz;

// === 关键类型重导出,简化外部导入 ===
pub use app::TuiApp;
pub use command_palette::CommandPalette;
// Concord W2:斜杠命令补全面板与解析器(命令翻转基础设施)
pub use slash_surface::{candidates as slash_candidates, SlashCandidate};
// P6.1/P6.3:重导出 Theme / ThemeColors / ColorKind / ColorScheme,供 CLI 入口
// 与下游 crate 在运行时切换主题、查询颜色方案、细粒度覆盖颜色
// (如 chimera-cli 根据配置渲染启动画面)。
pub use config::{
    tui_bible::{KeyBinding, LayoutTemplate, TuiBible},
    ColorKind, ColorScheme, Theme, ThemeColors, TuiConfig,
};
pub use data::{
    metrics_history::MetricsHistory,
    resource_history::{gradient_color, MetricSample, ResourceHistory, ThresholdLevel},
    AsaInterventionSummary, BudgetMetrics, BudgetSync, CriticalDroppedSync, DataPipeline,
    DataSnapshot, DataSourceConfig, HealthMetrics, MemoryMetrics, MemorySync, ProtocolDataSource,
    QuestSync, RedTeamAuditSummary, SecurityState, SecuritySync, SkepticVetoSummary,
    StubDataSource, TuiDataSource,
};
pub use error::TuiError;
pub use focus::FocusManager;
pub use panels::{
    BudgetPanel, ChatPanel, ChtcPanel, DecayPanel, EventStreamPanel, HealthPanel, HelpPanel,
    LogPanel, McpNodesPanel, MemoryPanel, MetricsDashboardPanel, Panel, ParliamentPanel,
    QuestPanel, ResourceMonitorPanel, RouterPanel, SecurityPanel, SysinfoPanel, TaskManagerPanel,
    TimelinePanel,
};
pub use popup::{PopupKind, PopupStack, Severity};
pub use render::{
    gauge, gauge_thresholded, horizontal_bar_chart, latency_line, sparkline, sparkline_dual,
    sparkline_dual_colored, sparkline_thresholded, utilization_bar, virtual_scroll_window,
    GaugeThreshold, FOOTER_TEXT, VIRTUAL_SCROLL_BUFFER,
};
pub use subscriber::EventSubscriber;
pub use types::{
    ChatMessage, ChatRole, ChtcAdapterInfo, ChtcState, DecayMetrics, InputMode, LayoutMode,
    McpNodeStatus, NodeStatus, PanelId, QuestAction, RouterMetrics, RouterStatsInfo, SortMode,
    TimelineSnapshot, TuiCommand, TuiState, ViewMode,
};
// P9.1:重导出 viz 公共 API(5 个高阶图表 widget + VizChartKind 枚举 + VizWidget trait),
// 供 MetricsDashboardPanel / 外部测试 / 命令面板预览使用
// NOTE: viz::gauge 名称与 render::gauge 冲突,不在 lib.rs 顶层重导出,
// 调用方通过 `chimera_tui::viz::gauge` 访问(避免命名空间污染)
pub use viz::{bar_chart, heatmap, histogram, line_chart, VizChartKind, VizWidget};

// === v3.1 交互式重构(ADR-029)关键类型重导出 ===
// engine 的 Rect/Style/Color/Buffer 与 ratatui 同名,不在顶层重导出以避免命名
// 空间污染,调用方经 `chimera_tui::engine::*` 访问;此处只导出无歧义的高层类型。
pub use actions::{ActionDescriptor, ActionDomain, ActionRegistry};
// Concord 重构 T1.1:斜杠命令独立注册表(R9 解法,命令表不占动作 40 项预算)
pub use actions::{SlashCommandDesc, SlashCommandRegistry, SlashDomain, SlashTier};
// Concord W3 T3.2:会话模式视图编排(Chat 第一默认,ADR-076)
pub use chat_mode::{split_chat_layout, ChatLayout};
// Concord W3 T3.3:会话流内嵌卡片(计划卡/失败复盘卡)
pub use chat_cards::{
    derive_quest_plan_card, derive_reflection_card, render_quest_plan_card, render_reflection_card,
    PlanStep, QuestPlanCard, ReflectionCard,
};
// Concord W4 T4.1:审批模式三态状态机(ADR-074)
pub use approval_mode::ApprovalMode;
// Concord W4 T4.5:Esc Esc 回退检测器与 @ 引用候选
pub use components::{ComponentPanel, LayoutNode, ViewContext};
// Concord W6 T6.2:composer 历史导航器(↑↓ 回溯纯函数状态机)
pub use composer_history::{ComposerHistory, HISTORY_CAPACITY};
pub use i18n::{current_locale, set_locale, toggle_locale, Locale};
pub use input::{InputRouter, RouteTarget, RouterMode};
pub use mention::{extract_mention_tail, mention_candidates, MAX_MENTION_CANDIDATES};
// Concord W6 T6.1:模式常驻横幅(Plan/Auto 态 Chat 视图一行警示)
pub use mode_banner::{banner_line, render_banner};
pub use rewind::{is_double_esc, DOUBLE_ESC_WINDOW_MS};

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::app::TuiApp;
    pub use crate::command_palette::CommandPalette;
    // prelude 只暴露 Theme(用户频繁切换),ColorKind/ThemeColors 用于细粒度
    // 颜色定制,使用频率低,不放入 prelude 避免命名空间污染。
    pub use crate::config::{Theme, TuiConfig};
    pub use crate::data::{
        resource_history::{ResourceHistory, ThresholdLevel},
        BudgetMetrics, DataPipeline, DataSnapshot, DataSourceConfig, StubDataSource, TuiDataSource,
    };
    pub use crate::error::TuiError;
    pub use crate::focus::FocusManager;
    pub use crate::panels::{
        BudgetPanel, ChatPanel, ChtcPanel, DecayPanel, EventStreamPanel, HealthPanel, HelpPanel,
        LogPanel, McpNodesPanel, MemoryPanel, Panel, ParliamentPanel, QuestPanel,
        ResourceMonitorPanel, RouterPanel, SecurityPanel, TimelinePanel,
    };
    pub use crate::popup::{PopupKind, PopupStack, Severity};
    pub use crate::subscriber::EventSubscriber;
    // P6.2:LayoutMode 加入 prelude,便于 CLI 入口与测试用 `use prelude::*` 直接构造
    pub use crate::types::{InputMode, LayoutMode, PanelId, TuiCommand, TuiState};
}
