//! omega-learner 错误类型 — 库层 thiserror enum
//!
//! 对应任务: **P4-W13.1.3**（LinUCB 算法核心错误处理）
//! 对应 ADR: **ADR-031**（omega-learner 边界）
//!
//! # 设计原则（§4.1 通用约定）
//!
//! - 库层错误用 `thiserror` enum,应用层用 `anyhow`
//! - 错误变体携带足够上下文（dimension/arm/reward）便于诊断
//! - 不实现 `From<std::io::Error>` 等外部错误转换(本 crate 不做 IO)
//! - 数值不稳定错误独立变体（LinUCB 矩阵病态时检测）

use thiserror::Error;

/// omega-learner 错误类型
///
/// # 错误分类
///
/// | 类别 | 变体 | 触发场景 |
/// |------|------|---------|
/// | 参数错误 | `InvalidDimension` / `NoArms` / `InvalidAlpha` / `InvalidReward` | 构造或更新参数越界 |
/// | 维度不匹配 | `ContextDimensionMismatch` | 上下文向量维度与模型维度不一致 |
/// | 索引越界 | `ArmOutOfRange` | `update` 时 arm 索引超过臂数 |
/// | 数值不稳定 | `NumericalInstability` | Sherman-Morrison 分母接近零(矩阵病态) |
/// | 序列化 | `SerializationError` | LinUCB 状态持久化失败 |
#[derive(Debug, Error)]
pub enum LearnerError {
    /// 上下文维度为零(必须 ≥ 1)
    #[error("context dimension must be ≥ 1, got 0")]
    InvalidDimension,

    /// 臂数为零(必须 ≥ 1)
    #[error("number of arms must be ≥ 1, got 0")]
    NoArms,

    /// 探索强度 α 非法(必须 > 0 且有限)
    #[error("alpha must be positive and finite, got {alpha}")]
    InvalidAlpha {
        /// 非法的 α 值
        alpha: f64,
    },

    /// 奖励值非法(NaN / ±Infinity)
    #[error("reward must be finite, got {reward}")]
    InvalidReward {
        /// 非法的奖励值
        reward: f64,
    },

    /// 上下文维度与模型期望维度不匹配
    #[error("context dimension mismatch: expected {expected}, got {actual}")]
    ContextDimensionMismatch {
        /// 模型期望的维度
        expected: usize,
        /// 实际传入的维度
        actual: usize,
    },

    /// 臂索引越界
    #[error("arm index {arm} out of range (total arms: {total})")]
    ArmOutOfRange {
        /// 越界的臂索引
        arm: usize,
        /// 总臂数
        total: usize,
    },

    /// 数值不稳定(Sherman-Morrison 分母 ≤ 0 或非有限)
    ///
    /// WHY 触发条件: LinUCB 的 A_a 矩阵在长期运行后可能病态,
    /// 此时 `1 + x^T A_a^{-1} x` 分母可能 ≤ 0 或 NaN,需中断更新避免污染模型。
    /// 实践中 α 选型合理 + 上下文归一化时几乎不触发,作为防御性错误保留。
    #[error(
        "numerical instability detected: Sherman-Morrison denominator non-positive or non-finite"
    )]
    NumericalInstability,

    /// LinUCB 状态序列化/反序列化失败
    #[error("serialization error: {0}")]
    SerializationError(#[from] serde_json::Error),

    // ----- P4-W16.2.2: R1 离线 RL（CQL/IQL）错误变体 -----
    /// 回放池为空，无法采样训练
    ///
    /// WHY 触发条件: CQL/IQL 训练需要从 `ReplayPool` 采样 mini-batch，
    /// 空池无法提供样本。调用方应先填充 ≥ `RecallQuotaConfig::min_pool_size` 条轨迹再训练。
    #[error("replay pool is empty, cannot sample mini-batch for training")]
    EmptyReplayPool,

    /// 样本数不足，无法训练
    ///
    /// WHY 触发条件: CQL/IQL 训练需要至少 `batch_size` 条样本才能组成一个 mini-batch。
    /// 实际样本数 < batch_size 时，为保持梯度估计稳定性拒绝训练。
    #[error("insufficient samples for training: required {required}, got {actual}")]
    InsufficientSamples {
        /// 需要的最小样本数（通常 = batch_size）
        required: usize,
        /// 实际可用的样本数
        actual: usize,
    },

    /// R1 配置非法（gamma / cql_alpha / iql_tau / lr / l2_reg 等超参越界）
    ///
    /// WHY 触发条件: 超参越界会导致训练不收敛或数值不稳定，
    /// 在 `RecallQuotaConfig::validate()` 中前置检查，避免运行时故障。
    #[error("invalid R1 config: {field} = {value}")]
    InvalidConfig {
        /// 非法字段名（如 "gamma" / "cql_alpha"）
        field: &'static str,
        /// 非法值（字符串化便于诊断）
        value: String,
    },

    /// R1 训练数值不稳定（log-sum-exp 溢出 / 矩阵病态 / 梯度爆炸）
    ///
    /// WHY 触发条件: CQL 的 log-sum-exp 在 Q 值过大时可能溢出；
    /// IQL 的 expectile 回归在 V 与 Q 差距过大时可能发散。
    /// 通过数值稳定实现（减 max_q / 梯度裁剪）防止，但极端情况下仍可能触发。
    #[error("R1 numerical instability: {detail}")]
    R1NumericalInstability {
        /// 不稳定详情（如 "log-sum-exp overflow" / "gradient explosion"）
        detail: &'static str,
    },
}

/// omega-learner Result 别名
pub type Result<T> = std::result::Result<T, LearnerError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_display_invalid_dimension() {
        let err = LearnerError::InvalidDimension;
        assert_eq!(err.to_string(), "context dimension must be ≥ 1, got 0");
    }

    #[test]
    fn test_error_display_no_arms() {
        let err = LearnerError::NoArms;
        assert_eq!(err.to_string(), "number of arms must be ≥ 1, got 0");
    }

    #[test]
    fn test_error_display_invalid_alpha() {
        let err = LearnerError::InvalidAlpha { alpha: -1.5 };
        assert_eq!(
            err.to_string(),
            "alpha must be positive and finite, got -1.5"
        );
    }

    #[test]
    fn test_error_display_invalid_reward() {
        let err = LearnerError::InvalidReward { reward: f64::NAN };
        assert!(err.to_string().contains("NaN"));
    }

    #[test]
    fn test_error_display_dimension_mismatch() {
        let err = LearnerError::ContextDimensionMismatch {
            expected: 8,
            actual: 4,
        };
        assert!(err.to_string().contains("expected 8"));
        assert!(err.to_string().contains("got 4"));
    }

    #[test]
    fn test_error_display_arm_out_of_range() {
        let err = LearnerError::ArmOutOfRange { arm: 5, total: 3 };
        assert!(err.to_string().contains("arm index 5"));
        assert!(err.to_string().contains("total arms: 3"));
    }

    #[test]
    fn test_error_display_numerical_instability() {
        let err = LearnerError::NumericalInstability;
        assert!(err.to_string().contains("numerical instability"));
    }

    #[test]
    fn test_error_from_serde_json() {
        let json_err = serde_json::from_str::<i32>("not a number").unwrap_err();
        let learner_err: LearnerError = json_err.into();
        assert!(matches!(learner_err, LearnerError::SerializationError(_)));
    }

    #[test]
    fn test_result_alias_compiles() {
        fn _returns_result() -> Result<u32> {
            Ok(42)
        }
        assert_eq!(_returns_result().unwrap(), 42);
    }
}
