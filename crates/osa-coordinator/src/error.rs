//! OSA 错误类型 — 库层 thiserror enum
//!
//! 对应架构层:L6 Router
//!
//! # 设计决策(WHY)
//! - 使用 `thiserror` 而非 `anyhow`:库层错误需明确变体,便于调用方按错误类型决策
//! - 5 个变体覆盖掩码计算的全生命周期:输入校验 → 计算 → 事件发布 → 配置 → 稀疏度
//! - `EventBusError` 包装 `event_bus::EventBusError`,跨层通信失败时向上传播
//! - `SparsityOutOfRange` 专门校验稀疏度边界 [0.0, 1.0],避免下游 NaN 污染

use thiserror::Error;

/// OSA 协调器错误类型 — 覆盖掩码计算的全生命周期失败场景
#[derive(Debug, Error)]
pub enum OsaError {
    /// 无效任务特征(complexity_score 超出 [0.0, 1.0]、候选集为空等)
    #[error("无效任务特征: {0}")]
    InvalidTaskProfile(String),

    /// 掩码计算失败(序列化错误、内部状态不一致等)
    #[error("掩码计算失败: {0}")]
    MaskComputationFailed(String),

    /// 事件总线错误(发布 OmniSparseMasksComputed 事件失败)
    #[error("事件总线错误: {0}")]
    EventBusError(String),

    /// 无效配置(routing_top_k_bounds 下限 > 上限、budget_protection_threshold 超出 [0.0, 1.0] 等)
    #[error("无效配置: {0}")]
    InvalidConfig(String),

    /// 稀疏度超出范围 [0.0, 1.0]
    ///
    /// WHY:稀疏度是掩码的核心指标,超出范围会导致下游(如 HCW)计算异常,
    /// 必须在 OSA 层严格校验并返回明确错误
    #[error("稀疏度超出范围: {value}, 应在 [0.0, 1.0]")]
    SparsityOutOfRange {
        /// 实际稀疏度值
        value: f32,
    },
}

impl From<event_bus::EventBusError> for OsaError {
    /// 将 EventBus 错误转换为 OsaError::EventBusError
    fn from(e: event_bus::EventBusError) -> Self {
        Self::EventBusError(e.to_string())
    }
}

impl From<serde_json::Error> for OsaError {
    /// 将 serde_json 错误转换为 OsaError::MaskComputationFailed
    ///
    /// WHY:JSON 序列化失败发生在 mask_hash 计算阶段,归为掩码计算失败
    fn from(e: serde_json::Error) -> Self {
        Self::MaskComputationFailed(format!("JSON 序列化失败: {e}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invalid_task_profile_display() {
        let err = OsaError::InvalidTaskProfile("complexity_score > 1.0".into());
        assert!(err.to_string().contains("complexity_score > 1.0"));
    }

    #[test]
    fn test_sparsity_out_of_range_display() {
        let err = OsaError::SparsityOutOfRange { value: 1.5 };
        let msg = err.to_string();
        assert!(msg.contains("1.5"));
        assert!(msg.contains("[0.0, 1.0]"));
    }

    #[test]
    fn test_mask_computation_failed_display() {
        let err = OsaError::MaskComputationFailed("sha256 error".into());
        assert!(err.to_string().contains("sha256 error"));
    }

    #[test]
    fn test_from_serde_json_error() {
        let json_err = serde_json::from_str::<String>("not a string").unwrap_err();
        let osa_err: OsaError = json_err.into();
        assert!(matches!(osa_err, OsaError::MaskComputationFailed(_)));
    }
}
