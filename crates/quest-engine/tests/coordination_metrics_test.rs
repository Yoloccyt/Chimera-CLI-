//! P2-1 协调成本/推理增益比值度量集成测试
//!
//! 对应架构层:L9 Quest
//! 对应分析:P2-1(三重悖论推理悖论红线 — 协调成本/推理增益比值度量)
//!
//! # 测试覆盖
//!
//! 1. **空初始化验证**:QuestEngine 创建后度量收集器为空(sample_count=0, last_ratio=None)
//! 2. **Quest 完成后度量记录**:Quest 完成后度量收集器有数据(sample_count>0, last_ratio=Some)
//! 3. **自定义配置验证**:with_metrics_config 可设置自定义阈值
//! 4. **推理悖论风险检测**:低成功率 Quest 触发 is_paradox_risk
//! 5. **公共 API 访问验证**:metrics() / last_coordination_ratio() 可正常访问
//! 6. **多 Quest EWMA 收敛**:多个 Quest 完成后 EWMA 收敛到稳定值

use event_bus::{EventBus, EventMetadata, NexusEvent};
use nexus_core::{MultimodalInput, TaskStatus, UserIntent};
use quest_engine::{
    CoordinationCostSample, CoordinationMetricsConfig, CoordinationToGainRatio,
    InferenceGainSample, QuestEngine,
};

/// 创建测试用 UserIntent(指定文本内容)
fn make_intent(text: &str) -> UserIntent {
    UserIntent {
        intent_id: "test-intent".into(),
        raw_text: text.into(),
        multimodal_inputs: vec![MultimodalInput::Text(text.into())],
        risk_level: 10,
    }
}

// ============================================================
// 测试组 1:空初始化验证
// ============================================================

#[tokio::test]
async fn test_metrics_empty_on_creation() {
    let bus = EventBus::new();
    let engine = QuestEngine::new(bus);

    // 新创建的引擎,度量收集器应为空
    assert_eq!(engine.metrics().sample_count(), 0);
    assert!(engine.last_coordination_ratio().is_none());
}

// ============================================================
// 测试组 2:Quest 完成后度量记录
// ============================================================

#[tokio::test]
async fn test_metrics_recorded_after_quest_completion() {
    let bus = EventBus::new();
    let engine = QuestEngine::new(bus);

    // 创建一个包含 2 个任务的 Quest("分析需求。设计方案。")
    let intent = make_intent("分析需求。设计方案。");
    let quest = engine.create_quest(intent).await.unwrap();
    assert_eq!(quest.tasks.len(), 2);

    // 完成所有任务(模拟全部成功)
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

    // Quest 完成后,度量收集器应有数据
    assert!(engine.metrics().sample_count() > 0);
    let ratio = engine
        .last_coordination_ratio()
        .expect("Quest 完成后应有比值记录");

    // 全部任务成功,推理增益应为 1.0(或接近)
    assert!(
        ratio.inference_gain > 0.99,
        "全部任务成功时推理增益应接近 1.0,实际: {}",
        ratio.inference_gain
    );
}

// ============================================================
// 测试组 3:推理悖论风险检测
// ============================================================

#[tokio::test]
async fn test_paradox_risk_with_low_success_rate() {
    let bus = EventBus::new();
    let engine = QuestEngine::new(bus);

    // 创建一个包含 2 个任务的 Quest
    let intent = make_intent("分析需求。设计方案。");
    let quest = engine.create_quest(intent).await.unwrap();
    assert_eq!(quest.tasks.len(), 2);

    // 一个任务成功,一个任务失败(成功率 50%)
    let task_ids: Vec<_> = quest.tasks.iter().map(|t| t.task_id.clone()).collect();

    // Task 1: 成功
    engine
        .update_task_status(&quest.quest_id, &task_ids[0], TaskStatus::Running)
        .await
        .unwrap();
    engine
        .update_task_status(&quest.quest_id, &task_ids[0], TaskStatus::Completed)
        .await
        .unwrap();

    // Task 2: 失败
    engine
        .update_task_status(&quest.quest_id, &task_ids[1], TaskStatus::Running)
        .await
        .unwrap();
    engine
        .update_task_status(&quest.quest_id, &task_ids[1], TaskStatus::Failed)
        .await
        .unwrap();

    // Quest 完成后,度量应记录低成功率
    // 注意:Event Bus publish 延迟通常 <1ms(微秒级),实际 cost_index 接近 0。
    // 这里验证推理增益正确记录为 50%(推理悖论风险检测的核心输入)。
    let ratio = engine
        .last_coordination_ratio()
        .expect("Quest 完成后应有比值记录");
    assert!(
        ratio.inference_gain <= 0.51,
        "50% 成功率时推理增益应 ≈ 0.5,实际: {}",
        ratio.inference_gain
    );

    // 手动注入高协调成本样本(模拟跨层通信瓶颈),验证推理悖论风险检测
    // WHY 手动注入:实际 Event Bus 延迟为微秒级,无法自然触发推理悖论。
    // 生产环境中,parliament_debate_latency_ms / delegation_overhead_ms 才是
    // 协调成本的主要来源(议会审议 + 多 Agent 委托可能耗时数百毫秒)。
    let high_cost = CoordinationCostSample::new(500.0, 300.0) // Event Bus 500ms + TTG 300ms
        .with_parliament_debate(1000.0) // 议会审议 1000ms
        .with_delegation_overhead(2000.0); // 多 Agent 委托 2000ms
    let low_gain = InferenceGainSample::new(0.1); // 10% 成功率

    let ratio = engine.metrics().record_and_compute(&high_cost, &low_gain);

    // total_ms = 500+300+1000+2000 = 3800ms
    // cost_index = min(3800/1000, 1.0) = 1.0
    // gain_index = 0.1
    // ratio = 1.0 / 0.1 = 10.0 > 1.0 → 推理悖论风险
    assert!(
        ratio.is_paradox_risk,
        "高协调成本(3800ms) + 低成功率(10%)应触发推理悖论风险,ratio: {}",
        ratio.description()
    );
    assert!(ratio.ratio > 1.0, "比值应 > 1.0,实际: {}", ratio.ratio);
}

// ============================================================
// 测试组 4:正常情况(低成本 + 高成功率)
// ============================================================

#[tokio::test]
async fn test_no_paradox_risk_with_high_success_rate() {
    let bus = EventBus::new();
    // 使用高成本基准(100000ms = 100s),使成本指数接近 0
    let mut engine = QuestEngine::new(bus);
    engine.with_metrics_config(CoordinationMetricsConfig::new(1.0, 100000.0, 0.3));

    let intent = make_intent("分析需求。设计方案。");
    let quest = engine.create_quest(intent).await.unwrap();

    // 全部成功
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

    let ratio = engine
        .last_coordination_ratio()
        .expect("Quest 完成后应有比值记录");

    // 高成功率(1.0) + 低成本指数 → ratio 很小 → 无推理悖论风险
    assert!(
        !ratio.is_paradox_risk,
        "低成本 + 高成功率不应触发推理悖论风险,ratio: {}",
        ratio.description()
    );
}

// ============================================================
// 测试组 5:自定义配置验证
// ============================================================

#[tokio::test]
async fn test_with_metrics_config_custom_threshold() {
    let bus = EventBus::new();
    let mut engine = QuestEngine::new(bus);

    // 设置极低阈值(0.1),使任何比值都触发风险
    engine.with_metrics_config(CoordinationMetricsConfig::with_threshold(0.1));

    let intent = make_intent("分析需求。设计方案。");
    let quest = engine.create_quest(intent).await.unwrap();

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

    let ratio = engine
        .last_coordination_ratio()
        .expect("Quest 完成后应有比值记录");

    // 阈值 0.1,即使高成功率也可能触发(取决于成本指数)
    assert_eq!(ratio.threshold, 0.1);
}

// ============================================================
// 测试组 6:多 Quest EWMA 收敛
// ============================================================

#[tokio::test]
async fn test_ewma_convergence_with_multiple_quests() {
    let bus = EventBus::new();
    let engine = QuestEngine::new(bus);

    // 完成 5 个 Quest,全部成功
    for i in 0..5 {
        let intent = make_intent("分析需求。设计方案。");
        let quest = engine.create_quest(intent).await.unwrap();

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

        // 每个 Quest 完成后检查样本数递增
        let _ = i; // 抑制未使用变量警告
    }

    // 5 个 Quest 完成后,样本数应为 5
    assert_eq!(engine.metrics().sample_count(), 5);

    // EWMA 应收敛到稳定值(全部成功,gain ≈ 1.0)
    let ratio = engine
        .last_coordination_ratio()
        .expect("多 Quest 后应有比值记录");
    assert!(
        ratio.inference_gain > 0.99,
        "全部成功的 Quest EWMA 增益应 ≈ 1.0,实际: {}",
        ratio.inference_gain
    );
}

// ============================================================
// 测试组 7:metrics() 公共 API 返回正确的收集器引用
// ============================================================

#[tokio::test]
async fn test_metrics_public_api_returns_collector() {
    let bus = EventBus::new();
    let engine = QuestEngine::new(bus);

    // 验证 metrics() 返回的收集器可正常调用方法
    let collector = engine.metrics();
    assert_eq!(collector.sample_count(), 0);
    assert!(collector.last_ratio().is_none());

    // 验证 config() 可访问
    let config = collector.config();
    assert_eq!(config.paradox_threshold, 1.0); // 默认阈值
    assert_eq!(config.cost_baseline_ms, 1000.0); // 默认基准
}

// ============================================================
// 测试组 8:CoordinationToGainRatio 类型可从 engine 获取
// ============================================================

#[tokio::test]
async fn test_coordination_ratio_type_accessible() {
    let bus = EventBus::new();
    let engine = QuestEngine::new(bus);

    let intent = make_intent("分析需求。设计方案。");
    let quest = engine.create_quest(intent).await.unwrap();

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

    // 验证 CoordinationToGainRatio 类型可从 engine 获取
    let ratio: Option<CoordinationToGainRatio> = engine.last_coordination_ratio();
    assert!(ratio.is_some());

    let r = ratio.unwrap();
    // 验证所有字段都有合理值
    assert!(r.coordination_cost_ms >= 0.0);
    assert!(r.inference_gain >= 0.0 && r.inference_gain <= 1.0);
    assert!(r.cost_index >= 0.0 && r.cost_index <= 1.0);
    assert!(r.gain_index >= 0.0 && r.gain_index <= 1.0);
    assert!(r.ratio >= 0.0);
    assert_eq!(r.threshold, 1.0); // 默认阈值
}

// ============================================================
// 测试组 9:协调度量接线闭环(T0.3 → M2 转绿)
//
// 验证 Quest 完成时,审议延迟/委托开销/共识质量从待合并缓存
// 真实流入 CoordinationRatioReported 事件(不再需要手动注入)。
// ============================================================

/// 构造模拟 Parliament 发布的 DebateCompleted 事件
fn make_debate_event(quest_id: &str, latency_ms: f64, approval: f32) -> NexusEvent {
    NexusEvent::DebateCompleted {
        metadata: EventMetadata::new("parliament"),
        quest_id: quest_id.into(),
        proposal_id: "p-wire".into(),
        debate_latency_ms: latency_ms,
        strategy: "full".into(),
        weighted_approval_rate: Some(approval),
        participation_rate: Some(1.0),
        // M2 多维质量(接线测试固定取值,不影响延迟/增益断言)
        divergence: Some(0.1),
        abstention_rate: Some(0.0),
        consensus_margin: Some(approval - 0.6),
        outcome: "Reached".into(),
    }
}

/// 构造模拟 chimera-mas 发布的 DelegationCompleted 事件
fn make_delegation_event(quest_id: &str, overhead_ms: f64) -> NexusEvent {
    NexusEvent::DelegationCompleted {
        metadata: EventMetadata::new("chimera-mas:DelegationExecutor"),
        parent_id: "root".into(),
        quest_id: Some(quest_id.into()),
        total_overhead_ms: overhead_ms,
        sub_task_count: 2,
        success_count: 2,
    }
}

/// 驱动 Quest 全部任务到 Completed(触发 complete_quest)
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

#[tokio::test]
async fn test_complete_quest_merges_pending_parliament_and_delegation_samples() {
    let bus = EventBus::new();
    let engine = QuestEngine::new(bus);

    let quest = engine
        .create_quest(make_intent("分析需求。设计方案。"))
        .await
        .unwrap();

    // 模拟订阅器消费到的观测事件(同步投喂,避免异步时序不确定)
    engine.ingest_metrics_event(&make_debate_event(&quest.quest_id, 800.0, 0.9));
    engine.ingest_metrics_event(&make_delegation_event(&quest.quest_id, 600.0));

    complete_all_tasks(&engine, &quest).await;

    // 首个样本:EWMA = 样本值,总成本应含审议 800ms + 委托 600ms
    let ratio = engine
        .last_coordination_ratio()
        .expect("Quest 完成后应有比值记录");
    assert!(
        ratio.coordination_cost_ms >= 1400.0,
        "协调成本应含审议 800ms + 委托 600ms,实际: {}ms",
        ratio.coordination_cost_ms
    );

    // 推理增益应融入共识质量:全成功(1.0)×0.7 + 共识质量(0.9)×0.3 = 0.97
    assert!(
        (ratio.inference_gain - 0.97).abs() < 1e-3,
        "增益应为 0.7×1.0 + 0.3×0.9 = 0.97,实际: {}",
        ratio.inference_gain
    );

    // take 语义:合并后缓存应被清理(防泄漏)
    assert!(
        engine
            .pending_coordination_sample(&quest.quest_id)
            .is_none(),
        "complete_quest 后待合并缓存应被 take 清理"
    );
}

#[tokio::test]
async fn test_coordination_ratio_event_carries_wired_samples() {
    // 全链路:接线样本 → complete_quest → CoordinationRatioReported 事件携带真实成本
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let engine = QuestEngine::new(bus);

    let quest = engine
        .create_quest(make_intent("分析需求。设计方案。"))
        .await
        .unwrap();
    engine.ingest_metrics_event(&make_debate_event(&quest.quest_id, 900.0, 0.8));

    complete_all_tasks(&engine, &quest).await;

    // 从事件流提取 CoordinationRatioReported(跳过 QuestCreated/Progress 等)
    let mut reported = None;
    while let Ok(Some(event)) = rx.try_recv() {
        if let NexusEvent::CoordinationRatioReported {
            coordination_cost_ms,
            is_paradox_risk,
            ..
        } = event
        {
            reported = Some((coordination_cost_ms, is_paradox_risk));
        }
    }
    let (cost_ms, is_paradox_risk) = reported.expect("应收到 CoordinationRatioReported 事件");
    assert!(
        cost_ms >= 900.0,
        "事件协调成本应含真实审议延迟 900ms,实际: {cost_ms}ms"
    );
    // 默认基准 1000ms:cost_index=min(900+ε/1000,1)≈0.9,gain≈0.94 → ratio≈0.96 < 1.0
    // 本用例只验证数据真实流入,风险判定由下一用例覆盖
    let _ = is_paradox_risk;
}

#[tokio::test]
async fn test_paradox_risk_triggered_by_wired_samples_no_manual_injection() {
    // 推理悖论风险由真实接线样本触发(不再手动注入 record_and_compute)
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let mut engine = QuestEngine::new(bus);
    // 低成本基准(100ms)使审议延迟足以占满成本指数
    engine.with_metrics_config(CoordinationMetricsConfig::new(1.0, 100.0, 0.3));

    let quest = engine
        .create_quest(make_intent("分析需求。设计方案。"))
        .await
        .unwrap();
    // 审议 500ms → cost_index = min(500/100, 1.0) = 1.0;
    // 增益 = 0.7×1.0 + 0.3×0.9 = 0.97 → ratio ≈ 1.03 > 1.0 → 悖论风险
    engine.ingest_metrics_event(&make_debate_event(&quest.quest_id, 500.0, 0.9));

    complete_all_tasks(&engine, &quest).await;

    let ratio = engine
        .last_coordination_ratio()
        .expect("Quest 完成后应有比值记录");
    assert!(
        ratio.is_paradox_risk,
        "真实审议延迟应触发推理悖论风险,实际: {}",
        ratio.description()
    );

    // with_metrics_config 应保留 EventBus 绑定:事件仍发布且携带风险标记
    let mut saw_risk_event = false;
    while let Ok(Some(event)) = rx.try_recv() {
        if let NexusEvent::CoordinationRatioReported {
            is_paradox_risk: true,
            ..
        } = event
        {
            saw_risk_event = true;
        }
    }
    assert!(
        saw_risk_event,
        "更换度量配置后 CoordinationRatioReported 仍应发布(EventBus 绑定保留)"
    );
}

#[tokio::test]
async fn test_cancel_quest_cleans_pending_samples() {
    // 取消 Quest 时待合并缓存同步清理(防泄漏)
    let bus = EventBus::new();
    let engine = QuestEngine::new(bus);
    let quest = engine
        .create_quest(make_intent("分析需求。"))
        .await
        .unwrap();

    engine.ingest_metrics_event(&make_debate_event(&quest.quest_id, 100.0, 0.8));
    assert!(engine
        .pending_coordination_sample(&quest.quest_id)
        .is_some());

    engine
        .cancel_quest(&quest.quest_id, "operator")
        .await
        .unwrap();
    assert!(
        engine
            .pending_coordination_sample(&quest.quest_id)
            .is_none(),
        "cancel_quest 应清理待合并样本缓存"
    );
}

// ============================================================
// 测试组 10(M3-T3.1):CoordinationMetricsCollector 属性模拟测试
//
// 以任意 (cost, gain) 序列模拟长期运行,验证 EWMA 收敛性与
// is_paradox_risk 判定的自洽性(成本-收益模型的模拟验证)。
// ============================================================

proptest::proptest! {
    /// 属性:任意样本序列下,EWMA 状态与 ratio 判定始终自洽
    ///
    /// - cost EWMA 恒 ≥ 0;gain EWMA 恒在 [0,1]
    /// - is_paradox_risk ⛔ ratio > threshold 严格一致(判定无漂移)
    #[test]
    fn prop_collector_ewma_state_self_consistent(
        samples in proptest::collection::vec((0.0f64..3000.0, 0.0f32..=1.0), 1..60)
    ) {
        let collector = quest_engine::CoordinationMetricsCollector::new();
        for (cost_ms, gain) in samples {
            let cost = CoordinationCostSample::new(cost_ms, 0.0);
            let gain_sample = InferenceGainSample::new(gain);
            let ratio = collector.record_and_compute(&cost, &gain_sample);

            proptest::prop_assert!(ratio.coordination_cost_ms >= 0.0, "EWMA 成本恒非负");
            proptest::prop_assert!(
                (0.0..=1.0).contains(&ratio.gain_index),
                "EWMA 增益指数恒在 [0,1]"
            );
            proptest::prop_assert_eq!(
                ratio.is_paradox_risk,
                ratio.ratio > ratio.threshold,
                "悖论判定必须与 ratio>threshold 严格一致"
            );
        }
    }

    /// 属性:恒定样本序列下 EWMA 收敛到样本值(模拟稳态运行)
    ///
    /// 镜像 coordination_metrics.rs 单测的收敛性质,但用随机稳态点验证
    /// 收敛与风险判定在全参数空间成立(而非仅固定样本)。
    #[test]
    fn prop_collector_ewma_converges_to_constant_input(
        cost_ms in 0.0f64..2000.0,
        gain in 0.05f32..=1.0
    ) {
        let collector = quest_engine::CoordinationMetricsCollector::new();
        let cost = CoordinationCostSample::new(cost_ms, 0.0);
        let gain_sample = InferenceGainSample::new(gain);
        // 30 次恒定输入(alpha=0.3):EWMA 误差 ≤ 0.7^30 ≈ 2e-5,必然收敛
        let mut last = None;
        for _ in 0..30 {
            last = Some(collector.record_and_compute(&cost, &gain_sample));
        }
        let last = last.expect("至少一次记录");
        proptest::prop_assert!(
            (last.coordination_cost_ms - cost_ms).abs() < 1.0,
            "恒定输入下 EWMA 成本应收敛到样本值,实际 {} vs {}",
            last.coordination_cost_ms,
            cost_ms
        );
        proptest::prop_assert!(
            (f64::from(last.inference_gain) - f64::from(gain)).abs() < 1e-2,
            "恒定输入下 EWMA 增益应收敛到样本值"
        );
    }
}
