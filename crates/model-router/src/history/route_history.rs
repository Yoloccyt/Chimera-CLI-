//! `route_history` — 通道化路由历史存储(ADR-065 M3,§5.4)
//!
//! 对应架构层:L1 Core(model-router)
//! 对应设计源:`Chimera_全模型亲和适配体系设计文档_v1.0.md` §5.4
//!
//! # 新建表而非改造(R3 风险缓解)
//! 通道化后路由键是三元组(provider/model/thinking),与既有 `history` 表的
//! `model_id TEXT PRIMARY KEY` 冲突。**新建 `route_history` 表**(含
//! provider/cost_actual/cache_hit_tokens/ttft_ms 列),既有 `history` 表
//! (PK=model_id)零改动继续服务既有 MoE 路由策略,双表并存至 M4 验收后
//! 再评估归并(设计文档 §10 M3 计划)。
//!
//! # Schema
//! ```sql
//! CREATE TABLE IF NOT EXISTS route_history (
//!     route_key         TEXT PRIMARY KEY,   -- provider/model
//!     provider          TEXT NOT NULL,
//!     success_count     INTEGER NOT NULL DEFAULT 0,
//!     total_count       INTEGER NOT NULL DEFAULT 0,
//!     cost_actual_micro INTEGER NOT NULL DEFAULT 0,  -- 累计实际成本(微元)
//!     cache_hit_tokens  INTEGER NOT NULL DEFAULT 0,  -- 累计缓存命中 token
//!     ttft_ms_ewma      REAL NOT NULL DEFAULT 0       -- TTFT EWMA(毫秒)
//! );
//! ```
//!
//! # C7 红线
//! 同步 `Mutex<Connection>`(与既有 SqliteHistoryStore 一致);调用方在 async
//! 上下文用 `spawn_blocking` 包装(§4.4 #7)。锁内 SELECT-merge-UPSERT 全部
//! 完成,不跨 await。

#![forbid(unsafe_code)]

use std::path::Path;
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};

use crate::error::RouterError;

/// TTFT EWMA 平滑系数(对齐 mca-gateway health.rs 与 ADR-037)
const TTFT_EWMA_ALPHA: f64 = 0.1;

/// 单条通道路由记录(读回快照)
#[derive(Debug, Clone, PartialEq)]
pub struct RouteRecord {
    /// 路由键 provider/model
    pub route_key: String,
    /// 厂商标识字符串
    pub provider: String,
    /// 成功次数
    pub success_count: u64,
    /// 总调用次数
    pub total_count: u64,
    /// 累计实际成本(微元)
    pub cost_actual_micro: u64,
    /// 累计缓存命中 token
    pub cache_hit_tokens: u64,
    /// TTFT EWMA(毫秒)
    pub ttft_ms_ewma: f64,
}

impl RouteRecord {
    /// 成功率(无调用返回 0)
    pub fn success_rate(&self) -> f64 {
        if self.total_count == 0 {
            0.0
        } else {
            self.success_count as f64 / self.total_count as f64
        }
    }
}

/// 通道化路由历史存储 — 新建 route_history 表(与 history 表并存)
#[derive(Debug)]
pub struct RouteHistoryStore {
    conn: Mutex<Connection>,
}

impl RouteHistoryStore {
    /// 打开/创建 route_history 数据库(WAL 模式 + 建表)
    pub fn new(path: &Path) -> Result<Self, RouterError> {
        let conn = Connection::open(path)
            .map_err(|e| RouterError::SqliteHistoryError(format!("open route_history: {e}")))?;
        conn.pragma_update(None, "journal_mode", "WAL")
            .map_err(|e| RouterError::SqliteHistoryError(format!("pragma journal_mode: {e}")))?;
        conn.pragma_update(None, "synchronous", "NORMAL")
            .map_err(|e| RouterError::SqliteHistoryError(format!("pragma synchronous: {e}")))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS route_history (
                route_key         TEXT PRIMARY KEY,
                provider          TEXT NOT NULL,
                success_count     INTEGER NOT NULL DEFAULT 0,
                total_count       INTEGER NOT NULL DEFAULT 0,
                cost_actual_micro INTEGER NOT NULL DEFAULT 0,
                cache_hit_tokens  INTEGER NOT NULL DEFAULT 0,
                ttft_ms_ewma      REAL NOT NULL DEFAULT 0
            );",
        )
        .map_err(|e| RouterError::SqliteHistoryError(format!("init route_history schema: {e}")))?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// 打开内存库(测试用)
    pub fn in_memory() -> Result<Self, RouterError> {
        let conn = Connection::open_in_memory()
            .map_err(|e| RouterError::SqliteHistoryError(format!("open memory: {e}")))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS route_history (
                route_key         TEXT PRIMARY KEY,
                provider          TEXT NOT NULL,
                success_count     INTEGER NOT NULL DEFAULT 0,
                total_count       INTEGER NOT NULL DEFAULT 0,
                cost_actual_micro INTEGER NOT NULL DEFAULT 0,
                cache_hit_tokens  INTEGER NOT NULL DEFAULT 0,
                ttft_ms_ewma      REAL NOT NULL DEFAULT 0
            );",
        )
        .map_err(|e| RouterError::SqliteHistoryError(format!("init route_history schema: {e}")))?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// 记录一次通道调用结果(SELECT-merge-UPSERT,锁内完成不跨 await)
    ///
    /// 消费 `StreamSessionCompleted` 事件字段:cost/cache_hit/ttft 累加,
    /// TTFT 走 EWMA(首样本直取,后续平滑)。
    pub fn record(
        &self,
        route_key: &str,
        provider: &str,
        success: bool,
        cost_micro: u64,
        cache_hit_tokens: u64,
        ttft_ms: u64,
    ) -> Result<(), RouterError> {
        let guard = self
            .conn
            .lock()
            .map_err(|_| RouterError::SqliteHistoryError("route_history mutex poisoned".into()))?;
        // 读旧值(计算 EWMA 基线)
        let prev = read_row(&guard, route_key)?;
        let ttft_ewma = match prev.as_ref() {
            // 首个样本或旧 EWMA 为 0:直接取当前值(避免 0 基线拖低)
            Some(r) if r.ttft_ms_ewma > 0.0 => {
                TTFT_EWMA_ALPHA * ttft_ms as f64 + (1.0 - TTFT_EWMA_ALPHA) * r.ttft_ms_ewma
            }
            _ => ttft_ms as f64,
        };
        let (succ0, total0, cost0, cache0) = prev
            .map(|r| {
                (
                    r.success_count,
                    r.total_count,
                    r.cost_actual_micro,
                    r.cache_hit_tokens,
                )
            })
            .unwrap_or((0, 0, 0, 0));
        guard
            .execute(
                "INSERT OR REPLACE INTO route_history
                    (route_key, provider, success_count, total_count,
                     cost_actual_micro, cache_hit_tokens, ttft_ms_ewma)
                    VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    route_key,
                    provider,
                    (succ0 + u64::from(success)) as i64,
                    (total0 + 1) as i64,
                    (cost0 + cost_micro) as i64,
                    (cache0 + cache_hit_tokens) as i64,
                    ttft_ewma,
                ],
            )
            .map_err(|e| RouterError::SqliteHistoryError(format!("upsert route_history: {e}")))?;
        Ok(())
    }

    /// 读取通道路由记录(不存在返回 None)
    pub fn get(&self, route_key: &str) -> Result<Option<RouteRecord>, RouterError> {
        let guard = self
            .conn
            .lock()
            .map_err(|_| RouterError::SqliteHistoryError("route_history mutex poisoned".into()))?;
        read_row(&guard, route_key)
    }
}

/// 读取单行(内部辅助,复用于 record 的 SELECT-merge)
fn read_row(conn: &Connection, route_key: &str) -> Result<Option<RouteRecord>, RouterError> {
    conn.query_row(
        "SELECT route_key, provider, success_count, total_count,
                cost_actual_micro, cache_hit_tokens, ttft_ms_ewma
            FROM route_history WHERE route_key = ?1",
        params![route_key],
        |row| {
            Ok(RouteRecord {
                route_key: row.get(0)?,
                provider: row.get(1)?,
                success_count: row.get::<_, i64>(2)? as u64,
                total_count: row.get::<_, i64>(3)? as u64,
                cost_actual_micro: row.get::<_, i64>(4)? as u64,
                cache_hit_tokens: row.get::<_, i64>(5)? as u64,
                ttft_ms_ewma: row.get(6)?,
            })
        },
    )
    .optional()
    .map_err(|e| RouterError::SqliteHistoryError(format!("select route_history: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_and_get_accumulates() {
        let store = RouteHistoryStore::in_memory().unwrap();
        store
            .record(
                "deep_seek/deepseek-v4-flash",
                "deep_seek",
                true,
                1000,
                600,
                200,
            )
            .unwrap();
        store
            .record(
                "deep_seek/deepseek-v4-flash",
                "deep_seek",
                false,
                2000,
                0,
                400,
            )
            .unwrap();
        let r = store.get("deep_seek/deepseek-v4-flash").unwrap().unwrap();
        assert_eq!(r.total_count, 2);
        assert_eq!(r.success_count, 1);
        assert_eq!(r.cost_actual_micro, 3000);
        assert_eq!(r.cache_hit_tokens, 600);
        assert!((r.success_rate() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn ttft_ewma_first_sample_direct_then_smoothed() {
        let store = RouteHistoryStore::in_memory().unwrap();
        store
            .record("zhipu/glm-5.2", "zhipu", true, 0, 0, 100)
            .unwrap();
        let r1 = store.get("zhipu/glm-5.2").unwrap().unwrap();
        assert!((r1.ttft_ms_ewma - 100.0).abs() < 1e-6, "首样本直取");
        store
            .record("zhipu/glm-5.2", "zhipu", true, 0, 0, 300)
            .unwrap();
        let r2 = store.get("zhipu/glm-5.2").unwrap().unwrap();
        // EWMA: 0.1*300 + 0.9*100 = 120
        assert!(
            (r2.ttft_ms_ewma - 120.0).abs() < 1e-6,
            "EWMA 平滑,实际 {}",
            r2.ttft_ms_ewma
        );
    }

    #[test]
    fn get_missing_returns_none() {
        let store = RouteHistoryStore::in_memory().unwrap();
        assert!(store.get("unknown/model").unwrap().is_none());
    }

    #[test]
    fn coexists_with_legacy_history_table() {
        // route_history 与既有 history 表并存:同库建两表互不干扰(R3 缓解验证)
        let store = RouteHistoryStore::in_memory().unwrap();
        // 在同连接上建旧 history 表,验证不冲突
        {
            let guard = store.conn.lock().unwrap();
            guard
                .execute_batch(
                    "CREATE TABLE IF NOT EXISTS history (
                        model_id TEXT PRIMARY KEY,
                        success_count INTEGER NOT NULL DEFAULT 0,
                        total_count INTEGER NOT NULL DEFAULT 0,
                        latency_samples BLOB NOT NULL DEFAULT x''
                    );",
                )
                .unwrap();
        }
        // route_history 仍正常工作
        store.record("x/y", "x", true, 100, 50, 150).unwrap();
        assert_eq!(store.get("x/y").unwrap().unwrap().total_count, 1);
    }
}
