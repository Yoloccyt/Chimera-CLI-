//! TUI ExperienceCardViz 面板 — 经验卡片可视化(Phase 10 §15.2b, OpenMLE)
//!
//! 对应架构层:L10 Interface
//! 对应设计源:`Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §15.2
//! 对应论文:清华 OpenMLE(经验卡片全局统计 + 方法分布 + 错误聚类)
//!
//! # 设计决策(WHY)
//! - **数据经 `ExperienceCardStatsProvider` trait 注入**(D-1 偏差适配):
//!   规范原型直接持有 `mlc_engine::ExperienceCardSystem`,但 Phase 6 D-6 治理
//!   先例规定 chimera-tui 不直接依赖 L2+ crate;trait 注入保持解耦
//!   (MemorySyncHook/MemoryTidyHook 先例),统计实现由 mlc-engine 或调用方包装。
//! - **无提供者时诚实展示** "Awaiting stats provider...":杜绝虚假数据固化
//!   (SelfAssessment "Awaiting first HarnessReportGenerated" 先例)。
//! - **文本布局而非 viz widget**:与 SelfAssessment 面板同款纯文本实现,
//!   复用既有视觉语言。

use std::sync::Arc;

use crossterm::event::KeyEvent;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::panels::Panel;
use crate::types::{PanelId, TuiCommand, TuiState};

/// 方法分布最多展示条数(面板高度有限)
const MAX_METHODS_SHOWN: usize = 8;

/// 经验卡片全局统计 — 面板展示的自足统计类型(D-1)
///
/// WHY 自足类型:chimera-tui 不依赖 mlc-engine,提供者负责从
/// `ExperienceCardSystem::global_board()/method_stats()` 聚合为本类型。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ExperienceCardVizStats {
    /// 总卡片数
    pub total_cards: usize,
    /// 已评估卡片数
    pub evaluated: usize,
    /// 唯一错误签名数(错误聚类,铁律7)
    pub unique_errors: usize,
    /// 方法分布(method_family → 计数)
    pub method_distribution: Vec<(String, u32)>,
    /// 最佳分数
    pub best_score: f32,
    /// 平均分数
    pub average_score: f32,
}

/// 经验卡片统计提供者 — 依赖倒置注入点(D-1)
///
/// 由 mlc-engine `ExperienceCardSystem` 或调用方包装实现;
/// 未注入时面板诚实展示等待提示(不渲染虚假数据)。
pub trait ExperienceCardStatsProvider: Send + Sync + std::fmt::Debug {
    /// 采集当前全局统计快照
    fn global_stats(&self) -> ExperienceCardVizStats;
}

/// ExperienceCardViz 面板(规范 §15.2 经验卡片可视化)
#[derive(Debug, Default)]
pub struct ExperienceCardVizPanel {
    /// 统计提供者(None = 未接线,诚实展示等待提示)
    provider: Option<Arc<dyn ExperienceCardStatsProvider>>,
}

impl ExperienceCardVizPanel {
    /// 创建未接线面板(诚实展示等待提示)
    pub fn new() -> Self {
        Self { provider: None }
    }

    /// 注入统计提供者(接线后渲染真实统计)
    pub fn with_provider(provider: Arc<dyn ExperienceCardStatsProvider>) -> Self {
        Self {
            provider: Some(provider),
        }
    }

    /// 构建面板文本内容
    ///
    /// 有提供者 → 总卡片/已评估/唯一错误/方法分布/最佳分/平均分(规范 §15.2 格式);
    /// 无提供者 → "Awaiting stats provider..."(诚实展示)。
    pub fn content(&self) -> Text<'static> {
        let mut lines: Vec<Line<'static>> = vec![
            Line::from("Experience Card Viz (OpenMLE)"),
            Line::from("──────────────────────────────────────"),
        ];

        let Some(provider) = &self.provider else {
            lines.push(Line::from(Span::styled(
                "Awaiting stats provider...",
                Style::default().fg(Color::Gray),
            )));
            return Text::from(lines);
        };

        let stats = provider.global_stats();
        lines.push(Line::from(format!(
            "Total: {} | Evaluated: {} | Unique errors: {}",
            stats.total_cards, stats.evaluated, stats.unique_errors
        )));
        lines.push(Line::from(format!(
            "Best score: {:.2} | Average score: {:.2}",
            stats.best_score, stats.average_score
        )));
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Method distribution:",
            Style::default().add_modifier(Modifier::BOLD),
        )));

        if stats.method_distribution.is_empty() {
            lines.push(Line::from(Span::styled(
                "No cards recorded yet.",
                Style::default().fg(Color::Gray),
            )));
        } else {
            // 计数降序展示 Top-N(面板高度有限)
            let mut sorted = stats.method_distribution.clone();
            sorted.sort_by_key(|item| std::cmp::Reverse(item.1));
            for (method, count) in sorted.into_iter().take(MAX_METHODS_SHOWN) {
                lines.push(Line::from(format!("  {method:<20} {count}")));
            }
        }

        Text::from(lines)
    }
}

impl Panel for ExperienceCardVizPanel {
    fn id(&self) -> PanelId {
        PanelId::ExperienceCardViz
    }

    fn title(&self) -> Line<'static> {
        Line::from(crate::t!("panel.border.experience_card_viz"))
    }

    fn render(&mut self, _state: &TuiState, area: Rect, buf: &mut Buffer) {
        let block = Block::default().borders(Borders::ALL).title(self.title());
        let paragraph = Paragraph::new(self.content()).block(block);
        paragraph.render(area, buf);
    }

    fn handle_key(&mut self, _key: KeyEvent, _state: &mut TuiState) -> Option<TuiCommand> {
        // 展示型面板不处理专属按键(同 SelfAssessment 面板)
        None
    }

    fn shortcuts(&self) -> Vec<(&'static str, &'static str)> {
        vec![("Tab", crate::t!("shortcut.switch_panel"))]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mock 统计提供者 — 返回固定统计快照
    #[derive(Debug, Default)]
    struct MockProvider {
        stats: ExperienceCardVizStats,
    }

    impl ExperienceCardStatsProvider for MockProvider {
        fn global_stats(&self) -> ExperienceCardVizStats {
            self.stats.clone()
        }
    }

    #[test]
    fn panel_id_semantics() {
        let panel = ExperienceCardVizPanel::new();
        assert_eq!(panel.id(), PanelId::ExperienceCardViz);
    }

    #[test]
    fn content_awaiting_when_no_provider() {
        // 诚实展示:无提供者不渲染虚假统计
        let panel = ExperienceCardVizPanel::new();
        let content = panel.content().to_string();
        assert!(content.contains("Awaiting stats provider..."));
    }

    #[test]
    fn content_renders_stats_with_provider() {
        let panel = ExperienceCardVizPanel::with_provider(Arc::new(MockProvider {
            stats: ExperienceCardVizStats {
                total_cards: 42,
                evaluated: 30,
                unique_errors: 5,
                method_distribution: vec![
                    ("Improve".to_string(), 20),
                    ("Draft".to_string(), 15),
                    ("Debug".to_string(), 7),
                ],
                best_score: 0.92,
                average_score: 0.65,
            },
        }));
        let content = panel.content().to_string();
        assert!(content.contains("Total: 42"));
        assert!(content.contains("Evaluated: 30"));
        assert!(content.contains("Unique errors: 5"));
        assert!(content.contains("Best score: 0.92"));
        assert!(content.contains("Average score: 0.65"));
    }

    #[test]
    fn method_distribution_sorted_desc() {
        let panel = ExperienceCardVizPanel::with_provider(Arc::new(MockProvider {
            stats: ExperienceCardVizStats {
                method_distribution: vec![("Draft".to_string(), 5), ("Improve".to_string(), 20)],
                ..Default::default()
            },
        }));
        let content = panel.content().to_string();
        // Improve(20) 应排在 Draft(5) 之前
        let improve_pos = content.find("Improve").expect("Improve 存在");
        let draft_pos = content.find("Draft").expect("Draft 存在");
        assert!(improve_pos < draft_pos, "计数降序排列");
    }

    #[test]
    fn empty_distribution_honest_message() {
        let panel = ExperienceCardVizPanel::with_provider(Arc::new(MockProvider::default()));
        let content = panel.content().to_string();
        assert!(content.contains("No cards recorded yet."));
    }
}
