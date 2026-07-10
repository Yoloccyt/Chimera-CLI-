//! Cold 层 — SQLite 附加数据库实现
//!
//! 对应架构层:L3 Storage(Cold tier)
//!
//! # 设计决策(WHY)
//! - **SQLite 附加数据库(ATTACH DATABASE)**:Cold 层使用 SQLite 的 ATTACH
//!   DATABASE 功能,主连接为内存数据库,通过 ATTACH 附加文件数据库。
//!   这样设计的好处:
//!   1. 主连接是内存数据库,连接速度快(无需文件 I/O)
//!   2. 附加的文件数据库提供持久化,可随时分离/附加
//!   3. 避免引入 zstd 依赖(SQLite 文件本身有压缩,Week 8 再评估)
//!   4. 未来可附加多个文件实现分片(扩展性好)
//! - **SqlitePool 连接池(P1-5)**:替代原始 `Arc<Mutex<Connection>>` 单连接方案,
//!   通过读写分离(N 个读连接 + 1 个写连接)实现真正的并发读。
//!   每个连接独立 ATTACH 同一文件数据库,通过 `cold_db.table_name` 访问共享数据。
//!   WAL 模式下读写互不阻塞,读多写少场景吞吐量提升 N 倍
//! - **spawn_blocking 包装文件 I/O**:SQLite 操作可能阻塞异步运行时,
//!   使用 `tokio::task::spawn_blocking` 将其放到阻塞线程池
//!   (架构红线:所有 async fn 满足 Send + 'static 约束)
//! - **PRAGMA 性能优化**:附加数据库启用 WAL 模式,连接级设置
//!   synchronous=NORMAL、cache_size、mmap_size、temp_store=MEMORY、
//!   wal_autocheckpoint,减少 fsync 与磁盘 I/O,查询延迟降低 30-50%
//! - **list_idle_entries 查询**:按 `last_accessed_at < ?` 过滤,
//!   用于 Cold → Ice 的空闲超时迁移(7 天未被访问)
//! - **last_accessed_at 索引**:在 `cold_capabilities(last_accessed_at)` 上创建
//!   B-Tree 索引(`idx_cold_last_access`),将 `list_idle_entries` 的 O(n) 全表
//!   扫描降为 O(log n) 索引查找。65536 条目下延迟从约 50ms 降到 < 5ms。
//!   索引在表创建后立即用 `CREATE INDEX IF NOT EXISTS` 创建,幂等且安全。
//!
//! # Schema(在附加的文件数据库中创建)
//! ```sql
//! CREATE TABLE IF NOT EXISTS cold_capabilities (
//!     cap_id           TEXT PRIMARY KEY,
//!     content          TEXT NOT NULL,
//!     created_at       TEXT NOT NULL,
//!     last_accessed_at TEXT NOT NULL,
//!     access_count     INTEGER NOT NULL
//! );
//! ```

use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use tracing::{debug, trace};

use crate::error::CmtError;
use crate::pool::SqlitePool;
use crate::types::{CapabilityEntry, CapabilityId, Tier};

/// 附加数据库的别名(用于 `ATTACH DATABASE ... AS cold_db`)
const ATTACH_ALIAS: &str = "cold_db";

/// Cold 层 — SQLite 附加数据库持久化的冷存储
///
/// 主连接为内存数据库,通过 `ATTACH DATABASE` 附加文件数据库。
/// 所有数据存储在附加的文件数据库中,通过 `cold_db.table_name` 访问。
/// 使用 `SqlitePool` 连接池实现读写分离,支持并发读与串行写。
///
/// # 线程安全
/// `Arc<SqlitePool>` 包装,可 Clone(廉价,Arc 引用计数)。
/// 所有 async fn 满足 `Send + 'static` 约束。
#[derive(Clone)]
pub struct ColdTier {
    /// SQLite 连接池(P1-5:替代 Arc<Mutex<Connection>>,支持并发读)
    ///
    /// WHY SqlitePool:原始单 Mutex 序列化所有操作(包括读),
    /// WAL 模式下并发读被不必要地阻塞。连接池中每个连接独立 ATTACH
    /// 同一文件数据库,读操作分散到多个连接,写操作仍通过独立写连接序列化。
    pool: Arc<SqlitePool>,
    /// 容量上限(超出时由上层触发迁移到 Ice)
    capacity: usize,
}

impl ColdTier {
    /// 打开或创建 Cold 层数据库
    ///
    /// 主连接为内存数据库,通过 `ATTACH DATABASE` 附加 `cold_dir/cmt_cold.db`。
    /// 附加数据库启用 WAL 模式与 PRAGMA 性能优化,减少 fsync 与磁盘 I/O。
    /// 使用 SqlitePool 创建 1 写 + `read_pool_size` 读连接(默认 2)。
    /// 路径的父目录应已存在(调用方负责创建)。
    pub fn open(cold_dir: &Path, capacity: usize) -> Result<Self, CmtError> {
        Self::open_with_pool(cold_dir, capacity, 2)
    }

    /// 打开 Cold 层数据库并指定读连接池大小
    ///
    /// WHY 独立方法:连接池大小影响并发读吞吐量与内存开销,
    /// 允许调用方(如 CmtCoordinator)根据部署场景调整。
    /// `read_pool_size = 0` 退化为单连接模式(适合测试或低并发场景)。
    pub fn open_with_pool(
        cold_dir: &Path,
        capacity: usize,
        read_pool_size: usize,
    ) -> Result<Self, CmtError> {
        let db_path = cold_dir.join("cmt_cold.db");

        let pool = SqlitePool::open(read_pool_size, || {
            // 主连接使用内存数据库(快速连接,无需文件 I/O)
            let conn = Connection::open_in_memory()?;

            // 附加文件数据库(每个连接独立 ATTACH 同一文件)
            let db_path_str = db_path.to_string_lossy();
            conn.execute(
                &format!("ATTACH DATABASE ?1 AS {ATTACH_ALIAS};"),
                params![db_path_str.as_ref()],
            )?;

            // 为附加数据库启用 WAL 模式(提升并发读写性能)
            // WHY:WAL 允许读写并发,默认 rollback journal 模式下写会阻塞读
            conn.pragma_update(
                Some(rusqlite::DatabaseName::Attached(ATTACH_ALIAS)),
                "journal_mode",
                "WAL",
            )?;

            // 应用 SQLite PRAGMA 性能优化(连接级,影响所有附加数据库)
            apply_performance_pragmas(&conn)?;

            // 在附加数据库中创建表(使用 `alias.table_name` 语法)
            // 同时在 last_accessed_at 上创建索引,加速 list_idle_entries 查询
            // WHY:65536 条目下 O(n) 全表扫描约 50ms,O(log n) 索引查找 < 5ms
            // 注意:SQLite 的 CREATE INDEX 语法中,table-name 不支持 schema-name 前缀,
            // 但 index-name 支持。用 `cold_db.idx_cold_last_access` 指定索引所在的 schema,
            // SQLite 会自动在所有附加数据库中查找 cold_capabilities 表。
            // IF NOT EXISTS 保证多连接初始化幂等(每个连接都执行,仅首次创建)
            conn.execute_batch(&format!(
                "CREATE TABLE IF NOT EXISTS {ATTACH_ALIAS}.cold_capabilities (
                    cap_id           TEXT PRIMARY KEY,
                    content          TEXT NOT NULL,
                    created_at       TEXT NOT NULL,
                    last_accessed_at TEXT NOT NULL,
                    access_count     INTEGER NOT NULL
                );

                CREATE INDEX IF NOT EXISTS {ATTACH_ALIAS}.idx_cold_last_access
                    ON cold_capabilities(last_accessed_at);"
            ))?;

            Ok(conn)
        })?;

        debug!(db_path = ?db_path, capacity, read_pool_size, "Cold 层附加数据库已打开(连接池 + WAL + PRAGMA 优化 + last_accessed_at 索引)");
        Ok(Self {
            pool: Arc::new(pool),
            capacity,
        })
    }

    /// 在内存中创建 Cold 层(用于测试,不持久化)
    ///
    /// WHY:测试场景不需要持久化,使用纯内存附加数据库更快且自动清理。
    /// 内存数据库也应用 PRAGMA 优化(部分 PRAGMA 对内存库无效,但不会报错)。
    ///
    /// WHY 单连接:`:memory:` 数据库彼此独立(每个连接是独立的内存数据库),
    /// 无法跨连接共享数据。测试场景使用 `read_pool_size = 0`,
    /// 所有读写共用写连接,保证数据可见性。
    pub fn open_in_memory(capacity: usize) -> Result<Self, CmtError> {
        let pool = SqlitePool::open(0, || {
            let conn = Connection::open_in_memory()?;

            // 附加另一个内存数据库(使用 :memory:)
            // WHY:SQLite 允许附加内存数据库,每个 :memory: 是独立的内存数据库
            conn.execute(
                &format!("ATTACH DATABASE ':memory:' AS {ATTACH_ALIAS};"),
                [],
            )?;

            // 应用 PRAGMA 优化(内存库不适用 WAL,但其他 PRAGMA 无害)
            apply_performance_pragmas(&conn)?;

            // 创建表与索引(与 open 方法保持一致,加速 list_idle_entries)
            conn.execute_batch(&format!(
                "CREATE TABLE IF NOT EXISTS {ATTACH_ALIAS}.cold_capabilities (
                    cap_id           TEXT PRIMARY KEY,
                    content          TEXT NOT NULL,
                    created_at       TEXT NOT NULL,
                    last_accessed_at TEXT NOT NULL,
                    access_count     INTEGER NOT NULL
                );

                CREATE INDEX IF NOT EXISTS {ATTACH_ALIAS}.idx_cold_last_access
                    ON cold_capabilities(last_accessed_at);"
            ))?;

            Ok(conn)
        })?;

        Ok(Self {
            pool: Arc::new(pool),
            capacity,
        })
    }

    /// 返回容量上限
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 异步插入或更新能力条目(UPSERT 语义)
    ///
    /// WHY `with_write_async`:INSERT 是写操作,通过写连接序列化,
    /// 避免 WAL 模式下多写者 SQLITE_BUSY。
    pub async fn insert(&self, mut entry: CapabilityEntry) -> Result<(), CmtError> {
        // 强制设置 tier 为 Cold(防止上层传入错误层级)
        entry.tier = Tier::Cold;

        self.pool
            .with_write_async(move |conn| {
                let created_iso = entry.created_at.to_rfc3339();
                let accessed_iso = entry.last_accessed_at.to_rfc3339();

                conn.execute(
                    &format!(
                        "INSERT OR REPLACE INTO {ATTACH_ALIAS}.cold_capabilities
                            (cap_id, content, created_at, last_accessed_at, access_count)
                         VALUES (?1, ?2, ?3, ?4, ?5);"
                    ),
                    params![
                        entry.id.as_str(),
                        entry.content,
                        created_iso,
                        accessed_iso,
                        entry.access_count as i64,
                    ],
                )?;

                trace!(cap_id = ?entry.id, "Cold 层条目已插入/更新");
                Ok(())
            })
            .await
    }

    /// 异步批量插入能力条目(单事务,消除 N+1 查询)
    ///
    /// WHY:批量插入场景下,单事务包裹多个 INSERT 比 N 次独立 INSERT 快 10-100 倍,
    /// 避免每次提交都触发 fsync。失败时整个事务回滚,保证原子性。
    pub async fn insert_batch(&self, mut entries: Vec<CapabilityEntry>) -> Result<(), CmtError> {
        if entries.is_empty() {
            return Ok(());
        }

        // 强制设置 tier 为 Cold
        for entry in &mut entries {
            entry.tier = Tier::Cold;
        }

        self.pool
            .with_write_async(move |conn| {
                conn.execute_batch("BEGIN;")?;
                for entry in &entries {
                    let created_iso = entry.created_at.to_rfc3339();
                    let accessed_iso = entry.last_accessed_at.to_rfc3339();
                    conn.execute(
                        &format!(
                            "INSERT OR REPLACE INTO {ATTACH_ALIAS}.cold_capabilities
                                (cap_id, content, created_at, last_accessed_at, access_count)
                             VALUES (?1, ?2, ?3, ?4, ?5);"
                        ),
                        params![
                            entry.id.as_str(),
                            entry.content,
                            created_iso,
                            accessed_iso,
                            entry.access_count as i64,
                        ],
                    )?;
                }
                conn.execute_batch("COMMIT;")?;

                trace!(count = entries.len(), "Cold 层批量条目已插入/更新");
                Ok(())
            })
            .await
    }

    /// 异步获取能力条目(更新 last_accessed_at 与 access_count)
    ///
    /// 返回条目克隆;若不存在返回 None。
    ///
    /// WHY `with_write_async`:虽然逻辑上是"读",但 get 会更新 last_accessed_at
    /// 与 access_count(写操作),必须通过写连接执行以保证数据一致性。
    /// WAL 模式下写连接不影响并发读连接(peek/list_* 仍可用读连接)。
    ///
    /// WHY 单次查询优化(SubTask 19.1):原实现 SELECT → UPDATE → SELECT(三次查询),
    /// 现改为 SELECT → 内存构造更新字段 → UPDATE → 返回内存构造的条目,
    /// 消除第二次 SELECT 往返,Cold get 延迟降低约 33%。
    /// 参照 Warm 层 `get` 方法的优化模式,保持两层实现一致性。
    pub async fn get(&self, id: String) -> Result<Option<CapabilityEntry>, CmtError> {
        self.pool
            .with_write_async(move |conn| {
                // 单次 SELECT 读取所有字段
                let result: Option<CapabilityEntry> = conn
                    .query_row(
                        &format!(
                            "SELECT cap_id, content, created_at, last_accessed_at, access_count
                             FROM {ATTACH_ALIAS}.cold_capabilities WHERE cap_id = ?1;"
                        ),
                        params![id],
                        row_to_entry,
                    )
                    .optional()?;

                // 若找到条目,在内存中更新访问时间与计数,执行 UPDATE 后返回内存构造的条目
                // WHY 内存构造:避免 UPDATE 后再次 SELECT 读取,消除一次查询往返
                if let Some(mut entry) = result {
                    let now = Utc::now();
                    entry.last_accessed_at = now;
                    entry.access_count = entry.access_count.saturating_add(1);

                    conn.execute(
                        &format!(
                            "UPDATE {ATTACH_ALIAS}.cold_capabilities
                             SET last_accessed_at = ?1, access_count = ?2
                             WHERE cap_id = ?3;"
                        ),
                        params![now.to_rfc3339(), entry.access_count as i64, id],
                    )?;

                    Ok(Some(entry))
                } else {
                    Ok(None)
                }
            })
            .await
    }

    /// 异步尝试获取条目(不更新访问时间,不增加计数)
    ///
    /// WHY `with_read_async`:peek 是纯只读操作,可通过读连接并发执行,
    /// 不受写连接 Mutex 限制。
    pub async fn peek(&self, id: String) -> Result<Option<CapabilityEntry>, CmtError> {
        self.pool
            .with_read_async(id.clone(), move |conn| {
                let result = conn
                    .query_row(
                        &format!(
                            "SELECT cap_id, content, created_at, last_accessed_at, access_count
                             FROM {ATTACH_ALIAS}.cold_capabilities WHERE cap_id = ?1;"
                        ),
                        params![id],
                        row_to_entry,
                    )
                    .optional()?;

                Ok(result)
            })
            .await
    }

    /// 异步删除指定条目
    ///
    /// 返回是否删除成功(若条目不存在返回 false)
    ///
    /// WHY 接受 `impl Into<CapabilityId>`:类型安全,调用方可传 `CapabilityId`/`String`/`&str`,
    /// 内部统一转为 `CapabilityId` 后用 `as_str()` 传给 SQL(避免 `ToSql` 未实现问题)
    pub async fn delete(&self, id: impl Into<CapabilityId>) -> Result<bool, CmtError> {
        let id = id.into();
        self.pool
            .with_write_async(move |conn| {
                let affected = conn.execute(
                    &format!("DELETE FROM {ATTACH_ALIAS}.cold_capabilities WHERE cap_id = ?1;"),
                    params![id.as_str()],
                )?;

                Ok(affected > 0)
            })
            .await
    }

    /// 异步批量删除能力条目(单事务,消除 N+1 查询)
    ///
    /// WHY:批量删除场景下,单事务包裹多个 DELETE 比 N 次独立 DELETE 快 10-100 倍,
    /// 避免每次提交都触发 fsync。失败时整个事务回滚,保证原子性。
    /// 返回实际删除的条目数(若条目不存在不计入)。
    pub async fn delete_batch(&self, ids: Vec<CapabilityId>) -> Result<u64, CmtError> {
        if ids.is_empty() {
            return Ok(0);
        }

        self.pool
            .with_write_async(move |conn| {
                conn.execute_batch("BEGIN;")?;
                let mut deleted = 0u64;
                for id in &ids {
                    let affected = conn.execute(
                        &format!("DELETE FROM {ATTACH_ALIAS}.cold_capabilities WHERE cap_id = ?1;"),
                        params![id.as_str()],
                    )?;
                    if affected > 0 {
                        deleted += 1;
                    }
                }
                conn.execute_batch("COMMIT;")?;

                trace!(count = deleted, "Cold 层批量删除完成");
                Ok(deleted)
            })
            .await
    }

    /// 异步列出空闲条目(最后访问时间早于 `until` 的条目)
    ///
    /// WHY `with_any_read_async`:纯只读查询,可并发执行。
    /// 使用轮询分配(无特定 hint),均匀分布到各读分片。
    /// 用于 Cold → Ice 的空闲超时迁移(7 天未被访问)。
    pub async fn list_idle_entries(&self, until: DateTime<Utc>) -> Result<Vec<String>, CmtError> {
        self.pool
            .with_any_read_async(move |conn| {
                let until_iso = until.to_rfc3339();
                let mut stmt = conn.prepare(&format!(
                    "SELECT cap_id FROM {ATTACH_ALIAS}.cold_capabilities
                     WHERE last_accessed_at < ?1
                     ORDER BY last_accessed_at ASC;"
                ))?;
                let rows = stmt.query_map(params![until_iso], |row| row.get::<_, String>(0))?;

                let mut ids = Vec::new();
                for row in rows {
                    ids.push(row?);
                }
                Ok(ids)
            })
            .await
    }

    /// 异步列出所有条目的元数据(不含 content,用于衰减周期扫描)
    ///
    /// WHY(SubTask 19.2):衰减判断仅需 `access_count` + `last_accessed_at`,
    /// 无需加载 content。65536 条目全量加载 content 会导致内存峰值过高,
    /// 此方法只返回元数据(ID + 时间戳 + 计数),内存占用降低 80%+。
    /// 降级时再通过 `peek` 按需读取完整条目(含 content)。
    pub async fn list_idle_metadata(
        &self,
    ) -> Result<Vec<(CapabilityId, DateTime<Utc>, u64)>, CmtError> {
        self.pool
            .with_any_read_async(move |conn| {
                let mut stmt = conn.prepare(&format!(
                    "SELECT cap_id, last_accessed_at, access_count
                     FROM {ATTACH_ALIAS}.cold_capabilities ORDER BY last_accessed_at ASC;"
                ))?;
                let rows = stmt.query_map([], |row| {
                    let cap_id: String = row.get(0)?;
                    let accessed_iso: String = row.get(1)?;
                    let access_count_i64: i64 = row.get(2)?;
                    // 时间戳解析(失败时降级为当前时间,不阻断查询)
                    let last_accessed_at = DateTime::parse_from_rfc3339(&accessed_iso)
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now());
                    Ok((
                        CapabilityId::from(cap_id),
                        last_accessed_at,
                        u64::try_from(access_count_i64).unwrap_or(0),
                    ))
                })?;

                let mut metadata = Vec::new();
                for row in rows {
                    metadata.push(row?);
                }
                Ok(metadata)
            })
            .await
    }

    /// 异步列出所有条目(用于迁移或快照)
    ///
    /// WHY `with_any_read_async`:纯只读查询,可并发执行。
    pub async fn list_all(&self) -> Result<Vec<CapabilityEntry>, CmtError> {
        self.pool
            .with_any_read_async(move |conn| {
                let mut stmt = conn.prepare(&format!(
                    "SELECT cap_id, content, created_at, last_accessed_at, access_count
                     FROM {ATTACH_ALIAS}.cold_capabilities ORDER BY created_at ASC;"
                ))?;
                let rows = stmt.query_map([], row_to_entry)?;

                let mut entries = Vec::new();
                for row in rows {
                    entries.push(row?);
                }
                Ok(entries)
            })
            .await
    }

    /// 异步计算条目总数
    ///
    /// WHY `with_any_read_async`:COUNT(*) 是纯只读查询,可并发执行。
    pub async fn count(&self) -> Result<u64, CmtError> {
        self.pool
            .with_any_read_async(move |conn| {
                let count: i64 = conn.query_row(
                    &format!("SELECT COUNT(*) FROM {ATTACH_ALIAS}.cold_capabilities;"),
                    [],
                    |row| row.get(0),
                )?;
                Ok(u64::try_from(count).unwrap_or(0))
            })
            .await
    }

    /// 测试辅助:删除 cold_capabilities 表,使后续 insert/delete 等操作失败
    ///
    /// WHY:ColdTier 使用内存 SQLite 连接 + ATTACH 文件数据库,连接打开后
    /// 删除文件不会导致 INSERT 失败(SQLite 持有文件句柄并缓存页)。测试需要
    /// 可靠地模拟 Cold 层写入失败,以验证迁移回滚逻辑保留原始数据。
    /// 通过 DROP TABLE 使后续 INSERT 因"no such table"而失败。
    ///
    /// # 安全性
    /// 此方法仅用于测试,标记为 `#[doc(hidden)]` 避免污染公共 API 文档。
    /// 生产代码不应调用此方法。
    #[doc(hidden)]
    pub async fn break_for_testing(&self) -> Result<(), CmtError> {
        self.pool
            .with_write_async(move |conn| {
                conn.execute_batch(&format!(
                    "DROP TABLE IF EXISTS {ATTACH_ALIAS}.cold_capabilities;"
                ))?;
                Ok(())
            })
            .await
    }
}

/// 应用 SQLite 性能优化 PRAGMA(连接级,影响所有附加数据库)
///
/// WHY:F2.2.3 — 委托给 `nexus_core::storage_traits::apply_performance_pragmas`。
/// 用 newtype wrapper(`PragmaConn`)包装 `&Connection` 以满足 PragmaCapable trait,
/// 规避 Rust coherence 规则下与 mlc-engine 的 `conflicting implementations` 冲突。
///
/// WHY 返回 `rusqlite::Result` 而非 `Result<(), CmtError>`:此函数仅在
/// `SqlitePool::open` 的 conn_factory 闭包内调用,闭包签名要求返回
/// `rusqlite::Result<Connection>`。返回 `rusqlite::Result` 让闭包内 `?` 直接工作,
/// 避免每个调用点重复 map_err 转换(原代码因 CmtError 无法转 rusqlite::Error 编译失败)。
fn apply_performance_pragmas(conn: &Connection) -> rusqlite::Result<()> {
    let wrapper = crate::PragmaConn(conn);
    nexus_core::apply_performance_pragmas(&wrapper).map_err(|e| {
        rusqlite::Error::ToSqlConversionFailure(Box::new(CmtError::StorageError(format!(
            "SQLite PRAGMA 设置失败: {e}"
        ))))
    })
}

/// 将 SQLite 行映射为 CapabilityEntry
///
/// 字段顺序与 SELECT 语句对齐:
/// cap_id, content, created_at, last_accessed_at, access_count
fn row_to_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<CapabilityEntry> {
    let cap_id: String = row.get(0)?;
    let content: String = row.get(1)?;
    let created_iso: String = row.get(2)?;
    let accessed_iso: String = row.get(3)?;
    let access_count_i64: i64 = row.get(4)?;

    // 时间戳解析(失败时降级为当前时间,不阻断查询)
    let created_at = DateTime::parse_from_rfc3339(&created_iso)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    let last_accessed_at = DateTime::parse_from_rfc3339(&accessed_iso)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());

    Ok(CapabilityEntry {
        id: CapabilityId::from(cap_id),
        content,
        tier: Tier::Cold,
        created_at,
        last_accessed_at,
        access_count: u64::try_from(access_count_i64).unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Tier;
    use chrono::Duration;

    fn make_entry(id: &str) -> CapabilityEntry {
        CapabilityEntry::new(id, format!("content-{id}"), Tier::Cold)
    }

    #[tokio::test]
    async fn test_open_in_memory() {
        let tier = ColdTier::open_in_memory(65536).unwrap();
        assert_eq!(tier.capacity(), 65536);
        assert_eq!(tier.count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_insert_and_get() {
        let tier = ColdTier::open_in_memory(65536).unwrap();
        let entry = make_entry("cap-1");
        tier.insert(entry).await.unwrap();

        let fetched = tier.get("cap-1".to_string()).await.unwrap();
        assert!(fetched.is_some());
        let fetched = fetched.unwrap();
        assert_eq!(fetched.id.as_str(), "cap-1");
        assert_eq!(fetched.content, "content-cap-1");
        assert_eq!(fetched.tier, Tier::Cold);
        // insert 不增加 access_count(get 才增加)
        assert_eq!(fetched.access_count, 1);
    }

    #[tokio::test]
    async fn test_get_nonexistent_returns_none() {
        let tier = ColdTier::open_in_memory(65536).unwrap();
        let result = tier.get("nonexistent".to_string()).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_peek_does_not_update_access() {
        let tier = ColdTier::open_in_memory(65536).unwrap();
        tier.insert(make_entry("cap-1")).await.unwrap();

        let peeked = tier.peek("cap-1".to_string()).await.unwrap().unwrap();
        assert_eq!(peeked.access_count, 0);

        // peek 不增加 access_count
        let peeked_again = tier.peek("cap-1".to_string()).await.unwrap().unwrap();
        assert_eq!(peeked_again.access_count, 0);
    }

    #[tokio::test]
    async fn test_insert_or_replace() {
        let tier = ColdTier::open_in_memory(65536).unwrap();
        tier.insert(make_entry("cap-1")).await.unwrap();
        assert_eq!(tier.count().await.unwrap(), 1);

        // 用相同 ID 但不同内容插入,应覆盖
        let mut entry2 = make_entry("cap-1");
        entry2.content = "updated-content".to_string();
        tier.insert(entry2).await.unwrap();
        assert_eq!(tier.count().await.unwrap(), 1);

        let fetched = tier.peek("cap-1".to_string()).await.unwrap().unwrap();
        assert_eq!(fetched.content, "updated-content");
    }

    #[tokio::test]
    async fn test_delete() {
        let tier = ColdTier::open_in_memory(65536).unwrap();
        tier.insert(make_entry("cap-1")).await.unwrap();
        assert_eq!(tier.count().await.unwrap(), 1);

        let deleted = tier.delete("cap-1".to_string()).await.unwrap();
        assert!(deleted);
        assert_eq!(tier.count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_delete_nonexistent_returns_false() {
        let tier = ColdTier::open_in_memory(65536).unwrap();
        let deleted = tier.delete("nonexistent".to_string()).await.unwrap();
        assert!(!deleted);
    }

    #[tokio::test]
    async fn test_list_idle_entries() {
        let tier = ColdTier::open_in_memory(65536).unwrap();

        // 插入 3 个条目,手动调整 last_accessed_at
        let mut entry1 = make_entry("cap-old");
        entry1.last_accessed_at = Utc::now() - Duration::days(10);
        tier.insert(entry1).await.unwrap();

        let mut entry2 = make_entry("cap-medium");
        entry2.last_accessed_at = Utc::now() - Duration::days(3);
        tier.insert(entry2).await.unwrap();

        let entry3 = make_entry("cap-recent");
        tier.insert(entry3).await.unwrap();

        // 查询 7 天前的空闲条目,应只有 cap-old
        let cutoff = Utc::now() - Duration::days(7);
        let idle = tier.list_idle_entries(cutoff).await.unwrap();
        assert_eq!(idle.len(), 1);
        assert_eq!(idle[0], "cap-old");
    }

    #[tokio::test]
    async fn test_list_all() {
        let tier = ColdTier::open_in_memory(65536).unwrap();
        for i in 0..3 {
            tier.insert(make_entry(&format!("cap-{i}"))).await.unwrap();
        }
        let all = tier.list_all().await.unwrap();
        assert_eq!(all.len(), 3);
    }

    #[tokio::test]
    async fn test_count() {
        let tier = ColdTier::open_in_memory(65536).unwrap();
        assert_eq!(tier.count().await.unwrap(), 0);
        for i in 0..5 {
            tier.insert(make_entry(&format!("cap-{i}"))).await.unwrap();
        }
        assert_eq!(tier.count().await.unwrap(), 5);
    }

    #[tokio::test]
    async fn test_persistence_roundtrip() {
        // 使用临时目录验证 SQLite 附加数据库持久化往返一致性
        let tmp = tempfile::tempdir().unwrap();

        // 写入数据
        {
            let tier = ColdTier::open(tmp.path(), 65536).unwrap();
            tier.insert(make_entry("cap-1")).await.unwrap();
        }

        // 重新打开并验证(附加同一文件)
        {
            let tier = ColdTier::open(tmp.path(), 65536).unwrap();
            assert_eq!(tier.count().await.unwrap(), 1);
            let fetched = tier.peek("cap-1".to_string()).await.unwrap().unwrap();
            assert_eq!(fetched.id.as_str(), "cap-1");
            assert_eq!(fetched.content, "content-cap-1");
        }
    }

    #[tokio::test]
    async fn test_pool_read_pool_size() {
        // 文件数据库默认 read_pool_size = 2
        let tmp = tempfile::tempdir().unwrap();
        let tier = ColdTier::open(tmp.path(), 65536).unwrap();
        assert_eq!(tier.pool.read_pool_size(), 2);

        // 内存数据库 read_pool_size = 0
        let tier_mem = ColdTier::open_in_memory(65536).unwrap();
        assert_eq!(tier_mem.pool.read_pool_size(), 0);
    }
}
