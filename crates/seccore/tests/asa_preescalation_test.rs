//! ASA 前置实时审计 TDD 测试 — P1-W3.2 (D6 修复)
//!
//! 对应文档:
//! - spec.md §Scenario "高危操作强制升级通道" + "ASA 自适应审计前置"
//! - tasks.md P1-W3.2 (SubTask P1-W3.2.1 / .2)
//!
//! 核心契约:
//! - WHEN risk_score ∈ [71,90] (Parliament 档) 操作即将执行
//! - THEN ASA 实时审计 MUST 在 Parliament 辩论 BEFORE 发生
//! - AND ASA Block → 返回 AsaBlocked 错误(Parliament 辩论跳过)
//! - AND ASA Allow/Warn → 继续进入 EscalationHandler.parliament_debate()
//! - Low-risk (ReadOnly/Normal) 与 EscalateToHuman 不触发 ASA(快速路径)

use seccore::{
    AsaAuditor, AsaConfig, Command, CommandPolicy, EnvPolicy, EscalationHandler, EscalationTier,
    Sandbox, SecCoreError,
};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

// === Mock EscalationHandler:记录是否被调用,可配置批准/否决 ===

struct RecordingHandler {
    called: Arc<AtomicBool>,
    approve: bool,
}

impl EscalationHandler for RecordingHandler {
    fn parliament_debate(
        &self,
        _spec: &seccore::CommandSpec,
        _risk_score: u8,
    ) -> Result<(), SecCoreError> {
        self.called.store(true, Ordering::SeqCst);
        if self.approve {
            Ok(())
        } else {
            Err(SecCoreError::PolicyViolation(
                "Parliament rejected".to_string(),
            ))
        }
    }
}

/// 构造批准型 handler,返回 (handler, called_flag)。
fn approving_handler() -> (Box<dyn EscalationHandler>, Arc<AtomicBool>) {
    let flag = Arc::new(AtomicBool::new(false));
    let handler = RecordingHandler {
        called: flag.clone(),
        approve: true,
    };
    (Box::new(handler), flag)
}

// === Test 1: ASA Block → handler NOT called ===
// "rm -f secret password" → 3 关键字匹配(rm/secret/password) → safety=0.4 → Block

#[tokio::test]
async fn test_asa_blocks_parliament_tier_handler_not_called() {
    // 自定义 ASA config:Block 阈值 0.7,safety=0.4 必然 Block
    let config = AsaConfig {
        safety_threshold_allow: 0.9,
        safety_threshold_warn: 0.7,
        safety_threshold_block: 0.7,
        ..AsaConfig::default()
    };
    let auditor = AsaAuditor::new(config);

    // 自定义 policy:允许 rm,无 blocked_patterns(否则 secret/password 会被静态分析拦截)
    let policy = CommandPolicy::new().allow_command("rm");
    let env_policy = EnvPolicy::default_secure();
    let (handler, handler_called) = approving_handler();

    let mut sandbox = Sandbox::new(policy, env_policy)
        .with_escalation_handler(handler)
        .with_asa_auditor(auditor);

    // "rm" → risk_score=80 (Parliament);content "rm -f secret password" 匹配 3 关键字
    let cmd = Command::new("rm").arg("-f").arg("secret").arg("password");
    let result = sandbox.audit_and_execute(cmd).await;

    assert!(
        matches!(result, Err(SecCoreError::AsaBlocked { .. })),
        "ASA 应阻断含多个风险关键字的 Parliament 档操作, got: {result:?}"
    );
    assert!(
        !handler_called.load(Ordering::SeqCst),
        "ASA Block 时 EscalationHandler 不应被调用"
    );
}

// === Test 2: ASA Allow → handler IS called ===
// "rm -f test" → 1 关键字匹配(rm) → safety=0.8 → Allow

#[tokio::test]
async fn test_asa_allows_parliament_tier_handler_called() {
    // 默认 ASA config:Allow 阈值 0.8,safety=0.8 → Allow
    let auditor = AsaAuditor::with_default_config();

    let policy = CommandPolicy::new().allow_command("rm");
    let env_policy = EnvPolicy::default_secure();
    let (handler, handler_called) = approving_handler();

    let mut sandbox = Sandbox::new(policy, env_policy)
        .with_escalation_handler(handler)
        .with_asa_auditor(auditor);

    // "rm -f test" → 1 关键字匹配(rm) → safety=1.0-0.2=0.8 → Allow
    let cmd = Command::new("rm").arg("-f").arg("test");
    let result = sandbox.audit_and_execute(cmd).await;

    assert!(
        handler_called.load(Ordering::SeqCst),
        "ASA Allow 时 EscalationHandler 应被调用"
    );
    assert!(
        !matches!(result, Err(SecCoreError::AsaBlocked { .. })),
        "ASA Allow 不应返回 AsaBlocked"
    );
    assert!(
        !matches!(result, Err(SecCoreError::EscalateToHuman { .. })),
        "Parliament 档不应返回 EscalateToHuman"
    );
}

// === Test 3: 无 ASA 配置 → handler 直接被调用(P1-W3.1 既有行为) ===

#[tokio::test]
async fn test_no_asa_handler_called_directly() {
    let policy = CommandPolicy::new().allow_command("rm");
    let env_policy = EnvPolicy::default_secure();
    let (handler, handler_called) = approving_handler();

    // 不调用 with_asa_auditor() → asa_auditor 为 None
    let mut sandbox = Sandbox::new(policy, env_policy).with_escalation_handler(handler);

    let cmd = Command::new("rm").arg("-f");
    let _ = sandbox.audit_and_execute(cmd).await;

    assert!(
        handler_called.load(Ordering::SeqCst),
        "未配置 ASA 时 handler 应直接被调用(回退到 P1-W3.1 行为)"
    );
}

// === Test 4: ReadOnly 档 → ASA 不触发(快速路径) ===
// "echo hello" → risk_score=10 (ReadOnly)

#[tokio::test]
async fn test_readonly_tier_asa_not_called() {
    let auditor = AsaAuditor::with_default_config();

    let mut sandbox = Sandbox::with_default_policy().with_asa_auditor(auditor);

    // "echo hello" → risk_score=10 (ReadOnly) → 不触发 ASA
    let cmd = Command::new("echo").arg("hello");
    let _ = sandbox.audit_and_execute(cmd).await;

    // ASA 历史应为空(audit_and_intervene 未被调用)
    let (total, _fail) = sandbox
        .asa_auditor()
        .expect("asa_auditor 应存在")
        .history_stats();
    assert_eq!(total, 0, "ReadOnly 档不应触发 ASA 审计");
}

// === Test 5: EscalateToHuman 档 → ASA 不触发(操作在 ASA 前被拒绝) ===
// "dd" → risk_score=95 (EscalateToHuman)

#[tokio::test]
async fn test_escalate_to_human_asa_not_called() {
    let auditor = AsaAuditor::with_default_config();

    let policy = CommandPolicy::new().allow_command("dd");
    let env_policy = EnvPolicy::default_secure();
    let (handler, _handler_called) = approving_handler();

    let mut sandbox = Sandbox::new(policy, env_policy)
        .with_escalation_handler(handler)
        .with_asa_auditor(auditor);

    // "dd" → risk_score=95 (EscalateToHuman) → 直接返回错误,不触发 ASA
    let cmd = Command::new("dd").arg("if=/dev/zero");
    let result = sandbox.audit_and_execute(cmd).await;

    assert!(
        matches!(result, Err(SecCoreError::EscalateToHuman { .. })),
        "risk_score ≥ 91 应返回 EscalateToHuman, got: {result:?}"
    );
    let (total, _fail) = sandbox
        .asa_auditor()
        .expect("asa_auditor 应存在")
        .history_stats();
    assert_eq!(total, 0, "EscalateToHuman 档不应触发 ASA 审计");
}

// === Test 6: ASA Warn → 操作继续进入 Parliament 辩论 ===
// "rm sudo test" → 2 关键字匹配(rm/sudo) → safety=0.6 → Warn

#[tokio::test]
async fn test_asa_warn_proceeds_to_debate() {
    // 默认 ASA config:Warn 阈值 0.5,Allow 阈值 0.8;safety=0.6 → Warn
    let auditor = AsaAuditor::with_default_config();

    let policy = CommandPolicy::new().allow_command("rm");
    let env_policy = EnvPolicy::default_secure();
    let (handler, handler_called) = approving_handler();

    let mut sandbox = Sandbox::new(policy, env_policy)
        .with_escalation_handler(handler)
        .with_asa_auditor(auditor);

    // "rm sudo test" → 2 关键字匹配(rm, sudo) → safety=1.0-0.4=0.6 → Warn
    let cmd = Command::new("rm").arg("sudo").arg("test");
    let result = sandbox.audit_and_execute(cmd).await;

    assert!(
        handler_called.load(Ordering::SeqCst),
        "ASA Warn 时操作应继续进入 Parliament 辩论(handler 被调用)"
    );
    assert!(
        !matches!(result, Err(SecCoreError::AsaBlocked { .. })),
        "ASA Warn 不应返回 AsaBlocked"
    );
}

// === Test 7: Normal 档 → ASA 不触发(快速路径) ===
// "echo hello > file" → risk_score=50 (Normal,因 args 含 '>')

#[tokio::test]
async fn test_normal_tier_asa_not_called() {
    let auditor = AsaAuditor::with_default_config();

    // 自定义 policy:允许 echo,无 blocked_patterns(否则 '>' 会被 Injection 拦截)
    let policy = CommandPolicy::new().allow_command("echo");
    let env_policy = EnvPolicy::default_secure();

    let mut sandbox = Sandbox::new(policy, env_policy).with_asa_auditor(auditor);

    // "echo hello > file" → args 含 '>' → risk_score=50 (Normal)
    let cmd = Command::new("echo").arg("hello").arg(">").arg("file");
    let _ = sandbox.audit_and_execute(cmd).await;

    let (total, _fail) = sandbox
        .asa_auditor()
        .expect("asa_auditor 应存在")
        .history_stats();
    assert_eq!(total, 0, "Normal 档不应触发 ASA 审计");
}

// === Test 8: 验证 EscalationTier 档位边界对齐 ===
// 确保 risk_score=71 与 90 都属于 Parliament 档(边界值)

#[tokio::test]
async fn test_parliament_tier_boundaries() {
    assert_eq!(EscalationTier::from_score(71), EscalationTier::Parliament);
    assert_eq!(EscalationTier::from_score(90), EscalationTier::Parliament);
    assert_eq!(EscalationTier::from_score(70), EscalationTier::Normal);
    assert_eq!(
        EscalationTier::from_score(91),
        EscalationTier::EscalateToHuman
    );
}
