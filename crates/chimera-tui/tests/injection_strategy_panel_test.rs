//! InjectionStrategyPanel 集成测试 — 面板注册 + 三段渲染闭环（v3.4.0 §15.3）
//!
//! 覆盖: 面板注册可达（REGISTERED_FOCUS_ORDER）/ next/prev 往返 /
//! mock 快照三段渲染 / 诚实展示 / L0 AtomicMemoryCard 消费

#![forbid(unsafe_code)]

use std::sync::Arc;

use chimera_tui::panels::{
    InjectionSnapshot, InjectionSnapshotProvider, InjectionStrategyPanel, Panel,
};
use chimera_tui::types::PanelId;
use nexus_contracts::memory_pyramid::{AtomicCardType, AtomicMemoryCard};

/// Mock 快照提供者
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

// ----------------------------------------------------------
// 面板注册闭环（D-5 红线）
// ----------------------------------------------------------

#[test]
fn panel_registered_in_focus_order() {
    assert!(
        PanelId::REGISTERED_FOCUS_ORDER.contains(&PanelId::InjectionStrategy),
        "InjectionStrategy 应注册进焦点环"
    );
}

#[test]
fn panel_next_prev_roundtrip() {
    let panel = PanelId::InjectionStrategy;
    assert_eq!(panel.next().prev(), panel);
    assert_eq!(panel.prev().next(), panel);
}

#[test]
fn panel_type_name_and_title() {
    assert_eq!(PanelId::InjectionStrategy.as_str(), "InjectionStrategy");
    assert_eq!(PanelId::InjectionStrategy.title(), " Injection ");
}

#[test]
fn i18n_title_key_exists() {
    let zh = chimera_tui::i18n::tr("panel.border.injection_strategy");
    assert!(!zh.is_empty(), "zh 标题键应存在");
}

// ----------------------------------------------------------
// 渲染闭环
// ----------------------------------------------------------

#[test]
fn panel_id_and_honest_display_without_provider() {
    let panel = InjectionStrategyPanel::new();
    assert_eq!(panel.id(), PanelId::InjectionStrategy);
    let content = panel.content().to_string();
    assert!(content.contains("Awaiting injection snapshot provider..."));
}

#[test]
fn panel_renders_three_sections_with_provider() {
    let panel = InjectionStrategyPanel::with_provider(Arc::new(MockProvider {
        snap: InjectionSnapshot {
            dynamic_cards: vec![card("coding", "prefer rust idioms")],
            persona_summary: Some("Senior systems engineer".to_string()),
            cache_hit_rate: 0.62,
            token_savings: 8500,
        },
    }));
    let content = panel.content().to_string();
    // 段 1:动态卡片（L0 AtomicMemoryCard 消费）
    assert!(content.contains("[Preference] coding: prefer rust idioms"));
    // 段 2:人格摘要
    assert!(content.contains("Senior systems engineer"));
    // 段 3:缓存统计
    assert!(content.contains("Cache hit rate: 62.0%"));
    assert!(content.contains("Token savings: 8500"));
}

#[test]
fn panel_honest_defaults_with_empty_snapshot() {
    let panel = InjectionStrategyPanel::with_provider(Arc::new(MockProvider::default()));
    let content = panel.content().to_string();
    assert!(content.contains("No dynamic cards."));
    assert!(content.contains("None"));
    assert!(content.contains("Cache hit rate: 0.0%"));
}

#[test]
fn panel_trait_object_dispatch() {
    let panel: Box<dyn Panel> = Box::new(InjectionStrategyPanel::new());
    assert_eq!(panel.id(), PanelId::InjectionStrategy);
    assert!(!panel.shortcuts().is_empty());
}
