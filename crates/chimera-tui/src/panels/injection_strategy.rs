//! TUI InjectionStrategy 面板 — 上下文注入策略可视化(Phase 10 §15.3, TencentDB)
//!
//! 对应架构层:L10 Interface
//! 对应设计源:`Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §15.3
//! 对应论文:TencentDB Agent Memory(注入策略优化:动态卡片放用户消息前每轮都变,
//! 人格摘要放系统提示末尾几轮才变,利用缓存)
//!
//! # 设计决策(WHY)
//! - **数据经 `InjectionSnapshotProvider` trait 注入**(D-1 偏差适配):
//!   规范原型直接持有注入状态,但 Phase 6 D-6 治理先例规定 chimera-tui 不直接
//!   依赖 L2+ crate;快照由 mlc-engine 或调用方提供。
//! - **消费 L0 `AtomicMemoryCard`**(D-2):nexus-contracts 为 L0 纯类型契约层,
//!   L10→L0 向下合规,不引入耦合。
//! - **无提供者时诚实展示**:杜绝虚假数据固化(同 ExperienceCardViz 先例)。

use std::sync::Arc;

use crossterm::event::KeyEvent;
use nexus_contracts::memory_pyramid::AtomicMemoryCard;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::panels::Panel;
use crate::types::{PanelId, TuiCommand, TuiState};

/// 动态卡片列表最多展示条数(面板高度有限)
const MAX_CARDS_SHOWN: usize = 6;

/// 注入策略快照 — 面板展示的自足快照类型(D-1)
#[derive(Debug, Clone, Default)]
pub struct InjectionSnapshot {
    /// 当前动态卡片(每轮更新,注入用户消息前)
    pub dynamic_cards: Vec<AtomicMemoryCard>,
    /// 人格摘要(几轮才变,注入系统提示末尾利用缓存;None = 无摘要)
    pub persona_summary: Option<String>,
    /// 缓存命中率 ∈ [0,1]
    pub cache_hit_rate: f32,
    /// Token 节省量
    pub token_savings: u32,
}

/// 注入策略快照提供者 — 依赖倒置注入点(D-1)
///
/// 由 mlc-engine 注入策略模块或调用方包装实现;
/// 未注入时面板诚实展示等待提示。
pub trait InjectionSnapshotProvider: Send + Sync + std::fmt::Debug {
    /// 采集当前注入策略快照
    fn snapshot(&self) -> InjectionSnapshot;
}

/// InjectionStrategy 面板(规范 §15.3 上下文注入策略)
#[derive(Debug, Default)]
pub struct InjectionStrategyPanel {
    /// 快照提供者(None = 未接线,诚实展示等待提示)
    provider: Option<Arc<dyn InjectionSnapshotProvider>>,
}

impl InjectionStrategyPanel {
    /// 创建未接线面板(诚实展示等待提示)
    pub fn new() -> Self {
        Self { provider: None }
    }

    /// 注入快照提供者(接线后渲染真实快照)
    pub fn with_provider(provider: Arc<dyn InjectionSnapshotProvider>) -> Self {
        Self {
            provider: Some(provider),
        }
    }

    /// 构建面板文本内容(规范 §15.3 三段结构)
    ///
    /// 1. 动态卡片列表(`[card_type] scene: content` 格式,用户消息前注入)
    /// 2. 人格摘要(系统提示末尾注入,无则 "None")
    /// 3. 缓存统计(命中率百分比 + Token 节省 + 策略说明)
    pub fn content(&self) -> Text<'static> {
        let mut lines: Vec<Line<'static>> = vec![
            Line::from("Injection Strategy (TencentDB)"),
            Line::from("──────────────────────────────────────"),
        ];

        let Some(provider) = &self.provider else {
            lines.push(Line::from(Span::styled(
                "Awaiting injection snapshot provider...",
                Style::default().fg(Color::Gray),
            )));
            return Text::from(lines);
        };

        let snap = provider.snapshot();

        // 段 1:动态卡片(用户消息前,每轮更新)
        lines.push(Line::from(Span::styled(
            "Dynamic cards (before user msg):",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        if snap.dynamic_cards.is_empty() {
            lines.push(Line::from(Span::styled(
                "  No dynamic cards.",
                Style::default().fg(Color::Gray),
            )));
        } else {
            for card in snap.dynamic_cards.iter().take(MAX_CARDS_SHOWN) {
                lines.push(Line::from(format!(
                    "  [{:?}] {}: {}",
                    card.card_type, card.scene, card.content
                )));
            }
        }

        // 段 2:人格摘要(系统提示末尾,几轮才变利用缓存)
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Persona summary (system prompt tail):",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        match &snap.persona_summary {
            Some(summary) => lines.push(Line::from(format!("  {summary}"))),
            None => lines.push(Line::from(Span::styled(
                "  None",
                Style::default().fg(Color::Gray),
            ))),
        }

        // 段 3:缓存统计
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Cache stats:",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(format!(
            "  Cache hit rate: {:.1}% | Token savings: {}",
            snap.cache_hit_rate.clamp(0.0, 1.0) * 100.0,
            snap.token_savings
        )));
        lines.push(Line::from(Span::styled(
            "  Strategy: dynamic cards refresh every turn | persona reuses cache",
            Style::default().fg(Color::Gray),
        )));

        Text::from(lines)
    }
}

impl Panel for InjectionStrategyPanel {
    fn id(&self) -> PanelId {
        PanelId::InjectionStrategy
    }

    fn title(&self) -> Line<'static> {
        Line::from(crate::t!("panel.border.injection_strategy"))
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
    use nexus_contracts::memory_pyramid::AtomicCardType;

    /// Mock 快照提供者 — 返回固定快照
    #[derive(Debug, Default)]
    struct MockProvider {
        snap: InjectionSnapshot,
    }

    impl InjectionSnapshotProvider for MockProvider {
        fn snapshot(&self) -> InjectionSnapshot {
            self.snap.clone()
        }
    }

    fn card(scene: &str, content: &str) -> AtomicMemoryCard {
        AtomicMemoryCard::new(
            "card-1",
            AtomicCardType::Preference,
            128,
            scene,
            content,
            None,
            None,
            None,
            None,
            0,
        )
    }

    #[test]
    fn panel_id_semantics() {
        let panel = InjectionStrategyPanel::new();
        assert_eq!(panel.id(), PanelId::InjectionStrategy);
    }

    #[test]
    fn content_awaiting_when_no_provider() {
        let panel = InjectionStrategyPanel::new();
        let content = panel.content().to_string();
        assert!(content.contains("Awaiting injection snapshot provider..."));
    }

    #[test]
    fn content_renders_three_sections() {
        let panel = InjectionStrategyPanel::with_provider(Arc::new(MockProvider {
            snap: InjectionSnapshot {
                dynamic_cards: vec![card("coding", "prefer rust")],
                persona_summary: Some("Senior Rust engineer".to_string()),
                cache_hit_rate: 0.75,
                token_savings: 1234,
            },
        }));
        let content = panel.content().to_string();
        // 段 1:动态卡片
        assert!(content.contains("[Preference] coding: prefer rust"));
        // 段 2:人格摘要
        assert!(content.contains("Senior Rust engineer"));
        // 段 3:缓存统计
        assert!(content.contains("Cache hit rate: 75.0%"));
        assert!(content.contains("Token savings: 1234"));
    }

    #[test]
    fn persona_none_honest_message() {
        let panel = InjectionStrategyPanel::with_provider(Arc::new(MockProvider::default()));
        let content = panel.content().to_string();
        assert!(content.contains("No dynamic cards."));
        assert!(content.contains("None"));
    }
}
