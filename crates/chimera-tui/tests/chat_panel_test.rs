//! Chat 面板渲染集成测试(M3b)
//!
//! 对应架构层:L10 Interface
//!
//! # 测试策略(WHY)
//! - **断言 locale 无关的用户数据**:消息内容(用户输入/Agent 文本)与语言无关,
//!   断言其出现在渲染缓冲中,避免依赖 i18n 文案(规避并行测试的全局 locale 竞争)。
//! - **只读渲染**:构造带 `chat_messages` 的 `TuiState`,经 `ChatPanel::render` 渲染到
//!   `TestBackend` 内存缓冲,验证对外可观测输出。状态/流式逻辑由 ChatSync 单测覆盖。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use chimera_tui::{ChatMessage, ChatPanel, ChatRole, Panel, TuiState};
use event_bus::ChatStatus;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;
use unicode_width::UnicodeWidthStr;

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

// ============================================================
// F-25:超宽消息按面板宽度预折行(修复前 ratatui 无 wrap 走
// LineTruncator,超宽即横向截断,尾部内容永久不可读且无横向滚动)
// ============================================================

#[test]
fn long_message_wraps_instead_of_truncated() {
    // WHY 借公开的 ChatPanel::content_lines 取折行末段文本作 marker:折行断点
    // 由实现决定,不硬编码;marker 与渲染帧同宽(42 外宽 → 40 内容宽)自洽。
    // 判别力:修复前 content_lines 不折行,marker 为全文 130 列,渲染帧必然
    // 截断不含 marker;修复后 marker 是折行末段,完整渲染可见。
    let tail = "TAIL-MARK";
    let content = format!("{} {tail}", "x".repeat(120));
    let state = state_with(vec![msg(ChatRole::User, &content)], ChatStatus::Idle);

    let lines = ChatPanel::content_lines(&[msg(ChatRole::User, &content)], 40);
    assert!(lines.len() > 1, "130 列内容在 40 宽面板应折为多行");
    let raw_last: String = lines
        .last()
        .unwrap()
        .spans
        .iter()
        .map(|s| s.content.to_string())
        .collect();
    let marker = raw_last.trim_start().to_string();
    assert!(
        marker.len() < content.len(),
        "折行末段应短于原文(折行确实发生)"
    );

    // 渲染层:外宽 42(边框 2)→ 内容宽 40,与上方 marker 计算同宽;
    // 首段 + 末段均可见 = 全文经折行后可读,不是仅首行
    let out = render_chat(&state, 42, 12);
    let head = lines.first().unwrap().spans[1].content.to_string();
    assert!(out.contains(&head), "折行首段应可见");
    assert!(
        out.contains(&marker),
        "折行末段 {marker:?} 应在渲染帧可见(修复前被横向截断,尾部永久不可读)"
    );
}

#[test]
fn content_lines_wraps_cjk_by_display_width() {
    // CJK 双宽字符按显示宽度折行:宽度口径与 ratatui 同源(unicode-width)。
    // WHY 不做 TestBackend 渲染断言:CJK 宽字符在 TestBackend 逐单元格重组时
    // 续位格被 reset 为空格,contains(中文) 永假(见本文件头注释),故只在
    // content_lines 层断言行数/行宽/无损性。
    let content = "中".repeat(60); // 显示宽 120 列
    let lines = ChatPanel::content_lines(&[msg(ChatRole::User, &content)], 40);
    assert!(lines.len() > 1, "120 列 CJK 内容在 40 宽面板应折为多行");

    // 行宽约束:每行 spans 显示宽合计 ≤ 面板内容宽
    for line in &lines {
        let w: usize = line.spans.iter().map(|s| s.content.width()).sum();
        assert!(w <= 40, "折行后行显示宽 {w} 不应超过面板内容宽 40");
    }

    // 无损性:首行内容段 + 各续行内容段(spans[1])拼接应还原原文(折行不丢字符)
    let restored = lines
        .iter()
        .map(|l| l.spans[1].content.to_string())
        .collect::<Vec<_>>()
        .join("");
    assert_eq!(
        restored, content,
        "折行不得丢失字符(全文可读是 F-25 的核心诉求)"
    );
}
