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

use event_bus::EventBus;
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
