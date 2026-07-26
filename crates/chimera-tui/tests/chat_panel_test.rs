//! Chat 面板渲染集成测试(M3b)
//!
//! 对应架构层:L10 Interface
//!
//! # 测试策略(WHY)
//! - **断言 locale 无关的用户数据**:消息内容(用户输入/Agent 文本)与语言无关,
//!   断言其出现在渲染缓冲中,避免依赖 i18n 文案(规避并行测试的全局 locale 竞争)。
//! - **只读渲染**:构造带 `chat_messages` 的 `TuiState`,经 `ChatPanel::render` 渲染到
//!   `TestBackend` 内存缓冲,验证对外可观测输出。状态/流式逻辑由 ChatSync 单测覆盖。

#![forbid(unsafe_code)]

use chimera_tui::{ChatMessage, ChatPanel, ChatRole, Panel, TuiState};
use event_bus::ChatStatus;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;

/// 构造带对话历史与状态的 TuiState
fn state_with(messages: Vec<ChatMessage>, status: ChatStatus) -> TuiState {
    let mut s = TuiState::new();
    s.chat_messages = messages;
    s.chat_status = status;
    s
}

/// 便捷构造消息
fn msg(role: ChatRole, content: &str) -> ChatMessage {
    ChatMessage {
        role,
        content: content.to_string(),
    }
}

/// 在内存后端渲染 ChatPanel 并收集全部 cell 字符
fn render_chat(state: &TuiState, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    let mut panel = ChatPanel::new();
    terminal
        .draw(|f| panel.render(state, Rect::new(0, 0, width, height), f.buffer_mut()))
        .unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol().chars().next().unwrap_or(' '))
        .collect()
}

#[test]
fn renders_user_and_assistant_messages() {
    let state = state_with(
        vec![
            msg(ChatRole::User, "hello world"),
            msg(ChatRole::Assistant, "hi there"),
        ],
        ChatStatus::Idle,
    );
    let out = render_chat(&state, 48, 12);
    assert!(out.contains("hello world"), "应渲染用户消息内容");
    assert!(out.contains("hi there"), "应渲染 Agent 回答内容");
}

#[test]
fn empty_history_renders_without_panic() {
    let state = state_with(vec![], ChatStatus::Idle);
    let out = render_chat(&state, 40, 10);
    // 空历史仅渲染边框(含状态指示器),不 panic 且非空
    assert!(!out.trim().is_empty(), "空历史应渲染边框且非空");
}

#[test]
fn multiline_message_content_renders() {
    // 多行内容(含 \n)应按行拆分渲染,两行文本均可见
    let state = state_with(
        vec![msg(ChatRole::Assistant, "line-one\nline-two")],
        ChatStatus::Idle,
    );
    let out = render_chat(&state, 48, 12);
    assert!(out.contains("line-one"), "应渲染首行");
    assert!(out.contains("line-two"), "应渲染续行");
}

#[test]
fn many_messages_render_without_panic() {
    // 消息数远超可见高度:验证贴底自动滚动的偏移钳制不越界、不 panic
    let messages: Vec<ChatMessage> = (0..100)
        .map(|i| msg(ChatRole::User, &format!("m{i}")))
        .collect();
    let state = state_with(messages, ChatStatus::Idle);
    let out = render_chat(&state, 40, 10);
    // 贴底跟随:最新消息 "m99" 应可见,最旧 "m0" 被滚出视口
    assert!(out.contains("m99"), "贴底跟随应显示最新消息");
}

#[test]
fn different_statuses_render_without_panic() {
    // 三种会话状态均能渲染状态指示器而不 panic(文案 locale 相关,此处只验证不 panic)
    for status in [
        ChatStatus::Thinking,
        ChatStatus::ToolExecuting,
        ChatStatus::Idle,
    ] {
        let state = state_with(vec![msg(ChatRole::User, "q")], status);
        let out = render_chat(&state, 40, 8);
        assert!(out.contains('q'), "状态 {status:?} 下应渲染消息");
    }
}
