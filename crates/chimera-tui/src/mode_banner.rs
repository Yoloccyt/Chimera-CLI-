//! ModeBanner 模式常驻横幅(Concord W6 T6.1,方案 §7.2)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! yottacode banner 实证(方案 §5.1 模式一键):审批模式为 Plan/Auto 时,
//! 会话视图需要一行**常驻横幅**明示当前模式(仅 statusline 徽标不够醒目,
//! 操作员视线焦点在会话流)。Normal 态横幅隐藏——零认知噪声。
//!
//! # 与 statusline 徽标的分工
//! - statusline 徽标:双视图常驻,三态色彩区分(W4 已建);
//! - ModeBanner:仅 Chat 视图 + 仅 Plan/Auto 态,一行警示横幅。
//!
//! Dashboard 视图不渲染 banner(R2 冻结策略:新功能只进 Chat)。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Paragraph, Widget};

use crate::approval_mode::ApprovalMode;

/// 横幅规格:i18n 键 + 前景色(背景统一用暗底反衬)
///
/// # 返回
/// - `None`:Normal 态,横幅隐藏;
/// - `Some((key, fg))`:Plan/Auto 态,渲染对应文案。
pub fn banner_line(mode: ApprovalMode) -> Option<(&'static str, Color)> {
    match mode {
        ApprovalMode::Normal => None,
        ApprovalMode::Plan => Some(("banner.plan", Color::Yellow)),
        ApprovalMode::Auto => Some(("banner.auto", Color::Red)),
    }
}

/// 渲染横幅到指定区域(单行,无 borders,暗底 + 警示前景 + 加粗)
///
/// # 参数
/// - `mode`:当前审批模式(Normal 时本函数不应被调用,防御性直接返回)
/// - `area`:横幅区域(高度应为 1,调用方布局保证)
/// - `buf`:目标缓冲
pub fn render_banner(mode: ApprovalMode, area: Rect, buf: &mut Buffer) {
    let Some((key, fg)) = banner_line(mode) else {
        return;
    };
    if area.height == 0 || area.width == 0 {
        return;
    }
    let line = Line::from(Span::styled(
        format!(" {} ", crate::t!(key)),
        Style::default()
            .fg(fg)
            .bg(Color::DarkGray)
            .add_modifier(Modifier::BOLD),
    ));
    Paragraph::new(line).render(area, buf);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_hidden_in_normal() {
        assert_eq!(banner_line(ApprovalMode::Normal), None);
    }

    #[test]
    fn banner_plan_yellow() {
        let (key, color) = banner_line(ApprovalMode::Plan).expect("Plan 应有横幅");
        assert_eq!(key, "banner.plan");
        assert_eq!(color, Color::Yellow);
    }

    #[test]
    fn banner_auto_red() {
        let (key, color) = banner_line(ApprovalMode::Auto).expect("Auto 应有横幅");
        assert_eq!(key, "banner.auto");
        assert_eq!(color, Color::Red);
    }

    #[test]
    fn render_banner_writes_text_and_normal_noop() {
        // WHY locale 锁:文案断言依赖 En 前缀,并行测试切 locale 会偶发失败
        let _guard = crate::i18n::locale_test_guard();
        crate::i18n::set_locale(crate::i18n::Locale::En);
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 1));
        render_banner(ApprovalMode::Plan, Rect::new(0, 0, 40, 1), &mut buf);
        let row: String = (0..40).map(|x| buf[(x, 0)].symbol()).collect();
        assert!(row.contains("PLAN"), "Plan 横幅应含文案: {row}");

        // Normal 防御性无操作(不 panic、不写入)
        let mut buf2 = Buffer::empty(Rect::new(0, 0, 40, 1));
        render_banner(ApprovalMode::Normal, Rect::new(0, 0, 40, 1), &mut buf2);
        let row2: String = (0..40).map(|x| buf2[(x, 0)].symbol()).collect();
        assert_eq!(row2.trim(), "", "Normal 态不应写入横幅");

        // 零尺寸防御
        let mut buf3 = Buffer::empty(Rect::new(0, 0, 0, 0));
        render_banner(ApprovalMode::Auto, Rect::new(0, 0, 0, 0), &mut buf3);
    }
}
