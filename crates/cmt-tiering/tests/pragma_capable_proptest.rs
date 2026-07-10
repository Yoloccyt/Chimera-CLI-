//! PragmaCapable trait 属性测试 — 验证 PRAGMA 不变量
//!
//! 对应任务:F2.4.3 任务 2(proptest 验证 PragmaCapable trait 不变量)
//! 架构层:L3 Storage
//!
//! # 验证的不变量
//! 1. **PRAGMA 幂等性**:多次调用 `apply_performance_pragmas`,PRAGMA 值不变
//! 2. **WAL 模式持久性**:设置 WAL + apply → 关闭 → 重开 → journal_mode 仍为 WAL,
//!    且重新 apply 后连接级 PRAGMA 恢复预期值
//!
//! # PRAGMA 持久化语义(WHY 教育性注释)
//! SQLite PRAGMA 分两类:
//! - **数据库级持久化**:`journal_mode=WAL` 修改数据库文件头,跨连接重启仍生效
//! - **连接级临时**:`synchronous`/`cache_size`/`mmap_size`/`temp_store`/`wal_autocheckpoint`
//!   仅当前连接有效,重开连接后需重新 apply
//!
//! 因此持久性测试分两步:
//! 1. 重开后验证 journal_mode=WAL 仍生效(数据库级持久化)
//! 2. 重新 apply 后验证连接级 PRAGMA 恢复预期值
//!
//! # 语法约束(§4.1)
//! proptest 1.11+ 用 block-named 语法:`fn name(arg in strategy) { body }`

#![forbid(unsafe_code)]

use cmt_tiering::PragmaConn;
use nexus_core::apply_performance_pragmas;
use proptest::prelude::*;
use tempfile::tempdir;

/// 辅助:构造已设置 WAL 模式的内存数据库连接
fn make_wal_connection() -> rusqlite::Connection {
    let conn = rusqlite::Connection::open_in_memory()
        .unwrap_or_else(|e| panic!("open_in_memory 应当成功: {e}"));
    conn.pragma_update(None, "journal_mode", "WAL")
        .unwrap_or_else(|e| panic!("设置 journal_mode=WAL 应当成功: {e}"));
    conn
}

/// 辅助:查询指定 PRAGMA 的 i64 值
fn query_pragma_i64(conn: &rusqlite::Connection, key: &str) -> i64 {
    conn.pragma_query_value(None, key, |row| row.get::<_, i64>(0))
        .unwrap_or_else(|e| panic!("查询 PRAGMA {key} 应当成功: {e}"))
}

proptest! {
    /// 不变量 1:PRAGMA 幂等性 — 多次调用 apply_performance_pragmas,PRAGMA 值不变
    ///
    /// 策略:生成 n_calls ∈ [1, 10],重复调用 n 次,每次后查询 cache_size 与
    /// temp_store,断言值始终为 -65536 与 2(MEMORY)。
    ///
    /// WHY 重要:幂等性是 PragmaCapable trait 的核心契约 —— 调用方可以安全地
    /// 在连接复用场景(如连接池)多次调用,不会产生累积副作用。
    ///
    /// WHY 仅验证 cache_size 与 temp_store:这两个是查询型 PRAGMA,在内存数据库中
    /// 用 `pragma_query_value` 能稳定返回行。`mmap_size` 在 `:memory:` 数据库中
    /// 查询返回 no rows(内存数据库不使用 mmap),故不纳入幂等性验证。
    /// cache_size 与 temp_store 已足够代表连接级 PRAGMA 的幂等行为。
    #[test]
    fn test_pragma_idempotent(n_calls in 1u32..=10u32) {
        let conn = make_wal_connection();
        let wrapper = PragmaConn(&conn);

        for i in 0..n_calls {
            let result = apply_performance_pragmas(&wrapper);
            prop_assert!(
                result.is_ok(),
                "第 {} 次调用 apply_performance_pragmas 应当成功,实际: {:?}",
                i + 1,
                result
            );

            // 每次调用后查询,验证值不变
            let cache_size = query_pragma_i64(&conn, "cache_size");
            prop_assert_eq!(
                cache_size, -65536,
                "第 {} 次调用后 cache_size 应保持 -65536,实际: {}",
                i + 1,
                cache_size
            );

            let temp_store = query_pragma_i64(&conn, "temp_store");
            prop_assert_eq!(
                temp_store, 2,
                "第 {} 次调用后 temp_store 应保持 2(MEMORY),实际: {}",
                i + 1,
                temp_store
            );
        }
    }
}

proptest! {
    /// 不变量 2:WAL 模式持久性 — 设置 WAL + apply → 关闭 → 重开 →
    /// journal_mode 仍为 WAL(数据库级持久化),且重新 apply 后连接级 PRAGMA 恢复
    ///
    /// 策略:用 tempfile 创建临时目录,在其中创建数据库文件。proptest 生成
    /// 一个无关的 seed(用于触发多次独立运行,每次用不同临时目录)。
    ///
    /// WHY 文件数据库而非内存:内存数据库(`:memory:`)关闭后数据全部丢失,
    /// 无法验证持久化。文件数据库 + WAL 模式才能体现 journal_mode 的持久化语义。
    #[test]
    fn test_wal_persistence_across_reopen(_seed in 0u32..5u32) {
        // tempfile::tempdir 在 drop 时自动清理整个目录(RAII),避免污染
        let dir = tempdir().expect("创建临时目录应当成功");
        let db_path = dir.path().join("pragma_persistence.db");

        // 阶段 1:首次打开,设置 WAL + apply
        {
            let conn = rusqlite::Connection::open(&db_path)
                .expect("首次打开文件数据库应当成功");
            conn.pragma_update(None, "journal_mode", "WAL")
                .expect("设置 journal_mode=WAL 应当成功");
            let wrapper = PragmaConn(&conn);
            apply_performance_pragmas(&wrapper)
                .expect("首次 apply_performance_pragmas 应当成功");

            // 验证首次设置成功
            let cache_size = query_pragma_i64(&conn, "cache_size");
            prop_assert_eq!(cache_size, -65536, "首次设置后 cache_size 应为 -65536");
            let temp_store = query_pragma_i64(&conn, "temp_store");
            prop_assert_eq!(temp_store, 2, "首次设置后 temp_store 应为 2(MEMORY)");
        } // conn drop,数据库关闭

        // 阶段 2:重新打开同一文件,验证 journal_mode=WAL 持久化
        {
            let conn = rusqlite::Connection::open(&db_path)
                .expect("重新打开文件数据库应当成功");

            // journal_mode=WAL 是数据库级持久化(修改文件头),应保持
            let journal_mode: String = conn
                .pragma_query_value(None, "journal_mode", |row| row.get::<_, String>(0))
                .expect("查询 journal_mode 应当成功");
            prop_assert_eq!(
                journal_mode.to_lowercase(),
                "wal",
                "重开连接后 journal_mode 应保持 WAL(数据库级持久化),实际: {}",
                journal_mode
            );

            // WHY 连接级 PRAGMA 不持久化:cache_size/temp_store 等仅当前连接有效,
            // 重开后会恢复默认值(如 cache_size 默认 -2000,temp_store 默认 0/1)。
            // 这里不严格断言默认值(因 SQLite 版本/编译选项可能差异),
            // 只验证它们不等于 apply 后的目标值,证明需要重新 apply。
            let cache_size_before = query_pragma_i64(&conn, "cache_size");
            prop_assert_ne!(
                cache_size_before, -65536,
                "重开连接后 cache_size 应恢复默认(非 -65536),实际: {}",
                cache_size_before
            );

            // 阶段 3:重新 apply,验证连接级 PRAGMA 恢复预期值
            let wrapper = PragmaConn(&conn);
            apply_performance_pragmas(&wrapper)
                .expect("重新 apply_performance_pragmas 应当成功");

            let cache_size_after = query_pragma_i64(&conn, "cache_size");
            prop_assert_eq!(
                cache_size_after, -65536,
                "重新 apply 后 cache_size 应恢复 -65536,实际: {}",
                cache_size_after
            );

            let temp_store_after = query_pragma_i64(&conn, "temp_store");
            prop_assert_eq!(
                temp_store_after, 2,
                "重新 apply 后 temp_store 应恢复 2(MEMORY),实际: {}",
                temp_store_after
            );

            // WHY 仅验证 cache_size 与 temp_store:与幂等性测试一致,这两个查询型
            // PRAGMA 能稳定返回行。mmap_size 在某些 SQLite 配置下查询行为不稳定,
            // 不纳入持久性验证。cache_size 与 temp_store 已足够证明连接级 PRAGMA
            // 在重新 apply 后恢复预期值。
        }
    }
}
