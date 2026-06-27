//! Repo Wiki 错误类型 — 库层 thiserror enum
//!
//! 遵循 §4.1:库层用自定义 thiserror enum,应用层才用 anyhow
//! 对应架构层:L5 Knowledge

use thiserror::Error;

/// Repo Wiki 错误枚举 — 覆盖数据库、向量索引、条目生命周期、序列化与 IO
#[derive(Debug, Error)]
pub enum WikiError {
    /// SQLite 数据库错误 — 连接、查询、事务失败
    #[error("database error: {0}")]
    DatabaseError(#[from] rusqlite::Error),

    /// 向量索引错误 — 维度不匹配、KNN 检索失败、内部状态异常
    #[error("vector index error: {0}")]
    VectorIndexError(String),

    /// 条目不存在 — 按 entry_id 查找时未找到
    #[error("entry not found: {0}")]
    EntryNotFound(String),

    /// 锚点悬空 — 引用了不存在的条目(预留,Week 2 暂不触发)
    #[error("anchor dangling: {0}")]
    AnchorDangling(String),

    /// 序列化/反序列化失败 — tags JSON 或 embedding BLOB 编解码错误
    #[error("serialization error: {0}")]
    SerializationError(String),

    /// IO 错误 — 文件读写等底层 IO 失败
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    /// 事件总线错误 — 发布 WikiUpdated 事件失败
    #[error("event bus error: {0}")]
    EventBusError(#[from] event_bus::EventBusError),
}

/// 从 serde_json 错误转换 — tags 序列化/反序列化失败
impl From<serde_json::Error> for WikiError {
    fn from(err: serde_json::Error) -> Self {
        Self::SerializationError(format!("json: {err}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_database_error_display() {
        let io_err = rusqlite::Error::InvalidParameterName("bad".into());
        let wiki_err: WikiError = io_err.into();
        assert!(wiki_err.to_string().contains("database error"));
    }

    #[test]
    fn test_vector_index_error_display() {
        let e = WikiError::VectorIndexError("dim mismatch".into());
        assert!(e.to_string().contains("dim mismatch"));
    }

    #[test]
    fn test_entry_not_found_display() {
        let e = WikiError::EntryNotFound("e-123".into());
        assert!(e.to_string().contains("e-123"));
    }

    #[test]
    fn test_serialization_error_from_json() {
        let json_err = serde_json::from_str::<Vec<String>>("not json").unwrap_err();
        let wiki_err: WikiError = json_err.into();
        assert!(matches!(wiki_err, WikiError::SerializationError(_)));
    }

    #[test]
    fn test_io_error_from() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        let wiki_err: WikiError = io_err.into();
        assert!(matches!(wiki_err, WikiError::IoError(_)));
    }
}
