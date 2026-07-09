//! Nexus Core 错误类型 — 库层 thiserror enum
//!
//! 遵循 §4.1:库层用自定义 thiserror enum,应用层才用 anyhow
//! 对应架构层:L1 Core

use thiserror::Error;

/// Nexus Core 错误枚举 — 覆盖维度校验、Quest 生命周期、序列化与 IO
#[derive(Debug, Error)]
pub enum NexusError {
    /// CLV 维度不匹配 — 构造时传入的向量长度不等于 512
    #[error("CLV dimension must be {expected}, got {actual}")]
    InvalidClvDimension {
        /// 期望维度(固定为 512)
        expected: usize,
        /// 实际传入维度
        actual: usize,
    },

    /// Quest 不存在 — 按 ID 查找时未找到
    #[error("quest not found: {0}")]
    QuestNotFound(String),

    /// Quest 已存在 — 重复注册时触发
    #[error("quest already exists: {0}")]
    QuestAlreadyExists(String),

    /// 序列化/反序列化失败 — JSON 或 MessagePack 编解码错误
    #[error("serialization error: {0}")]
    SerializationError(String),

    /// IO 错误 — 文件读写等底层 IO 失败
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

/// 从 serde_json 错误转换 — 快照哈希等场景的序列化失败
impl From<serde_json::Error> for NexusError {
    fn from(err: serde_json::Error) -> Self {
        Self::SerializationError(format!("json: {err}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_invalid_clv_dimension_display() {
        let e = NexusError::InvalidClvDimension {
            expected: 512,
            actual: 256,
        };
        let msg = e.to_string();
        assert!(msg.contains("512"));
        assert!(msg.contains("256"));
    }

    #[test]
    fn test_quest_not_found_display() {
        let e = NexusError::QuestNotFound("q-123".into());
        assert!(e.to_string().contains("q-123"));
    }

    #[test]
    fn test_quest_already_exists_display() {
        let e = NexusError::QuestAlreadyExists("q-456".into());
        assert!(e.to_string().contains("q-456"));
    }

    #[test]
    fn test_io_error_from() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "missing");
        let nexus_err: NexusError = io_err.into();
        assert!(matches!(nexus_err, NexusError::IoError(_)));
    }
}
