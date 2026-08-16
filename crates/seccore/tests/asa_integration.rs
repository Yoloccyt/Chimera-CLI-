//! ASA 自适应安全审计集成测试 — 从 src/asa.rs 内联测试模块外移(L4-P2-1)
//!
//! 外移说明:原 #[cfg(test)] mod tests 混在生产文件(369 行,占 36%),
//! 外移后 asa.rs 仅保留生产代码。覆盖:规则评分、干预分级、AsaIntervention
//! 事件发布、升级通道阈值边界。
use seccore::{AsaAuditor, AsaConfig, InterventionAction, OperationAuditInput, SecCoreError};

/// 构造测试用 OperationAuditInput。
fn make_input(content: &str, keywords: Vec<&str>, complexity: f32) -> OperationAuditInput {
    OperationAuditInput {
        operation_id: "test-op-001".to_string(),
        content: content.to_string(),
        risk_keywords: keywords.iter().map(|s| s.to_string()).collect(),
        complexity_score: complexity,
    }
}

// === SubTask 32.2: 评分模型测试 ===

#[test]
fn test_audit_allow_no_keywords() {
    // 无风险关键字,无历史失败 → safety_score = 1.0 → Allow
    let auditor = AsaAuditor::with_default_config();
    let input = make_input("echo hello", vec![], 0.1);
    let result = auditor.audit(&input);
    assert_eq!(result.intervention, InterventionAction::Allow);
    assert!(result.safety_score >= 0.8);
}

#[test]
fn test_audit_warn_with_keywords() {
    // 2 个风险关键字 → safety_score = 1.0 - 0.4 = 0.6 → Warn
    let auditor = AsaAuditor::with_default_config();
    let input = make_input("sudo rm", vec!["sudo", "rm"], 0.1);
    let result = auditor.audit(&input);
    assert_eq!(result.intervention, InterventionAction::Warn);
    assert!(result.safety_score >= 0.5 && result.safety_score < 0.8);
}

#[test]
fn test_audit_block_with_many_keywords() {
    // 3 个风险关键字 → safety_score = 1.0 - 0.6 = 0.4 → Block
    let auditor = AsaAuditor::with_default_config();
    let input = make_input("sudo rm secret", vec!["sudo", "rm", "secret"], 0.1);
    let result = auditor.audit(&input);
    assert_eq!(result.intervention, InterventionAction::Block);
    assert!(result.safety_score < 0.5);
}

#[test]
fn test_audit_boundary_allow_threshold() {
    // safety_score 刚好 = 0.8 → Allow(>= 0.8)
    // 1 个关键字:1.0 - 0.2 = 0.8
    let auditor = AsaAuditor::with_default_config();
    let input = make_input("sudo test", vec!["sudo"], 0.0);
    let result = auditor.audit(&input);
    assert_eq!(result.intervention, InterventionAction::Allow);
    assert!((result.safety_score - 0.8).abs() < 0.001);
}

#[test]
fn test_audit_boundary_warn_threshold() {
    // safety_score 刚好 = 0.5 → Warn(>= 0.5)
    // history_failure_rate = 0.3,1 个关键字:1.0 - 0.2 - 0.3 = 0.5
    let auditor = AsaAuditor::with_default_config();
    auditor.record_success();
    auditor.record_success();
    auditor.record_success();
    auditor.record_failure("fail-op"); // total=4, fail=1, rate=0.25
                                       // 1 个关键字:1.0 - 0.2 - 0.25 = 0.55 → Warn
    let input = make_input("sudo test", vec!["sudo"], 0.0);
    let result = auditor.audit(&input);
    assert_eq!(result.intervention, InterventionAction::Warn);
}

#[test]
fn test_audit_history_failure_rate_impact() {
    // 高历史失败率 → safety_score 降低
    let auditor = AsaAuditor::with_default_config();
    auditor.record_success();
    auditor.record_failure("fail-1"); // total=2, fail=1, rate=0.5
                                      // 无关键字:1.0 - 0 - 0.5 = 0.5 → Warn
    let input = make_input("safe op", vec![], 0.0);
    let result = auditor.audit(&input);
    assert_eq!(result.intervention, InterventionAction::Warn);
    assert!((result.safety_score - 0.5).abs() < 0.001);
}

#[test]
fn test_audit_history_failure_rate_block() {
    // 高历史失败率 + 关键字 → Block
    let auditor = AsaAuditor::with_default_config();
    auditor.record_success();
    auditor.record_failure("fail-1"); // rate=0.5
                                      // 1 个关键字:1.0 - 0.2 - 0.5 = 0.3 → Block
    let input = make_input("sudo test", vec!["sudo"], 0.0);
    let result = auditor.audit(&input);
    assert_eq!(result.intervention, InterventionAction::Block);
}

#[test]
fn test_audit_correctness_score_paren_matched() {
    let auditor = AsaAuditor::with_default_config();
    let input = make_input("func(arg)", vec![], 0.0);
    let result = auditor.audit(&input);
    assert!((result.correctness_score - 0.9).abs() < 0.001);
}

#[test]
fn test_audit_correctness_score_paren_unmatched() {
    let auditor = AsaAuditor::with_default_config();
    let input = make_input("func(arg", vec![], 0.0);
    let result = auditor.audit(&input);
    assert!((result.correctness_score - 0.3).abs() < 0.001);
}

#[test]
fn test_audit_efficiency_score() {
    let auditor = AsaAuditor::with_default_config();
    // complexity = 0.4 → efficiency = 1.0 - 0.4*0.5 = 0.8
    let input = make_input("op", vec![], 0.4);
    let result = auditor.audit(&input);
    assert!((result.efficiency_score - 0.8).abs() < 0.001);
}

#[test]
fn test_audit_keyword_case_insensitive() {
    // 大小写不敏感匹配
    let auditor = AsaAuditor::with_default_config();
    let input = make_input("SUDO RM", vec!["sudo", "rm"], 0.0);
    let result = auditor.audit(&input);
    // 2 个关键字匹配:1.0 - 0.4 = 0.6 → Warn
    assert_eq!(result.intervention, InterventionAction::Warn);
}

// === SubTask 32.3: 干预动作分级测试(15 个用例) ===

// --- Allow 级别 5 个用例 ---

#[test]
fn test_intervene_allow_1() {
    let auditor = AsaAuditor::with_default_config();
    let input = make_input("echo hello", vec![], 0.0);
    let result = auditor.audit_and_intervene(&input);
    assert!(result.is_ok());
    assert_eq!(result.unwrap().intervention, InterventionAction::Allow);
}

#[test]
fn test_intervene_allow_2() {
    let auditor = AsaAuditor::with_default_config();
    let input = make_input("ls -la", vec![], 0.1);
    let result = auditor.audit_and_intervene(&input);
    assert!(result.is_ok());
    assert_eq!(result.unwrap().intervention, InterventionAction::Allow);
}

#[test]
fn test_intervene_allow_3() {
    let auditor = AsaAuditor::with_default_config();
    let input = make_input("pwd", vec![], 0.0);
    let result = auditor.audit_and_intervene(&input);
    assert!(result.is_ok());
    assert_eq!(result.unwrap().intervention, InterventionAction::Allow);
}

#[test]
fn test_intervene_allow_4() {
    // 1 个关键字:0.8 → Allow(边界)
    let auditor = AsaAuditor::with_default_config();
    let input = make_input("sudo test", vec!["sudo"], 0.0);
    let result = auditor.audit_and_intervene(&input);
    assert!(result.is_ok());
    assert_eq!(result.unwrap().intervention, InterventionAction::Allow);
}

#[test]
fn test_intervene_allow_5() {
    let auditor = AsaAuditor::with_default_config();
    let input = make_input("whoami", vec![], 0.2);
    let result = auditor.audit_and_intervene(&input);
    assert!(result.is_ok());
    assert_eq!(result.unwrap().intervention, InterventionAction::Allow);
}

// --- Warn 级别 5 个用例 ---

#[test]
fn test_intervene_warn_1() {
    // 2 个关键字:0.6 → Warn
    let auditor = AsaAuditor::with_default_config();
    let input = make_input("sudo rm", vec!["sudo", "rm"], 0.0);
    let result = auditor.audit_and_intervene(&input);
    assert!(result.is_ok());
    assert_eq!(result.unwrap().intervention, InterventionAction::Warn);
}

#[test]
fn test_intervene_warn_2() {
    // 2 个关键字:0.6 → Warn
    let auditor = AsaAuditor::with_default_config();
    let input = make_input("chmod chown", vec!["chmod", "chown"], 0.0);
    let result = auditor.audit_and_intervene(&input);
    assert!(result.is_ok());
    assert_eq!(result.unwrap().intervention, InterventionAction::Warn);
}

#[test]
fn test_intervene_warn_3() {
    // 1 个关键字 + 0.1 失败率:1.0 - 0.2 - 0.1 = 0.7 → Warn
    let auditor = AsaAuditor::with_default_config();
    for _ in 0..9 {
        auditor.record_success();
    }
    auditor.record_failure("fail"); // total=10, fail=1, rate=0.1
    let input = make_input("sudo test", vec!["sudo"], 0.0);
    let result = auditor.audit_and_intervene(&input);
    assert!(result.is_ok());
    assert_eq!(result.unwrap().intervention, InterventionAction::Warn);
}

#[test]
fn test_intervene_warn_4() {
    // 2 个关键字:0.6 → Warn
    let auditor = AsaAuditor::with_default_config();
    let input = make_input("secret password", vec!["secret", "password"], 0.0);
    let result = auditor.audit_and_intervene(&input);
    assert!(result.is_ok());
    assert_eq!(result.unwrap().intervention, InterventionAction::Warn);
}

#[test]
fn test_intervene_warn_5() {
    // 边界:safety_score = 0.5 → Warn(>= 0.5)
    let auditor = AsaAuditor::with_default_config();
    auditor.record_success();
    auditor.record_failure("fail"); // total=2, fail=1, rate=0.5
                                    // 无关键字:1.0 - 0 - 0.5 = 0.5 → Warn
    let input = make_input("safe op", vec![], 0.0);
    let result = auditor.audit_and_intervene(&input);
    assert!(result.is_ok());
    assert_eq!(result.unwrap().intervention, InterventionAction::Warn);
}

// --- Block 级别 5 个用例 ---

#[test]
fn test_intervene_block_1() {
    // 3 个关键字:0.4 → Block
    let auditor = AsaAuditor::with_default_config();
    let input = make_input("sudo rm secret", vec!["sudo", "rm", "secret"], 0.0);
    let result = auditor.audit_and_intervene(&input);
    assert!(matches!(result, Err(SecCoreError::AsaBlocked { .. })));
}

#[test]
fn test_intervene_block_2() {
    // 3 个关键字:0.4 → Block
    let auditor = AsaAuditor::with_default_config();
    let input = make_input("sudo chmod chown", vec!["sudo", "chmod", "chown"], 0.0);
    let result = auditor.audit_and_intervene(&input);
    assert!(matches!(result, Err(SecCoreError::AsaBlocked { .. })));
}

#[test]
fn test_intervene_block_3() {
    // 高失败率 + 关键字:1.0 - 0.2 - 0.5 = 0.3 → Block
    let auditor = AsaAuditor::with_default_config();
    auditor.record_success();
    auditor.record_failure("fail"); // rate=0.5
    let input = make_input("sudo test", vec!["sudo"], 0.0);
    let result = auditor.audit_and_intervene(&input);
    assert!(matches!(result, Err(SecCoreError::AsaBlocked { .. })));
}

#[test]
fn test_intervene_block_4() {
    // 5 个关键字:1.0 - 1.0 = 0.0 → Block
    let auditor = AsaAuditor::with_default_config();
    let input = make_input(
        "sudo rm secret password chmod",
        vec!["sudo", "rm", "secret", "password", "chmod"],
        0.0,
    );
    let result = auditor.audit_and_intervene(&input);
    assert!(matches!(result, Err(SecCoreError::AsaBlocked { .. })));
}

#[test]
fn test_intervene_block_5() {
    // 高失败率(无关键字):1.0 - 0 - 0.8 = 0.2 → Block
    let auditor = AsaAuditor::with_default_config();
    for _ in 0..4 {
        auditor.record_failure("fail");
    }
    auditor.record_success(); // total=5, fail=4, rate=0.8
    let input = make_input("safe op", vec![], 0.0);
    let result = auditor.audit_and_intervene(&input);
    assert!(matches!(result, Err(SecCoreError::AsaBlocked { .. })));
}

// === 历史记录测试 ===

#[test]
fn test_record_success_updates_total() {
    let auditor = AsaAuditor::with_default_config();
    auditor.record_success();
    auditor.record_success();
    let (total, fail) = auditor.history_stats();
    assert_eq!(total, 2);
    assert_eq!(fail, 0);
}

#[test]
fn test_record_failure_updates_counts() {
    let auditor = AsaAuditor::with_default_config();
    auditor.record_success();
    auditor.record_failure("fail-1");
    let (total, fail) = auditor.history_stats();
    assert_eq!(total, 2);
    assert_eq!(fail, 1);
}

#[test]
fn test_max_history_records_limit() {
    // 验证 recent_failures 长度受 max_history_records 限制
    let config = AsaConfig {
        max_history_records: 3,
        ..AsaConfig::default()
    };
    let auditor = AsaAuditor::new(config);
    auditor.record_failure("fail-1");
    auditor.record_failure("fail-2");
    auditor.record_failure("fail-3");
    auditor.record_failure("fail-4");
    auditor.record_failure("fail-5");
    // total=5, fail=5,recent_failures 限制为 3,但 failure_rate 仍正确
    let (total, fail) = auditor.history_stats();
    assert_eq!(total, 5);
    assert_eq!(fail, 5);
    // failure_rate = 5/5 = 1.0,safety_score = 1.0 - 0 - 1.0 = 0.0
    let result = auditor.audit(&make_input("test", vec![], 0.0));
    assert!((result.safety_score - 0.0).abs() < 0.001);
}

// === 配置测试 ===

#[test]
fn test_custom_config_thresholds() {
    let config = AsaConfig {
        safety_threshold_allow: 0.9,
        safety_threshold_warn: 0.7,
        safety_threshold_block: 0.7,
        risk_weight: 0.1,
        history_failure_weight: 0.3,
        max_history_records: 1000,
    };
    let auditor = AsaAuditor::new(config);
    // 1 个关键字:1.0 - 0.1 = 0.9 → Allow(>= 0.9)
    let input = make_input("sudo test", vec!["sudo"], 0.0);
    let result = auditor.audit(&input);
    assert_eq!(result.intervention, InterventionAction::Allow);
}

#[test]
fn test_audit_reason_not_empty() {
    let auditor = AsaAuditor::with_default_config();
    let input = make_input("echo hello", vec![], 0.0);
    let result = auditor.audit(&input);
    assert!(!result.audit_reason.is_empty());
    assert!(result.audit_reason.contains("Allow"));
}
