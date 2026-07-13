//! Chimera TUI 集成测试 — 验证布局渲染、输入模式切换与键盘事件处理
//!
//! 对应 Week 8 Task 5(测试体系补齐)与 M1/M2 架构重构
//! 架构层:L10 Interface
//!
//! # 测试目标
//! - 验证多面板布局渲染(ratatui TestBackend 内存渲染,无需真实终端)
//! - 验证输入模式切换(Tab/Shift+Tab/数字键 1-8/F1-F8)
//! - 验证命令面板切换面板与退出
//! - 验证键盘事件处理(crossterm 0.28 KeyEvent::new 双参数 API)
//! - M2 新增:验证 Memory/Security/Health 面板渲染与切换
//!
//! # 设计约束
//! - WHY TestBackend 而非 CrosstermBackend:TestBackend 为内存后端,
//!   不依赖真实终端,CI 环境可运行(无 TTY)
//! - WHY KeyEvent::new(code, modifiers):crossterm 0.28 API 变更,
//!   旧的单参数构造已废弃,kind 默认为 Press,state 默认为 NONE
//! - WHY 测试 Release 事件过滤:Windows 平台 crossterm 会触发 Release,
//!   必须过滤避免重复响应(§4.4 平台兼容性)

#![forbid(unsafe_code)]

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use event_bus::{EventMetadata, NexusEvent};
use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::collections::VecDeque;

use chimera_tui::config::Theme;
use chimera_tui::{
    BudgetMetrics, DataSnapshot, DataSourceConfig, HealthMetrics, InputMode, MemoryMetrics,
    PanelId, PopupKind, SecurityState, Severity, TuiApp, TuiConfig, TuiDataSource, TuiError,
};

// ============================================================
// 辅助函数与可配置静态数据源
// ============================================================

/// 构造测试用 TUI 应用(默认配置)
fn make_app() -> TuiApp {
    TuiApp::new(TuiConfig::default()).unwrap()
}

/// 在 TestBackend 上渲染应用,返回渲染后的字符串内容
fn render_to_string(app: &mut TuiApp, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();

    let buffer = terminal.backend().buffer();
    buffer
        .content()
        .iter()
        .map(|c| c.symbol().chars().next().unwrap_or(' '))
        .collect()
}

/// 通用静态快照数据源 — M1 清理项 #3
///
/// WHY 统一 helper:替代原先 `CriticalBudgetSource`/`QuestTestSource`/
/// `ParliamentTestSource`/`LogTestSource` 四处重复实现。
#[derive(Debug)]
struct StaticSnapshotSource {
    snapshot: DataSnapshot,
    config: DataSourceConfig,
}

impl StaticSnapshotSource {
    fn new(snapshot: DataSnapshot) -> Self {
        Self {
            snapshot,
            config: DataSourceConfig::default(),
        }
    }
}

impl TuiDataSource for StaticSnapshotSource {
    fn snapshot(&self) -> Result<DataSnapshot, TuiError> {
        Ok(self.snapshot.clone())
    }

    fn config(&self) -> &DataSourceConfig {
        &self.config
    }
}

/// 构造包含完整默认字段的 DataSnapshot
fn full_snapshot(
    quest_list: Vec<Quest>,
    latest_events: VecDeque<NexusEvent>,
    budget: BudgetMetrics,
) -> DataSnapshot {
    DataSnapshot {
        quest_list,
        latest_events,
        budget_metrics: budget,
        memory_metrics: MemoryMetrics::default(),
        security_state: SecurityState::default(),
        health_metrics: HealthMetrics::default(),
        budget_history: Vec::new(),
        memory_history: Vec::new(),
        event_rate_history: Vec::new(),
    }
}

// ============================================================
// A. 布局渲染测试
// ============================================================

#[test]
fn test_tui_layout_rendering() {
    // WHY TestBackend:验证 render() 产出非空内容,包含核心 UI 元素
    let mut app = make_app();
    let content = render_to_string(&mut app, 80, 24);

    // 验证:渲染内容包含面板标签(顶部 Tabs)
    assert!(
        content.contains("Quest") || content.contains("Panels"),
        "rendered layout should contain panel tabs, got: {}",
        &content[..content.len().min(200)]
    );

    // 验证:渲染内容包含状态栏(底部)
    assert!(
        content.contains("Panel:") || content.contains("Running:"),
        "rendered layout should contain status bar"
    );

    // 验证:渲染内容包含当前面板标题(Quest Tasks)
    assert!(
        content.contains("Quest Tasks"),
        "rendered layout should contain Quest panel title"
    );
}

#[test]
fn test_tui_layout_rendering_all_panels() {
    // WHY 全面板渲染:验证 8 个面板都能正确渲染各自内容
    let panels = [
        PanelId::Quest,
        PanelId::Parliament,
        PanelId::Budget,
        PanelId::Memory,
        PanelId::Security,
        PanelId::Health,
        PanelId::Log,
        PanelId::Help,
    ];

    for panel in panels {
        let mut app = make_app();
        app.switch_panel_to(panel);
        let content = render_to_string(&mut app, 80, 24);

        // 每个面板渲染后应包含面板标题
        assert!(
            content.contains(panel.title().trim()),
            "panel {:?} should render its title '{}'",
            panel,
            panel.title().trim()
        );
    }
}

#[test]
fn test_tui_layout_rendering_theme_switch() {
    // WHY 主题切换:验证 Light/Dark 主题下都能正常渲染
    let mut dark_app = make_app();
    let dark_content = render_to_string(&mut dark_app, 80, 24);
    assert!(
        !dark_content.is_empty(),
        "Dark theme should render non-empty content"
    );

    let mut light_app = TuiApp::new(TuiConfig {
        theme: Theme::Light,
        ..Default::default()
    })
    .unwrap();
    let light_content = render_to_string(&mut light_app, 80, 24);
    assert!(
        !light_content.is_empty(),
        "Light theme should render non-empty content"
    );

    // 两次渲染内容应包含相同的核心元素(主题只影响颜色,不影响布局)
    assert!(
        dark_content.contains("Quest") && light_content.contains("Quest"),
        "both themes should render Quest panel"
    );
}

// ============================================================
// B. 输入模式切换测试
// ============================================================

#[test]
fn test_tui_input_mode_switching() {
    // WHY 输入模式:验证 Tab/Shift+Tab/数字键切换面板
    let mut app = make_app();
    assert_eq!(app.current_panel(), PanelId::Quest);

    // Tab:Quest → Parliament
    app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(app.current_panel(), PanelId::Parliament);

    // Tab:Parliament → Budget
    app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(app.current_panel(), PanelId::Budget);

    // Shift+Tab(BackTab):Budget → Parliament
    app.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
    assert_eq!(app.current_panel(), PanelId::Parliament);

    // 数字键 8:跳转到 Help
    app.handle_key_event(KeyEvent::new(KeyCode::Char('8'), KeyModifiers::NONE));
    assert_eq!(app.current_panel(), PanelId::Help);

    // 数字键 1:跳转回 Quest
    app.handle_key_event(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE));
    assert_eq!(app.current_panel(), PanelId::Quest);
}

#[test]
fn test_tui_input_mode_circular_navigation() {
    // WHY 循环导航:验证 Quest → ... → Help → Quest 的完整循环
    let mut app = make_app();

    // 连续 Tab 8 次应回到原点
    for expected in [
        PanelId::Parliament,
        PanelId::Budget,
        PanelId::Memory,
        PanelId::Security,
        PanelId::Health,
        PanelId::Log,
        PanelId::Help,
        PanelId::Quest,
    ] {
        app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
        assert_eq!(
            app.current_panel(),
            expected,
            "circular Tab navigation failed at {:?}",
            expected
        );
    }

    // Shift+Tab 反向循环 8 次也应回到原点
    for expected in [
        PanelId::Help,
        PanelId::Log,
        PanelId::Health,
        PanelId::Security,
        PanelId::Memory,
        PanelId::Budget,
        PanelId::Parliament,
        PanelId::Quest,
    ] {
        app.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT));
        assert_eq!(
            app.current_panel(),
            expected,
            "circular Shift+Tab navigation failed at {:?}",
            expected
        );
    }
}

#[test]
fn test_tui_input_mode_direct_jump() {
    // WHY 直接跳转:验证数字键 1-8 直接跳转到对应面板
    let mut app = make_app();

    let cases = [
        ('1', PanelId::Quest),
        ('2', PanelId::Parliament),
        ('3', PanelId::Budget),
        ('4', PanelId::Memory),
        ('5', PanelId::Security),
        ('6', PanelId::Health),
        ('7', PanelId::Log),
        ('8', PanelId::Help),
    ];

    for (ch, expected_panel) in cases {
        app.handle_key_event(KeyEvent::new(KeyCode::Char(ch), KeyModifiers::NONE));
        assert_eq!(
            app.current_panel(),
            expected_panel,
            "key '{}' should jump to {:?}",
            ch,
            expected_panel
        );
    }
}

#[test]
fn test_tui_f_keys_jump_to_panels() {
    let mut app = make_app();

    app.handle_key_event(KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE));
    assert_eq!(app.current_panel(), PanelId::Budget);

    app.handle_key_event(KeyEvent::new(KeyCode::F(1), KeyModifiers::NONE));
    assert_eq!(app.current_panel(), PanelId::Quest);
}

#[test]
fn test_tui_f_keys_new_panels() {
    let mut app = make_app();

    app.handle_key_event(KeyEvent::new(KeyCode::F(4), KeyModifiers::NONE));
    assert_eq!(app.current_panel(), PanelId::Memory);

    app.handle_key_event(KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE));
    assert_eq!(app.current_panel(), PanelId::Security);

    app.handle_key_event(KeyEvent::new(KeyCode::F(6), KeyModifiers::NONE));
    assert_eq!(app.current_panel(), PanelId::Health);
}

// ============================================================
// C. 键盘事件处理测试
// ============================================================

#[test]
fn test_tui_keyboard_event_handling() {
    // WHY 键盘事件:crossterm 0.28 KeyEvent::new(code, modifiers) 双参数
    let mut app = make_app();

    // q 键:退出应用
    app.handle_key_event(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    assert!(!app.state().running, "'q' should quit the app");

    // Esc 键:也退出
    let mut app2 = make_app();
    app2.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(!app2.state().running, "Esc should quit the app");
}

#[test]
fn test_tui_keyboard_release_event_ignored() {
    // WHY Release 过滤:Windows crossterm 会触发 Release 事件,
    // handle_key_event 必须过滤避免重复响应(平台兼容性)
    let mut app = make_app();

    // Release 事件应被忽略 — 用 new_with_kind 显式构造 Release
    app.handle_key_event(KeyEvent::new_with_kind(
        KeyCode::Char('q'),
        KeyModifiers::NONE,
        KeyEventKind::Release,
    ));
    assert!(
        app.state().running,
        "Release event should be ignored (Windows compatibility)"
    );

    // 同样,Tab 的 Release 也应被忽略
    app.handle_key_event(KeyEvent::new_with_kind(
        KeyCode::Tab,
        KeyModifiers::NONE,
        KeyEventKind::Release,
    ));
    assert_eq!(
        app.current_panel(),
        PanelId::Quest,
        "Tab Release should not switch panel"
    );
}

#[test]
fn test_tui_keyboard_command_input_buffer() {
    // WHY 输入缓冲:命令模式下 ASCII 可打印字符进入 input_buffer,Backspace 删除
    let mut app = make_app();

    // 进入命令模式
    app.handle_key_event(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE));
    assert_eq!(app.state().input_mode, InputMode::Command);

    // 输入 "hello"
    for c in ['h', 'e', 'l', 'l', 'o'] {
        app.handle_key_event(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    assert_eq!(app.state().input_buffer, "hello");

    // Backspace 删除最后一个字符
    app.handle_key_event(KeyEvent::new(KeyCode::Backspace, KeyModifiers::NONE));
    assert_eq!(app.state().input_buffer, "hell");

    // 空格也应被接受
    app.handle_key_event(KeyEvent::new(KeyCode::Char(' '), KeyModifiers::NONE));
    assert_eq!(app.state().input_buffer, "hell ");
}

#[test]
fn test_tui_keyboard_non_printable_ignored_in_command_mode() {
    // WHY 非可打印字符:验证 Enter/Arrow 等非可打印键不进入 input_buffer
    let mut app = make_app();

    // 进入命令模式
    app.handle_key_event(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE));

    // Enter 不应进入 input_buffer
    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(
        app.state().input_buffer.is_empty(),
        "Enter should not enter input_buffer"
    );

    // 方向键不应进入 input_buffer
    app.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    app.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key_event(KeyEvent::new(KeyCode::Left, KeyModifiers::NONE));
    app.handle_key_event(KeyEvent::new(KeyCode::Right, KeyModifiers::NONE));
    assert!(
        app.state().input_buffer.is_empty(),
        "Arrow keys should not enter input_buffer"
    );
}

#[test]
fn test_tui_render_after_keyboard_input() {
    // WHY 渲染+输入联合:验证键盘输入后渲染仍能正常工作
    let mut app = make_app();

    // 进入命令模式并输入
    app.handle_key_event(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE));
    for c in "budget".chars() {
        app.handle_key_event(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }

    // 渲染应正常工作,不 panic,且显示命令面板
    let content = render_to_string(&mut app, 80, 24);
    assert!(
        content.contains("Command"),
        "render after command input should show command palette"
    );
    assert!(
        content.contains("budget"),
        "render after input should show input buffer content"
    );
}

// ============================================================
// D. 命令面板集成测试
// ============================================================

#[test]
fn test_tui_command_palette_switch_panel() {
    let mut app = make_app();

    // 进入命令模式并输入 budget
    app.handle_key_event(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE));
    for c in "budget".chars() {
        app.handle_key_event(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_eq!(app.current_panel(), PanelId::Budget);
    assert_eq!(app.state().input_mode, InputMode::Normal);
    assert!(app.state().input_buffer.is_empty());
}

#[test]
fn test_tui_command_palette_quit() {
    let mut app = make_app();

    app.handle_key_event(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE));
    for c in "quit".chars() {
        app.handle_key_event(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(!app.state().running);
}

#[test]
fn test_tui_command_palette_new_panels() {
    let mut app = make_app();

    for (cmd, expected) in [
        ("memory", PanelId::Memory),
        ("security", PanelId::Security),
        ("health", PanelId::Health),
    ] {
        app.handle_key_event(KeyEvent::new(KeyCode::Char(':'), KeyModifiers::NONE));
        for c in cmd.chars() {
            app.handle_key_event(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
        }
        app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(app.current_panel(), expected, "command '{}' failed", cmd);
    }
}

#[test]
fn test_tui_search_mode_accepts_input() {
    let mut app = make_app();

    app.handle_key_event(KeyEvent::new(KeyCode::Char('/'), KeyModifiers::NONE));
    assert_eq!(app.state().input_mode, InputMode::Search);

    for c in "query".chars() {
        app.handle_key_event(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    assert_eq!(app.state().input_buffer, "query");

    // 提交后回到 Normal,不改变面板
    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(app.state().input_mode, InputMode::Normal);
    assert_eq!(app.current_panel(), PanelId::Quest);
}

// ============================================================
// E. 弹窗栈集成测试
// ============================================================

#[test]
fn test_tui_popup_esc_closes() {
    let mut app = make_app();
    app.state_mut().popup_stack.push(PopupKind::Notification {
        message: "test notification".into(),
        severity: Severity::Warning,
    });

    app.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(app.state().popup_stack.is_empty());
}

#[test]
fn test_tui_popup_enter_closes() {
    let mut app = make_app();
    app.state_mut().popup_stack.push(PopupKind::Detail {
        title: "Detail".into(),
        content: "content".into(),
    });

    app.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert!(app.state().popup_stack.is_empty());
}

// ============================================================
// F. Budget 数据驱动渲染测试
// ============================================================

#[test]
fn test_budget_panel_shows_critical_state() {
    // WHY 数据驱动渲染:验证 Budget 面板在 is_exceeded=true 时正确显示 EXCEEDED
    let snapshot = full_snapshot(
        Vec::new(),
        VecDeque::new(),
        BudgetMetrics {
            total_consumption: 9500.0,
            remaining_budget: 500.0,
            utilization_rate: 0.95,
            current_tier: "Critical".into(),
            coefficient: 1.2,
            is_exceeded: true,
            alert: Some("Budget cap exceeded".into()),
        },
    );

    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(StaticSnapshotSource::new(snapshot)),
    )
    .unwrap();

    // 从数据源拉取快照,确保 state.budget 反映测试数据
    app.update();
    app.switch_panel_to(PanelId::Budget);

    let content = render_to_string(&mut app, 80, 24);
    assert!(
        content.contains("EXCEEDED"),
        "Budget panel should render EXCEEDED status when budget is exceeded, got: {}",
        &content[..content.len().min(300)]
    );
}

// ============================================================
// G. Quest 数据驱动渲染测试
// ============================================================

#[test]
fn test_quest_panel_renders_real_quest_data() {
    // WHY 数据驱动 Quest 面板:验证自定义数据源提供的 Quest 能被渲染到面板中
    let quest = Quest {
        quest_id: "quest-panel-test-001".into(),
        title: "Panel Data Quest".into(),
        tasks: vec![
            Task {
                task_id: "t1".into(),
                description: "first task".into(),
                status: TaskStatus::Completed,
                dependencies: vec![],
            },
            Task {
                task_id: "t2".into(),
                description: "second task".into(),
                status: TaskStatus::Running,
                dependencies: vec![],
            },
            Task {
                task_id: "t3".into(),
                description: "third task".into(),
                status: TaskStatus::Pending,
                dependencies: vec![],
            },
        ],
        thinking_mode: ThinkingMode::Deep,
        checkpoint_id: Some("cp-1".into()),
    };

    let snapshot = full_snapshot(vec![quest], VecDeque::new(), BudgetMetrics::default());

    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(StaticSnapshotSource::new(snapshot)),
    )
    .unwrap();

    // 从数据源拉取快照,确保 state.quest_list 反映测试数据
    app.update();

    let content = render_to_string(&mut app, 80, 24);
    assert!(
        content.contains("Panel Data Quest"),
        "Quest panel should render quest title, got: {}",
        &content[..content.len().min(300)]
    );
    assert!(
        content.contains("quest-panel-test-001"),
        "Quest panel should render quest_id, got: {}",
        &content[..content.len().min(300)]
    );
    assert!(
        content.contains("Deep"),
        "Quest panel should render thinking mode, got: {}",
        &content[..content.len().min(300)]
    );
    assert!(
        content.contains("3 total"),
        "Quest panel should render task summary, got: {}",
        &content[..content.len().min(300)]
    );
    assert!(
        content.contains("1 running"),
        "Quest panel should include Running count, got: {}",
        &content[..content.len().min(300)]
    );
}

// ============================================================
// H. Parliament 面板数据驱动渲染测试
// ============================================================

#[test]
fn test_parliament_panel_renders_recent_events() {
    // WHY 数据驱动 Parliament 面板:验证自定义数据源提供的议会事件能被渲染到面板中
    let snapshot = full_snapshot(
        Vec::new(),
        VecDeque::from([
            NexusEvent::SkepticVeto {
                metadata: EventMetadata::new("parliament"),
                quest_id: "q-1".into(),
                veto_reason: "unsafe shell injection detected".into(),
                frozen_capabilities: vec!["shell_exec".into()],
            },
            NexusEvent::RedTeamAudit {
                metadata: EventMetadata::new("parliament"),
                vulnerability_type: "prompt_injection".into(),
                failed_probes: 5,
                total_probes: 20,
                detection_rate: 0.25,
                remediation_suggestion: "add input sanitization".into(),
            },
        ]),
        BudgetMetrics::default(),
    );

    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(StaticSnapshotSource::new(snapshot)),
    )
    .unwrap();

    // 从数据源拉取快照,确保 state.latest_events 反映测试数据
    app.update();
    app.switch_panel_to(PanelId::Parliament);

    let content = render_to_string(&mut app, 80, 24);
    assert!(
        content.contains("unsafe shell injection detected"),
        "Parliament panel should render SkepticVeto reason, got: {}",
        &content[..content.len().min(300)]
    );
    assert!(
        content.contains("q-1"),
        "Parliament panel should render quest_id, got: {}",
        &content[..content.len().min(300)]
    );
    assert!(
        content.contains("prompt_injection") || content.contains("RedTeamAudit"),
        "Parliament panel should render RedTeamAudit info, got: {}",
        &content[..content.len().min(300)]
    );
    assert!(
        !content.contains("No recent parliament events"),
        "should not show empty hint when parliament events exist"
    );
}

// ============================================================
// I. Log 面板数据驱动渲染测试
// ============================================================

#[test]
fn test_log_panel_renders_events() {
    // WHY 数据驱动 Log 面板:验证自定义数据源提供的事件能被渲染到主 Log 面板中。
    // M1 移除底部固定日志面板,Log 作为普通主面板存在。
    let snapshot = full_snapshot(
        Vec::new(),
        VecDeque::from([
            NexusEvent::CacheHit {
                metadata: EventMetadata::new("scc-cache"),
                cache_key: "k1".into(),
            },
            NexusEvent::BudgetExceeded {
                metadata: EventMetadata::new("efficiency-monitor"),
                budget_type: "token".into(),
                current: 9500,
                limit: 10000,
            },
        ]),
        BudgetMetrics::default(),
    );

    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(StaticSnapshotSource::new(snapshot)),
    )
    .unwrap();

    // 从数据源拉取快照,确保 state.latest_events 反映测试数据
    app.update();

    // 切换到 Log 主面板,验证主面板渲染事件摘要
    app.switch_panel_to(PanelId::Log);
    let content = render_to_string(&mut app, 80, 24);
    assert!(
        content.contains("CacheHit"),
        "Main Log panel should render event type, got: {}",
        &content[..content.len().min(300)]
    );
    assert!(
        content.contains("BudgetExceeded"),
        "Main Log panel should render critical event type, got: {}",
        &content[..content.len().min(300)]
    );
}

// ============================================================
// J. Memory/Security/Health 面板切换测试
// ============================================================

#[test]
fn test_memory_panel_switch_and_render() {
    let mut app = make_app();
    app.switch_panel_to(PanelId::Memory);
    assert_eq!(app.current_panel(), PanelId::Memory);

    let content = render_to_string(&mut app, 80, 24);
    assert!(
        content.contains("Memory"),
        "Memory panel should render title, got: {}",
        &content[..content.len().min(300)]
    );
}

#[test]
fn test_security_panel_switch_and_render() {
    let mut app = make_app();
    app.switch_panel_to(PanelId::Security);
    assert_eq!(app.current_panel(), PanelId::Security);

    let content = render_to_string(&mut app, 80, 24);
    assert!(
        content.contains("Security"),
        "Security panel should render title, got: {}",
        &content[..content.len().min(300)]
    );
}

#[test]
fn test_health_panel_switch_and_render() {
    let mut app = make_app();
    app.switch_panel_to(PanelId::Health);
    assert_eq!(app.current_panel(), PanelId::Health);

    let content = render_to_string(&mut app, 80, 24);
    assert!(
        content.contains("Health"),
        "Health panel should render title, got: {}",
        &content[..content.len().min(300)]
    );
}
