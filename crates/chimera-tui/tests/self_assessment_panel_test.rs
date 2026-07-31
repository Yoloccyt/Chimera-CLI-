//! Task 3.2: L2 Memory 协同 — SelfAssessmentPanel 记忆策略阶段集成测试
//!
//! 验证 SelfAssessmentPanel 调用 `mlc_engine::current_memory_stage()` 显示
//! 当前记忆策略阶段,实现 L10 Panel ↔ L2 Memory 真实数据闭环。
//!
//! # 测试策略
//! - 测试 1(稳定): 验证面板显示 "Memory Strategy Stage:" 行且值合法
//! - 测试 2(串行): 验证 MemoryStage 切换时面板内容实时更新
//!
//! # 竞态说明
//! `current_memory_stage()` 读取进程级全局快照(`OnceLock<RwLock<MemoryStage>>`),
//! `cargo test --workspace` 时 mlc-engine 单元测试可能并行更新全局快照导致偶发失败。
//! 建议用 `cargo test -p chimera-tui --test self_assessment_panel_test` 单独运行,
//! 或通过 `STAGE_TEST_LOCK` 保证当前文件内串行。

use std::sync::Mutex;

use chimera_tui::panels::SelfAssessmentPanel;
use chimera_tui::types::TuiState;
use mlc_engine::current_memory_stage;
use mlc_engine::memory_strategy_learner::MemoryStrategyLearnerHolder;
use mlc_engine::{MemoryStrategy, MemoryStrategyPolicy};

/// 串行化当前文件内的全局快照操作,避免测试间竞态
static STAGE_TEST_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn test_self_assessment_panel_shows_memory_stage() {
    let _guard = STAGE_TEST_LOCK.lock().unwrap();

    let state = TuiState::new();
    let content = SelfAssessmentPanel::content(&state).to_string();

    // 面板应包含 "Memory Strategy Stage:" 行
    assert!(
        content.contains("Memory Strategy Stage:"),
        "面板应显示记忆策略阶段,实际内容: {content}"
    );

    // 阶段值应为 5 个合法 short_name 之一
    // (minimal/standard/reformulation/pruning/time-focused)
    let valid_stages = [
        "minimal",
        "standard",
        "reformulation",
        "pruning",
        "time-focused",
    ];
    let has_valid_stage = valid_stages
        .iter()
        .any(|s| content.contains(&format!("Memory Strategy Stage: {s}")));
    assert!(
        has_valid_stage,
        "面板应显示合法记忆策略阶段值,实际内容: {content}"
    );
}

#[test]
fn test_memory_stage_updates_when_strategy_changes() {
    let _guard = STAGE_TEST_LOCK.lock().unwrap();

    // 1. 更新策略为 TimeFocused,验证全局快照同步
    let holder = MemoryStrategyLearnerHolder::new();
    holder.update_policy(MemoryStrategyPolicy::learned(
        42,
        MemoryStrategy::TimeFocused,
    ));

    let stage = current_memory_stage();
    assert_eq!(
        stage,
        MemoryStrategy::TimeFocused,
        "holder.update_policy(TimeFocused) 后全局快照应同步更新"
    );

    // 2. 面板应显示新策略 time-focused
    let state = TuiState::new();
    let content = SelfAssessmentPanel::content(&state).to_string();
    assert!(
        content.contains("Memory Strategy Stage: time-focused"),
        "面板应显示更新后的记忆策略阶段(time-focused),实际内容: {content}"
    );

    // 3. 切换回 StandardTopK(fallback),验证面板实时更新
    holder.fallback_to_static();

    let content2 = SelfAssessmentPanel::content(&state).to_string();
    assert!(
        content2.contains("Memory Strategy Stage: standard"),
        "面板应显示回退后的记忆策略阶段(standard),实际内容: {content2}"
    );
}
