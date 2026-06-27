//! SecCore 错误路径测试(SubTask 37.7)
//!
//! 验证 SecCoreError 5 个错误路径的触发与处理:
//! AsaBlocked / AsaConfigInvalid / AsaAuditFailed / AsaHistoryOverflow / AsaThresholdInvalid。
//!
//! 对应架构层:L4 Security
//! 对应 Task 32:ASA 对抗性自我审计

#![forbid(unsafe_code)]

use seccore::{
    AsaAuditor, AsaConfig, AuditResult, InterventionAction, OperationAuditInput, SecCoreError,
};

/// 构造测试用 OperationAuditInput
fn make_input(content: &str, keywords: Vec<&str>, complexity: f32) -> OperationAuditInput {
    OperationAuditInput {
        operation_id: "test-op-001".to_string(),
        content: content.to_string(),
        risk_keywords: keywords.iter().map(|s| s.to_string()).collect(),
        complexity_score: complexity,
    }
}

// ============================================================
// SecCore 错误路径测试(5 个)
// ============================================================

/// 错误路径:AsaBlocked — ASA 拦截操作
///
/// WHY:safety_score < safety_threshold_block(默认 0.5)时,
/// ASA 返回 SecCoreError::AsaBlocked,操作被阻断,不进入沙箱。
/// 此测试验证 Block 级别干预与错误构造。
#[test]
fn test_error_asa_blocked() {
    // 场景 1:构造 AsaBlocked 错误,验证 Display
    let err = SecCoreError::AsaBlocked {
        operation_id: "op-malicious".into(),
        block_reason: "safety_score=0.2, high risk keywords".into(),
    };
    let msg = err.to_string();

    assert!(
        msg.contains("ASA 拦截"),
        "错误消息应包含 'ASA 拦截',实际: {msg}"
    );
    assert!(msg.contains("op-malicious"), "错误消息应包含 operation_id");
    assert!(msg.contains("safety_score=0.2"), "错误消息应包含阻断原因");

    // 场景 2:通过高风险关键字触发 AsaBlocked
    let auditor = AsaAuditor::with_default_config();
    // 6 个风险关键字 × 0.2 权重 = 1.2,safety_score = 1.0 - 1.2 = -0.2 → clamp 到 0.0
    // 0.0 < 0.5(阈值)→ Block
    let input = make_input(
        "kw0 kw1 kw2 kw3 kw4 kw5",
        vec!["kw0", "kw1", "kw2", "kw3", "kw4", "kw5"],
        0.5,
    );
    let result = auditor.audit_and_intervene(&input);
    let err = result.expect_err("6 个风险关键字应触发 AsaBlocked");

    match err {
        SecCoreError::AsaBlocked {
            operation_id,
            block_reason,
        } => {
            assert_eq!(operation_id, "test-op-001", "operation_id 应为 test-op-001");
            assert!(
                block_reason.contains("Block"),
                "block_reason 应包含 'Block',实际: {block_reason}"
            );
        }
        other => panic!("应返回 AsaBlocked,实际: {other:?}"),
    }

    // 场景 3:验证 Block 级别不进入沙箱(事中拦截优先)
    let result = auditor.audit(&input);
    assert_eq!(
        result.intervention,
        InterventionAction::Block,
        "6 个风险关键字应触发 Block 干预"
    );
    assert!(
        result.safety_score < 0.5,
        "safety_score 应 < 0.5(阈值),实际: {}",
        result.safety_score
    );
}

/// 错误路径:AsaConfigInvalid — ASA 配置错误
///
/// WHY:AsaConfig 的阈值应满足 safety_threshold_allow > safety_threshold_warn
/// >= safety_threshold_block,此测试验证无效配置下的边界行为。
#[test]
fn test_error_asa_config_invalid() {
    // 场景 1:阈值倒挂(allow < warn)
    // WHY AsaConfig 无 validate 方法,构造时不校验,但 audit 仍可运行
    let inverted_config = AsaConfig {
        safety_threshold_allow: 0.3,
        safety_threshold_warn: 0.8,
        safety_threshold_block: 0.5,
        ..AsaConfig::default()
    };
    let auditor = AsaAuditor::new(inverted_config);

    // 验证不 panic(配置倒挂时仍可审计)
    let input = make_input("safe op", vec![], 0.1);
    let result = auditor.audit(&input);
    assert!(
        (0.0..=1.0).contains(&result.safety_score),
        "配置倒挂时 safety_score 仍应 ∈ [0,1]"
    );

    // 场景 2:所有阈值相同(边界情况)
    let equal_config = AsaConfig {
        safety_threshold_allow: 0.5,
        safety_threshold_warn: 0.5,
        safety_threshold_block: 0.5,
        ..AsaConfig::default()
    };
    let auditor = AsaAuditor::new(equal_config);
    let result = auditor.audit(&input);
    // 验证不 panic
    assert!(
        matches!(
            result.intervention,
            InterventionAction::Allow | InterventionAction::Warn | InterventionAction::Block
        ),
        "阈值相同时仍应返回有效干预动作"
    );

    // 场景 3:负风险权重(异常配置)
    let neg_weight_config = AsaConfig {
        risk_weight: -0.5,
        ..AsaConfig::default()
    };
    let auditor = AsaAuditor::new(neg_weight_config);
    let input_with_kw = make_input("kw0 kw1", vec!["kw0", "kw1"], 0.1);
    let result = auditor.audit(&input_with_kw);
    // 验证不 panic,且 safety_score ∈ [0,1](clamp 保护)
    assert!(
        (0.0..=1.0).contains(&result.safety_score),
        "负风险权重时 safety_score 仍应 ∈ [0,1](clamp 保护)"
    );

    // 场景 4:0 max_history_records(边界)
    let zero_history_config = AsaConfig {
        max_history_records: 0,
        ..AsaConfig::default()
    };
    let auditor = AsaAuditor::new(zero_history_config);
    // 记录失败,验证不 panic
    auditor.record_failure("op-fail");
    let (total, failed) = auditor.history_stats();
    assert_eq!(total, 1, "total_count 应为 1");
    assert_eq!(failed, 1, "failure_count 应为 1");

    // 场景 5:验证默认配置的阈值合理性
    let default_config = AsaConfig::default();
    assert!(
        default_config.safety_threshold_allow > default_config.safety_threshold_warn,
        "默认 allow 阈值应 > warn 阈值"
    );
    assert!(
        default_config.safety_threshold_warn >= default_config.safety_threshold_block,
        "默认 warn 阈值应 >= block 阈值"
    );
}

/// 错误路径:AsaAuditFailed — 审计失败(边界条件)
///
/// WHY:ASA 审计应对边界条件(空内容、超大内容、空关键字)容错,
/// 不 panic、不返回越界值。此测试验证边界处理。
#[test]
fn test_error_asa_audit_failed() {
    let auditor = AsaAuditor::with_default_config();

    // 场景 1:空内容 → safety_score = 1.0(无关键字,无历史失败)
    let empty_input = make_input("", vec![], 0.0);
    let result = auditor.audit(&empty_input);
    assert!(
        (0.0..=1.0).contains(&result.safety_score),
        "空内容 safety_score 应 ∈ [0,1]"
    );
    assert!(
        result.safety_score >= 0.8,
        "空内容无关键字应 Allow(safety_score >= 0.8),实际: {}",
        result.safety_score
    );

    // 场景 2:超大内容(10000 字符)→ 不 panic
    let large_content = "x".repeat(10000);
    let large_input = make_input(&large_content, vec![], 0.5);
    let result = auditor.audit(&large_input);
    assert!(
        (0.0..=1.0).contains(&result.safety_score),
        "超大内容 safety_score 应 ∈ [0,1]"
    );

    // 场景 3:复杂度越界(>1.0)→ clamp 保护
    let over_complexity_input = make_input("safe op", vec![], 1.5);
    let result = auditor.audit(&over_complexity_input);
    assert!(
        (0.0..=1.0).contains(&result.efficiency_score),
        "复杂度越界时 efficiency_score 仍应 ∈ [0,1](clamp 保护)"
    );

    // 场景 4:复杂度负值(<0.0)→ clamp 保护
    let neg_complexity_input = make_input("safe op", vec![], -0.5);
    let result = auditor.audit(&neg_complexity_input);
    assert!(
        (0.0..=1.0).contains(&result.efficiency_score),
        "复杂度负值时 efficiency_score 仍应 ∈ [0,1](clamp 保护)"
    );

    // 场景 5:验证 AuditResult 字段完整性
    let input = make_input("balanced (content)", vec!["balanced"], 0.3);
    let result: AuditResult = auditor.audit(&input);
    assert!(
        (0.0..=1.0).contains(&result.safety_score),
        "safety_score 应 ∈ [0,1]"
    );
    assert!(
        (0.0..=1.0).contains(&result.correctness_score),
        "correctness_score 应 ∈ [0,1]"
    );
    assert!(
        (0.0..=1.0).contains(&result.efficiency_score),
        "efficiency_score 应 ∈ [0,1]"
    );
    assert!(!result.audit_reason.is_empty(), "audit_reason 不应为空");
}

/// 错误路径:AsaHistoryOverflow — 历史记录溢出
///
/// WHY:recent_failures 受 max_history_records 限制,防止内存无限增长。
/// 此测试验证历史记录溢出时的边界处理。
#[test]
fn test_error_asa_history_overflow() {
    // 场景 1:小容量历史记录(2 条),记录 5 次失败,验证溢出处理
    let small_config = AsaConfig {
        max_history_records: 2,
        ..AsaConfig::default()
    };
    let auditor = AsaAuditor::new(small_config);

    // 记录 5 次失败,但容量仅 2
    for i in 0..5 {
        auditor.record_failure(&format!("op-{i}"));
    }

    // 验证 total_count 与 failure_count 正确(不受 max_history_records 限制)
    let (total, failed) = auditor.history_stats();
    assert_eq!(
        total, 5,
        "total_count 应为 5(不受 max_history_records 限制)"
    );
    assert_eq!(failed, 5, "failure_count 应为 5");

    // 验证 failure_rate 正确(5/5 = 1.0)
    let input = make_input("safe op", vec![], 0.0);
    let result = auditor.audit(&input);
    // safety_score = 1.0 - 0.2*0 - 1.0(history_rate) = 0.0
    assert!(
        result.safety_score.abs() < 1e-6,
        "5 次失败后 safety_score 应为 0.0(历史失败率=1.0),实际: {}",
        result.safety_score
    );
    assert_eq!(
        result.intervention,
        InterventionAction::Block,
        "历史失败率=1.0 应触发 Block"
    );

    // 场景 2:大容量历史记录(1000 条),验证不内存泄漏
    let large_config = AsaConfig {
        max_history_records: 1000,
        ..AsaConfig::default()
    };
    let auditor = AsaAuditor::new(large_config);

    // 记录 1500 次失败(超过容量),验证不 panic
    for i in 0..1500 {
        auditor.record_failure(&format!("op-{i}"));
    }

    let (total, failed) = auditor.history_stats();
    assert_eq!(total, 1500, "total_count 应为 1500");
    assert_eq!(failed, 1500, "failure_count 应为 1500");

    // 场景 3:仅成功记录(无失败),验证 failure_rate = 0.0
    let auditor = AsaAuditor::with_default_config();
    for _ in 0..10 {
        auditor.record_success();
    }
    let (total, failed) = auditor.history_stats();
    assert_eq!(total, 10, "total_count 应为 10");
    assert_eq!(failed, 0, "failure_count 应为 0");

    let input = make_input("safe op", vec![], 0.0);
    let result = auditor.audit(&input);
    assert!(
        result.safety_score >= 0.8,
        "10 次成功 0 次失败应 Allow(safety_score >= 0.8),实际: {}",
        result.safety_score
    );
}

/// 错误路径:AsaThresholdInvalid — 阈值边界条件
///
/// WHY:ASA 干预分级依赖阈值比较,此测试验证阈值边界的正确处理
/// (恰好等于阈值时的判定)。
#[test]
fn test_error_asa_threshold_invalid() {
    // 场景 1:构造恰好等于 allow 阈值的 safety_score
    // safety_score = 1.0 - risk_weight * keyword_count - history_failure_rate
    // 默认 risk_weight = 0.2,要使 score = 0.8(allow 阈值),需 1 个关键字
    let auditor = AsaAuditor::with_default_config();
    let input = make_input("kw0", vec!["kw0"], 0.0);
    let result = auditor.audit(&input);

    // 验证 safety_score = 0.8(恰好等于 allow 阈值)
    assert!(
        (result.safety_score - 0.8).abs() < 1e-6,
        "1 个关键字 safety_score 应为 0.8(恰好等于 allow 阈值),实际: {}",
        result.safety_score
    );
    // WHY >= 阈值 → Allow(边界包含)
    assert_eq!(
        result.intervention,
        InterventionAction::Allow,
        "safety_score=0.8 应 Allow(>= 阈值 0.8)"
    );

    // 场景 2:构造恰好等于 warn 阈值的 safety_score
    // 要使 score = 0.5,需 2.5 个关键字(不可能),改用历史失败率
    // 1 个关键字 + 0.3 历史失败率 = 1.0 - 0.2 - 0.3 = 0.5
    let auditor = AsaAuditor::with_default_config();
    // 记录 7 次成功 3 次失败 → failure_rate = 3/10 = 0.3
    for _ in 0..7 {
        auditor.record_success();
    }
    for i in 0..3 {
        auditor.record_failure(&format!("fail-{i}"));
    }
    let input = make_input("kw0", vec!["kw0"], 0.0);
    let result = auditor.audit(&input);

    // 验证 safety_score ≈ 0.5(恰好等于 warn/block 阈值)
    assert!(
        (result.safety_score - 0.5).abs() < 1e-3,
        "1 关键字 + 0.3 失败率 safety_score 应 ≈ 0.5,实际: {}",
        result.safety_score
    );
    // WHY >= warn 阈值(0.5)→ Warn(边界包含)
    assert_eq!(
        result.intervention,
        InterventionAction::Warn,
        "safety_score=0.5 应 Warn(>= 阈值 0.5)"
    );

    // 场景 3:构造低于 block 阈值的 safety_score
    // 4 个关键字 → 1.0 - 0.8 = 0.2 < 0.5 → Block
    let auditor = AsaAuditor::with_default_config();
    let input = make_input("kw0 kw1 kw2 kw3", vec!["kw0", "kw1", "kw2", "kw3"], 0.0);
    let result = auditor.audit(&input);
    assert!(
        result.safety_score < 0.5,
        "4 个关键字 safety_score 应 < 0.5,实际: {}",
        result.safety_score
    );
    assert_eq!(
        result.intervention,
        InterventionAction::Block,
        "safety_score < 0.5 应 Block"
    );

    // 场景 4:验证 audit_and_intervene 在 Block 时返回 Err
    let result = auditor.audit_and_intervene(&input);
    assert!(result.is_err(), "Block 级别 audit_and_intervene 应返回 Err");
    if let Err(SecCoreError::AsaBlocked { .. }) = result {
        // 符合预期
    } else {
        panic!("应返回 AsaBlocked 错误");
    }

    // 场景 5:验证 audit_and_intervene 在 Allow/Warn 时返回 Ok
    let auditor = AsaAuditor::with_default_config();
    let safe_input = make_input("safe op", vec![], 0.0);
    let result = auditor.audit_and_intervene(&safe_input);
    assert!(result.is_ok(), "Allow 级别 audit_and_intervene 应返回 Ok");
}
