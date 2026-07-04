//! OSA 配置 — 全维稀疏协调器的参数化配置
//!
//! 对应架构层:L6 Router
//!
//! # 设计决策(WHY)
//! - **routing_top_k_bounds (8, 32)**:routing 维度 Top-K 的上下界,
//!   Simple 档位取下界(8),UltraComplex 档位取上界(32),中间档位线性插值
//! - **context_scope_multipliers [1, 10, 100, 1000]**:context 维度按复杂度档位的保留数量,
//!   对应架构手册四档分级(Simple=1, Regular=10, Complex=100, UltraComplex=1000)
//! - **audit_rate_by_risk [0.1, 0.3, 0.7, 1.0]**:audit 维度按风险等级的采样率,
//!   实际采样率取复杂度档位默认值与风险等级配置值的最大值(更保守)
//! - **budget_protection_threshold 0.8**:budget 维度的保护比例上限,
//!   复杂度越高,保护比例越低(保留更多任务以避免预算耗尽)

use serde::{Deserialize, Serialize};

use crate::error::OsaError;
use crate::types::ComplexityBand;

/// OSA 协调器配置 — 五维度稀疏化的参数化控制
///
/// 所有字段在创建时填充,不可变(无需内部可变性)。
/// `validate()` 方法校验配置合法性,`Default` 提供架构手册推荐的默认值。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OsaConfig {
    /// routing 维度 Top-K 上下界(默认 (8, 32))
    ///
    /// WHY:下界 8 对应 Simple 档位,上界 32 对应 UltraComplex 档位。
    /// 中间档位(Regular/Complex)按 4 档线性插值:8/16/24/32
    pub routing_top_k_bounds: (usize, usize),

    /// context 维度按复杂度档位的保留数量(默认 [1, 10, 100, 1000])
    ///
    /// WHY:对应架构手册四档分级:
    /// - Simple:1 文件(最小上下文)
    /// - Regular:10 文件(标准上下文)
    /// - Complex:100 文件(增强上下文)
    /// - UltraComplex:1000 文件(最大上下文)
    pub context_scope_multipliers: [usize; 4],

    /// audit 维度按风险等级的采样率(默认 [0.1, 0.3, 0.7, 1.0])
    ///
    /// WHY:按 RiskLevel 索引(Low=0, Medium=1, High=2, Critical=3)。
    /// 实际采样率取复杂度档位默认值与风险等级配置值的最大值(更保守)。
    /// 例如:Simple 档位 + Critical 风险 → max(0.1, 1.0) = 1.0(全审计)
    pub audit_rate_by_risk: [f32; 4],

    /// budget 维度的保护比例上限(默认 0.8)
    ///
    /// WHY:0.8 表示最多保留 80% 的活跃任务。
    /// 复杂度越高,实际保护比例越低(保留更多任务以避免预算耗尽):
    /// protection = threshold × (1.0 - complexity × 0.5)
    pub budget_protection_threshold: f32,

    /// 复杂度档位阈值(默认 (0.25, 0.5, 0.75))
    ///
    /// WHY:SubTask 14.4 — 原硬编码在 `ComplexityBand::from_complexity` 中,
    /// 移入配置支持调优。三个阈值将 [0.0, 1.0] 分为四档:
    /// - `[0.0, t1)`:Simple
    /// - `[t1, t2)`:Regular
    /// - `[t2, t3)`:Complex
    /// - `[t3, 1.0]`:UltraComplex
    ///
    /// 校验约束:0 < t1 < t2 < t3 < 1.0
    pub complexity_thresholds: (f32, f32, f32),
}

impl OsaConfig {
    /// 创建默认配置
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置 routing 维度 Top-K 上下界
    pub fn with_routing_top_k_bounds(mut self, min: usize, max: usize) -> Self {
        self.routing_top_k_bounds = (min, max);
        self
    }

    /// 设置 context 维度按复杂度档位的保留数量
    pub fn with_context_scope_multipliers(mut self, multipliers: [usize; 4]) -> Self {
        self.context_scope_multipliers = multipliers;
        self
    }

    /// 设置 audit 维度按风险等级的采样率
    pub fn with_audit_rate_by_risk(mut self, rates: [f32; 4]) -> Self {
        self.audit_rate_by_risk = rates;
        self
    }

    /// 设置 budget 维度的保护比例上限
    pub fn with_budget_protection_threshold(mut self, threshold: f32) -> Self {
        self.budget_protection_threshold = threshold;
        self
    }

    /// 设置复杂度档位阈值(SubTask 14.4)
    ///
    /// # 参数
    /// - `t1`:Simple/Regular 分界(默认 0.25)
    /// - `t2`:Regular/Complex 分界(默认 0.5)
    /// - `t3`:Complex/UltraComplex 分界(默认 0.75)
    ///
    /// 校验约束:0 < t1 < t2 < t3 < 1.0(在 `validate()` 中校验)
    pub fn with_complexity_thresholds(mut self, t1: f32, t2: f32, t3: f32) -> Self {
        self.complexity_thresholds = (t1, t2, t3);
        self
    }

    /// 获取复杂度档位阈值(SubTask 14.4)
    pub fn complexity_thresholds(&self) -> (f32, f32, f32) {
        self.complexity_thresholds
    }

    /// 校验配置合法性,返回 OsaError 描述具体问题
    ///
    /// 校验规则:
    /// - routing_top_k_bounds 下限 ≤ 上限,且均 > 0
    /// - context_scope_multipliers 各元素 > 0
    /// - audit_rate_by_risk 各元素 ∈ [0.0, 1.0]
    /// - budget_protection_threshold ∈ [0.0, 1.0]
    /// - complexity_thresholds 满足 0 < t1 < t2 < t3 < 1.0
    pub fn validate(&self) -> Result<(), OsaError> {
        let (min, max) = self.routing_top_k_bounds;
        if min == 0 || max == 0 {
            return Err(OsaError::InvalidConfig(
                "routing_top_k_bounds 不能为 0".into(),
            ));
        }
        if min > max {
            return Err(OsaError::InvalidConfig(format!(
                "routing_top_k_bounds 下限 {min} > 上限 {max}"
            )));
        }
        for (i, &m) in self.context_scope_multipliers.iter().enumerate() {
            if m == 0 {
                return Err(OsaError::InvalidConfig(format!(
                    "context_scope_multipliers[{i}] 不能为 0"
                )));
            }
        }
        for (i, &r) in self.audit_rate_by_risk.iter().enumerate() {
            if !(0.0..=1.0).contains(&r) {
                return Err(OsaError::InvalidConfig(format!(
                    "audit_rate_by_risk[{i}] = {r} 超出 [0.0, 1.0]"
                )));
            }
        }
        if !(0.0..=1.0).contains(&self.budget_protection_threshold) {
            return Err(OsaError::InvalidConfig(format!(
                "budget_protection_threshold = {} 超出 [0.0, 1.0]",
                self.budget_protection_threshold
            )));
        }
        // SubTask 14.4:校验复杂度阈值满足 0 < t1 < t2 < t3 < 1.0
        let (t1, t2, t3) = self.complexity_thresholds;
        if !(0.0 < t1 && t1 < t2 && t2 < t3 && t3 < 1.0) {
            return Err(OsaError::InvalidConfig(format!(
                "complexity_thresholds = ({t1}, {t2}, {t3}) 不满足 0 < t1 < t2 < t3 < 1.0"
            )));
        }
        Ok(())
    }

    /// 获取指定复杂度档位的 routing Top-K
    ///
    /// 按 4 档线性插值:Simple=下界, Regular=1/3, Complex=2/3, UltraComplex=上界
    pub fn routing_top_k_for(&self, band: ComplexityBand) -> usize {
        let (min, max) = self.routing_top_k_bounds;
        match band {
            ComplexityBand::Simple => min,
            ComplexityBand::Regular => min + (max - min) / 3,
            ComplexityBand::Complex => min + 2 * (max - min) / 3,
            ComplexityBand::UltraComplex => max,
        }
    }

    /// 获取指定复杂度档位的 context 保留数量
    pub fn context_scope_for(&self, band: ComplexityBand) -> usize {
        self.context_scope_multipliers[band.as_index()]
    }

    /// 获取指定风险等级的 audit 采样率
    pub fn audit_rate_for(&self, risk_index: usize) -> f32 {
        self.audit_rate_by_risk[risk_index]
    }
}

impl Default for OsaConfig {
    fn default() -> Self {
        Self {
            routing_top_k_bounds: (8, 32),
            context_scope_multipliers: [1, 10, 100, 1000],
            audit_rate_by_risk: [0.1, 0.3, 0.7, 1.0],
            budget_protection_threshold: 0.8,
            // SubTask 14.4:默认阈值与原硬编码一致,保持向后兼容
            complexity_thresholds: (0.25, 0.5, 0.75),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::RiskLevel;

    #[test]
    fn test_default_config() {
        let config = OsaConfig::default();
        assert_eq!(config.routing_top_k_bounds, (8, 32));
        assert_eq!(config.context_scope_multipliers, [1, 10, 100, 1000]);
        assert_eq!(config.audit_rate_by_risk, [0.1, 0.3, 0.7, 1.0]);
        assert!((config.budget_protection_threshold - 0.8).abs() < 1e-6);
        // SubTask 14.4:默认复杂度阈值
        assert_eq!(config.complexity_thresholds, (0.25, 0.5, 0.75));
    }

    #[test]
    fn test_validate_valid() {
        let config = OsaConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_zero_bounds() {
        let config = OsaConfig::new().with_routing_top_k_bounds(0, 32);
        let err = config.validate().unwrap_err();
        assert!(matches!(err, OsaError::InvalidConfig(_)));
    }

    #[test]
    fn test_validate_inverted_bounds() {
        let config = OsaConfig::new().with_routing_top_k_bounds(32, 8);
        let err = config.validate().unwrap_err();
        assert!(matches!(err, OsaError::InvalidConfig(_)));
    }

    #[test]
    fn test_validate_invalid_audit_rate() {
        let config = OsaConfig::new().with_audit_rate_by_risk([0.1, 0.3, 0.7, 1.5]);
        let err = config.validate().unwrap_err();
        assert!(matches!(err, OsaError::InvalidConfig(_)));
    }

    #[test]
    fn test_validate_invalid_threshold() {
        let config = OsaConfig::new().with_budget_protection_threshold(1.5);
        let err = config.validate().unwrap_err();
        assert!(matches!(err, OsaError::InvalidConfig(_)));
    }

    // ===== SubTask 14.4:复杂度阈值校验测试 =====

    #[test]
    fn test_validate_valid_complexity_thresholds() {
        // 默认阈值应通过校验
        assert!(OsaConfig::default().validate().is_ok());
        // 自定义合法阈值
        let config = OsaConfig::default().with_complexity_thresholds(0.3, 0.6, 0.9);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_complexity_thresholds_t1_le_zero() {
        // t1 <= 0 不合法
        let config = OsaConfig::default().with_complexity_thresholds(0.0, 0.5, 0.75);
        let err = config.validate().unwrap_err();
        assert!(matches!(err, OsaError::InvalidConfig(_)));
    }

    #[test]
    fn test_validate_complexity_thresholds_t1_ge_t2() {
        // t1 >= t2 不合法
        let config = OsaConfig::default().with_complexity_thresholds(0.5, 0.5, 0.75);
        let err = config.validate().unwrap_err();
        assert!(matches!(err, OsaError::InvalidConfig(_)));
    }

    #[test]
    fn test_validate_complexity_thresholds_t2_ge_t3() {
        // t2 >= t3 不合法
        let config = OsaConfig::default().with_complexity_thresholds(0.25, 0.75, 0.75);
        let err = config.validate().unwrap_err();
        assert!(matches!(err, OsaError::InvalidConfig(_)));
    }

    #[test]
    fn test_validate_complexity_thresholds_t3_ge_one() {
        // t3 >= 1.0 不合法
        let config = OsaConfig::default().with_complexity_thresholds(0.25, 0.5, 1.0);
        let err = config.validate().unwrap_err();
        assert!(matches!(err, OsaError::InvalidConfig(_)));
    }

    #[test]
    fn test_routing_top_k_for_bands() {
        let config = OsaConfig::default();
        assert_eq!(config.routing_top_k_for(ComplexityBand::Simple), 8);
        assert_eq!(config.routing_top_k_for(ComplexityBand::UltraComplex), 32);
        // Regular = 8 + (32-8)/3 = 8 + 8 = 16
        assert_eq!(config.routing_top_k_for(ComplexityBand::Regular), 16);
        // Complex = 8 + 2*(32-8)/3 = 8 + 16 = 24
        assert_eq!(config.routing_top_k_for(ComplexityBand::Complex), 24);
    }

    #[test]
    fn test_context_scope_for_bands() {
        let config = OsaConfig::default();
        assert_eq!(config.context_scope_for(ComplexityBand::Simple), 1);
        assert_eq!(config.context_scope_for(ComplexityBand::Regular), 10);
        assert_eq!(config.context_scope_for(ComplexityBand::Complex), 100);
        assert_eq!(config.context_scope_for(ComplexityBand::UltraComplex), 1000);
    }

    #[test]
    fn test_audit_rate_for_risk() {
        let config = OsaConfig::default();
        assert!((config.audit_rate_for(RiskLevel::Low.as_index()) - 0.1).abs() < 1e-6);
        assert!((config.audit_rate_for(RiskLevel::Critical.as_index()) - 1.0).abs() < 1e-6);
    }
}
