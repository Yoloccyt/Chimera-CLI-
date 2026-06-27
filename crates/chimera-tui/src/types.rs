//! TUI 核心类型 — 面板类型与应用状态
//!
//! 对应架构层:L10 Interface
//! 对应创新点:无(用户交互入口)
//!
//! # 设计决策(WHY)
//! - `PanelKind` 为 enum:五面板(Quest/Parliament/Budget/Log/Help)语义清晰,
//!   匹配 §6 架构红线的"禁止功能标志"——面板是 UI 模式的离散投影
//! - `TuiState` 为状态结构体:封装当前面板、运行标志、输入缓冲,
//!   支持纯逻辑测试(无需终端)

use serde::{Deserialize, Serialize};

// ============================================================
// 面板类型 — 五面板枚举
// ============================================================

/// 面板类型 — Chimera TUI 的五个主面板
///
/// - `Quest`:Quest 任务面板,显示任务列表与进度
/// - `Parliament`:议会面板,显示议员投票与共识
/// - `Budget`:预算面板,显示预算级别与消耗
/// - `Log`:日志面板,显示系统日志流
/// - `Help`:帮助面板,显示快捷键说明
///
/// WHY Copy + PartialEq:面板类型频繁参与比较与传递,Copy 避免克隆开销。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum PanelKind {
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
}

impl PanelKind {
    /// 返回面板的人类可读名称
    pub fn as_str(&self) -> &'static str {
        match self {
            PanelKind::Quest => "Quest",
            PanelKind::Parliament => "Parliament",
            PanelKind::Budget => "Budget",
            PanelKind::Log => "Log",
            PanelKind::Help => "Help",
        }
    }

    /// 返回面板的标题(用于渲染边框)
    pub fn title(&self) -> &'static str {
        match self {
            PanelKind::Quest => " Quest Tasks ",
            PanelKind::Parliament => " Parliament ",
            PanelKind::Budget => " Budget ",
            PanelKind::Log => " System Log ",
            PanelKind::Help => " Help ",
        }
    }

    /// 切换到下一个面板(循环顺序:Quest → Parliament → Budget → Log → Help → Quest)
    pub fn next(&self) -> PanelKind {
        match self {
            PanelKind::Quest => PanelKind::Parliament,
            PanelKind::Parliament => PanelKind::Budget,
            PanelKind::Budget => PanelKind::Log,
            PanelKind::Log => PanelKind::Help,
            PanelKind::Help => PanelKind::Quest,
        }
    }

    /// 切换到上一个面板(循环顺序)
    pub fn prev(&self) -> PanelKind {
        match self {
            PanelKind::Quest => PanelKind::Help,
            PanelKind::Parliament => PanelKind::Quest,
            PanelKind::Budget => PanelKind::Parliament,
            PanelKind::Log => PanelKind::Budget,
            PanelKind::Help => PanelKind::Log,
        }
    }
}

impl std::fmt::Display for PanelKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ============================================================
// TUI 状态 — 应用运行时状态
// ============================================================

/// TUI 状态 — 应用运行时的可变状态
///
/// WHY 独立结构体:将状态与渲染逻辑分离,便于纯逻辑测试(无需终端)。
/// `running` 标志控制事件循环退出,`current_panel` 控制主面板显示。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TuiState {
    /// 当前激活的面板
    pub current_panel: PanelKind,
    /// 是否正在运行(false 时事件循环退出)
    pub running: bool,
    /// 输入缓冲(用于命令行输入)
    pub input_buffer: String,
    /// 已渲染的帧数(用于调试与性能监控)
    pub frame_count: u64,
}

impl TuiState {
    /// 创建新的初始状态(默认 Quest 面板,运行中)
    pub fn new() -> Self {
        Self {
            current_panel: PanelKind::Quest,
            running: true,
            input_buffer: String::new(),
            frame_count: 0,
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
    pub fn switch_to(&mut self, panel: PanelKind) {
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
    // PanelKind 测试
    // ============================================================

    #[test]
    fn test_panel_kind_as_str() {
        assert_eq!(PanelKind::Quest.as_str(), "Quest");
        assert_eq!(PanelKind::Parliament.as_str(), "Parliament");
        assert_eq!(PanelKind::Budget.as_str(), "Budget");
        assert_eq!(PanelKind::Log.as_str(), "Log");
        assert_eq!(PanelKind::Help.as_str(), "Help");
    }

    #[test]
    fn test_panel_kind_title() {
        assert_eq!(PanelKind::Quest.title(), " Quest Tasks ");
        assert_eq!(PanelKind::Budget.title(), " Budget ");
    }

    #[test]
    fn test_panel_kind_next() {
        assert_eq!(PanelKind::Quest.next(), PanelKind::Parliament);
        assert_eq!(PanelKind::Parliament.next(), PanelKind::Budget);
        assert_eq!(PanelKind::Budget.next(), PanelKind::Log);
        assert_eq!(PanelKind::Log.next(), PanelKind::Help);
        // 循环:Help → Quest
        assert_eq!(PanelKind::Help.next(), PanelKind::Quest);
    }

    #[test]
    fn test_panel_kind_prev() {
        assert_eq!(PanelKind::Parliament.prev(), PanelKind::Quest);
        assert_eq!(PanelKind::Budget.prev(), PanelKind::Parliament);
        assert_eq!(PanelKind::Log.prev(), PanelKind::Budget);
        assert_eq!(PanelKind::Help.prev(), PanelKind::Log);
        // 循环:Quest → Help
        assert_eq!(PanelKind::Quest.prev(), PanelKind::Help);
    }

    #[test]
    fn test_panel_kind_next_prev_roundtrip() {
        // next 再 prev 应回到原面板
        for panel in [
            PanelKind::Quest,
            PanelKind::Parliament,
            PanelKind::Budget,
            PanelKind::Log,
            PanelKind::Help,
        ] {
            assert_eq!(panel.next().prev(), panel);
            assert_eq!(panel.prev().next(), panel);
        }
    }

    #[test]
    fn test_panel_kind_display() {
        assert_eq!(PanelKind::Quest.to_string(), "Quest");
    }

    #[test]
    fn test_panel_kind_serde_roundtrip() {
        let panel = PanelKind::Budget;
        let json = serde_json::to_string(&panel).unwrap();
        let restored: PanelKind = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, panel);
    }

    // ============================================================
    // TuiState 测试
    // ============================================================

    #[test]
    fn test_state_new() {
        let state = TuiState::new();
        assert_eq!(state.current_panel, PanelKind::Quest);
        assert!(state.running);
        assert!(state.input_buffer.is_empty());
        assert_eq!(state.frame_count, 0);
    }

    #[test]
    fn test_state_switch_next() {
        let mut state = TuiState::new();
        state.switch_next();
        assert_eq!(state.current_panel, PanelKind::Parliament);
    }

    #[test]
    fn test_state_switch_prev() {
        let mut state = TuiState::new();
        state.switch_prev();
        assert_eq!(state.current_panel, PanelKind::Help);
    }

    #[test]
    fn test_state_switch_to() {
        let mut state = TuiState::new();
        state.switch_to(PanelKind::Budget);
        assert_eq!(state.current_panel, PanelKind::Budget);
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
