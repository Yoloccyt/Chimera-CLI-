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

use crate::actions::{ActionDomain, ActionRegistry};
use crate::types::{PanelId, TuiCommand, TuiState};

/// 从 ActionRegistry 按 domain 自动生成面板快捷键列表(Task 2.5)
///
/// 返回 `Vec<(key_name, title_key)>` 用于渲染。每条 action 的展示规则:
/// - 优先使用 `default_key`(如 "Ctrl+L")
/// - 其次使用斜杠词(如 "quest pause")
/// - 两者皆无则回退到 action id(如 "quest.pause")
///
/// WHY 自动生成:新增面板只需在 `domains/` 注册 action 并覆写 `action_domain()`,
/// `shortcuts_with_registry` 默认实现会自动调用本 helper,无需手写 shortcuts 清单。
/// 学术支撑:单一事实源原则(ADR-029 §4.2),与 codegen 三入口一致性对齐。
pub fn shortcuts_from_domain(
    registry: &ActionRegistry,
    domain: ActionDomain,
) -> Vec<(&'static str, &'static str)> {
    registry
        .by_domain(domain)
        .into_iter()
        .map(|action| {
            // default_key > slash > id:键位最直观,斜杠词次之,id 兜底
            let key = action.default_key.or(action.slash).unwrap_or(action.id);
            (key, action.title_key)
        })
        .collect()
}

pub mod budget;
pub mod chat;
pub mod chtc;
pub mod clv_vector;
/// polish-v2.7 closure Stage B-10:DAG 可视化面板(Quest 任务 DAG 层级树,北大 DataFlow)
pub mod dag_viz;
pub mod decay;
pub mod event_stream;
pub mod health;
pub mod help;
pub(crate) mod list_state;
pub mod log;
pub mod mcp_nodes;
pub mod memory;
pub mod metrics_dashboard;
pub mod osa_sparse;
pub mod parliament;
/// Task 3.7:PVL 过程评分面板 — 九维度过程评分（L10 → L7 向下依赖）
pub mod pvl_score;
pub mod quest;
pub mod resource_monitor;
pub mod router;
pub mod security;
/// polish-v2.7 P1-5:自评仪表盘面板(五维度 Harness 自我评估,ADR-049)
pub mod self_assessment;
pub mod sysinfo;
pub mod task_manager;
pub mod timeline;

pub use budget::BudgetPanel;
pub use chat::ChatPanel;
pub use chtc::ChtcPanel;
pub use clv_vector::ClvVectorPanel;
pub use dag_viz::DagVizPanel;
pub use decay::DecayPanel;
pub use event_stream::EventStreamPanel;
pub use health::HealthPanel;
pub use help::HelpPanel;
pub use log::LogPanel;
pub use mcp_nodes::McpNodesPanel;
pub use memory::MemoryPanel;
pub use metrics_dashboard::MetricsDashboardPanel;
pub use osa_sparse::OsaSparsePanel;
pub use parliament::ParliamentPanel;
pub use pvl_score::PvlScorePanel;
pub use quest::QuestPanel;
pub use resource_monitor::ResourceMonitorPanel;
pub use router::RouterPanel;
pub use security::SecurityPanel;
pub use self_assessment::SelfAssessmentPanel;
pub use sysinfo::SysinfoPanel;
pub use task_manager::TaskManagerPanel;
pub use timeline::TimelinePanel;

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

    /// 滚动到面板顶部
    ///
    /// 默认空实现;列表型面板应覆盖以将选中项/滚动偏移归零。
    /// 需要 `state` 参数:部分面板(如 EventStream)需同步 `TuiState::auto_scroll`,
    /// 列表型面板需在状态中获取当前项目数以钳制底部边界。
    fn scroll_to_top(&mut self, _state: &mut TuiState) {}

    /// 滚动到面板底部
    ///
    /// 默认空实现;列表型面板应覆盖以将选中项移到最后一项。
    /// 需要 `state` 参数的原因同 `scroll_to_top`。
    fn scroll_to_bottom(&mut self, _state: &mut TuiState) {}

    /// 返回当前面板支持的快捷键列表（键名, 描述）
    ///
    /// 默认返回空列表；面板可覆盖以提供上下文感知的快捷键帮助。
    /// 返回值用于 `PopupKind::HelpOverlay` 在 `?` 键触发时展示
    /// "面板快捷键"章节。
    ///
    /// # 示例
    /// ```ignore
    /// fn shortcuts(&self) -> Vec<(&'static str, &'static str)> {
    ///     vec![
    ///         ("↑/↓", "导航"),
    ///         ("Enter", "查看详情"),
    ///         ("P", "暂停任务"),
    ///     ]
    /// }
    /// ```
    fn shortcuts(&self) -> Vec<(&'static str, &'static str)> {
        vec![]
    }

    /// 返回面板关联的 Action 域(Task 2.5)
    ///
    /// 默认 `None`:展示型面板无关联域,`shortcuts_with_registry` 回退到 `shortcuts()`。
    /// 面板覆写返回 `Some(domain)` 后,`shortcuts_with_registry` 默认实现会调用
    /// `shortcuts_from_domain(&registry, domain)` 自动派生快捷键,无需手写 `shortcuts()`。
    ///
    /// WHY Option 而非 ActionDomain:并非所有面板都有对应域(如 HelpPanel/ChatPanel),
    /// 强制返回 domain 会迫使面板虚构域归属,违背 §4.2 分域原则。
    fn action_domain(&self) -> Option<ActionDomain> {
        None
    }

    /// 返回面板快捷键,可访问 ActionRegistry 自动派生(Task 2.5)
    ///
    /// 默认实现(渐进增强):
    /// - 先取 `shortcuts()`(面板特定的 UI 键位,如 "↑/↓" 导航)
    /// - 若 `action_domain()` 返回 `Some(d)`,追加 `shortcuts_from_domain(&registry, d)`
    ///   自动派生的功能动作快捷键(如 "Ctrl+L" 切换 locale)
    ///
    /// WHY 合并而非替换:面板 UI 键位(导航/翻页)与 ActionRegistry 功能动作
    /// (quest.pause / system.toggle_locale)语义不同,两者互补。新面板可只覆写
    /// `action_domain()`,功能动作自动接入;UI 键位按需手写 `shortcuts()`。
    ///
    /// 调用方(如 `open_help_action`)应优先使用本方法,以便新面板自动接入。
    fn shortcuts_with_registry(
        &self,
        registry: &ActionRegistry,
    ) -> Vec<(&'static str, &'static str)> {
        let mut all = self.shortcuts();
        if let Some(domain) = self.action_domain() {
            all.extend(shortcuts_from_domain(registry, domain));
        }
        all
    }

    /// 返回当前选中项的上下文 id(如选中 Quest 的 quest_id),供 quest.* 动作精确定位(§1.3b)。
    ///
    /// 默认 None(展示型面板无选中上下文);列表型面板(Quest/TaskManager)覆写返回选中项 id,
    /// 使命令面板/面板动作菜单派发的 quest.* 精确作用于焦点面板选中的 Quest。
    fn selected_context_id(&self, _state: &TuiState) -> Option<String> {
        None
    }
}
