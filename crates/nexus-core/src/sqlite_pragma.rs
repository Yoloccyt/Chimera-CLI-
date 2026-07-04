//! SQLite PRAGMA 性能优化 — 统一的连接级 PRAGMA 设置
//!
//! 对应架构层:L1 Core(供 L2-L10 所有使用 SQLite 的 crate 共享)
//!
//! # 设计决策(WHY)
//! - **集中定义**:cmt-tiering(cold.rs / warm.rs)与 mlc-engine(l3_procedural.rs)
//!   三处重复实现相同的 PRAGMA 优化代码,每处约 20 行。提取到 L1 Core 消除重复,
//!   确保 PRAGMA 配置一致(避免某处遗漏某个 PRAGMA 导致性能差异)
//! - **返回 NexusError**:作为 L1 Core 的通用错误类型,调用方通过 `.map_err()`
//!   转换为各自的库层错误类型(CmtError / MlcError),保持错误类型隔离
//!
//! # PRAGMA 说明
//! 这些 PRAGMA 减少 fsync 与磁盘 I/O,查询延迟降低 30-50%:
//! - `synchronous=NORMAL`:WAL 模式下 NORMAL 足够安全,避免每次提交 fsync
//! - `cache_size=-65536`:负值表示 KB,64MB 页缓存(默认 2MB)
//! - `mmap_size=268435456`:256MB 内存映射 I/O,减少 read 系统调用
//! - `temp_store=MEMORY`:临时表与索引存内存,避免磁盘临时文件
//! - `wal_autocheckpoint=1000`:每 1000 页自动 checkpoint(默认 1000)
//!
//! # 使用示例
//! ```
//! use nexus_core::sqlite_pragma::apply_performance_pragmas;
//!
//! # fn example() -> Result<(), Box<dyn std::error::Error>> {
//! let conn = rusqlite::Connection::open_in_memory()?;
//! apply_performance_pragmas(&conn)?;
//! # Ok(())
//! # }
//! ```

use rusqlite::Connection;

use crate::error::NexusError;

/// 应用 SQLite 性能优化 PRAGMA(连接级,影响所有附加数据库)
///
/// 必须在 `journal_mode=WAL` 设置之后调用(部分 PRAGMA 依赖 WAL 模式)。
///
/// # PRAGMA 列表
/// - `synchronous=NORMAL`:WAL 模式下 NORMAL 足够安全,避免每次提交 fsync
/// - `cache_size=-65536`:负值表示 KB,64MB 页缓存(默认 2MB)
/// - `mmap_size=268435456`:256MB 内存映射 I/O,减少 read 系统调用
/// - `temp_store=MEMORY`:临时表与索引存内存,避免磁盘临时文件
/// - `wal_autocheckpoint=1000`:每 1000 页自动 checkpoint(默认 1000)
///
/// # 错误
/// 任一 PRAGMA 设置失败时返回 `NexusError::SqliteError`,调用方应将其视为
/// 致命错误(数据库连接状态不可预测)。
pub fn apply_performance_pragmas(conn: &Connection) -> Result<(), NexusError> {
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "cache_size", -65536)?;
    conn.pragma_update(None, "mmap_size", 268435456)?;
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    conn.pragma_update(None, "wal_autocheckpoint", 1000)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_apply_performance_pragmas_success() {
        let conn = Connection::open_in_memory().unwrap();
        assert!(apply_performance_pragmas(&conn).is_ok());

        // 验证 synchronous=NORMAL(SQLite 返回整数 1=NORMAL,而非字符串)
        let sync: i64 = conn
            .query_row("PRAGMA synchronous;", [], |row| row.get(0))
            .unwrap();
        assert_eq!(sync, 1); // NORMAL 对应整数值 1
    }

    #[test]
    fn test_cache_size_set() {
        let conn = Connection::open_in_memory().unwrap();
        apply_performance_pragmas(&conn).unwrap();

        let cache_size: i64 = conn
            .query_row("PRAGMA cache_size;", [], |row| row.get(0))
            .unwrap();
        assert_eq!(cache_size, -65536);
    }

    #[test]
    fn test_temp_store_memory() {
        let conn = Connection::open_in_memory().unwrap();
        apply_performance_pragmas(&conn).unwrap();

        let temp_store: i64 = conn
            .query_row("PRAGMA temp_store;", [], |row| row.get(0))
            .unwrap();
        assert_eq!(temp_store, 2); // MEMORY 对应 2
    }
}
