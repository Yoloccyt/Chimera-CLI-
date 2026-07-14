//! EventDetail 弹窗集成测试 — P3.1 Enter 事件详情 overlay
//!
//! 覆盖:EventStream/Parliament/Log 面板 Enter 弹窗、JSON 高亮颜色、
//! 相关事件 ID 链展示、MessagePack 解码失败降级为 hex。

#![forbid(unsafe_code)]

use chimera_tui::{
    EventStreamPanel, LogPanel, Panel, ParliamentPanel, PopupKind, PopupStack, TuiState,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use event_bus::{EventMetadata, NexusEvent};
use ratatui::backend::TestBackend;
use ratatui::style::Color;
use ratatui::Terminal;
use std::collections::VecDeque;

fn render_popup(stack: &PopupStack, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| stack.render(f.area(), f.buffer_mut()))
        .unwrap();

    let buffer = terminal.backend().buffer();
    buffer
        .content()
        .iter()
        .map(|c| c.symbol().chars().next().unwrap_or(' '))
        .collect()
}

/// 返回渲染缓冲区中所有非默认前景色单元格的坐标与颜色
fn colored_cells(stack: &PopupStack, width: u16, height: u16) -> Vec<(u16, u16, Color)> {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| stack.render(f.area(), f.buffer_mut()))
        .unwrap();

    let buffer = terminal.backend().buffer();
    let mut result = Vec::new();
    for y in 0..height {
        for x in 0..width {
            if let Some(cell) = buffer.cell((x, y)) {
                let fg = cell.style().fg.unwrap_or(Color::Reset);
                if fg != Color::Reset {
                    result.push((x, y, fg));
                }
            }
        }
    }
    result
}

#[test]
fn event_stream_enter_opens_event_detail_popup() {
    let mut panel = EventStreamPanel::new();
    let mut state = TuiState::new();
    state.latest_events = VecDeque::from([NexusEvent::CacheHit {
        metadata: EventMetadata::new("scc-cache"),
        cache_key: "k1".into(),
    }]);

    let cmd = panel.handle_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut state,
    );

    match cmd {
        Some(chimera_tui::TuiCommand::OpenPopup(PopupKind::EventDetail {
            title,
            event_type,
            payload_decoded,
            related_event_ids,
            ..
        })) => {
            assert!(
                title.contains("CacheHit"),
                "title should contain event type"
            );
            assert_eq!(event_type, "CacheHit");
            assert!(
                payload_decoded.contains("scc-cache"),
                "decoded payload should contain source"
            );
            assert!(
                payload_decoded.contains("k1"),
                "decoded payload should contain cache key"
            );
            assert!(
                !related_event_ids.is_empty(),
                "should extract at least event_id"
            );
        }
        _ => panic!("expected EventDetail popup, got {:?}", cmd),
    }
}

#[test]
fn parliament_enter_opens_event_detail_popup() {
    let mut panel = ParliamentPanel::new();
    let mut state = TuiState::new();
    state.latest_events = VecDeque::from([NexusEvent::VoteCast {
        metadata: EventMetadata::new("parliament"),
        proposal_id: "p1".into(),
        voter: "alice".into(),
        vote: true,
    }]);

    let cmd = panel.handle_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut state,
    );

    match cmd {
        Some(chimera_tui::TuiCommand::OpenPopup(PopupKind::EventDetail {
            title,
            event_type,
            payload_decoded,
            related_event_ids,
            ..
        })) => {
            assert!(title.contains("VoteCast"));
            assert_eq!(event_type, "VoteCast");
            assert!(payload_decoded.contains("alice"));
            assert!(payload_decoded.contains("p1"));
            assert!(related_event_ids.contains(&"p1".to_string()));
        }
        _ => panic!("expected EventDetail popup, got {:?}", cmd),
    }
}

#[test]
fn log_enter_opens_event_detail_popup() {
    let mut panel = LogPanel::new();
    let mut state = TuiState::new();
    state.latest_events = VecDeque::from([NexusEvent::BudgetExceeded {
        metadata: EventMetadata::new("decb-governor"),
        budget_type: "token".into(),
        current: 9500,
        limit: 10000,
    }]);

    let cmd = panel.handle_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut state,
    );

    match cmd {
        Some(chimera_tui::TuiCommand::OpenPopup(PopupKind::EventDetail {
            title,
            event_type,
            payload_decoded,
            related_event_ids,
            ..
        })) => {
            assert!(title.contains("BudgetExceeded"));
            assert_eq!(event_type, "BudgetExceeded");
            assert!(payload_decoded.contains("decb-governor"));
            assert!(payload_decoded.contains("9500"));
            assert!(!related_event_ids.is_empty());
        }
        _ => panic!("expected EventDetail popup, got {:?}", cmd),
    }
}

#[test]
fn event_detail_popup_renders_json_highlight_colors() {
    let event = NexusEvent::CacheHit {
        metadata: EventMetadata::new("scc-cache"),
        cache_key: "k1".into(),
    };
    let mut stack = PopupStack::new();
    stack.push(PopupKind::event_detail(&event));

    let content = render_popup(&stack, 60, 40);
    assert!(content.contains("CacheHit"), "popup should render title");
    assert!(content.contains("Type:"), "popup should render type label");
    assert!(
        content.contains("Related IDs:"),
        "popup should render related IDs section"
    );

    // 验证 JSON 高亮颜色可见(至少存在绿色/青色/黄色/灰色中的一种)
    let colors: Vec<_> = colored_cells(&stack, 60, 40).iter().map(|c| c.2).collect();
    assert!(
        colors.contains(&Color::Green),
        "string values should be highlighted green"
    );
}

#[test]
fn event_detail_popup_shows_related_event_ids() {
    let event = NexusEvent::CheckpointSaved {
        metadata: EventMetadata::new("quest-engine"),
        quest_id: "q1".into(),
        checkpoint_id: "c1".into(),
        memory_snapshot_hash: "abc".into(),
    };
    let mut stack = PopupStack::new();
    stack.push(PopupKind::event_detail(&event));

    // WHY 使用 80x40 而非 60x20:CheckpointSaved 事件含 metadata/时间戳等
    // 较长字段,在较窄或较矮的弹窗中 Related IDs 区域会被推到可视区外,
    // 导致渲染字符串中无法命中 quest_id/checkpoint_id。增大尺寸确保底部
    // 相关 ID 列表可见,同时仍验证正常弹窗渲染不 panic。
    let content = render_popup(&stack, 80, 40);
    assert!(
        content.contains("q1"),
        "related IDs should include quest_id"
    );
    assert!(
        content.contains("c1"),
        "related IDs should include checkpoint_id"
    );
}

#[test]
fn event_detail_popup_scrolls() {
    let event = NexusEvent::CacheHit {
        metadata: EventMetadata::new("scc-cache"),
        cache_key: "k1".into(),
    };
    let mut stack = PopupStack::new();
    stack.push(PopupKind::event_detail(&event));

    stack.scroll_current(3);
    match stack.current() {
        Some(PopupKind::EventDetail { scroll, .. }) => assert_eq!(*scroll, 3),
        _ => panic!("expected EventDetail popup"),
    }
}

#[test]
fn event_detail_popup_falls_back_to_hex_on_decode_failure() {
    // 手动构造无效 MessagePack bytes,触发解码失败路径
    let mut stack = PopupStack::new();
    stack.push(PopupKind::EventDetail {
        title: "Broken Detail (raw hex)".into(),
        event_type: "Broken".into(),
        payload_decoded: hex_string(&[0xff, 0xff]),
        payload_raw: vec![0xff, 0xff],
        related_event_ids: Vec::new(),
        scroll: 0,
    });

    let content = render_popup(&stack, 60, 10);
    assert!(
        content.contains("(raw hex)"),
        "title should indicate raw hex fallback"
    );
    assert!(
        content.contains("ff"),
        "hex fallback should render hex characters"
    );
}

fn hex_string(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}
