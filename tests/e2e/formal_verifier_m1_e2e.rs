//! FormalVerifier M1 端到端闭环测试(P7-T6)
//!
//! 对应架构层: L4 FormalVerifier + L0 Contracts(formal_props 类型)
//! 对应 ADR: ADR-047(M1 集成验证:5 属性全覆盖)
//! 对应计划: `IMPLEMENTATION_PLAN_Harness_Engineering_V3.md` Phase 7 P7-T6
//!
//! # M1 五属性验证器矩阵(属性定义 → 验证执行 → 结果报告 全链路)
//!
//! | # | 属性 | 验证器 | 落点 | 里程碑 |
//! |---|------|--------|------|--------|
//! | 1 | GSOE 谱系 DAG 完整性 | LineageChecker | gsoe-evolution | M0(既有) |
//! | 2 | AEGIS Critic 单调性 | CriticMonotonicityChecker | gsoe-evolution | M0(既有) |
//! | 3 | AutoDPO 偏好对一致性 | PreferenceConsistencyChecker | auto-dpo | **M1 新增** |
//! | 4 | 事件因果一致性 | CausalConsistencyChecker | event-bus | **M1 新增** |
//! | 5 | 学习单调性 | LearningMonotonicityChecker | omega-learner | **M1 新增** |
//!
//! # 测试覆盖
//!
//! 1. 属性 #3 正反路径(合法对集 Satisfied / 倒置对 Violated)
//! 2. 属性 #4 正反路径(真实 EventBus 发布流 Satisfied / 乱序流 Violated)
//! 3. 属性 #5 正反路径(收敛轨迹 Satisfied / 发散轨迹 Violated)
//! 4. S8 学习器真实轨迹 → 属性 #5 验证(学习层与验证器的真实集成)
//! 5. 五属性综合管线:InvariantSpec 定义 → 各验证器执行 → 汇总全 Satisfied

use auto_dpo::formal::PreferenceConsistencyChecker;
use auto_dpo::types::{PreferencePair, SampleQuality};
use event_bus::formal::CausalConsistencyChecker;
use event_bus::EventMetadata;
use gsoe_evolution::formal::critic_monotonicity::CriticMonotonicityChecker;
use gsoe_evolution::formal::lineage_checker::{
    LineageChecker, LineageEdge, LineageGraph, LineageNode,
};
use nexus_contracts::formal_props::{
    InvariantSpec, PropertyCategory, VerificationMethod, VerificationResult,
};
use omega_learner::formal::LearningMonotonicityChecker;
use omega_learner::s2_memory::TaskPhase;
use omega_learner::s8_mem_pi::{S8Context, S8Learner, S8Reward};

/// 构造测试偏好对
fn pair(id: &str, cs: f32, rs: f32) -> PreferencePair {
    PreferencePair {
        pair_id: id.to_string(),
        chosen: format!("chosen-{id}"),
        rejected: format!("rejected-{id}"),
        chosen_score: cs,
        rejected_score: rs,
        quality: SampleQuality::High,
    }
}

// ============================================================
// 属性 #3:AutoDPO 偏好对一致性(M1 新增)
// ============================================================

#[test]
fn test_property3_preference_consistency_satisfied() {
    let checker = PreferenceConsistencyChecker::new();
    let pairs = vec![pair("p1", 0.9, 0.4), pair("p2", 0.8, 0.6)];

    assert!(checker.verify_preference_asymmetry(&pairs).is_satisfied());
    assert!(checker.verify_no_self_preference(&pairs).is_satisfied());
    assert!(checker
        .verify_margin_bounded(&pairs, 0.05, 0.8)
        .is_satisfied());
}

#[test]
fn test_property3_inverted_pair_violated_with_counterexample() {
    let checker = PreferenceConsistencyChecker::new();
    let pairs = vec![pair("good", 0.9, 0.4), pair("inverted", 0.3, 0.8)];

    match checker.verify_preference_asymmetry(&pairs) {
        VerificationResult::Violated {
            counterexample,
            samples_tested,
        } => {
            // 反例必须精确指向违规对,且全部样本被检查
            assert!(counterexample.contains("inverted"));
            assert_eq!(samples_tested, 2);
        }
        other => panic!("期望 Violated,实际: {other:?}"),
    }
}

// ============================================================
// 属性 #4:事件因果一致性(M1 新增,真实 EventBus 流)
// ============================================================

#[test]
fn test_property4_real_event_stream_satisfied() {
    // 真实发布序:EventMetadata::new 生成 UUIDv7 + Utc::now,天然满足因果性质
    let stream: Vec<EventMetadata> = ["quest-engine", "parliament", "quest-engine", "seccore"]
        .iter()
        .map(|s| EventMetadata::new(*s))
        .collect();

    let checker = CausalConsistencyChecker::new();
    assert!(checker.verify_event_id_ordering(&stream).is_satisfied());
    assert!(checker
        .verify_per_source_timestamp_monotonic(&stream)
        .is_satisfied());
    assert!(checker.verify_event_id_unique(&stream).is_satisfied());
}

#[test]
fn test_property4_reordered_stream_violated() {
    let mut stream: Vec<EventMetadata> = ["a", "b", "c"]
        .iter()
        .map(|s| EventMetadata::new(*s))
        .collect();
    // 人为乱序(模拟事件重放攻击/lag 后错误重排)
    stream.swap(0, 2);

    let checker = CausalConsistencyChecker::new();
    assert!(matches!(
        checker.verify_event_id_ordering(&stream),
        VerificationResult::Violated { .. }
    ));
}

#[test]
fn test_property4_duplicate_delivery_violated() {
    let mut stream: Vec<EventMetadata> =
        ["a", "b"].iter().map(|s| EventMetadata::new(*s)).collect();
    stream[1].event_id = stream[0].event_id; // 重复投递

    let checker = CausalConsistencyChecker::new();
    assert!(matches!(
        checker.verify_event_id_unique(&stream),
        VerificationResult::Violated { .. }
    ));
}

// ============================================================
// 属性 #5:学习单调性(M1 新增)
// ============================================================

#[test]
fn test_property5_converging_trajectory_satisfied() {
    let checker = LearningMonotonicityChecker::new();

    // 步数严格递增 + 奖励有界 + 后悔率收敛:健康学习轨迹
    assert!(checker
        .verify_steps_monotonic(&[10, 20, 35, 50])
        .is_satisfied());
    assert!(checker
        .verify_reward_bounded(&[0.2, 0.9, 0.7], 0.0, 1.0)
        .is_satisfied());
    assert!(checker
        .verify_regret_non_increasing(&[0.8, 0.8, 0.5, 0.5, 0.2, 0.2], 2, 0.05)
        .is_satisfied());
}

#[test]
fn test_property5_diverging_regret_violated() {
    let checker = LearningMonotonicityChecker::new();
    // 后悔率发散(0.2 → 0.9):学习器失效,应触发熔断
    let result = checker.verify_regret_non_increasing(&[0.2, 0.2, 0.9, 0.9], 2, 0.05);
    assert!(matches!(result, VerificationResult::Violated { .. }));
}

#[test]
fn test_property5_s8_learner_real_trajectory() {
    // 学习层与验证器的真实集成:S8 学习器跑 30 步,轨迹快照必须过属性 #5
    let mut learner = S8Learner::with_default_alpha().expect("S8 学习器创建失败");
    let ctx = S8Context::new(TaskPhase::LongRun, 0.3, 0.7, 0.4).expect("上下文构造失败");

    let mut step_snapshots: Vec<u64> = Vec::new();
    let mut rewards: Vec<f64> = Vec::new();
    for i in 0..30 {
        let decision = learner.select(&ctx).expect("select 失败");
        // 奖励设计:达成率随训练小幅提升(模拟学习收益),噪声固定
        let achievement = 0.6 + (i as f64 / 30.0) * 0.3;
        let reward = S8Reward::new(achievement, 0.1).expect("奖励构造失败");
        learner
            .update(&ctx, decision, &reward)
            .expect("update 失败");
        step_snapshots.push(learner.total_steps());
        rewards.push(reward.reward());
    }

    let checker = LearningMonotonicityChecker::new();
    // 步数严格递增(update 每步 +1)
    assert!(checker
        .verify_steps_monotonic(&step_snapshots)
        .is_satisfied());
    // S8 奖励公式有界:[-λ, 1] = [-0.4, 1.0]
    assert!(checker
        .verify_reward_bounded(&rewards, -0.4, 1.0)
        .is_satisfied());
}

// ============================================================
// 综合管线:五属性全链路(定义 → 执行 → 汇总)
// ============================================================

#[test]
fn test_m1_five_property_pipeline_all_satisfied() {
    // Step 1: 属性定义(InvariantSpec,L0 契约层)
    let specs = [
        InvariantSpec::new(
            "M1-P1",
            "GSOE 谱系构成 DAG 且回滚可达",
            PropertyCategory::LineageIntegrity,
            "gsoe-evolution",
            VerificationMethod::PropTest,
        ),
        InvariantSpec::new(
            "M1-P2",
            "AEGIS Critic 评分单调不减",
            PropertyCategory::ScoreMonotonicity,
            "gsoe-evolution",
            VerificationMethod::PropTest,
        ),
        InvariantSpec::new(
            "M1-P3",
            "AutoDPO 偏好对 chosen > rejected",
            PropertyCategory::InvariantPreservation,
            "auto-dpo",
            VerificationMethod::PropTest,
        ),
        InvariantSpec::new(
            "M1-P4",
            "事件流 UUIDv7 时序且无重复投递",
            PropertyCategory::InvariantPreservation,
            "event-bus",
            VerificationMethod::PropTest,
        ),
        InvariantSpec::new(
            "M1-P5",
            "学习步数单调且后悔率收敛",
            PropertyCategory::ScoreMonotonicity,
            "omega-learner",
            VerificationMethod::PropTest,
        ),
    ];

    // Step 2: 各验证器执行(五属性各取一条代表性验证)
    let lineage_graph = LineageGraph::with_nodes_and_edges(
        vec![
            LineageNode::new("v1"),
            LineageNode::new("v2"),
            LineageNode::new("v3"),
        ],
        vec![LineageEdge::new("v1", "v2"), LineageEdge::new("v2", "v3")],
    );
    let r1 = LineageChecker::verify_dag_property(&lineage_graph);

    let critic = CriticMonotonicityChecker::new();
    let r2 = critic.verify_monotonicity(&[0.1, 0.5, 0.9], &[0.2, 0.4, 0.8]);

    let preference = PreferenceConsistencyChecker::new();
    let r3 = preference.verify_preference_asymmetry(&[pair("p1", 0.9, 0.3)]);

    let causal = CausalConsistencyChecker::new();
    let stream: Vec<EventMetadata> = ["a", "b"].iter().map(|s| EventMetadata::new(*s)).collect();
    let r4 = causal.verify_event_id_ordering(&stream);

    let learning = LearningMonotonicityChecker::new();
    let r5 = learning.verify_steps_monotonic(&[1, 2, 3]);

    // Step 3: 汇总报告(5/5 Satisfied = M1 门禁达成)
    let results = [r1, r2, r3, r4, r5];
    let satisfied = results.iter().filter(|r| r.is_satisfied()).count();
    assert_eq!(satisfied, 5, "M1 门禁要求 5/5 属性通过,实际 {satisfied}/5");
    assert_eq!(specs.len(), results.len());
}
