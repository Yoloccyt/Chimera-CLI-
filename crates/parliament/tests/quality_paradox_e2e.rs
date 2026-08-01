//! ADR-064:质量趋势分析器与悖论风险仪表盘端到端集成测试
//!
//! 对应架构层:L8 Parliament
//! 对应分析:ADR-064 L8 Parliament 深度优化第二轮
//!
//! # 测试目标
//! 验证质量趋势分析器完整流程(正常趋势/分歧异常/弃权趋势/健康分反馈)
//! 和悖论风险预警完整流程(Yellow/Red/恢复正常/边界条件)在端到端场景下正确工作。
//!
//! # 设计决策(WHY)
//! - 独立集成测试文件,与 `quality_trend.rs`/`paradox_dashboard.rs` 内的单元测试互补:
//!   单元测试聚焦方法级正确性,集成测试验证完整流程与多组件交互。
//! - 使用 `ConsensusQualityMetrics` 直接构造,不依赖 `deliberate_with_policy` 流程:
//!   保证测试输入精确可控,不受辩论流程随机性影响。
//! - 使用 `nexus_contracts::ActivationStrategy` 验证自适应策略选择器:
//!   质量趋势分析器与自适应策略选择器的集成是 ADR-064 的关键闭环。

#![forbid(unsafe_code)]

use nexus_contracts::ActivationStrategy;
use parliament::{paradox_dashboard::ParadoxRiskDashboard, RiskLevel};
use parliament::{
    quality_trend::QualityTrendAnalyzer, AdaptiveStrategySelector, ConsensusQualityMetrics,
};

// ============================================================
// 辅助函数
// ============================================================

/// 构造测试用 ConsensusQualityMetrics
fn make_metrics(
    approval_rate: f32,
    abstention_rate: f32,
    divergence: f32,
) -> ConsensusQualityMetrics {
    ConsensusQualityMetrics {
        approval_rate,
        abstention_rate,
        divergence,
        consensus_margin: approval_rate - 0.6,
        skeptic_stance: 0.5,
    }
}

// ============================================================
// 质量趋势分析器端到端测试
// ============================================================

/// 正常趋势：连续推送 10 条正常质量指标(divergence ≤ 0.7, abstention_rate ≤ 0.4)
/// 验证健康评分保持在 100，无异常标志
#[test]
fn test_quality_trend_analyzer_e2e_normal_trend() {
    let mut analyzer = QualityTrendAnalyzer::new(None);

    for _ in 0..10 {
        analyzer.push(make_metrics(0.85, 0.1, 0.2));
    }

    assert_eq!(
        analyzer.consensus_health_score(),
        100,
        "10 条正常指标应保持健康评分 100"
    );
    assert!(!analyzer.divergence_anomaly(), "正常趋势不应有分歧异常");
    assert!(!analyzer.abstention_trend(), "正常趋势不应有弃权趋势");

    // 验证报告正确
    let report = analyzer.generate_report();
    assert!(!report.has_divergence_anomaly);
    assert!(!report.has_abstention_anomaly);
    assert_eq!(report.health_score, 100);
    assert_eq!(report.sample_count, 10);
}

/// 分歧异常：连续推送 5 条 divergence > 0.7 的指标
/// 验证 divergence_anomaly() 返回 true，健康评分降至 80(100 - 20)
#[test]
fn test_quality_trend_analyzer_e2e_divergence_anomaly() {
    let mut analyzer = QualityTrendAnalyzer::new(None);

    // 前 4 条：不触发分歧异常
    for _ in 0..4 {
        analyzer.push(make_metrics(0.5, 0.2, 0.8));
        assert!(!analyzer.divergence_anomaly(), "4 次分歧不应触发异常");
    }

    // 第 5 条：触发分歧异常
    analyzer.push(make_metrics(0.5, 0.2, 0.8));
    assert!(analyzer.divergence_anomaly(), "连续 5 次分歧应触发异常");
    assert!(!analyzer.abstention_trend(), "分歧异常不应触发弃权趋势");

    // 分歧异常扣 20 分；approval_rate=0.5 不 < 0.5，无低赞成率扣分
    assert_eq!(
        analyzer.consensus_health_score(),
        80,
        "分歧异常应扣 20 分，剩余 80"
    );

    // 验证报告包含分歧异常标志
    let report = analyzer.generate_report();
    assert!(report.has_divergence_anomaly);
    assert!(!report.has_abstention_anomaly);
    assert_eq!(report.health_score, 80);
}

/// 弃权趋势：连续推送 10 条同时 divergence > 0.7 和 abstention_rate > 0.4 的指标
/// 验证 abstention_trend() 返回 true，健康评分降至 65(100 - 20 - 15)
#[test]
fn test_quality_trend_analyzer_e2e_abstention_trend() {
    let mut analyzer = QualityTrendAnalyzer::new(None);

    // 同时触发分歧异常和弃权趋势
    // approval_rate=0.5 不 < 0.5，避免低赞成率扣分干扰
    for _ in 0..10 {
        analyzer.push(make_metrics(0.5, 0.5, 0.8));
    }

    // 分歧异常在连续 5 次时触发
    assert!(
        analyzer.divergence_anomaly(),
        "连续 10 次分歧应触发分歧异常"
    );

    // 弃权趋势需要连续 10 次 abstention_rate > 0.4
    assert!(analyzer.abstention_trend(), "连续 10 次弃权应触发弃权趋势");

    // 分歧异常(-20) + 弃权趋势(-15) = 扣 35 分 → 100 - 35 = 65
    assert_eq!(
        analyzer.consensus_health_score(),
        65,
        "分歧异常(20)+弃权趋势(15)应扣 35 分，剩余 65"
    );

    // 验证报告包含两种异常标志
    let report = analyzer.generate_report();
    assert!(report.has_divergence_anomaly);
    assert!(report.has_abstention_anomaly);
    assert_eq!(report.health_score, 65);
}

/// 健康分反馈：模拟 health_score < 40 的场景
/// 验证自适应策略选择器应返回 Full
#[test]
fn test_quality_trend_analyzer_e2e_health_score_feedback() {
    let mut analyzer = QualityTrendAnalyzer::new(None);

    // 20 条低赞成率 + 高分歧 → 健康分趋近于 0
    for _ in 0..20 {
        analyzer.push(make_metrics(0.2, 0.5, 0.8));
    }

    let score = analyzer.consensus_health_score();
    assert!(score < 40, "低质量场景健康分应 < 40, 实际: {score}");

    // 验证自适应策略选择器在 health_score < 40 时返回 Full
    let selector = AdaptiveStrategySelector::new(None);
    let suggested = selector.select(
        0.3,                            // risk_level: 中等
        0.0,                            // ratio: 正常
        0.3,                            // system_load: 正常
        score,                          // health_score: 极低
        ActivationStrategy::Simplified, // 当前策略
    );
    assert_eq!(
        suggested,
        ActivationStrategy::Full,
        "health_score({score}) < 40 时自适应策略应返回 Full"
    );
}

// ============================================================
// 悖论风险预警端到端测试
// ============================================================

/// Yellow 预警：单信号超标(ratio > 1.5)
/// 验证 risk_level() 返回 RiskLevel::Yellow
#[test]
fn test_paradox_risk_alert_e2e_yellow() {
    let mut dashboard = ParadoxRiskDashboard::new(None, None);

    // 初始状态为 Green
    assert_eq!(dashboard.risk_level(), RiskLevel::Green);

    // 仅 ratio 超标(> 1.5)，其他信号正常
    dashboard.update(2.0, 0.1, 80);
    assert_eq!(
        dashboard.risk_level(),
        RiskLevel::Yellow,
        "ratio=2.0 > 1.5 应触发 Yellow 预警"
    );
}

/// Red 预警：两信号超标(ratio > 1.5 AND health_score < 40)
/// 验证 risk_level() 返回 RiskLevel::Red
#[test]
fn test_paradox_risk_alert_e2e_red() {
    let mut dashboard = ParadoxRiskDashboard::new(None, None);

    // ratio 超标 + 健康分过低 → 两信号超标 → Red
    dashboard.update(2.0, 0.1, 30);
    assert_eq!(
        dashboard.risk_level(),
        RiskLevel::Red,
        "ratio=2.0 > 1.5 且 health_score=30 < 40 应触发 Red 熔断"
    );
}

/// 恢复正常：连续 5 次更新所有信号正常
/// 验证 risk_level() 返回 RiskLevel::Green
#[test]
fn test_paradox_risk_alert_e2e_recovery() {
    let mut dashboard = ParadoxRiskDashboard::new(None, None);

    // 先触发 Red
    dashboard.update(2.0, 0.5, 30);
    assert_eq!(dashboard.risk_level(), RiskLevel::Red);

    // 连续 4 次正常 → 仍为 Red（连续正常计数未达到 5）
    for i in 0..4 {
        dashboard.update(0.5, 0.1, 80);
        assert_eq!(
            dashboard.risk_level(),
            RiskLevel::Red,
            "第 {} 次正常不应恢复 Green(需 5 次连续正常)",
            i + 1
        );
    }

    // 第 5 次正常 → 恢复 Green
    dashboard.update(0.5, 0.1, 80);
    assert_eq!(
        dashboard.risk_level(),
        RiskLevel::Green,
        "连续 5 次正常应恢复 Green"
    );

    // 验证报告
    let report = dashboard.generate_report();
    assert_eq!(report.current_risk_level, RiskLevel::Green);
    assert_eq!(report.recent_alert_count, 2); // Green→Red, Red→Green 两次预警
}

/// 边界测试：ratio 恰好等于 1.5 不触发，等于 1.5001 触发
#[test]
fn test_paradox_risk_alert_e2e_boundary() {
    let mut dashboard = ParadoxRiskDashboard::new(None, None);

    // ratio 恰好等于 1.5 → 不超标 → Green
    dashboard.update(1.5, 0.1, 80);
    assert_eq!(
        dashboard.risk_level(),
        RiskLevel::Green,
        "ratio=1.5 恰好等于阈值不应触发预警"
    );

    // health_score 恰好等于 40 → 不超标 → Green
    dashboard.update(0.5, 0.1, 40);
    assert_eq!(
        dashboard.risk_level(),
        RiskLevel::Green,
        "health_score=40 恰好等于阈值不应触发预警"
    );

    // ratio = 1.5001 > 1.5 → 单信号超标 → Yellow
    dashboard.update(1.5001, 0.1, 80);
    assert_eq!(
        dashboard.risk_level(),
        RiskLevel::Yellow,
        "ratio=1.5001 略超阈值应触发 Yellow 预警"
    );

    // health_score = 39 < 40 + ratio 超标 → 两信号超标 → Red
    dashboard.update(1.5001, 0.1, 39);
    assert_eq!(
        dashboard.risk_level(),
        RiskLevel::Red,
        "ratio=1.5001 且 health_score=39 应触发 Red 熔断"
    );

    // 恢复正常后，veto_anomaly_rate 恰好等于 0.3 → 不超标
    dashboard.update(0.5, 0.3, 80);
    // 注意：连续正常计数需要累积，这里前面有异常，连续正常计数为 0
    // 所以第一次正常后，new_risk_level=Green, consecutive_normal=1, 但 < 5, risk_level 保持原样
    // 实际上 dashboard 的 risk_level 在 update 中的逻辑是：
    // 如果 new_risk_level == Green, 增加 consecutive_normal
    // 只有 consecutive_normal >= 5 时才设置 risk_level = Green
    // 否则保持之前的 risk_level (Red)
    // 所以第一次正常后，risk_level 仍然是 Red
    // 需要连续 5 次正常才能恢复 Green

    // 重置：重新创建一个 dashboard 单独测试 veto_anomaly 边界
    let mut dashboard2 = ParadoxRiskDashboard::new(None, None);
    // veto_anomaly_rate = 0.3 恰好等于阈值 → 不超标
    dashboard2.update(0.5, 0.3, 80);
    assert_eq!(
        dashboard2.risk_level(),
        RiskLevel::Green,
        "veto_anomaly_rate=0.3 恰好等于阈值不应触发预警"
    );

    // veto_anomaly_rate = 0.3001 > 0.3 → 单信号超标 → Yellow
    dashboard2.update(0.5, 0.3001, 80);
    assert_eq!(
        dashboard2.risk_level(),
        RiskLevel::Yellow,
        "veto_anomaly_rate=0.3001 略超阈值应触发 Yellow 预警"
    );
}
