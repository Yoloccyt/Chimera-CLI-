//! W1 工具裁剪闭环集成测试（ADR-084，规范 §11.5 + §18.3 熔断）
//!
//! 覆盖三条闭环链路:
//! 1. coordinator 接线: 相关性 Top-K → 使用统计二次裁剪 → masks 终态
//! 2. ledger 适配: TokenLedgerEntry → PruneTrajectory → 裁剪器统计
//! 3. 铁律6: 决策日志 → RLTrajectory 导出

use std::collections::HashMap;

use event_bus::EventBus;
use nexus_contracts::token_evidence::{TokenLedgerEntry, ToolCallRecord};
use osa_coordinator::tool_pruning::{
    prune_trajectories_from_ledger, success_if_result_nonempty, ToolSchemaPruner,
};
use osa_coordinator::{
    AffectedScope, ComplexityBand, OmniSparseCoordinator, PruneToolSchema, RiskLevel, TaskId,
    TaskProfile, TaskType, TimePressure, ToolId,
};

/// 泄漏为 &'static str（测试内一次性工具名,进程生命周期可接受）
fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// 构造带工具集的 TaskProfile（routing 维度输入）
fn profile_with_tools(tools: Vec<&str>) -> TaskProfile {
    TaskProfile {
        task_id: TaskId::new("task-w1"),
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

/// 喂入使用统计: hot 工具高频成功, cold 工具低频失败
fn feed_usage(pruner: &mut ToolSchemaPruner, hot: &str, cold: &str) {
    for _ in 0..10 {
        pruner.record_tool_step(hot, true, 50);
    }
    pruner.record_tool_step(cold, false, 10);
}

#[tokio::test]
async fn coordinator_prunes_routing_mask_by_usage() {
    // 闭环 1: 相关性 Top-K(8 个幸存) → keep_count=3 使用统计裁剪
    let tools: Vec<&str> = (0..8).map(|i| leak(format!("tool-{i}"))).collect();
    let mut pruner = ToolSchemaPruner::new(0.99, 0);
    feed_usage(&mut pruner, "tool-0", "tool-7");

    let coord = OmniSparseCoordinator::new(EventBus::new())
        .with_tool_pruner(pruner)
        .with_tool_keep_count(3);
    let masks = coord
        .compute_all_masks(&profile_with_tools(tools))
        .await
        .expect("掩码计算成功");

    // routing 终态 = 相关性 Top-K ∩ 使用统计保留 3 个
    assert_eq!(masks.routing.active_count(), 3, "keep_count=3 裁剪生效");
    // 可观测性: last_prune_result 反映裁剪
    let prune = coord.last_prune_result().expect("裁剪结果已记录");
    assert_eq!(prune.pruned_count, 5);
}

#[tokio::test]
async fn whitelist_survives_in_coordinator() {
    // §18.3 熔断: 白名单工具在 coordinator 接线路径下仍钉住
    let tools: Vec<&str> = (0..6).map(|i| leak(format!("tool-{i}"))).collect();
    let mut pruner = ToolSchemaPruner::new(0.99, 0).with_whitelist(vec!["tool-5".into()]);
    feed_usage(&mut pruner, "tool-0", "tool-1");

    let coord = OmniSparseCoordinator::new(EventBus::new())
        .with_tool_pruner(pruner)
        .with_tool_keep_count(2);
    let masks = coord
        .compute_all_masks(&profile_with_tools(tools))
        .await
        .expect("掩码计算成功");

    // 裁剪语义: keep=2 为总上限, 钉住 1 → 非白名单配额 1 → 终态 2
    assert_eq!(masks.routing.active_count(), 2);
    assert!(
        masks.routing.is_active(&ToolId::new("tool-5")),
        "白名单工具必须在 routing 终态中幸存"
    );
}

#[tokio::test]
async fn no_pruner_or_keep_count_is_backward_compatible() {
    // 未注入 pruner / 未设 keep_count: routing = 纯相关性 Top-K（旧行为）
    let tools: Vec<&str> = (0..50).map(|i| leak(format!("tool-{i}"))).collect();
    let coord = OmniSparseCoordinator::new(EventBus::new());
    let masks = coord
        .compute_all_masks(&profile_with_tools(tools))
        .await
        .expect("掩码计算成功");
    // complexity=0.6 → Complex 档位 Top-24（config 默认档界）
    let band = ComplexityBand::Complex;
    assert_eq!(masks.routing.active_count(), 24, "默认行为不变（{band:?}）");
    assert!(coord.last_prune_result().is_none(), "无裁剪记录");
}

#[tokio::test]
async fn runtime_keep_count_update_from_control_plane() {
    // W2 前置: set_tool_keep_count 运行时动态下发（六维 D2 控制面入口）
    let tools: Vec<&str> = (0..10).map(|i| leak(format!("tool-{i}"))).collect();
    let mut pruner = ToolSchemaPruner::new(0.99, 0);
    feed_usage(&mut pruner, "tool-0", "tool-9");

    let coord = OmniSparseCoordinator::new(EventBus::new()).with_tool_pruner(pruner);
    // 初始未设 keep_count → 不裁剪
    let masks_before = coord
        .compute_all_masks(&profile_with_tools(tools.clone()))
        .await
        .expect("首次计算");
    assert!(masks_before.routing.active_count() > 4, "未设 keep 不裁剪");

    // 控制面收紧 → 裁剪生效
    coord.set_tool_keep_count(Some(4));
    let masks_after = coord
        .compute_all_masks(&profile_with_tools(tools))
        .await
        .expect("二次计算");
    assert_eq!(masks_after.routing.active_count(), 4, "运行时收紧生效");

    // 控制面关闭 → 回到不裁剪
    coord.set_tool_keep_count(None);
    let masks_restored = coord.compute_all_masks(&profile_with_tools(vec![])).await;
    assert!(masks_restored.is_ok());
}

#[tokio::test]
async fn online_stats_via_handle_affect_next_mask() {
    // 在线喂入闭环: tool_pruner_handle → record_tool_step → 下次掩码反映新统计
    let tools: Vec<&str> = (0..6).map(|i| leak(format!("tool-{i}"))).collect();
    let pruner = ToolSchemaPruner::new(0.99, 0);
    let coord = OmniSparseCoordinator::new(EventBus::new())
        .with_tool_pruner(pruner)
        .with_tool_keep_count(1);
    let masks_cold = coord
        .compute_all_masks(&profile_with_tools(tools.clone()))
        .await
        .expect("冷启动计算");

    // 在线喂入: tool-3 大量成功（其余无统计 → 评分 0）
    let handle = coord.tool_pruner_handle().expect("裁剪器句柄");
    {
        let mut pruner = handle.lock().expect("锁正常");
        for _ in 0..8 {
            pruner.record_tool_step("tool-3", true, 20);
        }
    }
    let masks_warm = coord
        .compute_all_masks(&profile_with_tools(tools))
        .await
        .expect("热更新计算");

    // keep=1: 冷启动全零分并列取首个; 热更新后 tool-3 必然胜出
    assert_eq!(masks_warm.routing.active_count(), 1);
    assert!(
        masks_warm.routing.is_active(&ToolId::new("tool-3")),
        "在线统计必须改变下次裁剪决策（冷启动 {}/{}）",
        masks_cold.routing.active_count(),
        masks_warm.routing.active_count()
    );
}

#[tokio::test]
async fn ledger_to_mask_closed_loop() {
    // 闭环 2: TokenLedger → PruneTrajectory → analyze → coordinator 裁剪
    let make_entry = |id: &str, tool: &str, result: &str| {
        let calls = vec![ToolCallRecord::new(tool, "{}", result, 10)];
        TokenLedgerEntry::new(
            id,
            1,
            "s-1",
            "i-1",
            vec![],
            vec![],
            vec![],
            vec![],
            "v1",
            calls,
            None,
            0,
        )
    };
    let entries = vec![
        make_entry("e-1", "tool-0", "ok"),
        make_entry("e-2", "tool-0", "ok"),
        make_entry("e-3", "tool-1", ""),
        make_entry("e-4", "tool-0", "ok"),
    ];
    let trajectories = prune_trajectories_from_ledger(&entries, success_if_result_nonempty);
    let mut pruner = ToolSchemaPruner::new(0.99, 0);
    pruner.analyze_trajectories(&trajectories);
    // tool-0: 3 成功; tool-1: 1 失败
    assert_eq!(pruner.usage_stats().get("tool-0").unwrap().call_count, 3);
    assert_eq!(pruner.usage_stats().get("tool-1").unwrap().success_count, 0);

    let tools: Vec<&str> = vec!["tool-0", "tool-1", "tool-2"];
    let coord = OmniSparseCoordinator::new(EventBus::new())
        .with_tool_pruner(pruner)
        .with_tool_keep_count(1)
        .with_tool_schema_tokens(HashMap::from([
            ("tool-0".to_string(), 425),
            ("tool-1".to_string(), 400),
            ("tool-2".to_string(), 400),
        ]));
    let masks = coord
        .compute_all_masks(&profile_with_tools(tools))
        .await
        .expect("掩码计算");
    assert!(
        masks.routing.is_active(&ToolId::new("tool-0")),
        "高频工具幸存"
    );
    // tokens_saved 观测: 1225 总 - 425 保留 = 800（schema_tokens 注入后真实计量）
    let prune = coord.last_prune_result().expect("裁剪记录");
    assert_eq!(prune.tokens_saved, 800);
}

#[tokio::test]
async fn pruner_trajectory_export_from_coordinator_handle() {
    // 闭环 3(铁律6): coordinator 句柄 → export_trajectory
    let tools: Vec<&str> = (0..5).map(|i| leak(format!("tool-{i}"))).collect();
    let mut pruner = ToolSchemaPruner::new(0.99, 0);
    feed_usage(&mut pruner, "tool-0", "tool-4");
    // 预埋两条决策历史
    let available: Vec<PruneToolSchema> = (0..5)
        .map(|i| PruneToolSchema {
            name: format!("tool-{i}"),
            schema_tokens: 100,
        })
        .collect();
    pruner.prune_tools(&available, 2);
    pruner.prune_tools(&available, 4);

    let coord = OmniSparseCoordinator::new(EventBus::new())
        .with_tool_pruner(pruner)
        .with_tool_keep_count(2);
    let _ = coord
        .compute_all_masks(&profile_with_tools(tools))
        .await
        .expect("掩码计算");

    let handle = coord.tool_pruner_handle().expect("句柄");
    let pruner_ref = handle.lock().expect("锁正常");
    let trajectory = pruner_ref.export_trajectory("episode-w1-closed-loop");
    // 2 条预埋 + 1 条 coordinator 接线决策
    assert_eq!(trajectory.len(), 3);
    assert!(!trajectory.is_empty());
    assert!(trajectory.timestamps.iter().all(|t| *t > 0));
}
