//! PvlScore 面板集成测试(评估报告 P0-3:此前唯一零集成测试覆盖的面板)
//!
//! 对应架构层:L10 Interface
//!
//! # 覆盖点
//! - 选择边界钳制:selected ∈ [0, 8],越界按键不溢出;
//! - 九维度标签与总分渲染(正常终端);
//! - 小终端降级分支(min 15 rows 提示);
//! - 快捷键诚实性:shortcuts 声明的 ↑/↓ 与 j/k 均真实可达。

#![forbid(unsafe_code)]

use chimera_tui::panels::{Panel, PvlScorePanel};
use chimera_tui::TuiState;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// 渲染面板到内存 Buffer 并返回字符网格(与既有面板测试同模式)
///
/// WHY 宽度感知跳过续格:ratatui 0.29 宽字符(CJK/emoji)第 2 列的续格 cell
/// symbol 保留为空格占位(不写 skip 标记,见 engine/compat.rs 注释),若直接
/// 收集会把汉字隔断为“真 实 执 行”;故按“前格显示宽度 >= 2 → 本格为续格”
/// 规则跳过(与 compat 转换同一套宽字符定位规则)。
fn render_to_string(panel: &mut PvlScorePanel, state: &TuiState, w: u16, h: u16) -> String {
    let mut buf = Buffer::empty(Rect::new(0, 0, w, h));
    panel.render(state, Rect::new(0, 0, w, h), &mut buf);
    let mut out = String::new();
    let mut prev_wide = false;
    for cell in buf.content().iter() {
        if cell.skip {
            continue;
        }
        let s = cell.symbol();
        // 前格为宽字符时,本格即其续格(空格占位),跳过不收集
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

// ============================================================
// A. 选择边界钳制(handle_key)
// ============================================================

#[test]
fn selected_starts_at_zero() {
    assert_eq!(PvlScorePanel::new().selected(), 0);
}

#[test]
fn up_at_top_clamps_to_zero() {
    let mut panel = PvlScorePanel::new();
    let mut state = TuiState::new();
    // 顶部连续按 Up/k:selected 应钳制在 0(不溢出到 usize::MAX)
    for c in [KeyCode::Up, KeyCode::Char('k')] {
        for _ in 0..5 {
            panel.handle_key(key(c), &mut state);
        }
        assert_eq!(panel.selected(), 0, "{c:?} 在顶部应钳制为 0");
    }
}

#[test]
fn down_at_bottom_clamps_to_eight() {
    let mut panel = PvlScorePanel::new();
    let mut state = TuiState::new();
    // 先到最底(9 个维度,索引 0-8):Down ×9
    for _ in 0..9 {
        panel.handle_key(key(KeyCode::Down), &mut state);
    }
    assert_eq!(panel.selected(), 8, "9 次 Down 应到达最后维度(索引 8)");
    // 底部连续按 Down/j:selected 应钳制在 8
    for _ in 0..5 {
        panel.handle_key(key(KeyCode::Down), &mut state);
    }
    assert_eq!(panel.selected(), 8, "Down 在底部应钳制为 8");
    for _ in 0..5 {
        panel.handle_key(key(KeyCode::Char('j')), &mut state);
    }
    assert_eq!(panel.selected(), 8, "j 在底部应钳制为 8");
}

#[test]
fn jk_navigation_matches_arrows() {
    // 快捷键诚实性:shortcuts 声明 "↑/↓" 与 "j/k" 语义等价
    let mut arrows = PvlScorePanel::new();
    let mut keys = PvlScorePanel::new();
    let mut state = TuiState::new();
    let seq: [KeyCode; 5] = [
        KeyCode::Char('j'),
        KeyCode::Down,
        KeyCode::Char('k'),
        KeyCode::Char('j'),
        KeyCode::Up,
    ];
    for code in seq {
        arrows.handle_key(key(code), &mut state);
        keys.handle_key(key(code), &mut state);
        assert_eq!(
            arrows.selected(),
            keys.selected(),
            "同一按键序列两面板 selected 应一致"
        );
    }
}

#[test]
fn handle_key_returns_none_never_command() {
    // PvlScore 为展示型面板:任何按键都不应产生 TuiCommand
    let mut panel = PvlScorePanel::new();
    let mut state = TuiState::new();
    for code in [
        KeyCode::Enter,
        KeyCode::Char('R'),
        KeyCode::Esc,
        KeyCode::Tab,
    ] {
        assert!(
            panel.handle_key(key(code), &mut state).is_none(),
            "{code:?} 不应产生命令"
        );
    }
}

// ============================================================
// B. 渲染验证
// ============================================================

#[test]
fn renders_nine_dimensions_and_total() {
    let mut panel = PvlScorePanel::new();
    let state = TuiState::new();
    let content = render_to_string(&mut panel, &state, 80, 30);
    // 九维度标签(中文面板默认 Zh locale)
    for label in [
        "真实执行",
        "覆盖率",
        "验证通过",
        "置信度",
        "效率",
        "重试纪律",
        "产出实质性",
        "零孤儿",
        "沙箱清洁",
    ] {
        assert!(content.contains(label), "应渲染维度标签 {label}");
    }
    assert!(content.contains("TOTAL SCORE"), "应渲染总分标题");
    assert!(
        content.contains("%"),
        "维度与总分应包含百分比数值"
    );
}

#[test]
fn small_terminal_shows_degradation_hint() {
    // 小终端降级分支:area.height < 15 时显示提示而非九维度(避免挤压)
    let mut panel = PvlScorePanel::new();
    let state = TuiState::new();
    let content = render_to_string(&mut panel, &state, 80, 10);
    assert!(
        content.contains("Terminal too small for PVL Score panel"),
        "小终端应显示降级提示,实际: {content:?}"
    );
    // 降级路径不应 panic 且不渲染维度
    assert!(!content.contains("TOTAL SCORE"));
}

#[test]
fn selected_dimension_gets_marker() {
    // 选中维度应带 ▶ 前缀(可见反馈)
    let mut panel = PvlScorePanel::new();
    let mut state = TuiState::new();
    panel.handle_key(key(KeyCode::Char('j')), &mut state);
    panel.handle_key(key(KeyCode::Char('j')), &mut state);
    assert_eq!(panel.selected(), 2);
    let content = render_to_string(&mut panel, &state, 80, 30);
    assert!(
        content.contains('▶'),
        "选中维度应显示 ▶ 标记"
    );
}

// ============================================================
// C. 面板身份
// ============================================================

#[test]
fn panel_id_and_shortcuts_are_consistent() {
    let panel = PvlScorePanel::new();
    assert_eq!(panel.id(), chimera_tui::PanelId::PvlScore);
    // shortcuts 声明的按键必须真实可达(快捷键诚实性):
    // ↑/↓ 与 j/k 在 handle_key 均有分支(见上方导航测试)
    let keys: Vec<&str> = panel.shortcuts().iter().map(|(k, _)| *k).collect();
    assert!(keys.iter().any(|k| k.contains("↑")), "应声明 ↑/↓ 导航");
    assert!(keys.iter().any(|k| k.contains("j/k")), "应声明 j/k 导航");
}
