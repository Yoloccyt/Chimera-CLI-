//! 高危操作升级处理器 — 处理 Parliament 辩论与人工升级的抽象接口。
//!
//! 对应架构层:L4 Security
//! 对应文档:spec.md §Scenario "高危操作强制升级通道" (D6 修复)
//!
//! WHY trait 而非硬依赖 parliament crate:
//! seccore 位于 L4 (Security),parliament 位于 L8。依赖铁律(§2.2)禁止 L4 → L8。
//! 通过 trait 注入,上层 (chimera-cli / quest-engine at L9/L10) 注入实际
//! Parliament 实现,seccore 仅定义契约。这是依赖倒置原则(DIP)的应用:
//! 高层策略(L8 Parliament)实现低层(L4 seccore)定义的抽象 trait。

use crate::error::SecCoreError;
use crate::types::CommandSpec;

/// 升级处理器 — 处理高危操作 (`risk_score ∈ [71,90]`) 的 Parliament 辩论 + 自白通道复核。
///
/// 实现者负责:
/// 1. 发起 Parliament 完整辩论(提案 → 辩论 → 投票)
/// 2. 自白通道复核(操作意图披露 + 风险确认)
/// 3. 返回辩论结果(批准 / 否决)
///
/// # 契约
/// - 仅在 `EscalationTier::Parliament` 档位(risk_score ∈ [71,90])被调用
/// - `EscalateToHuman` 档位(risk_score ∈ [91,100])在调用 handler 之前直接返回错误,
///   不会调用此 trait
/// - 实现必须线程安全(`Send + Sync`),因 `Sandbox` 可能在异步任务间共享
pub trait EscalationHandler: Send + Sync {
    /// 处理高危操作的 Parliament 辩论 + 自白通道复核。
    ///
    /// # 参数
    /// - `spec`:校验通过的命令规格(含 `risk_score`)
    /// - `risk_score`:风险评分 (0-100,调用时保证 ∈ [71,90])
    ///
    /// # 返回
    /// - `Ok(())`:Parliament 批准执行,调用方继续沙箱执行流程
    /// - `Err`:Parliament 否决或自白通道复核失败,操作被拦截
    fn parliament_debate(&self, spec: &CommandSpec, risk_score: u8) -> Result<(), SecCoreError>;
}

/// 默认升级处理器 — 未配置实际 Parliament 时的占位实现。
///
/// WHY: 对 Parliament 档位操作返回 `Err`,强制调用方配置真实 handler。
/// 这确保没有"忘记配置 handler"导致高危操作静默执行的安全漏洞。
///
/// 安全契约:`DefaultEscalationHandler` 永远不会批准 Parliament 档操作。
/// 调用方必须通过 `Sandbox::with_escalation_handler()` 注入实际 Parliament 实现,
/// 否则所有 `risk_score ∈ [71,90]` 的操作都会被拦截。
pub struct DefaultEscalationHandler;

impl EscalationHandler for DefaultEscalationHandler {
    /// # P1-W4.1 tracing 贯穿观测
    ///
    /// span 携带 `program` / `risk_score` / `tier` / `decision` 字段,与
    /// `Sandbox::audit_and_execute` 顶层 span 字段对齐。`tier` 固定为
    /// `"Parliament"`(本方法仅在 Parliament 档位被调用),`decision` 固定为
    /// `"rejected"`(默认 handler 永远拒绝)。这样 efficiency-monitor 可通过
    /// `tier=Parliament AND decision=rejected` 过滤出所有未配置 handler 的
    /// 高危操作拦截事件,运维据此判断是否需要补配置真实 Parliament 实现。
    #[tracing::instrument(
        skip(self),
        fields(
            program = %spec.program,
            risk_score = risk_score,
            tier = "Parliament",
            decision = "rejected"
        )
    )]
    fn parliament_debate(&self, spec: &CommandSpec, risk_score: u8) -> Result<(), SecCoreError> {
        // P1-W4.1: 结构化 warn 日志,携带 decision_chain_id 占位(由调用方 sandbox
        // 在 record_id 获得后填充顶层 span;此处 warn 用于审计拒绝原因)
        tracing::warn!(
            program = %spec.program,
            risk_score = risk_score,
            tier = "Parliament",
            decision = "rejected",
            reason = "default_handler_unconfigured",
            "DefaultEscalationHandler 拒绝 Parliament 档操作(未配置升级处理器)"
        );
        Err(SecCoreError::PolicyViolation(format!(
            "高危操作 (risk_score={risk_score}) 需要升级处理器但未配置 \
             (程序: {})。请通过 Sandbox::with_escalation_handler() 注入 Parliament 实现",
            spec.program
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::RiskLevel;
    use std::collections::HashMap;

    fn make_spec(program: &str, risk_score: u8) -> CommandSpec {
        CommandSpec {
            program: program.to_string(),
            allowed_args: Vec::new(),
            env_whitelist: HashMap::new(),
            risk_level: RiskLevel::from_score(risk_score),
            risk_score,
        }
    }

    #[test]
    fn test_default_handler_rejects_parliament_tier() {
        let handler = DefaultEscalationHandler;
        let spec = make_spec("rm", 80);
        let result = handler.parliament_debate(&spec, 80);
        assert!(
            matches!(result, Err(SecCoreError::PolicyViolation(_))),
            "Default handler 必须拒绝 Parliament 档操作"
        );
    }

    #[test]
    fn test_default_handler_error_contains_program_name() {
        let handler = DefaultEscalationHandler;
        let spec = make_spec("rm", 80);
        let err = handler
            .parliament_debate(&spec, 80)
            .expect_err("应返回错误");
        let msg = match err {
            SecCoreError::PolicyViolation(msg) => msg,
            _ => panic!("应为 PolicyViolation"),
        };
        assert!(msg.contains("rm"), "错误消息应包含程序名 'rm': {msg}");
        assert!(
            msg.contains("with_escalation_handler"),
            "错误消息应提示注入 handler 的方法: {msg}"
        );
    }
}
