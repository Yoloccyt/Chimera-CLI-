//! 推测上下文缓存 — 基于访问模式推测性预取的上下文缓存
//!
//! 对应架构层:L3 Storage
//! 对应创新点:SCC(Speculative Context Cache)
//!
//! # 核心职责
//! - 基于 DashMap 的并发安全上下文缓存(Draft/Verify 共享 `Arc<ContextEntry>`)
//! - 二阶马尔可夫链访问模式学习与推测性预取
//! - LRU 驱逐策略(Arc 引用保护:strong_count > 1 时不驱逐)
//! - EventBus 集成:发布 CacheHit/CacheMiss/CachePrefetched/CacheStatsReported 事件
//!
//! # 架构红线
//! - 所有跨层通信走 EventBus(§2.2 依赖铁律)
//! - SCC(L3)→ GQEP(L6) 向上依赖禁止,预取逻辑在 SCC 内部 tokio::spawn 完成
//! - 单函数 ≤ 200 行,禁止 unwrap()/expect() 在非测试代码
//! - 所有 async fn 满足 Send + 'static 约束
//!
//! # 快速示例
//! ```no_run
//! use scc_cache::{SccCache, SccConfig, ContextEntry, AccessPatternLearner, ContextId};
//! use event_bus::EventBus;
//! use std::sync::Arc;
//!
//! # async fn run() {
//! let bus = EventBus::new();
//! let cache = SccCache::new(SccConfig::default(), bus.clone());
//! let learner = Arc::new(AccessPatternLearner::new(bus, 0.6));
//!
//! // 插入上下文
//! cache.insert(ContextEntry::new("ctx-1", "content-1"));
//!
//! // Producer 与 Verifier 共享同一 Arc<ContextEntry>
//! let producer_entry = cache.get_or_prefetch(&ContextId::new("ctx-1")).unwrap();
//! let verifier_entry = cache.get_or_prefetch(&ContextId::new("ctx-1")).unwrap();
//! assert!(Arc::ptr_eq(&producer_entry, &verifier_entry));
//!
//! // 学习访问模式并预取(二阶马尔可夫链:previous → current → next)
//! learner.record_access(&ContextId::new("ctx-0"), &ContextId::new("ctx-1"), &ContextId::new("ctx-2"));
//! let prefetched = learner.prefetch(&ContextId::new("ctx-0"), &ContextId::new("ctx-1"), &cache);
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

// === 模块声明 ===
pub mod cache;
pub mod config;
pub mod error;
pub mod lru;
pub mod prefetch;
pub mod types;
pub mod wal;

// === 关键类型重导出,简化外部导入 ===
pub use cache::SccCache;
pub use config::SccConfig;
pub use error::SccError;
pub use prefetch::AccessPatternLearner;
pub use types::{AccessPattern, CacheStats, ContextEntry, ContextId};
// WHY 顶层导出 SqliteWal:C-02 修复后 SqliteWal 改为 async API(不再实现 WalTrait),
// 外部调用方需直接引用 SqliteWal 类型以调用 async 方法
pub use wal::{InMemoryWal, SqliteWal, WalEntry, WalOperation, WalTrait};

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::cache::SccCache;
    pub use crate::config::SccConfig;
    pub use crate::error::SccError;
    pub use crate::prefetch::AccessPatternLearner;
    pub use crate::types::{AccessPattern, CacheStats, ContextEntry, ContextId};
    pub use crate::wal::{InMemoryWal, SqliteWal, WalEntry, WalOperation, WalTrait};
}
