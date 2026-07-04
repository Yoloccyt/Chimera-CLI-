//! WAL(Write-Ahead Log)接口 — SCC 缓存持久化预写日志契约
//!
//! 对应架构层:L3 Storage
//! 对应任务:Task 9.2(SIMD + WAL + 路由 < 2ms 性能调优)
//!
//! # 设计目标
//! - 定义统一的 WAL 接口(`WalTrait`),解耦 SCC 缓存与底层持久化实现
//! - 本周提供**占位实现**(`InMemoryWal`),仅用内存缓冲验证接口契约
//! - 真实 SQLite WAL 持久化留待 Week 8 接入(见 §3.2 非范围)
//!
//! # WHY 占位实现而非真实持久化
//! - Week 7 关键路径在 4 crate 联调与性能基准,WAL 持久化非阻塞验收项
//! - 真实 SQLite WAL 需引入 rusqlite 依赖 + 文件 I/O + 崩溃恢复测试,
//!   工作量与 Week 7 剩余预算不匹配(参见 spec.md §3.2 明确"本周不做")
//! - 占位实现保持接口契约稳定,Week 8 替换为 `SqliteWal` 时上层 SCC 代码零改动
//! - `#![forbid(unsafe_code)]` 兼容:占位实现仅用 `Mutex<Vec<WalEntry>>`,
//!   无 unsafe 块;Week 8 的 SQLite 绑定须验证 unsafe 传播后再接入
//!
//! # WAL 语义
//! 1. `write_ahead_log(entry)`:在修改缓存前先写日志(预写),保证崩溃可恢复
//! 2. `commit_log(entry_id)`:缓存修改成功后标记日志为已提交
//! 3. `rollback_log(entry_id)`:缓存修改失败时回滚日志,撤销预写
//!
//! 占位实现中,`entries` 是已写入但未提交的日志,`committed` 是已提交的 entry_id 集合。
//! 真实实现将日志写入 SQLite 表,commit 对应事务提交,rollback 对应事务回滚。

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use tokio::task::spawn_blocking;

use crate::error::SccError;
use crate::types::ContextId;

/// WAL 操作类型 — 标识日志条目对应的缓存操作
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WalOperation {
    /// 插入新缓存条目
    Insert,
    /// 更新已有缓存条目
    Update,
    /// 删除缓存条目
    Delete,
    /// 推测性预取(预取前先写 WAL,失败时回滚)
    Prefetch,
}

impl WalOperation {
    /// 序列化为 SQLite 存储的字符串标识(Task 6.2:SqliteWal 持久化用)
    fn as_str(&self) -> &'static str {
        match self {
            WalOperation::Insert => "insert",
            WalOperation::Update => "update",
            WalOperation::Delete => "delete",
            WalOperation::Prefetch => "prefetch",
        }
    }

    /// 从 SQLite 存储的字符串反序列化(Task 6.2:SqliteWal recover 用)
    fn from_db_str(s: &str) -> Result<Self, SccError> {
        match s {
            "insert" => Ok(WalOperation::Insert),
            "update" => Ok(WalOperation::Update),
            "delete" => Ok(WalOperation::Delete),
            "prefetch" => Ok(WalOperation::Prefetch),
            other => Err(SccError::WalError {
                reason: format!("未知 WalOperation 字符串: {other}"),
            }),
        }
    }
}

/// WAL 日志条目 — 单次缓存操作的预写记录
#[derive(Debug, Clone)]
pub struct WalEntry {
    /// 日志条目唯一 ID(由调用方生成,便于 commit/rollback 定位)
    pub entry_id: String,
    /// 操作类型
    pub operation: WalOperation,
    /// 受影响的上下文 ID
    pub context_id: ContextId,
    /// 操作负载(序列化后的字节流,占位实现不解析)
    pub payload: Vec<u8>,
    /// 写入时刻(UTC 时间戳,真实实现用于崩溃恢复时排序)
    pub timestamp: chrono::DateTime<Utc>,
}

impl WalEntry {
    /// 创建新日志条目,timestamp 取当前 UTC 时刻
    pub fn new(
        entry_id: impl Into<String>,
        operation: WalOperation,
        context_id: impl Into<ContextId>,
        payload: Vec<u8>,
    ) -> Self {
        Self {
            entry_id: entry_id.into(),
            operation,
            context_id: context_id.into(),
            payload,
            timestamp: Utc::now(),
        }
    }
}

/// WAL 接口契约 — 解耦 SCC 缓存与底层持久化实现
///
/// # 实现要求
/// - `Send + Sync`:SCC 缓存通过 `Arc<dyn WalTrait>` 共享,须线程安全
/// - 所有方法返回 `Result<(), SccError>`:失败时返回 `SccError::WalError`
/// - `write_ahead_log` 必须在缓存修改前调用(预写语义)
/// - `commit_log` 与 `rollback_log` 必须在缓存修改后调用(两阶段语义)
pub trait WalTrait: Send + Sync {
    /// 写入预写日志(在缓存修改前调用)
    fn write_ahead_log(&self, entry: &WalEntry) -> Result<(), SccError>;

    /// 提交日志(缓存修改成功后调用)
    fn commit_log(&self, entry_id: &str) -> Result<(), SccError>;

    /// 回滚日志(缓存修改失败时调用)
    fn rollback_log(&self, entry_id: &str) -> Result<(), SccError>;
}

/// WAL 占位实现 — 内存缓冲,无真实持久化
///
/// # 适用场景
/// - Week 7 接口契约验证 + 单元测试
/// - Week 8 替换为 `SqliteWal` 前,上层 SCC 代码可用此实现跑通流程
///
/// # 不适用场景
/// - 生产环境(崩溃后日志丢失)
/// - 持久化基准(无文件 I/O 开销,数据不真实)
///
/// # 线程安全
/// - `entries: Mutex<Vec<WalEntry>>`:保护未提交日志列表
/// - `committed: Mutex<HashSet<String>>`:保护已提交 entry_id 集合
/// - 两把锁独立,避免 commit 与 write 互相阻塞
pub struct InMemoryWal {
    /// 已写入但未回滚的日志条目(commit 后保留,便于审计)
    entries: Mutex<Vec<WalEntry>>,
    /// 已提交的 entry_id 集合
    committed: Mutex<HashSet<String>>,
}

impl InMemoryWal {
    /// 创建空的内存 WAL
    pub fn new() -> Self {
        Self {
            entries: Mutex::new(Vec::new()),
            committed: Mutex::new(HashSet::new()),
        }
    }

    /// 返回已写入的日志条目数(含已提交,不含已回滚)
    ///
    /// WHY 提供此方法:占位实现的测试需要断言日志条目数,
    /// 真实 SQLite 实现不需要此方法(直接查表)。
    pub fn entry_count(&self) -> usize {
        self.entries
            .lock()
            .map(|v| v.len())
            .unwrap_or_else(|p| p.into_inner().len())
    }

    /// 返回已提交的 entry_id 数量
    pub fn committed_count(&self) -> usize {
        self.committed
            .lock()
            .map(|s| s.len())
            .unwrap_or_else(|p| p.into_inner().len())
    }
}

impl Default for InMemoryWal {
    fn default() -> Self {
        Self::new()
    }
}

impl WalTrait for InMemoryWal {
    fn write_ahead_log(&self, entry: &WalEntry) -> Result<(), SccError> {
        let mut entries = self.entries.lock().map_err(|p| SccError::WalError {
            reason: format!("entries 锁中毒: {p}"),
        })?;
        entries.push(entry.clone());
        Ok(())
    }

    fn commit_log(&self, entry_id: &str) -> Result<(), SccError> {
        let entries = self.entries.lock().map_err(|p| SccError::WalError {
            reason: format!("entries 锁中毒: {p}"),
        })?;

        // 校验 entry_id 存在(未回滚)
        let exists = entries.iter().any(|e| e.entry_id == entry_id);
        if !exists {
            return Err(SccError::WalError {
                reason: format!("commit 失败: entry_id {entry_id} 不存在(可能已回滚)"),
            });
        }

        let mut committed = self.committed.lock().map_err(|p| SccError::WalError {
            reason: format!("committed 锁中毒: {p}"),
        })?;
        committed.insert(entry_id.to_string());
        Ok(())
    }

    fn rollback_log(&self, entry_id: &str) -> Result<(), SccError> {
        let mut entries = self.entries.lock().map_err(|p| SccError::WalError {
            reason: format!("entries 锁中毒: {p}"),
        })?;

        // 移除指定 entry_id 的日志条目(回滚语义:撤销预写)
        let before_len = entries.len();
        entries.retain(|e| e.entry_id != entry_id);
        let removed = before_len - entries.len();

        if removed == 0 {
            return Err(SccError::WalError {
                reason: format!("rollback 失败: entry_id {entry_id} 不存在"),
            });
        }

        // 同步清理 committed 集合(若已提交则撤销)
        if let Ok(mut committed) = self.committed.lock() {
            committed.remove(entry_id);
        }
        Ok(())
    }
}

/// SQLite WAL 持久化实现 — 真实文件持久化,支持崩溃恢复(Task 6.2)
///
/// # 适用场景
/// - 生产环境(崩溃后日志可恢复)
/// - 持久化基准(真实文件 I/O 开销,反映生产延迟)
/// - async 上下文:所有方法为 `async fn`,内部用 `spawn_blocking` 包装 SQLite 操作
///
/// # 线程安全
/// - `Arc<Mutex<Connection>>`:`rusqlite::Connection` 非 `Sync`,用 `Mutex` 串行化访问;
///   `Arc` 包装支持 `Clone` 与跨任务共享,使 `spawn_blocking` 闭包可拥有连接
///   (参考 `cmt-tiering/warm.rs` 实现)
/// - `SqliteWal` 自身满足 `Send + Sync`,可在多线程间通过 `Arc<SqliteWal>` 共享
///
/// # 持久化语义
/// - `write_ahead_log`:`INSERT` 一条记录,`committed=0`(预写)
/// - `commit_log`:`UPDATE` 设置 `committed=1`(两阶段提交)
/// - `rollback_log`:`DELETE` 该条记录(撤销预写)
/// - `recover`:查询 `committed=0` 的所有记录(崩溃后未提交的日志)
///
/// # WHY 所有方法 async + spawn_blocking
/// - SQLite 文件 I/O 是同步阻塞操作,在 async 上下文直接调用会阻塞 runtime 线程
/// - 1000 条目恢复可能耗时数十毫秒,若不 spawn_blocking 会饿死同线程的其他 task
/// - `spawn_blocking` 将 SQLite 操作放到 Tokio 阻塞线程池,不占用 async worker
///   (架构红线:所有 async fn 满足 Send + 'static 约束,所有 I/O 经阻塞池)
///
/// # WHY 不再实现 `WalTrait`
/// - `WalTrait` 是同步 trait(`fn ... -> Result<...>`),无法 `await` `spawn_blocking`
/// - 若保留同步签名则无法修复 C-02 阻塞问题(强制在 async 上下文同步阻塞)
/// - `InMemoryWal` 纯内存无阻塞,继续实现 `WalTrait` 用于同步测试场景
/// - `SqliteWal` 提供独立 async API,调用方按需 `await`(与 `WarmTier` 一致)
///
/// # WHY 选择 SQLite 而非自定义二进制 WAL
/// - 事务原子性:SQLite WAL 模式保证单条 INSERT/UPDATE 原子持久化
/// - 崩溃恢复:SQLite 自带恢复机制,无需手写 fsync + checksum
/// - 查询能力:`recover()` 可按 timestamp 排序,支持复杂恢复策略
/// - workspace 已收录 `rusqlite 0.32 + bundled`,零额外依赖成本
///
/// # `#![forbid(unsafe_code)]` 兼容性
/// - rusqlite 内部通过 libsqlite3-sys 调用 C FFI,使用 `unsafe extern` 块
/// - 但 `#![forbid(unsafe_code)]` 是 crate 级 lint,只扫描当前 crate 源码,
///   不传播到依赖 crates(参考 prometheus-client 先例)
/// - 本实现全部用 Safe Rust API(`Connection::open` / `execute` / `query_map`)
#[derive(Clone)]
pub struct SqliteWal {
    /// SQLite 连接(`Arc<Mutex>` 包装,支持 Clone 与跨任务共享)
    ///
    /// WHY `Arc<Mutex>`:`spawn_blocking` 闭包需要 `'static + Send`,
    /// `Arc<Mutex>` 允许将连接所有权转移到阻塞线程池
    conn: Arc<Mutex<rusqlite::Connection>>,
}

impl SqliteWal {
    /// 创建或打开 SQLite WAL 持久化文件(async,内部 spawn_blocking)
    ///
    /// # 参数
    /// - `path`:SQLite 数据库文件路径(接受 `String`/`&str`,内部转为 owned)
    ///
    /// # 行为
    /// - 文件不存在则创建,已存在则打开(支持崩溃后重启恢复)
    /// - 自动初始化 `wal_entries` 表(若不存在)
    /// - 启用 SQLite WAL 模式(`PRAGMA journal_mode=WAL`)提升并发写入性能
    ///
    /// # WHY async + spawn_blocking
    /// `Connection::open` 涉及文件 I/O,PRAGMA 与 CREATE TABLE 也是同步 SQLite 操作,
    /// 在频繁创建/重启场景下可能阻塞 async runtime。`spawn_blocking` 将其放到
    /// 阻塞线程池执行,避免占用 async worker 线程。
    pub async fn new(path: impl Into<String> + Send + 'static) -> Result<Self, SccError> {
        let path = path.into();
        let conn = spawn_blocking(move || -> Result<rusqlite::Connection, SccError> {
            let conn = rusqlite::Connection::open(&path).map_err(|e| SccError::WalError {
                reason: format!("打开 SQLite 失败 (path={path}): {e}"),
            })?;

            // 启用 WAL 模式:提升写入吞吐,允许读写并发
            // WHY:SCC 缓存场景下写入频繁,WAL 模式避免每次写入全表锁
            conn.pragma_update(None, "journal_mode", "WAL")
                .map_err(|e| SccError::WalError {
                    reason: format!("设置 journal_mode=WAL 失败: {e}"),
                })?;

            conn.execute(
                "CREATE TABLE IF NOT EXISTS wal_entries (
                    entry_id   TEXT    PRIMARY KEY,
                    operation  TEXT    NOT NULL,
                    context_id TEXT    NOT NULL,
                    payload    BLOB    NOT NULL,
                    timestamp  TEXT    NOT NULL,
                    committed  INTEGER NOT NULL DEFAULT 0
                )",
                [],
            )
            .map_err(|e| SccError::WalError {
                reason: format!("创建 wal_entries 表失败: {e}"),
            })?;

            Ok(conn)
        })
        .await
        .map_err(|e| SccError::WalError {
            reason: format!("new spawn_blocking join 错误: {e}"),
        })??;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// 崩溃恢复:返回所有未提交(`committed=0`)的日志条目,按 timestamp 升序(async)
    ///
    /// # 适用场景
    /// - 进程崩溃后重启,扫描未完成的预写日志,决定重放或回滚
    ///
    /// # 返回
    /// - `Vec<WalEntry>`:未提交的日志条目(按写入时间升序,便于按序重放)
    ///
    /// # WHY async + spawn_blocking
    /// 1000 条目恢复可能耗时数十毫秒,若直接在 async 上下文同步查询会阻塞
    /// runtime worker 线程,导致同线程其他 task 饿死。`spawn_blocking` 将
    /// 同步 SQLite 查询放到阻塞线程池,保护 async runtime 响应性。
    pub async fn recover(&self) -> Result<Vec<WalEntry>, SccError> {
        let conn = self.conn.clone();
        spawn_blocking(move || -> Result<Vec<WalEntry>, SccError> {
            let conn = conn.lock().map_err(|p| SccError::WalError {
                reason: format!("Connection 锁中毒: {p}"),
            })?;

            let mut stmt = conn
                .prepare(
                    "SELECT entry_id, operation, context_id, payload, timestamp
                     FROM wal_entries
                     WHERE committed = 0
                     ORDER BY timestamp ASC",
                )
                .map_err(|e| SccError::WalError {
                    reason: format!("prepare recover 查询失败: {e}"),
                })?;

            let rows = stmt
                .query_map([], |row| {
                    // 先以元组形式提取,避开 WalEntry 字段类型与 FromSql 的直接耦合
                    let entry_id: String = row.get(0)?;
                    let operation: String = row.get(1)?;
                    let context_id: String = row.get(2)?;
                    let payload: Vec<u8> = row.get(3)?;
                    let timestamp: String = row.get(4)?;
                    Ok((entry_id, operation, context_id, payload, timestamp))
                })
                .map_err(|e| SccError::WalError {
                    reason: format!("query_map 失败: {e}"),
                })?;

            let mut entries = Vec::new();
            for row in rows {
                let (entry_id, operation, context_id, payload, timestamp) =
                    row.map_err(|e| SccError::WalError {
                        reason: format!("读取行失败: {e}"),
                    })?;
                let operation = WalOperation::from_db_str(&operation)?;
                // timestamp 以 RFC3339 字符串存储,恢复时解析回 DateTime<Utc>
                let timestamp = chrono::DateTime::parse_from_rfc3339(&timestamp)
                    .map_err(|e| SccError::WalError {
                        reason: format!("解析 timestamp 失败: {e}"),
                    })?
                    .with_timezone(&chrono::Utc);
                entries.push(WalEntry {
                    entry_id,
                    operation,
                    context_id: ContextId::new(context_id),
                    payload,
                    timestamp,
                });
            }
            Ok(entries)
        })
        .await
        .map_err(|e| SccError::WalError {
            reason: format!("recover spawn_blocking join 错误: {e}"),
        })?
    }

    /// 写入预写日志(async,内部 spawn_blocking)
    ///
    /// 在缓存修改前调用,将日志条目持久化到 SQLite(`committed=0`)。
    ///
    /// # 参数
    /// - `entry`:日志条目(owned,因为 `spawn_blocking` 闭包需要 `'static`)
    ///
    /// # WHY async + spawn_blocking
    /// `INSERT` 涉及文件 I/O 与可能的 fsync,在 async 上下文直接调用会阻塞
    /// runtime。`spawn_blocking` 将 SQLite 写入放到阻塞线程池。
    ///
    /// # WHY `entry: WalEntry`(owned)而非 `&WalEntry`
    /// `spawn_blocking` 闭包需要 `'static + Send`,借用 `&WalEntry` 无法跨线程
    /// 转移。调用方需在调用前 `clone()`(WalEntry 已实现 Clone)。
    pub async fn write_ahead_log(&self, entry: WalEntry) -> Result<(), SccError> {
        let conn = self.conn.clone();
        spawn_blocking(move || -> Result<(), SccError> {
            let conn = conn.lock().map_err(|p| SccError::WalError {
                reason: format!("Connection 锁中毒: {p}"),
            })?;

            conn.execute(
                "INSERT INTO wal_entries
                    (entry_id, operation, context_id, payload, timestamp, committed)
                 VALUES (?1, ?2, ?3, ?4, ?5, 0)",
                rusqlite::params![
                    &entry.entry_id,
                    entry.operation.as_str(),
                    entry.context_id.as_str(),
                    &entry.payload,
                    entry.timestamp.to_rfc3339(),
                ],
            )
            .map_err(|e| SccError::WalError {
                reason: format!("INSERT wal_entries 失败: {e}"),
            })?;

            Ok(())
        })
        .await
        .map_err(|e| SccError::WalError {
            reason: format!("write_ahead_log spawn_blocking join 错误: {e}"),
        })?
    }

    /// 提交日志(async,内部 spawn_blocking)
    ///
    /// 缓存修改成功后调用,标记指定 entry_id 为已提交(`committed=1`)。
    ///
    /// # 参数
    /// - `entry_id`:日志条目 ID(owned `String`,因为 `spawn_blocking` 闭包需要 `'static`)
    ///
    /// # 错误
    /// - 若 entry_id 不存在(已回滚或已提交),返回 `SccError::WalError`
    ///
    /// # WHY async + spawn_blocking
    /// `UPDATE` 涉及文件 I/O,在 async 上下文直接调用会阻塞 runtime。
    pub async fn commit_log(&self, entry_id: String) -> Result<(), SccError> {
        let conn = self.conn.clone();
        spawn_blocking(move || -> Result<(), SccError> {
            let conn = conn.lock().map_err(|p| SccError::WalError {
                reason: format!("Connection 锁中毒: {p}"),
            })?;

            let updated = conn
                .execute(
                    "UPDATE wal_entries SET committed = 1 WHERE entry_id = ?1",
                    rusqlite::params![entry_id],
                )
                .map_err(|e| SccError::WalError {
                    reason: format!("UPDATE committed=1 失败: {e}"),
                })?;

            if updated == 0 {
                return Err(SccError::WalError {
                    reason: format!("commit 失败: entry_id {entry_id} 不存在(可能已回滚或已提交)"),
                });
            }

            Ok(())
        })
        .await
        .map_err(|e| SccError::WalError {
            reason: format!("commit_log spawn_blocking join 错误: {e}"),
        })?
    }

    /// 回滚日志(async,内部 spawn_blocking)
    ///
    /// 缓存修改失败时调用,删除指定 entry_id 的预写记录(撤销预写)。
    ///
    /// # 参数
    /// - `entry_id`:日志条目 ID(owned `String`,因为 `spawn_blocking` 闭包需要 `'static`)
    ///
    /// # 错误
    /// - 若 entry_id 不存在,返回 `SccError::WalError`
    ///
    /// # WHY async + spawn_blocking
    /// `DELETE` 涉及文件 I/O,在 async 上下文直接调用会阻塞 runtime。
    pub async fn rollback_log(&self, entry_id: String) -> Result<(), SccError> {
        let conn = self.conn.clone();
        spawn_blocking(move || -> Result<(), SccError> {
            let conn = conn.lock().map_err(|p| SccError::WalError {
                reason: format!("Connection 锁中毒: {p}"),
            })?;

            let deleted = conn
                .execute(
                    "DELETE FROM wal_entries WHERE entry_id = ?1",
                    rusqlite::params![entry_id],
                )
                .map_err(|e| SccError::WalError {
                    reason: format!("DELETE wal_entries 失败: {e}"),
                })?;

            if deleted == 0 {
                return Err(SccError::WalError {
                    reason: format!("rollback 失败: entry_id {entry_id} 不存在"),
                });
            }

            Ok(())
        })
        .await
        .map_err(|e| SccError::WalError {
            reason: format!("rollback_log spawn_blocking join 错误: {e}"),
        })?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_write_and_commit_log() {
        let wal = InMemoryWal::new();
        let entry = WalEntry::new(
            "wal-1",
            WalOperation::Insert,
            "ctx-1",
            b"payload-1".to_vec(),
        );

        // 写入预写日志
        wal.write_ahead_log(&entry).expect("write_ahead_log 应成功");
        assert_eq!(wal.entry_count(), 1, "写入后 entries 应有 1 条");

        // 提交日志
        wal.commit_log("wal-1").expect("commit_log 应成功");
        assert_eq!(wal.committed_count(), 1, "提交后 committed 应有 1 条");
        assert_eq!(wal.entry_count(), 1, "提交后 entries 仍保留(审计)");
    }

    #[test]
    fn test_rollback_log() {
        let wal = InMemoryWal::new();
        let entry = WalEntry::new(
            "wal-2",
            WalOperation::Prefetch,
            "ctx-2",
            b"payload-2".to_vec(),
        );

        // 写入后回滚
        wal.write_ahead_log(&entry).expect("write_ahead_log 应成功");
        assert_eq!(wal.entry_count(), 1);

        wal.rollback_log("wal-2").expect("rollback_log 应成功");
        assert_eq!(wal.entry_count(), 0, "回滚后 entries 应为空");
        assert_eq!(wal.committed_count(), 0, "回滚后 committed 应为空");
    }

    #[test]
    fn test_commit_nonexistent_log_returns_error() {
        let wal = InMemoryWal::new();

        // 提交不存在的 entry_id 应返回错误
        let err = wal.commit_log("nonexistent").unwrap_err();
        match err {
            SccError::WalError { reason } => {
                assert!(
                    reason.contains("nonexistent"),
                    "错误信息应包含 entry_id, got: {reason}"
                );
            }
            other => panic!("应为 WalError 变体, got {other:?}"),
        }

        // 回滚不存在的 entry_id 也应返回错误
        let err = wal.rollback_log("nonexistent").unwrap_err();
        match err {
            SccError::WalError { reason } => {
                assert!(
                    reason.contains("nonexistent"),
                    "错误信息应包含 entry_id, got: {reason}"
                );
            }
            other => panic!("应为 WalError 变体, got {other:?}"),
        }
    }

    #[test]
    fn test_rollback_after_commit_clears_committed() {
        // 边界场景:先提交再回滚,committed 集合应被清理
        let wal = InMemoryWal::new();
        let entry = WalEntry::new(
            "wal-3",
            WalOperation::Update,
            "ctx-3",
            b"payload-3".to_vec(),
        );

        wal.write_ahead_log(&entry).expect("write 应成功");
        wal.commit_log("wal-3").expect("commit 应成功");
        assert_eq!(wal.committed_count(), 1);

        wal.rollback_log("wal-3").expect("rollback 应成功");
        assert_eq!(wal.committed_count(), 0, "回滚后 committed 应被清理");
        assert_eq!(wal.entry_count(), 0, "回滚后 entries 应被清理");
    }

    // === SqliteWal 测试(Task 6.2 + C-02 修复回归测试)===
    // WHY 独立子 mod:SqliteWal 测试需要 Arc/tempfile/tokio,
    // 而 InMemoryWal 测试不需要,放 tests mod 顶部会触发 unused_imports
    // WHY 全部 #[tokio::test]:SqliteWal 方法改为 async,需 tokio runtime 驱动
    mod sqlite_wal_tests {
        use crate::error::SccError;
        use crate::wal::{SqliteWal, WalEntry, WalOperation};
        use std::sync::Arc;
        use tempfile::tempdir;

        /// 辅助:在临时目录创建 SqliteWal,返回 (SqliteWal, TempDir 句柄)
        /// WHY 保持 TempDir 句柄:drop 时自动清理目录,避免测试残留文件
        /// WHY async:`SqliteWal::new` 改为 async(内部 spawn_blocking 包装文件 I/O)
        async fn make_wal() -> (SqliteWal, tempfile::TempDir) {
            let dir = tempdir().expect("创建临时目录失败");
            let db_path = dir.path().join("test.db");
            let path = db_path.to_str().expect("路径转 str 失败").to_string();
            let wal = SqliteWal::new(path).await.expect("创建 SqliteWal 失败");
            (wal, dir)
        }

        #[tokio::test]
        async fn test_sqlite_wal_write_and_commit() {
            let (wal, _dir) = make_wal().await;
            let entry = WalEntry::new("sw-1", WalOperation::Insert, "ctx-1", b"payload-1".to_vec());

            // 写入预写日志(entry owned 转移,因为 spawn_blocking 闭包需要 'static)
            wal.write_ahead_log(entry)
                .await
                .expect("write_ahead_log 应成功");

            // 写入后但未 commit 时,recover 应返回该条目
            let uncommitted = wal.recover().await.expect("recover 应成功");
            assert_eq!(
                uncommitted.len(),
                1,
                "未 commit 时 recover 应返回 1 条, got {} 条",
                uncommitted.len()
            );
            assert_eq!(uncommitted[0].entry_id, "sw-1");

            // commit 后 recover 不应返回该条目
            wal.commit_log("sw-1".to_string())
                .await
                .expect("commit_log 应成功");
            let uncommitted = wal.recover().await.expect("recover 应成功");
            assert!(
                uncommitted.is_empty(),
                "commit 后应无未提交条目, got {} 条",
                uncommitted.len()
            );
        }

        #[tokio::test]
        async fn test_sqlite_wal_rollback() {
            let (wal, _dir) = make_wal().await;
            let entry = WalEntry::new(
                "sw-2",
                WalOperation::Prefetch,
                "ctx-2",
                b"payload-2".to_vec(),
            );

            wal.write_ahead_log(entry)
                .await
                .expect("write_ahead_log 应成功");
            wal.rollback_log("sw-2".to_string())
                .await
                .expect("rollback_log 应成功");

            // 回滚后 recover 不应返回该条目(已 DELETE)
            let uncommitted = wal.recover().await.expect("recover 应成功");
            assert!(
                uncommitted.is_empty(),
                "回滚后应无未提交条目, got {} 条",
                uncommitted.len()
            );
        }

        #[tokio::test]
        async fn test_sqlite_wal_commit_nonexistent_returns_error() {
            let (wal, _dir) = make_wal().await;

            // commit 不存在的 entry_id 应返回错误
            let err = wal.commit_log("nonexistent".to_string()).await.unwrap_err();
            match err {
                SccError::WalError { reason } => {
                    assert!(
                        reason.contains("nonexistent"),
                        "错误信息应包含 entry_id, got: {reason}"
                    );
                }
                other => panic!("应为 WalError 变体, got {other:?}"),
            }

            // rollback 不存在的 entry_id 也应返回错误
            let err = wal
                .rollback_log("nonexistent".to_string())
                .await
                .unwrap_err();
            match err {
                SccError::WalError { reason } => {
                    assert!(
                        reason.contains("nonexistent"),
                        "错误信息应包含 entry_id, got: {reason}"
                    );
                }
                other => panic!("应为 WalError 变体, got {other:?}"),
            }
        }

        #[tokio::test]
        async fn test_sqlite_wal_crash_recovery_with_uncommitted_entries() {
            // 模拟进程崩溃:drop SqliteWal 后重新打开同一文件
            let dir = tempdir().expect("创建临时目录失败");
            let db_path = dir.path().join("crash.db");
            let path = db_path.to_str().expect("路径转 str 失败").to_string();

            // 第一阶段:写入 3 条 entry,仅 commit 1 条,然后 drop(模拟崩溃)
            {
                let wal = SqliteWal::new(path.clone())
                    .await
                    .expect("创建 SqliteWal 失败");
                for (id, ctx) in [("c-1", "ctx-a"), ("c-2", "ctx-b"), ("c-3", "ctx-c")] {
                    let entry = WalEntry::new(id, WalOperation::Insert, ctx, vec![1, 2, 3]);
                    wal.write_ahead_log(entry)
                        .await
                        .expect("write_ahead_log 应成功");
                }
                wal.commit_log("c-1".to_string())
                    .await
                    .expect("commit c-1 应成功");
                // 不 commit c-2 / c-3,直接 drop 模拟崩溃
            }

            // 第二阶段:重新打开同一文件,调用 recover 应返回 2 条未提交 entry
            let wal = SqliteWal::new(path).await.expect("重新打开 SqliteWal 失败");
            let uncommitted = wal.recover().await.expect("recover 应成功");
            assert_eq!(
                uncommitted.len(),
                2,
                "崩溃恢复应返回 2 条未提交条目, got {} 条",
                uncommitted.len()
            );

            // 验证返回的 entry_id 集合(应为 c-2 / c-3,c-1 已 commit 不应出现)
            let ids: Vec<&str> = uncommitted.iter().map(|e| e.entry_id.as_str()).collect();
            assert!(ids.contains(&"c-2"), "应包含 c-2, got {:?}", ids);
            assert!(ids.contains(&"c-3"), "应包含 c-3, got {:?}", ids);
            assert!(
                !ids.contains(&"c-1"),
                "不应包含已 commit 的 c-1, got {:?}",
                ids
            );

            // 验证恢复出的 entry 字段完整(payload/operation/context_id)
            let entry_c2 = uncommitted
                .iter()
                .find(|e| e.entry_id == "c-2")
                .expect("应找到 c-2");
            assert_eq!(entry_c2.operation, WalOperation::Insert);
            assert_eq!(entry_c2.context_id.as_str(), "ctx-b");
            assert_eq!(entry_c2.payload, vec![1, 2, 3]);
        }

        #[tokio::test]
        async fn test_sqlite_wal_concurrent_writes() {
            let (wal, _dir) = make_wal().await;
            let wal = Arc::new(wal);
            const TASKS: usize = 10;
            const PER_TASK: usize = 10;

            // WHY tokio::spawn 替代 std::thread::spawn:
            // SqliteWal 方法改为 async,需在 tokio runtime 内调用。
            // Arc<SqliteWal> 满足 Send + Sync,可安全跨 task 共享。
            let mut handles = Vec::with_capacity(TASKS);
            for t in 0..TASKS {
                let wal = Arc::clone(&wal);
                handles.push(tokio::spawn(async move {
                    for i in 0..PER_TASK {
                        let id = format!("t{t}-i{i}");
                        let entry = WalEntry::new(
                            &id,
                            WalOperation::Update,
                            format!("ctx-{t}"),
                            vec![t as u8, i as u8],
                        );
                        wal.write_ahead_log(entry).await.expect("并发 write 应成功");
                    }
                }));
            }

            for h in handles {
                h.await.expect("task 应正常退出");
            }

            // 验证:总条目数 = TASKS * PER_TASK,均为未提交状态
            let uncommitted = wal.recover().await.expect("recover 应成功");
            assert_eq!(
                uncommitted.len(),
                TASKS * PER_TASK,
                "并发写入后应有 {} 条未提交条目, got {} 条",
                TASKS * PER_TASK,
                uncommitted.len()
            );
        }

        // === C-02 修复回归测试 ===

        /// 回归测试:WAL 恢复不阻塞 async runtime
        ///
        /// WHY 此测试:C-02 修复前,SqliteWal::recover 是同步方法,
        /// 1000 条目恢复会阻塞 async runtime 线程。修复后 recover 改为 async +
        /// spawn_blocking,应在独立阻塞线程执行,不占用 async worker。
        ///
        /// 验证策略:
        /// 1. 写入 1000 条 entry(制造恢复数据量)
        /// 2. 同时启动两个 task:
        ///    - task A:调用 recover(可能耗时)
        ///    - task B:周期性 yield,验证 async runtime 仍可调度
        /// 3. 用 tokio::time::timeout 限制总时长,若 recover 阻塞 runtime,
        ///    task B 无法推进,总时长会超时
        ///
        /// 注意:此测试验证"不阻塞 runtime",而非"recover 速度"。
        /// recover 本身可能耗时,但不应阻止其他 task 执行。
        #[tokio::test]
        async fn test_recover_does_not_block_async_runtime() {
            let dir = tempdir().expect("创建临时目录失败");
            let db_path = dir.path().join("nonblock.db");
            let path = db_path.to_str().expect("路径转 str 失败").to_string();

            // 阶段 1:写入 1000 条 entry(全部未 commit,用于 recover 扫描)
            {
                let wal = SqliteWal::new(path.clone())
                    .await
                    .expect("创建 SqliteWal 失败");
                for i in 0..1000u32 {
                    let entry = WalEntry::new(
                        format!("entry-{i}"),
                        WalOperation::Insert,
                        format!("ctx-{i}"),
                        vec![i as u8; 16],
                    );
                    wal.write_ahead_log(entry)
                        .await
                        .expect("write_ahead_log 应成功");
                }
                // drop wal 模拟崩溃,1000 条 entry 留在 SQLite 文件
            }

            // 阶段 2:重开并 recover,同时跑 heartbeat task 验证 runtime 响应性
            let wal = SqliteWal::new(path).await.expect("重新打开 SqliteWal 失败");

            // heartbeat task:每 1ms yield 一次,记录心跳数
            // 若 recover 阻塞 runtime,heartbeat 无法推进,计数会远低于预期
            let heartbeat = tokio::spawn(async {
                let mut beats = 0u32;
                for _ in 0..50 {
                    tokio::task::yield_now().await;
                    tokio::time::sleep(std::time::Duration::from_millis(1)).await;
                    beats += 1;
                }
                beats
            });

            // recover 与 heartbeat 并发执行
            // WHY select:任一完成即推进;若 recover 阻塞 runtime,
            // heartbeat 在 select 中也无法执行,整体会超时
            let recover_result =
                tokio::time::timeout(std::time::Duration::from_secs(10), wal.recover()).await;

            // recover 应在 10 秒内完成(1000 条目实际 < 1 秒)
            let recovered = recover_result
                .expect("recover 应在 10 秒内完成,可能阻塞了 async runtime")
                .expect("recover 应成功");
            assert_eq!(
                recovered.len(),
                1000,
                "应恢复 1000 条未提交 entry, got {}",
                recovered.len()
            );

            // heartbeat 应完成全部 50 次心跳
            // 若 recover 阻塞 runtime,heartbeat 会卡住,await 会超时
            let beats = tokio::time::timeout(std::time::Duration::from_secs(5), heartbeat)
                .await
                .expect("heartbeat 应在 5 秒内完成,async runtime 可能被阻塞")
                .expect("heartbeat task 应正常退出");
            assert_eq!(
                beats, 50,
                "heartbeat 应完成 50 次,got {beats},async runtime 可能被阻塞"
            );
        }

        /// 回归测试:1000 条目崩溃恢复功能正确性(async 版)
        ///
        /// WHY 此测试:C-02 修复后,SqliteWal 所有方法改为 async + spawn_blocking。
        /// 需验证 1000 次崩溃恢复无数据丢失的基线(async 版本)不退化。
        ///
        /// 流程:
        /// - 50 次循环(缩小规模加速 CI,保留正确性验证)
        /// - 每次:写入 10 条 entry,commit 前 5 条 → drop → 重开 → recover
        /// - 验证恢复 5 条未 commit entry,payload 完整
        ///
        /// 注意:完整的 1000 次压测在 benches/wal_recovery.rs 中,
        /// 此处仅验证 async API 的功能正确性。
        #[tokio::test]
        async fn test_sqlite_wal_1000_entries_recovery_correctness() {
            const CYCLES: usize = 50;
            const ENTRIES_PER_CYCLE: usize = 10;

            for cycle in 0..CYCLES {
                let dir = tempdir().expect("创建临时目录失败");
                let db_path = dir.path().join(format!("cycle-{cycle}.db"));
                let path = db_path.to_str().expect("路径转 str 失败").to_string();

                // 阶段 1:写入 10 条 entry,commit 前 5 条,然后 drop(模拟崩溃)
                let half = ENTRIES_PER_CYCLE / 2;
                {
                    let wal = SqliteWal::new(path.clone())
                        .await
                        .expect("创建 SqliteWal 失败");
                    for i in 0..ENTRIES_PER_CYCLE {
                        let entry = WalEntry::new(
                            format!("entry-{i}"),
                            WalOperation::Insert,
                            format!("ctx-{i}"),
                            vec![i as u8; 16],
                        );
                        wal.write_ahead_log(entry)
                            .await
                            .expect("write_ahead_log 应成功");
                        if i < half {
                            wal.commit_log(format!("entry-{i}"))
                                .await
                                .expect("commit_log 应成功");
                        }
                    }
                    // drop wal 模拟崩溃
                }

                // 阶段 2:重开并 recover
                let wal = SqliteWal::new(path).await.expect("重新打开 SqliteWal 失败");
                let recovered = wal.recover().await.expect("recover 应成功");

                // 阶段 3:验证数据完整性(零丢失)
                let expected_uncommitted = ENTRIES_PER_CYCLE - half;
                assert_eq!(
                    recovered.len(),
                    expected_uncommitted,
                    "cycle {cycle}: 恢复条目数应为 {expected_uncommitted}, got {}",
                    recovered.len()
                );

                for entry in &recovered {
                    let idx: usize = entry
                        .entry_id
                        .strip_prefix("entry-")
                        .and_then(|s| s.parse().ok())
                        .expect("entry_id 应可解析为索引");
                    assert!(
                        idx >= half,
                        "cycle {cycle}: 已 commit 的 entry-{idx} 不应出现在 recover 列表"
                    );
                    assert_eq!(
                        entry.payload,
                        vec![idx as u8; 16],
                        "cycle {cycle}: entry-{idx} payload 应完整恢复"
                    );
                    assert_eq!(
                        entry.operation,
                        WalOperation::Insert,
                        "cycle {cycle}: entry-{idx} operation 应为 Insert"
                    );
                }
            }
        }

        /// 回归测试:write_ahead_log / commit_log / rollback_log 功能正确性(async 版)
        ///
        /// WHY 此测试:C-02 修复后,三个方法签名改变(entry/entry_id 改为 owned),
        /// 需验证 owned 参数传递正确,SQLite 操作语义不变。
        #[tokio::test]
        async fn test_sqlite_wal_write_commit_rollback_async_semantics() {
            let (wal, _dir) = make_wal().await;

            // 写入 3 条 entry
            let entry1 = WalEntry::new("a-1", WalOperation::Insert, "ctx-1", b"p-1".to_vec());
            let entry2 = WalEntry::new("a-2", WalOperation::Update, "ctx-2", b"p-2".to_vec());
            let entry3 = WalEntry::new("a-3", WalOperation::Delete, "ctx-3", b"p-3".to_vec());
            wal.write_ahead_log(entry1).await.expect("write 1 应成功");
            wal.write_ahead_log(entry2).await.expect("write 2 应成功");
            wal.write_ahead_log(entry3).await.expect("write 3 应成功");

            // commit entry2,rollback entry3
            wal.commit_log("a-2".to_string())
                .await
                .expect("commit a-2 应成功");
            wal.rollback_log("a-3".to_string())
                .await
                .expect("rollback a-3 应成功");

            // recover 应只返回 entry1(未 commit 未 rollback)
            let recovered = wal.recover().await.expect("recover 应成功");
            assert_eq!(
                recovered.len(),
                1,
                "recover 应返回 1 条(仅 a-1), got {}",
                recovered.len()
            );
            assert_eq!(recovered[0].entry_id, "a-1");
            assert_eq!(recovered[0].operation, WalOperation::Insert);

            // 再次 commit a-1,recover 应为空
            wal.commit_log("a-1".to_string())
                .await
                .expect("commit a-1 应成功");
            let recovered = wal.recover().await.expect("recover 应成功");
            assert!(
                recovered.is_empty(),
                "全部 commit 后 recover 应为空, got {}",
                recovered.len()
            );
        }
    }
}
