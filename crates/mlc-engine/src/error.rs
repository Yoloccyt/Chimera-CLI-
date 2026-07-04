//! MLC 错误类型 — 库层 thiserror enum
//!
//! 对应架构层:L2 Memory
//!
//! # 设计决策(WHY)
//! - 使用 `thiserror` 而非 `anyhow`:库层错误需明确变体,便于调用方按错误类型决策
//! - 8 个变体覆盖四级记忆的所有失败场景,不引入多余抽象
//! - `StorageError` 包装 `rusqlite::Error`,提供 SQLite 持久化失败的上下文
//! - `EventBusError` 包装 `event_bus::EventBusError`,跨层通信失败时向上传播

use thiserror::Error;

/// MLC 引擎错误类型 — 覆盖四级记忆的所有失败场景
#[derive(Debug, Error)]
pub enum MlcError {
    /// 记忆条目未找到(按 ID 或模式签名查询时)
    #[error("记忆条目未找到: {0}")]
    EntryNotFound(String),

    /// 层级容量溢出(插入超出 capacity 且驱逐失败或被禁用)
    #[error("层级 {tier} 容量溢出: 当前 {current}, 上限 {capacity}")]
    TierOverflow {
        /// 溢出的层级名称
        tier: String,
        /// 当前条目数
        current: usize,
        /// 配置的容量上限
        capacity: usize,
    },

    /// CLV 向量维度不匹配(L2 语义记忆插入时校验)
    #[error("CLV 向量维度不匹配: 期望 {expected}, 实际 {actual}")]
    VectorDimensionMismatch {
        /// 期望维度(512)
        expected: usize,
        /// 实际维度
        actual: usize,
    },

    /// 序列化/反序列化失败(JSON 或 MessagePack)
    #[error("序列化失败: {0}")]
    SerializationFailed(String),

    /// 存储错误(SQLite 持久化层失败)
    #[error("存储错误: {0}")]
    StorageError(String),

    /// 模式冲突(L3 程序记忆插入时,模式签名已存在且 output 不同)
    #[error("模式冲突: 签名 {signature} 已存在且产出不同")]
    PatternConflict {
        /// 冲突的模式签名(序列化字符串)
        signature: String,
    },

    /// 无效配置(容量为 0、路径非法等)
    #[error("无效配置: {0}")]
    InvalidConfig(String),

    /// 事件总线错误(发布事件失败)
    #[error("事件总线错误: {0}")]
    EventBusError(String),
}

impl From<rusqlite::Error> for MlcError {
    /// 将 rusqlite 错误转换为 MlcError::StorageError
    fn from(e: rusqlite::Error) -> Self {
        Self::StorageError(e.to_string())
    }
}

impl From<serde_json::Error> for MlcError {
    /// 将 serde_json 错误转换为 MlcError::SerializationFailed
    fn from(e: serde_json::Error) -> Self {
        Self::SerializationFailed(e.to_string())
    }
}

impl From<event_bus::EventBusError> for MlcError {
    /// 将 EventBus 错误转换为 MlcError::EventBusError
    fn from(e: event_bus::EventBusError) -> Self {
        Self::EventBusError(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entry_not_found_display() {
        let err = MlcError::EntryNotFound("m-1".into());
        assert!(err.to_string().contains("m-1"));
    }

    #[test]
    fn test_tier_overflow_display() {
        let err = MlcError::TierOverflow {
            tier: "L0".into(),
            current: 65,
            capacity: 64,
        };
        let msg = err.to_string();
        assert!(msg.contains("L0"));
        assert!(msg.contains("65"));
        assert!(msg.contains("64"));
    }

    #[test]
    fn test_vector_dimension_mismatch_display() {
        let err = MlcError::VectorDimensionMismatch {
            expected: 512,
            actual: 256,
        };
        assert!(err.to_string().contains("512"));
        assert!(err.to_string().contains("256"));
    }

    #[test]
    fn test_pattern_conflict_display() {
        let err = MlcError::PatternConflict {
            signature: "sig-1".into(),
        };
        assert!(err.to_string().contains("sig-1"));
    }

    #[test]
    fn test_from_rusqlite_error() {
        let sqlite_err = rusqlite::Error::InvalidQuery;
        let mlc_err: MlcError = sqlite_err.into();
        assert!(matches!(mlc_err, MlcError::StorageError(_)));
    }

    #[test]
    fn test_from_serde_json_error() {
        let json_err = serde_json::from_str::<String>("not a string").unwrap_err();
        let mlc_err: MlcError = json_err.into();
        assert!(matches!(mlc_err, MlcError::SerializationFailed(_)));
    }
}
