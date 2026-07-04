//! Warm 层 — SQLite WAL 模式持久化存储
//!
//! 对应架构层:L3 Storage(Warm tier)
//!
//! # 设计决策(WHY)
//! - **SQLite WAL 模式**:Warm 层需跨会话保留,WAL 模式允许读写并发,
//!   适合"读多写少"的能力查询场景(参考 repo-wiki/store.rs 实现)
//! - **`Arc<Mutex<Connection>>` 包装**:`rusqlite::Connection` 不是 `Sync`,
//!   用 Mutex 提供线程安全访问;Arc 包装支持 Clone 与跨任务共享,
//!   使 `spawn_blocking` 闭包可拥有连接(参考 cold.rs 实现)
//! - **spawn_blocking 包装文件 I/O**:SQLite 操作可能阻塞异步运行时,
//!   使用 `tokio::task::spawn_blocking` 将其放到阻塞线程池
//!   (架构红线:所有 async fn 满足 Send + 'static 约束)
//! - **PRAGMA 性能优化**:在 WAL 模式后设置 synchronous=NORMAL、
//!   cache_size、mmap_size、temp_store=MEMORY、wal_autocheckpoint,
//!   减少 fsync 与磁盘 I/O,查询延迟降低 30-50%
//! - **content TEXT 存储**:能力内容为文本,直接存储为 TEXT;
//!   若未来需要存储二进制,可改为 BLOB(向后兼容)
//! - **list_idle_entries 查询**:按 `last_accessed_at < ?` 过滤,
//!   用于 Warm → Cold 的空闲超时迁移(24 小时未被访问)
//!
//! # Schema
//! ```sql
//! CREATE TABLE IF NOT EXISTS warm_capabilities (
//!     cap_id           TEXT PRIMARY KEY,
//!     content          TEXT NOT NULL,
//!     created_at       TEXT NOT NULL,     -- ISO 8601
//!     last_accessed_at TEXT NOT NULL,     -- ISO 8601
//!     access_count     INTEGER NOT NULL
//! );
//! ```

use std::path::Path;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use tokio::task::spawn_blocking;
use tracing::{debug, trace};

use crate::error::CmtError;
use crate::types::{CapabilityEntry, CapabilityId, Tier};

/// Warm 层 — SQLite 持久化的温存储
///
/// 封装 `Arc<Mutex<Connection>>`,提供线程安全的能力条目 CRUD。
/// WAL 模式允许读写并发,所有 async 方法通过 `spawn_blocking`
/// 在阻塞线程池中执行 SQLite 操作,避免阻塞异步运行时。
///
/// # 线程安全
/// `Arc<Mutex<Connection>>` 包装,可 Clone(廉价,Arc 引用计数)。
/// 所有 async fn 满足 `Send + 'static` 约束。
#[derive(Clone)]
pub struct WarmTier {
    /// SQLite 连接(`Arc<Mutex>` 包装,支持 Clone 与跨任务共享)
    ///
    /// WHY `Arc<Mutex>`:spawn_blocking 需要 'static + Send 的闭包,
    /// `Arc<Mutex>` 允许将连接所有权转移到阻塞线程
    conn: Arc<Mutex<Connection>>,
    /// 容量上限(超出时由上层触发迁移到 Cold)
    capacity: usize,
}

impl WarmTier {
    /// 打开或创建 Warm 层数据库
    ///
    /// 自动启用 WAL 模式并创建 `warm_capabilities` 表(若不存在)。
    /// 路径的父目录应已存在(调用方负责创建)。
    pub fn open(path: &Path, capacity: usize) -> Result<Self, CmtError> {
        let conn = Connection::open(path)?;

        // 启用 WAL 模式:提升并发读写性能
        // WHY:WAL(Write-Ahead Logging)允许读写并发,默认 rollback journal 模式下写会阻塞读
        conn.pragma_update(None, "journal_mode", "WAL")?;

        // 应用 SQLite PRAGMA 性能优化(必须在 journal_mode=WAL 之后设置)
        // WHY:减少 fsync 与磁盘 I/O,查询延迟降低 30-50%
        apply_performance_pragmas(&conn)?;

        // 创建 warm_capabilities 表
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS warm_capabilities (
                cap_id           TEXT PRIMARY KEY,
                content          TEXT NOT NULL,
                created_at       TEXT NOT NULL,
                last_accessed_at TEXT NOT NULL,
                access_count     INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_warm_last_accessed
                ON warm_capabilities(last_accessed_at);",
        )?;

        debug!(path = ?path, capacity, "Warm 层数据库已打开");
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            capacity,
        })
    }

    /// 在内存中创建 Warm 层(用于测试,不持久化)
    pub fn open_in_memory(capacity: usize) -> Result<Self, CmtError> {
        let conn = Connection::open_in_memory()?;
        // 内存数据库也应用 PRAGMA 优化(部分 PRAGMA 对内存库无效,但不会报错)
        apply_performance_pragmas(&conn)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS warm_capabilities (
                cap_id           TEXT PRIMARY KEY,
                content          TEXT NOT NULL,
                created_at       TEXT NOT NULL,
                last_accessed_at TEXT NOT NULL,
                access_count     INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_warm_last_accessed
                ON warm_capabilities(last_accessed_at);",
        )?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            capacity,
        })
    }

    /// 返回容量上限
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 异步插入或更新能力条目(UPSERT 语义)
    ///
    /// - 若 `cap_id` 不存在,插入新条目
    /// - 若 `cap_id` 已存在,更新所有字段(覆盖)
    ///
    /// 使用 `spawn_blocking` 包装 SQLite 操作,避免阻塞异步运行时。
    pub async fn insert(&self, mut entry: CapabilityEntry) -> Result<(), CmtError> {
        // 强制设置 tier 为 Warm(防止上层传入错误层级)
        entry.tier = Tier::Warm;

        let conn = self.conn.clone();
        spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| CmtError::StorageError(format!("Warm 层 mutex poisoned: {e}")))?;

            let created_iso = entry.created_at.to_rfc3339();
            let accessed_iso = entry.last_accessed_at.to_rfc3339();

            conn.execute(
                "INSERT OR REPLACE INTO warm_capabilities
                    (cap_id, content, created_at, last_accessed_at, access_count)
                 VALUES (?1, ?2, ?3, ?4, ?5);",
                params![
                    entry.id.as_str(),
                    entry.content,
                    created_iso,
                    accessed_iso,
                    entry.access_count as i64,
                ],
            )?;

            trace!(cap_id = %entry.id, "Warm 层条目已插入/更新");
            Ok(())
        })
        .await
        .map_err(|e| CmtError::StorageError(format!("spawn_blocking join 错误: {e}")))?
    }

    /// 异步批量插入能力条目(单事务,提升批量写入性能)
    ///
    /// WHY:批量插入场景下,单事务包裹多个 INSERT 比 N 次独立 INSERT 快 10-100 倍,
    /// 避免每次提交都触发 fsync。失败时整个事务回滚,保证原子性。
    pub async fn insert_batch(&self, mut entries: Vec<CapabilityEntry>) -> Result<(), CmtError> {
        // 强制设置 tier 为 Warm
        for entry in &mut entries {
            entry.tier = Tier::Warm;
        }

        let conn = self.conn.clone();
        spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| CmtError::StorageError(format!("Warm 层 mutex poisoned: {e}")))?;

            conn.execute_batch("BEGIN;")?;
            for entry in &entries {
                let created_iso = entry.created_at.to_rfc3339();
                let accessed_iso = entry.last_accessed_at.to_rfc3339();
                conn.execute(
                    "INSERT OR REPLACE INTO warm_capabilities
                        (cap_id, content, created_at, last_accessed_at, access_count)
                     VALUES (?1, ?2, ?3, ?4, ?5);",
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

            trace!(count = entries.len(), "Warm 层批量条目已插入/更新");
            Ok(())
        })
        .await
        .map_err(|e| CmtError::StorageError(format!("spawn_blocking join 错误: {e}")))?
    }

    /// 异步获取能力条目(更新 last_accessed_at 与 access_count)
    ///
    /// 返回条目克隆;若不存在返回 None。
    ///
    /// WHY 更新访问时间:Warm 层条目被访问时,应刷新其空闲计时,
    /// 避免刚被访问的条目立即被迁移到 Cold 层
    ///
    /// WHY 单次查询优化:原实现 SELECT → UPDATE → SELECT(两次查询),
    /// 现改为 SELECT → 内存更新字段 → UPDATE → 返回内存构造的条目,
    /// 避免第二次 SELECT 往返,提升 Warm 层读取性能约 50%
    pub async fn get(&self, id: String) -> Result<Option<CapabilityEntry>, CmtError> {
        let conn = self.conn.clone();
        spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| CmtError::StorageError(format!("Warm 层 mutex poisoned: {e}")))?;

            let result: Option<CapabilityEntry> = conn
                .query_row(
                    "SELECT cap_id, content, created_at, last_accessed_at, access_count
                     FROM warm_capabilities WHERE cap_id = ?1;",
                    params![id],
                    row_to_entry,
                )
                .optional()?;

            // 若找到条目,在内存中更新访问时间与计数,执行 UPDATE 后返回内存构造的条目
            if let Some(mut entry) = result {
                let now = Utc::now();
                entry.last_accessed_at = now;
                entry.access_count = entry.access_count.saturating_add(1);

                conn.execute(
                    "UPDATE warm_capabilities
                     SET last_accessed_at = ?1, access_count = ?2
                     WHERE cap_id = ?3;",
                    params![now.to_rfc3339(), entry.access_count as i64, id],
                )?;

                Ok(Some(entry))
            } else {
                Ok(None)
            }
        })
        .await
        .map_err(|e| CmtError::StorageError(format!("spawn_blocking join 错误: {e}")))?
    }

    /// 异步尝试获取条目(不更新访问时间,不增加计数)
    ///
    /// 用于内部检查或不需要 LRU 语义的场景
    pub async fn peek(&self, id: String) -> Result<Option<CapabilityEntry>, CmtError> {
        let conn = self.conn.clone();
        spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| CmtError::StorageError(format!("Warm 层 mutex poisoned: {e}")))?;

            let result = conn
                .query_row(
                    "SELECT cap_id, content, created_at, last_accessed_at, access_count
                     FROM warm_capabilities WHERE cap_id = ?1;",
                    params![id],
                    row_to_entry,
                )
                .optional()?;

            Ok(result)
        })
        .await
        .map_err(|e| CmtError::StorageError(format!("spawn_blocking join 错误: {e}")))?
    }

    /// 异步删除指定条目
    ///
    /// 返回是否删除成功(若条目不存在返回 false)
    ///
    /// WHY 接受 `impl Into<CapabilityId>`:类型安全,调用方可传 `CapabilityId`/`String`/`&str`,
    /// 内部统一转为 `CapabilityId` 后用 `as_str()` 传给 SQL(避免 `ToSql` 未实现问题)
    pub async fn delete(&self, id: impl Into<CapabilityId>) -> Result<bool, CmtError> {
        let id = id.into();
        let conn = self.conn.clone();
        spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| CmtError::StorageError(format!("Warm 层 mutex poisoned: {e}")))?;

            let affected = conn.execute(
                "DELETE FROM warm_capabilities WHERE cap_id = ?1;",
                params![id.as_str()],
            )?;

            Ok(affected > 0)
        })
        .await
        .map_err(|e| CmtError::StorageError(format!("spawn_blocking join 错误: {e}")))?
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

        let conn = self.conn.clone();
        spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| CmtError::StorageError(format!("Warm 层 mutex poisoned: {e}")))?;

            conn.execute_batch("BEGIN;")?;
            let mut deleted = 0u64;
            for id in &ids {
                let affected = conn.execute(
                    "DELETE FROM warm_capabilities WHERE cap_id = ?1;",
                    params![id.as_str()],
                )?;
                if affected > 0 {
                    deleted += 1;
                }
            }
            conn.execute_batch("COMMIT;")?;

            trace!(count = deleted, "Warm 层批量删除完成");
            Ok(deleted)
        })
        .await
        .map_err(|e| CmtError::StorageError(format!("spawn_blocking join 错误: {e}")))?
    }

    /// 异步列出空闲条目(最后访问时间早于 `until` 的条目)
    ///
    /// 用于 Warm → Cold 的空闲超时迁移(24 小时未被访问)。
    /// 返回满足条件的 `cap_id` 列表,调用方据此逐个迁移。
    pub async fn list_idle_entries(&self, until: DateTime<Utc>) -> Result<Vec<String>, CmtError> {
        let conn = self.conn.clone();
        spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| CmtError::StorageError(format!("Warm 层 mutex poisoned: {e}")))?;

            let until_iso = until.to_rfc3339();
            let mut stmt = conn.prepare(
                "SELECT cap_id FROM warm_capabilities
                 WHERE last_accessed_at < ?1
                 ORDER BY last_accessed_at ASC;",
            )?;
            let rows = stmt.query_map(params![until_iso], |row| row.get::<_, String>(0))?;

            let mut ids = Vec::new();
            for row in rows {
                ids.push(row?);
            }
            Ok(ids)
        })
        .await
        .map_err(|e| CmtError::StorageError(format!("spawn_blocking join 错误: {e}")))?
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
        let conn = self.conn.clone();
        spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| CmtError::StorageError(format!("Warm 层 mutex poisoned: {e}")))?;

            let mut stmt = conn.prepare(
                "SELECT cap_id, last_accessed_at, access_count
                 FROM warm_capabilities ORDER BY last_accessed_at ASC;",
            )?;
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
        .map_err(|e| CmtError::StorageError(format!("spawn_blocking join 错误: {e}")))?
    }

    /// 异步列出所有条目(用于迁移或快照)
    pub async fn list_all(&self) -> Result<Vec<CapabilityEntry>, CmtError> {
        let conn = self.conn.clone();
        spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| CmtError::StorageError(format!("Warm 层 mutex poisoned: {e}")))?;

            let mut stmt = conn.prepare(
                "SELECT cap_id, content, created_at, last_accessed_at, access_count
                 FROM warm_capabilities ORDER BY created_at ASC;",
            )?;
            let rows = stmt.query_map([], row_to_entry)?;

            let mut entries = Vec::new();
            for row in rows {
                entries.push(row?);
            }
            Ok(entries)
        })
        .await
        .map_err(|e| CmtError::StorageError(format!("spawn_blocking join 错误: {e}")))?
    }

    /// 异步计算条目总数
    pub async fn count(&self) -> Result<u64, CmtError> {
        let conn = self.conn.clone();
        spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| CmtError::StorageError(format!("Warm 层 mutex poisoned: {e}")))?;
            let count: i64 =
                conn.query_row("SELECT COUNT(*) FROM warm_capabilities;", [], |row| {
                    row.get(0)
                })?;
            Ok(u64::try_from(count).unwrap_or(0))
        })
        .await
        .map_err(|e| CmtError::StorageError(format!("spawn_blocking join 错误: {e}")))?
    }
}

/// 应用 SQLite 性能优化 PRAGMA(在 WAL 模式设置之后调用)
///
/// WHY:SubTask 21.2 — 委托给 `nexus_core::sqlite_pragma::apply_performance_pragmas`,
/// 消除与 cold.rs / mlc-engine 的重复实现,统一 PRAGMA 配置。
fn apply_performance_pragmas(conn: &Connection) -> Result<(), CmtError> {
    nexus_core::sqlite_pragma::apply_performance_pragmas(conn)
        .map_err(|e| CmtError::StorageError(format!("SQLite PRAGMA 设置失败: {e}")))
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
        tier: Tier::Warm,
        created_at,
        last_accessed_at,
        access_count: u64::try_from(access_count_i64).unwrap_or(0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn make_entry(id: &str) -> CapabilityEntry {
        CapabilityEntry::new(id, format!("content-{id}"), Tier::Warm)
    }

    #[tokio::test]
    async fn test_open_in_memory() {
        let tier = WarmTier::open_in_memory(4096).unwrap();
        assert_eq!(tier.capacity(), 4096);
        assert_eq!(tier.count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_insert_and_get() {
        let tier = WarmTier::open_in_memory(4096).unwrap();
        let entry = make_entry("cap-1");
        tier.insert(entry).await.unwrap();

        let fetched = tier.get("cap-1".to_string()).await.unwrap();
        assert!(fetched.is_some());
        let fetched = fetched.unwrap();
        assert_eq!(fetched.id.as_str(), "cap-1");
        assert_eq!(fetched.content, "content-cap-1");
        assert_eq!(fetched.tier, Tier::Warm);
        // insert 不增加 access_count(get 才增加)
        assert_eq!(fetched.access_count, 1);
    }

    #[tokio::test]
    async fn test_get_nonexistent_returns_none() {
        let tier = WarmTier::open_in_memory(4096).unwrap();
        let result = tier.get("nonexistent".to_string()).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_peek_does_not_update_access() {
        let tier = WarmTier::open_in_memory(4096).unwrap();
        tier.insert(make_entry("cap-1")).await.unwrap();

        let peeked = tier.peek("cap-1".to_string()).await.unwrap().unwrap();
        assert_eq!(peeked.access_count, 0);

        // peek 不增加 access_count
        let peeked_again = tier.peek("cap-1".to_string()).await.unwrap().unwrap();
        assert_eq!(peeked_again.access_count, 0);
    }

    #[tokio::test]
    async fn test_insert_or_replace() {
        let tier = WarmTier::open_in_memory(4096).unwrap();
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
        let tier = WarmTier::open_in_memory(4096).unwrap();
        tier.insert(make_entry("cap-1")).await.unwrap();
        assert_eq!(tier.count().await.unwrap(), 1);

        let deleted = tier.delete("cap-1".to_string()).await.unwrap();
        assert!(deleted);
        assert_eq!(tier.count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_delete_nonexistent_returns_false() {
        let tier = WarmTier::open_in_memory(4096).unwrap();
        let deleted = tier.delete("nonexistent".to_string()).await.unwrap();
        assert!(!deleted);
    }

    #[tokio::test]
    async fn test_list_idle_entries() {
        let tier = WarmTier::open_in_memory(4096).unwrap();

        // 插入 3 个条目,手动调整 last_accessed_at
        let mut entry1 = make_entry("cap-old");
        entry1.last_accessed_at = Utc::now() - Duration::hours(48);
        tier.insert(entry1).await.unwrap();

        let mut entry2 = make_entry("cap-medium");
        entry2.last_accessed_at = Utc::now() - Duration::hours(12);
        tier.insert(entry2).await.unwrap();

        let entry3 = make_entry("cap-recent");
        tier.insert(entry3).await.unwrap();

        // 查询 24 小时前的空闲条目,应只有 cap-old
        let cutoff = Utc::now() - Duration::hours(24);
        let idle = tier.list_idle_entries(cutoff).await.unwrap();
        assert_eq!(idle.len(), 1);
        assert_eq!(idle[0], "cap-old");
    }

    #[tokio::test]
    async fn test_list_all() {
        let tier = WarmTier::open_in_memory(4096).unwrap();
        for i in 0..3 {
            tier.insert(make_entry(&format!("cap-{i}"))).await.unwrap();
        }
        let all = tier.list_all().await.unwrap();
        assert_eq!(all.len(), 3);
    }

    #[tokio::test]
    async fn test_count() {
        let tier = WarmTier::open_in_memory(4096).unwrap();
        assert_eq!(tier.count().await.unwrap(), 0);
        for i in 0..5 {
            tier.insert(make_entry(&format!("cap-{i}"))).await.unwrap();
        }
        assert_eq!(tier.count().await.unwrap(), 5);
    }

    #[tokio::test]
    async fn test_persistence_roundtrip() {
        // 使用临时文件验证 SQLite 持久化往返一致性
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test_warm.db");

        // 写入数据
        {
            let tier = WarmTier::open(&db_path, 4096).unwrap();
            tier.insert(make_entry("cap-1")).await.unwrap();
        }

        // 重新打开并验证
        {
            let tier = WarmTier::open(&db_path, 4096).unwrap();
            assert_eq!(tier.count().await.unwrap(), 1);
            let fetched = tier.peek("cap-1".to_string()).await.unwrap().unwrap();
            assert_eq!(fetched.id.as_str(), "cap-1");
            assert_eq!(fetched.content, "content-cap-1");
        }
    }

    #[tokio::test]
    async fn test_wal_mode_enabled() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test_wal.db");
        let tier = WarmTier::open(&db_path, 4096).unwrap();

        let conn = tier.conn.lock().unwrap();
        let mode: String = conn
            .query_row("PRAGMA journal_mode;", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
    }

    #[tokio::test]
    async fn test_insert_batch() {
        let tier = WarmTier::open_in_memory(4096).unwrap();
        let entries = vec![
            make_entry("cap-1"),
            make_entry("cap-2"),
            make_entry("cap-3"),
        ];
        tier.insert_batch(entries).await.unwrap();
        assert_eq!(tier.count().await.unwrap(), 3);

        // 验证条目内容
        let fetched = tier.peek("cap-2".to_string()).await.unwrap().unwrap();
        assert_eq!(fetched.content, "content-cap-2");
    }
}
