//! `InMemoryHistoryStore` — DashMap 并发安全内存历史存储(v1.3.0 默认实现)
//!
//! 对应架构层:L1 Core(model-router)
//!
//! # 设计要点
//! - DashMap 并发读写无锁(sharded locking),适合路由热路径多线程记录场景
//! - `entry().or_default()` 原子写入避免 TOCTOU 竞态
//! - v1.4.0 P1:从 `moe.rs` 迁移到 `history` 模块,行为完全不变(向后兼容)

#![forbid(unsafe_code)]

use dashmap::DashMap;

use crate::history::{HistoryRecord, HistoryStore};

/// 内存实现(DashMap 并发安全)— 默认 HistoryStore 实现
///
/// WHY DashMap:并发读写无锁(sharded locking),适合路由热路径多线程
/// 记录场景。DashMap 内部 unsafe 不传播到当前 crate(§4.1 forbid 语义)。
///
/// # 使用示例
/// ```
/// use model_router::{InMemoryHistoryStore, HistoryStore};
///
/// let store = InMemoryHistoryStore::new();
/// store.record("gpt-4", 200.0, true);
/// let record = store.get("gpt-4").unwrap();
/// assert_eq!(record.total_count, 1);
/// assert_eq!(record.success_count, 1);
/// ```
#[derive(Debug, Default)]
pub struct InMemoryHistoryStore {
    records: DashMap<String, HistoryRecord>,
}

impl InMemoryHistoryStore {
    /// 创建空的历史存储
    pub fn new() -> Self {
        Self {
            records: DashMap::new(),
        }
    }
}

impl HistoryStore for InMemoryHistoryStore {
    fn get(&self, model_id: &str) -> Option<HistoryRecord> {
        // 返回 owned clone:避免返回 DashMap Ref guard(生命周期约束复杂,
        // 且 guard 持锁可能影响并发写入)。克隆成本 ~400B,路由热路径可忽略。
        self.records.get(model_id).map(|r| r.clone())
    }

    fn record(&self, model_id: &str, latency_ms: f32, success: bool) {
        // WHY entry().or_default() 而非 get_mut:原子地"不存在则创建+写入",
        // 避免 get → 判空 → insert 的 TOCTOU 竞态(两线程同时创建同一 model_id)。
        let mut r = self.records.entry(model_id.to_string()).or_default();
        r.record(latency_ms, success);
    }
}
