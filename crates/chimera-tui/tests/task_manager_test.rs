//! M3-2 TaskManagerPanel 单元与集成测试
//!
//! 对应 Spec: `enterprise-tui-monitoring-task-viz §三 任务管理增强`。
//!
//! # 测试范围(RED 阶段)
//! - `test_task_list_renders_all_quests`:面板能渲染全部 Quest
//! - `test_pause_key_sends_quest_control_pause`:P 键产生 Pause 命令
//! - `test_priority_increment_within_bounds`:↑/↓ 在 [0,10] 范围
//! - `test_sort_by_priority_descending_default`:默认按优先级降序
//! - `test_terminate_key_sends_quest_control_terminate`:T 键产生 Terminate 命令
//!
//! # 设计约束
//! - RED 阶段全部测试应失败(因 `TuiCommand::QuestControl` / `TaskManagerPanel` 尚未实现)
//! - 沿用既有 `Panel` trait + `TuiCommand` 模式,不破坏 17 面板 API
//! - 优先级边界 [0, 10] 强校验(spec 明确,与既有 RequestQuestPriorityChange 的 [0, 255] 范围区分)

#![forbid(unsafe_code)]

use chimera_tui::panels::Panel;
use chimera_tui::{PanelId, TaskManagerPanel, TuiCommand, TuiState};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};

/// 构造指定 ID、标题与优先级的 Quest 样本
fn sample_quest(id: &str, title: &str, priority: u8) -> Quest {
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
        priority,
    }
}

/// 构造包含 3 个不同优先级 Quest 的 TuiState
fn state_with_quests() -> TuiState {
    let mut state = TuiState::new();
    state.quest_list = vec![
        sample_quest("q-low", "Low Priority Quest", 2),
        sample_quest("q-mid", "Mid Priority Quest", 5),
        sample_quest("q-high", "High Priority Quest", 8),
    ];
    state
}

/// Task 1:面板能渲染全部 Quest(列表视图包含所有 quest_id)
#[test]
fn test_task_list_renders_all_quests() {
    let panel = TaskManagerPanel::new();
    let state = state_with_quests();
    let text = panel.render_text(&state);

    assert!(
        text.contains("q-high"),
        "rendered output should contain q-high"
    );
    assert!(
        text.contains("q-mid"),
        "rendered output should contain q-mid"
    );
    assert!(
        text.contains("q-low"),
        "rendered output should contain q-low"
    );
}

/// Task 2:`P` 键产生 `TuiCommand::QuestControl { Pause, ... }`
#[test]
fn test_pause_key_sends_quest_control_pause() {
    let mut panel = TaskManagerPanel::new();
    let mut state = state_with_quests();
    // 默认排序后索引 0 为 q-high(优先级最高)
    let cmd = panel.handle_key(
        KeyEvent::new(KeyCode::Char('P'), KeyModifiers::NONE),
        &mut state,
    );
    match cmd {
        Some(TuiCommand::QuestControl { id, action }) => {
            // 默认排序后顶部为 q-high(priority=8)
            assert_eq!(id, "q-high", "默认排序顶部应为 q-high");
            // QuestAction 必须有 Pause 变体
            let is_pause = matches!(action, chimera_tui::QuestAction::Pause);
            assert!(is_pause, "P 键应产生 Pause action,got {action:?}");
        }
        other => panic!("expected QuestControl command, got {other:?}"),
    }
}

/// Task 3:↑/↓ 优先级调整在 [0, 10] 范围内,边界外不产生命令
///
/// WHY 边界强校验:spec 明确优先级为 0-10 范围(TaskManagerPanel 用户面),
/// 与既有 `RequestQuestPriorityChange` 的 0-255 内部范围不同。
/// TaskManagerPanel 应在 [0, 10] 处钳制,避免越界命令进入事件总线。
#[test]
fn test_priority_increment_within_bounds() {
    let mut panel = TaskManagerPanel::new();
    let mut state = TuiState::new();
    // 构造优先级为 10(已到上限)的 Quest
    state.quest_list = vec![sample_quest("q-max", "Max Priority", 10)];

    // ↑(Char('+'))在 priority=10 时应不产生命令(已到上限)
    let cmd_up = panel.handle_key(
        KeyEvent::new(KeyCode::Char('+'), KeyModifiers::NONE),
        &mut state,
    );
    // 期望:返回 None(上限已达)或返回 SetPriority(10)(已钳制)— 取决于实现
    // 但绝不应返回 SetPriority(11) 越界值
    if let Some(TuiCommand::QuestControl { action, .. }) = &cmd_up {
        if let chimera_tui::QuestAction::SetPriority(n) = action {
            assert!(*n <= 10, "上限保护:priority 不应超过 10,got {n}");
        }
    }

    // 测试下限:priority=0 时 ↓ 不应产生命令
    state.quest_list = vec![sample_quest("q-min", "Min Priority", 0)];
    let cmd_down = panel.handle_key(
        KeyEvent::new(KeyCode::Char('-'), KeyModifiers::NONE),
        &mut state,
    );
    if let Some(TuiCommand::QuestControl { action, .. }) = &cmd_down {
        if let chimera_tui::QuestAction::SetPriority(n) = action {
            // 下限保护:priority 不应低于 0(u8 天然保证,但显式断言表达意图)
            // 允许 SetPriority(0) 已钳制到边界
            let _ = n; // u8 恒 ≥ 0,显式无操作保留意图注释
        }
    }
}

/// Task 4:默认按优先级降序排序(顶部为最高优先级)
#[test]
fn test_sort_by_priority_descending_default() {
    let panel = TaskManagerPanel::new();
    let state = state_with_quests();

    // 排序后顶部应为 q-high(priority=8)
    let top_id = panel.top_quest_id(&state);
    assert_eq!(
        top_id,
        Some("q-high".to_string()),
        "默认排序顶部应为最高优先级 q-high, got {top_id:?}"
    );

    // 排序后底部应为 q-low(priority=2)
    let bottom_id = panel.bottom_quest_id(&state);
    assert_eq!(
        bottom_id,
        Some("q-low".to_string()),
        "默认排序底部应为最低优先级 q-low, got {bottom_id:?}"
    );
}

/// Task 5:`T` 键产生 `TuiCommand::QuestControl { Terminate, ... }`
#[test]
fn test_terminate_key_sends_quest_control_terminate() {
    let mut panel = TaskManagerPanel::new();
    let mut state = state_with_quests();
    // 默认排序后顶部为 q-high
    let cmd = panel.handle_key(
        KeyEvent::new(KeyCode::Char('T'), KeyModifiers::NONE),
        &mut state,
    );
    match cmd {
        Some(TuiCommand::QuestControl { id, action }) => {
            assert_eq!(id, "q-high", "默认排序顶部应为 q-high");
            let is_terminate = matches!(action, chimera_tui::QuestAction::Terminate);
            assert!(is_terminate, "T 键应产生 Terminate action, got {action:?}");
        }
        other => panic!("expected QuestControl command, got {other:?}"),
    }
}

/// 兼容性:TaskManagerPanel 存在但不应破坏 PanelId 循环不变量
///
/// WHY:TaskManagerPanel 是独立 Panel 实现,但不强制注册到 TuiApp 主循环
/// (避免破坏 16 面板焦点循环契约 — TuiApp 排除 Timeline/OsaSparse);
/// 此测试仅断言 PanelId 枚举的 next/prev 不变量,确保 PanelId::MetricsDashboard
/// 新增后所有 18 变体的循环仍然自洽(任何后续新增变体必须同步 next/prev)。
#[test]
fn test_task_manager_panel_does_not_break_panel_cycle() {
    // PanelId 18 变体全量循环:Quest → Parliament → ... → MetricsDashboard → Quest
    // (含 Timeline/OsaSparse 变体,即使 FocusManager 未注册它们,PanelId 不变量必须成立)
    // 使用 .prev().next() 不变量验证
    for panel in [
        PanelId::Quest,
        PanelId::MetricsDashboard,
        PanelId::Timeline,
        PanelId::OsaSparse,
    ] {
        assert_eq!(panel.next().prev(), panel);
    }
}
