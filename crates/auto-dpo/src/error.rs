//! AutoDPO 错误类型 — 库层错误用 thiserror enum(§4.1)
//!
//! 对应架构层:L5 Knowledge
//! 对应创新点:无(知识层辅助模块)
//!
//! WHY thiserror:库层错误用自定义 enum(§4.1),应用层才用 anyhow。
//! 所有变体携带足够上下文,便于调用方定位问题。

use thiserror::Error;

/// AutoDPO 错误类型
///
/// WHY:AutoDPO 作为偏好对生成器,需要在样本不足、质量过低、配置错误等
/// 场景向调用方传递结构化错误信息。每个变体携带足够上下文用于审计与日志。
#[derive(Debug, Error)]
pub enum AutoDpoError {
    /// 样本不足 — 输入候选数少于 2,无法构造偏好对
    ///
    /// WHY:DPO 至少需要 2 个候选(一个 chosen,一个 rejected),
    /// 少于 2 个无法构造偏好对,携带实际数量便于定位
    #[error("insufficient samples: need at least 2, got {actual}")]
    InsufficientSamples {
        /// 实际输入的候选数
        actual: usize,
    },

    /// 样本质量过低 — 所有候选质量分数均低于阈值
    ///
    /// WHY:低质量样本会污染训练集,必须过滤。携带阈值与最高分便于调参
    #[error("all samples below quality threshold: threshold={threshold}, best_score={best_score}")]
    QualityTooLow {
        /// 质量阈值
        threshold: f32,
        /// 当前批次最高质量分
        best_score: f32,
    },

    /// 偏好对生成失败 — 内部逻辑错误
    ///
    /// WHY:携带原因,便于定位生成逻辑 bug
    #[error("pair generation failed: {reason}")]
    GenerationFailed {
        /// 失败原因(人类可读)
        reason: String,
    },

    /// 配置错误 — 配置项非法(如阈值为负、样本数为 0 等)
    #[error("config error: {detail}")]
    ConfigError {
        /// 配置错误详情
        detail: String,
    },

    // ============================================================
    // P5.1 RHI-CG 通道 A 新增错误变体（ADR-032 决策 1）
    // ============================================================
    /// 评判器调用失败 — LLM 评判器返回错误或不可达
    ///
    /// WHY:P5.1 RHI-CG 通道 A 的 JudgeClient::judge() 失败时返回此错误。
    /// 携带原因便于排查（如 LLM 服务不可达、返回格式非法、超时等）。
    /// 解析失败时使用 InvalidVerdict 而非此变体。
    #[error("judge client invocation failed: {reason}")]
    JudgeFailed {
        /// 失败原因（人类可读，如 "LLM service unreachable" / "timeout after 30s"）
        reason: String,
    },

    /// 评判结果非法 — 评判器返回的 JudgeVerdict 字段越界或逻辑不一致
    ///
    /// WHY:评判器（特别是外部 LLM）可能返回非法数据（如 confidence > 1.0、
    /// winner_score < loser_score）。此错误在 from_adjacent_specs 校验时触发。
    /// 携带字段名与实际值便于定位。
    #[error("invalid judge verdict: field={field}, value={value}")]
    InvalidVerdict {
        /// 非法字段名（如 "confidence" / "winner_score"）
        field: String,
        /// 实际值（已格式化为字符串，避免 f32 精度问题）
        value: String,
    },

    // ============================================================
    // P5.1.3 自比较历史持久化新增错误变体（ADR-044 决策 3）
    // ============================================================
    /// 存储错误 — mlc-engine L2 语义记忆操作失败
    ///
    /// WHY:P5.1.3 `SelfComparisonHistory` 包装 `mlc_engine::SemanticMemory`，
    /// 任何 `insert` / `get` / `recall_by_clv` 失败均向上传播为此错误。
    /// 携带原因便于定位（如 "L2 rwlock poisoned" / "CLV dimension mismatch"）。
    ///
    /// # 与 MlcError 的关系
    /// `MlcError` 的所有变体（EntryNotFound / StorageError / VectorDimensionMismatch 等）
    /// 统一转换为此变体，避免上层依赖 mlc-engine 的具体错误类型。EntryNotFound
    /// 在 `SelfComparisonHistory::get()` 中特判为 `Ok(None)`（语义为"未找到记录"），
    /// 不触发此错误。
    #[error("storage error: {reason}")]
    StorageError {
        /// 失败原因（人类可读，如 "L2 rwlock poisoned" / "CLV dimension mismatch"）
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insufficient_samples_display() {
        let err = AutoDpoError::InsufficientSamples { actual: 1 };
        assert!(err.to_string().contains("1"));
        assert!(err.to_string().contains("2"));
    }

    #[test]
    fn test_quality_too_low_display() {
        let err = AutoDpoError::QualityTooLow {
            threshold: 0.5,
            best_score: 0.3,
        };
        assert!(err.to_string().contains("0.5"));
        assert!(err.to_string().contains("0.3"));
    }

    #[test]
    fn test_generation_failed_display() {
        let err = AutoDpoError::GenerationFailed {
            reason: "no valid pair".into(),
        };
        assert!(err.to_string().contains("no valid pair"));
    }

    #[test]
    fn test_config_error_display() {
        let err = AutoDpoError::ConfigError {
            detail: "threshold negative".into(),
        };
        assert!(err.to_string().contains("threshold negative"));
    }

    // ============================================================
    // P5.1 RHI-CG 通道 A 新增错误变体测试
    // ============================================================

    #[test]
    fn test_judge_failed_display() {
        let err = AutoDpoError::JudgeFailed {
            reason: "LLM service unreachable".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("LLM service unreachable"));
        assert!(msg.contains("judge client invocation failed"));
    }

    #[test]
    fn test_invalid_verdict_display() {
        let err = AutoDpoError::InvalidVerdict {
            field: "confidence".into(),
            value: "1.5".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("confidence"));
        assert!(msg.contains("1.5"));
    }

    // ============================================================
    // P5.1.3 自比较历史持久化新增错误变体测试
    // ============================================================

    #[test]
    fn test_storage_error_display() {
        let err = AutoDpoError::StorageError {
            reason: "L2 rwlock poisoned: lock poisoned".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("storage error"));
        assert!(msg.contains("L2 rwlock poisoned"));
    }

    #[test]
    fn test_storage_error_with_clv_dimension_mismatch() {
        // 模拟 mlc-engine CLV 维度错误转换后的 StorageError
        let err = AutoDpoError::StorageError {
            reason: "CLV dimension mismatch: expected 512, actual 256".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("CLV dimension mismatch"));
        assert!(msg.contains("512"));
        assert!(msg.contains("256"));
    }
}
