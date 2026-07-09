//! mlc-engine 的 PragmaCapable 实现 — newtype wrapper 模式
//!
//! WHY newtype wrapper: Rust coherence 规则禁止两个 crate 同时 impl
//! 同一 trait for 同一 type。mlc-engine 和 cmt-tiering 都需要 impl
//! PragmaCapable for rusqlite::Connection,但根 Cargo.toml 的 E2E 测试
//! 同时依赖两者,会触发 `conflicting implementations`。各 crate 用独立
//! newtype wrapper 避免冲突(spec F2.1.5.6 预置回滚方案,用户 2026-07-08 决策采纳)。

use nexus_core::{NexusError, PragmaCapable};

/// PRAGMA 能力 wrapper — 包装 `&rusqlite::Connection` 以实现 PragmaCapable
///
/// 使用借用引用(`'a`),避免移动 Connection 所有权。调用方构造
/// `PragmaConn(&conn)` 后传入 `nexus_core::apply_performance_pragmas`。
pub struct PragmaConn<'a>(pub &'a rusqlite::Connection);

impl<'a> PragmaCapable for PragmaConn<'a> {
    fn pragma_update_string(&self, key: &str, value: &str) -> Result<(), NexusError> {
        self.0.pragma_update(None, key, value).map_err(|e| {
            NexusError::SerializationError(format!("SQLite PRAGMA {key}={value} 失败: {e}"))
        })
    }

    fn pragma_update_int(&self, key: &str, value: i64) -> Result<(), NexusError> {
        self.0.pragma_update(None, key, value).map_err(|e| {
            NexusError::SerializationError(format!("SQLite PRAGMA {key}={value} 失败: {e}"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证 PragmaConn wrapper 能成功对真实 in-memory SQLite 连接设置 PRAGMA
    #[test]
    fn test_pragma_conn_applies_pragmas_to_real_connection() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        // WAL 模式是 apply_performance_pragmas 的前置条件(部分 PRAGMA 依赖 WAL)
        conn.pragma_update(None, "journal_mode", "WAL").unwrap();

        let wrapper = PragmaConn(&conn);
        let result = nexus_core::apply_performance_pragmas(&wrapper);
        assert!(
            result.is_ok(),
            "apply_performance_pragmas 应当成功: {:?}",
            result
        );
    }
}
