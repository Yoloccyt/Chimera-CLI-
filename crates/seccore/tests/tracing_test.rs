//! P1-W4.1 tracing 贯穿观测测试 — 验证 seccore 三条关键路径的 span 与结构化字段
//!
//! 架构层:L4 Security(seccore)
//!
//! # 测试范围
//! 1. `test_audit_and_execute_emits_tracing_span` — audit_and_execute 顶层 span(program / tier 字段)
//! 2. `test_escalation_tier_logged` — EscalationTier 在低危路径也被记录
//! 3. `test_default_escalation_handler_tracing` — DefaultEscalationHandler 结构化 warn(tier=Parliament, decision=rejected)
//! 4. `test_asa_audit_tracing` — ASA audit() span(operation_id 字段)
//! 5. `test_asa_audit_and_intervene_block_tracing` — audit_and_intervene Block 级 intervention 字段
//!
//! # 测试方法
//! 使用 `tracing_test::traced_test` 宏自动捕获 tracing 事件。
//! WHY 属性顺序 `#[tracing_test::traced_test]` 在 `#[tokio::test]`/`#[test]` 之前:
//! proc macro 属性从上到下应用,traced_test 需先包装函数注入 mock subscriber,
//! 再由 tokio::test 包装为同步入口。若顺序反转,tokio::test 先将 async fn 转为
//! sync fn,traced_test 无法正确注入 `logs_contain` 函数。
//! API: `logs_contain("substring")` 返回 bool,检查捕获日志是否包含子串。

#![forbid(unsafe_code)]

use seccore::{
    AsaAuditor, Command, CommandSpec, DefaultEscalationHandler, EscalationHandler,
    OperationAuditInput, RiskLevel, Sandbox,
};
use std::collections::HashMap;

/// 构造测试用 OperationAuditInput — 复用 asa.rs 单元测试的工厂模式。
fn make_audit_input(content: &str, keywords: Vec<&str>, complexity: f32) -> OperationAuditInput {
    OperationAuditInput {
        operation_id: "test-op-001".to_string(),
        content: content.to_string(),
        risk_keywords: keywords.iter().map(|s| s.to_string()).collect(),
        complexity_score: complexity,
    }
}

// ============================================================
// SubTask P1-W4.1.1 路径 2:audit_and_execute 顶层 span
// ============================================================

/// 验证 `Sandbox::audit_and_execute` 发出顶层 tracing span,携带 `program` 与 `tier` 字段。
///
/// # 流程
/// 1. 构造默认策略的 Sandbox
/// 2. 执行 `whoami`(低危,ReadOnly 档)
/// 3. 验证 logs 包含 "whoami" 程序名与 "tier" 字段
///
/// # P1-W4.1 验证目标
/// - 顶层 span 的 `program` 字段来自 `command.program`(instrument 直接引用函数参数)
/// - `tier` 字段在函数内部计算后通过 `Span::current().record()` 填充
/// - 低危路径(ReadOnly/Normal)也记录 tier,与高危路径字段对齐
///
/// # Windows 兼容性
/// WHY 用 whoami 而非 echo:`echo` 在 Windows 是 cmd.exe 内建 / PowerShell 别名,
/// 无独立 .exe 可供 tokio::process::Command 直接 execve。`whoami.exe` 是 Windows
/// System32 内的独立可执行文件,即使 env_clear() 后 Windows 可执行搜索路径仍包含
/// System32,且 whoami 在默认白名单内、属只读无副作用命令(与 escalation_test.rs 一致)。
#[tracing_test::traced_test]
#[tokio::test]
async fn test_audit_and_execute_emits_tracing_span() {
    let mut sandbox = Sandbox::with_default_policy();
    let cmd = Command::new("whoami");
    let result = sandbox.audit_and_execute(cmd).await;
    assert!(result.is_ok(), "whoami 应执行成功");

    // 验证 program 字段被记录(instrument fields 直接引用 command.program)
    assert!(logs_contain("whoami"), "tracing 日志应包含 program=whoami");
    // P1-W4.1:验证 tier 字段被记录(低危路径 ReadOnly/Normal 也应记录)
    assert!(
        logs_contain("tier"),
        "tracing 日志应包含 tier 字段(低危路径也应记录)"
    );
}

/// 验证 `EscalationTier` 在 tracing 日志中被结构化记录。
///
/// # 流程
/// 1. 构造默认策略的 Sandbox
/// 2. 执行 `whoami`(低危,ReadOnly 档)
/// 3. 验证 logs 包含 tier 字段(具体档位为 ReadOnly 或 Normal)
///
/// # P1-W4.1 验证目标
/// - `tier = ?tier` 在 instrument fields 中通过 `tracing::field::debug(&tier)` 填充
/// - tier 字段是 efficiency-monitor 过滤聚合的关键维度
///
/// # Windows 兼容性
/// WHY 用 whoami 而非 echo:同 `test_audit_and_execute_emits_tracing_span` 注释。
#[tracing_test::traced_test]
#[tokio::test]
async fn test_escalation_tier_logged() {
    let mut sandbox = Sandbox::with_default_policy();
    let cmd = Command::new("whoami");
    let _ = sandbox.audit_and_execute(cmd).await;

    // 验证 tier 字段被结构化记录(无论是 ReadOnly / Normal / Parliament / EscalateToHuman)
    assert!(logs_contain("tier"), "tracing 日志应包含 tier 字段");
}

// ============================================================
// SubTask P1-W4.1.1 路径 2:DefaultEscalationHandler 结构化 warn
// ============================================================

/// 验证 `DefaultEscalationHandler::parliament_debate` 发出结构化 warn 日志,
/// 携带 `tier=Parliament` 与 `decision=rejected` 字段。
///
/// # 流程
/// 1. 构造 DefaultEscalationHandler(未配置实际 Parliament 实现)
/// 2. 构造 risk_score=80 的 CommandSpec(Parliament 档)
/// 3. 调用 parliament_debate(应返回 Err)
/// 4. 验证 logs 包含 tier=Parliament、decision=rejected、reason=default_handler_unconfigured
///
/// # P1-W4.1 验证目标
/// - span fields 中 tier 固定为 "Parliament"(本方法仅在 Parliament 档位被调用)
/// - decision 固定为 "rejected"(默认 handler 永远拒绝)
/// - 结构化 warn 日志携带 reason 字段,供运维判断是否需要补配置真实 handler
#[tracing_test::traced_test]
#[test]
fn test_default_escalation_handler_tracing() {
    let handler = DefaultEscalationHandler;
    let spec = CommandSpec {
        program: "rm".to_string(),
        allowed_args: vec!["-rf".to_string()],
        env_whitelist: HashMap::new(),
        risk_level: RiskLevel::High,
        risk_score: 80,
    };
    let result = handler.parliament_debate(&spec, 80);
    assert!(
        result.is_err(),
        "DefaultEscalationHandler 应拒绝 Parliament 档操作"
    );

    // P1-W4.1:验证结构化字段
    assert!(
        logs_contain("Parliament"),
        "tracing 日志应包含 tier=Parliament"
    );
    assert!(
        logs_contain("rejected"),
        "tracing 日志应包含 decision=rejected"
    );
    assert!(
        logs_contain("default_handler_unconfigured"),
        "tracing 日志应包含 reason=default_handler_unconfigured"
    );
}

// ============================================================
// SubTask P1-W4.1.1 路径 3:ASA 审计 tracing
// ============================================================

/// 验证 `AsaAuditor::audit` 发出 tracing span,携带 `operation_id` 字段。
///
/// # 流程
/// 1. 构造默认配置的 AsaAuditor
/// 2. 构造 OperationAuditInput(operation_id = "test-op-001")
/// 3. 调用 audit()
/// 4. 验证 logs 包含 operation_id 字段值
///
/// # P1-W4.1 验证目标
/// - span 的 `operation_id` 字段来自 `input.operation_id`(instrument 直接引用)
/// - `safety_score` / `intervention` / `risk_level` 在函数内部计算后填充
/// - operation_id 是审计链 record_id 的关联键,供 efficiency-monitor 跨日志关联
#[tracing_test::traced_test]
#[test]
fn test_asa_audit_tracing() {
    let auditor = AsaAuditor::with_default_config();
    let input = make_audit_input("echo hello", vec![], 0.1);
    let result = auditor.audit(&input);
    assert_eq!(result.intervention, seccore::InterventionAction::Allow);

    // 验证 operation_id 字段被记录
    assert!(
        logs_contain("test-op-001"),
        "tracing 日志应包含 operation_id=test-op-001"
    );
}

/// 验证 `AsaAuditor::audit_and_intervene` 在 Block 级别发出 tracing error 日志,
/// 携带 `intervention=Block` 字段。
///
/// # 流程
/// 1. 构造默认配置的 AsaAuditor
/// 2. 构造含 3 个风险关键字的 input(safety_score = 0.4 → Block)
/// 3. 调用 audit_and_intervene(应返回 Err(AsaBlocked))
/// 4. 验证 logs 包含 intervention=Block 与 "ASA 拦截" 消息
///
/// # P1-W4.1 验证目标
/// - `intervention` 字段在 audit() 返回后通过 `Span::current().record()` 填充
/// - Block 级别用 tracing::error! 记录(对应安全红线 §6.2)
/// - intervention 字段供 efficiency-monitor 过滤 ASA 拦截事件
#[tracing_test::traced_test]
#[test]
fn test_asa_audit_and_intervene_block_tracing() {
    let auditor = AsaAuditor::with_default_config();
    // 3 个关键字:safety_score = 1.0 - 0.2*3 = 0.4 → Block
    let input = make_audit_input("sudo rm secret", vec!["sudo", "rm", "secret"], 0.0);
    let result = auditor.audit_and_intervene(&input);
    assert!(result.is_err(), "3 个风险关键字应触发 Block 级干预");

    // 验证 intervention=Block 字段被记录
    assert!(
        logs_contain("Block"),
        "tracing 日志应包含 intervention=Block"
    );
    assert!(
        logs_contain("ASA 拦截"),
        "tracing 日志应包含 'ASA 拦截' 消息"
    );
}

/// 验证 `AsaAuditor::audit_and_intervene` 在 Warn 级别发出 tracing warn 日志,
/// 携带 `intervention=Warn` 字段。
///
/// # 流程
/// 1. 构造默认配置的 AsaAuditor
/// 2. 构造含 2 个风险关键字的 input(safety_score = 0.6 → Warn)
/// 3. 调用 audit_and_intervene(应返回 Ok)
/// 4. 验证 logs 包含 intervention=Warn 与 "ASA 告警" 消息
#[tracing_test::traced_test]
#[test]
fn test_asa_audit_and_intervene_warn_tracing() {
    let auditor = AsaAuditor::with_default_config();
    // 2 个关键字:safety_score = 1.0 - 0.2*2 = 0.6 → Warn
    let input = make_audit_input("sudo rm", vec!["sudo", "rm"], 0.0);
    let result = auditor.audit_and_intervene(&input);
    assert!(result.is_ok(), "2 个风险关键字应触发 Warn 级告警(继续执行)");

    assert!(logs_contain("Warn"), "tracing 日志应包含 intervention=Warn");
    assert!(
        logs_contain("ASA 告警"),
        "tracing 日志应包含 'ASA 告警' 消息"
    );
}
