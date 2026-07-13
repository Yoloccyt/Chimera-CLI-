//! TUI 核心类型 — 面板标识与应用状态
//!
//! 对应架构层:L10 Interface
//! 对应创新点:无(用户交互入口)
//!
//! # 设计决策(WHY)
//! - `PanelId` 为 enum:主面板(Quest/Parliament/Budget/Log/Help)语义清晰,
//!   并预留 Memory/Security/Health 占位,为 M2 扩展做准备。
//!   匹配 §6 架构红线的"禁止功能标志"——面板是 UI 模式的离散投影。
//! - `TuiState` 为状态结构体:封装当前面板、运行标志、输入缓冲、弹窗栈等,
//!   支持纯逻辑测试(无需终端)。

use std::collections::VecDeque;

use crate::data::BudgetMetrics;
use crate::popup::{PopupStack, Severity};
use event_bus::NexusEvent;
use nexus_core::Quest;
use serde::{Deserialize, Serialize};

// ============================================================
// 面板标识 — 主面板枚举(含 M2 占位)
// ============================================================

/// 面板标识 — Chimera TUI 的主面板
///
/// - `Quest`:Quest 任务面板,显示任务列表与进度
/// - `Parliament`:议会面板,显示议员投票与共识
/// - `Budget`:预算面板,显示预算级别与消耗
/// - `Log`:日志面板,显示系统日志流
/// - `Help`:帮助面板,显示快捷键说明
/// - `Memory`/`Security`/`Health`:M2 占位,当前仅用于架构扩展
///
/// WHY Copy + PartialEq:面板标识频繁参与比较与传递,Copy 避免克隆开销。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PanelId {
    /// Quest 任务面板 — 显示任务列表与进度
    Quest,
    /// 议会面板 — 显示议员投票与共识
    Parliament,
    /// 预算面板 — 显示预算级别与消耗
    Budget,
    /// 日志面板 — 显示系统日志流
    Log,
    /// 帮助面板 — 显示快捷键说明
    Help,
    /// 记忆面板占位(M2)
    Memory,
    /// 安全面板占位(M2)
    Security,
    /// 健康面板占位(M2)
    Health,
}

impl PanelId {
    /// 返回面板的人类可读名称
    pub fn as_str(&self) -> &'static str {
        match self {
            PanelId::Quest => "Quest",
            PanelId::Parliament => "Parliament",
            PanelId::Budget => "Budget",
            PanelId::Log => "Log",
            PanelId::Help => "Help",
            PanelId::Memory => "Memory",
            PanelId::Security => "Security",
            PanelId::Health => "Health",
        }
    }

    /// 返回面板的标题(用于渲染边框)
    pub fn title(&self) -> &'static str {
        match self {
            PanelId::Quest => " Quest Tasks ",
            PanelId::Parliament => " Parliament ",
            PanelId::Budget => " Budget ",
            PanelId::Log => " System Log ",
            PanelId::Help => " Help ",
            PanelId::Memory => " Memory ",
            PanelId::Security => " Security ",
            PanelId::Health => " Health ",
        }
    }

    /// 切换到下一个面板(循环顺序)
    ///
    /// M1 仅循环 5 个主面板;占位面板不参与默认导航。
    pub fn next(&self) -> PanelId {
        match self {
            PanelId::Quest => PanelId::Parliament,
            PanelId::Parliament => PanelId::Budget,
            PanelId::Budget => PanelId::Log,
            PanelId::Log => PanelId::Help,
            PanelId::Help => PanelId::Quest,
            // 占位面板默认回到 Quest
            PanelId::Memory | PanelId::Security | PanelId::Health => PanelId::Quest,
        }
    }

    /// 切换到上一个面板(循环顺序)
    ///
    /// M1 仅循环 5 个主面板;占位面板不参与默认导航。
    pub fn prev(&self) -> PanelId {
        match self {
            PanelId::Quest => PanelId::Help,
            PanelId::Parliament => PanelId::Quest,
            PanelId::Budget => PanelId::Parliament,
            PanelId::Log => PanelId::Budget,
            PanelId::Help => PanelId::Log,
            // 占位面板默认回到 Help
            PanelId::Memory | PanelId::Security | PanelId::Health => PanelId::Help,
        }
    }
}

impl std::fmt::Display for PanelId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ============================================================
// 输入模式 — 命令面板/搜索面板/普通模式
// ============================================================

/// 输入模式 — 控制底部输入栏的行为
///
/// - `Normal`:普通模式,底部显示状态栏
/// - `Command`:命令模式(由 `:` 触发),解析并执行面板切换/退出等命令
/// - `Search`:搜索模式(由 `/` 触发),M1 为占位,仅接受输入
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum InputMode {
    /// 普通模式
    Normal,
    /// 命令模式
    Command,
    /// 搜索模式
    Search,
}

// ============================================================
// 高层命令 — 面板返回的语义化动作
// ============================================================

/// 高层命令 — 由面板或命令面板产生,由 `TuiApp` 统一解释执行
///
/// WHY 引入命令抽象:将"按键语义"与"应用动作"解耦,
/// 后续 M3/M4 的控制事件可在此基础上扩展,而不影响面板实现。
#[derive(Debug, Clone, PartialEq)]
pub enum TuiCommand {
    /// 退出应用
    Quit,
    /// 切换到指定面板
    SwitchPanel(PanelId),
    /// 显示帮助面板
    ShowHelp,
    /// 打开弹窗
    OpenPopup(crate::popup::PopupKind),
}

// ============================================================
// TUI 状态 — 应用运行时状态
// ============================================================

/// TUI 状态 — 应用运行时的可变状态
///
/// WHY 独立结构体:将状态与渲染逻辑分离,便于纯逻辑测试(无需终端)。
/// `running` 标志控制事件循环退出,`current_panel` 控制主面板显示。
///
/// WHY 移除 `Eq`: `BudgetMetrics` 包含浮点字段(f32/f64),浮点数不满足
/// `Eq`;保留 `PartialEq` 以便测试比较与快照校验。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TuiState {
    /// 当前激活的面板
    pub current_panel: PanelId,
    /// 是否正在运行(false 时事件循环退出)
    pub running: bool,
    /// 当前输入模式
    pub input_mode: InputMode,
    /// 输入缓冲(命令模式/搜索模式使用)
    pub input_buffer: String,
    /// 已渲染的帧数(用于调试与性能监控)
    pub frame_count: u64,
    /// 当前 Quest 列表(数据驱动 Quest 面板)
    pub quest_list: Vec<Quest>,
    /// 当前预算指标(数据驱动 Budget 面板)
    pub budget: BudgetMetrics,
    /// 最近事件流(数据驱动 Parliament / Log 面板)
    pub latest_events: VecDeque<NexusEvent>,
    /// 弹窗栈(详情/通知/确认)
    pub popup_stack: PopupStack,
    /// 临时状态栏消息(内容 + 严重级别)
    pub status_message: Option<(String, Severity)>,
}

impl TuiState {
    /// 创建新的初始状态(默认 Quest 面板,运行中)
    pub fn new() -> Self {
        Self {
            current_panel: PanelId::Quest,
            running: true,
            input_mode: InputMode::Normal,
            input_buffer: String::new(),
            frame_count: 0,
            quest_list: Vec::new(),
            budget: BudgetMetrics::default(),
            latest_events: VecDeque::new(),
            popup_stack: PopupStack::new(),
            status_message: None,
        }
    }

    /// 切换到下一个面板
    pub fn switch_next(&mut self) {
        self.current_panel = self.current_panel.next();
    }

    /// 切换到上一个面板
    pub fn switch_prev(&mut self) {
        self.current_panel = self.current_panel.prev();
    }

    /// 切换到指定面板
    pub fn switch_to(&mut self, panel: PanelId) {
        self.current_panel = panel;
    }

    /// 退出应用(设置 running = false)
    pub fn quit(&mut self) {
        self.running = false;
    }

    /// 追加输入到缓冲
    pub fn append_input(&mut self, ch: char) {
        self.input_buffer.push(ch);
    }

    /// 清空输入缓冲
    pub fn clear_input(&mut self) {
        self.input_buffer.clear();
    }

    /// 增加帧计数
    pub fn tick_frame(&mut self) {
        self.frame_count += 1;
    }
}

impl Default for TuiState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================
    // PanelId 测试
    // ============================================================

    #[test]
    fn test_panel_id_as_str() {
        assert_eq!(PanelId::Quest.as_str(), "Quest");
        assert_eq!(PanelId::Parliament.as_str(), "Parliament");
        assert_eq!(PanelId::Budget.as_str(), "Budget");
        assert_eq!(PanelId::Log.as_str(), "Log");
        assert_eq!(PanelId::Help.as_str(), "Help");
        assert_eq!(PanelId::Memory.as_str(), "Memory");
        assert_eq!(PanelId::Security.as_str(), "Security");
        assert_eq!(PanelId::Health.as_str(), "Health");
    }

    #[test]
    fn test_panel_id_title() {
        assert_eq!(PanelId::Quest.title(), " Quest Tasks ");
        assert_eq!(PanelId::Budget.title(), " Budget ");
    }

    #[test]
    fn test_panel_id_next() {
        assert_eq!(PanelId::Quest.next(), PanelId::Parliament);
        assert_eq!(PanelId::Parliament.next(), PanelId::Budget);
        assert_eq!(PanelId::Budget.next(), PanelId::Log);
        assert_eq!(PanelId::Log.next(), PanelId::Help);
        // 循环:Help → Quest
        assert_eq!(PanelId::Help.next(), PanelId::Quest);
    }

    #[test]
    fn test_panel_id_prev() {
        assert_eq!(PanelId::Parliament.prev(), PanelId::Quest);
        assert_eq!(PanelId::Budget.prev(), PanelId::Parliament);
        assert_eq!(PanelId::Log.prev(), PanelId::Budget);
        assert_eq!(PanelId::Help.prev(), PanelId::Log);
        // 循环:Quest → Help
        assert_eq!(PanelId::Quest.prev(), PanelId::Help);
    }

    #[test]
    fn test_panel_id_next_prev_roundtrip() {
        // next 再 prev 应回到原面板
        for panel in [
            PanelId::Quest,
            PanelId::Parliament,
            PanelId::Budget,
            PanelId::Log,
            PanelId::Help,
        ] {
            assert_eq!(panel.next().prev(), panel);
            assert_eq!(panel.prev().next(), panel);
        }
    }

    #[test]
    fn test_panel_id_display() {
        assert_eq!(PanelId::Quest.to_string(), "Quest");
    }

    #[test]
    fn test_panel_id_serde_roundtrip() {
        let panel = PanelId::Budget;
        let json = serde_json::to_string(&panel).unwrap();
        let restored: PanelId = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, panel);
    }

    // ============================================================
    // InputMode 测试
    // ============================================================

    #[test]
    fn test_input_mode_equality() {
        assert_eq!(InputMode::Normal, InputMode::Normal);
        assert_ne!(InputMode::Normal, InputMode::Command);
    }

    // ============================================================
    // TuiCommand 测试
    // ============================================================

    #[test]
    fn test_tui_command_variants() {
        let cmd = TuiCommand::SwitchPanel(PanelId::Budget);
        assert_eq!(cmd, TuiCommand::SwitchPanel(PanelId::Budget));
    }

    // ============================================================
    // TuiState 测试
    // ============================================================

    #[test]
    fn test_state_new() {
        let state = TuiState::new();
        assert_eq!(state.current_panel, PanelId::Quest);
        assert!(state.running);
        assert_eq!(state.input_mode, InputMode::Normal);
        assert!(state.input_buffer.is_empty());
        assert_eq!(state.frame_count, 0);
        assert!(state.popup_stack.is_empty());
        assert!(state.status_message.is_none());
    }

    #[test]
    fn test_state_switch_next() {
        let mut state = TuiState::new();
        state.switch_next();
        assert_eq!(state.current_panel, PanelId::Parliament);
    }

    #[test]
    fn test_state_switch_prev() {
        let mut state = TuiState::new();
        state.switch_prev();
        assert_eq!(state.current_panel, PanelId::Help);
    }

    #[test]
    fn test_state_switch_to() {
        let mut state = TuiState::new();
        state.switch_to(PanelId::Budget);
        assert_eq!(state.current_panel, PanelId::Budget);
    }

    #[test]
    fn test_state_quit() {
        let mut state = TuiState::new();
        assert!(state.running);
        state.quit();
        assert!(!state.running);
    }

    #[test]
    fn test_state_input_buffer() {
        let mut state = TuiState::new();
        state.append_input('a');
        state.append_input('b');
        state.append_input('c');
        assert_eq!(state.input_buffer, "abc");
        state.clear_input();
        assert!(state.input_buffer.is_empty());
    }

    #[test]
    fn test_state_tick_frame() {
        let mut state = TuiState::new();
        assert_eq!(state.frame_count, 0);
        state.tick_frame();
        state.tick_frame();
        state.tick_frame();
        assert_eq!(state.frame_count, 3);
    }

    #[test]
    fn test_state_serde_roundtrip() {
        let state = TuiState::new();
        let json = serde_json::to_string(&state).unwrap();
        let restored: TuiState = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, state);
    }
}
