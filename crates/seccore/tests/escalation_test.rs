//! 高危操作强制升级通道 TDD 测试 — P1-W3.1 (D6 修复)
//!
//! 对应文档:
//! - spec.md §Scenario "高危操作强制升级通道" (L199-206)
//! - tasks.md P1-W3.1 (SubTask P1-W3.1.1 / .2 / .3)
//!
//! 测试覆盖 5 档:
//! 1. ReadOnly (0-30): 直接执行
//! 2. Normal (31-70): 直接执行
//! 3. Parliament (71-90): 强制 Parliament 辩论 + 自白通道复核
//! 4. EscalateToHuman (91-100): 拒绝执行,返回 EscalateToHuman 错误
//! 5. 边界值 (0/30/31/70/71/90/91/100): 档位分类正确性
//!
//! TDD 流程:本文件先写(RED),实现升级通道后转 GREEN。

use seccore::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

// =============================================================================
// Part 1: EscalationTier::from_score 单元测试 (边界值覆盖)
// =============================================================================
// 验证 0-100 风险评分到 4 级档位的映射,重点测试边界值正确性。

#[test]
fn test_boundary_score_0_is_readonly() {
    assert_eq!(EscalationTier::from_score(0), EscalationTier::ReadOnly);
}

#[test]
fn test_boundary_score_30_is_readonly() {
    assert_eq!(EscalationTier::from_score(30), EscalationTier::ReadOnly);
}

#[test]
fn test_boundary_score_31_is_normal() {
    assert_eq!(EscalationTier::from_score(31), EscalationTier::Normal);
}

#[test]
fn test_boundary_score_70_is_normal() {
    assert_eq!(EscalationTier::from_score(70), EscalationTier::Normal);
}

#[test]
fn test_boundary_score_71_is_parliament() {
    assert_eq!(EscalationTier::from_score(71), EscalationTier::Parliament);
}

#[test]
fn test_boundary_score_90_is_parliament() {
    assert_eq!(EscalationTier::from_score(90), EscalationTier::Parliament);
}

#[test]
fn test_boundary_score_91_is_escalate_to_human() {
    assert_eq!(
        EscalationTier::from_score(91),
        EscalationTier::EscalateToHuman
    );
}

#[test]
fn test_boundary_score_100_is_escalate_to_human() {
    assert_eq!(
        EscalationTier::from_score(100),
        EscalationTier::EscalateToHuman
    );
}

// 防御性:超过 100 的评分(理论不应出现)归入 EscalateToHuman,避免漏过
#[test]
fn test_oversized_score_clamps_to_escalate_to_human() {
    assert_eq!(
        EscalationTier::from_score(255),
        EscalationTier::EscalateToHuman
    );
}

// =============================================================================
// Part 2: RiskLevel::from_score 派生映射测试
// =============================================================================
// 验证旧 RiskLevel 枚举可从数值评分派生,保持向后兼容。

#[test]
fn test_risk_level_from_score_low() {
    assert_eq!(RiskLevel::from_score(10), RiskLevel::Low);
    assert_eq!(RiskLevel::from_score(30), RiskLevel::Low);
}

#[test]
fn test_risk_level_from_score_medium() {
    assert_eq!(RiskLevel::from_score(50), RiskLevel::Medium);
    assert_eq!(RiskLevel::from_score(70), RiskLevel::Medium);
}

#[test]
fn test_risk_level_from_score_high() {
    assert_eq!(RiskLevel::from_score(80), RiskLevel::High);
    assert_eq!(RiskLevel::from_score(90), RiskLevel::High);
}

#[test]
fn test_risk_level_from_score_critical() {
    assert_eq!(RiskLevel::from_score(95), RiskLevel::Critical);
    assert_eq!(RiskLevel::from_score(100), RiskLevel::Critical);
}

// =============================================================================
// Part 3: Mock 升级处理器 — 用于 Sandbox 集成测试
// =============================================================================

/// 批准型 handler — Parliament 辩论通过
struct ApprovingHandler;
impl EscalationHandler for ApprovingHandler {
    fn parliament_debate(&self, _spec: &CommandSpec, _risk_score: u8) -> Result<(), SecCoreError> {
        Ok(())
    }
}

/// 拒绝型 handler — Parliament 辩论否决
struct RejectingHandler;
impl EscalationHandler for RejectingHandler {
    fn parliament_debate(&self, spec: &CommandSpec, risk_score: u8) -> Result<(), SecCoreError> {
        Err(SecCoreError::PolicyViolation(format!(
            "Parliament 否决高危操作 (程序: {}, risk_score={})",
            spec.program, risk_score
        )))
    }
}

/// 记录调用型 handler — 验证 handler 是否被调用
struct RecordingHandler {
    called: Arc<AtomicBool>,
}
impl EscalationHandler for RecordingHandler {
    fn parliament_debate(&self, _spec: &CommandSpec, _risk_score: u8) -> Result<(), SecCoreError> {
        self.called.store(true, Ordering::SeqCst);
        Ok(())
    }
}

// =============================================================================
// Part 4: Sandbox 集成测试 — 5 档执行路径
// =============================================================================

// Tier 1: ReadOnly (0-30) — whoami, risk_score=10,直接执行
//
// WHY whoami 而非 echo: `echo` 在 Windows 是 cmd.exe 内建 / PowerShell 别名,
// 无独立 .exe 可供 tokio::process::Command 直接 execve。`whoami.exe` 是 Windows
// System32 内的独立可执行文件,即使 env_clear() 后 Windows 可执行搜索路径仍包含
// System32,且 whoami 在默认白名单内、属只读无副作用命令。
#[tokio::test]
async fn test_readonly_tier_direct_execution() {
    let mut sandbox = Sandbox::with_default_policy();
    let cmd = Command::new("whoami");
    let result = sandbox.audit_and_execute(cmd).await;
    assert!(result.is_ok(), "ReadOnly tier should execute directly");
    assert_eq!(result.unwrap().exit_code, 0);
}

// Tier 2: Normal (31-70) — whoami 含 `>` 参数,risk_score=50,直接执行
//
// WHY: `>` 出现在 args 中触发 assess_risk 返回 50 (Normal tier)。
// whoami 收到未知参数会打印错误并返回非零退出码,但进程本身成功启动并执行,
// 沙箱返回 Ok(ExecutionResult { exit_code: 非0, ... })。
// 关键断言:升级通道未拦截(NOT EscalateToHuman / NOT PolicyViolation)。
#[tokio::test]
async fn test_normal_tier_direct_execution() {
    let mut sandbox = Sandbox::with_default_policy();
    // args 含 `>` 触发 risk_score=50 (Normal tier)
    let cmd = Command::new("whoami").arg("hello>");
    let result = sandbox.audit_and_execute(cmd).await;
    // 进程成功启动(即使退出码非 0),沙箱返回 Ok
    assert!(result.is_ok(), "Normal tier should execute directly");
    // 升级通道未拦截
    assert!(
        !matches!(result, Err(SecCoreError::EscalateToHuman { .. })),
        "Normal tier must not return EscalateToHuman"
    );
}

// Tier 3: Parliament (71-90) — rm with ApprovingHandler,执行路径继续
#[tokio::test]
async fn test_parliament_tier_approved_proceeds_to_execution() {
    // 自定义策略允许 rm(不在默认白名单)
    let policy = CommandPolicy::new().allow_command("rm");
    let env_policy = EnvPolicy::default_secure();
    let mut sandbox =
        Sandbox::new(policy, env_policy).with_escalation_handler(Box::new(ApprovingHandler));
    // rm 映射到 risk_score=80 (Parliament tier)
    let cmd = Command::new("rm").arg("-f");
    let result = sandbox.audit_and_execute(cmd).await;
    // 执行可能失败(rm -f 无文件 / rm 在 Windows 不存在),
    // 但关键断言:不应返回 EscalateToHuman 或 handler 拒绝(PolicyViolation)
    assert!(
        !matches!(result, Err(SecCoreError::EscalateToHuman { .. })),
        "Parliament 批准的操作不应返回 EscalateToHuman"
    );
    assert!(
        !matches!(result, Err(SecCoreError::PolicyViolation(_))),
        "Parliament 批准的操作不应返回 PolicyViolation (handler 拒绝)"
    );
}

// Tier 3: Parliament (71-90) — rm with RejectingHandler,执行被拦截
#[tokio::test]
async fn test_parliament_tier_rejected_blocks_execution() {
    let policy = CommandPolicy::new().allow_command("rm");
    let env_policy = EnvPolicy::default_secure();
    let mut sandbox =
        Sandbox::new(policy, env_policy).with_escalation_handler(Box::new(RejectingHandler));
    let cmd = Command::new("rm").arg("-f");
    let result = sandbox.audit_and_execute(cmd).await;
    assert!(
        matches!(result, Err(SecCoreError::PolicyViolation(_))),
        "Parliament 否决的操作应返回 PolicyViolation"
    );
}

// Tier 3: Parliament — handler 被调用
#[tokio::test]
async fn test_parliament_tier_handler_is_called() {
    let policy = CommandPolicy::new().allow_command("rm");
    let env_policy = EnvPolicy::default_secure();
    let called = Arc::new(AtomicBool::new(false));
    let handler = RecordingHandler {
        called: called.clone(),
    };
    let mut sandbox = Sandbox::new(policy, env_policy).with_escalation_handler(Box::new(handler));
    let cmd = Command::new("rm").arg("-f");
    let _ = sandbox.audit_and_execute(cmd).await;
    assert!(
        called.load(Ordering::SeqCst),
        "Parliament tier 必须调用 EscalationHandler"
    );
}

// Tier 4: EscalateToHuman (91-100) — dd 映射到 risk_score=95
#[tokio::test]
async fn test_escalate_to_human_tier_returns_error() {
    let policy = CommandPolicy::new().allow_command("dd");
    let env_policy = EnvPolicy::default_secure();
    let mut sandbox =
        Sandbox::new(policy, env_policy).with_escalation_handler(Box::new(ApprovingHandler));
    let cmd = Command::new("dd").arg("if=/dev/zero");
    let result = sandbox.audit_and_execute(cmd).await;
    assert!(
        matches!(result, Err(SecCoreError::EscalateToHuman { risk_score, .. }) if risk_score >= 91),
        "EscalateToHuman 档位必须返回 EscalateToHuman 错误且 risk_score ≥ 91"
    );
}

// Tier 4: EscalateToHuman — handler 不应被调用(错误在 handler 之前返回)
#[tokio::test]
async fn test_escalate_to_human_tier_handler_not_called() {
    let policy = CommandPolicy::new().allow_command("dd");
    let env_policy = EnvPolicy::default_secure();
    let called = Arc::new(AtomicBool::new(false));
    let handler = RecordingHandler {
        called: called.clone(),
    };
    let mut sandbox = Sandbox::new(policy, env_policy).with_escalation_handler(Box::new(handler));
    let cmd = Command::new("dd").arg("if=/dev/zero");
    let _ = sandbox.audit_and_execute(cmd).await;
    assert!(
        !called.load(Ordering::SeqCst),
        "EscalateToHuman 档位不应调用 handler (错误在 handler 之前返回)"
    );
}

// =============================================================================
// Part 5: 默认 handler 安全契约测试
// =============================================================================
// 未配置实际 handler 时,DefaultEscalationHandler 必须拒绝 Parliament 档操作,
// 防止"忘记配置 handler"导致高危操作静默执行的安全漏洞。

#[tokio::test]
async fn test_default_handler_rejects_parliament_tier() {
    let policy = CommandPolicy::new().allow_command("rm");
    let env_policy = EnvPolicy::default_secure();
    // 不调用 with_escalation_handler → 使用 DefaultEscalationHandler
    let mut sandbox = Sandbox::new(policy, env_policy);
    let cmd = Command::new("rm").arg("-f");
    let result = sandbox.audit_and_execute(cmd).await;
    assert!(
        matches!(result, Err(SecCoreError::PolicyViolation(_))),
        "Default handler 应拒绝 Parliament 档操作 (未配置 handler)"
    );
}

// =============================================================================
// Part 6: risk_score 字段填充测试
// =============================================================================
// 验证 validate_command 正确填充 risk_score 字段。

#[test]
fn test_risk_score_populated_in_spec() {
    let policy = CommandPolicy::default_secure();
    let cmd = Command::new("echo").arg("hello");
    let spec = validate_command(&cmd, &policy).expect("echo 应通过校验");
    assert_eq!(
        spec.risk_score, 10,
        "echo 应映射到 risk_score=10 (ReadOnly)"
    );
    assert_eq!(spec.risk_level, RiskLevel::Low);
}

#[test]
fn test_risk_score_redirect_arg_is_normal() {
    let policy = CommandPolicy::default_secure();
    // args 含 `>` → risk_score=50 (Normal)
    let cmd = Command::new("echo").arg("hello>");
    let spec = validate_command(&cmd, &policy).expect("echo 应通过校验");
    assert_eq!(spec.risk_score, 50);
    assert_eq!(spec.risk_level, RiskLevel::Medium);
}

#[test]
fn test_risk_score_wildcard_arg_is_normal() {
    let policy = CommandPolicy::default_secure();
    // args 含 `*` → risk_score=40 (Normal)
    let cmd = Command::new("echo").arg("*");
    let spec = validate_command(&cmd, &policy).expect("echo 应通过校验");
    assert_eq!(spec.risk_score, 40);
    assert_eq!(spec.risk_level, RiskLevel::Medium);
}
