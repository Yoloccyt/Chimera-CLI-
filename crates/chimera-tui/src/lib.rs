//! Chimera TUI — 基于 Ratatui 的多面板终端用户界面
//!
//! 对应架构层:L10 Interface
//! 对应创新点:无(用户交互入口)
//!
//! # 核心职责
//! - 多面板终端渲染(Quest / Parliament / Budget / Log / Help)
//! - 键盘事件处理(面板切换、退出等)
//! - 状态管理(当前面板、运行状态)
//!
//! # 依赖方向(§2.2 依赖铁律)
//! Chimera TUI 是 L10 层,向下依赖 L1 的 event-bus。作为用户交互入口,
//! 不直接调用下层逻辑,通过 EventBus 订阅事件更新状态。
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
pub mod config;
pub mod error;
pub mod types;

// === 关键类型重导出,简化外部导入 ===
pub use app::TuiApp;
pub use config::TuiConfig;
pub use error::TuiError;
pub use types::{PanelKind, TuiState};

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::app::TuiApp;
    pub use crate::config::TuiConfig;
    pub use crate::error::TuiError;
    pub use crate::types::{PanelKind, TuiState};
}
