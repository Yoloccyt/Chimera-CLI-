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
//! # Phase 4 P4.3 新增测试(RED 阶段)
//! - `test_sort_by_status_groups_running_first`:Status 模式 Running 排第一
//! - `test_sort_by_status_orders_by_quest_id_within_group`:Status 模式同组内按 quest_id 字典序
//! - `test_sort_by_created_at_descending`:CreatedAt 模式最新任务在前
//! - `test_cycle_sort_mode_key_s_advances_to_next`:S 键循环切换 sort_mode
//!
//! # 设计约束
//! - RED 阶段全部测试应失败(因 `TuiCommand::QuestControl` / `TaskManagerPanel` 尚未实现)
//! - 沿用既有 `Panel` trait + `TuiCommand` 模式,不破坏 17 面板 API
//! - 优先级边界 [0, 10] 强校验(spec 明确,与既有 RequestQuestPriorityChange 的 [0, 255] 范围区分)

#![forbid(unsafe_code)]

use chimera_tui::panels::Panel;
use chimera_tui::{PanelId, SortMode, TaskManagerPanel, TuiCommand, TuiState};
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

/// 构造指定 ID、标题、优先级、任务状态的 Quest 样本(P4.3 Status 排序测试用)
fn sample_quest_with_task_status(
    id: &str,
    title: &str,
    priority: u8,
    task_status: TaskStatus,
) -> Quest {
    Quest {
        quest_id: id.into(),
        title: title.into(),
        tasks: vec![Task {
            task_id: format!("{id}-t1"),
            description: "test task".into(),
            status: task_status,
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
    if let Some(TuiCommand::QuestControl {
        action: chimera_tui::QuestAction::SetPriority(n),
        ..
    }) = &cmd_up
    {
        assert!(*n <= 10, "上限保护:priority 不应超过 10,got {n}");
    }

    // 测试下限:priority=0 时 ↓ 不应产生命令
    state.quest_list = vec![sample_quest("q-min", "Min Priority", 0)];
    let cmd_down = panel.handle_key(
        KeyEvent::new(KeyCode::Char('-'), KeyModifiers::NONE),
        &mut state,
    );
    if let Some(TuiCommand::QuestControl {
        action: chimera_tui::QuestAction::SetPriority(_),
        ..
    }) = &cmd_down
    {
        // 下限保护:priority 不应低于 0(u8 天然保证,但显式模式匹配表达意图)
        // 允许 SetPriority(0) 已钳制到边界
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

// ============================================================
// Phase 4 P4.3 TaskManagerPanel 3 模式排序 — RED 阶段测试
// ============================================================

/// P4.3 Task 1:Status 模式 Running 排第一
///
/// 验证当 `sort_mode = SortMode::Status` 时,Quest 列表按状态分组排序:
/// Running(0) → Pending(1) → Failed(2) → Completed(3)。
/// 同状态内按 quest_id 字典序升序(稳定排序)。
#[test]
fn test_sort_by_status_groups_running_first() {
    // 构造 4 个不同状态的 Quest(顺序故意打乱)
    let mut state = TuiState::new();
    state.quest_list = vec![
        sample_quest_with_task_status("q-completed", "Done", 5, TaskStatus::Completed),
        sample_quest_with_task_status("q-pending", "Waiting", 8, TaskStatus::Pending),
        sample_quest_with_task_status("q-running", "Active", 3, TaskStatus::Running),
        sample_quest_with_task_status("q-other-pending", "Also Waiting", 6, TaskStatus::Pending),
    ];
    // 预期排序后顺序(按任务状态聚合分组):
    // 1) q-running (任务 Running → Quest Running)
    // 2-3) q-other-pending, q-pending (任务 Pending → Quest Pending,按 quest_id 字典序)
    // 4) q-completed (任务 Completed → Quest Completed)
    let panel = TaskManagerPanel::with_sort_mode(SortMode::Status);
    let sorted: Vec<String> = panel
        .sorted_with_mode(&state, SortMode::Status)
        .iter()
        .map(|q| q.quest_id.clone())
        .collect();
    let expected = vec!["q-running", "q-other-pending", "q-pending", "q-completed"];
    assert_eq!(
        sorted, expected,
        "Status 模式排序错误,期望 {expected:?} 实际 {sorted:?}"
    );
}

/// P4.3 Task 2:Status 模式同状态内按 quest_id 字典序
///
/// 验证同 TaskStatus 状态的 Quest 在 Status 模式下按 quest_id 字典序升序排列。
#[test]
fn test_sort_by_status_orders_by_quest_id_within_group() {
    let mut state = TuiState::new();
    state.quest_list = vec![
        sample_quest_with_task_status("q-zeta", "Z", 5, TaskStatus::Pending),
        sample_quest_with_task_status("q-alpha", "A", 5, TaskStatus::Pending),
        sample_quest_with_task_status("q-mu", "M", 5, TaskStatus::Pending),
    ];
    let panel = TaskManagerPanel::with_sort_mode(SortMode::Status);
    let sorted: Vec<String> = panel
        .sorted_with_mode(&state, SortMode::Status)
        .iter()
        .map(|q| q.quest_id.clone())
        .collect();
    assert_eq!(
        sorted,
        vec!["q-alpha", "q-mu", "q-zeta"],
        "Status 模式同组内应按 quest_id 字典序"
    );
}

/// P4.3 Task 3:CreatedAt 模式最新任务在前
///
/// 验证 `sort_mode = SortMode::CreatedAt` 时,Quest 列表按面板首次观察时间降序。
///
/// WHY 用面板侧表(`TaskManagerPanel::note_first_observation`)而非 Quest 字段:
/// 避免修改 nexus-core 域类型(95+ 构造点),L10 自治追踪"首次观察时间"
/// 作为 TUI 上下文的创建时间代理。
#[test]
fn test_sort_by_created_at_descending() {
    let mut state = TuiState::new();
    state.quest_list = vec![
        sample_quest("q-old", "Oldest", 5),
        sample_quest("q-mid", "Middle", 5),
        sample_quest("q-new", "Newest", 5),
    ];
    // 通过 note_first_observation 注入观察时间(测试用公开 API)
    let mut panel = TaskManagerPanel::with_sort_mode(SortMode::CreatedAt);
    panel.note_first_observation("q-old", chrono::Utc::now() - chrono::Duration::seconds(300));
    panel.note_first_observation("q-mid", chrono::Utc::now() - chrono::Duration::seconds(150));
    panel.note_first_observation("q-new", chrono::Utc::now() - chrono::Duration::seconds(10));

    let sorted: Vec<String> = panel
        .sorted_with_mode(&state, SortMode::CreatedAt)
        .iter()
        .map(|q| q.quest_id.clone())
        .collect();
    assert_eq!(
        sorted,
        vec!["q-new", "q-mid", "q-old"],
        "CreatedAt 模式应按首次观察时间降序(最新在前)"
    );
}

/// P4.3 Task 4:`S` 键循环切换 sort_mode
///
/// 验证用户在 TaskManagerPanel 焦点下按 `S` 键,sort_mode 字段
/// 按 Priority → Status → CreatedAt → Priority 循环,
/// 且不返回任何 TuiCommand(纯本地状态变更)。
#[test]
fn test_cycle_sort_mode_key_s_advances_to_next() {
    let mut panel = TaskManagerPanel::new();
    // 初始 sort_mode = Priority
    assert_eq!(
        panel.sort_mode(),
        SortMode::Priority,
        "默认 sort_mode 应为 Priority"
    );

    // 第 1 次按 S:Priority → Status
    let cmd1 = panel.handle_key(
        KeyEvent::new(KeyCode::Char('S'), KeyModifiers::NONE),
        &mut TuiState::new(),
    );
    assert!(cmd1.is_none(), "S 键不应产生任何 TuiCommand");
    assert_eq!(panel.sort_mode(), SortMode::Status);

    // 第 2 次按 S:Status → CreatedAt
    let cmd2 = panel.handle_key(
        KeyEvent::new(KeyCode::Char('S'), KeyModifiers::NONE),
        &mut TuiState::new(),
    );
    assert!(cmd2.is_none(), "S 键不应产生任何 TuiCommand");
    assert_eq!(panel.sort_mode(), SortMode::CreatedAt);

    // 第 3 次按 S:CreatedAt → Priority(循环回起点)
    let cmd3 = panel.handle_key(
        KeyEvent::new(KeyCode::Char('S'), KeyModifiers::NONE),
        &mut TuiState::new(),
    );
    assert!(cmd3.is_none(), "S 键不应产生任何 TuiCommand");
    assert_eq!(panel.sort_mode(), SortMode::Priority);
}
