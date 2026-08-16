//! Phase 6 D-6 治理:SelfAssessmentPanel 记忆策略阶段事件驱动化集成测试
//!
//! 原测试固化已删除的 `mlc_engine::current_memory_stage()` 全局占位函数
//! (虚假数据固化,测试固化错误断言)。治理后面板从 `latest_events` 事件流
//! 派生最近 `MemConStrategyAdjusted` 事件的 `to_strategy`;无事件时显示 N/A。
//!
//! # 测试策略
//! - 测试 1: 无事件时面板诚实显示 "Memory Strategy Stage: N/A"
//! - 测试 2: 注入 MemConStrategyAdjusted 事件后面板显示事件携带的策略阶段
//! - 测试 3: 多事件时取最近一条(反向扫描语义)

use std::collections::VecDeque;

use chimera_tui::panels::SelfAssessmentPanel;
use chimera_tui::types::TuiState;
use event_bus::{EventMetadata, NexusEvent};

fn strategy_event(from: &str, to: &str, reason: &str) -> NexusEvent {
    NexusEvent::MemConStrategyAdjusted {
        metadata: EventMetadata::new("mlc-engine"),
        from_strategy: from.into(),
        to_strategy: to.into(),
        reason: reason.into(),
        ghost_rate: None,
    }
}

#[test]
fn test_no_events_shows_na_placeholder() {
    let state = TuiState::new();
    let content = SelfAssessmentPanel::content(&state).to_string();
    // 无事件时诚实显示 N/A(不虚报全局快照)
    assert!(
        content.contains("Memory Strategy Stage: N/A"),
        "无 MemConStrategyAdjusted 事件时应显示 N/A,实际内容: {content}"
    );
}

#[test]
fn test_strategy_event_derives_stage() {
    let state = TuiState {
        latest_events: VecDeque::from(vec![strategy_event(
            "StandardTopK",
            "AggressivePruning",
            "ghost_memory_detected",
        )]),
        ..Default::default()
    };
    let content = SelfAssessmentPanel::content(&state).to_string();
    assert!(
        content.contains("Memory Strategy Stage: AggressivePruning"),
        "面板应显示事件携带的策略阶段,实际内容: {content}"
    );
}

#[test]
fn test_latest_event_wins() {
    // 反向扫描:最近一条事件的 to_strategy 生效
    let state = TuiState {
        latest_events: VecDeque::from(vec![
            strategy_event("StandardTopK", "AggressivePruning", "ghost_memory_detected"),
            strategy_event("AggressivePruning", "StandardTopK", "stable_recovery"),
        ]),
        ..Default::default()
    };
    let content = SelfAssessmentPanel::content(&state).to_string();
    assert!(
        content.contains("Memory Strategy Stage: StandardTopK"),
        "应取最近事件的策略阶段,实际内容: {content}"
    );
    assert!(
        !content.contains("Memory Strategy Stage: AggressivePruning"),
        "旧事件的策略不应覆盖最近事件"
    );
}
