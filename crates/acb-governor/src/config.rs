//! ACB 配置类型 — 各级预算上限与调整阈值
//!
//! 对应架构层:L8 Parliament
//! 对应创新点:ACB(Adaptive Cognitive Budget)
//!
//! # 设计决策(WHY)
//! - 各级预算上限递增(L0 < L1 < L2 < L3):级别越高,单次请求允许的消耗越大,
//!   validate() 校验此不变量,防止配置倒挂导致级别判定异常
//! - `degrade_threshold` > `upgrade_threshold`:降级阈值高于升级阈值,形成滞后带,
//!   避免在阈值附近频繁切换(抖动)(§6 架构红线:避免短视)
//! - 默认值:L0=1000 / L1=5000 / L2=20000 / L3=100000 Token,适配典型 Quest 规模

use serde::{Deserialize, Serialize};

use crate::error::AcbError;
use crate::types::{BudgetAllocation, BudgetTier};

/// ACB 治理器配置
///
/// 所有字段均有合理默认值,可通过 `Default::default()` 快速构造。
/// 构造 `AcbGovernor` 时会调用 `validate()` 校验配置合法性。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AcbGovernorConfig {
    /// L0 降级级别单次请求 Token 上限(最低)
    pub l0_token_limit: u64,
    /// L1 基础级别单次请求 Token 上限
    pub l1_token_limit: u64,
    /// L2 标准级别单次请求 Token 上限
    pub l2_token_limit: u64,
    /// L3 充足级别单次请求 Token 上限(最高)
    pub l3_token_limit: u64,
    /// 总预算上限(Token),所有请求累计消耗的上限
    pub total_budget_limit: u64,
    /// 降级阈值 [0.0, 1.0]:利用率超过此值触发自动降级
    pub degrade_threshold: f32,
    /// 升级阈值 [0.0, 1.0]:利用率低于此值触发自动升级
    pub upgrade_threshold: f32,
}

impl Default for AcbGovernorConfig {
    fn default() -> Self {
        Self {
            // 各级 Token 上限递增:1000 → 5000 → 20000 → 100000
            l0_token_limit: 1_000,
            l1_token_limit: 5_000,
            l2_token_limit: 20_000,
            l3_token_limit: 100_000,
            // 总预算:100 万 Token,与 DECB 的 total_budget_limit 量级对齐
            total_budget_limit: 1_000_000,
            // WHY 0.8/0.3 滞后带:利用率 > 80% 降级,< 30% 升级,
            // 中间 50% 区间为稳定带,避免频繁切换
            degrade_threshold: 0.8,
            upgrade_threshold: 0.3,
        }
    }
}

impl AcbGovernorConfig {
    /// 校验配置合法性
    ///
    /// WHY:在构造 AcbGovernor 时调用,提前暴露配置错误,
    /// 避免运行时预算检查产生异常。
    ///
    /// # 校验规则
    /// - 各级预算上限严格递增:L0 < L1 < L2 < L3
    /// - `total_budget_limit` > 0
    /// - `degrade_threshold` ∈ (0.0, 1.0]
    /// - `upgrade_threshold` ∈ [0.0, 1.0)
    /// - `degrade_threshold` > `upgrade_threshold`(滞后带不变量)
    pub fn validate(&self) -> Result<(), AcbError> {
        // 各级预算上限严格递增
        if !(self.l0_token_limit < self.l1_token_limit
            && self.l1_token_limit < self.l2_token_limit
            && self.l2_token_limit < self.l3_token_limit)
        {
            return Err(AcbError::ConfigError {
                detail: format!(
                    "token limits must be strictly increasing: L0={} L1={} L2={} L3={}",
                    self.l0_token_limit,
                    self.l1_token_limit,
                    self.l2_token_limit,
                    self.l3_token_limit
                ),
            });
        }
        if self.total_budget_limit == 0 {
            return Err(AcbError::ConfigError {
                detail: "total_budget_limit must be > 0".into(),
            });
        }
        // 阈值范围校验
        if self.degrade_threshold.is_nan() || !(0.0..=1.0).contains(&self.degrade_threshold) {
            return Err(AcbError::ConfigError {
                detail: format!(
                    "degrade_threshold must be in [0.0, 1.0], got {}",
                    self.degrade_threshold
                ),
            });
        }
        if self.upgrade_threshold.is_nan() || !(0.0..=1.0).contains(&self.upgrade_threshold) {
            return Err(AcbError::ConfigError {
                detail: format!(
                    "upgrade_threshold must be in [0.0, 1.0], got {}",
                    self.upgrade_threshold
                ),
            });
        }
        // 滞后带不变量:降级阈值必须高于升级阈值
        if self.degrade_threshold <= self.upgrade_threshold {
            return Err(AcbError::ConfigError {
                detail: format!(
                    "degrade_threshold ({}) must be > upgrade_threshold ({}) to form hysteresis band",
                    self.degrade_threshold, self.upgrade_threshold
                ),
            });
        }
        Ok(())
    }

    /// 获取指定级别的预算分配
    pub fn allocation_for(&self, tier: BudgetTier) -> BudgetAllocation {
        let token_limit = match tier {
            BudgetTier::L0 => self.l0_token_limit,
            BudgetTier::L1 => self.l1_token_limit,
            BudgetTier::L2 => self.l2_token_limit,
            BudgetTier::L3 => self.l3_token_limit,
        };
        BudgetAllocation::new(tier, token_limit)
    }

    /// 获取当前级别的 Token 上限
    pub fn token_limit_for(&self, tier: BudgetTier) -> u64 {
        match tier {
            BudgetTier::L0 => self.l0_token_limit,
            BudgetTier::L1 => self.l1_token_limit,
            BudgetTier::L2 => self.l2_token_limit,
            BudgetTier::L3 => self.l3_token_limit,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_values() {
        let cfg = AcbGovernorConfig::default();
        assert_eq!(cfg.l0_token_limit, 1_000);
        assert_eq!(cfg.l1_token_limit, 5_000);
        assert_eq!(cfg.l2_token_limit, 20_000);
        assert_eq!(cfg.l3_token_limit, 100_000);
        assert_eq!(cfg.total_budget_limit, 1_000_000);
        assert!((cfg.degrade_threshold - 0.8).abs() < 1e-6);
        assert!((cfg.upgrade_threshold - 0.3).abs() < 1e-6);
    }

    #[test]
    fn test_validate_ok() {
        let cfg = AcbGovernorConfig::default();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn test_validate_limits_not_increasing() {
        let cfg = AcbGovernorConfig {
            l1_token_limit: 500, // L1 < L0,违反递增
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_limits_equal() {
        let cfg = AcbGovernorConfig {
            l2_token_limit: 5_000, // L2 == L1,非严格递增
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_zero_total_budget() {
        let cfg = AcbGovernorConfig {
            total_budget_limit: 0,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_degrade_threshold_out_of_range() {
        let cfg = AcbGovernorConfig {
            degrade_threshold: 1.5,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_upgrade_threshold_out_of_range() {
        let cfg = AcbGovernorConfig {
            upgrade_threshold: -0.1,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_threshold_inverted() {
        // WHY 阈值倒挂:degrade <= upgrade 会导致滞后带消失,频繁切换
        let cfg = AcbGovernorConfig {
            degrade_threshold: 0.3,
            upgrade_threshold: 0.8,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_threshold_equal() {
        let cfg = AcbGovernorConfig {
            degrade_threshold: 0.5,
            upgrade_threshold: 0.5,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_allocation_for() {
        let cfg = AcbGovernorConfig::default();
        assert_eq!(cfg.allocation_for(BudgetTier::L0).token_limit, 1_000);
        assert_eq!(cfg.allocation_for(BudgetTier::L1).token_limit, 5_000);
        assert_eq!(cfg.allocation_for(BudgetTier::L2).token_limit, 20_000);
        assert_eq!(cfg.allocation_for(BudgetTier::L3).token_limit, 100_000);
    }

    #[test]
    fn test_token_limit_for() {
        let cfg = AcbGovernorConfig::default();
        assert_eq!(cfg.token_limit_for(BudgetTier::L2), 20_000);
    }

    #[test]
    fn test_serde_roundtrip() {
        let cfg = AcbGovernorConfig::default();
        let json = serde_json::to_string(&cfg).unwrap();
        let restored: AcbGovernorConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.l0_token_limit, cfg.l0_token_limit);
        assert_eq!(restored.total_budget_limit, cfg.total_budget_limit);
        assert!((restored.degrade_threshold - cfg.degrade_threshold).abs() < 1e-6);
    }
}
