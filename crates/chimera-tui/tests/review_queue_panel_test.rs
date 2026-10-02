//! ReviewQueuePanel 集成测试 — F-59 专项测试补齐 + §(b.1) 铁律集成层锁定(R5)
//!
//! 覆盖: 面板注册可达 / next/prev 往返 / i18n 标题键 / 空态诚实展示 /
//! 事件注入派生渲染(app 级,经 latest_events)/「仅 Certified 渲染为已认证」铁律 /
//! F-40 Retracted 可辨识(标签 + 渲染色,缓冲行级)。范式: experience_card_viz_panel_test.rs。
//!
//! # 范围注记(F-60 关联)
//! ClaimIssued/CertifyIssued/ClaimRetracted 当前生产侧零发布(见
//! event_coverage_drift_test.rs 的 F-60 诚实登记),本文件以测试注入事件驱动
//! 派生渲染——锁定的是消费侧契约,不伪造生产数据链。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code
#![forbid(unsafe_code)]

mod common;

use common::mock::MockDataSource;
use std::collections::VecDeque;
use std::sync::Arc;

use chimera_tui::panels::review_queue::stage_label;
use chimera_tui::panels::{Panel, ReviewQueuePanel, ReviewStage};
use chimera_tui::types::{PanelId, TuiState, ViewMode};
use chimera_tui::{TuiApp, TuiConfig};
use event_bus::{EventMetadata, NexusEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::Terminal;

fn claim(id: &str) -> NexusEvent {
    NexusEvent::ClaimIssued {
        metadata: EventMetadata::new("t"),
        claim_id: id.into(),
        subject: format!("命题-{id}"),
        claimant: "agent-1".into(),
    }
}

fn certify(id: &str) -> NexusEvent {
    NexusEvent::CertifyIssued {
        metadata: EventMetadata::new("parliament"),
        claim_ref: id.into(),
        subject: format!("命题-{id}"),
        verifier: "Compiler".into(),
    }
}

fn retract(id: &str) -> NexusEvent {
    NexusEvent::ClaimRetracted {
        metadata: EventMetadata::new("t"),
        claim_ref: id.into(),
        reason: "作废".into(),
    }
}

/// app 级渲染(En pin + Dashboard):注入快照事件后收集整帧文本
fn render_app_with_events(events: Vec<NexusEvent>, w: u16, h: u16) -> String {
    let _locale_guard = chimera_tui::i18n::locale_test_guard();
    chimera_tui::set_locale(chimera_tui::Locale::En);

    let snapshot = chimera_tui::data::DataSnapshot {
        latest_events: Arc::new(VecDeque::from(events)),
        ..Default::default()
    };
    let ds = MockDataSource::new(snapshot);
    let mut app = TuiApp::with_data_source(
        TuiConfig {
            default_view_mode: ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
        Box::new(ds),
    )
    .expect("with_data_source should succeed");
    app.switch_panel_to(PanelId::ReviewQueue);
    app.update();

    let backend = ratatui::backend::TestBackend::new(w, h);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal.draw(|f| app.render(f)).expect("draw");
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol().chars().next().unwrap_or(' '))
        .collect()
}

/// 在缓冲中定位含 marker 的行,返回 marker 首字符处 cell 的前景色
///
/// WHY marker 定位而非行首:行左侧是块边框 '│'(默认色),整行内容为单一
/// Span 统一着色(review_queue.rs content),marker 处 cell 的 fg 即阶段色。
fn row_fg_at_marker(buf: &Buffer, marker: &str) -> Option<Color> {
    let w = buf.area.width as usize;
    let cells = buf.content();
    for y in 0..buf.area.height as usize {
        let row: String = cells[y * w..(y + 1) * w]
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect();
        if let Some(pos) = row.find(marker) {
            return Some(cells[y * w + pos].fg);
        }
    }
    None
}

// ----------------------------------------------------------
// 面板注册闭环(D-5 红线)
// ----------------------------------------------------------

#[test]
fn panel_registered_in_focus_order() {
    assert!(
        PanelId::REGISTERED_FOCUS_ORDER.contains(&PanelId::ReviewQueue),
        "ReviewQueue 应注册进焦点环"
    );
}

#[test]
fn panel_next_prev_roundtrip() {
    let panel = PanelId::ReviewQueue;
    assert_eq!(panel.next().prev(), panel);
    assert_eq!(panel.prev().next(), panel);
}

#[test]
fn panel_type_name_and_title() {
    assert_eq!(PanelId::ReviewQueue.as_str(), "ReviewQueue");
    assert_eq!(PanelId::ReviewQueue.title(), " Review Queue ");
}

#[test]
fn i18n_title_key_exists() {
    assert!(
        !chimera_tui::i18n::tr("panel.border.review_queue").is_empty(),
        "panel.border.review_queue 键应存在"
    );
}

// ----------------------------------------------------------
// 渲染闭环
// ----------------------------------------------------------

#[test]
fn empty_state_is_honest_without_events() {
    let content = ReviewQueuePanel::content(&TuiState::new()).to_string();
    assert!(
        content.contains("无审查项") || content.contains("No review items"),
        "空事件流应诚实展示无审查项, got: {content}"
    );
}

#[test]
fn app_level_claim_without_certify_never_renders_certified() {
    // §(b.1) 铁律(集成层):仅有 ClaimIssued 的事件流,任何渲染都不得出现
    // 「Certified」标签——「未获 Certify 即渲染为已认证」结构性不可达。
    // WHY 大小写敏感安全:En 未认证标签 "Claim (not certified)" 含小写
    // "certified",不含大写 "Certified",contains 精确判定不受干扰。
    let content = render_app_with_events(vec![claim("c-1")], 100, 30);
    assert!(
        content.contains("Claim (not certified)") && content.contains("c-1"),
        "Claim 行应派生渲染, got: {content}"
    );
    assert!(
        !content.contains("Certified"),
        "未获 Certify 不得渲染为已认证(§(b.1)), got: {content}"
    );
}

#[test]
fn app_level_claim_with_certify_renders_certified() {
    // 注入 Claim + 同 id Certify → 集成层(app 快照 → latest_events → 面板)派生为已认证
    let content = render_app_with_events(vec![claim("c-1"), certify("c-1")], 100, 30);
    assert!(
        content.contains("Certified") && content.contains("c-1"),
        "同 id Certify 后应派生渲染已认证, got: {content}"
    );
}

#[test]
fn retracted_distinguishable_from_claim_in_render() {
    // F-40(集成层锁定):Retracted 与 Claim 标签互异,且渲染色可辨识
    // (Retracted=Red,Claim=DarkGray),撤回/否决状态不得与待审混淆。
    // WHY pin En:缓冲行级 marker 用 En 标签(ASCII 可靠),guard 护全局 locale。
    let _locale_guard = chimera_tui::i18n::locale_test_guard();
    chimera_tui::set_locale(chimera_tui::Locale::En);

    // 标签层:stage_label 公开函数直断。
    assert_ne!(
        stage_label(ReviewStage::Retracted),
        stage_label(ReviewStage::Claim),
        "Retracted 与 Claim 的展示标签必须可辨识"
    );

    // 渲染层:缓冲行级前景色核对(整行单一 Span,marker 处 fg 即阶段色)
    let mut state = TuiState::new();
    state.latest_events = Arc::new(VecDeque::from([claim("c-1"), retract("c-2")]));
    let mut panel = ReviewQueuePanel::new();
    let area = Rect::new(0, 0, 60, 14);
    let mut buf = Buffer::empty(area);
    panel.render(&state, area, &mut buf);

    assert_eq!(
        row_fg_at_marker(&buf, "Claim (not certified)"),
        Some(Color::DarkGray),
        "Claim 行应保持暗灰(缓冲行级)"
    );
    assert_eq!(
        row_fg_at_marker(&buf, "Retracted"),
        Some(Color::Red),
        "Retracted 行应为红色可辨识(缓冲行级)"
    );
    // 渲染文本两标签同帧在场(可辨识的前提)
    let text: String = buf
        .content()
        .iter()
        .map(|c| c.symbol().chars().next().unwrap_or(' '))
        .collect();
    assert!(
        text.contains("Retracted") && text.contains("Claim (not certified)"),
        "两阶段标签应同帧渲染, got: {text}"
    );
}

#[test]
fn panel_trait_object_dispatch() {
    let panel: Box<dyn Panel> = Box::new(ReviewQueuePanel::new());
    assert_eq!(panel.id(), PanelId::ReviewQueue);
    assert!(!panel.shortcuts().is_empty());
}
