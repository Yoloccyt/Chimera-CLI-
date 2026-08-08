//! R2 解冻就绪检查器（Milestone C-5 前置，ADR-053 rev4 阶段③ 自动化载体）
//!
//! 对应方案: `CHIMERA_V3_专项优化方案_v2.21基线.md` §6 C-5
//! 对应 ADR: ADR-042（R2 冻结）/ ADR-043（影子模式 2 周）/ ADR-047（FormalVerifier）/
//! ADR-053 rev4（阶段③ 解冻程序）/ ADR-054（治理签署）
//!
//! # 职责
//!
//! 将解冻五要素组合为**单一可执行就绪判定**（供编排器/CLI 在解冻决策点调用）：
//! 1. 熔断器 Armed（decay-engine ShadowModeCircuitBreaker，未跳闸）
//! 2. 解冻范围 Allowed（gsoe UnfreezeGovernor，fail-closed 白名单含目标）
//! 3. 阶段③ 前置齐备（chimera-mas Stage3Prerequisites，ADR-053 rev4）
//! 4. 影子期 ≥ 14 天（ADR-043 硬约束）
//! 5. r2_freeze_violation_e2e 绿（治理批准移除前的最后验证）
//! 6. FormalVerifier 全绿（M0/M1/M2 属性 #1-7）
//!
//! # R2 冻结声明（ADR-042）
//!
//! 本模块**只读状态 + 组合判定**，不执行任何 RL 训练/梯度更新；
//! 是解冻前的就绪检查基建，本身不解冻。
//!
//! # 依赖铁律
//!
//! L9 chimera-mas → L5 gsoe-evolution / L4 decay-engine（向下依赖合规，§2.2）。

use decay_engine::shadow_breaker::{BreakerState, ShadowModeCircuitBreaker};
use gsoe_evolution::unfreeze_governor::{UnfreezeDecision, UnfreezeGovernor};
use gsoe_evolution::unfreeze_scope::RlUpdateTarget;

use crate::shadow::orchestrator::Stage3Prerequisites;

/// 影子期硬约束（ADR-043：影子模式 2 周）
pub const SHADOW_PERIOD_DAYS_REQUIRED: u64 = 14;

/// 就绪检查外部观测输入（运行时状态快照）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct R2UnfreezeReadinessInput {
    /// 影子期已观察天数（≥ 14 满足 ADR-043）
    pub shadow_period_days: u64,
    /// r2_freeze_violation_e2e 是否绿（治理批准移除前的最后验证）
    pub r2_freeze_e2e_passed: bool,
    /// FormalVerifier 属性全绿（M0/M1/M2 属性 #1-7）
    pub formal_verifier_all_green: bool,
}

/// 就绪检查器 — 五要素组合判定
#[derive(Debug, Clone)]
pub struct R2UnfreezeReadiness {
    /// 熔断器（decay-engine；Tripped = fail-closed 永久拒绝）
    pub breaker: ShadowModeCircuitBreaker,
    /// 解冻范围治理器（gsoe；fail-closed 白名单）
    pub governor: UnfreezeGovernor,
    /// ADR-053 rev4 阶段③ 前置（chimera-mas）
    pub stage3: Stage3Prerequisites,
}

/// 就绪报告
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReadinessReport {
    /// 全部要素满足
    pub ready: bool,
    /// 未满足要素清单（人类可读，供审计/终判拒绝原因）
    pub missing: Vec<String>,
}

impl R2UnfreezeReadiness {
    /// 装配就绪检查器
    pub fn new(
        breaker: ShadowModeCircuitBreaker,
        governor: UnfreezeGovernor,
        stage3: Stage3Prerequisites,
    ) -> Self {
        Self {
            breaker,
            governor,
            stage3,
        }
    }

    /// 综合就绪判定 — 六要素全满足才 ready（fail-closed）
    pub fn evaluate(&self, input: &R2UnfreezeReadinessInput) -> ReadinessReport {
        let mut missing: Vec<String> = Vec::new();

        // 1. 熔断器 Armed（未跳闸）
        if self.breaker.state() != BreakerState::Armed {
            missing.push("熔断器跳闸（fail-closed：需人工复位）".to_string());
        }

        // 2. 解冻范围：至少一个目标决策 Allowed
        let scope_ok = [
            RlUpdateTarget::GsoeVariantSelection,
            RlUpdateTarget::AutoDpoPreference,
            RlUpdateTarget::SeamPolicy(1),
            RlUpdateTarget::SeamPolicy(2),
            RlUpdateTarget::SeamPolicy(3),
            RlUpdateTarget::SeamPolicy(4),
            RlUpdateTarget::SeamPolicy(5),
            RlUpdateTarget::SeamPolicy(6),
            RlUpdateTarget::SeamPolicy(7),
            RlUpdateTarget::SeamPolicy(8),
        ]
        .into_iter()
        .any(|t| {
            let mut governor = self.governor.clone();
            matches!(governor.decide(&t, true), UnfreezeDecision::Allowed)
        });
        if !scope_ok {
            missing.push("解冻范围全冻结（UnfreezeScope 白名单未纳入任何目标）".to_string());
        }

        // 3. 阶段③ 前置齐备（ADR-053 rev4）
        let stage3_missing = self.stage3.missing();
        if !stage3_missing.is_empty() {
            missing.push(format!(
                "阶段③ 前置缺失 {} 项: {}",
                stage3_missing.len(),
                stage3_missing.join(" / ")
            ));
        }

        // 4. 影子期 ≥ 2 周（ADR-043）
        if input.shadow_period_days < SHADOW_PERIOD_DAYS_REQUIRED {
            missing.push(format!(
                "影子期不足（{}/{} 天，ADR-043 需 2 周）",
                input.shadow_period_days, SHADOW_PERIOD_DAYS_REQUIRED
            ));
        }

        // 5. R2 冻结 E2E 绿
        if !input.r2_freeze_e2e_passed {
            missing.push("r2_freeze_violation_e2e 未通过（治理批准移除前必须绿）".to_string());
        }

        // 6. FormalVerifier 全绿（ADR-047）
        if !input.formal_verifier_all_green {
            missing.push("FormalVerifier 属性未全绿（M0/M1/M2 属性 #1-7）".to_string());
        }

        ReadinessReport {
            ready: missing.is_empty(),
            missing,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_assembly_has_all_elements() {
        let readiness = R2UnfreezeReadiness::new(
            ShadowModeCircuitBreaker::new(),
            UnfreezeGovernor::new(
                gsoe_evolution::unfreeze_scope::UnfreezeScope::frozen()
                    .with_target(RlUpdateTarget::GsoeVariantSelection),
            ),
            Stage3Prerequisites { alpha_composite_calibrated: true, power_intra_batch_verified: true, binomial_sf_comments_corrected: true, payload_rotation_ready: true, coverage_instrumentation_ready: true, s_min_final_confirmed: true },
        );
        let report = readiness.evaluate(&R2UnfreezeReadinessInput {
            shadow_period_days: 14,
            r2_freeze_e2e_passed: true,
            formal_verifier_all_green: true,
        });
        assert!(report.ready);
    }

    #[test]
    fn shadow_period_constant_is_two_weeks() {
        assert_eq!(SHADOW_PERIOD_DAYS_REQUIRED, 14);
    }
}
