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

use seccore::{AsaAuditor, OperationAuditInput, RiskLevel};

/// 构造测试用 OperationAuditInput。
fn make_input(content: &str, keywords: Vec<&str>, complexity: f32) -> OperationAuditInput {
    OperationAuditInput {
        operation_id: "test-op-001".to_string(),
        content: content.to_string(),
        risk_keywords: keywords.iter().map(|s| s.to_string()).collect(),
        complexity_score: complexity,
        semantic_vector: None,
        reference_risk_vectors: Vec::new(),
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
