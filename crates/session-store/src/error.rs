//! 错误类型 — 库层 thiserror enum（§4.1 通用约定）
//!
//! 对应架构层: **L3 Storage**（session-store,Phase 2 新增,ADR-141）
//! 对应任务: **P2-T2** 会话持久化升级（append-only 段 + CBMR 微批写,手册 W9 T-07）
//!
//! # 设计决策（WHY）
//!
//! - 使用 `thiserror` 而非 `anyhow`:库层错误需明确变体,便于调用方按错误类型决策
//!   （与 scc-cache `SccError` 同构,scc-cache/src/error.rs 先例）
//! - 变体覆盖存储面三类失败:
//!   - `Io`:段文件系统 IO 失败（写/读/fsync/截断）
//!   - `WalCorrupt`:段文件尾部损坏（长度前缀指示长度超过实际剩余字节）
//!   - `OffsetMismatch`:Offset 序列不连续（调用方传错起始序列号 / 恢复校验失败）
//!   - `Sqlite`:SQLite 树索引操作失败（PRAGMA/建表/insert/query）
//!   - `Serialization`:rmp-serde / serde_json 序列化失败
//!   - `Join`:`spawn_blocking` join 错误（阻塞池任务 panic 或被取消）
//!   - `LockPoisoned`:std Mutex 中毒（持锁线程 panic）
//!   - `InvalidInput`:非法入参（空目录、非法 ID 等）
//!   - `NotFound`:查询目标不存在（会话/段/事件未找到）
//!   - `ForkViolation`:fork 语义违规（from_offset 超出父会话历史覆盖范围）
//!
//! # 红线
//!
//! - 库代码禁 unwrap/expect:所有失败经本 enum 传播（thiserror 链）

use thiserror::Error;

/// 会话存储错误类型 — 覆盖 append-only 段 + SQLite 树索引的全部失败场景
#[derive(Debug, Error)]
pub enum StoreError {
    /// 文件系统 IO 失败（段文件写/读/fsync/截断/目录创建）
    #[error("IO 错误: {context} (source: {source})")]
    Io {
        /// 失败上下文描述（文件路径 + 操作名）
        context: String,
        /// 底层 IO 错误
        #[source]
        source: std::io::Error,
    },

    /// 段文件尾部损坏 — 长度前缀指示的数据长度超过实际剩余字节
    ///
    /// WHY:append-only 段以「长度前缀 + JSON 数据」为记录格式,崩溃若发生在
    /// 写数据中途,恢复扫描会发现剩余字节不足（半条）,按设计应截断而非报错;
    /// 仅在截断本身失败或记录内部结构违反长度前缀时抛出本变体。
    #[error("段文件损坏: {path} — {reason}")]
    WalCorrupt {
        /// 损坏的段文件路径
        path: String,
        /// 损坏原因描述（如"第 3 条记录长度前缀越界"）
        reason: String,
    },

    /// Offset 序列不连续 — 期望的起始序列号与恢复出的实际值不一致
    #[error("Offset 不连续: 期望 {expected}, 实际 {actual}")]
    OffsetMismatch {
        /// 期望值（调用方传入的起始序列号）
        expected: u64,
        /// 实际值（段文件恢复扫描出的序列号）
        actual: u64,
    },

    /// SQLite 树索引操作失败
    #[error("SQLite 错误: {reason}")]
    Sqlite {
        /// 失败原因描述（含操作名与底层错误）
        reason: String,
    },

    /// 序列化失败（rmp-serde 或 serde_json）
    #[error("序列化错误: {reason}")]
    Serialization {
        /// 失败原因描述
        reason: String,
    },

    /// spawn_blocking join 错误 — 阻塞池任务 panic 或被取消
    #[error("spawn_blocking join 错误: {reason}")]
    Join {
        /// 失败原因描述
        reason: String,
    },

    /// std Mutex 锁中毒 — 持锁线程 panic
    #[error("锁中毒: {reason}")]
    LockPoisoned {
        /// 锁归属描述
        reason: String,
    },

    /// 非法入参（空数据目录、非法 ID 等）
    #[error("非法输入: {reason}")]
    InvalidInput {
        /// 非法原因描述
        reason: String,
    },

    /// 查询目标不存在（会话/段/事件未找到）
    #[error("未找到: {what}")]
    NotFound {
        /// 未找到的目标描述
        what: String,
    },

    /// fork 语义违规 — from_offset 超出父会话历史覆盖范围
    #[error("fork 违规: {reason}")]
    ForkViolation {
        /// 违规原因描述
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn store_error_display_io() {
        let err = StoreError::Io {
            context: "写段文件 data/s.0.jsonl".into(),
            source: std::io::Error::other("磁盘已满"),
        };
        let s = err.to_string();
        assert!(s.contains("IO 错误") && s.contains("磁盘已满"));
    }

    #[test]
    fn store_error_display_offset_mismatch() {
        let err = StoreError::OffsetMismatch {
            expected: 8,
            actual: 5,
        };
        assert!(err.to_string().contains("8") && err.to_string().contains("5"));
    }

    #[test]
    fn store_error_display_wal_corrupt() {
        let err = StoreError::WalCorrupt {
            path: "data/s.0.jsonl".into(),
            reason: "长度前缀越界".into(),
        };
        assert!(err.to_string().contains("s.0.jsonl"));
    }

    #[test]
    fn store_error_display_fork_violation() {
        let err = StoreError::ForkViolation {
            reason: "from_offset 超出历史".into(),
        };
        assert!(err.to_string().contains("fork"));
    }
}
