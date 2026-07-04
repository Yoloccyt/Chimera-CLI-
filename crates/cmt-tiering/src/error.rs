//! CMT 错误类型 — 库层 thiserror enum
//!
//! 对应架构层:L3 Storage
//!
//! # 设计决策(WHY)
//! - 使用 `thiserror` 而非 `anyhow`:库层错误需明确变体,便于调用方按错误类型决策
//! - 7 个变体覆盖四级存储的所有失败场景,不引入多余抽象
//! - `StorageError` 包装 `rusqlite::Error` 与文件 I/O 错误,提供持久化失败的上下文
//! - `EventBusError` 包装 `event_bus::EventBusError`,跨层通信失败时向上传播
//! - `MigrationFailed` 携带源层与目标层信息,便于定位迁移链路问题

use thiserror::Error;

/// CMT 引擎错误类型 — 覆盖四级存储的所有失败场景
#[derive(Debug, Error)]
pub enum CmtError {
    /// 能力条目未找到(按 ID 查询时)
    #[error("能力条目未找到: {0}")]
    EntryNotFound(String),

    /// 层级容量溢出(插入超出 capacity 且驱逐失败或被禁用)
    #[error("层级 {tier} 容量溢出: 当前 {current}, 上限 {capacity}")]
    TierFull {
        /// 溢出的层级名称
        tier: String,
        /// 当前条目数
        current: usize,
        /// 配置的容量上限
        capacity: usize,
    },

    /// 存储错误(SQLite 持久化层失败或文件 I/O 错误)
    #[error("存储错误: {0}")]
    StorageError(String),

    /// 迁移失败(跨层迁移时源层读取或目标层写入失败)
    #[error("迁移失败: {from} -> {to}, 原因: {reason}")]
    MigrationFailed {
        /// 源层级名称
        from: String,
        /// 目标层级名称
        to: String,
        /// 失败原因
        reason: String,
    },

    /// 衰减计算失败(时间戳解析或数学计算异常)
    #[error("衰减计算失败: {0}")]
    DecayFailed(String),

    /// 无效配置(容量为 0、路径非法等)
    #[error("无效配置: {0}")]
    InvalidConfig(String),

    /// 事件总线错误(发布事件失败)
    #[error("事件总线错误: {0}")]
    EventBusError(String),
}

impl From<rusqlite::Error> for CmtError {
    /// 将 rusqlite 错误转换为 CmtError::StorageError
    fn from(e: rusqlite::Error) -> Self {
        Self::StorageError(e.to_string())
    }
}

impl From<serde_json::Error> for CmtError {
    /// 将 serde_json 错误转换为 CmtError::StorageError
    ///
    /// WHY:JSON 序列化失败在 CMT 中主要发生在 SQLite 存取条目时,
    /// 归类为 StorageError 而非单独变体,避免变体过多
    fn from(e: serde_json::Error) -> Self {
        Self::StorageError(format!("JSON 序列化失败: {e}"))
    }
}

impl From<std::io::Error> for CmtError {
    /// 将 std::io 错误转换为 CmtError::StorageError
    ///
    /// WHY:Ice 层使用文件存储,文件读写错误归类为 StorageError
    fn from(e: std::io::Error) -> Self {
        Self::StorageError(format!("文件 I/O 错误: {e}"))
    }
}

impl From<event_bus::EventBusError> for CmtError {
    /// 将 EventBus 错误转换为 CmtError::EventBusError
    fn from(e: event_bus::EventBusError) -> Self {
        Self::EventBusError(e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entry_not_found_display() {
        let err = CmtError::EntryNotFound("cap-1".into());
        assert!(err.to_string().contains("cap-1"));
    }

    #[test]
    fn test_tier_full_display() {
        let err = CmtError::TierFull {
            tier: "Hot".into(),
            current: 257,
            capacity: 256,
        };
        let msg = err.to_string();
        assert!(msg.contains("Hot"));
        assert!(msg.contains("257"));
        assert!(msg.contains("256"));
    }

    #[test]
    fn test_migration_failed_display() {
        let err = CmtError::MigrationFailed {
            from: "Hot".into(),
            to: "Warm".into(),
            reason: "sqlite locked".into(),
        };
        let msg = err.to_string();
        assert!(msg.contains("Hot"));
        assert!(msg.contains("Warm"));
        assert!(msg.contains("sqlite locked"));
    }

    #[test]
    fn test_from_rusqlite_error() {
        let sqlite_err = rusqlite::Error::InvalidQuery;
        let cmt_err: CmtError = sqlite_err.into();
        assert!(matches!(cmt_err, CmtError::StorageError(_)));
    }

    #[test]
    fn test_from_serde_json_error() {
        let json_err = serde_json::from_str::<String>("not a string").unwrap_err();
        let cmt_err: CmtError = json_err.into();
        assert!(matches!(cmt_err, CmtError::StorageError(_)));
    }

    #[test]
    fn test_from_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "file missing");
        let cmt_err: CmtError = io_err.into();
        assert!(matches!(cmt_err, CmtError::StorageError(_)));
    }
}
