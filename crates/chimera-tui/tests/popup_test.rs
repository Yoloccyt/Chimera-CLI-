//! PopupStack 集成测试 — 验证弹窗渲染、详情滚动与确认弹窗行为

#![forbid(unsafe_code)]

use chimera_tui::{PopupKind, PopupStack, Severity};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

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

#[test]
fn popup_stack_render_notification() {
    let mut stack = PopupStack::new();
    stack.push(PopupKind::Notification {
        message: "hello world".into(),
        severity: Severity::Warning,
    });

    let content = render_popup(&stack, 40, 10);
    assert!(
        content.contains("Notification"),
        "notification popup should render title"
    );
    assert!(
        content.contains("hello world"),
        "notification popup should render message"
    );
}

#[test]
fn popup_stack_render_detail() {
    let mut stack = PopupStack::new();
    stack.push(PopupKind::Detail {
        title: "Event Detail".into(),
        content: "Type: CacheHit\nSource: scc-cache".into(),
        scroll: 0,
    });

    let content = render_popup(&stack, 50, 12);
    assert!(
        content.contains("Event Detail"),
        "detail popup should render title"
    );
    assert!(
        content.contains("CacheHit"),
        "detail popup should render content"
    );
    assert!(
        content.contains("scc-cache"),
        "detail popup should render source"
    );
}

#[test]
fn popup_stack_render_confirm() {
    let mut stack = PopupStack::new();
    stack.push(PopupKind::Confirm {
        prompt: "Are you sure?".into(),
        on_confirm: "quit".into(),
        confirmed: true,
    });

    let content = render_popup(&stack, 50, 12);
    assert!(
        content.contains("Confirm"),
        "confirm popup should render title"
    );
    assert!(
        content.contains("Are you sure?"),
        "confirm popup should render prompt"
    );
    assert!(
        content.contains("Yes"),
        "confirm popup should render Yes option"
    );
    assert!(
        content.contains("No"),
        "confirm popup should render No option"
    );
}

#[test]
fn popup_stack_detail_scroll() {
    let mut stack = PopupStack::new();
    stack.push(PopupKind::Detail {
        title: "Scrollable".into(),
        content: "line1\nline2\nline3\nline4\nline5".into(),
        scroll: 0,
    });

    stack.scroll_current(2);
    match stack.current() {
        Some(PopupKind::Detail { scroll, .. }) => assert_eq!(*scroll, 2),
        _ => panic!("expected Detail popup"),
    }

    stack.scroll_current(-5);
    match stack.current() {
        Some(PopupKind::Detail { scroll, .. }) => assert_eq!(*scroll, 0),
        _ => panic!("expected Detail popup"),
    }
}

#[test]
fn popup_stack_confirm_toggle() {
    let mut stack = PopupStack::new();
    stack.push(PopupKind::Confirm {
        prompt: "Quit?".into(),
        on_confirm: "quit".into(),
        confirmed: true,
    });

    stack.toggle_confirm();
    match stack.current() {
        Some(PopupKind::Confirm { confirmed, .. }) => assert!(!confirmed),
        _ => panic!("expected Confirm popup"),
    }

    stack.toggle_confirm();
    match stack.current() {
        Some(PopupKind::Confirm { confirmed, .. }) => assert!(confirmed),
        _ => panic!("expected Confirm popup"),
    }
}

#[test]
fn popup_stack_empty_render_is_noop() {
    let stack = PopupStack::new();
    let backend = TestBackend::new(40, 10);
    let mut terminal = Terminal::new(backend).unwrap();
    // 空栈渲染不应 panic
    terminal
        .draw(|f| stack.render(f.area(), f.buffer_mut()))
        .unwrap();
}

#[test]
fn popup_stack_render_topmost_only() {
    let mut stack = PopupStack::new();
    stack.push(PopupKind::Notification {
        message: "bottom".into(),
        severity: Severity::Info,
    });
    stack.push(PopupKind::Notification {
        message: "top".into(),
        severity: Severity::Error,
    });

    let content = render_popup(&stack, 40, 10);
    assert!(content.contains("top"));
    // 底层消息不应出现在最终渲染输出中
    assert!(!content.contains("bottom"));
}

#[test]
fn popup_stack_pop_returns_last_pushed() {
    let mut stack = PopupStack::new();
    stack.push(PopupKind::Notification {
        message: "first".into(),
        severity: Severity::Info,
    });
    stack.push(PopupKind::Detail {
        title: "second".into(),
        content: "detail content".into(),
        scroll: 0,
    });

    let popped = stack.pop();
    assert_eq!(
        popped,
        Some(PopupKind::Detail {
            title: "second".into(),
            content: "detail content".into(),
            scroll: 0,
        })
    );
    assert!(stack.current().is_some());
}

#[test]
fn popup_stack_detail_render_respects_scroll() {
    let mut stack = PopupStack::new();
    stack.push(PopupKind::Detail {
        title: "Scroll".into(),
        content: "line1\nline2\nHIDDEN_LINE".into(),
        scroll: 2,
    });

    let content = render_popup(&stack, 40, 6);
    assert!(content.contains("HIDDEN_LINE"));
}
