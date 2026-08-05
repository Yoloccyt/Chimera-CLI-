//! 错误类型 — GSOE 库层 thiserror enum
//!
//! 遵循 §4.1:库层用自定义 thiserror enum,应用层才用 anyhow

use thiserror::Error;

/// GSOE 进化引擎错误
#[derive(Debug, Error)]
pub enum GsoeError {
    /// 策略参数非法(超出范围或违反约束)
    #[error("非法策略参数: {reason}")]
    InvalidPolicy {
        /// 错误原因描述
        reason: String,
    },

    /// 变异失败(幅度越界或类型不匹配)
    #[error("变异失败: {reason}")]
    MutationFailed {
        /// 错误原因描述
        reason: String,
    },

    /// 配置错误(字段非法或缺失)
    #[error("配置错误: {reason}")]
    ConfigError {
        /// 错误原因描述
        reason: String,
    },

    /// 达到最大世代数,进化已终止
    #[error("达到最大世代数 {max_generation},进化已终止")]
    MaxGenerationReached {
        /// 配置的最大世代数
        max_generation: u64,
    },

    /// P5.2.1: CI 执行门调用失败 — cargo test / clippy 子进程不可达或超时
    ///
    /// 触发场景:`CargoCiGate::execute()` 内部子进程调用失败。
    /// 处理策略:返回错误,调用方应降级为"沿用上一版本 spec"并告警。
    #[error("CI 执行门调用失败: {reason}")]
    CiGateExecutionFailed {
        /// 失败原因描述
        reason: String,
    },

    /// P5.2.1: INV-9 委托图有环 — 通道 B 否决路径检测到 MAS 委托图存在环
    ///
    /// 触发场景:`CargoCiGate::execute()` 调用 gsoe-evolution 内部 INV-9 检查时
    /// 检测到委托关系构成的有向图存在环。
    ///
    /// WHY 独立变体而非复用 `chimera_mas::MasError::DelegationCycleDetected`:
    /// gsoe-evolution (L5) 不能依赖 chimera-mas (L9),违反 §2.2 依赖铁律。
    /// 本变体在 L5 层独立承载 INV-9 语义,与 L9 的同名变体语义镜像但不共享实现。
    /// 详见 ADR-045 决策 1(架构层归属)+ 本 crate ci_gate.rs 设计偏差记录。
    #[error("INV-9 委托图有环: cycle_path = {cycle_path:?}")]
    DelegationCycleDetected {
        /// 检测到的环路径(Agent ID 序列,首尾相同构成环,如 ["A", "B", "C", "A"])
        cycle_path: Vec<String>,
    },

    /// P5.2.2: 否决证据不足 — 通道 B 显著性检测未达否决阈值
    ///
    /// 触发场景:`SignificanceDetector::is_significant()` 返回 false 时,
    /// 调用方尝试调用 `check_veto_evidence()` 强制否决。
    ///
    /// WHY 独立逻辑(非 INV-9):ADR-045 决策 1 明确"否决证据充分性"与
    /// "委托图无环"是两个独立概念。本变体承载"连续 3 次统计显著回归才否决"
    /// 的判定逻辑,与 INV-9 委托图无环检查分离。
    #[error("否决证据不足: regression_streak={regression_streak}, significance={significance}")]
    VetoEvidenceInsufficient {
        /// 当前连续回归次数
        regression_streak: u32,
        /// 当前显著性 p-value
        significance: f64,
    },

    /// P5.2.3: 不可进化面违反 — spec 候选触碰 13 红线 / Critical 清单 / INV-7/8/9
    ///
    /// 触发场景:`SpecRegistry::register()` 校验失败,或通道 B 否决通过后
    /// 尝试将 spec 加入谱系时被不可进化面守护拒绝。
    ///
    /// WHY 包装而非转换:保留原始 `SpecRegistryError` 上下文(如 ImmutableSpecOverwrite
    /// 的 spec name),供调用方安全审计追溯。
    #[error("不可进化面违反: {reason}")]
    ImmutableSurfaceViolated {
        /// 违反原因描述(含 spec name 与具体违反类型)
        reason: String,
    },

    /// P1-2: M0 形式化验证失败 — Critic 候选未通过单调性/反奖励黑客/有界性检查
    ///
    /// 触发场景: AEGIS Critic 在 CiGate 前运行 M0 守卫时，候选的复合分数
    /// 序列违反 CriticMonotonicityChecker 的三项验证之一。
    /// 处理策略: 拒绝候选（不进入 CiGate），记录违规详情供审计。
    #[error("M0 形式化验证失败: {property} — {detail}")]
    FormalVerificationFailed {
        /// 失败的属性名（如 "critic-monotonicity" / "anti-reward-hacking" / "score-bounded"）
        property: String,
        /// 违规详情（来自 VerificationResult::Violated 的 counterexample）
        detail: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invalid_policy_display() {
        let e = GsoeError::InvalidPolicy {
            reason: "mutation_rate out of range".into(),
        };
        assert!(e.to_string().contains("非法策略参数"));
        assert!(e.to_string().contains("mutation_rate out of range"));
    }

    #[test]
    fn test_mutation_failed_display() {
        let e = GsoeError::MutationFailed {
            reason: "magnitude overflow".into(),
        };
        assert!(e.to_string().contains("变异失败"));
    }

    #[test]
    fn test_config_error_display() {
        let e = GsoeError::ConfigError {
            reason: "bad value".into(),
        };
        assert!(e.to_string().contains("配置错误"));
    }

    #[test]
    fn test_max_generation_display() {
        let e = GsoeError::MaxGenerationReached {
            max_generation: 1000,
        };
        assert!(e.to_string().contains("1000"));
    }

    #[test]
    fn test_ci_gate_execution_failed_display() {
        let e = GsoeError::CiGateExecutionFailed {
            reason: "cargo test subprocess timeout".into(),
        };
        assert!(e.to_string().contains("CI 执行门调用失败"));
        assert!(e.to_string().contains("cargo test subprocess timeout"));
    }

    #[test]
    fn test_delegation_cycle_detected_display() {
        let e = GsoeError::DelegationCycleDetected {
            cycle_path: vec!["A".into(), "B".into(), "A".into()],
        };
        assert!(e.to_string().contains("INV-9"));
        assert!(e.to_string().contains("A"));
        assert!(e.to_string().contains("B"));
    }

    #[test]
    fn test_veto_evidence_insufficient_display() {
        let e = GsoeError::VetoEvidenceInsufficient {
            regression_streak: 2,
            significance: 0.125,
        };
        let msg = e.to_string();
        assert!(msg.contains("否决证据不足"));
        assert!(msg.contains("regression_streak=2"));
        assert!(msg.contains("significance=0.125"));
    }

    #[test]
    fn test_immutable_surface_violated_display() {
        let e = GsoeError::ImmutableSurfaceViolated {
            reason: "spec 'critical-rule' immutable=true".into(),
        };
        assert!(e.to_string().contains("不可进化面违反"));
        assert!(e.to_string().contains("critical-rule"));
    }

    #[test]
    fn test_formal_verification_failed_display() {
        let e = GsoeError::FormalVerificationFailed {
            property: "critic-monotonicity".into(),
            detail: "位置 3: 适应度 0.5→0.6 (不减), 但评分 0.8→0.7 (递减)".into(),
        };
        let msg = e.to_string();
        assert!(msg.contains("M0 形式化验证失败"));
        assert!(msg.contains("critic-monotonicity"));
        assert!(msg.contains("递减"));
    }
}
