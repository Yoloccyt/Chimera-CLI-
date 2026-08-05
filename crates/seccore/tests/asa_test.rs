//! ASA 空关键字绕过修复测试 — Task I-2 [N4]
//!
//! 对应漏洞:N4 ASA 空关键字绕过(High 级别安全漏洞)
//! 修复目标:当 risk_keywords 为空时,audit() 返回的 AuditResult.risk_level
//!         必须为 RiskLevel::Unknown,触发下游额外审计检查。
//!
//! 安全语义(WHY):
//! - 调用者若不提供任何风险关键字,系统无法评估真实风险等级
//! - 旧实现将空关键字等价于"无风险"(Low),调用者可通过省略关键字列表绕过检测
//! - 修复后空关键字 → RiskLevel::Unknown,作为信号触发 Parliament/下游消费者额外审计
//!
//! TDD 流程:本文件先写(RED),实现 asa.rs 改造后转 GREEN。

use seccore::{AsaAuditor, AsaConfig, OperationAuditInput, PpoCritic, RiskLevel};

/// 构造测试用 OperationAuditInput。
fn make_input(content: &str, keywords: Vec<&str>, complexity: f32) -> OperationAuditInput {
    OperationAuditInput {
        operation_id: "test-op-001".to_string(),
        content: content.to_string(),
        risk_keywords: keywords.iter().map(|s| s.to_string()).collect(),
        complexity_score: complexity,
    }
}

// =============================================================================
// N4 修复核心测试:空关键字列表 → RiskLevel::Unknown
// =============================================================================
// 验证:当调用者不提供任何风险关键字时,系统不能默认"无风险"(Low),
// 必须返回 RiskLevel::Unknown 作为信号,触发下游额外审计检查。
// 这防止调用者通过省略关键字列表绕过风险检测。

#[test]
fn test_audit_empty_keywords_returns_unknown() {
    // 空风险关键字列表 → risk_level 必须为 Unknown(非 Low)
    // 安全语义:未提供检测维度 = 风险无法评估 = Unknown
    let auditor = AsaAuditor::with_default_config();
    let input = make_input("echo hello", vec![], 0.1);
    let result = auditor.audit(&input);
    assert_eq!(
        result.risk_level,
        RiskLevel::Unknown,
        "空风险关键字列表必须返回 RiskLevel::Unknown 以触发额外审计, \
         防止调用者通过省略关键字列表绕过风险检测"
    );
}

#[test]
fn test_audit_nonempty_keywords_returns_known_risk_level() {
    // 对照测试:非空关键字列表(无匹配)→ risk_level 必须为 Low(已知低风险)
    // 此测试确保修复不影响正常路径:调用者提供了关键字列表(即使无匹配)
    // 系统也能正常评估为 Low(而非误判为 Unknown)
    let auditor = AsaAuditor::with_default_config();
    let input = make_input("echo hello", vec!["nonexistent_keyword"], 0.1);
    let result = auditor.audit(&input);
    assert_eq!(
        result.risk_level,
        RiskLevel::Low,
        "非空关键字列表(无匹配)应返回 Low,而非 Unknown"
    );
}

// =============================================================================
// P3-3: ASA PPO 强化学习接入测试
// =============================================================================
// 验证 PPO Critic 模型与 AsaAuditor 的集成,包括:
// - with_ppo 构造函数(无崩溃)
// - safety_score ∈ [0, 1] 不变量(PPO 随机权重不影响)
// - record_success / record_failure 训练 PPO(历史统计正确)
// - 安全优先:规则评分 Block 时 fused_score < 0.5(PPO 不降级)

#[test]
fn test_audit_with_ppo_constructor_no_crash() {
    // with_ppo 创建带 PPO 的审计器,验证无崩溃且 safety_score ∈ [0, 1]
    // WHY 不验证具体干预动作:PPO 随机权重导致输出非确定,仅验证不变量
    let config = AsaConfig::default();
    let bus = event_bus::EventBus::new();
    let critic = PpoCritic::new();
    let auditor = AsaAuditor::with_ppo(config, bus, critic);

    let input = make_input("echo hello", vec![], 0.1);
    let result = auditor.audit(&input);
    assert!(
        (0.0..=1.0).contains(&result.safety_score),
        "PPO 模式下 safety_score 应在 [0,1], 实际: {}",
        result.safety_score
    );
    assert!(
        (0.0..=1.0).contains(&result.correctness_score),
        "correctness_score 应在 [0,1]"
    );
    assert!(
        (0.0..=1.0).contains(&result.efficiency_score),
        "efficiency_score 应在 [0,1]"
    );
}

#[test]
fn test_audit_with_ppo_record_success_history_stats() {
    // record_success 更新历史统计(PPO 训练不影响历史计数)
    let config = AsaConfig::default();
    let bus = event_bus::EventBus::new();
    let critic = PpoCritic::new();
    let auditor = AsaAuditor::with_ppo(config, bus, critic);

    for _ in 0..10 {
        auditor.record_success();
    }

    // 验证历史统计正确(PPO 训练不影响)
    let (total, fail) = auditor.history_stats();
    assert_eq!(total, 10, "成功次数应为 10");
    assert_eq!(fail, 0, "失败次数应为 0");

    // 验证 PPO 已训练(至少 10 步)
    // 注:无法直接访问 PPO 内部状态,通过 history_stats 间接验证
}

#[test]
fn test_audit_with_ppo_record_failure_history_stats() {
    // record_failure 更新历史统计(PPO 训练不影响历史计数)
    let config = AsaConfig::default();
    let bus = event_bus::EventBus::new();
    let critic = PpoCritic::new();
    let auditor = AsaAuditor::with_ppo(config, bus, critic);

    for i in 0..5 {
        auditor.record_failure(&format!("fail-{i}"));
    }

    let (total, fail) = auditor.history_stats();
    assert_eq!(total, 5, "总次数应为 5");
    assert_eq!(fail, 5, "失败次数应为 5");
}

#[test]
fn test_audit_with_ppo_rule_priority_block() {
    // 规则评分 Block 时,fused_score < 0.5(安全优先,PPO 不降级)
    // WHY 此测试可验证:规则评分(0.0) < 0.5 → ScoreFusion 安全优先分支
    // → fused_score = rule_score.min(ppo_score) ≤ 0.0 < 0.5
    let config = AsaConfig::default();
    let bus = event_bus::EventBus::new();
    let critic = PpoCritic::new();
    let auditor = AsaAuditor::with_ppo(config, bus, critic);

    // 高失败率(5/5 = 1.0) + 5 个关键字 → 规则评分 clamp 到 0.0
    for i in 0..5 {
        auditor.record_failure(&format!("fail-{i}"));
    }

    let input = make_input(
        "sudo rm secret password chmod",
        vec!["sudo", "rm", "secret", "password", "chmod"],
        0.0,
    );
    let result = auditor.audit(&input);
    // 安全优先:规则评分 < 0.5 → fused_score ≤ 规则评分 < 0.5
    assert!(
        result.safety_score < 0.5,
        "安全优先:规则评分 Block 时 fused_score({}) < 0.5",
        result.safety_score
    );
}

#[test]
fn test_audit_with_ppo_history_rate_high_blocks() {
    // 高历史失败率导致规则评分 < 0.5 → fused_score < 0.5(安全优先)
    let config = AsaConfig::default();
    let bus = event_bus::EventBus::new();
    let critic = PpoCritic::new();
    let auditor = AsaAuditor::with_ppo(config, bus, critic);

    // 8 次失败,2 次成功 → rate = 0.8 → 规则评分 = 1.0 - 0 - 0.8 = 0.2 < 0.5
    for _ in 0..8 {
        auditor.record_failure("fail");
    }
    for _ in 0..2 {
        auditor.record_success();
    }

    let input = make_input("echo hello", vec![], 0.1);
    let result = auditor.audit(&input);
    // 规则评分 0.2 < 0.5 → 安全优先 → fused_score ≤ 0.2 < 0.5
    assert!(
        result.safety_score < 0.5,
        "高失败率: fused_score({}) < 0.5",
        result.safety_score
    );
}
