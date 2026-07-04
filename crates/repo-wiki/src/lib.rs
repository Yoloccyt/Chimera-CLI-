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
//! - 所有 SQLite 操作通过 `Arc<Mutex<Connection>>` 串行化(线程安全),
//!   并使用 `spawn_blocking` 转移到阻塞线程池(避免阻塞 async runtime)
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
pub mod error;
pub mod generator;
pub mod iscm;
pub mod store;
pub mod types;
pub mod vector;

// === 关键类型重导出,简化外部导入 ===
pub use error::WikiError;
pub use generator::WikiGenerator;
pub use iscm::{IscmAnchor, Layer};
pub use store::WikiStore;
pub use types::{WikiConfig, WikiEntry};
pub use vector::VectorIndex;

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::error::WikiError;
    pub use crate::generator::WikiGenerator;
    pub use crate::iscm::{IscmAnchor, Layer};
    pub use crate::store::WikiStore;
    pub use crate::types::{WikiConfig, WikiEntry};
    pub use crate::vector::VectorIndex;
}
