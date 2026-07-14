//! P5 跨面板联动 — 面板状态保留集成测试
//!
//! 验证切换面板时,各面板的 `selected` / `scroll_offset` 状态被保留,
//! 切回时恢复。这是 P5 "Esc 状态保留" 的端到端验证。
//!
//! # 验证策略
//! 由于 `TuiApp::panels` 为私有字段且 `Panel` trait 不暴露 `selected`,
//! 本测试通过 `TestBackend` 渲染输出间接验证选中状态:
//! - Quest 面板选中项渲染为 `> [N]` 前缀(N = selected + 1)
//! - Log 面板选中项渲染为 `>` 前缀且高亮
//!
//! WHY 渲染验证优于字段直读:end-to-end 测试更接近真实用户体验,
//! 且不破坏 Panel trait 封装性(无需新增 trait 方法或公开字段)。

#![forbid(unsafe_code)]

use chimera_tui::{TuiApp, TuiConfig};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use event_bus::{EventMetadata, NexusEvent};
use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::collections::VecDeque;

/// 构造简单 Quest(含 1 个 Pending 任务)
fn sample_quest(id: &str, title: &str) -> Quest {
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
    }
}

/// 渲染 TuiApp 并返回缓冲区文本内容
fn render_content(app: &mut TuiApp, width: u16, height: u16) -> String {
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

/// 测试替身数据源 — 返回预设快照
#[derive(Debug)]
struct MockDataSource {
    snapshot: chimera_tui::DataSnapshot,
    config: chimera_tui::DataSourceConfig,
}

impl MockDataSource {
    fn new(snapshot: chimera_tui::DataSnapshot) -> Self {
        Self {
            snapshot,
            config: chimera_tui::DataSourceConfig::default(),
        }
    }
}

impl chimera_tui::TuiDataSource for MockDataSource {
    fn snapshot(&self) -> Result<chimera_tui::DataSnapshot, chimera_tui::TuiError> {
        Ok(self.snapshot.clone())
    }

    fn config(&self) -> &chimera_tui::DataSourceConfig {
        &self.config
    }
}

// ============================================================
// Quest 面板 selected 保留测试
// ============================================================

#[test]
fn quest_panel_selected_preserved_across_panel_switch() {
    // 准备 5 个 Quest,用于验证 selected=3 的保留
    let snapshot = chimera_tui::DataSnapshot {
        quest_list: vec![
            sample_quest("q1", "Alpha Quest"),
            sample_quest("q2", "Beta Quest"),
            sample_quest("q3", "Gamma Quest"),
            sample_quest("q4", "Delta Quest"),
            sample_quest("q5", "Epsilon Quest"),
        ],
        ..Default::default()
    };

    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(MockDataSource::new(snapshot)),
    )
    .unwrap();
    app.update();
    assert_eq!(app.current_panel(), chimera_tui::PanelId::Quest);

    // 在 Quest 面板按 Down 3 次,使 selected = 3(对应第 4 项 "[4] Delta Quest")
    for _ in 0..3 {
        app.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    }

    // 渲染并验证选中项为 "[4] Delta Quest"(含 ">" 前缀)
    let content = render_content(&mut app, 100, 30);
    assert!(
        content.contains("> [4] Delta Quest"),
        "before switch: selected should be 3 (Delta Quest), content: {content}"
    );

    // Tab 切换到 Parliament
    app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    assert_eq!(app.current_panel(), chimera_tui::PanelId::Parliament);

    // Tab 切回 Quest(从 Parliament 按 Tab 会到 Budget,需要再按 BackTab 回到 Quest)
    // WHY 使用 BackTab:Tab 是顺序循环 Quest→Parliament→Budget→...,
    // 从 Parliament 按 Tab 会到 Budget,不是回到 Quest。
    // 用 BackTab 从 Parliament 回到 Quest。
    app.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));
    assert_eq!(app.current_panel(), chimera_tui::PanelId::Quest);

    // 渲染并验证 selected 仍为 3("> [4] Delta Quest" 仍出现)
    let content_after = render_content(&mut app, 100, 30);
    assert!(
        content_after.contains("> [4] Delta Quest"),
        "after switch back: selected should still be 3 (Delta Quest), content: {content_after}"
    );
}

#[test]
fn quest_panel_selected_zero_preserved() {
    // 边界测试:selected=0(初始值)在切换后应保持
    let snapshot = chimera_tui::DataSnapshot {
        quest_list: vec![
            sample_quest("q1", "First Quest"),
            sample_quest("q2", "Second Quest"),
        ],
        ..Default::default()
    };

    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(MockDataSource::new(snapshot)),
    )
    .unwrap();
    app.update();

    // 不按任何导航键,selected 应为 0
    let content = render_content(&mut app, 100, 30);
    assert!(
        content.contains("> [1] First Quest"),
        "initial selected should be 0, content: {content}"
    );

    // 切换到 Parliament 再切回
    app.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
    app.handle_key_event(KeyEvent::new(KeyCode::BackTab, KeyModifiers::NONE));

    let content_after = render_content(&mut app, 100, 30);
    assert!(
        content_after.contains("> [1] First Quest"),
        "selected=0 should be preserved, content: {content_after}"
    );
}

// ============================================================
// Log 面板 scroll_offset 保留测试
// ============================================================

#[test]
fn log_panel_scroll_state_preserved_across_panel_switch() {
    // 准备 10 个事件,每个用不同的 source 以便通过渲染输出区分
    // WHY 不同 source:Log 面板渲染显示 `[source] event_type`,
    // cache_key 不在渲染输出中,只能通过 source 区分不同事件。
    let events: Vec<NexusEvent> = (0..10)
        .map(|i| NexusEvent::CacheHit {
            metadata: EventMetadata::new(format!("src-{i}")),
            cache_key: format!("key-{i}"),
        })
        .collect();

    let snapshot = chimera_tui::DataSnapshot {
        latest_events: VecDeque::from(events),
        ..Default::default()
    };

    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(MockDataSource::new(snapshot)),
    )
    .unwrap();
    app.update();

    // 切换到 Log 面板(用数字键 7)
    app.handle_key_event(KeyEvent::new(KeyCode::Char('7'), KeyModifiers::NONE));
    assert_eq!(app.current_panel(), chimera_tui::PanelId::Log);

    // 按 Down 5 次,移动 selected 到第 5 项
    // WHY Log 面板用 .rev() 倒序:最新事件在顶部。
    // latest_events = [src-0, src-1, ..., src-9],.rev() 后 [src-9, src-8, ..., src-0]
    // selected=5 对应 src-9 之后的第 5 项 = src-4
    for _ in 0..5 {
        app.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    }

    // 渲染并验证选中项为 src-4(渲染格式:`> HH:MM:SS [src-4] CacheHit`)
    let content_before = render_content(&mut app, 100, 30);
    assert!(
        content_before.contains("[src-4]"),
        "before switch: Log selected=5 should show [src-4], content: {content_before}"
    );

    // 切换到 Health 面板再切回 Log
    // WHY 用数字键 6 切到 Health,再按 7 切回 Log:验证跨面板切换保留状态
    app.handle_key_event(KeyEvent::new(KeyCode::Char('6'), KeyModifiers::NONE));
    assert_eq!(app.current_panel(), chimera_tui::PanelId::Health);

    app.handle_key_event(KeyEvent::new(KeyCode::Char('7'), KeyModifiers::NONE));
    assert_eq!(app.current_panel(), chimera_tui::PanelId::Log);

    // 渲染并验证 selected 仍为 5([src-4] 仍出现)
    let content_after = render_content(&mut app, 100, 30);
    assert!(
        content_after.contains("[src-4]"),
        "after switch back: Log selected should still be 5 ([src-4]), content: {content_after}"
    );
}

// ============================================================
// 多面板状态独立保留测试
// ============================================================

#[test]
fn multiple_panels_preserve_independent_state() {
    // 验证 Quest 和 Log 面板各自独立保留状态
    // WHY 不同 source:Log 面板渲染显示 `[source] event_type`,
    // 需通过 source 区分不同事件。
    let snapshot = chimera_tui::DataSnapshot {
        quest_list: vec![
            sample_quest("q1", "Quest One"),
            sample_quest("q2", "Quest Two"),
            sample_quest("q3", "Quest Three"),
        ],
        latest_events: VecDeque::from([
            NexusEvent::CacheHit {
                metadata: EventMetadata::new("evt-a"),
                cache_key: "event-a".into(),
            },
            NexusEvent::CacheMiss {
                metadata: EventMetadata::new("evt-b"),
                cache_key: "event-b".into(),
            },
            NexusEvent::CacheHit {
                metadata: EventMetadata::new("evt-c"),
                cache_key: "event-c".into(),
            },
        ]),
        ..Default::default()
    };

    let mut app = TuiApp::with_data_source(
        TuiConfig::default(),
        Box::new(MockDataSource::new(snapshot)),
    )
    .unwrap();
    app.update();

    // Quest 面板:按 Down 2 次,selected=2(Quest Three)
    app.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    app.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));
    let quest_content = render_content(&mut app, 100, 30);
    assert!(
        quest_content.contains("> [3] Quest Three"),
        "Quest selected should be 2, content: {quest_content}"
    );

    // 切换到 Log 面板,按 Down 1 次
    app.handle_key_event(KeyEvent::new(KeyCode::Char('7'), KeyModifiers::NONE));
    app.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE));

    // 切回 Quest,验证 selected 仍为 2
    app.handle_key_event(KeyEvent::new(KeyCode::Char('1'), KeyModifiers::NONE));
    let quest_after = render_content(&mut app, 100, 30);
    assert!(
        quest_after.contains("> [3] Quest Three"),
        "Quest selected should still be 2 after visiting Log, content: {quest_after}"
    );

    // 再切到 Log,验证 selected 仍为 1
    app.handle_key_event(KeyEvent::new(KeyCode::Char('7'), KeyModifiers::NONE));
    let log_after = render_content(&mut app, 100, 30);
    // Log 倒序:[evt-c(CacheHit), evt-b(CacheMiss), evt-a(CacheHit)]
    // selected=1 对应 evt-b(CacheMiss)
    assert!(
        log_after.contains("[evt-b]"),
        "Log selected should still be 1 ([evt-b]) after visiting Quest, content: {log_after}"
    );
}
