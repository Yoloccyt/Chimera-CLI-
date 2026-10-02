//! ClvVector 面板集成测试 — Task P7 TDD-RED→GREEN
//!
//! 验证: 初始化 / 默认值 / 空状态 / J/K 滚动 / 未映射按键
//!
//! # 设计决策(WHY)
//! - 直接测试 Panel trait 方法(不通过 TuiApp),聚焦面板逻辑正确性
//! - 使用 TuiState::new() 构造测试状态(空状态,clv_summary = None)
//! - 参考 timeline_panel_test.rs 的测试模式,保持一致性
//! - WHY 空状态测试:ClvSummary 由 event-bus crate 定义,集成测试中保持
//!   空状态(None)验证面板的空状态处理逻辑,避免跨 crate 构造复杂数据

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use chimera_tui::panels::{ClvVectorPanel, Panel};
use chimera_tui::types::{PanelId, TuiState};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

// ============================================================
// A. 初始化测试
// ============================================================

#[test]
fn test_clv_vector_panel_initialization() {
    let _locale_guard = chimera_tui::i18n::locale_test_guard();
    let panel = ClvVectorPanel::new();
    assert_eq!(panel.id(), PanelId::ClvVector);
    // title 返回 Line<'static>;i18n 后本地化,固定英文捕获后复位再断言。
    chimera_tui::set_locale(chimera_tui::Locale::En);
    let title = panel.title().to_string();
    chimera_tui::set_locale(chimera_tui::Locale::Zh);
    assert_eq!(title, " CLV Vector ");
}

#[test]
fn test_clv_vector_panel_default() {
    let panel = ClvVectorPanel::default();
    assert_eq!(panel.id(), PanelId::ClvVector);
    assert_eq!(panel.selected(), 0);
    assert_eq!(panel.scroll_offset(), 0);
}

// ============================================================
// B. 空状态测试
// ============================================================

#[test]
fn test_clv_vector_panel_empty_state() {
    // 空状态(clv_summary = None)时 handle_key 不应 panic
    let mut panel = ClvVectorPanel::new();
    let mut state = TuiState::new();
    assert!(state.clv_summary.is_none());

    let key = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
    let result = panel.handle_key(key, &mut state);
    assert!(result.is_none(), "Down on empty list should return None");
    assert_eq!(
        panel.selected(),
        0,
        "selected should stay at 0 on empty list"
    );
}

// ============================================================
// C. J/K 滚动测试(空状态 — 验证不 panic)
// ============================================================

#[test]
fn test_clv_vector_panel_navigation_j_k() {
    // WHY 空状态测试:clv_summary = None 时 count = 0,
    // j/k 应安全返回 None 且 selected 保持 0,不 panic
    let mut panel = ClvVectorPanel::new();
    let mut state = TuiState::new();
    assert!(state.clv_summary.is_none());

    // j 向下(空列表应保持 0)
    let key = KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE);
    let result = panel.handle_key(key, &mut state);
    assert!(result.is_none(), "j should not return a command");
    assert_eq!(panel.selected(), 0, "j on empty list should stay at 0");

    // k 向上(空列表应保持 0)
    let key = KeyEvent::new(KeyCode::Char('k'), KeyModifiers::NONE);
    let result = panel.handle_key(key, &mut state);
    assert!(result.is_none(), "k should not return a command");
    assert_eq!(panel.selected(), 0, "k on empty list should stay at 0");
}

// ============================================================
// D. 未映射按键测试
// ============================================================

#[test]
fn test_clv_vector_panel_help_key_returns_none() {
    // WHY P3.2:`?` 已由 TuiApp 全局拦截为 Help overlay,面板不再处理。
    let mut panel = ClvVectorPanel::new();
    let mut state = TuiState::new();

    let key = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE);
    let result = panel.handle_key(key, &mut state);
    assert_eq!(result, None);
}

// ============================================================
// E. 选中行可见性(批次三终审 #1:窗口化守护归属从 proptest 扩展到专项文件)
// ============================================================

/// 宽字符感知的渲染收集(仿 osa_sparse_panel_test 同款口径:跳过 CJK 续格,
/// 避免 zh chrome 文案被续格空格隔断导致 contains 失配)
fn render_panel_to_string(
    panel: &mut ClvVectorPanel,
    state: &TuiState,
    w: u16,
    h: u16,
) -> String {
    let area = ratatui::layout::Rect::new(0, 0, w, h);
    let mut buf = ratatui::buffer::Buffer::empty(area);
    panel.render(state, area, &mut buf);
    let mut out = String::new();
    let mut prev_wide = false;
    for cell in buf.content().iter() {
        if cell.skip {
            continue;
        }
        let s = cell.symbol();
        if prev_wide {
            prev_wide = false;
            continue;
        }
        if s.is_empty() {
            continue;
        }
        let ch = s.chars().next().unwrap_or(' ');
        prev_wide = unicode_width::UnicodeWidthStr::width(s) >= 2;
        out.push(ch);
    }
    out
}

#[test]
fn test_clv_vector_panel_selected_row_visible_after_scroll() {
    let _locale_guard = chimera_tui::i18n::locale_test_guard();
    let mut panel = ClvVectorPanel::new();
    let mut state = TuiState::new();
    // WHY 构造 ClvSummary:Top 维度表是本面板唯一列表区,空状态(None)
    // 无选中行可言;24 条互异 dim 号保证选中行文本全帧唯一可锚定
    state.clv_summary = Some(event_bus::ClvSummary {
        block_means: vec![0.1; 8],
        l2_norm: 2.5,
        top_dims: (0..24).map(|i| (100 + i, 0.5_f32)).collect(),
    });

    // Down×18 → selected=18;100x40(inner 38)下 Top 维度表行数充足
    // (对齐 proptest clv 用例 area_height ≥ 20 的布局守卫)
    let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
    for _ in 0..18 {
        panel.handle_key(down, &mut state);
    }
    let content = render_panel_to_string(&mut panel, &state, 100, 40);

    // 帧内 "> " 唯一来源是选中行前缀(未选中行为 "  ",chrome 无 ">" 字样)
    assert!(content.contains("> "), "选中行应带 > 前缀, got: {content}");
    assert!(
        content.contains("dim=118"),
        "选中行文本应随前缀同帧可见, got: {content}"
    );
}
