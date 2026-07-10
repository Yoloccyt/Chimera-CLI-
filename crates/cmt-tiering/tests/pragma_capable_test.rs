//! PragmaCapable trait 集成测试 — 迁移自已删除的 nexus-core/src/sqlite_pragma.rs
//!
//! 对应任务:F2.4.3 任务 1(迁移原 3 个测试到下游 cmt-tiering crate)
//! 架构层:L3 Storage(下游使用 rusqlite 的 crate)
//!
//! # 迁移背景
//! F2.3 阶段已将 nexus-core(L1)的 rusqlite 依赖下沉到下游 crate,
//! 原 `sqlite_pragma.rs`(56 行)与其中 3 个测试随之删除。本文件将
//! 这 3 个测试迁移到 cmt-tiering,使用真实 `rusqlite::Connection` +
//! newtype wrapper `PragmaConn<'a>`(由 F2.2.3 在 `src/storage_impl.rs` 实现)。
//!
//! # 测试覆盖(对应原 sqlite_pragma.rs 3 个测试)
//! 1. `test_apply_performance_pragmas_success`:apply 成功返回 Ok(())
//! 2. `test_cache_size_set`:cache_size PRAGMA = -65536(64MB KB 单位)
//! 3. `test_temp_store_memory`:temp_store PRAGMA = 2(MEMORY 数值表示)
//!
//! # 调用模式
//! ```ignore
//! use cmt_tiering::PragmaConn;
//! use nexus_core::apply_performance_pragmas;
//!
//! let conn = rusqlite::Connection::open_in_memory().unwrap();
//! conn.pragma_update(None, "journal_mode", "WAL").unwrap();
//! let wrapper = PragmaConn(&conn);
//! apply_performance_pragmas(&wrapper).unwrap();
//! ```
//!
//! # PRAGMA 数值参考
//! - temp_store:0=DEFAULT, 1=FILE, 2=MEMORY
//! - cache_size:负值表示 KB 单位,正值表示页数;-65536 = 64MB

#![forbid(unsafe_code)]

use cmt_tiering::PragmaConn;
use nexus_core::apply_performance_pragmas;

/// 辅助:构造已设置 WAL 模式的内存数据库连接
///
/// WHY WAL 前置:apply_performance_pragmas 部分依赖 WAL 模式
/// (如 synchronous=NORMAL 在 WAL 下才安全,详见 nexus-core 文档)
fn make_wal_connection() -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory().unwrap();
    conn.pragma_update(None, "journal_mode", "WAL")
        .expect("设置 journal_mode=WAL 应当成功");
    conn
}

/// 测试 1:apply_performance_pragmas 对真实 SQLite 连接成功应用全部 PRAGMA
///
/// 迁移自原 sqlite_pragma.rs test_apply_performance_pragmas_success。
/// 仅验证函数返回 Ok(()),具体 PRAGMA 值由后续测试单独验证。
#[test]
fn test_apply_performance_pragmas_success() {
    let conn = make_wal_connection();
    let wrapper = PragmaConn(&conn);

    let result = apply_performance_pragmas(&wrapper);
    assert!(
        result.is_ok(),
        "apply_performance_pragmas 应当成功,实际: {:?}",
        result
    );
}

/// 测试 2:apply_performance_pragmas 设置 cache_size = -65536(64MB KB 单位)
///
/// 迁移自原 sqlite_pragma.rs test_cache_size_set。
/// 用 `pragma_query_value` 读取实际 PRAGMA 值,严格断言等于 -65536。
#[test]
fn test_cache_size_set() {
    let conn = make_wal_connection();
    let wrapper = PragmaConn(&conn);

    apply_performance_pragmas(&wrapper).expect("apply 应当成功");

    // 查询 PRAGMA cache_size,负值表示 KB 单位(-65536 = 64MB)
    let cache_size: i64 = conn
        .pragma_query_value(None, "cache_size", |row| row.get::<_, i64>(0))
        .expect("查询 cache_size 应当成功");
    assert_eq!(
        cache_size, -65536,
        "cache_size 应当为 -65536(64MB KB 单位),实际: {cache_size}"
    );
}

/// 测试 3:apply_performance_pragmas 设置 temp_store = 2(MEMORY)
///
/// 迁移自原 sqlite_pragma.rs test_temp_store_memory。
/// temp_store 数值:0=DEFAULT, 1=FILE, 2=MEMORY。
#[test]
fn test_temp_store_memory() {
    let conn = make_wal_connection();
    let wrapper = PragmaConn(&conn);

    apply_performance_pragmas(&wrapper).expect("apply 应当成功");

    // 查询 PRAGMA temp_store,2 表示 MEMORY(0=DEFAULT, 1=FILE, 2=MEMORY)
    let temp_store: i64 = conn
        .pragma_query_value(None, "temp_store", |row| row.get::<_, i64>(0))
        .expect("查询 temp_store 应当成功");
    assert_eq!(
        temp_store, 2,
        "temp_store 应当为 2(MEMORY),实际: {temp_store}"
    );
}
