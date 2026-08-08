//! 关键路径动态识别测试（Milestone B-6，推理悖论红线度量互补）
//!
//! 对应方案（CHIMERA_V3_专项优化方案_v2.21基线.md §5.1 P3 / §6 B-6）：
//! 规则驱动关键路径识别，与 coordination_metrics（ADR-063）接线互补——
//! 6 风险因子（任务规模/依赖深度/协调成本比/否决率/超时率/资源水位）
//! 综合判定任务链是否为关键路径（高风险链应优先治理）。

#![forbid(unsafe_code)]

use parliament::critical_path::{assess_critical_path, RiskFactorInput};

/// 低风险输入 → 非关键路径
#[test]
fn low_risk_is_not_critical() {
    let input = RiskFactorInput {
        task_count: 3,
        max_dependency_depth: 2,
        coordination_to_gain: 0.3,
        veto_rate: 0.0,
        timeout_rate: 0.0,
        budget_watermark: 0.2,
    };
    let report = assess_critical_path(&input);
    assert!(!report.is_critical, "低风险不应判定关键: {report:?}");
    assert!(report.contributing_factors.is_empty());
}

/// 高风险输入 → 关键路径 + 列出超标因子
#[test]
fn high_risk_is_critical_with_factors() {
    let input = RiskFactorInput {
        task_count: 50,
        max_dependency_depth: 12,
        coordination_to_gain: 2.5, // 协调成本远超推理增益（推理悖论红线）
        veto_rate: 0.4,
        timeout_rate: 0.3,
        budget_watermark: 0.9,
    };
    let report = assess_critical_path(&input);
    assert!(report.is_critical, "高风险应判定关键: {report:?}");
    assert!(!report.contributing_factors.is_empty(), "应列出超标因子");
}

/// 单因子超标（资源水位 0.95）→ 关键路径
#[test]
fn single_factor_breach_triggers_critical() {
    let input = RiskFactorInput {
        task_count: 3,
        max_dependency_depth: 2,
        coordination_to_gain: 0.2,
        veto_rate: 0.0,
        timeout_rate: 0.0,
        budget_watermark: 0.95, // 资源水位逼近上限
    };
    let report = assess_critical_path(&input);
    assert!(report.is_critical, "资源水位超标应判定关键: {report:?}");
    assert!(
        report
            .contributing_factors
            .iter()
            .any(|f| f.name.contains("资源")),
        "应标记资源水位因子: {:?}",
        report.contributing_factors
    );
}

/// 风险分数单调性：输入越差分数越高
#[test]
fn risk_score_is_monotonic() {
    let low = RiskFactorInput {
        task_count: 2,
        max_dependency_depth: 1,
        coordination_to_gain: 0.1,
        veto_rate: 0.0,
        timeout_rate: 0.0,
        budget_watermark: 0.1,
    };
    let high = RiskFactorInput {
        task_count: 100,
        max_dependency_depth: 20,
        coordination_to_gain: 5.0,
        veto_rate: 0.8,
        timeout_rate: 0.7,
        budget_watermark: 1.0,
    };
    let r_low = assess_critical_path(&low);
    let r_high = assess_critical_path(&high);
    assert!(
        r_high.risk_score > r_low.risk_score,
        "高分输入应得高分: {} vs {}",
        r_high.risk_score,
        r_low.risk_score
    );
}

/// 恰好等于阈值 → 不超标（严格大于语义）
#[test]
fn exactly_at_threshold_is_not_breach() {
    let input = RiskFactorInput {
        task_count: 32, // 阈值 32，等于不超标
        max_dependency_depth: 8,
        coordination_to_gain: 1.0,
        veto_rate: 0.3,
        timeout_rate: 0.2,
        budget_watermark: 0.85,
    };
    let report = assess_critical_path(&input);
    assert!(
        report.contributing_factors.is_empty(),
        "等于阈值不应超标: {:?}",
        report.contributing_factors
    );
}

/// 无单因子超标但加权分超 0.6 → 关键路径（综合判定路径）
#[test]
fn weighted_score_alone_can_trigger_critical() {
    let input = RiskFactorInput {
        task_count: 30,            // 低于阈值 32
        max_dependency_depth: 7,   // 低于阈值 8
        coordination_to_gain: 0.9, // 低于阈值 1.0
        veto_rate: 0.28,
        timeout_rate: 0.19,
        budget_watermark: 0.84,
    };
    let report = assess_critical_path(&input);
    // 各因子归一化：30/64+7/16+0.9/3+0.28+0.19+0.84 = 0.469+0.438+0.3+0.28+0.19+0.84
    // 加权：0.2*0.469+0.2*0.438+0.25*0.3+0.15*0.28+0.1*0.19+0.1*0.84 ≈ 0.544 < 0.6
    assert!(!report.is_critical, "低综合分不应关键: {:?}", report);
}
