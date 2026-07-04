//! SecCore proptest — ASA 不变量属性测试(SubTask 37.7)
//!
//! 验证 safety_score ∈ [0,1] 与干预动作分级一致性。
//!
//! 对应架构层:L4 Security
//! 对应 Task 32:ASA 对抗性自我审计

#![forbid(unsafe_code)]

use proptest::prelude::*;
use seccore::{AsaAuditor, AsaConfig, InterventionAction, OperationAuditInput};

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
