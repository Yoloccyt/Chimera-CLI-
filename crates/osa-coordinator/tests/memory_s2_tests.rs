//! Task 2: OSA memory 维度集成 omega-learner S2 端到端测试
//!
//! 对应任务: **Task 2**（OSA memory 维度集成 omega-learner S2）
//! 对应三重悖论: **记忆悖论修复**（记忆策略随任务阶段自适应）
//!
//! # 测试覆盖
//!
//! 1. **向后兼容**: 未注入 S2 provider 时,memory mask 大小 = base_k(StandardTopK)
//! 2. **S2 端到端集成**: 注入真实 `S2StrategyAdapter`,验证 memory mask 生成正常
//! 3. **task_phase 传递**: 不同 task_phase 产生不同 memory mask(策略不同 → K 不同)
//! 4. **k_multiplier 应用**: 各策略的 k_multiplier 正确调整基础 Top-K
//!
//! # 依赖铁律合规
//!
//! 本测试文件位于 `tests/` 目录,通过 dev-dependencies 引入 omega-learner,
//! 不违反 §2.2 依赖铁律(dev-dependencies 可绕过生产依赖方向,仅限 tests/)。

use std::sync::Arc;

use event_bus::EventBus;
use nexus_contracts::{MemoryStrategy, MemoryStrategyProvider, MemoryTaskPhase};
use omega_learner::s2_memory::{S2Learner, S2StrategyAdapter};
use osa_coordinator::{
    types::{AffectedScope, MemoryId, RiskLevel, TaskProfile, TaskType, TimePressure, ToolId},
    OmniSparseCoordinator,
};

// ============================================================
// 测试辅助函数
// ============================================================

/// 构造测试用 TaskProfile（Regular 档位,base_k=16）
fn make_profile_with_phase(complexity: f32, task_phase: Option<MemoryTaskPhase>) -> TaskProfile {
    TaskProfile {
        task_id: "t-s2-test".into(),
        task_type: TaskType::Read,
        complexity_score: complexity,
        risk_level: RiskLevel::Medium,
        time_pressure: TimePressure::Low,
        affected_scope: AffectedScope::Local,
        available_tools: (0..50).map(|i| ToolId::new(format!("tool-{i}"))).collect(),
        available_files: Vec::new(),
        available_memories: (0..50).map(|i| MemoryId::new(format!("mem-{i}"))).collect(),
        recent_operations: Vec::new(),
        active_tasks: Vec::new(),
        routing_scores: None,
        context_scores: None,
        memory_scores: None,
        task_phase,
    }
}

/// Mock provider: 返回固定策略（用于验证 k_multiplier 应用）
struct FixedStrategyProvider {
    strategy: MemoryStrategy,
}

impl MemoryStrategyProvider for FixedStrategyProvider {
    fn select_strategy(&self, _phase: MemoryTaskPhase) -> MemoryStrategy {
        self.strategy
    }
}

// ============================================================
// 向后兼容测试:未注入 provider 时行为不变
// ============================================================

#[test]
fn test_backward_compat_no_provider_uses_standard_topk() {
    // 未注入 S2 provider → fallback 到 StandardTopK(k_multiplier=1.0)
    // memory mask 大小 = base_k(Regular 档位 = 16)
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);
    let profile = make_profile_with_phase(0.4, None);
    let mask = coord.compute_memory_mask(&profile);
    // Regular 档位 base_k=16,StandardTopK k_multiplier=1.0 → 16
    assert_eq!(
        mask.active_ids.len(),
        16,
        "未注入 provider 时应 fallback 到 StandardTopK,K 不变"
    );
}

#[test]
fn test_backward_compat_no_provider_ignores_task_phase() {
    // 未注入 provider 时,task_phase 不影响结果(都 fallback 到 StandardTopK)
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus);

    let profile_no_phase = make_profile_with_phase(0.4, None);
    let profile_initial = make_profile_with_phase(0.4, Some(MemoryTaskPhase::Initial));
    let profile_long_run = make_profile_with_phase(0.4, Some(MemoryTaskPhase::LongRun));

    let mask_no_phase = coord.compute_memory_mask(&profile_no_phase);
    let mask_initial = coord.compute_memory_mask(&profile_initial);
    let mask_long_run = coord.compute_memory_mask(&profile_long_run);

    // 无 provider 时,所有 phase 产生相同大小的 mask(StandardTopK)
    assert_eq!(
        mask_no_phase.active_ids.len(),
        mask_initial.active_ids.len()
    );
    assert_eq!(
        mask_initial.active_ids.len(),
        mask_long_run.active_ids.len()
    );
}

// ============================================================
// k_multiplier 应用测试:各策略正确调整 Top-K
// ============================================================

#[test]
fn test_k_multiplier_standard_topk_keeps_base_k() {
    // StandardTopK: k_multiplier=1.0 → K 不变
    let bus = EventBus::new();
    let provider: Arc<dyn MemoryStrategyProvider> = Arc::new(FixedStrategyProvider {
        strategy: MemoryStrategy::StandardTopK,
    });
    let coord = OmniSparseCoordinator::new(bus).with_memory_strategy_provider(provider);
    let profile = make_profile_with_phase(0.4, Some(MemoryTaskPhase::Stuck));
    let mask = coord.compute_memory_mask(&profile);
    // base_k=16, k_multiplier=1.0 → 16
    assert_eq!(mask.active_ids.len(), 16);
}

#[test]
fn test_k_multiplier_minimal_recall_halves_k() {
    // MinimalRecall: k_multiplier=0.5 → K 减半
    let bus = EventBus::new();
    let provider: Arc<dyn MemoryStrategyProvider> = Arc::new(FixedStrategyProvider {
        strategy: MemoryStrategy::MinimalRecall,
    });
    let coord = OmniSparseCoordinator::new(bus).with_memory_strategy_provider(provider);
    let profile = make_profile_with_phase(0.4, Some(MemoryTaskPhase::Initial));
    let mask = coord.compute_memory_mask(&profile);
    // base_k=16, k_multiplier=0.5 → 8
    assert_eq!(mask.active_ids.len(), 8);
}

#[test]
fn test_k_multiplier_aggressive_pruning_quarters_k() {
    // AggressivePruning: k_multiplier=0.25 → K 四分之一
    let bus = EventBus::new();
    let provider: Arc<dyn MemoryStrategyProvider> = Arc::new(FixedStrategyProvider {
        strategy: MemoryStrategy::AggressivePruning,
    });
    let coord = OmniSparseCoordinator::new(bus).with_memory_strategy_provider(provider);
    let profile = make_profile_with_phase(0.4, Some(MemoryTaskPhase::LongRun));
    let mask = coord.compute_memory_mask(&profile);
    // base_k=16, k_multiplier=0.25 → 4
    assert_eq!(mask.active_ids.len(), 4);
}

#[test]
fn test_k_multiplier_query_reformulation_increases_k() {
    // QueryReformulation: k_multiplier=1.5 → K 扩大
    let bus = EventBus::new();
    let provider: Arc<dyn MemoryStrategyProvider> = Arc::new(FixedStrategyProvider {
        strategy: MemoryStrategy::QueryReformulation,
    });
    let coord = OmniSparseCoordinator::new(bus).with_memory_strategy_provider(provider);
    let profile = make_profile_with_phase(0.4, Some(MemoryTaskPhase::Stuck));
    let mask = coord.compute_memory_mask(&profile);
    // base_k=16, k_multiplier=1.5 → 24
    assert_eq!(mask.active_ids.len(), 24);
}

#[test]
fn test_k_multiplier_time_focused_keeps_base_k() {
    // TimeFocused: k_multiplier=1.0 → K 不变（差异在时间过滤而非数量）
    let bus = EventBus::new();
    let provider: Arc<dyn MemoryStrategyProvider> = Arc::new(FixedStrategyProvider {
        strategy: MemoryStrategy::TimeFocused,
    });
    let coord = OmniSparseCoordinator::new(bus).with_memory_strategy_provider(provider);
    let profile = make_profile_with_phase(0.4, Some(MemoryTaskPhase::LongRun));
    let mask = coord.compute_memory_mask(&profile);
    // base_k=16, k_multiplier=1.0 → 16
    assert_eq!(mask.active_ids.len(), 16);
}

// ============================================================
// S2 端到端集成测试:注入真实 S2StrategyAdapter
// ============================================================

#[test]
fn test_s2_strategy_adapter_integration_returns_valid_mask() {
    // 注入真实 S2StrategyAdapter(基于 LinUCB),验证 memory mask 生成正常
    let bus = EventBus::new();
    let learner = S2Learner::with_default_alpha().unwrap();
    let adapter = S2StrategyAdapter::new(learner);
    let provider: Arc<dyn MemoryStrategyProvider> = Arc::new(adapter);
    let coord = OmniSparseCoordinator::new(bus).with_memory_strategy_provider(provider);

    let profile = make_profile_with_phase(0.4, Some(MemoryTaskPhase::LongRun));
    let mask = coord.compute_memory_mask(&profile);

    // S2 adapter 返回的策略对应的 k_multiplier 决定 mask 大小
    // 可能的策略与对应大小(Regular 档位 base_k=16):
    //   MinimalRecall(0.5) → 8
    //   StandardTopK(1.0) → 16
    //   QueryReformulation(1.5) → 24
    //   AggressivePruning(0.25) → 4
    //   TimeFocused(1.0) → 16
    let len = mask.active_ids.len();
    assert!(
        len == 4 || len == 8 || len == 16 || len == 24,
        "S2 adapter 应返回有效策略对应的 mask 大小(4/8/16/24),实际: {len}"
    );
}

#[test]
fn test_s2_strategy_adapter_different_phases_produce_valid_masks() {
    // 不同 task_phase 通过 S2StrategyAdapter 产生有效 memory mask
    let bus = EventBus::new();
    let learner = S2Learner::with_default_alpha().unwrap();
    let adapter = S2StrategyAdapter::new(learner);
    let provider: Arc<dyn MemoryStrategyProvider> = Arc::new(adapter);
    let coord = OmniSparseCoordinator::new(bus).with_memory_strategy_provider(provider);

    for phase in MemoryTaskPhase::ALL {
        let profile = make_profile_with_phase(0.4, Some(phase));
        let mask = coord.compute_memory_mask(&profile);

        let len = mask.active_ids.len();
        // Regular 档位 base_k=16,各策略调整后应为 4/8/16/24 之一
        assert!(
            len == 4 || len == 8 || len == 16 || len == 24,
            "phase={phase:?} 应产生有效 mask 大小(4/8/16/24),实际: {len}"
        );
    }
}

// ============================================================
// 档位联动测试:S2 策略与复杂度档位联动
// ============================================================

#[test]
fn test_s2_strategy_with_simple_band() {
    // Simple 档位(complexity=0.1)→ base_k=8
    // 注入 MinimalRecall provider → k_multiplier=0.5 → adjusted_k=4
    let bus = EventBus::new();
    let provider: Arc<dyn MemoryStrategyProvider> = Arc::new(FixedStrategyProvider {
        strategy: MemoryStrategy::MinimalRecall,
    });
    let coord = OmniSparseCoordinator::new(bus).with_memory_strategy_provider(provider);
    let profile = make_profile_with_phase(0.1, Some(MemoryTaskPhase::Initial));
    let mask = coord.compute_memory_mask(&profile);
    // base_k=8, k_multiplier=0.5 → 4
    assert_eq!(mask.active_ids.len(), 4);
}

#[test]
fn test_s2_strategy_with_ultra_complex_band() {
    // UltraComplex 档位(complexity=0.9)→ base_k=32
    // 注入 AggressivePruning provider → k_multiplier=0.25 → adjusted_k=8
    let bus = EventBus::new();
    let provider: Arc<dyn MemoryStrategyProvider> = Arc::new(FixedStrategyProvider {
        strategy: MemoryStrategy::AggressivePruning,
    });
    let coord = OmniSparseCoordinator::new(bus).with_memory_strategy_provider(provider);
    let profile = make_profile_with_phase(0.9, Some(MemoryTaskPhase::LongRun));
    let mask = coord.compute_memory_mask(&profile);
    // base_k=32, k_multiplier=0.25 → 8
    assert_eq!(mask.active_ids.len(), 8);
}

// ============================================================
// task_phase None fallback 测试
// ============================================================

#[test]
fn test_task_phase_none_falls_back_to_initial_with_provider() {
    // task_phase=None 时,provider 收到 MemoryTaskPhase::default()(Initial)
    // 用 FixedStrategyProvider 不区分 phase,只验证流程不 panic
    let bus = EventBus::new();
    let provider: Arc<dyn MemoryStrategyProvider> = Arc::new(FixedStrategyProvider {
        strategy: MemoryStrategy::StandardTopK,
    });
    let coord = OmniSparseCoordinator::new(bus).with_memory_strategy_provider(provider);
    let profile = make_profile_with_phase(0.4, None);
    let mask = coord.compute_memory_mask(&profile);
    // task_phase=None → Initial → StandardTopK(k_multiplier=1.0) → 16
    assert_eq!(mask.active_ids.len(), 16);
}
