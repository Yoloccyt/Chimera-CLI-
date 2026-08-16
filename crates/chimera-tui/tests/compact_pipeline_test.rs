//! W9 策展波集成测试 — /compact 管道回环 + /context 网格(Concord W9,ADR-081)
//!
//! 对应架构层:L10 Interface
//!
//! # 覆盖
//! - E2E 回环:EventBus → DataPipeline ChatSync 成史 → compact_chat_history
//!   命令信道 → 策展替换 → 快照报告 seq 回传(正常路径);
//! - 边界:空历史 /context 诚实提示;Stub 数据源拒绝策展(不支持时不伪造);
//! - 异常:/compact 非法参数诚实反馈用法;
//! - 守恒:用户轮次(Pinned)零驱逐,策展后历史长度收缩且含摘要消息。

#![forbid(unsafe_code)]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use chimera_tui::data::curator::{CompactPolicy, CompactRequest, CurationConfig};
use chimera_tui::{
    DataPipeline, DataSourceConfig, EventSubscriber, InputMode, PopupKind, Severity, TuiApp,
    TuiConfig, TuiDataSource,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use event_bus::{EventBus, EventMetadata, NexusEvent};

/// 串行化所有会读写全局 locale 的测试(与 integration.rs 同范式,防并行 flaky)
static LOCALE_LOCK: Mutex<()> = Mutex::new(());

fn locale_guard() -> std::sync::MutexGuard<'static, ()> {
    LOCALE_LOCK.lock().unwrap_or_else(|p| p.into_inner())
}

fn press(app: &mut TuiApp, code: KeyCode) {
    app.handle_key_event(KeyEvent::new(code, KeyModifiers::NONE));
}

fn type_str(app: &mut TuiApp, s: &str) {
    for c in s.chars() {
        app.handle_key_event(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
}

/// 输入并提交一条斜杠命令(先按 `/` 进入 Slash 模式)
fn slash_submit(app: &mut TuiApp, cmd: &str) {
    press(app, KeyCode::Char('/'));
    type_str(app, cmd);
    press(app, KeyCode::Enter);
}

/// 等待谓词成立(轮询快照,超时 fail)
async fn wait_for<F: Fn() -> bool>(what: &str, f: F) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while tokio::time::Instant::now() < deadline {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("等待超时: {what}");
}

// ============================================================
// A. E2E:管道命令信道策展回环
// ============================================================

#[tokio::test]
async fn compact_pipeline_replaces_history_and_reports() {
    let bus = EventBus::new();
    let subscriber = EventSubscriber::new(bus.clone());
    let pipeline = Arc::new(DataPipeline::new(
        subscriber,
        DataSourceConfig {
            tick_interval_ms: 50, // 快 tick 加速配速释放与报告回传
            ..Default::default()
        },
    ));

    // 构造历史:1 条用户消息 + 10 条长 assistant 消息(10 轮流式轮次)
    bus.publish(NexusEvent::TuiChatSubmitted {
        metadata: EventMetadata::new("test"),
        session_id: "s1".into(),
        query: "start the session".into(),
        slash_command: None,
    })
    .await
    .expect("publish submitted");
    for i in 0..10 {
        bus.publish(NexusEvent::TuiChatResponseChunk {
            metadata: EventMetadata::new("orchestrator"),
            session_id: "s1".into(),
            delta: format!("detailed answer {i} {}\n", "x".repeat(80)),
            cursor_hint: 0,
        })
        .await
        .expect("publish chunk");
        bus.publish(NexusEvent::TuiChatCompleted {
            metadata: EventMetadata::new("orchestrator"),
            session_id: "s1".into(),
            tool_use: None,
        })
        .await
        .expect("publish completed");
    }

    // 等待历史经闸门完整落库(配速队列随 tick 释放)
    // WHY 显式 inherent 调用:TuiDataSource trait 在作用域内,同名方法
    // 会解析为返回 Result 的 trait 版本
    let p = Arc::clone(&pipeline);
    wait_for("chat 历史落库 11 条", move || {
        DataPipeline::snapshot(&p).chat_messages.len() >= 11
    })
    .await;

    // 紧预算策展:候选段应被大量驱逐/摘要
    let req = CompactRequest {
        policy: CompactPolicy::Conservative,
        cfg: CurationConfig {
            budget_tokens: 60,
            recent_turns: 1,
            ..Default::default()
        },
    };
    assert!(pipeline.compact_chat_history(req), "命令信道应受理策展请求");

    // 等待报告 seq 回传(≥1)
    let p = Arc::clone(&pipeline);
    wait_for("策展报告回传", move || {
        DataPipeline::snapshot(&p).compact_report_seq >= 1
    })
    .await;

    let snap = DataPipeline::snapshot(&pipeline);
    let report = snap.last_compact_report.as_ref().expect("报告应随快照回传");
    assert_eq!(report.seq, snap.compact_report_seq, "seq 应一致");
    assert_eq!(
        report.before_messages, 11,
        "策展前 11 条(1 user + 10 assistant)"
    );
    assert!(
        report.after_messages < report.before_messages,
        "紧预算应收缩历史: {} → {}",
        report.before_messages,
        report.after_messages
    );
    assert!(
        (0.0..=1.0).contains(&report.retained_value_ratio),
        "保留价值比应在合法域"
    );

    // 守恒:用户轮次(Pinned)零驱逐
    let user_msgs: Vec<_> = snap
        .chat_messages
        .iter()
        .filter(|m| m.role == chimera_tui::ChatRole::User)
        .collect();
    assert_eq!(user_msgs.len(), 1, "用户消息不可驱逐");
    assert_eq!(user_msgs[0].content, "start the session");
    // 落选高价值段应产生摘要消息(每条 21 token 高价值)
    assert!(
        snap.chat_messages
            .iter()
            .any(|m| m.content.starts_with("[上下文策展摘要]")),
        "应有抽取式摘要消息插入"
    );
    pipeline.shutdown().await;
}

// ============================================================
// B. 边界:Stub 数据源不支持策展(诚实拒绝,不伪造)
// ============================================================

#[test]
fn compact_rejected_on_stub_source() {
    let stub = chimera_tui::StubDataSource::new();
    let req = CompactRequest {
        policy: CompactPolicy::Balanced,
        cfg: CurationConfig::default(),
    };
    assert!(
        !stub.compact_chat_history(req),
        "Stub 数据源无 ChatSync,应返回 false(默认 no-op)"
    );
}

#[test]
fn compact_slash_on_stub_source_shows_rejected_status() {
    let _guard = locale_guard();
    // TuiApp 默认即 StubDataSource:/compact 应诚实反馈拒绝
    let mut app = TuiApp::new(TuiConfig::default()).unwrap();
    slash_submit(&mut app, "compact");
    let (msg, severity) = app.state().status_message.as_ref().expect("应有状态反馈");
    assert_eq!(*severity, Severity::Warning, "拒绝应为警告级");
    assert!(!msg.is_empty(), "拒绝反馈不应为空(不伪造完成)");
}

#[test]
fn compact_invalid_policy_shows_usage() {
    let _guard = locale_guard();
    let mut app = TuiApp::new(TuiConfig::default()).unwrap();
    slash_submit(&mut app, "compact --policy turbo");
    let (msg, severity) = app.state().status_message.as_ref().expect("应有状态反馈");
    assert_eq!(*severity, Severity::Warning);
    assert!(msg.contains("/compact"), "非法参数应反馈用法,实际: {msg}");
}

// ============================================================
// C. /context 网格弹窗(五段分布 + 空历史诚实提示)
// ============================================================

#[test]
fn context_popup_empty_history_honest_hint() {
    let _guard = locale_guard();
    let mut app = TuiApp::new(TuiConfig {
        persist_state: false,
        ..Default::default()
    })
    .unwrap();
    app.state_mut().chat_messages.clear();
    slash_submit(&mut app, "context");
    let popup = app
        .state()
        .popup_stack
        .current()
        .expect("/context 应弹出 Detail 弹窗");
    match popup {
        PopupKind::Detail { content, .. } => {
            assert!(
                chimera_tui::i18n::zh::lookup("context.empty")
                    .is_some_and(|hint| content.contains(hint)),
                "空历史应诚实提示,实际: {content}"
            );
        }
        other => panic!("应为 Detail 弹窗,实际: {other:?}"),
    }
}

#[test]
fn context_popup_shows_segment_distribution() {
    let _guard = locale_guard();
    chimera_tui::set_locale(chimera_tui::Locale::En);
    let mut app = TuiApp::new(TuiConfig {
        persist_state: false,
        ..Default::default()
    })
    .unwrap();
    // 注入历史:2 轮(2 user + 2 assistant),Recent 窗口默认末 8 条 → 全在窗口内
    app.state_mut().chat_messages = vec![
        chimera_tui::ChatMessage {
            role: chimera_tui::ChatRole::User,
            content: "question one".into(),
        },
        chimera_tui::ChatMessage {
            role: chimera_tui::ChatRole::Assistant,
            content: "answer one with content".into(),
        },
        chimera_tui::ChatMessage {
            role: chimera_tui::ChatRole::User,
            content: "question two".into(),
        },
        chimera_tui::ChatMessage {
            role: chimera_tui::ChatRole::Assistant,
            content: "answer two with content".into(),
        },
    ];
    slash_submit(&mut app, "context");
    let content = match app.state().popup_stack.current() {
        Some(PopupKind::Detail { content, .. }) => content.clone(),
        other => panic!("应为 Detail 弹窗,实际: {other:?}"),
    };
    chimera_tui::set_locale(chimera_tui::Locale::Zh); // 立即复位默认中文
                                                      // 四段分布行齐备(En 标签)
    for label in ["Pinned (user)", "Recent window", "Candidates", "Reserved"] {
        assert!(content.contains(label), "缺少分段行 {label}: {content}");
    }
    // 总量与预算行
    assert!(content.contains("Total tokens (est.)"), "缺少总量行");
    assert!(content.contains("Curation budget"), "缺少预算行");
    // 分布条字符
    assert!(content.contains('█') || content.contains('░'), "应有分布条");
    assert_eq!(
        app.state().input_mode,
        InputMode::Normal,
        "弹窗不应改变输入模式"
    );
}
