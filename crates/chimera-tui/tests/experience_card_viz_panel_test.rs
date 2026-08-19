//! ExperienceCardVizPanel 集成测试 — 面板注册 + 渲染闭环（v3.4.0 §15.2b）
//!
//! 覆盖: 面板注册可达（REGISTERED_FOCUS_ORDER）/ next/prev 往返 /
//! mock 提供者渲染 / 诚实展示 / i18n 标题键存在

#![forbid(unsafe_code)]

use std::sync::Arc;

use chimera_tui::panels::{
    ExperienceCardStatsProvider, ExperienceCardVizPanel, ExperienceCardVizStats, Panel,
};
use chimera_tui::types::PanelId;

/// Mock 统计提供者
#[derive(Debug, Default)]
struct MockProvider {
    stats: ExperienceCardVizStats,
}

impl ExperienceCardStatsProvider for MockProvider {
    fn global_stats(&self) -> ExperienceCardVizStats {
        self.stats.clone()
    }
}

// ----------------------------------------------------------
// 面板注册闭环（D-5 红线）
// ----------------------------------------------------------

#[test]
fn panel_registered_in_focus_order() {
    assert!(
        PanelId::REGISTERED_FOCUS_ORDER.contains(&PanelId::ExperienceCardViz),
        "ExperienceCardViz 应注册进焦点环"
    );
}

#[test]
fn panel_next_prev_roundtrip() {
    let panel = PanelId::ExperienceCardViz;
    assert_eq!(panel.next().prev(), panel);
    assert_eq!(panel.prev().next(), panel);
}

#[test]
fn panel_type_name_and_title() {
    assert_eq!(PanelId::ExperienceCardViz.as_str(), "ExperienceCardViz");
    assert_eq!(PanelId::ExperienceCardViz.title(), " Card Viz ");
}

#[test]
fn i18n_title_key_exists() {
    // 中英双语标题键存在（i18n_completeness 红线）
    let zh = chimera_tui::i18n::tr("panel.border.experience_card_viz");
    assert!(!zh.is_empty(), "zh 标题键应存在");
}

// ----------------------------------------------------------
// 渲染闭环
// ----------------------------------------------------------

#[test]
fn panel_id_and_honest_display_without_provider() {
    let panel = ExperienceCardVizPanel::new();
    assert_eq!(panel.id(), PanelId::ExperienceCardViz);
    // 未接线诚实展示（虚假数据治理先例）
    let content = panel.content().to_string();
    assert!(content.contains("Awaiting stats provider..."));
}

#[test]
fn panel_renders_provider_stats() {
    let panel = ExperienceCardVizPanel::with_provider(Arc::new(MockProvider {
        stats: ExperienceCardVizStats {
            total_cards: 100,
            evaluated: 80,
            unique_errors: 12,
            method_distribution: vec![
                ("Improve".to_string(), 50),
                ("Draft".to_string(), 30),
                ("Crossover".to_string(), 20),
            ],
            best_score: 0.95,
            average_score: 0.71,
        },
    }));
    let content = panel.content().to_string();
    assert!(content.contains("Total: 100"));
    assert!(content.contains("Evaluated: 80"));
    assert!(content.contains("Unique errors: 12"));
    assert!(content.contains("Best score: 0.95"));
    assert!(content.contains("Average score: 0.71"));
    assert!(content.contains("Improve"));
}

#[test]
fn panel_trait_object_dispatch() {
    // Box<dyn Panel> 动态分派验证（工厂 make_panel 同款路径）
    let panel: Box<dyn Panel> = Box::new(ExperienceCardVizPanel::new());
    assert_eq!(panel.id(), PanelId::ExperienceCardViz);
    assert!(!panel.shortcuts().is_empty());
}
