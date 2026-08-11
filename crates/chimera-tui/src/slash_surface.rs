//! SlashCommandSurface — 斜杠命令补全面板(Concord W2 · T2.2)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - **纯函数模型**:候选计算/选中钳制/渲染均为无状态函数,输入 = (注册表,
//!   查询, 选中索引),便于单测穷举;状态(输入缓冲/选中索引)存于 `TuiState`。
//! - **数据源为 SlashCommandRegistry**:与 Ctrl+P 动作面板(CommandPaletteModel)
//!   交互同构但数据源不同——后者消费 ActionRegistry(≤40 动作),本面板消费
//!   命令表(53 命令三分层),故独立建模而非复用(R9 预算分层的一贯性)。
//! - **i18n 唯一直面文案来源**:候选行标题经 `i18n::tr` 解析,tier 标记走
//!   `slash.tier.*` 键,零硬编码 CJK(T2.5 门禁)。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::actions::slash_registry::{SlashCommandRegistry, SlashTier};

/// 补全候选 — 补全列表一行的展示模型
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashCandidate {
    /// 命令词(不含前导 `/`,子命令空格分隔)
    pub name: &'static str,
    /// 已解析为当前 locale 的命令标题
    pub title: String,
    /// 执行分层(展示 tier 标记)
    pub tier: SlashTier,
}

/// tier 的 i18n 展示键(即时/编排/Agent)
fn tier_label_key(tier: SlashTier) -> &'static str {
    match tier {
        SlashTier::Instant => "slash.tier.instant",
        SlashTier::Orchestrated => "slash.tier.orchestrated",
        SlashTier::Agent => "slash.tier.agent",
    }
}

/// 计算补全候选列表(模糊查询;空查询返回全部,注册序稳定)
///
/// # 参数
/// - `reg`:斜杠命令注册表(单一事实源)
/// - `query`:当前输入(不含前导 `/`;可为空)
pub fn candidates(reg: &SlashCommandRegistry, query: &str) -> Vec<SlashCandidate> {
    reg.fuzzy(query)
        .into_iter()
        .map(|d| SlashCandidate {
            name: d.name,
            title: crate::i18n::tr(d.title_key).to_string(),
            tier: d.tier,
        })
        .collect()
}

/// 选中索引钳制:列表收缩(输入变长)时防越界;空列表归零
pub fn clamp_selected(selected: usize, count: usize) -> usize {
    if count == 0 {
        0
    } else {
        selected.min(count - 1)
    }
}

/// 补全列表可见行数上限(边框 2 行 + 内容 N 行,避免遮挡主面板)
pub const MAX_VISIBLE_ROWS: usize = 10;

/// 渲染补全列表到给定区域(选中行高亮;超出行数滚动跟随选中项)
///
/// # 参数
/// - `cands`:候选列表(candidates 计算结果)
/// - `selected`:选中索引(调用方应先经 clamp_selected 钳制)
/// - `area`:渲染区域(由 app render 计算,位于底部输入栏上方)
/// - `buf`:目标缓冲
pub fn render_candidates(cands: &[SlashCandidate], selected: usize, area: Rect, buf: &mut Buffer) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(crate::t!("slash.surface.title"));
    let inner = block.inner(area);
    block.render(area, buf);
    if cands.is_empty() || inner.height == 0 || inner.width == 0 {
        return;
    }

    let page = (inner.height as usize).min(MAX_VISIBLE_ROWS);
    // 滚动偏移:选中项超出页尾时上移窗口(跟随选中,不回跳)
    let offset = selected.saturating_sub(page - 1);
    let sel_style = Style::default().add_modifier(Modifier::REVERSED);

    for (row, cand) in cands.iter().skip(offset).take(page).enumerate() {
        let idx = offset + row;
        let marker = format!("[{}]", crate::i18n::tr(tier_label_key(cand.tier)));
        let line = Line::from(vec![
            Span::styled(
                format!("{:<12} ", marker),
                Style::default().add_modifier(Modifier::DIM),
            ),
            Span::raw(format!("/{:<20} ", cand.name)),
            Span::styled(cand.title.clone(), Style::default()),
        ]);
        let y = inner.y + row as u16;
        if y >= inner.y + inner.height {
            break;
        }
        let row_area = Rect::new(inner.x, y, inner.width, 1);
        let line = if idx == selected {
            // 选中行:整行反色(REVERSED 作用于各 span)
            Line::from(
                line.spans
                    .into_iter()
                    .map(|s| Span::styled(s.content, s.style.patch(sel_style)))
                    .collect::<Vec<_>>(),
            )
        } else {
            line
        };
        Paragraph::new(line).render(row_area, buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn reg() -> SlashCommandRegistry {
        SlashCommandRegistry::with_builtin_commands()
    }

    #[test]
    fn candidates_empty_query_returns_all() {
        let r = reg();
        assert_eq!(candidates(&r, "").len(), r.len());
        assert_eq!(candidates(&r, "   ").len(), r.len());
    }

    #[test]
    fn candidates_fuzzy_filter_and_titles_resolved() {
        let _guard = crate::i18n::locale_test_guard();
        crate::i18n::set_locale(crate::i18n::Locale::Zh);
        let r = reg();
        let cands = candidates(&r, "theme");
        assert!(cands.iter().any(|c| c.name == "theme"));
        let theme = cands.iter().find(|c| c.name == "theme").unwrap();
        assert_eq!(theme.title, "切换主题", "标题应经 zh i18n 解析");
        assert_eq!(theme.tier, SlashTier::Instant);
        crate::i18n::set_locale(crate::i18n::Locale::Zh);
    }

    #[test]
    fn candidates_multi_word_query_hits_subcommands() {
        let r = reg();
        let cands = candidates(&r, "quest pause");
        assert!(cands.iter().any(|c| c.name == "quest pause"));
    }

    #[test]
    fn clamp_selected_bounds() {
        assert_eq!(clamp_selected(5, 0), 0, "空列表归零");
        assert_eq!(clamp_selected(5, 3), 2, "越界钳制到末项");
        assert_eq!(clamp_selected(1, 3), 1, "界内不变");
    }

    #[test]
    fn render_candidates_smoke_small_area() {
        // 冒烟:极小区域与空列表不 panic,渲染后缓冲有边框字符
        let mut buf = Buffer::empty(Rect::new(0, 0, 30, 5));
        let r = reg();
        let cands = candidates(&r, "");
        render_candidates(&cands, 0, Rect::new(0, 0, 30, 5), &mut buf);
        // 边框存在
        assert_ne!(buf[(0, 0)].symbol(), " ", "应渲染边框");
        // 空列表不 panic
        let mut buf2 = Buffer::empty(Rect::new(0, 0, 30, 5));
        render_candidates(&[], 0, Rect::new(0, 0, 30, 5), &mut buf2);
        // 零面积不 panic
        let mut buf3 = Buffer::empty(Rect::new(0, 0, 0, 0));
        render_candidates(&cands, 0, Rect::new(0, 0, 0, 0), &mut buf3);
    }

    proptest! {
        /// 属性:任意选中索引经钳制后必然落在 [0, count) 或归零(空列表)
        #[test]
        fn clamp_selected_always_in_range(selected in 0usize..1000, count in 0usize..100) {
            let c = clamp_selected(selected, count);
            if count == 0 {
                prop_assert_eq!(c, 0);
            } else {
                prop_assert!(c < count);
            }
        }

        /// 属性:候选列表规模恒等于 fuzzy 命中数(补全不增不减)
        #[test]
        fn candidates_len_matches_fuzzy(q in "[a-z ]{0,8}") {
            let r = reg();
            prop_assert_eq!(candidates(&r, &q).len(), r.fuzzy(&q).len());
        }
    }
}
