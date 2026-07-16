//! 指标历史 SQLite 持久化 — `MetricsHistory`
//!
//! 对应 spec:enterprise-tui-monitoring-task-viz §二·系统监控增强 / Task 2.3
//! 对应创新点:资源监控趋势图的历史持久化层(Ω-Sparse 持久化兜底)
//!
//! # 设计目标
//! 1. **跨重启保留**:`ResourceMonitorPanel` 趋势图需要历史 5-7 天的 CPU/内存/网络数据,
//!    内存滑动窗口只能保存 5 分钟(~300 采样点);SQLite 持久化让趋势图在 TUI 重启后
//!    能回填历史曲线,符合"运维诊断需要一周回溯"的产品诉求。
//! 2. **复合主键幂等**:`(unix_ts, metric)` 复合主键 + `ON CONFLICT REPLACE` 让
//!    重试 / 重连产生的重复时间戳只保留最后值(避免历史曲线出现阶梯)。
//! 3. **保留期可配**:`TuiConfig.metrics_history_retention_days` 控制保留期,
//!    `cleanup()` 后台任务定期删除过期行,避免 DB 无限膨胀。
//!
//! # 关键约束(来自全局规则 §4.4 #2 + §6.2 实战红线)
//! - **所有 rusqlite 调用必须 `tokio::task::spawn_blocking`**:
//!   rusqlite 是同步阻塞 I/O,直接在 async 上下文中调用会阻塞 Tokio
//!   runtime 的工作线程。`spawn_blocking` 将阻塞操作转移到专用阻塞线程池。
//!   同样的模式在 repo-wiki / scc-cache 79 处已使用,本模块沿用之。
//! - **`Arc<Mutex<Connection>>` + 闭包内加锁**:
//!   `std::sync::MutexGuard` **不能** 跨 `spawn_blocking` 边界(需要 `Send`),
//!   正确模式是把 `Arc<Mutex<Connection>>` clone 进闭包,**在闭包内部**重新
//!   `lock().unwrap_or_else()` 拿 guard,guard 仅在闭包内使用,自动随闭包结束 drop。
//! - **复合主键顺序**:`PRIMARY KEY(unix_ts, metric)` 把时间戳放第一列,
//!   `query_range(start, end)` 可命中主键前缀索引 → O(log n) 范围扫描,
//!   避免全表扫描(7 天 × 86_400s × 1Hz ≈ 604_800 行的设计上限)。
//!
//! # 错误传播
//! 库层用 `TuiError::SqliteError` 透传 rusqlite 错误(避免 `anyhow` 入侵库层,
//!   §4.1 库层错误用 `thiserror` 自定义 enum)。
//!
//! # Schema
//! ```sql
//! CREATE TABLE IF NOT EXISTS samples (
//!     unix_ts INTEGER NOT NULL,    -- 毫秒级 Unix 时间戳
//!     metric  TEXT    NOT NULL,    -- 指标名(如 "cpu_usage" / "mem_usage")
//!     value   REAL    NOT NULL,    -- 指标值(原始浮点,精度由调用方决定)
//!     PRIMARY KEY(unix_ts, metric)
//! );
//! ```
//! WHY 用 `samples` 表名:与"指标历史"语义匹配;表名简短利于 SQLite 元数据查找。

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection};
use tokio::task::spawn_blocking;

use crate::data::resource_history::MetricSample;
use crate::error::TuiError;

/// 一天对应的毫秒数 — 内部用于保留期换算
///
/// WHY 常量:`retention_days` 计算 `now - retention_days * MS_PER_DAY`,
/// 常量避免在多处重复魔数(24 × 60 × 60 × 1000 = 86_400_000)。
const MS_PER_DAY: u64 = 86_400_000;

/// 指标历史持久化器 — SQLite 后端 + spawn_blocking 包装
///
/// # 线程安全
/// - 内部 `Arc<Mutex<Connection>>` 允许把共享句柄 move 进 `spawn_blocking` 闭包,
///   在闭包内重新加锁(详见模块级文档)。
/// - `Send + Sync` 满足跨任务共享(可 `Arc<MetricsHistory>` 跨 tick 持有)
/// - `Mutex::lock()` 在 panic 时 poison,所有方法用 `unwrap_or_else` 恢复
///
/// # 性能特性
/// - 写:`INSERT ... ON CONFLICT REPLACE`,复合主键 O(log n) 定位
/// - 读:`query_range(metric, start, end)` 走主键前缀索引,O(log n + k) 范围扫描
/// - 清理:`cleanup()` 单 SQL `DELETE WHERE unix_ts < ?` 一次扫描完成
pub struct MetricsHistory {
    /// SQLite 连接(单连接 + Mutex 串行化,与 model-router 持久化层模式一致)
    ///
    /// WHY 单连接而非连接池:本模块是"低频写、偶发读"场景(每秒 1 写),
    /// 单连接简化事务语义;后续若 TUI 引入多面板并发读历史,
    /// 可参照 repo-wiki 升级为 read_pool_size + spawn_blocking 模式。
    ///
    /// WHY 用 `Arc<Mutex<...>>` 而非 `Mutex<...>`:
    /// `spawn_blocking` 要求闭包 `Send`,`MutexGuard` 不实现 `Send`,
    /// 所以连接必须用 `Arc` 共享所有权,guard 在闭包内获取并随闭包结束释放。
    conn: Arc<Mutex<Connection>>,
    /// 数据库文件路径(供调试与诊断日志使用)
    db_path: PathBuf,
}

impl std::fmt::Debug for MetricsHistory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MetricsHistory")
            .field("db_path", &self.db_path)
            .finish_non_exhaustive()
    }
}

impl MetricsHistory {
    /// 打开/创建指标历史 SQLite 数据库
    ///
    /// # 行为
    /// 1. 若 `db_path` 不存在,自动创建空文件
    /// 2. 启用 WAL 模式(并发读写友好,崩溃恢复友好)
    /// 3. 创建 `samples` 表(若不存在)
    /// 4. 所有上述操作在 `spawn_blocking` 中执行,避免阻塞 async runtime
    ///
    /// # 幂等性
    /// - 同一路径多次 `new()` 不应失败(`CREATE TABLE IF NOT EXISTS`)
    /// - 文件已存在 + schema 已就绪 → 成功
    /// - 文件存在但损坏 → 失败,返回 `TuiError::SqliteError`
    pub async fn new(db_path: &Path) -> Result<Self, TuiError> {
        // WHY 提前 clone:db_path 需 move 进 spawn_blocking 闭包,函数返回 Self
        // 也需持有副本(供调试日志用)
        let db_path = db_path.to_path_buf();
        let db_path_for_blocking = db_path.clone();

        // WHY spawn_blocking:rusqlite Connection::open + pragma_update +
        // execute_batch 都是同步阻塞 I/O,直接 await 会阻塞 Tokio runtime
        // 工作线程(§4.4 #2 实战红线)
        let conn = spawn_blocking(move || -> Result<Connection, TuiError> {
            let conn = Connection::open(&db_path_for_blocking)
                .map_err(|e| TuiError::SqliteError(format!("open db: {e}")))?;

            // WHY WAL 模式:write-ahead logging 让读操作不被写操作阻塞,
            // 即使后续 TUI 多面板并发读历史也能流畅。
            // `synchronous=NORMAL` 在 WAL 下仍保证一致性但减少 fsync 开销。
            conn.pragma_update(None, "journal_mode", "WAL")
                .map_err(|e| TuiError::SqliteError(format!("pragma journal_mode: {e}")))?;
            conn.pragma_update(None, "synchronous", "NORMAL")
                .map_err(|e| TuiError::SqliteError(format!("pragma synchronous: {e}")))?;

            // CREATE TABLE IF NOT EXISTS 保证幂等(同路径多次 open 不冲突)
            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS samples (
                    unix_ts INTEGER NOT NULL,
                    metric  TEXT    NOT NULL,
                    value   REAL    NOT NULL,
                    PRIMARY KEY(unix_ts, metric)
                );",
            )
            .map_err(|e| TuiError::SqliteError(format!("create table: {e}")))?;

            Ok(conn)
        })
        .await
        .map_err(|e| TuiError::SqliteError(format!("spawn_blocking join: {e}")))??;

        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
            db_path,
        })
    }

    /// 打开数据库到默认路径:`~/.chimera/metrics_history.sqlite`
    ///
    /// # 路径解析(WHY)
    /// - 优先 `HOME`(Unix / Git Bash)
    /// - 回退 `USERPROFILE`(Windows 原生)
    /// - 终极回退 `.`(避免 panic,虽然路径不合理但调用方可检测)
    ///
    /// 与 `TuiConfig::default_path` 解析策略一致,保证 TUI 子系统的
    /// 配置与历史文件位于同一 `~/.chimera/` 目录。
    pub async fn open_default() -> Result<Self, TuiError> {
        let path = Self::default_db_path();
        // 确保父目录存在(首次启动时 `~/.chimera/` 可能尚未创建)
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| TuiError::SqliteError(format!("create dir: {e}")))?;
        }
        Self::new(&path).await
    }

    /// 返回默认数据库路径:`~/.chimera/metrics_history.sqlite`
    pub fn default_db_path() -> PathBuf {
        let home = std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .unwrap_or_else(|_| ".".to_string());
        PathBuf::from(home)
            .join(".chimera")
            .join("metrics_history.sqlite")
    }

    /// 返回数据库文件路径(用于调试与诊断)
    pub fn db_path(&self) -> &Path {
        &self.db_path
    }

    /// 插入或替换一个采样点(`INSERT ... ON CONFLICT REPLACE`)
    ///
    /// # 幂等语义
    /// 同 `(ts, metric)` 重复插入会替换为最新 `value`(非追加),
    /// 保证主键唯一 + 历史曲线平滑(无阶梯)。
    ///
    /// # 性能
    /// 复合主键定位 O(log n),单次插入微秒级,可在 DataPipeline 每个
    /// 采样 tick(默认 1Hz)安全调用。
    pub async fn insert(&self, ts: u64, metric: &str, value: f64) -> Result<(), TuiError> {
        let metric = metric.to_string();
        let conn = Arc::clone(&self.conn);
        spawn_blocking(move || {
            // WHY 在闭包内 lock:MutexGuard 不能跨 spawn_blocking 边界(需 Send),
            // 必须在闭包内获取并随闭包结束自动 drop。锁持有时间 < 1ms(单 INSERT)
            let guard = lock_conn(&conn)?;
            guard
                .execute(
                    "INSERT INTO samples (unix_ts, metric, value) VALUES (?1, ?2, ?3)
                     ON CONFLICT(unix_ts, metric) DO UPDATE SET value = excluded.value;",
                    params![ts as i64, metric, value],
                )
                .map_err(|e| TuiError::SqliteError(format!("insert: {e}")))?;
            Ok(())
        })
        .await
        .map_err(|e| TuiError::SqliteError(format!("spawn_blocking join: {e}")))?
    }

    /// 查询指定时间范围内某 metric 的所有采样点
    ///
    /// # 返回
    /// 按 `unix_ts ASC` 排序的 `Vec<MetricSample>`,便于面板直接绘制曲线
    /// (sparkline / line_chart 都按时间升序输入)。
    ///
    /// # 边界
    /// - `start..=end` 是闭区间 SQL 语义,实现为 `unix_ts >= ?1 AND unix_ts <= ?2`
    /// - 空范围(无命中行)返回 `Ok(Vec::new())`,非错误
    /// - `metric` 为空字符串时返回 `Ok(Vec::new())`(避免误查"全部"代价过高)
    pub async fn query_range(
        &self,
        metric: &str,
        start: u64,
        end: u64,
    ) -> Result<Vec<MetricSample>, TuiError> {
        let metric = metric.to_string();
        if metric.is_empty() {
            // WHY 早返回:空 metric 命中 SQL `metric = ''` 会扫描全表过滤空串,
            // 几乎必返回空集。提前返回避免无谓 spawn_blocking + SQL 执行。
            return Ok(Vec::new());
        }
        let conn = Arc::clone(&self.conn);
        spawn_blocking(move || {
            let guard = lock_conn(&conn)?;
            let mut stmt = guard
                .prepare(
                    "SELECT unix_ts, value FROM samples
                     WHERE unix_ts >= ?1 AND unix_ts <= ?2
                       AND metric = ?3
                     ORDER BY unix_ts ASC;",
                )
                .map_err(|e| TuiError::SqliteError(format!("prepare query_range: {e}")))?;
            let rows = stmt
                .query_map(params![start as i64, end as i64, metric], |row| {
                    let ts: i64 = row.get(0)?;
                    let value: f64 = row.get(1)?;
                    Ok(MetricSample::new(ts as u64, value as f32))
                })
                .map_err(|e| TuiError::SqliteError(format!("query_map: {e}")))?;
            let mut out = Vec::new();
            for r in rows {
                out.push(r.map_err(|e| TuiError::SqliteError(format!("row: {e}")))?);
            }
            Ok(out)
        })
        .await
        .map_err(|e| TuiError::SqliteError(format!("spawn_blocking join: {e}")))?
    }

    /// 删除早于保留期(单位:天)的所有行
    ///
    /// # 算法
    /// `now_ms = unix_time_ms()`,然后 `DELETE WHERE unix_ts < now_ms - retention_days * 86_400_000`。
    /// 一次 SQL 扫描完成删除(7 天 × 604_800 行在 SQLite 中 < 10ms)。
    ///
    /// # 返回
    /// 受影响行数(便于上层做监控 / 日志)。
    pub async fn cleanup(&self, retention_days: u32) -> Result<usize, TuiError> {
        let now_ms = current_unix_ms();
        let cutoff = now_ms.saturating_sub(u64::from(retention_days) * MS_PER_DAY);
        let conn = Arc::clone(&self.conn);
        spawn_blocking(move || {
            let guard = lock_conn(&conn)?;
            let affected = guard
                .execute(
                    "DELETE FROM samples WHERE unix_ts < ?1;",
                    params![cutoff as i64],
                )
                .map_err(|e| TuiError::SqliteError(format!("cleanup delete: {e}")))?;
            Ok(affected)
        })
        .await
        .map_err(|e| TuiError::SqliteError(format!("spawn_blocking join: {e}")))?
    }
}

/// 加锁 SQLite 连接(自由函数,供 `spawn_blocking` 闭包内调用)
///
/// WHY 独立函数:闭包内 `lock().unwrap_or_else()` 模板代码重复 3 处,
/// 提取为自由函数避免复制粘贴错误。
///
/// # Mutex poison 恢复(§4.4 #1 反模式防御)
/// Mutex poison 表示持锁任务 panic,但连接本身仍可安全使用(无未完成事务时);
/// 继续执行而非 propagate 错误,保证后台任务不会因历史 panic 而永久失败。
fn lock_conn(conn: &Arc<Mutex<Connection>>) -> Result<MutexGuard<'_, Connection>, TuiError> {
    Ok(conn.lock().unwrap_or_else(|poisoned| {
        tracing::warn!("MetricsHistory Mutex was poisoned; recovering");
        poisoned.into_inner()
    }))
}

/// 返回当前 Unix 毫秒时间戳
///
/// WHY 独立函数:`SystemTime::now()` 错误时(理论上不可能,系统时间倒退)
/// fallback 到 0,避免 propagate 错误到 cleanup() 调用方。
/// 历史上 Linux 闰秒调整 / NTP 同步可能让 `duration_since(UNIX_EPOCH)` 失败。
fn current_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 单元测试:仅验证 `default_db_path` 路径语义(不依赖文件系统)
    #[test]
    fn test_default_db_path_ends_with_metrics_history_sqlite() {
        let path = MetricsHistory::default_db_path();
        assert!(
            path.ends_with("metrics_history.sqlite"),
            "default path should end with metrics_history.sqlite, got: {path:?}"
        );
    }

    /// 单元测试:验证时间戳换算常量的正确性
    #[test]
    fn test_ms_per_day_constant() {
        assert_eq!(MS_PER_DAY, 24 * 60 * 60 * 1000);
        assert_eq!(MS_PER_DAY, 86_400_000);
    }

    /// 单元测试:current_unix_ms 必须返回非零正值(系统时间未倒退)
    #[test]
    fn test_current_unix_ms_returns_reasonable_value() {
        let now = current_unix_ms();
        // 2020-01-01 = 1_577_836_800_000 ms
        assert!(
            now > 1_577_836_800_000,
            "now ({now}) should be after 2020-01-01"
        );
    }
}
