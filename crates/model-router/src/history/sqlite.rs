//! `SqliteHistoryStore` — SQLite 持久化历史存储(v1.4.0 P1 新增)
//!
//! 对应架构层:L1 Core(model-router)
//!
//! # 设计动机
//! v1.3.0 的 `InMemoryHistoryStore` 在进程重启后丢失历史,导致 M2 RL 路由
//! 触发条件(历史数据 > 10000 条)无法在短周期内达成。`SqliteHistoryStore`
//! 将历史数据写入 SQLite 文件,跨重启保留,为 M2 RL 路由提供持久化基础。
//!
//! # 关键设计决策
//! - **同步实现**:`HistoryStore` trait 是同步的(`&self` 方法),`SqliteHistoryStore`
//!   的 get/record 也是同步的。SQLite 单行 UPSERT/SELECT 是微秒级操作,在同步
//!   上下文中调用可接受。调用方在 async 上下文中调用 `gate()` 时,需用
//!   `spawn_blocking` 包装整个 `gate()` 调用(§4.4 #7 fire-and-forget 评估框架)。
//! - **`Mutex<Connection>`**:与 repo-wiki 模式一致,写操作互斥。
//!   单连接简化事务语义(无需连接池),适合"低频写、偶发读"的历史统计场景。
//! - **MessagePack 序列化**:`VecDeque<f32>` → rmp-serde → BLOB。
//!   WHY rmp-serde 而非 JSON:ADR-004 已采用 MessagePack 作为消息序列化协议,
//!   二进制格式比 JSON 紧凑(每 f32 用 5 字节 vs JSON 的 ~12 字符),且
//!   `VecDeque<f32>` 已实现 serde Serialize/Deserialize(标准支持)。
//! - **UPSERT 语义**:`SELECT 旧值 → 合并 → INSERT OR REPLACE`。
//!   WHY 不用 `ON CONFLICT DO UPDATE SET`:SQLite UPSERT 只能做简单算术合并
//!   (`success_count = success_count + ?`),无法表达 `VecDeque` 滑动窗口的
//!   `pop_front + push_back` 合并语义。`Mutex<Connection>` 保证串行访问,
//!   SELECT-merge-INSERT OR REPLACE 在 Mutex 保护下不存在 TOCTOU(§4.4 #1
//!   反模式:锁内取快照→释放→await;这里锁内全部完成,不跨 await)。
//! - **WAL 模式**:Write-Ahead Logging 提升崩溃恢复友好性,进程异常退出后
//!   下次打开自动恢复 WAL 中的未 checkpoint 数据(测试 2 持久化验证依赖此特性)。
//!
//! # Schema
//! ```sql
//! CREATE TABLE IF NOT EXISTS history (
//!     model_id        TEXT PRIMARY KEY,
//!     success_count  INTEGER NOT NULL DEFAULT 0,
//!     total_count    INTEGER NOT NULL DEFAULT 0,
//!     latency_samples BLOB NOT NULL        -- MessagePack 序列化 VecDeque<f32>
//! );
//! ```
//!
//! # 向后兼容
//! 默认仍使用 `InMemoryHistoryStore`(v1.3.0 行为不变),SQLite 为 opt-in
//! (`RouterConfig.history_persistence = HistoryPersistence::Sqlite { db_path }`)。

#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::RouterError;
use crate::history::{HistoryRecord, HistoryStore, LATENCY_WINDOW_CAPACITY};

/// SQLite 持久化历史存储 — v1.4.0 P1 新增
///
/// # 线程安全
/// - 内部 `Mutex<Connection>` 保证写操作串行,无需外部同步
/// - `Send + Sync` 满足 `HistoryStore` trait 约束
/// - `Mutex::lock()` 在 panic 时 poison,`record`/`get` 用 `into_inner` 恢复
///   (trait 签名不允许返回 Result,故选择继续而非 panic propagate)
///
/// # 使用示例
/// ```
/// use model_router::{SqliteHistoryStore, HistoryStore};
/// use tempfile::tempdir;
///
/// # fn main() -> Result<(), Box<dyn std::error::Error>> {
/// let tmp = tempdir()?;
/// let db_path = tmp.path().join("history.db");
/// let store = SqliteHistoryStore::new(&db_path)?;
/// store.record("gpt-4", 200.0, true);
/// let record = store.get("gpt-4").unwrap();
/// assert_eq!(record.total_count, 1);
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct SqliteHistoryStore {
    /// 单连接 + Mutex 串行化所有读写(与 repo-wiki 写线程模式不同:
    /// 历史存储是低频写偶发读,单连接简化事务语义,无需连接池)
    conn: Mutex<Connection>,
    /// 数据库文件路径(用于调试与诊断)
    db_path: PathBuf,
}

impl SqliteHistoryStore {
    /// 打开/创建 SQLite 历史数据库
    ///
    /// 自动启用 WAL 模式(崩溃恢复友好)+ 创建 schema(若不存在)。
    /// 若 `db_path` 指向的文件已存在(如重启场景),自动加载已有数据。
    ///
    /// # 错误
    /// - 文件路径不可写 / 无权限
    /// - SQLite pragma 设置失败
    /// - schema 创建失败
    pub fn new(path: &Path) -> Result<Self, RouterError> {
        let conn = Connection::open(path)
            .map_err(|e| RouterError::SqliteHistoryError(format!("open: {e}")))?;

        // 启用 WAL 模式:Write-Ahead Logging 提升崩溃恢复友好性
        // WHY WAL 而非 DELETE/TRUNCATE:WAL 在进程异常退出后,下次打开自动恢复
        // 未 checkpoint 的数据,保证持久化测试(Drop → 重新打开)数据不丢失。
        // synchronous=NORMAL:WAL 下仍保证一致性但减少 fsync 开销(与 repo-wiki 一致)
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| RouterError::SqliteHistoryError(format!("pragma journal_mode: {e}")))?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(|e| RouterError::SqliteHistoryError(format!("pragma synchronous: {e}")))?;

        init_schema(&conn)?;

        Ok(Self {
            conn: Mutex::new(conn),
            db_path: path.to_path_buf(),
        })
    }

    /// 返回数据库文件路径(用于调试与诊断)
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }
}

/// 初始化 schema — 创建 history 表(若不存在)
fn init_schema(conn: &Connection) -> Result<(), RouterError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS history (
            model_id        TEXT PRIMARY KEY,
            success_count  INTEGER NOT NULL DEFAULT 0,
            total_count    INTEGER NOT NULL DEFAULT 0,
            latency_samples BLOB NOT NULL DEFAULT x''
        );",
    )
    .map_err(|e| RouterError::SqliteHistoryError(format!("init schema: {e}")))?;
    Ok(())
}

impl HistoryStore for SqliteHistoryStore {
    fn get(&self, model_id: &str) -> Option<HistoryRecord> {
        // WHY lock().into_inner():trait 签名返回 Option,无法传播 Mutex poison 错误。
        // poisoned Mutex 意味着前一次操作 panic,数据可能不一致 — 返回 None 降级
        // (MoeGate 降级三维评分,不阻断路由),比 panic propagate 更安全。
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        // SELECT 旧值:optional() 将 "无行" 转为 Ok(None),非错误
        // .ok().flatten()?:Result<Option<T>, E> → Option<Option<T>> → Option<T> → T(or None)
        // `?` 在 fn -> Option<HistoryRecord> 中:None 时提前返回 None(降级三维评分)
        let row: (i64, i64, Vec<u8>) = conn
            .query_row(
                "SELECT success_count, total_count, latency_samples
                 FROM history WHERE model_id = ?1;",
                params![model_id],
                |row| {
                    let sc: i64 = row.get(0)?;
                    let tc: i64 = row.get(1)?;
                    let blob: Vec<u8> = row.get(2)?;
                    Ok((sc, tc, blob))
                },
            )
            .optional()
            .ok()
            .flatten()?;

        let (success_count, total_count, blob) = row;

        // 反序列化 latency_samples BLOB → VecDeque<f32>
        // WHY unwrap_or_default:blob 损坏(如旧版本数据)时降级为空 VecDeque,
        // 而非 panic 或返回 None(total_count/success_count 仍可用)
        let latency_samples: VecDeque<f32> = rmp_serde::from_slice(&blob).unwrap_or_else(|_| {
            tracing::warn!(
                model_id = model_id,
                blob_len = blob.len(),
                "latency_samples 反序列化失败,降级为空 VecDeque"
            );
            VecDeque::with_capacity(LATENCY_WINDOW_CAPACITY)
        });

        Some(HistoryRecord {
            success_count: success_count as u64,
            total_count: total_count as u64,
            latency_samples,
        })
    }

    fn record(&self, model_id: &str, latency_ms: f32, success: bool) {
        // §4.4 #1 反模式防护:锁内全部完成,不跨 await(MutexGuard 不跨 .await)
        let conn = self.conn.lock().unwrap_or_else(|e| e.into_inner());

        // 1. SELECT 旧值(Mutex 保证串行访问,无 TOCTOU)
        let old: Option<(u64, u64, Vec<u8>)> = conn
            .query_row(
                "SELECT success_count, total_count, latency_samples
                 FROM history WHERE model_id = ?1;",
                params![model_id],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)? as u64,
                        row.get::<_, i64>(1)? as u64,
                        row.get::<_, Vec<u8>>(2)?,
                    ))
                },
            )
            .optional()
            .ok()
            .flatten();

        // 2. 合并:构造新的 HistoryRecord
        // WHY 不用 ON CONFLICT DO UPDATE:滑动窗口合并(VecDeque pop_front + push_back)
        // 无法用 SQL 表达,必须在应用层反序列化→合并→序列化。Mutex 保证此过程原子。
        let mut record = match old {
            Some((sc, tc, blob)) => {
                let samples: VecDeque<f32> = rmp_serde::from_slice(&blob).unwrap_or_else(|_| {
                    tracing::warn!(
                        model_id = model_id,
                        blob_len = blob.len(),
                        "旧 latency_samples 反序列化失败,降级为空 VecDeque"
                    );
                    VecDeque::with_capacity(LATENCY_WINDOW_CAPACITY)
                });
                HistoryRecord {
                    success_count: sc,
                    total_count: tc,
                    latency_samples: samples,
                }
            }
            None => HistoryRecord::new(),
        };
        record.record(latency_ms, success);

        // 3. 序列化 latency_samples → BLOB
        let blob = match rmp_serde::to_vec(&record.latency_samples) {
            Ok(b) => b,
            Err(e) => {
                // fire-and-forget 语义:序列化失败仅记日志,不 panic(§4.4 #7)
                tracing::warn!(error = %e, model_id = model_id, "latency_samples 序列化失败,跳过本次 record");
                return;
            }
        };

        // 4. INSERT OR REPLACE(Mutex 保证串行,无 TOCTOU)
        if let Err(e) = conn.execute(
            "INSERT OR REPLACE INTO history
                (model_id, success_count, total_count, latency_samples)
             VALUES (?1, ?2, ?3, ?4);",
            params![
                model_id,
                record.success_count as i64,
                record.total_count as i64,
                blob,
            ],
        ) {
            // fire-and-forget 语义:写入失败仅记日志,不 panic
            tracing::warn!(error = %e, model_id = model_id, "SQLite history record 写入失败");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_creates_schema() {
        let tmp = tempfile::tempdir().expect("tempdir 失败");
        let db_path = tmp.path().join("schema_test.db");
        let store = SqliteHistoryStore::new(&db_path).expect("打开失败");

        // 验证 schema 已创建:查询 sqlite_master 表
        let conn = store.conn.lock().unwrap();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='history';",
                [],
                |row| row.get(0),
            )
            .expect("查询 sqlite_master 失败");
        assert_eq!(count, 1, "history 表应已创建");
    }

    #[test]
    fn test_new_reuses_existing_database() {
        let tmp = tempfile::tempdir().expect("tempdir 失败");
        let db_path = tmp.path().join("reuse_test.db");

        // 第一次打开,写入数据
        {
            let store = SqliteHistoryStore::new(&db_path).expect("首次打开失败");
            store.record("model-a", 100.0, true);
        }

        // 第二次打开同路径,验证数据保留
        let store = SqliteHistoryStore::new(&db_path).expect("重新打开失败");
        let record = store.get("model-a").expect("数据应保留");
        assert_eq!(record.total_count, 1);
        assert_eq!(record.success_count, 1);
    }

    #[test]
    fn test_db_path_accessor() {
        let tmp = tempfile::tempdir().expect("tempdir 失败");
        let db_path = tmp.path().join("path_test.db");
        let store = SqliteHistoryStore::new(&db_path).expect("打开失败");
        assert_eq!(store.db_path(), db_path);
    }
}
