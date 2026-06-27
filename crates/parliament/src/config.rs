//! Parliament 配置类型 — 5 角色投票权重、共识阈值与辩论超时
//!
//! 对应架构层:L8 Parliament
//! 对应创新点:AHIRT(Anti-Hack Intelligent Red Team,反黑客红队)
//!
//! # 设计决策(WHY)
//! - 5 角色权重默认和为 1.0(0.25+0.30+0.20+0.15+0.10),保证加权赞成率归一化
//! - Skeptic 权重最高(0.30):红队视角的风险审查是 AHIRT 核心,权重倾斜
//! - `consensus_threshold` 默认 0.6:超过 60% 加权赞成率才达成共识,
//!   避免边缘提案通过(对应架构红线"功能乱?禁止功能标志")
//! - `quorum_threshold` 默认 0.6:法定人数要求 60% 角色参与,
//!   防止少数角色垄断决策
//! - `debate_timeout_ms` 默认 5000:5 秒超时,平衡深度辩论与响应延迟

use serde::{Deserialize, Serialize};

use crate::error::ParliamentError;

/// Parliament 配置 — 5 角色权重、共识阈值与辩论超时
///
/// 所有字段均有合理默认值,可通过 `Default::default()` 快速构造。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParliamentConfig {
    /// Architect(架构师)投票权重,默认 0.25
    pub architect_weight: f32,
    /// Skeptic(怀疑者)投票权重,默认 0.30(红队权重倾斜)
    pub skeptic_weight: f32,
    /// Optimizer(优化者)投票权重,默认 0.20
    pub optimizer_weight: f32,
    /// Librarian(图书馆员)投票权重,默认 0.15
    pub librarian_weight: f32,
    /// Bard(吟游诗人)投票权重,默认 0.10
    pub bard_weight: f32,
    /// 共识阈值:加权赞成率 ≥ 此值才达成共识,默认 0.6
    pub consensus_threshold: f32,
    /// 法定人数阈值:参与率 ≥ 此值才有效,默认 0.6
    pub quorum_threshold: f32,
    /// 辩论超时(毫秒),5 角色需在此时间内完成,默认 5000
    pub debate_timeout_ms: u64,
}

impl Default for ParliamentConfig {
    fn default() -> Self {
        Self {
            architect_weight: 0.25,
            skeptic_weight: 0.30,
            optimizer_weight: 0.20,
            librarian_weight: 0.15,
            bard_weight: 0.10,
            consensus_threshold: 0.6,
            quorum_threshold: 0.6,
            debate_timeout_ms: 5000,
        }
    }
}

impl ParliamentConfig {
    /// 校验配置合法性
    ///
    /// WHY:在构造 Parliament 时调用,提前暴露配置错误,
    /// 避免运行时投票计算产生 NaN 或负值导致共识判定异常
    pub fn validate(&self) -> Result<(), ParliamentError> {
        // 权重应为非负
        let weights = [
            self.architect_weight,
            self.skeptic_weight,
            self.optimizer_weight,
            self.librarian_weight,
            self.bard_weight,
        ];
        if weights.iter().any(|&w| w < 0.0) {
            return Err(ParliamentError::ConfigError {
                detail: "weights must be non-negative".into(),
            });
        }

        // 权重和应接近 1.0(允许浮点误差)
        let sum: f32 = weights.iter().sum();
        if (sum - 1.0).abs() > 1e-3 {
            return Err(ParliamentError::ConfigError {
                detail: format!("weights sum must be ~1.0, got {sum}"),
            });
        }

        // 阈值应在 [0.0, 1.0] 区间
        if !(0.0..=1.0).contains(&self.consensus_threshold) {
            return Err(ParliamentError::ConfigError {
                detail: "consensus_threshold must be in [0.0, 1.0]".into(),
            });
        }
        if !(0.0..=1.0).contains(&self.quorum_threshold) {
            return Err(ParliamentError::ConfigError {
                detail: "quorum_threshold must be in [0.0, 1.0]".into(),
            });
        }

        // 超时应大于 0
        if self.debate_timeout_ms == 0 {
            return Err(ParliamentError::ConfigError {
                detail: "debate_timeout_ms must be > 0".into(),
            });
        }

        Ok(())
    }

    /// 获取指定角色的投票权重
    pub fn weight_of(&self, role: crate::types::Role) -> f32 {
        use crate::types::Role;
        match role {
            Role::Architect => self.architect_weight,
            Role::Skeptic => self.skeptic_weight,
            Role::Optimizer => self.optimizer_weight,
            Role::Librarian => self.librarian_weight,
            Role::Bard => self.bard_weight,
        }
    }
}

// ============================================================
// AHIRT 配置
// ============================================================

/// AHIRT 配置 — 反黑客红队的探测周期、检测率阈值与批次大小
///
/// WHY 独立 struct(非嵌入 ParliamentConfig):`AhirtRedTeam::with_config`
/// 只需 AHIRT 相关参数,无需整个 ParliamentConfig,遵循接口隔离原则。
/// 两者复用同一 `ParliamentError::ConfigError` 错误变体,避免错误类型膨胀。
///
/// # 字段语义
/// - `probe_cycle_secs`:周期探测的间隔(秒),默认 300(5 分钟)。
///   下限 60 秒,防止探测过于频繁拖慢系统。
/// - `detection_rate_threshold`:漏洞判定阈值,默认 0.95。
///   探测通过率 < 此值时发布 RedTeamAudit `[Critical]` 事件。
///   注意:此字段为 `f64`(配置精度),与 `AhirtStats.detection_rate`
///   的 `f32`(运行时统计)不同,比较时需显式转换。
/// - `payload_batch_size`:每次 `probe` 调用的逻辑批次大小,默认 25。
///   用于 `chunks` 分批处理,为将来并行探测/限流预留扩展点。
///   下限 1,因为 `chunks(0)` 会 panic。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AhirtConfig {
    /// 周期探测间隔(秒),默认 300(5 分钟),下限 60
    pub probe_cycle_secs: u64,
    /// 检测率阈值 [0.0, 1.0],默认 0.95
    pub detection_rate_threshold: f64,
    /// 探测载荷批次大小,默认 25,下限 1
    pub payload_batch_size: usize,
}

impl Default for AhirtConfig {
    fn default() -> Self {
        Self {
            probe_cycle_secs: 300,
            detection_rate_threshold: 0.95,
            payload_batch_size: 25,
        }
    }
}

impl AhirtConfig {
    /// 校验配置合法性
    ///
    /// WHY:在构造 `AhirtRedTeam` 前调用,提前暴露配置错误,
    /// 避免运行时 `chunks(0)` panic 或阈值越界导致漏洞判定异常。
    ///
    /// # 校验规则
    /// - `detection_rate_threshold` ∈ [0.0, 1.0]
    /// - `probe_cycle_secs` ≥ 60(防止探测过频拖慢系统)
    /// - `payload_batch_size` ≥ 1(`chunks(0)` 会 panic)
    pub fn validate(&self) -> Result<(), ParliamentError> {
        // 检测率阈值必须在 [0.0, 1.0] 区间
        if !(0.0..=1.0).contains(&self.detection_rate_threshold) {
            return Err(ParliamentError::ConfigError {
                detail: format!(
                    "detection_rate_threshold must be in [0.0, 1.0], got {}",
                    self.detection_rate_threshold
                ),
            });
        }

        // 探测周期至少 60 秒,防止过频探测拖慢主流程
        if self.probe_cycle_secs < 60 {
            return Err(ParliamentError::ConfigError {
                detail: format!(
                    "probe_cycle_secs must be >= 60, got {}",
                    self.probe_cycle_secs
                ),
            });
        }

        // 批次大小至少 1,因为 Vec::chunks(0) 会 panic
        if self.payload_batch_size == 0 {
            return Err(ParliamentError::ConfigError {
                detail: "payload_batch_size must be >= 1, got 0".into(),
            });
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Role;

    #[test]
    fn test_default_values() {
        let cfg = ParliamentConfig::default();
        assert!((cfg.architect_weight - 0.25).abs() < 1e-6);
        assert!((cfg.skeptic_weight - 0.30).abs() < 1e-6);
        assert!((cfg.optimizer_weight - 0.20).abs() < 1e-6);
        assert!((cfg.librarian_weight - 0.15).abs() < 1e-6);
        assert!((cfg.bard_weight - 0.10).abs() < 1e-6);
        assert!((cfg.consensus_threshold - 0.6).abs() < 1e-6);
        assert!((cfg.quorum_threshold - 0.6).abs() < 1e-6);
        assert_eq!(cfg.debate_timeout_ms, 5000);
    }

    #[test]
    fn test_default_weights_sum_to_one() {
        let cfg = ParliamentConfig::default();
        let sum = cfg.architect_weight
            + cfg.skeptic_weight
            + cfg.optimizer_weight
            + cfg.librarian_weight
            + cfg.bard_weight;
        assert!(
            (sum - 1.0).abs() < 1e-6,
            "weights sum should be 1.0, got {sum}"
        );
    }

    #[test]
    fn test_validate_ok() {
        let cfg = ParliamentConfig::default();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn test_validate_negative_weight() {
        let cfg = ParliamentConfig {
            architect_weight: -0.1,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_weights_sum_not_one() {
        let cfg = ParliamentConfig {
            architect_weight: 0.5,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_consensus_threshold_out_of_range() {
        let cfg = ParliamentConfig {
            consensus_threshold: 1.5,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_quorum_threshold_out_of_range() {
        let cfg = ParliamentConfig {
            quorum_threshold: -0.1,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_zero_timeout() {
        let cfg = ParliamentConfig {
            debate_timeout_ms: 0,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_weight_of() {
        let cfg = ParliamentConfig::default();
        assert!((cfg.weight_of(Role::Architect) - 0.25).abs() < 1e-6);
        assert!((cfg.weight_of(Role::Skeptic) - 0.30).abs() < 1e-6);
        assert!((cfg.weight_of(Role::Optimizer) - 0.20).abs() < 1e-6);
        assert!((cfg.weight_of(Role::Librarian) - 0.15).abs() < 1e-6);
        assert!((cfg.weight_of(Role::Bard) - 0.10).abs() < 1e-6);
    }

    #[test]
    fn test_serde_roundtrip() {
        let cfg = ParliamentConfig::default();
        let json = serde_json::to_string(&cfg).unwrap();
        let restored: ParliamentConfig = serde_json::from_str(&json).unwrap();
        assert!((cfg.architect_weight - restored.architect_weight).abs() < 1e-6);
        assert_eq!(cfg.debate_timeout_ms, restored.debate_timeout_ms);
    }

    // === SubTask 8.1: AhirtConfig 测试 ===

    #[test]
    fn test_ahirt_config_default_values() {
        let cfg = AhirtConfig::default();
        assert_eq!(cfg.probe_cycle_secs, 300, "默认周期应为 300 秒(5 分钟)");
        assert!(
            (cfg.detection_rate_threshold - 0.95).abs() < 1e-9,
            "默认检测率阈值应为 0.95"
        );
        assert_eq!(cfg.payload_batch_size, 25, "默认批次大小应为 25");
    }

    #[test]
    fn test_ahirt_config_validate_ok() {
        let cfg = AhirtConfig::default();
        assert!(cfg.validate().is_ok(), "默认配置应验证通过");
    }

    #[test]
    fn test_ahirt_config_validate_detection_rate_above_one() {
        let cfg = AhirtConfig {
            detection_rate_threshold: 1.5,
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(
            err.to_string().contains("detection_rate_threshold"),
            "错误信息应包含字段名, got: {err}"
        );
    }

    #[test]
    fn test_ahirt_config_validate_detection_rate_negative() {
        let cfg = AhirtConfig {
            detection_rate_threshold: -0.1,
            ..Default::default()
        };
        assert!(cfg.validate().is_err(), "负检测率应验证失败");
    }

    #[test]
    fn test_ahirt_config_validate_cycle_too_short() {
        let cfg = AhirtConfig {
            probe_cycle_secs: 59,
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(
            err.to_string().contains("probe_cycle_secs"),
            "错误信息应包含字段名, got: {err}"
        );
    }

    #[test]
    fn test_ahirt_config_validate_batch_zero() {
        let cfg = AhirtConfig {
            payload_batch_size: 0,
            ..Default::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(
            err.to_string().contains("payload_batch_size"),
            "错误信息应包含字段名, got: {err}"
        );
    }

    #[test]
    fn test_ahirt_config_validate_boundary_values() {
        // WHY 边界值测试:验证下限恰好通过(60 秒、0.0、1.0、批次 1)
        // 确保校验为 >= 而非 >,边界值合法
        let boundary = AhirtConfig {
            probe_cycle_secs: 60,
            detection_rate_threshold: 0.0,
            payload_batch_size: 1,
        };
        assert!(boundary.validate().is_ok(), "下边界值(60/0.0/1)应通过");

        let upper = AhirtConfig {
            probe_cycle_secs: 60,
            detection_rate_threshold: 1.0,
            payload_batch_size: 1,
        };
        assert!(upper.validate().is_ok(), "检测率上边界 1.0 应通过");
    }

    #[test]
    fn test_ahirt_config_serde_roundtrip() {
        let cfg = AhirtConfig {
            probe_cycle_secs: 120,
            detection_rate_threshold: 0.85,
            payload_batch_size: 10,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let restored: AhirtConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, restored, "序列化往返应保持配置不变");
    }

    #[test]
    fn test_ahirt_config_partial_eq() {
        let a = AhirtConfig::default();
        let b = AhirtConfig::default();
        assert_eq!(a, b, "两个默认配置应相等");

        let c = AhirtConfig {
            probe_cycle_secs: 600,
            ..Default::default()
        };
        assert_ne!(a, c, "不同周期应不相等");
    }
}
