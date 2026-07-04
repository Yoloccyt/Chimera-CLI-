//! DECB 配置类型 — 预算系数、档位阈值与成本单价
//!
//! 对应架构层:L8 Parliament
//! 对应创新点:DECB(Dual-tier Cognitive Budget)
//!
//! # 设计决策(WHY)
//! - `base_budget` 默认 0.8:留 20% 余量,避免满预算时无降级空间
//! - `high_tier_threshold` > `low_tier_threshold`:档位阈值单调递增,
//!   validate() 校验此不变量,防止配置倒挂导致档位判定异常
//! - `tier_switch_lag_ms` 默认 10s:避免频繁切换(抖动),10 秒滞后保证
//!   档位切换有足够时间观察效果(§6 架构红线:避免短视)
//! - `cost_per_token` 默认 0.00001 美分:与 CACR 的美分单位对齐

use serde::{Deserialize, Serialize};

use crate::error::DecbError;

/// DECB 治理器配置
///
/// 所有字段均有合理默认值,可通过 `Default::default()` 快速构造。
/// 构造 `DecbGovernor` 时会调用 `validate()` 校验配置合法性。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DecbConfig {
    /// 基础预算系数 [0.0, 1.0],复杂度与紧急度的乘积基线
    pub base_budget: f32,
    /// 高档阈值:coefficient >= 此值 → HighTier
    pub high_tier_threshold: f32,
    /// 低档阈值:low_tier_threshold <= coefficient < high_tier_threshold → LowTier
    pub low_tier_threshold: f32,
    /// 总预算上限(美分,1 美元 = 100 美分)
    pub total_budget_limit: f64,
    /// 溢出检测周期(毫秒),后台监控任务的检查间隔
    pub overflow_check_interval_ms: u64,
    /// 档位切换滞后(毫秒),防止频繁切换(抖动)
    pub tier_switch_lag_ms: u64,
    /// 每 Token 成本(美分),用于计算消耗
    pub cost_per_token: f64,
    /// 每次工具调用成本(美分),用于计算消耗
    pub cost_per_tool_call: f64,
    /// 复杂度因子下限
    pub complexity_factor_min: f32,
    /// 复杂度因子上限
    pub complexity_factor_max: f32,
    /// 紧急度因子下限
    pub urgency_factor_min: f32,
    /// 紧急度因子上限
    pub urgency_factor_max: f32,
    /// 溢出警告阈值(默认 0.5):消耗占比 >= 此值仅告警,不降级
    ///
    /// WHY 配置化:不同部署场景对预算敏感度不同,边缘场景可能希望更早告警(0.4),
    /// 批处理场景可容忍更高占用(0.6)。默认 0.5 保持向后兼容
    pub overflow_warn_ratio: f64,
    /// 溢出降级阈值(默认 0.8):消耗占比 >= 此值降级到 LowTier
    ///
    /// WHY 配置化:与 overflow_warn_ratio 配合,允许按场景调整降级触发点
    pub overflow_degrade_ratio: f64,
    /// 溢出临界阈值(默认 1.0):消耗占比 >= 此值降级到 Degraded
    ///
    /// WHY 配置化:某些场景允许短暂超支(如 1.2),默认 1.0(100%)保持原语义
    pub overflow_critical_ratio: f64,
}

impl Default for DecbConfig {
    fn default() -> Self {
        Self {
            base_budget: 0.8,
            high_tier_threshold: 0.6,
            low_tier_threshold: 0.3,
            // 100 万 token 等价成本(美分),与 CACR 静态预算对齐
            total_budget_limit: 1_000_000.0,
            overflow_check_interval_ms: 10_000,
            tier_switch_lag_ms: 10_000,
            cost_per_token: 0.00001,
            cost_per_tool_call: 0.001,
            complexity_factor_min: 0.5,
            complexity_factor_max: 1.5,
            urgency_factor_min: 0.8,
            urgency_factor_max: 1.2,
            // WHY 默认值与原硬编码常量一致(0.5/0.8/1.0),保持向后兼容
            overflow_warn_ratio: 0.5,
            overflow_degrade_ratio: 0.8,
            overflow_critical_ratio: 1.0,
        }
    }
}

impl DecbConfig {
    /// 校验配置合法性
    ///
    /// WHY:在构造 DecbGovernor 时调用,提前暴露配置错误,
    /// 避免运行时预算计算产生 NaN 或档位判定异常。
    ///
    /// # 校验规则
    /// - `base_budget` ∈ [0.0, 1.0]
    /// - `high_tier_threshold` > `low_tier_threshold`(阈值单调递增)
    /// - `total_budget_limit` > 0
    /// - `complexity_factor_max` > `complexity_factor_min`
    /// - `urgency_factor_max` > `urgency_factor_min`
    /// - 所有时间间隔 > 0
    /// - 所有成本单价 >= 0
    pub fn validate(&self) -> Result<(), DecbError> {
        if !(0.0..=1.0).contains(&self.base_budget) || self.base_budget.is_nan() {
            return Err(DecbError::ConfigError {
                detail: format!(
                    "base_budget must be in [0.0, 1.0], got {}",
                    self.base_budget
                ),
            });
        }
        if self.high_tier_threshold <= self.low_tier_threshold {
            return Err(DecbError::ConfigError {
                detail: format!(
                    "high_tier_threshold ({}) must be > low_tier_threshold ({})",
                    self.high_tier_threshold, self.low_tier_threshold
                ),
            });
        }
        if !(0.0..=1.0).contains(&self.high_tier_threshold) {
            return Err(DecbError::ConfigError {
                detail: format!(
                    "high_tier_threshold must be in [0.0, 1.0], got {}",
                    self.high_tier_threshold
                ),
            });
        }
        if !(0.0..=1.0).contains(&self.low_tier_threshold) {
            return Err(DecbError::ConfigError {
                detail: format!(
                    "low_tier_threshold must be in [0.0, 1.0], got {}",
                    self.low_tier_threshold
                ),
            });
        }
        if self.total_budget_limit <= 0.0 {
            return Err(DecbError::ConfigError {
                detail: format!(
                    "total_budget_limit must be > 0, got {}",
                    self.total_budget_limit
                ),
            });
        }
        if self.overflow_check_interval_ms == 0 {
            return Err(DecbError::ConfigError {
                detail: "overflow_check_interval_ms must be > 0".into(),
            });
        }
        // WHY 允许 tier_switch_lag_ms = 0:表示无滞后,用于测试或需要立即切换的场景。
        // 生产环境建议使用默认值 10000ms 避免频繁切换。
        if self.cost_per_token < 0.0 {
            return Err(DecbError::ConfigError {
                detail: format!("cost_per_token must be >= 0, got {}", self.cost_per_token),
            });
        }
        if self.cost_per_tool_call < 0.0 {
            return Err(DecbError::ConfigError {
                detail: format!(
                    "cost_per_tool_call must be >= 0, got {}",
                    self.cost_per_tool_call
                ),
            });
        }
        if self.complexity_factor_max <= self.complexity_factor_min {
            return Err(DecbError::ConfigError {
                detail: format!(
                    "complexity_factor_max ({}) must be > complexity_factor_min ({})",
                    self.complexity_factor_max, self.complexity_factor_min
                ),
            });
        }
        if self.urgency_factor_max <= self.urgency_factor_min {
            return Err(DecbError::ConfigError {
                detail: format!(
                    "urgency_factor_max ({}) must be > urgency_factor_min ({})",
                    self.urgency_factor_max, self.urgency_factor_min
                ),
            });
        }
        // WHY 校验溢出阈值:三级阈值必须严格递增(warn < degrade < critical),
        // 否则 check_overflow 的 if-elif 链会失效(如 warn >= critical 时永不到达 critical 分支)。
        // 同时禁止 NaN/负值,防止 ratio 比较产生未定义行为。
        for (name, val) in [
            ("overflow_warn_ratio", self.overflow_warn_ratio),
            ("overflow_degrade_ratio", self.overflow_degrade_ratio),
            ("overflow_critical_ratio", self.overflow_critical_ratio),
        ] {
            if val.is_nan() || val < 0.0 {
                return Err(DecbError::ConfigError {
                    detail: format!("{name} ({val}) must be finite and >= 0.0"),
                });
            }
        }
        if self.overflow_warn_ratio >= self.overflow_degrade_ratio {
            return Err(DecbError::ConfigError {
                detail: format!(
                    "overflow_warn_ratio ({}) must be < overflow_degrade_ratio ({})",
                    self.overflow_warn_ratio, self.overflow_degrade_ratio
                ),
            });
        }
        if self.overflow_degrade_ratio >= self.overflow_critical_ratio {
            return Err(DecbError::ConfigError {
                detail: format!(
                    "overflow_degrade_ratio ({}) must be < overflow_critical_ratio ({})",
                    self.overflow_degrade_ratio, self.overflow_critical_ratio
                ),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_values() {
        let cfg = DecbConfig::default();
        assert!((cfg.base_budget - 0.8).abs() < 1e-6);
        assert!((cfg.high_tier_threshold - 0.6).abs() < 1e-6);
        assert!((cfg.low_tier_threshold - 0.3).abs() < 1e-6);
        assert!((cfg.total_budget_limit - 1_000_000.0).abs() < 1e-6);
        assert_eq!(cfg.overflow_check_interval_ms, 10_000);
        assert_eq!(cfg.tier_switch_lag_ms, 10_000);
        assert!((cfg.cost_per_token - 0.00001).abs() < 1e-9);
        assert!((cfg.cost_per_tool_call - 0.001).abs() < 1e-9);
        assert!((cfg.complexity_factor_min - 0.5).abs() < 1e-6);
        assert!((cfg.complexity_factor_max - 1.5).abs() < 1e-6);
        assert!((cfg.urgency_factor_min - 0.8).abs() < 1e-6);
        assert!((cfg.urgency_factor_max - 1.2).abs() < 1e-6);
        assert!((cfg.overflow_warn_ratio - 0.5).abs() < 1e-9);
        assert!((cfg.overflow_degrade_ratio - 0.8).abs() < 1e-9);
        assert!((cfg.overflow_critical_ratio - 1.0).abs() < 1e-9);
    }

    #[test]
    fn test_validate_ok() {
        let cfg = DecbConfig::default();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn test_validate_base_budget_out_of_range() {
        let cfg = DecbConfig {
            base_budget: 1.5,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_threshold_inverted() {
        // WHY 阈值倒挂:high <= low 会导致档位判定异常,必须拒绝
        let cfg = DecbConfig {
            high_tier_threshold: 0.3,
            low_tier_threshold: 0.6,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_threshold_equal() {
        let cfg = DecbConfig {
            high_tier_threshold: 0.5,
            low_tier_threshold: 0.5,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_high_threshold_out_of_range() {
        let cfg = DecbConfig {
            high_tier_threshold: 1.5,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_zero_budget_limit() {
        let cfg = DecbConfig {
            total_budget_limit: 0.0,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_zero_overflow_interval() {
        let cfg = DecbConfig {
            overflow_check_interval_ms: 0,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_zero_switch_lag_allowed() {
        // WHY 允许 tier_switch_lag_ms = 0:表示无滞后,用于测试或需要立即切换的场景
        let cfg = DecbConfig {
            tier_switch_lag_ms: 0,
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn test_validate_negative_cost_per_token() {
        let cfg = DecbConfig {
            cost_per_token: -0.1,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_complexity_factors_inverted() {
        let cfg = DecbConfig {
            complexity_factor_min: 1.5,
            complexity_factor_max: 0.5,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_urgency_factors_inverted() {
        let cfg = DecbConfig {
            urgency_factor_min: 1.2,
            urgency_factor_max: 0.8,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_overflow_ratios_not_strictly_increasing() {
        // warn == degrade → 违反严格递增
        let cfg = DecbConfig {
            overflow_warn_ratio: 0.8,
            overflow_degrade_ratio: 0.8,
            overflow_critical_ratio: 1.0,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());

        // degrade > critical → 违反严格递增
        let cfg = DecbConfig {
            overflow_warn_ratio: 0.5,
            overflow_degrade_ratio: 1.2,
            overflow_critical_ratio: 1.0,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_overflow_ratio_negative() {
        let cfg = DecbConfig {
            overflow_warn_ratio: -0.1,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_overflow_ratios_custom_valid() {
        // 自定义合法的严格递增阈值(允许 critical > 1.0)
        let cfg = DecbConfig {
            overflow_warn_ratio: 0.4,
            overflow_degrade_ratio: 0.7,
            overflow_critical_ratio: 1.2,
            ..Default::default()
        };
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn test_serde_roundtrip() {
        let cfg = DecbConfig::default();
        let json = serde_json::to_string(&cfg).unwrap();
        let restored: DecbConfig = serde_json::from_str(&json).unwrap();
        assert!((cfg.base_budget - restored.base_budget).abs() < 1e-6);
        assert_eq!(
            cfg.overflow_check_interval_ms,
            restored.overflow_check_interval_ms
        );
    }
}
