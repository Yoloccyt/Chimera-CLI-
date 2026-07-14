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
pub mod command_palette;
pub mod config;
pub mod data;
pub mod error;
pub mod focus;
pub mod panels;
pub mod popup;
pub mod render;
pub mod subscriber;
pub mod types;

// === 关键类型重导出,简化外部导入 ===
pub use app::TuiApp;
pub use command_palette::CommandPalette;
pub use config::TuiConfig;
pub use data::{
    AsaInterventionSummary, BudgetMetrics, BudgetSync, DataPipeline, DataSnapshot,
    DataSourceConfig, HealthMetrics, MemoryMetrics, MemorySync, QuestSync, RedTeamAuditSummary,
    SecurityState, SecuritySync, SkepticVetoSummary, StubDataSource, TuiDataSource,
};
pub use error::TuiError;
pub use focus::FocusManager;
pub use panels::{
    BudgetPanel, ChtcPanel, DecayPanel, EventStreamPanel, HealthPanel, HelpPanel, LogPanel,
    McpNodesPanel, MemoryPanel, Panel, ParliamentPanel, QuestPanel, RouterPanel, SecurityPanel,
};
pub use popup::{PopupKind, PopupStack, Severity};
pub use render::{
    gauge, latency_line, sparkline, utilization_bar, virtual_scroll_window, FOOTER_TEXT,
    VIRTUAL_SCROLL_BUFFER,
};
pub use subscriber::EventSubscriber;
pub use types::{
    ChtcAdapterInfo, ChtcState, DecayMetrics, InputMode, McpNodeStatus, NodeStatus, PanelId,
    RouterMetrics, RouterStatsInfo, TimelineSnapshot, TuiCommand, TuiState,
};

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::app::TuiApp;
    pub use crate::command_palette::CommandPalette;
    pub use crate::config::TuiConfig;
    pub use crate::data::{
        BudgetMetrics, DataPipeline, DataSnapshot, DataSourceConfig, StubDataSource, TuiDataSource,
    };
    pub use crate::error::TuiError;
    pub use crate::focus::FocusManager;
    pub use crate::panels::{
        BudgetPanel, ChtcPanel, DecayPanel, EventStreamPanel, HealthPanel, HelpPanel, LogPanel,
        McpNodesPanel, MemoryPanel, Panel, ParliamentPanel, QuestPanel, RouterPanel, SecurityPanel,
    };
    pub use crate::popup::{PopupKind, PopupStack, Severity};
    pub use crate::subscriber::EventSubscriber;
    pub use crate::types::{InputMode, PanelId, TuiCommand, TuiState};
}
