//! OSA 全维稀疏协调器属性测试 — 验证 sparsity 与 complexity_score 的不变量
//!
//! 对应 SubTask 15.12:引入 proptest 属性测试
//!
//! # 验证的不变量
//! 1. `TaskProfile::sparsity() + complexity_score == 1.0`(恒等式)
//! 2. routing 维度的 active_count 随 complexity_score 单调非递减
//!    (复杂度越高 → 档位越高 → Top-K 越大 → 保留更多工具)
//! 3. `OmniSparseMasks::average_sparsity()` ∈ [0.0, 1.0]
//!
//! # 实际不变量分析(源码确认)
//! - `TaskProfile::sparsity()` 返回 `1.0 - complexity_score`(types.rs:352)
//! - routing 的 k 由档位决定:Simple=8, Regular=16, Complex=24, UltraComplex=32
//! - 档位随 complexity_score 非递减 → k 非递减 → active_count 非递减

#![forbid(unsafe_code)]

use event_bus::EventBus;
use osa_coordinator::{
    AffectedScope, FileId, MemoryId, OmniSparseCoordinator, OmniSparseMasks, OperationId,
    RiskLevel, TaskId, TaskProfile, TaskType, TimePressure, ToolId,
};
use proptest::prelude::*;

/// 将任意可显示错误转换为 TestCaseError(避免 unwrap,用 ? 传播)
fn fail<E: std::fmt::Display>(e: E) -> TestCaseError {
    TestCaseError::fail(format!("{e}"))
}

/// 构造测试用 TaskProfile,固定候选集大小,仅 complexity 变化
///
/// WHY 固定候选集:隔离 complexity_score 的影响,确保 active_count 变化仅由档位驱动
fn make_profile(complexity: f32) -> TaskProfile {
    TaskProfile {
        // v1.5.0 新增语义评分字段(proptest 使用默认 None)
        tool_scores: None,
        file_scores: None,
        memory_scores: None,
        operation_scores: None,
        task_scores: None,
        task_id: TaskId::new("t-1"),
        task_type: TaskType::Read,
        complexity_score: complexity,
        risk_level: RiskLevel::Low,
        time_pressure: TimePressure::Low,
        affected_scope: AffectedScope::Local,
        available_tools: (0..50).map(|i| ToolId::new(format!("tool-{i}"))).collect(),
        available_files: (0..2000)
            .map(|i| FileId::new(format!("file-{i}")))
            .collect(),
        available_memories: (0..50).map(|i| MemoryId::new(format!("mem-{i}"))).collect(),
        recent_operations: (0..100)
            .map(|i| OperationId::new(format!("op-{i}")))
            .collect(),
        active_tasks: (0..10).map(|i| TaskId::new(format!("task-{i}"))).collect(),
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// 不变量 1:TaskProfile::sparsity() + complexity_score == 1.0
    ///
    /// TaskProfile::sparsity() 的实现为 `1.0 - complexity_score`(types.rs:352),
    /// 此恒等式对任意合法 complexity_score ∈ [0.0, 1.0] 成立
    #[test]
    fn test_sparsity_plus_complexity_equals_one(
        complexity in 0.0f32..=1.0f32,
    ) {
        let profile = make_profile(complexity);
        let sparsity = profile.sparsity();
        let sum = sparsity + complexity;
        prop_assert!(
            (sum - 1.0).abs() < 1e-6,
            "sparsity({}) + complexity({}) = {} != 1.0",
            sparsity, complexity, sum
        );
    }

    /// 不变量 2:routing active_count 随 complexity_score 单调非递减
    ///
    /// 复杂度越高 → ComplexityBand 档位越高 → routing_top_k 越大 → active_count 越大
    /// 档位映射:Simple(8) → Regular(16) → Complex(24) → UltraComplex(32)
    /// 对 c1 ≤ c2,active_count(c1) ≤ active_count(c2)
    #[test]
    fn test_routing_active_count_non_decreasing_with_complexity(
        c1 in 0.0f32..=1.0f32,
        c2 in 0.0f32..=1.0f32,
    ) {
        // 确保 c1 ≤ c2
        let (c1, c2) = if c1 <= c2 { (c1, c2) } else { (c2, c1) };

        let bus = EventBus::new();
        let coord = OmniSparseCoordinator::new(bus);

        let profile1 = make_profile(c1);
        let profile2 = make_profile(c2);

        let mask1 = coord.compute_routing_mask(&profile1);
        let mask2 = coord.compute_routing_mask(&profile2);

        prop_assert!(
            mask1.active_count() <= mask2.active_count(),
            "routing active_count(complexity={}) = {} 应 <= active_count(complexity={}) = {}",
            c1, mask1.active_count(), c2, mask2.active_count()
        );
    }

    /// 不变量 3:context active_count 随 complexity_score 单调非递减
    ///
    /// context 维度的 k 由档位决定:Simple=1, Regular=10, Complex=100, UltraComplex=1000
    #[test]
    fn test_context_active_count_non_decreasing_with_complexity(
        c1 in 0.0f32..=1.0f32,
        c2 in 0.0f32..=1.0f32,
    ) {
        let (c1, c2) = if c1 <= c2 { (c1, c2) } else { (c2, c1) };

        let bus = EventBus::new();
        let coord = OmniSparseCoordinator::new(bus);

        let mask1 = coord.compute_context_mask(&make_profile(c1));
        let mask2 = coord.compute_context_mask(&make_profile(c2));

        prop_assert!(
            mask1.active_count() <= mask2.active_count(),
            "context active_count(complexity={}) = {} 应 <= active_count(complexity={}) = {}",
            c1, mask1.active_count(), c2, mask2.active_count()
        );
    }

    /// 不变量 4:OmniSparseMasks::average_sparsity() ∈ [0.0, 1.0]
    ///
    /// 五维度 sparsity_ratio 的平均值应在 [0.0, 1.0] 范围内
    #[test]
    fn test_average_sparsity_in_unit_range(
        complexity in 0.0f32..=1.0f32,
    ) {
        let bus = EventBus::new();
        let coord = OmniSparseCoordinator::new(bus);
        let profile = make_profile(complexity);

        let routing = coord.compute_routing_mask(&profile);
        let context = coord.compute_context_mask(&profile);
        let memory = coord.compute_memory_mask(&profile);
        let audit = coord.compute_audit_mask(&profile);
        let budget = coord.compute_budget_mask(&profile);

        let masks = OmniSparseMasks::new(routing, context, memory, audit, budget).map_err(fail)?;
        let avg = masks.average_sparsity();

        prop_assert!(
            (0.0..=1.0).contains(&avg),
            "average_sparsity = {} 超出 [0.0, 1.0] (complexity={})",
            avg, complexity
        );
    }

    /// 不变量 5:每个维度的 sparsity_ratio ∈ [0.0, 1.0]
    ///
    /// SparseMask::select_top_k 计算的 sparsity = 1.0 - k/total,
    /// clamp 到 [0.0, 1.0](k=0 → 1.0,k≥total → 0.0)
    #[test]
    fn test_each_dimension_sparsity_in_unit_range(
        complexity in 0.0f32..=1.0f32,
    ) {
        let bus = EventBus::new();
        let coord = OmniSparseCoordinator::new(bus);
        let profile = make_profile(complexity);

        let routing = coord.compute_routing_mask(&profile);
        let context = coord.compute_context_mask(&profile);
        let memory = coord.compute_memory_mask(&profile);
        let audit = coord.compute_audit_mask(&profile);
        let budget = coord.compute_budget_mask(&profile);

        for (name, sparsity) in [
            ("routing", routing.sparsity_ratio),
            ("context", context.sparsity_ratio),
            ("memory", memory.sparsity_ratio),
            ("audit", audit.sparsity_ratio),
            ("budget", budget.sparsity_ratio),
        ] {
            prop_assert!(
                (0.0..=1.0).contains(&sparsity),
                "{} 维度 sparsity_ratio = {} 超出 [0.0, 1.0] (complexity={})",
                name, sparsity, complexity
            );
        }
    }

    /// 不变量 6:complexity 越高,average_sparsity 越低(单调非递增)
    ///
    /// 复杂度越高 → 各维度保留更多活跃项 → sparsity_ratio 更低 → 平均稀疏度更低
    #[test]
    fn test_average_sparsity_non_increasing_with_complexity(
        c1 in 0.0f32..=1.0f32,
        c2 in 0.0f32..=1.0f32,
    ) {
        let (c1, c2) = if c1 <= c2 { (c1, c2) } else { (c2, c1) };

        let bus = EventBus::new();
        let coord = OmniSparseCoordinator::new(bus);

        let compute_avg = |c: f32| -> Result<f32, TestCaseError> {
            let profile = make_profile(c);
            let routing = coord.compute_routing_mask(&profile);
            let context = coord.compute_context_mask(&profile);
            let memory = coord.compute_memory_mask(&profile);
            let audit = coord.compute_audit_mask(&profile);
            let budget = coord.compute_budget_mask(&profile);
            let masks = OmniSparseMasks::new(routing, context, memory, audit, budget).map_err(fail)?;
            Ok(masks.average_sparsity())
        };

        let avg1 = compute_avg(c1)?;
        let avg2 = compute_avg(c2)?;

        prop_assert!(
            avg1 >= avg2,
            "average_sparsity(complexity={}) = {} 应 >= average_sparsity(complexity={}) = {}",
            c1, avg1, c2, avg2
        );
    }
}
