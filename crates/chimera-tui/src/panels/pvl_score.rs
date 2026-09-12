//! TUI PVL 过程评分面板 — 九维度过程评分可视化（Task 3.7:L10 → L7 向下依赖）
//!
//! 对应架构层:L10 Interface
//! 对应 PanelId:PvlScore
//! 对应创新点:PVL(Producer-Verifier Loop,九维度过程评分,快手 KAT,ADR-049)
//!
//! # 核心职责(PS-2 批次3 起)
//! - **未接线态**:PVL 循环在生产装配面从未实例化,面板如实标注未接线
//!   (原实现读全局快照,未注册时回退全 1.0 → 九维恒显示满分,属假数据)
//! - 待 PVL 循环装配后,以事件驱动方式重建九维度纵向布局与总分渲染
//!
//! # 设计决策(WHY)
//! - **静态快照模式**:`pvl_score()` 返回 `ProcessScore` 值类型,无需异步上下文,
//!   面板渲染不阻塞 TUI 事件循环。TODO: v3.x 接入 RuntimeAuditor 实时采集。
//! - **颜色编码**:≥0.8 绿色(优秀)/ 0.5-0.8 黄色(一般)/ <0.5 红色(需关注),
//!   与 OsaSparsePanel 的颜色策略一致。
//! - **九维度标签映射**:real_execution→真实执行 / coverage→覆盖率 / verification→验证通过
//!   / confidence→置信度 / efficiency→效率 / retry_discipline→重试纪律
//!   / output_substance→产出实质性 / orphan_free→零孤儿 / sandbox_clean→沙箱清洁

use crossterm::event::{KeyCode, KeyEvent, MouseEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::panels::Panel;
use crate::types::{PanelId, TuiCommand, TuiState};
/// PVL 过程评分面板
///
/// PS-2 批次3:本面板此前读 `pvl_layer::pvl_score()`(L10→L7 越层依赖),
/// 但生产装配面**从未实例化 PVL 循环**(`register_pvl_score` 全仓仅测试调用),
/// 该函数恒回退 `fallback_pvl_score()`(**全 1.0**)→ 九维评分条长期显示"满分",
/// 比显示异常值更具误导性。现移除越层直调,改为**诚实标注未接线**;
/// 待 PVL 循环真正装配后,再以事件驱动方式重建评分渲染。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct PvlScorePanel {
    /// 当前选中维度索引（0-8,键盘导航）
    selected: usize,
}

impl PvlScorePanel {
    /// 创建新的 PVL 过程评分面板
    pub fn new() -> Self {
        Self::default()
    }

    /// 返回当前选中索引（测试用）
    pub fn selected(&self) -> usize {
        self.selected
    }

    /// 根据评分值返回对应的颜色编码(渲染重建时复用;当前未接线态不用)
    ///
    /// WHY 保留:PVL 循环装配后将以事件驱动方式重建评分渲染,阈值映射不变。
    /// 现阶段仅被 `#[cfg(test)]` 使用,故标注以抑制非测试构建的死代码告警。
    #[allow(dead_code)]
    fn score_color(value: f32) -> Color {
        if value >= 0.8 {
            Color::Green
        } else if value >= 0.5 {
            Color::Yellow
        } else {
            Color::Red
        }
    }
}

/// 构造维度评分条形字符串(填充 '█' + 空白 '░')
///
/// WHY 模块级自由函数而非关联函数:渲染路径与内联测试(`mod tests` 的
/// `use super::*`)共用同一实现,避免关联函数路径下的双份维护。
/// WHY 上界钳制:评分快照来自外部注册路径,越界值(>1.0)未经钳制会让填充
/// 字符撑出标签区(`filled = (value * width) as usize` 在 value=1.5 时溢出
/// 50%);NaN 经 `as usize` 折算为 0,自然落空条。
///
/// 现阶段仅被 `#[cfg(test)]` 使用(未接线态不渲染评分条),故标注以抑制告警。
#[allow(dead_code)]
fn score_bar(value: f32, gauge_width: usize) -> String {
    let filled = ((value * gauge_width as f32) as usize).min(gauge_width);
    let empty = gauge_width.saturating_sub(filled);
    format!("{}{}", "█".repeat(filled), "░".repeat(empty))
}

impl Panel for PvlScorePanel {
    // PS-3(I-4):实现 gg/G —— 本面板有 9 维评分列表(selected 0..8),
    // 此前走 Panel 默认空实现,gg/G 静默无响应(评估报告"死键"清单)。
    fn scroll_to_top(&mut self, _state: &mut TuiState) {
        self.selected = 0;
    }

    fn scroll_to_bottom(&mut self, _state: &mut TuiState) {
        // 9 维评分固定为 0..8(见 handle_key 的 `selected < 8` 边界),末项恒为 8。
        self.selected = 8;
    }

    fn id(&self) -> PanelId {
        PanelId::PvlScore
    }

    fn title(&self) -> Line<'static> {
        Line::from(PanelId::PvlScore.title()).style(
            Style::default()
                .fg(Color::Magenta)
                .add_modifier(Modifier::BOLD),
        )
    }

    fn render(&mut self, _state: &TuiState, area: Rect, buf: &mut Buffer) {
        let inner = Block::default()
            .borders(Borders::ALL)
            .title(Self::title(self))
            .border_style(Style::default().fg(Color::Magenta));

        // 最小终端高度检查:标题(1) + 未接线提示(2) + 边框(2)
        if area.height < 5 {
            let text = Text::from(crate::t!("panel.pvl.terminal_too_small"));
            let p = Paragraph::new(text).block(inner);
            Widget::render(p, area, buf);
            return;
        }

        let inner_area = inner.inner(area);
        Widget::render(inner, area, buf);

        // PS-2 批次3:PVL 循环在生产装配面从未实例化(证据见进度报告),
        // 原数据源恒回退"全 1.0",九维评分条会显示为满分 —— 这是**假数据**,
        // 且比异常值更具误导性(看起来"完美")。此处不再展示无法证实的评分,
        // 改为诚实标注未接线(与 parliament 免疫探针、task_manager 数据源同策略)。
        let text = Text::from(vec![
            Line::from(Span::styled(
                crate::t!("panel.pvl.unwired"),
                Style::default()
                    .fg(Color::DarkGray)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(Span::styled(
                crate::t!("panel.pvl.unwired_hint"),
                Style::default().fg(Color::DarkGray),
            )),
        ]);
        let p = Paragraph::new(text);
        Widget::render(p, inner_area, buf);
    }

    fn handle_key(&mut self, key: KeyEvent, _state: &mut TuiState) -> Option<TuiCommand> {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') if self.selected < 8 => {
                self.selected += 1;
            }
            _ => {}
        }
        None
    }

    fn handle_mouse(&mut self, _mouse: MouseEvent, _state: &mut TuiState) -> Option<TuiCommand> {
        None
    }

    fn shortcuts(&self) -> Vec<(&'static str, &'static str)> {
        vec![
            ("↑/↓", crate::t!("shortcut.select_dimension")),
            ("j/k", crate::t!("shortcut.navigate")),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn score_bar_clamps_overflow_to_width() {
        // value=1.5(150%):填充应钳满 gauge_width,不溢出标签区
        assert_eq!(score_bar(1.5_f32, 10), "█".repeat(10));
        assert_eq!(score_bar(f32::INFINITY, 7), "█".repeat(7));
    }

    #[test]
    fn score_bar_nan_renders_empty() {
        // NaN 经 as usize 折算为 0:空条而非 panic/乱码
        assert_eq!(score_bar(f32::NAN, 10), "░".repeat(10));
    }

    #[test]
    fn score_bar_partial_fill() {
        // 50%:半填充半空白
        let expected = format!("{}{}", "█".repeat(5), "░".repeat(5));
        assert_eq!(score_bar(0.5_f32, 10), expected);
    }

    // ========================================================
    // PS-3(I-4):gg/G 必须到达首/末维(此前默认空实现,静默无响应)
    // ========================================================

    #[test]
    fn gg_g_reach_first_and_last_dimension() {
        let mut panel = PvlScorePanel::new();
        let mut state = TuiState::new();

        // 模拟用户已下移到中间(第 5 维),gg 应回首
        panel.selected = 4;
        panel.scroll_to_top(&mut state);
        assert_eq!(
            panel.selected(),
            0,
            "gg should land on first dimension (idx 0)"
        );

        // G 应到达末维(9 维评分,末下标 8)
        panel.scroll_to_bottom(&mut state);
        assert_eq!(
            panel.selected(),
            8,
            "G should land on 9th dimension (idx 8)"
        );
    }
}

// ============================================================
// PS-2 批次3:未接线态(移除 L10→L7 越层直调后的诚实标注)
// ============================================================

#[cfg(test)]
mod ps2_batch3_tests {
    use super::*;

    fn render() -> String {
        let state = TuiState::new();
        let area = Rect::new(0, 0, 120, 30);
        let mut buf = Buffer::empty(area);
        let mut panel = PvlScorePanel::new();
        panel.render(&state, area, &mut buf);
        buf.content().iter().map(|c| c.symbol()).collect()
    }

    #[test]
    fn no_fabricated_scores_rendered() {
        let rendered = render();
        // 原实现读全局快照,未注册时回退全 1.0 → 九维恒显示 100.0%(假数据)。
        // 移除越层直调后,不得再出现任何评分数值或评分条。
        assert!(
            !rendered.contains("100.0%"),
            "must not render fabricated full scores, got: {rendered}"
        );
        assert!(
            !rendered.contains('█'),
            "must not render score bars without a real data source, got: {rendered}"
        );
    }

    #[test]
    fn unwired_marker_is_present() {
        let rendered = render();
        // 断言面板确实渲染了内容(未接线提示存在),而非空白
        assert!(
            rendered.contains("PVL") || rendered.contains('九'),
            "unwired marker expected, got: {rendered}"
        );
    }
}
