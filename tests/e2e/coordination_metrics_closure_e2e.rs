//! 协调度量接线闭环 E2E — L8 审议延迟/委托开销/共识质量真实流入告警链路
//!
//! # 验证链路(M2-T2.3)
//! Parliament.deliberate → DebateCompleted ─┐
//! DelegationExecutor.execute → DelegationCompleted ─┤→ quest-engine 订阅合并
//! → complete_quest 填充 CoordinationCostSample/InferenceGainSample
//! → CoordinationRatioReported 携带真实审议/委托成本
//! → efficiency-monitor 真实触发 paradox-risk-coordination-ratio 告警
//! (不再依赖手动注入样本)
//!
//! # 覆盖分支
//! 1. 正常路径:审议 + 委托 → ratio 事件含真实成本 → 低基准下触发告警
//! 2. 无审议 Quest:缓存为空,字段保持 None,度量正常记录不阻塞
//! 3. Vetoed 路径:审议延迟有值、共识质量 None(无投票)

use std::sync::Arc;
use std::time::Duration;

use chimera_mas::delegation::{DelegationExecutor, TaskRunner};
use chimera_mas::prelude::*;
use efficiency_monitor::{EfficiencyMonitor, MonitorConfig};
use event_bus::{EventBus, NexusEvent};
use nexus_core::{MultimodalInput, TaskStatus, UserIntent};
use parliament::{Parliament, ParliamentConfig, Proposal};
use quest_engine::{spawn_metrics_subscriber, CoordinationMetricsConfig, QuestEngine};

// ============================================================
// 辅助构造
// ============================================================

fn make_intent(text: &str) -> UserIntent {
    UserIntent {
        intent_id: "e2e-intent".into(),
        raw_text: text.into(),
        multimodal_inputs: vec![MultimodalInput::Text(text.into())],
        risk_level: 10,
    }
}

/// 注入固定延迟的委托 runner(模拟真实子 Agent 耗时,使开销可观测)
fn delay_runner(delay_ms: u64) -> TaskRunner {
    Arc::new(move |task: AgentTask| {
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            Ok(format!("done-{}", task.inner.task_id))
        })
    })
}

fn make_agent_task(task_id: &str, quest_id: &str) -> AgentTask {
    AgentTask::new(
        nexus_core::Task {
            task_id: task_id.into(),
            description: format!("e2e task {task_id}"),
            status: TaskStatus::Pending,
            dependencies: vec![],
        },
        TaskComplexity::Simple,
        100,
        Duration::from_secs(10),
        QualityLevel::Standard,
    )
    .with_quest(quest_id)
}

/// 轮询等待条件成立(异步事件投递的确定性等待,最多 2s)
async fn wait_until<F: Fn() -> bool>(cond: F, what: &str) {
    for _ in 0..40 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("等待超时: {what}");
}

/// 驱动 Quest 全部任务到 Completed(触发 complete_quest 与度量合并)
async fn complete_all_tasks(engine: &QuestEngine, quest: &nexus_core::Quest) {
    for task in &quest.tasks {
        engine
            .update_task_status(&quest.quest_id, &task.task_id, TaskStatus::Running)
            .await
            .unwrap();
        engine
            .update_task_status(&quest.quest_id, &task.task_id, TaskStatus::Completed)
            .await
            .unwrap();
    }
}

// ============================================================
// 分支 1:正常路径 — 审议 + 委托 → 真实成本 → 告警闭环
// ============================================================

#[tokio::test]
async fn test_e2e_full_closure_debate_delegation_to_paradox_alert() {
    let bus = EventBus::new();

    // 低成本基准(10ms):真实审议/委托开销(几十 ms 级)足以占满成本指数。
    // WHY 阈值 0.9:占位 Opinion 实现下低风险提案全票赞成(quality=1.0),
    // gain_index=1.0 使 ratio 恰为 1.0;阈值 0.9 保证真实接线数据可越阈
    // 触发告警(推理悖论告警由接线数据驱动,不再手动注入)。
    let mut engine = QuestEngine::new(bus.clone());
    engine.with_metrics_config(CoordinationMetricsConfig::new(0.9, 10.0, 1.0));
    let engine = Arc::new(engine);

    // 订阅器:subscribe-before-spawn(§4.4 反模式 #3)
    let sub_handle = spawn_metrics_subscriber(Arc::clone(&engine), &bus);

    // 1. 创建 Quest
    let quest = engine
        .create_quest(make_intent("分析需求。设计方案。"))
        .await
        .unwrap();

    // 2. 真实 L8 审议(Full 5 角色)→ Parliament 发布 DebateCompleted
    let parliament = Parliament::new(ParliamentConfig::default(), bus.clone());
    let proposal = Proposal::new("p-e2e", &quest.quest_id, "执行方案提案", 0.2);
    let consensus = parliament.deliberate(&quest, &proposal).await.unwrap();
    assert!(consensus.is_reached(), "低风险提案应达成共识");

    // 3. 真实委托执行(2 子任务 × 20ms)→ DelegationCompleted
    let executor =
        DelegationExecutor::with_runner(bus.clone(), Duration::from_secs(10), delay_runner(20));
    let tasks = vec![
        make_agent_task("t-a", &quest.quest_id),
        make_agent_task("t-b", &quest.quest_id),
    ];
    executor
        .execute_delegation("root-orchestrator", tasks)
        .await
        .unwrap();

    // 4. 等待订阅器消费两类观测事件(异步投递)
    {
        let engine = Arc::clone(&engine);
        let quest_id = quest.quest_id.clone();
        wait_until(
            move || {
                engine
                    .pending_coordination_sample(&quest_id)
                    .map(|s| {
                        s.parliament_debate_latency_ms.is_some()
                            && s.delegation_overhead_ms.is_some()
                    })
                    .unwrap_or(false)
            },
            "订阅器应缓存审议延迟与委托开销",
        )
        .await;
    }
    let pending = engine
        .pending_coordination_sample(&quest.quest_id)
        .expect("应有待合并样本");
    assert!(
        pending.consensus_quality.is_some(),
        "Full 审议应携带共识质量 proxy"
    );
    let delegation_overhead = pending.delegation_overhead_ms.expect("应有委托开销");
    assert!(
        delegation_overhead >= 20.0,
        "委托批次 wall-clock 应 ≥ 单任务延迟 20ms,实际 {delegation_overhead}ms"
    );

    // 5. 完成 Quest → 度量合并 → CoordinationRatioReported(订阅在完成前建立)
    let mut rx = bus.subscribe();
    complete_all_tasks(&engine, &quest).await;

    let mut ratio_event = None;
    while let Ok(Some(event)) = rx.try_recv() {
        if matches!(event, NexusEvent::CoordinationRatioReported { .. }) {
            ratio_event = Some(event);
        }
    }
    let ratio_event = ratio_event.expect("应发布 CoordinationRatioReported");
    let NexusEvent::CoordinationRatioReported {
        coordination_cost_ms,
        is_paradox_risk,
        ratio,
        threshold,
        ..
    } = ratio_event.clone()
    else {
        unreachable!()
    };
    assert!(
        coordination_cost_ms >= 20.0,
        "协调成本应包含真实审议+委托开销,实际 {coordination_cost_ms}ms"
    );
    assert!(
        is_paradox_risk,
        "低基准(10ms)+ 阈值 0.9 下真实成本应触发推理悖论风险,ratio={ratio}"
    );

    // 6. efficiency-monitor 消费 → 真实触发 paradox-risk 告警
    let mut alert_rx = bus.subscribe();
    let monitor = EfficiencyMonitor::with_event_bus(MonitorConfig::default(), bus.clone());
    monitor.record_event(&ratio_event);

    let mut saw_alert = false;
    while let Ok(Some(event)) = alert_rx.try_recv() {
        if let NexusEvent::EfficiencyAlertTriggered {
            rule_id,
            triggered_value,
            threshold: alert_threshold,
            ..
        } = event
        {
            if rule_id == "paradox-risk-coordination-ratio" {
                assert!((triggered_value - ratio).abs() < 1e-9);
                assert!((alert_threshold - threshold).abs() < 1e-9);
                saw_alert = true;
            }
        }
    }
    assert!(
        saw_alert,
        "efficiency-monitor 应由真实接线数据触发 paradox-risk-coordination-ratio 告警"
    );

    // 合并后缓存应被清理(take 语义,防泄漏)
    assert!(engine
        .pending_coordination_sample(&quest.quest_id)
        .is_none());
    sub_handle.abort();
}

// ============================================================
// 分支 2:无审议 Quest — 字段保持 None,度量不阻塞
// ============================================================

#[tokio::test]
async fn test_e2e_quest_without_debate_keeps_optional_fields_none() {
    let bus = EventBus::new();
    let engine = Arc::new(QuestEngine::new(bus.clone()));
    let sub_handle = spawn_metrics_subscriber(Arc::clone(&engine), &bus);

    let quest = engine
        .create_quest(make_intent("分析需求。"))
        .await
        .unwrap();

    // 无任何审议/委托,直接完成
    assert!(engine
        .pending_coordination_sample(&quest.quest_id)
        .is_none());
    complete_all_tasks(&engine, &quest).await;

    // 度量正常记录(尽力合并语义:缺失字段不阻塞)
    let ratio = engine
        .last_coordination_ratio()
        .expect("无审议 Quest 也应正常记录度量");
    assert!(
        !ratio.is_paradox_risk,
        "仅微秒级 EventBus 延迟不应触发悖论风险"
    );
    sub_handle.abort();
}

// ============================================================
// 分支 3:Vetoed 路径 — 审议延迟有值、共识质量 None
// ============================================================

#[tokio::test]
async fn test_e2e_vetoed_debate_reports_latency_without_quality() {
    let bus = EventBus::new();
    let engine = Arc::new(QuestEngine::new(bus.clone()));
    let sub_handle = spawn_metrics_subscriber(Arc::clone(&engine), &bus);

    let quest = engine
        .create_quest(make_intent("分析需求。"))
        .await
        .unwrap();

    // 恶意提案触发 Skeptic 前置否决(短路路径也发布 DebateCompleted)
    let parliament = Parliament::new(ParliamentConfig::default(), bus.clone());
    let proposal = Proposal::new("p-veto", &quest.quest_id, "sudo rm -rf /", 0.9);
    let consensus = parliament.deliberate(&quest, &proposal).await.unwrap();
    assert!(consensus.is_vetoed());

    {
        let engine = Arc::clone(&engine);
        let quest_id = quest.quest_id.clone();
        wait_until(
            move || engine.pending_coordination_sample(&quest_id).is_some(),
            "订阅器应缓存否决路径的审议延迟",
        )
        .await;
    }
    let pending = engine
        .pending_coordination_sample(&quest.quest_id)
        .expect("应有待合并样本");
    assert!(
        pending.parliament_debate_latency_ms.is_some(),
        "否决短路路径也应上报审议延迟"
    );
    assert!(
        pending.consensus_quality.is_none(),
        "否决路径无投票,共识质量应为 None"
    );
    sub_handle.abort();
}

// ============================================================
// 分支 4:M2 多维共识质量端到端(高分歧 / 全一致 / 高弃权三场景)
//
// 验证真实 Parliament.deliberate 产出的多维质量(divergence/abstention_rate/
// consensus_margin)随 DebateCompleted 流入 quest-engine 待合并缓存。
// 场景由 generate_opinion 规则 stub 驱动(见 debate.rs):
// - Architect:task_count≤3 赞成,>3 反对
// - Skeptic:risk<0.3 赞成,0.3-0.5 弃权,>0.5 反对
// - Optimizer:Fast 赞成,Standard 弃权,Deep 反对
// - Librarian:task_count≤5 赞成,>5 弃权
// - Bard:总是赞成
// ============================================================

/// 构造指定任务数与思考模式的 Quest(直接用于 Parliament.deliberate)
fn make_quest(
    quest_id: &str,
    task_count: usize,
    mode: nexus_core::ThinkingMode,
) -> nexus_core::Quest {
    let tasks: Vec<nexus_core::Task> = (0..task_count)
        .map(|i| nexus_core::Task {
            task_id: format!("t-{i}"),
            description: format!("任务 {i}"),
            status: TaskStatus::Pending,
            dependencies: vec![],
        })
        .collect();
    nexus_core::Quest {
        quest_id: quest_id.into(),
        title: "多维质量场景".into(),
        tasks,
        thinking_mode: mode,
        checkpoint_id: None,
        priority: 128,
    }
}

/// 在共享 bus 上真实审议并等待 quest-engine 缓存多维质量样本
async fn deliberate_and_fetch_quality(
    engine: &Arc<QuestEngine>,
    parliament: &Parliament,
    quest: &nexus_core::Quest,
    risk: f32,
) -> quest_engine::PendingCoordSample {
    let proposal = Proposal::new(
        format!("p-{}", quest.quest_id),
        &quest.quest_id,
        "多维质量提案",
        risk,
    );
    parliament.deliberate(quest, &proposal).await.unwrap();
    {
        let engine = Arc::clone(engine);
        let quest_id = quest.quest_id.clone();
        wait_until(
            move || {
                engine
                    .pending_coordination_sample(&quest_id)
                    .map(|s| s.divergence.is_some())
                    .unwrap_or(false)
            },
            "订阅器应缓存多维质量",
        )
        .await;
    }
    engine
        .pending_coordination_sample(&quest.quest_id)
        .expect("应有待合并样本")
}

#[tokio::test]
async fn test_e2e_multidim_quality_flows_through_pipeline() {
    let bus = EventBus::new();
    let engine = Arc::new(QuestEngine::new(bus.clone()));
    let sub_handle = spawn_metrics_subscriber(Arc::clone(&engine), &bus);
    let parliament = Parliament::new(ParliamentConfig::default(), bus.clone());

    // 场景 A:全一致(2 任务 + Fast + 低风险)→ 全票赞成 → divergence ≈ 0
    let agree = deliberate_and_fetch_quality(
        &engine,
        &parliament,
        &make_quest("q-agree", 2, nexus_core::ThinkingMode::Fast),
        0.2,
    )
    .await;
    let agree_div = agree.divergence.expect("应有分歧度");
    assert!(agree_div < 0.05, "全一致分歧度应近 0,实际 {agree_div}");
    assert!(agree.consensus_margin.is_some(), "应携带共识裕度");

    // 场景 B:高分歧(7 任务 + Deep + 低风险)→ 立场混杂 → divergence 显著 > A
    // Architect 反对(>3)、Skeptic 赞成(risk<0.3)、Optimizer 反对(Deep)、
    // Librarian 弃权(>5)、Bard 赞成 → position {0,1,0,0.5,1}
    let diverge = deliberate_and_fetch_quality(
        &engine,
        &parliament,
        &make_quest("q-diverge", 7, nexus_core::ThinkingMode::Deep),
        0.2,
    )
    .await;
    let diverge_div = diverge.divergence.expect("应有分歧度");
    assert!(
        diverge_div > agree_div,
        "高分歧场景分歧度({diverge_div})应显著大于全一致({agree_div})"
    );

    // 场景 C:高弃权(7 任务 + Standard + 中风险 0.4)→ 多角色弃权
    // Skeptic 弃权(0.3-0.5)、Optimizer 弃权(Standard)、Librarian 弃权(>5)
    let abstain = deliberate_and_fetch_quality(
        &engine,
        &parliament,
        &make_quest("q-abstain", 7, nexus_core::ThinkingMode::Standard),
        0.4,
    )
    .await;
    let abstain_rate = abstain.abstention_rate.expect("应有弃权率");
    assert!(
        abstain_rate > 0.0,
        "高弃权场景弃权率应 > 0,实际 {abstain_rate}"
    );

    sub_handle.abort();
}
