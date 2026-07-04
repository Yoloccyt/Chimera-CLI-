//! SCC 错误类型 — 库层 thiserror enum
//!
//! 对应架构层:L3 Storage
//!
//! # 设计决策(WHY)
//! - 使用 `thiserror` 而非 `anyhow`:库层错误需明确变体,便于调用方按错误类型决策
//! - 4 个变体覆盖 SCC 的所有失败场景(缓存未命中、预取失败、模式未找到、WAL 错误)
//! - `CacheMiss` 携带 context_id 字符串,便于调用方定位缺失的上下文
//! - `PrefetchFailed` 携带 reason 字符串,描述预取失败的具体原因
//! - `WalError` 携带 reason 字符串,覆盖 WAL 写入/提交/回滚三类失败(Task 9.2 新增)

use thiserror::Error;

/// SCC 引擎错误类型 — 覆盖推测上下文缓存的所有失败场景
#[derive(Debug, Error)]
pub enum SccError {
    /// 缓存未命中:请求的上下文不在缓存中
    #[error("缓存未命中: 上下文 {context_id}")]
    CacheMiss {
        /// 未命中的上下文 ID
        context_id: String,
    },

    /// 推测性预取失败:预取过程中发生错误
    #[error("预取失败: {reason}")]
    PrefetchFailed {
        /// 失败原因描述
        reason: String,
    },

    /// 访问模式未找到:指定上下文无历史转移记录
    #[error("访问模式未找到: 上下文 {context_id}")]
    PatternNotFound {
        /// 未找到模式的上下文 ID
        context_id: String,
    },

    /// WAL 操作失败:写入/提交/回滚日志时发生错误(Task 9.2 新增)
    #[error("WAL 错误: {reason}")]
    WalError {
        /// 失败原因描述(含 entry_id 与具体失败阶段)
        reason: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cache_miss_display() {
        let err = SccError::CacheMiss {
            context_id: "ctx-1".into(),
        };
        assert!(err.to_string().contains("ctx-1"));
    }

    #[test]
    fn test_prefetch_failed_display() {
        let err = SccError::PrefetchFailed {
            reason: "backing store unavailable".into(),
        };
        assert!(err.to_string().contains("backing store unavailable"));
    }

    #[test]
    fn test_pattern_not_found_display() {
        let err = SccError::PatternNotFound {
            context_id: "ctx-2".into(),
        };
        assert!(err.to_string().contains("ctx-2"));
    }
}
