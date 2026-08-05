//! SecCore 的 L0 `CommandValidator` trait 实现 — AHIRT 注入载体(ADR-054 决策 3,P9-T4)
//!
//! 对应架构层: L4 Security
//! 对应 ADR: **ADR-054 决策 3**(MemoryStrategyProvider 先例:L0 trait 解耦)
//!
//! # 核心职责
//!
//! 将 L0 `nexus_contracts::command_validation::CommandValidator` trait 的校验请求
//! 转发到 seccore 既有 `validate_command`(静态分析逻辑零改动,仅做错误类型适配):
//! - `SecCoreError::CommandBlocked` → L0 `CommandValidationError`(攻击类型 + 详情无损映射)
//! - 其余 `SecCoreError` 分支 → `AttackType::Abuse` 兜底(理论不可达,
//!   防御性保证 trait 返回类型无泄漏)
//!
//! # WHY 空结构体
//!
//! `validate_command` 为纯函数(无状态),实现无需携带任何字段;
//! unit struct 可 `Arc::new(SecCoreCommandValidator)` 直接构造,零配置。

use nexus_contracts::command_validation::{
    AttackType, Command, CommandPolicy, CommandValidationError, CommandValidator,
};

use crate::error::SecCoreError;
use crate::policy::validate_command;

/// SecCore 的 `CommandValidator` 实现 — 包装 `validate_command`,供 L8 parliament 注入。
#[derive(Debug, Clone, Copy, Default)]
pub struct SecCoreCommandValidator;

impl CommandValidator for SecCoreCommandValidator {
    fn validate(
        &self,
        cmd: &Command,
        policy: &CommandPolicy,
    ) -> Result<(), CommandValidationError> {
        match validate_command(cmd, policy) {
            Ok(_) => Ok(()),
            // 拦截主路径:攻击类型 + 详情无损映射到 L0 错误载体
            Err(SecCoreError::CommandBlocked {
                attack_type,
                detail,
            }) => Err(CommandValidationError {
                attack_type,
                detail,
            }),
            // 兜底分支:validate_command 目前仅产生 CommandBlocked(审查 policy.rs 确认),
            // 其余变体防御性映射为 Abuse,保证 trait 契约无泄漏。
            Err(e) => Err(CommandValidationError {
                attack_type: AttackType::Abuse,
                detail: e.to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 默认安全策略 + 空结构验证器(测试辅助)
    fn setup() -> (SecCoreCommandValidator, CommandPolicy) {
        (SecCoreCommandValidator, CommandPolicy::default_secure())
    }

    /// 危险载荷拦截: `$(whoami)` 应返回 Err(CommandValidationError) 且攻击类型为 Injection
    ///
    /// WHY 用 `$(whoami)`:AHIRT 命令注入载荷库首个载荷,匹配 `$(` 拦截模式,
    /// 与 parliament `probe_command` 的注入判定路径一致。
    #[test]
    fn test_validator_blocks_dangerous_payload() {
        let (validator, policy) = setup();
        let cmd = Command::new("$(whoami)");
        match validator.validate(&cmd, &policy) {
            Ok(()) => panic!("危险载荷 $(whoami) 应被拦截"),
            Err(e) => {
                assert_eq!(
                    e.attack_type,
                    AttackType::Injection,
                    "应判定为 Injection,实际: {:?}",
                    e.attack_type
                );
                assert!(!e.detail.is_empty(), "拦截详情不应为空");
            }
        }
    }

    /// 白名单放行: `echo` 应返回 Ok
    #[test]
    fn test_validator_allows_whitelisted_command() {
        let (validator, policy) = setup();
        let cmd = Command::new("echo").arg("hello");
        let result = validator.validate(&cmd, &policy);
        assert!(result.is_ok(), "白名单命令 echo 应放行,实际: {result:?}");
    }

    /// 与直接 validate_command 判定一致: 同一命令/策略下,trait 与底层判定同步
    #[test]
    fn test_validator_matches_validate_command_judgment() {
        let (validator, policy) = setup();
        for payload in [
            "$(whoami)",
            "cat /etc/shadow",
            "sudo ls",
            "../etc/passwd",
            "echo hi",
        ] {
            let cmd = Command::new(payload);
            let via_trait = validator.validate(&cmd, &policy).is_ok();
            let via_direct = validate_command(&cmd, &policy).is_ok();
            assert_eq!(
                via_trait, via_direct,
                "载荷 '{payload}' 的 trait 判定与 validate_command 不一致"
            );
        }
    }
}
