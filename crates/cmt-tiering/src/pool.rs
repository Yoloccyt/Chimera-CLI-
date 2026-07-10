//! SQLite 连接池 — 读写分离的多连接管理器
//!
//! 对应架构层:L3 Storage(辅助模块)
//!
//! # 设计决策(WHY)
//! - **读写分离**:SQLite WAL 模式支持并发读 + 单写,但原始 `Arc<Mutex<Connection>>`
//!   将所有操作(包括读)序列化到单个 Mutex。连接池通过:
//!   1. 多个只读连接(`read_conns`)实现真正的并发读
//!   2. 独立写连接(`write_conn`)序列化写操作
//!   3. WAL 模式保证读写互不阻塞
//! - **零新依赖**:不使用 r2d2/r2d2_sqlite,手动管理连接生命周期,
//!   避免引入外部依赖(项目约定:CMT 层不引入未被任务要求的依赖)
//! - **哈希分片**:读操作按输入 hint(通常为 cap_id)哈希取模选择连接,
//!   同一 ID 总是路由到同一分片,最大化 SQLite 页面缓存命中率
//! - **spawn_blocking 包装**:所有 SQLite 操作通过 `tokio::task::spawn_blocking`
//!   在阻塞线程池执行,不阻塞异步运行时(架构红线)
//!
//! # 预期效果
//! 读多写少的能力存储场景下,并发读吞吐量提升 N 倍(N = read_pool_size)。
//! 写操作仍为串行,但 WAL 模式下读写互不阻塞。
//!
//! # 连接池大小选择
//! - `read_pool_size = 0`:纯写模式或单连接模式(测试用,`:memory:` 数据库必须使用)
//! - `read_pool_size = 2`(默认):适合 CLI 工具,并发读有限但足够
//! - `read_pool_size = 4`:适合服务端场景,高并发读

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use tokio::task::spawn_blocking;

use crate::error::CmtError;

/// SQLite 连接池 — 读写分离,支持并发读与串行写
///
/// # 线程安全
/// `Arc<SqlitePool>` 包装后可跨任务共享。每个连接分片由独立 `Mutex` 保护,
/// 读分片之间互不干扰,写连接独立序列化。所有 async fn 满足 `Send + 'static`。
pub struct SqlitePool {
    /// 只读连接分片(每个分片一个独立 Mutex,支持并发读)
    ///
    /// WHY 多分片:单 Mutex 序列化所有读,多分片允许 N 个读操作并行执行。
    /// 分片数 = `read_pool_size`,通过哈希路由保证同一 ID 总是命中同一分片。
    read_conns: Vec<Arc<Mutex<Connection>>>,

    /// 写连接(独立序列化所有写操作)
    ///
    /// WHY 单写:SQLite WAL 模式只允许一个写者,多写者会 SQLITE_BUSY。
    /// 独立写连接确保写操作互斥,不与读连接争抢锁。
    write_conn: Arc<Mutex<Connection>>,

    /// 轮询计数器(用于无 hint 时均匀分配读连接到各分片)
    read_counter: AtomicUsize,
}

impl SqlitePool {
    /// 创建连接池(1 写 + N 读)
    ///
    /// `conn_factory` 负责创建并初始化每个连接(设置 WAL、PRAGMA、建表等)。
    /// 所有连接(包括写连接)都会调用 factory,确保一致的初始化状态。
    ///
    /// # Errors
    /// - factory 返回 `rusqlite::Error`(连接打开失败、PRAGMA 设置失败等)
    pub fn open(
        read_pool_size: usize,
        mut conn_factory: impl FnMut() -> rusqlite::Result<Connection>,
    ) -> rusqlite::Result<Self> {
        let write_conn = Arc::new(Mutex::new(conn_factory()?));

        let mut read_conns = Vec::with_capacity(read_pool_size);
        for _ in 0..read_pool_size {
            read_conns.push(Arc::new(Mutex::new(conn_factory()?)));
        }

        Ok(Self {
            read_conns,
            write_conn,
            read_counter: AtomicUsize::new(0),
        })
    }

    /// 获取只读连接(按 hint 哈希路由到固定分片)
    ///
    /// WHY 哈希路由:同一 cap_id 总是路由到同一连接分片,
    /// 最大化 SQLite 页面缓存命中率(连续查询同一 ID 不会在不同连接间切换)。
    /// 若无读连接(如测试模式 `read_pool_size = 0`),回退到写连接。
    #[inline]
    fn read_conn(&self, hint: &str) -> &Arc<Mutex<Connection>> {
        if self.read_conns.is_empty() {
            return &self.write_conn;
        }
        // 多项式滚动哈希(Java-style):轻量、分布均匀,无需引入 hash 库
        let hash = hint
            .bytes()
            .fold(0u64, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u64));
        let idx = (hash as usize) % self.read_conns.len();
        &self.read_conns[idx]
    }

    /// 获取只读连接(轮询分配,适用于无特定 ID 的批量查询)
    ///
    /// WHY 轮询:list_all、count 等操作无特定 ID,
    /// 轮询确保负载均匀分布到各读分片。
    #[inline]
    fn next_read_conn(&self) -> &Arc<Mutex<Connection>> {
        if self.read_conns.is_empty() {
            return &self.write_conn;
        }
        let idx = self.read_counter.fetch_add(1, Ordering::Relaxed) % self.read_conns.len();
        &self.read_conns[idx]
    }

    /// 同步执行只读操作(按 hint 路由)
    ///
    /// # Errors
    /// - Mutex poisoned(线程 panic 导致)
    /// - `f` 内部返回 `CmtError`
    pub fn with_read<R>(
        &self,
        hint: &str,
        f: impl FnOnce(&Connection) -> Result<R, CmtError>,
    ) -> Result<R, CmtError> {
        let conn = self
            .read_conn(hint)
            .lock()
            .map_err(|e| CmtError::StorageError(format!("SQLite read conn mutex poisoned: {e}")))?;
        f(&conn)
    }

    /// 同步执行写操作
    ///
    /// # Errors
    /// - Mutex poisoned(线程 panic 导致)
    /// - `f` 内部返回 `CmtError`
    pub fn with_write<R>(
        &self,
        f: impl FnOnce(&Connection) -> Result<R, CmtError>,
    ) -> Result<R, CmtError> {
        let conn = self.write_conn.lock().map_err(|e| {
            CmtError::StorageError(format!("SQLite write conn mutex poisoned: {e}"))
        })?;
        f(&conn)
    }

    /// 异步只读操作(spawn_blocking 包装,不阻塞异步运行时)
    pub async fn with_read_async<R, F>(self: &Arc<Self>, hint: String, f: F) -> Result<R, CmtError>
    where
        R: Send + 'static,
        F: FnOnce(&Connection) -> Result<R, CmtError> + Send + 'static,
    {
        let pool = Arc::clone(self);
        spawn_blocking(move || pool.with_read(&hint, f))
            .await
            .map_err(|e| CmtError::StorageError(format!("spawn_blocking join 错误: {e}")))?
    }

    /// 异步写操作(spawn_blocking 包装,不阻塞异步运行时)
    pub async fn with_write_async<R, F>(self: &Arc<Self>, f: F) -> Result<R, CmtError>
    where
        R: Send + 'static,
        F: FnOnce(&Connection) -> Result<R, CmtError> + Send + 'static,
    {
        let pool = Arc::clone(self);
        spawn_blocking(move || pool.with_write(f))
            .await
            .map_err(|e| CmtError::StorageError(format!("spawn_blocking join 错误: {e}")))?
    }

    /// 异步只读操作(轮询分配,适用于无特定 ID 的批量查询)
    pub async fn with_any_read_async<R, F>(self: &Arc<Self>, f: F) -> Result<R, CmtError>
    where
        R: Send + 'static,
        F: FnOnce(&Connection) -> Result<R, CmtError> + Send + 'static,
    {
        let pool = Arc::clone(self);
        spawn_blocking(move || {
            let conn = pool
                .next_read_conn()
                .lock()
                .map_err(|e| CmtError::StorageError(format!("read conn mutex poisoned: {e}")))?;
            f(&conn)
        })
        .await
        .map_err(|e| CmtError::StorageError(format!("spawn_blocking join 错误: {e}")))?
    }

    /// 返回读连接分片数(用于监控与调试)
    pub fn read_pool_size(&self) -> usize {
        self.read_conns.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 创建纯内存测试连接池(read_pool_size 个 :memory: 读连接 + 1 个 :memory: 写连接)
    ///
    /// 注意::memory: 连接彼此独立(不共享数据),测试中需注意:
    /// - 写入 write_conn 的数据在 read_conns 中不可见
    /// - 单连接模式(read_pool_size=0)下读写共用 write_conn,数据可见
    fn make_test_pool(read_pool_size: usize) -> SqlitePool {
        SqlitePool::open(read_pool_size, || {
            let conn = Connection::open_in_memory()?;
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS test_kv (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
            )?;
            Ok(conn)
        })
        .unwrap()
    }

    #[test]
    fn test_pool_creation_default() {
        let pool = make_test_pool(2);
        assert_eq!(pool.read_pool_size(), 2);
    }

    #[test]
    fn test_pool_creation_zero_read_conns() {
        let pool = make_test_pool(0);
        assert_eq!(pool.read_pool_size(), 0);
    }

    #[test]
    fn test_write_and_read_single_conn() {
        // read_pool_size=0 时,读写共用 write_conn,数据可见
        let pool = make_test_pool(0);

        pool.with_write(|conn| {
            conn.execute(
                "INSERT INTO test_kv (key, value) VALUES (?1, ?2);",
                rusqlite::params!["k1", "v1"],
            )?;
            Ok(())
        })
        .unwrap();

        let value: String = pool
            .with_read("k1", |conn| {
                let v: String = conn.query_row(
                    "SELECT value FROM test_kv WHERE key = ?1;",
                    rusqlite::params!["k1"],
                    |row| row.get(0),
                )?;
                Ok(v)
            })
            .unwrap();

        assert_eq!(value, "v1");
    }

    #[test]
    fn test_hash_routing_consistency() {
        // 同一 hint 总是路由到同一分片
        let pool = make_test_pool(4);
        let conn1 = pool.read_conn("cap-1") as *const _;
        let conn2 = pool.read_conn("cap-1") as *const _;
        assert_eq!(conn1, conn2, "同一 hint 应路由到同一分片");

        let conn3 = pool.read_conn("cap-2") as *const _;
        // 不同 hint 可能路由到不同分片(概率性,不强制断言)
        // 但验证不会 panic
        let _ = conn3;
    }

    #[test]
    fn test_empty_read_conns_fallback_to_write() {
        let pool = make_test_pool(0);
        let read_ptr = pool.read_conn("any") as *const _;
        let write_ptr = &pool.write_conn as *const _;
        assert_eq!(
            read_ptr, write_ptr,
            "无读连接时 read_conn 应回退到 write_conn"
        );
    }

    #[tokio::test]
    async fn test_async_write_and_read() {
        let pool = Arc::new(make_test_pool(0));

        pool.with_write_async(|conn| {
            conn.execute(
                "INSERT INTO test_kv (key, value) VALUES (?1, ?2);",
                rusqlite::params!["async-k1", "async-v1"],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let value: String = pool
            .with_read_async("async-k1".to_string(), |conn| {
                let v: String = conn.query_row(
                    "SELECT value FROM test_kv WHERE key = ?1;",
                    rusqlite::params!["async-k1"],
                    |row| row.get(0),
                )?;
                Ok(v)
            })
            .await
            .unwrap();

        assert_eq!(value, "async-v1");
    }

    #[tokio::test]
    async fn test_concurrent_reads_single_conn() {
        // 单连接模式下,验证并发读不会 panic(序列化执行)
        let pool = Arc::new(make_test_pool(0));

        pool.with_write_async(|conn| {
            for i in 0..5 {
                conn.execute(
                    "INSERT INTO test_kv (key, value) VALUES (?1, ?2);",
                    rusqlite::params![format!("k{i}"), format!("v{i}")],
                )?;
            }
            Ok(())
        })
        .await
        .unwrap();

        let mut handles = Vec::new();
        for i in 0..5 {
            let pool = Arc::clone(&pool);
            handles.push(tokio::spawn(async move {
                pool.with_read_async(format!("k{i}"), move |conn| {
                    let v: String = conn.query_row(
                        "SELECT value FROM test_kv WHERE key = ?1;",
                        rusqlite::params![format!("k{i}")],
                        |row| row.get(0),
                    )?;
                    Ok(v)
                })
                .await
            }));
        }

        for (i, handle) in handles.into_iter().enumerate() {
            let value = handle.await.unwrap().unwrap();
            assert_eq!(value, format!("v{i}"));
        }
    }

    #[tokio::test]
    async fn test_with_any_read_async() {
        let pool = Arc::new(make_test_pool(0));

        pool.with_write_async(|conn| {
            conn.execute(
                "INSERT INTO test_kv (key, value) VALUES (?1, ?2);",
                rusqlite::params!["any-k", "any-v"],
            )?;
            Ok(())
        })
        .await
        .unwrap();

        let count: u64 = pool
            .with_any_read_async(|conn| {
                let c: i64 =
                    conn.query_row("SELECT COUNT(*) FROM test_kv;", [], |row| row.get(0))?;
                Ok(u64::try_from(c).unwrap_or(0))
            })
            .await
            .unwrap();

        assert_eq!(count, 1);
    }
}
