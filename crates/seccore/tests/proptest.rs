//! SecCore proptest — ASA 不变量属性测试(SubTask 37.7)
//!
//! 验证 safety_score ∈ [0,1] 与干预动作分级一致性。
//!
//! 对应架构层:L4 Security
//! 对应 Task 32:ASA 对抗性自我审计

#![forbid(unsafe_code)]

use proptest::prelude::*;
use seccore::{
    AsaAuditor, AsaConfig, InterventionAction, OperationAuditInput, PpoCritic, ScoreFusion,
};

/// 构造测试用 OperationAuditInput
fn make_input(content: &str, keywords: Vec<String>, complexity: f32) -> OperationAuditInput {
    OperationAuditInput {
        operation_id: "prop-op-001".to_string(),
        content: content.to_string(),
        risk_keywords: keywords,
        complexity_score: complexity,
    }
}

// 不变量:safety_score ∈ [0.0, 1.0]
//
// 生成随机 risk_weight、keyword_count、history_failure_rate,
// 计算 safety_score,验证结果 ∈ [0,1](clamp 后)。
//
// WHY 此不变量:safety_score 是 ASA 干预分级的输入,
// 越界值会导致干预动作异常(§6 架构红线:安全防线)
#[test]
fn proptest_safety_score_in_range() {
    proptest!(|(keyword_count in 0u32..=20, content_len in 1u32..=500)| {
        let auditor = AsaAuditor::with_default_config();

        // 构造 content 包含 keyword_count 个匹配的关键字
        let keywords: Vec<String> = (0..keyword_count).map(|i| format!("kw{i}")).collect();
        let content = keywords.join(" ");
        // 确保 content_len 至少有内容
        let content = if content.is_empty() {
            "x".repeat(content_len as usize)
        } else {
            content
        };

        let input = make_input(&content, keywords, 0.5);
        let result = auditor.audit(&input);

        prop_assert!(
            (0.0..=1.0).contains(&result.safety_score),
            "safety_score {} 应在 [0,1] 区间(keyword_count={})",
            result.safety_score,
            keyword_count
        );
        prop_assert!(
            (0.0..=1.0).contains(&result.correctness_score),
            "correctness_score {} 应在 [0,1] 区间",
            result.correctness_score
        );
        prop_assert!(
            (0.0..=1.0).contains(&result.efficiency_score),
            "efficiency_score {} 应在 [0,1] 区间",
            result.efficiency_score
        );
    });
}

// 不变量:干预动作分级一致性
//
// 验证干预动作与 safety_score 的对应关系:
// - score ≥ 0.8 → Allow
// - 0.5 ≤ score < 0.8 → Warn
// - score < 0.5 → Block
//
// WHY 此不变量:确保 ASA 干预分级可预测,
// 防止高风险操作被 Allow 或低风险操作被 Block(§6 架构红线:安全防线)
#[test]
fn proptest_intervention_action_consistency() {
    proptest!(|(keyword_count in 0u32..=10, fail_count in 0u32..=10, success_count in 0u32..=10)| {
        let auditor = AsaAuditor::with_default_config();

        // 构造历史记录
        for _ in 0..success_count {
            auditor.record_success();
        }
        for i in 0..fail_count {
            auditor.record_failure(&format!("fail-{i}"));
        }

        // 构造 content 包含 keyword_count 个匹配的关键字
        let keywords: Vec<String> = (0..keyword_count).map(|i| format!("kw{i}")).collect();
        let content = keywords.join(" ");
        let content = if content.is_empty() {
            "safe op".to_string()
        } else {
            content
        };

        let input = make_input(&content, keywords, 0.0);
        let result = auditor.audit(&input);

        let config = AsaConfig::default();
        let score = result.safety_score;

        // 验证干预动作与阈值一致
        let expected_action = if score >= config.safety_threshold_allow {
            InterventionAction::Allow
        } else if score >= config.safety_threshold_warn {
            InterventionAction::Warn
        } else {
            InterventionAction::Block
        };

        prop_assert_eq!(
            result.intervention,
            expected_action,
            "干预动作不一致:score={}, 期望 {:?}, 实际 {:?}",
            score,
            expected_action,
            result.intervention
        );
    });
}

// 不变量:历史失败率越高,safety_score 越低(单调性)
//
// WHY 此不变量:反馈闭环要求历史失败率上升时,
// safety_score 下降,使后续审计更严格(§6 架构红线:反馈闭环)
#[test]
fn proptest_history_failure_rate_monotonicity() {
    proptest!(|(keyword_count in 0u32..=5)| {
        let keywords: Vec<String> = (0..keyword_count).map(|i| format!("kw{i}")).collect();
        let content = if keywords.is_empty() {
            "safe op".to_string()
        } else {
            keywords.join(" ")
        };

        // 低失败率场景:10 次成功,1 次失败(rate=0.091)
        let auditor_low = AsaAuditor::with_default_config();
        for _ in 0..10 {
            auditor_low.record_success();
        }
        auditor_low.record_failure("fail-low");
        let result_low = auditor_low.audit(&make_input(&content, keywords.clone(), 0.0));

        // 高失败率场景:1 次成功,10 次失败(rate=0.909)
        let auditor_high = AsaAuditor::with_default_config();
        auditor_high.record_success();
        for i in 0..10 {
            auditor_high.record_failure(&format!("fail-high-{i}"));
        }
        let result_high = auditor_high.audit(&make_input(&content, keywords, 0.0));

        prop_assert!(
            result_high.safety_score <= result_low.safety_score,
            "高失败率 safety_score ({}) 应 ≤ 低失败率 safety_score ({})",
            result_high.safety_score,
            result_low.safety_score
        );
    });
}

// 不变量:风险关键字越多,safety_score 越低(单调性)
//
// WHY 此不变量:风险关键字反映操作的危险程度,
// 关键字增多应降低 safety_score(§6 架构红线:安全防线)
#[test]
fn proptest_keyword_count_monotonicity() {
    proptest!(|(base_keyword_count in 0u32..=3, extra_keyword_count in 1u32..=5)| {
        // 基准关键字数
        let keywords_base: Vec<String> = (0..base_keyword_count).map(|i| format!("kw{i}")).collect();
        let content_base = if keywords_base.is_empty() {
            "safe op".to_string()
        } else {
            keywords_base.join(" ")
        };

        // 增加关键字数
        let total_count = base_keyword_count + extra_keyword_count;
        let keywords_more: Vec<String> = (0..total_count).map(|i| format!("kw{i}")).collect();
        let content_more = keywords_more.join(" ");

        let auditor = AsaAuditor::with_default_config();
        let result_base = auditor.audit(&make_input(&content_base, keywords_base, 0.0));
        let result_more = auditor.audit(&make_input(&content_more, keywords_more, 0.0));

        prop_assert!(
            result_more.safety_score <= result_base.safety_score,
            "更多关键字 safety_score ({}) 应 ≤ 基准 safety_score ({})",
            result_more.safety_score,
            result_base.safety_score
        );
    });
}

// =============================================================================
// P3-3: PPO Critic 不变量属性测试
// =============================================================================
//
// 验证 PPO 核心不变量:
// 1. 前向推理输出非 NaN(所有合法输入)
// 2. Q 值转评分 ∈ [0, 1]
// 3. 置信度 ∈ [0, 1]
// 4. 训练后损失为有限值

// 不变量:PPO 前向推理输出非 NaN
//
// 对所有合法状态输入,前向推理输出必须为有限值(非 NaN/Inf)。
// 此不变量防止权重初始化或计算过程中产生非法值(§6 架构红线:安全防线)
#[test]
fn proptest_ppo_forward_no_nan() {
    proptest!(|(
        kw in 0.0f32..=1.0,
        rate in 0.0f32..=1.0,
        complexity in 0.0f32..=1.0,
        op_type in 0.0f32..=1.0,
    )| {
        let critic = PpoCritic::new();
        let state = [kw, rate, complexity, op_type];
        let output = critic.forward(&state);
        for (i, v) in output.iter().enumerate() {
            prop_assert!(
                v.is_finite(),
                "PPO forward output[{}] = {} 应为有限值(state={:?})",
                i,
                v,
                state
            );
        }
    });
}

// 不变量:Q 值转评分 ∈ [0, 1]
//
// 对所有可能的 Q 值输出,转换后的评分必须在 [0, 1] 区间。
// 此不变量确保 PPO 评分与规则评分兼容(§6 架构红线:安全防线)
#[test]
fn proptest_ppo_q_values_to_score_in_range() {
    proptest!(|(
        allow_q in -10.0f32..=10.0,
        warn_q in -10.0f32..=10.0,
        block_q in -10.0f32..=10.0,
    )| {
        let output = [allow_q, warn_q, block_q];
        let score = PpoCritic::q_values_to_score(&output);
        prop_assert!(
            (0.0..=1.0).contains(&score),
            "Q值转评分 {} 应在 [0,1] (Q={:?})",
            score,
            output
        );
    });
}

// 不变量:置信度 ∈ [0, 1]
//
// 对所有可能的 Q 值输出,置信度必须在 [0, 1] 区间。
// 此不变量确保置信度判断不会越界(§6 架构红线:安全防线)
#[test]
fn proptest_ppo_confidence_in_range() {
    proptest!(|(
        allow_q in -10.0f32..=10.0,
        warn_q in -10.0f32..=10.0,
        block_q in -10.0f32..=10.0,
    )| {
        let critic = PpoCritic::new();
        let output = [allow_q, warn_q, block_q];
        let conf = critic.confidence(&output);
        prop_assert!(
            (0.0..=1.0).contains(&conf),
            "置信度 {} 应在 [0,1] (Q={:?})",
            conf,
            output
        );
    });
}

// 不变量:训练后损失为有限值
//
// 对所有合法状态输入和目标 Q 值,训练产生的损失必须为有限值。
// 此不变量确保反向传播数值稳定,不会产生 NaN/Inf(§6 架构红线:安全防线)
#[test]
fn proptest_ppo_training_loss_finite() {
    proptest!(|(
        kw in 0.0f32..=1.0,
        rate in 0.0f32..=1.0,
        complexity in 0.0f32..=1.0,
        target0 in -5.0f32..=5.0,
        target1 in -5.0f32..=5.0,
        target2 in -5.0f32..=5.0,
    )| {
        let mut critic = PpoCritic::new();
        let state = [kw, rate, complexity, 0.5];
        let target = [target0, target1, target2];
        let loss = critic.train(&state, &target);
        prop_assert!(
            loss.is_finite(),
            "训练损失 {} 应为有限值(state={:?}, target={:?})",
            loss,
            state,
            target
        );
        prop_assert!(
            critic.is_initialized(),
            "训练后 is_initialized() 应为 true"
        );
        prop_assert_eq!(
            critic.trained_steps(),
            1,
            "训练后 trained_steps 应为 1"
        );
    });
}

// 不变量:融合评分 ∈ [0, 1]
//
// 对所有可能的规则评分、PPO 评分和置信度组合,
// 融合评分必须在 [0, 1] 区间(§6 架构红线:安全防线)
#[test]
fn proptest_score_fusion_output_in_range() {
    proptest!(|(
        rule_score in -1.0f32..=2.0,
        ppo_score in -1.0f32..=2.0,
        confidence in -0.5f32..=1.5,
    )| {
        let fusion = ScoreFusion::new();
        let score = fusion.fuse(rule_score, Some(ppo_score), confidence);
        prop_assert!(
            (0.0..=1.0).contains(&score),
            "融合评分 {} 应在 [0,1] (rule={}, ppo={}, conf={})",
            score,
            rule_score,
            ppo_score,
            confidence
        );
    });
}

// 不变量:安全优先 — 高置信度时规则评分 < 0.5 则融合评分 ≤ 规则评分
//
// 当 PPO 置信度高(≥ 0.6)且规则评分检测到 Block 级别(< 0.5)时,
// 融合评分不得高于规则评分(安全优先,PPO 不降级安全阈值)。
// 低置信度时使用加权平均,融合评分可能高于规则评分,但不足以改变 Block 等级。
// 此不变量确保 PPO 在高置信度下不能降低安全阈值(§6 架构红线:安全防线)
#[test]
fn proptest_score_fusion_safety_priority() {
    proptest!(|(
        rule_score in 0.0f32..0.5, // 规则评分 Block 区间
        ppo_score in 0.0f32..=1.0,
        confidence in 0.6f32..=1.0, // 仅高置信度适用安全优先
    )| {
        let fusion = ScoreFusion::new();
        let score = fusion.fuse(rule_score, Some(ppo_score), confidence);
        // 安全优先:高置信度 + 规则评分 < 0.5 → 融合评分 ≤ 规则评分
        prop_assert!(
            score <= rule_score + 1e-6,
            "安全优先违反:规则评分 Block({}), 融合评分({}) > 规则评分(conf={})",
            rule_score,
            score,
            confidence
        );
    });
}
