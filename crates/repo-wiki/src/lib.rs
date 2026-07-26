//! 仓库知识沉淀 — 跨层共享索引的代码 Wiki 与知识图谱
//!
//! 对应架构层:L5 Knowledge
//! 对应创新点:ISCM(Inter-Shared Cross Module,跨层共享索引)
//!
//! # 核心职责
//! - 使用 SQLite 持久化 Wiki 条目(标题、内容、标签、嵌入向量)
//! - 提供向量相似度检索(KNN),支持语义召回
//! - 从 `nexus_core::Quest` 结果自动生成 Wiki 条目
//! - 通过 `event_bus::EventBus` 发布 `WikiUpdated` 事件通知上层
//!
//! # 架构红线
//! - 写操作通过专用写入线程(mpsc + oneshot)序列化,读操作通过只读连接池
//!   在 `spawn_blocking` 中并发执行;从而利用 SQLite WAL 的读写并发能力
//! - `#![forbid(unsafe_code)]` 禁止 unsafe,因此 sqlite-vec 集成降级为内存向量检索
//! - 单函数 ≤ 200 行,所有可能失败的边界用 `?` 处理
//!
//! # 快速示例
//! ```
//! use repo_wiki::{WikiStore, WikiEntry, VectorIndex};
//! use std::path::Path;
//!
//! # #[tokio::main] async fn main() -> Result<(), Box<dyn std::error::Error>> {
//! let tmp = tempfile::tempdir()?;
//! let store = WikiStore::open(&tmp.path().join("wiki.db"))?;
//!
//! let entry = WikiEntry::new(
//!     "e-1",
//!     "Rust 异步编程",
//!     "Tokio 是 Rust 生态最主流的异步运行时",
//!     vec!["rust".into(), "async".into()],
//!     vec![0.0; 512],
//! );
//! // 所有 SQLite 操作均为 async,通过 spawn_blocking 在阻塞线程池执行
//! store.insert(entry).await?;
//!
//! let fetched = store.get("e-1".to_string()).await?.unwrap();
//! assert_eq!(fetched.title, "Rust 异步编程");
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

pub mod config;
pub mod contradiction;
pub mod error;
pub mod fts;
pub mod generator;
pub mod iscm;
pub mod metrics;
pub mod relation;
pub mod store;
pub mod types;
pub mod vector;

// === 关键类型重导出,简化外部导入 ===
pub use contradiction::{
    ContradictionDetector, ContradictionResult, DEFAULT_CONTRADICTION_THRESHOLD,
};
pub use error::WikiError;
pub use fts::FtsCapability;
pub use generator::WikiGenerator;
pub use iscm::{IscmAnchor, Layer};
pub use metrics::WikiMetrics;
pub use relation::{EntryRelation, RelationKind};
pub use store::WikiStore;
pub use types::{WikiConfig, WikiEntry};
/// HNSW 向量存储生产路径实现(P2-W8.1)
pub use vector::hnsw_store::HnswStore;
/// 内存 KNN 向量存储 fallback 路径实现(P2-W8.2)
pub use vector::memory_knn_store::MemoryKnnStore;
/// 向量索引 — 历史类型名,现为 `MemoryKnnStore` 的类型别名(向后兼容)
pub use vector::VectorIndex;

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::contradiction::{
        ContradictionDetector, ContradictionResult, DEFAULT_CONTRADICTION_THRESHOLD,
    };
    pub use crate::error::WikiError;
    pub use crate::fts::FtsCapability;
    pub use crate::generator::WikiGenerator;
    pub use crate::iscm::{IscmAnchor, Layer};
    pub use crate::metrics::WikiMetrics;
    pub use crate::relation::{EntryRelation, RelationKind};
    pub use crate::store::WikiStore;
    pub use crate::types::{WikiConfig, WikiEntry};
    /// HNSW 向量存储生产路径实现(P2-W8.1)
    pub use crate::vector::hnsw_store::HnswStore;
    /// 内存 KNN 向量存储 fallback 路径实现(P2-W8.2)
    pub use crate::vector::memory_knn_store::MemoryKnnStore;
    /// 向量索引 — 历史类型名(向后兼容)
    pub use crate::vector::VectorIndex;
}
