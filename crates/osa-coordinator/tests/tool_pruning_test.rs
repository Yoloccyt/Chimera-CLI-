//! 工具 Schema 裁剪集成测试 — 顶层 API + Dressage 实证 + snapshot 治理协同（v3.4.0 §11.3）
//!
//! 覆盖: 顶层 API 可达性（re-export 验证）/ 33→4 实证端到端 /
//! D-6 占位治理（snapshot 真实数据源）/ proptest 裁剪不变量

#![forbid(unsafe_code)]

use event_bus::EventBus;
use osa_coordinator::{
    OmniSparseCoordinator, PruneStep, PruneToolSchema, PruneTrajectory, RiskLevel, TaskProfile,
    ToolSchemaPruner,
};
use proptest::prelude::*;

fn traj(tool: &str, success: bool) -> PruneTrajectory {
    PruneTrajectory {
        steps: vec![PruneStep {
            tool_name: Some(tool.to_string()),
            success,
        }],
    }
}

fn schema(name: &str, tokens: u32) -> PruneToolSchema {
    PruneToolSchema {
        name: name.to_string(),
        schema_tokens: tokens,
    }
}

// ----------------------------------------------------------
// 顶层 API 可达性（re-export 验证）
// ----------------------------------------------------------

#[test]
fn top_level_api_accessible() {
    let pruner = ToolSchemaPruner::new(0.3, 2);
    assert_eq!(pruner.pruning_threshold(), 0.3);
    assert_eq!(pruner.min_tools(), 2);
    assert!(pruner.usage_stats().is_empty());
}

// ----------------------------------------------------------
// Dressage 实证: 33 → 4 工具，13.5K → 1.7K tokens
// ----------------------------------------------------------

#[test]
fn dressage_scenario_end_to_end() {
    let mut pruner = ToolSchemaPruner::new(0.99, 0);
    // 4 个高频核心工具（各 10 次成功调用）
    for i in 0..4 {
        for _ in 0..10 {
            pruner.analyze_trajectories(&[traj(&format!("core-{i}"), true)]);
        }
    }
    // 29 个低频工具（各 1 次失败调用）
    for i in 0..29 {
        pruner.analyze_trajectories(&[traj(&format!("rare-{i}"), false)]);
    }
    // 33 个工具 schema（核心 425 tokens/个，低频 400 tokens/个）
    let mut available: Vec<PruneToolSchema> =
        (0..4).map(|i| schema(&format!("core-{i}"), 425)).collect();
    for i in 0..29 {
        available.push(schema(&format!("rare-{i}"), 400));
    }
    let total: u32 = available.iter().map(|t| t.schema_tokens).sum();
    assert_eq!(total, 4 * 425 + 29 * 400); // 13.3K tokens 量级（接近实证 13.5K）

    // 裁剪到 4 个
    let result = pruner.prune_tools(&available, 4);
    assert_eq!(result.kept.len(), 4, "33 → 4 裁剪");
    assert_eq!(result.pruned_count, 29);
    // 保留的必须是高频核心工具
    assert!(result.kept.iter().all(|t| t.name.starts_with("core-")));
    // token 收益: 裁掉 29 个低频工具
    assert_eq!(result.tokens_saved, 29 * 400);
    let kept_tokens: u32 = result.kept.iter().map(|t| t.schema_tokens).sum();
    assert_eq!(kept_tokens, 4 * 425); // 1.7K tokens 量级（接近实证 1.7K）
}

// ----------------------------------------------------------
// D-6 占位治理: snapshot 真实数据源（替代全零 five_dimension_masks）
// ----------------------------------------------------------

#[tokio::test]
async fn snapshot_returns_real_masks_after_compute() {
    let bus = EventBus::new();
    let coordinator = OmniSparseCoordinator::new(bus);
    // 未计算前: snapshot 为 None（不虚报全零）
    assert!(coordinator.snapshot().is_none());
    // 动态计算
    let profile = TaskProfile::new("t-1", 0.6, RiskLevel::Medium);
    let masks = coordinator
        .compute_all_masks(&profile)
        .await
        .expect("计算成功");
    // 计算后: snapshot 返回真实结果（与返回值一致）
    let snapshot = coordinator.snapshot().expect("计算后应有快照");
    assert_eq!(
        snapshot.average_sparsity(),
        masks.average_sparsity(),
        "snapshot 应反映最近一次真实计算"
    );
}

#[tokio::test]
async fn snapshot_tracks_latest_computation() {
    let bus = EventBus::new();
    let coordinator = OmniSparseCoordinator::new(bus);
    // 不同复杂度档位先后计算，snapshot 跟踪最新返回值
    let simple = TaskProfile::new("t-1", 0.1, RiskLevel::Low);
    let complex = TaskProfile::new("t-2", 0.9, RiskLevel::High);
    coordinator.compute_all_masks(&simple).await.expect("计算");
    let masks_complex = coordinator.compute_all_masks(&complex).await.expect("计算");
    let snapshot = coordinator.snapshot().expect("快照存在");
    // snapshot 应为最新（complex）计算的掩码
    assert_eq!(
        snapshot.average_sparsity(),
        masks_complex.average_sparsity(),
        "snapshot 应跟踪最新计算"
    );
    assert_eq!(
        snapshot.routing.active_ids.len(),
        masks_complex.routing.active_ids.len(),
        "routing 维度活跃数应与最新计算一致"
    );
}

// ----------------------------------------------------------
// proptest: 裁剪不变量
// ----------------------------------------------------------

proptest! {
    /// 任意工具规模与 keep_count: 保留数 ∈ [min(min_tools, 总数), 总数]
    /// 且 tokens_saved ≤ 总 token 数（不溢出）
    #[test]
    fn prune_invariants(
        n_tools in 0usize..40,
        keep in 0usize..50,
        min_tools in 0usize..10,
        calls in 0usize..10,
    ) {
        let mut pruner = ToolSchemaPruner::new(0.99, min_tools);
        for _ in 0..calls {
            pruner.analyze_trajectories(&[traj("tool-0", true)]);
        }
        let available: Vec<PruneToolSchema> =
            (0..n_tools).map(|i| schema(&format!("tool-{i}"), 100)).collect();
        let total: u32 = available.iter().map(|t| t.schema_tokens).sum();
        let result = pruner.prune_tools(&available, keep);
        // 保留数不超过总数
        prop_assert!(result.kept.len() <= n_tools);
        // min_tools 下限（不超总数）
        prop_assert!(result.kept.len() >= min_tools.min(n_tools));
        // token 收益守恒
        prop_assert!(result.tokens_saved <= total);
        prop_assert_eq!(result.kept.len() + result.pruned_count, n_tools);
    }
}
