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

    /// 进度计数非法 — 已完成数超过总数（物理上不可能的进度快照）
    ///
    /// WHY 新增变体而非静默接受：`update_quest_progress` 的两个进度参数此前
    /// 带 `_` 前缀不参与任何逻辑（死参数），调用方可传入 `completed > total`
    /// 这类物理上不可能的组合而无人看守。赋义后该不变量由类型系统外的
    /// 显式校验守住，且不改变任何既有合法调用点的行为。
    ///
    /// WHY 加变体不破坏下游：本 crate 的 `NexusError` 全仓消费方均以
    /// `matches!` 宏匹配（无裸 `match` 穷尽），不存在非穷尽编译错误面。
    #[error("invalid progress: completed {completed} exceeds total {total}")]
    InvalidProgress {
        /// 调用方声明的已完成数
        completed: u32,
        /// 调用方声明的总任务数
        total: u32,
    },
}

/// 从 serde_json 错误转换 — 快照哈希等场景的序列化失败
impl From<serde_json::Error> for NexusError {
    fn from(err: serde_json::Error) -> Self {
        Self::SerializationError(format!("json: {err}"))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // test-module unwrap is the Rust idiom; E-5 targets production code
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

    #[test]
    fn test_invalid_progress_display() {
        let e = NexusError::InvalidProgress {
            completed: 11,
            total: 10,
        };
        let msg = e.to_string();
        assert!(msg.contains("11"));
        assert!(msg.contains("10"));
    }
}
