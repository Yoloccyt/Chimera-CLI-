//! TUI 面板模块 — 统一 `Panel` trait 与具体面板实现
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - `Panel` trait 将渲染与输入处理封装为统一契约,`TuiApp` 只需维护
//!   `Vec<Box<dyn Panel>>`,新增面板无需修改主循环。
//! - `handle_key`/`handle_mouse` 返回 `Option<TuiCommand>`:面板只表达
//!   "意图",由 `TuiApp` 统一执行,避免面板直接操作全局状态。
//! - trait 要求 `Send`,使得面板集合可安全跨任务边界传递(与未来 async 渲染兼容)。

use crossterm::event::{KeyEvent, MouseEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::text::Line;

use crate::types::{PanelId, TuiCommand, TuiState};

pub mod budget;
pub mod health;
pub mod help;
pub(crate) mod list_state;
pub mod log;
pub mod memory;
pub mod parliament;
pub mod quest;
pub mod security;

pub use budget::BudgetPanel;
pub use health::HealthPanel;
pub use help::HelpPanel;
pub use log::LogPanel;
pub use memory::MemoryPanel;
pub use parliament::ParliamentPanel;
pub use quest::QuestPanel;
pub use security::SecurityPanel;

/// 面板 trait — 所有 TUI 面板的统一接口
///
/// 实现者负责:
/// - 返回唯一标识 `id`
/// - 渲染自身内容到给定 `Buffer` 区域
/// - 处理键盘/鼠标事件并返回高层命令
/// - 响应焦点变化(可选,用于高亮焦点状态)
pub trait Panel: Send {
    /// 返回面板唯一标识
    fn id(&self) -> PanelId;

    /// 返回面板标题(用于标签栏/边框)
    fn title(&self) -> Line<'static>;

    /// 渲染面板内容
    fn render(&mut self, state: &TuiState, area: Rect, buf: &mut Buffer);

    /// 处理键盘事件
    ///
    /// 返回 `Some(TuiCommand)` 表示产生高层命令;`None` 表示无命令。
    fn handle_key(&mut self, key: KeyEvent, state: &mut TuiState) -> Option<TuiCommand>;

    /// 处理鼠标事件
    ///
    /// M1 未启用鼠标处理,默认返回 `None`。
    fn handle_mouse(&mut self, _mouse: MouseEvent, _state: &mut TuiState) -> Option<TuiCommand> {
        None
    }

    /// 通知面板焦点状态变化
    ///
    /// 默认空实现;需要高亮焦点状态的面板可覆盖。
    fn focus(&mut self, _focused: bool) {}
}
