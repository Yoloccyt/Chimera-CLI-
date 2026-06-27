//! L3 程序记忆 — SQLite 持久化的可复用执行模式
//!
//! 对应架构层:L2 Memory(L3 Procedural tier)
//!
//! # 设计决策(WHY)
//! - **SQLite 持久化**:程序记忆需跨会话保留(如"常用工具组合"),
//!   SQLite 提供 ACID 持久化与 WAL 并发模式
//! - **pattern_signature 作为主键**:模式签名序列化为字符串作为 PRIMARY KEY,
//!   支持精确匹配查找;Week 6 后可扩展为编辑距离匹配(需全表扫描)
//! - **execution_stats JSON 序列化**:统计字段可能扩展(如增加 p50/p99 延迟),
//!   JSON 格式便于演进,避免频繁 ALTER TABLE
//! - **`Arc<Mutex<Connection>>` 包装**:`rusqlite::Connection` 不是 `Sync`,
//!   用 Mutex 提供线程安全访问;Arc 包装支持 Clone 与跨任务共享,
//!   使 `spawn_blocking` 闭包可拥有连接(参考 cold.rs 实现)
//! - **spawn_blocking 包装文件 I/O**:SQLite 操作可能阻塞异步运行时,
//!   使用 `tokio::task::spawn_blocking` 将其放到阻塞线程池
//!   (架构红线:所有 async fn 满足 Send + 'static 约束)
//! - **PRAGMA 性能优化**:在 WAL 模式后设置 synchronous=NORMAL、
//!   cache_size、mmap_size、temp_store=MEMORY、wal_autocheckpoint,
//!   减少 fsync 与磁盘 I/O,查询延迟降低 30-50%
//!
//! # Schema
//! ```sql
//! CREATE TABLE IF NOT EXISTS procedural_memory (
//!     pattern_key     TEXT PRIMARY KEY,  -- PatternSignature 序列化
//!     tool_sequence   TEXT NOT NULL,     -- JSON 数组
//!     context_hash    TEXT NOT NULL,
//!     output          TEXT NOT NULL,
//!     execution_stats TEXT NOT NULL,     -- JSON 序列化
//!     created_at      TEXT NOT NULL,     -- ISO 8601
//!     updated_at      TEXT NOT NULL
//! );
//! ```

use std::path::Path;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension};
use tokio::task::spawn_blocking;
use tracing::{debug, trace};

use crate::error::MlcError;
use crate::types::{ExecutionStats, PatternSignature, ProceduralEntry};

/// L3 程序记忆 — SQLite 持久化的可复用执行模式
///
/// 封装 `Arc<Mutex<Connection>>`,提供线程安全的程序记忆 CRUD。
/// 模式签名(`PatternSignature`)作为主键,支持精确匹配查找。
///
/// # 线程安全
/// `Arc<Mutex<Connection>>` 包装,可 Clone(廉价,Arc 引用计数)。
/// 所有 async fn 满足 `Send + 'static` 约束。
/// SQLite WAL 模式允许读写并发,所有 async 方法通过 `spawn_blocking`
/// 在阻塞线程池中执行 SQLite 操作,避免阻塞异步运行时。
#[derive(Clone)]
pub struct ProceduralMemory {
    /// SQLite 连接(`Arc<Mutex>` 包装,支持 Clone 与跨任务共享)
    conn: Arc<Mutex<Connection>>,
}

impl ProceduralMemory {
    /// 打开或创建程序记忆数据库
    ///
    /// 自动启用 WAL 模式并创建 `procedural_memory` 表(若不存在)。
    /// 路径的父目录应已存在(调用方负责创建)。
    pub fn open(path: &Path) -> Result<Self, MlcError> {
        let conn = Connection::open(path)?;

        // 启用 WAL 模式:提升并发读写性能
        // WHY:WAL(Write-Ahead Logging)允许读写并发,默认 rollback journal 模式下写会阻塞读
        conn.pragma_update(None, "journal_mode", "WAL")?;

        // 应用 SQLite PRAGMA 性能优化(必须在 journal_mode=WAL 之后设置)
        apply_performance_pragmas(&conn)?;

        // 创建 procedural_memory 表
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS procedural_memory (
                pattern_key     TEXT PRIMARY KEY,
                tool_sequence   TEXT NOT NULL,
                context_hash    TEXT NOT NULL,
                output          TEXT NOT NULL,
                execution_stats TEXT NOT NULL,
                created_at      TEXT NOT NULL,
                updated_at      TEXT NOT NULL
            );",
        )?;

        debug!(path = ?path, "L3 程序记忆数据库已打开");
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// 在内存中创建程序记忆(用于测试,不持久化)
    pub fn open_in_memory() -> Result<Self, MlcError> {
        let conn = Connection::open_in_memory()?;
        apply_performance_pragmas(&conn)?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS procedural_memory (
                pattern_key     TEXT PRIMARY KEY,
                tool_sequence   TEXT NOT NULL,
                context_hash    TEXT NOT NULL,
                output          TEXT NOT NULL,
                execution_stats TEXT NOT NULL,
                created_at      TEXT NOT NULL,
                updated_at      TEXT NOT NULL
            );",
        )?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// 异步插入或更新程序记忆条目(UPSERT 语义)
    ///
    /// - 若模式签名不存在,插入新条目
    /// - 若模式签名已存在且 output 相同,仅更新 execution_stats
    /// - 若模式签名已存在但 output 不同,返回 `PatternConflict` 错误
    ///
    /// WHY 接受引用:保持与同步版本兼容的 API,内部 clone 后传入 spawn_blocking
    pub async fn insert(&self, entry: &ProceduralEntry) -> Result<(), MlcError> {
        let conn = self.conn.clone();
        let entry = entry.clone();
        spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| MlcError::StorageError(format!("L3 mutex poisoned: {e}")))?;

            let pattern_key = entry.pattern_signature.to_key()?;
            let tool_seq_json = serde_json::to_string(&entry.pattern_signature.tool_sequence)?;
            let stats_json = serde_json::to_string(&entry.execution_stats)?;
            let created_iso = entry.created_at.to_rfc3339();
            let updated_iso = entry.updated_at.to_rfc3339();

            // 检查是否已存在相同签名
            let existing: Option<(String, String)> = conn
                .query_row(
                    "SELECT output, execution_stats FROM procedural_memory WHERE pattern_key = ?1;",
                    params![pattern_key],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                )
                .optional()?;

            if let Some((existing_output, _)) = existing {
                // 签名已存在,检查 output 是否一致
                if existing_output != entry.output {
                    return Err(MlcError::PatternConflict {
                        signature: pattern_key,
                    });
                }
                // output 一同,仅更新 execution_stats 与 updated_at
                conn.execute(
                    "UPDATE procedural_memory
                     SET execution_stats = ?1, updated_at = ?2
                     WHERE pattern_key = ?3;",
                    params![stats_json, updated_iso, pattern_key],
                )?;
                trace!(pattern_key = %pattern_key, "L3 程序记忆已更新执行统计");
            } else {
                // 新条目,插入
                conn.execute(
                    "INSERT INTO procedural_memory
                        (pattern_key, tool_sequence, context_hash, output,
                         execution_stats, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7);",
                    params![
                        pattern_key,
                        tool_seq_json,
                        entry.pattern_signature.context_hash,
                        entry.output,
                        stats_json,
                        created_iso,
                        updated_iso,
                    ],
                )?;
                trace!(pattern_key = %pattern_key, "L3 程序记忆已插入");
            }

            Ok(())
        })
        .await
        .map_err(|e| MlcError::StorageError(format!("spawn_blocking join 错误: {e}")))?
    }

    /// 异步批量插入程序记忆条目(单事务,提升批量写入性能)
    ///
    /// WHY:批量插入场景下,单事务包裹多个 INSERT 比 N 次独立 INSERT 快 10-100 倍,
    /// 避免每次提交都触发 fsync。失败时整个事务回滚,保证原子性。
    ///
    /// 注意:批量插入使用 INSERT 语义(主键冲突时返回错误并回滚整个事务),
    /// 适用于初始化加载场景。运行时插入请使用 `insert`。
    pub async fn insert_batch(&self, entries: Vec<ProceduralEntry>) -> Result<(), MlcError> {
        let conn = self.conn.clone();
        spawn_blocking(move || {
            let mut conn = conn
                .lock()
                .map_err(|e| MlcError::StorageError(format!("L3 mutex poisoned: {e}")))?;

            let tx = conn.transaction()?;
            // WHY 使用 transaction():原实现用手动 BEGIN/COMMIT,任一插入失败时
            // ? 提前返回但未显式 ROLLBACK,已插入条目可能残留。改用 Transaction 包裹,
            // Drop 自动回滚未 commit 的事务,确保原子性。
            for entry in &entries {
                let pattern_key = entry.pattern_signature.to_key()?;
                let tool_seq_json = serde_json::to_string(&entry.pattern_signature.tool_sequence)?;
                let stats_json = serde_json::to_string(&entry.execution_stats)?;
                let created_iso = entry.created_at.to_rfc3339();
                let updated_iso = entry.updated_at.to_rfc3339();

                tx.execute(
                    "INSERT INTO procedural_memory
                        (pattern_key, tool_sequence, context_hash, output,
                         execution_stats, created_at, updated_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7);",
                    params![
                        pattern_key,
                        tool_seq_json,
                        entry.pattern_signature.context_hash,
                        entry.output,
                        stats_json,
                        created_iso,
                        updated_iso,
                    ],
                )?;
            }
            tx.commit()?;

            trace!(count = entries.len(), "L3 程序记忆批量条目已插入/更新");
            Ok(())
        })
        .await
        .map_err(|e| MlcError::StorageError(format!("spawn_blocking join 错误: {e}")))?
    }

    /// 异步按模式签名精确匹配查找
    ///
    /// 返回匹配的程序记忆条目;若不存在返回 None。
    ///
    /// WHY 接受引用:保持与同步版本兼容的 API,内部 clone 后传入 spawn_blocking
    pub async fn match_pattern(
        &self,
        signature: &PatternSignature,
    ) -> Result<Option<ProceduralEntry>, MlcError> {
        let conn = self.conn.clone();
        let signature = signature.clone();
        spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| MlcError::StorageError(format!("L3 mutex poisoned: {e}")))?;

            let pattern_key = signature.to_key()?;
            let result = conn
                .query_row(
                    "SELECT pattern_key, tool_sequence, context_hash, output,
                            execution_stats, created_at, updated_at
                     FROM procedural_memory WHERE pattern_key = ?1;",
                    params![pattern_key],
                    row_to_entry,
                )
                .optional()?;

            Ok(result)
        })
        .await
        .map_err(|e| MlcError::StorageError(format!("spawn_blocking join 错误: {e}")))?
    }

    /// 异步更新执行统计(记录一次执行结果)
    ///
    /// 若模式签名不存在,返回 `EntryNotFound` 错误。
    ///
    /// WHY 接受引用:保持与同步版本兼容的 API,内部 clone 后传入 spawn_blocking
    pub async fn update_stats(
        &self,
        signature: &PatternSignature,
        success: bool,
        latency_ms: u64,
    ) -> Result<(), MlcError> {
        let conn = self.conn.clone();
        let signature = signature.clone();
        spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| MlcError::StorageError(format!("L3 mutex poisoned: {e}")))?;

            let pattern_key = signature.to_key()?;

            // WHY 原子更新:原实现 SELECT → 修改 → UPDATE 模式存在丢失更新,改为单条 SQL 原子更新
            let success_inc: i64 = if success { 1 } else { 0 };
            let failure_inc: i64 = if !success { 1 } else { 0 };
            let updated_iso = Utc::now().to_rfc3339();

            let affected = conn.execute(
                "UPDATE procedural_memory
                 SET execution_stats = json_set(
                        execution_stats,
                        '$.success_count',
                            json_extract(execution_stats, '$.success_count') + ?1,
                        '$.failure_count',
                            json_extract(execution_stats, '$.failure_count') + ?2,
                        '$.total_latency_ms',
                            json_extract(execution_stats, '$.total_latency_ms') + ?3,
                        '$.last_executed_at', ?4
                     ),
                     updated_at = ?5
                 WHERE pattern_key = ?6;",
                params![
                    success_inc,
                    failure_inc,
                    latency_ms as i64,
                    updated_iso,
                    updated_iso,
                    pattern_key,
                ],
            )?;

            if affected == 0 {
                return Err(MlcError::EntryNotFound(format!(
                    "L3 程序记忆模式: {pattern_key}"
                )));
            }

            trace!(
                pattern_key = %pattern_key,
                success,
                latency_ms,
                "L3 程序记忆执行统计已更新"
            );
            Ok(())
        })
        .await
        .map_err(|e| MlcError::StorageError(format!("spawn_blocking join 错误: {e}")))?
    }

    /// 异步加载所有程序记忆条目(用于启动时恢复缓存)
    pub async fn load_all(&self) -> Result<Vec<ProceduralEntry>, MlcError> {
        let conn = self.conn.clone();
        spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| MlcError::StorageError(format!("L3 mutex poisoned: {e}")))?;

            let mut stmt = conn.prepare(
                "SELECT pattern_key, tool_sequence, context_hash, output,
                        execution_stats, created_at, updated_at
                 FROM procedural_memory ORDER BY created_at ASC;",
            )?;
            let rows = stmt.query_map([], row_to_entry)?;

            let mut entries = Vec::new();
            for row in rows {
                entries.push(row?);
            }
            Ok(entries)
        })
        .await
        .map_err(|e| MlcError::StorageError(format!("spawn_blocking join 错误: {e}")))?
    }

    /// 异步删除指定模式签名的条目
    ///
    /// 若不存在,返回 `EntryNotFound` 错误。
    ///
    /// WHY 接受引用:保持与同步版本兼容的 API,内部 clone 后传入 spawn_blocking
    pub async fn delete(&self, signature: &PatternSignature) -> Result<(), MlcError> {
        let conn = self.conn.clone();
        let signature = signature.clone();
        spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| MlcError::StorageError(format!("L3 mutex poisoned: {e}")))?;

            let pattern_key = signature.to_key()?;
            let affected = conn.execute(
                "DELETE FROM procedural_memory WHERE pattern_key = ?1;",
                params![pattern_key],
            )?;

            if affected == 0 {
                return Err(MlcError::EntryNotFound(format!(
                    "L3 程序记忆模式: {pattern_key}"
                )));
            }
            Ok(())
        })
        .await
        .map_err(|e| MlcError::StorageError(format!("spawn_blocking join 错误: {e}")))?
    }

    /// 异步计算条目总数
    pub async fn count(&self) -> Result<u64, MlcError> {
        let conn = self.conn.clone();
        spawn_blocking(move || {
            let conn = conn
                .lock()
                .map_err(|e| MlcError::StorageError(format!("L3 mutex poisoned: {e}")))?;
            let count: i64 =
                conn.query_row("SELECT COUNT(*) FROM procedural_memory;", [], |row| {
                    row.get(0)
                })?;
            Ok(u64::try_from(count).unwrap_or(0))
        })
        .await
        .map_err(|e| MlcError::StorageError(format!("spawn_blocking join 错误: {e}")))?
    }
}

/// 应用 SQLite 性能优化 PRAGMA(在 WAL 模式设置之后调用)
///
/// WHY:SubTask 21.2 — 委托给 `nexus_core::sqlite_pragma::apply_performance_pragmas`,
/// 消除与 cmt-tiering(cold.rs / warm.rs)的重复实现,统一 PRAGMA 配置。
fn apply_performance_pragmas(conn: &Connection) -> Result<(), MlcError> {
    nexus_core::sqlite_pragma::apply_performance_pragmas(conn)
        .map_err(|e| MlcError::StorageError(format!("SQLite PRAGMA 设置失败: {e}")))
}

/// 将 SQLite 行映射为 ProceduralEntry
///
/// 字段顺序与 SELECT 语句对齐:
/// pattern_key, tool_sequence, context_hash, output,
/// execution_stats, created_at, updated_at
fn row_to_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProceduralEntry> {
    let _pattern_key: String = row.get(0)?;
    let tool_seq_json: String = row.get(1)?;
    let context_hash: String = row.get(2)?;
    let output: String = row.get(3)?;
    let stats_json: String = row.get(4)?;
    let created_iso: String = row.get(5)?;
    let updated_iso: String = row.get(6)?;

    // 反序列化工具序列列(JSON 失败时降级为空数组,不阻断查询)
    let tool_sequence: Vec<String> = serde_json::from_str(&tool_seq_json).unwrap_or_default();

    // 反序列化执行统计(JSON 失败时降级为空统计)
    let execution_stats: ExecutionStats = serde_json::from_str(&stats_json).unwrap_or_default();

    // 时间戳解析(失败时降级为当前时间,不阻断查询)
    let created_at = DateTime::parse_from_rfc3339(&created_iso)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    let updated_at = DateTime::parse_from_rfc3339(&updated_iso)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());

    Ok(ProceduralEntry {
        pattern_signature: PatternSignature::new(tool_sequence, context_hash),
        execution_stats,
        output,
        created_at,
        updated_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_signature(suffix: &str) -> PatternSignature {
        PatternSignature::new(
            vec!["tool_a".into(), format!("tool_{suffix}")],
            format!("hash-{suffix}"),
        )
    }

    #[tokio::test]
    async fn test_open_in_memory() {
        let mem = ProceduralMemory::open_in_memory().unwrap();
        assert_eq!(mem.count().await.unwrap(), 0);
    }

    #[tokio::test]
    async fn test_insert_and_match_pattern() {
        let mem = ProceduralMemory::open_in_memory().unwrap();
        let sig = make_signature("1");
        let entry = ProceduralEntry::new(sig.clone(), "output-1");

        mem.insert(&entry).await.unwrap();
        assert_eq!(mem.count().await.unwrap(), 1);

        let matched = mem.match_pattern(&sig).await.unwrap();
        assert!(matched.is_some());
        let matched = matched.unwrap();
        assert_eq!(matched.pattern_signature, make_signature("1"));
        assert_eq!(matched.output, "output-1");
    }

    #[tokio::test]
    async fn test_match_pattern_nonexistent() {
        let mem = ProceduralMemory::open_in_memory().unwrap();
        let sig = make_signature("nonexistent");
        let result = mem.match_pattern(&sig).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_insert_pattern_conflict() {
        let mem = ProceduralMemory::open_in_memory().unwrap();
        let sig = make_signature("1");

        // 插入 output-1
        let entry1 = ProceduralEntry::new(sig.clone(), "output-1");
        mem.insert(&entry1).await.unwrap();

        // 用相同签名但不同 output 插入,应返回 PatternConflict
        let entry2 = ProceduralEntry::new(sig.clone(), "output-2");
        let err = mem.insert(&entry2).await.unwrap_err();
        assert!(matches!(err, MlcError::PatternConflict { .. }));
    }

    #[tokio::test]
    async fn test_insert_same_output_updates_stats() {
        let mem = ProceduralMemory::open_in_memory().unwrap();
        let sig = make_signature("1");

        // 插入 entry(output-1, 空统计)
        let entry1 = ProceduralEntry::new(sig.clone(), "output-1");
        mem.insert(&entry1).await.unwrap();

        // 用相同签名相同 output 但不同统计插入,应更新统计
        let mut entry2 = ProceduralEntry::new(sig.clone(), "output-1");
        entry2.execution_stats.record(true, 100);
        mem.insert(&entry2).await.unwrap();

        let matched = mem.match_pattern(&sig).await.unwrap().unwrap();
        assert_eq!(matched.execution_stats.success_count, 1);
        assert_eq!(matched.execution_stats.total_latency_ms, 100);
    }

    #[tokio::test]
    async fn test_update_stats() {
        let mem = ProceduralMemory::open_in_memory().unwrap();
        let sig = make_signature("1");
        let entry = ProceduralEntry::new(sig.clone(), "output-1");
        mem.insert(&entry).await.unwrap();

        // 记录两次执行
        mem.update_stats(&sig, true, 100).await.unwrap();
        mem.update_stats(&sig, false, 50).await.unwrap();

        let matched = mem.match_pattern(&sig).await.unwrap().unwrap();
        assert_eq!(matched.execution_stats.success_count, 1);
        assert_eq!(matched.execution_stats.failure_count, 1);
        assert_eq!(matched.execution_stats.total_latency_ms, 150);
        assert!(matched.execution_stats.last_executed_at.is_some());
    }

    #[tokio::test]
    async fn test_update_stats_nonexistent() {
        let mem = ProceduralMemory::open_in_memory().unwrap();
        let sig = make_signature("nonexistent");
        let result = mem.update_stats(&sig, true, 100).await;
        assert!(matches!(result, Err(MlcError::EntryNotFound(_))));
    }

    #[tokio::test]
    async fn test_load_all() {
        let mem = ProceduralMemory::open_in_memory().unwrap();

        for i in 0..3 {
            let sig = make_signature(&i.to_string());
            let entry = ProceduralEntry::new(sig, format!("output-{i}"));
            mem.insert(&entry).await.unwrap();
        }

        let all = mem.load_all().await.unwrap();
        assert_eq!(all.len(), 3);
    }

    #[tokio::test]
    async fn test_load_all_empty() {
        let mem = ProceduralMemory::open_in_memory().unwrap();
        let all = mem.load_all().await.unwrap();
        assert!(all.is_empty());
    }

    #[tokio::test]
    async fn test_delete() {
        let mem = ProceduralMemory::open_in_memory().unwrap();
        let sig = make_signature("1");
        let entry = ProceduralEntry::new(sig.clone(), "output-1");
        mem.insert(&entry).await.unwrap();
        assert_eq!(mem.count().await.unwrap(), 1);

        mem.delete(&sig).await.unwrap();
        assert_eq!(mem.count().await.unwrap(), 0);
        assert!(mem.match_pattern(&sig).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn test_delete_nonexistent() {
        let mem = ProceduralMemory::open_in_memory().unwrap();
        let sig = make_signature("nonexistent");
        let result = mem.delete(&sig).await;
        assert!(matches!(result, Err(MlcError::EntryNotFound(_))));
    }

    #[tokio::test]
    async fn test_persistence_roundtrip() {
        // 使用临时文件验证 SQLite 持久化往返一致性
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test_procedural.db");

        // 写入数据
        {
            let mem = ProceduralMemory::open(&db_path).unwrap();
            let sig = make_signature("1");
            let entry = ProceduralEntry::new(sig.clone(), "output-1");
            mem.insert(&entry).await.unwrap();
            mem.update_stats(&sig, true, 100).await.unwrap();
        }

        // 重新打开并验证
        {
            let mem = ProceduralMemory::open(&db_path).unwrap();
            assert_eq!(mem.count().await.unwrap(), 1);

            let sig = make_signature("1");
            let matched = mem.match_pattern(&sig).await.unwrap().unwrap();
            assert_eq!(matched.output, "output-1");
            assert_eq!(matched.execution_stats.success_count, 1);
            assert_eq!(matched.execution_stats.total_latency_ms, 100);
        }
    }

    #[tokio::test]
    async fn test_wal_mode_enabled() {
        let tmp = tempfile::tempdir().unwrap();
        let db_path = tmp.path().join("test_wal.db");
        let mem = ProceduralMemory::open(&db_path).unwrap();

        let conn = mem.conn.lock().unwrap();
        let mode: String = conn
            .query_row("PRAGMA journal_mode;", [], |row| row.get(0))
            .unwrap();
        assert_eq!(mode.to_lowercase(), "wal");
    }

    #[tokio::test]
    async fn test_insert_batch() {
        let mem = ProceduralMemory::open_in_memory().unwrap();
        let entries = vec![
            ProceduralEntry::new(make_signature("1"), "output-1"),
            ProceduralEntry::new(make_signature("2"), "output-2"),
            ProceduralEntry::new(make_signature("3"), "output-3"),
        ];
        mem.insert_batch(entries).await.unwrap();
        assert_eq!(mem.count().await.unwrap(), 3);

        // 验证条目内容
        let sig = make_signature("2");
        let matched = mem.match_pattern(&sig).await.unwrap().unwrap();
        assert_eq!(matched.output, "output-2");
    }
}
