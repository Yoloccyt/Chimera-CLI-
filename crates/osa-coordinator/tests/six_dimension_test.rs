//! W2 六维调整器集成测试（ADR-084 决策 1，规范 §11.3）
//!
//! 覆盖三条链路:
//! 1. 事件驱动闭环: EventBus 发布反馈事件 → 调整循环 → 契约版本演进
//! 2. osa 消费接线: 调整器 D2.max_tools_per_step → compute_all_masks 裁剪 keep
//! 3. 铁律6: journal → RLTrajectory 导出

use event_bus::{EventBus, EventMetadata, NexusEvent, RouterStatsPayload};
use osa_coordinator::{
    AdjustmentLimits, OmniSparseCoordinator, SixDimensionAdjuster, TaskProfile, ToolId,
    ToolSchemaPruner,
};

// ---- 测试脚手架(与 tool_pruning_closed_loop_test 同构) ----

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

fn profile_with_tools(tools: Vec<&str>) -> TaskProfile {
    use osa_coordinator::{AffectedScope, RiskLevel, TaskId, TaskType, TimePressure};
    TaskProfile {
        task_id: TaskId::new("task-w2"),
        task_type: TaskType::Read,
        complexity_score: 0.6,
        risk_level: RiskLevel::Medium,
        time_pressure: TimePressure::Low,
        affected_scope: AffectedScope::Local,
        available_tools: tools.into_iter().map(ToolId::new).collect(),
        available_files: Vec::new(),
        available_memories: Vec::new(),
        recent_operations: Vec::new(),
        active_tasks: Vec::new(),
        routing_scores: None,
        context_scores: None,
        memory_scores: None,
        task_phase: None,
    }
}

fn stats(hit_rate: f32) -> RouterStatsPayload {
    RouterStatsPayload {
        hit_rate,
        p50_latency_us: 10,
        p95_latency_us: 100,
        p99_latency_us: 500,
        hot_capabilities: Vec::new(),
    }
}

fn budget_exceeded() -> NexusEvent {
    NexusEvent::BudgetExceeded {
        metadata: EventMetadata::new("test-w2"),
        budget_type: "token".into(),
        current: 1200,
        limit: 1000,
    }
}

fn recall_degraded() -> NexusEvent {
    NexusEvent::HcwRecallDegraded {
        metadata: EventMetadata::new("test-w2"),
        tier: "L2".into(),
        recall_rate: 0.3,
        baseline_recall: 0.8,
        reason: "test".into(),
    }
}

// ---- 链路 1: 事件驱动闭环 ----

#[tokio::test]
async fn adjustment_loop_consumes_published_events() {
    let bus = EventBus::new();
    let coord = OmniSparseCoordinator::new(bus.clone())
        .with_dimension_adjuster(SixDimensionAdjuster::new());
    let handle = coord
        .start_dimension_adjustment_loop()
        .expect("调整循环启动");
    // 等待订阅就绪(subscribe 先于 spawn,但调度需一拍)
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    bus.publish(budget_exceeded()).await.expect("发布成功");
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let adjuster = coord.dimension_adjuster_handle().expect("调整器句柄");
    {
        let adj = adjuster.lock().expect("锁正常");
        assert_eq!(adj.journal().len(), 1, "事件已被消费并调整");
        assert_eq!(adj.current_contract().version, "0.1.1");
    }
    handle.abort();
}

// ---- 链路 2: osa 消费接线(D2 → 裁剪 keep) ----

#[tokio::test]
async fn adjuster_d2_drives_pruning_keep() {
    let tools: Vec<&str> = (0..10).map(|i| leak(format!("tool-{i}"))).collect();
    let mut pruner = ToolSchemaPruner::new(0.99, 0);
    for _ in 0..5 {
        pruner.record_tool_step("tool-0", true, 10);
    }

    let coord = OmniSparseCoordinator::new(EventBus::new())
        .with_tool_pruner(pruner)
        .with_dimension_adjuster(SixDimensionAdjuster::new());
    // 默认 D2.max_tools_per_step = 5 → 首次裁剪保留 5
    let masks_v0 = coord
        .compute_all_masks(&profile_with_tools(tools.clone()))
        .await
        .expect("首次计算");
    assert_eq!(masks_v0.routing.active_count(), 5, "D2 默认值驱动 keep");

    // 预算超限 ×2 → D2 收紧 5 → 3 → 下一次掩码反映收紧
    let adjuster = coord.dimension_adjuster_handle().expect("句柄");
    {
        let mut adj = adjuster.lock().expect("锁正常");
        adj.apply_feedback(&budget_exceeded());
        adj.apply_feedback(&budget_exceeded());
    }
    let masks_v2 = coord
        .compute_all_masks(&profile_with_tools(tools))
        .await
        .expect("二次计算");
    assert_eq!(
        masks_v2.routing.active_count(),
        3,
        "调整器 D2 收紧后 keep=3 生效"
    );
}

#[tokio::test]
async fn explicit_keep_count_overrides_adjuster() {
    // 优先级: 显式 tool_keep_count > 调整器 D2
    let tools: Vec<&str> = (0..10).map(|i| leak(format!("tool-{i}"))).collect();
    let pruner = ToolSchemaPruner::new(0.99, 0);
    let coord = OmniSparseCoordinator::new(EventBus::new())
        .with_tool_pruner(pruner)
        .with_dimension_adjuster(SixDimensionAdjuster::new())
        .with_tool_keep_count(2);
    let masks = coord
        .compute_all_masks(&profile_with_tools(tools))
        .await
        .expect("计算成功");
    assert_eq!(masks.routing.active_count(), 2, "显式 keep=2 优先于 D2=5");
}

// ---- 链路 3: 铁律6 + 边界治理 ----

#[tokio::test]
async fn custom_limits_constrain_adjustment() {
    // 自定义边界: max_tools_per_step ∈ [3, 16] → 收紧到 3 即封底
    let mut adjuster = SixDimensionAdjuster::new().with_limits(AdjustmentLimits {
        min_max_tools_per_step: 3,
        ..AdjustmentLimits::default()
    });
    for _ in 0..10 {
        adjuster.apply_feedback(&budget_exceeded());
    }
    assert_eq!(adjuster.current_contract().d2_tool.max_tools_per_step, 3);
    // 5→3 共 2 次生效
    assert_eq!(adjuster.journal().len(), 2);
    let trajectory = adjuster.export_trajectory("episode-limits");
    assert_eq!(trajectory.len(), 2);
}

#[tokio::test]
async fn mixed_feedback_rules_coexist() {
    // 多规则共存: 召回退化(D1) + 预算超限(D2) + 熵均衡(D3) 依次应用
    //（熵加权预置 false 以验证 D3 规则的可观测翻转）
    let mut contract = nexus_contracts::HarnessConfigContract::default_contract();
    contract.d3_generation.entropy_weighting = false;
    let mut adjuster = SixDimensionAdjuster::with_contract(contract);
    adjuster.apply_feedback(&recall_degraded());
    adjuster.apply_feedback(&budget_exceeded());
    adjuster.apply_feedback(&NexusEvent::EntropyBalanced {
        metadata: EventMetadata::new("test-w2"),
        old_entropy: 2.0,
        new_entropy: 1.2,
        redistributed_count: 4,
    });
    let contract = adjuster.current_contract();
    assert_eq!(contract.d1_context.ancestor_retrieval_depth, 3);
    assert_eq!(contract.d2_tool.max_tools_per_step, 4);
    assert!(contract.d3_generation.entropy_weighting);
    // journal 顺序 = D1×2 → D2×1 → D3×1
    let dims: Vec<u8> = adjuster.journal().iter().map(|r| r.dimension).collect();
    assert_eq!(dims, vec![1, 1, 2, 3]);
}

#[tokio::test]
async fn router_stats_average_dead_zone() {
    // 三路由器平均命中率决定方向: (0.2 + 0.5 + 0.9)/3 = 0.533 → 死区 no-op
    let mut adjuster = SixDimensionAdjuster::new();
    adjuster.apply_feedback(&NexusEvent::RouterStatsReported {
        metadata: EventMetadata::new("test-w2"),
        kvbsr_stats: stats(0.2),
        sesa_stats: stats(0.5),
        faae_stats: stats(0.9),
    });
    assert_eq!(
        adjuster.current_contract().d2_tool.retrieval_top_k,
        10,
        "死区"
    );
    // (0.1 + 0.1 + 0.2)/3 < 0.5 → 加宽
    adjuster.apply_feedback(&NexusEvent::RouterStatsReported {
        metadata: EventMetadata::new("test-w2"),
        kvbsr_stats: stats(0.1),
        sesa_stats: stats(0.1),
        faae_stats: stats(0.2),
    });
    assert_eq!(adjuster.current_contract().d2_tool.retrieval_top_k, 11);
}
