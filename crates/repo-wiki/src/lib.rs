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
//! # 文档-代码偏差记录(《最新版》§10 对齐,P1-3 深度优化)
//! - **P1-1 嵌入接入**(2026-08):`WikiGenerator::with_text_encoder` 接入 L2
//!   nmc-encoder TextPerceptor,替换 DEFERRED(T8-3 Audit) 占位状态;
//!   默认路径(无编码器)保持 SHA-256 占位 512 维,向后兼容。
//! - **P1-2 检索收敛**(2026-08):`WikiStore::hybrid_query` 为 L5 唯一融合
//!   入口(HNSW dense + FTS5 sparse + RRF),chimera-mas WikiRetriever 与
//!   chimera-cli wiki 命令均已收敛到此入口(Ω₆-Reuse)。
//! - **Task 5 双层经验库**(2026-08,文档 §10.2 问题 4):`DualExperienceBank`
//!   落于本 crate(案例级 = WikiEntry,全局蒸馏 = distilled_insights 表),
//!   不触碰 L2 mlc-engine(记忆层职责边界)。
//! - 文档 §20 规划的独立 crate(dual-experience-bank 等)经 ADR-049 决策 1
//!   否决,以模块形式落地于现有 crate。
//! - **测试外移**(2026-08):store.rs 公共 API 测试迁至
//!   `tests/store_public_api.rs`;search.rs RRF 公共 API 单元测试迁至
//!   `tests/search_public_api.rs`;vector/(hnsw/memory_knn)与 fts.rs 内嵌
//!   测试依赖私有字段/私有 sanitize 函数,逐例判定保留原地。
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

/// polish-v2.7 closure Stage B-9:Agent Grep 双通道结构化搜索(jcode,ADR-049)
///
/// 知识通道(FTS5→LIKE 降级)+ 代码通道(BGPD 三级披露)合成,零新增索引。
pub mod agent_grep;
/// polish-v2.7 P4-8:行为定位(BGPD 三级渐进披露导航,腾讯 Handbook,ADR-049)
pub mod behavior_localization;
pub mod config;
pub mod contradiction;
pub mod error;
/// P1-3(计划 Task 5): 双层经验库 — 案例级(WikiEntry)+ 全局蒸馏(DistilledInsight)
pub mod experience_bank;
pub mod fts;
pub mod generator;
pub mod iscm;
pub mod metrics;
/// polish-v2.7 P4-7:流程蓝图提取器(轨迹→蓝图沉淀,北大 DataFlow,ADR-049)
pub mod procedural_blueprint;
pub mod relation;
/// RAG 混合检索融合 — Reciprocal Rank Fusion (RRF),融合 HNSW dense 与 FTS5 sparse 结果
pub mod search;
/// polish-v2.7 P4-6:技能依赖图与复用率优先推荐(Ω₆ Reuse,ADR-049)
pub mod skill_graph;
/// Phase 5 §10.5:Skill 生命周期状态机(MSCE Probationary→Active→Archived,ADR-049 内嵌)
pub mod skill_lifecycle;
/// Phase 6 §11.1:Skills 渐进加载(PenguinHarness Index First/Body on Demand,ADR-049 内嵌)
pub mod skills_progressive_loader;
pub mod store;
pub mod types;
pub mod vector;

// === 关键类型重导出,简化外部导入 ===
pub use contradiction::{
    ContradictionDetector, ContradictionResult, DEFAULT_CONTRADICTION_THRESHOLD,
};
pub use error::WikiError;
/// P1-3(计划 Task 5): 双层经验库公开 API 重导出
pub use experience_bank::{DistilledInsight, DualExperienceBank};
pub use fts::FtsCapability;
pub use generator::WikiGenerator;
pub use iscm::{IscmAnchor, Layer};
pub use metrics::WikiMetrics;
pub use relation::{EntryRelation, RelationKind};
/// RAG 混合检索融合 — RRF 算法融合 HNSW dense 与 FTS5 sparse 结果(Task 3)
pub use search::{hybrid_search, rrf_fuse, HybridSearchConfig, HybridSearchResult};
/// Phase 5 §10.5: Skill 生命周期管理器公开 API 重导出
pub use skill_lifecycle::SkillLifecycleManager;
/// Phase 6 §11.1: Skills 渐进加载器公开 API 重导出
pub use skills_progressive_loader::{
    skill_metadata_from_graph, BodyProvider, LoadedSkill, LoaderStats, ProgressiveSkillLoader,
    SkillBody, SkillMetadata,
};
pub use store::WikiStore;
pub use types::{HnswConfig, WikiConfig, WikiEntry};
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
    /// P1-3(计划 Task 5): 双层经验库 prelude 导出
    pub use crate::experience_bank::{DistilledInsight, DualExperienceBank};
    pub use crate::fts::FtsCapability;
    pub use crate::generator::WikiGenerator;
    pub use crate::iscm::{IscmAnchor, Layer};
    pub use crate::metrics::WikiMetrics;
    pub use crate::relation::{EntryRelation, RelationKind};
    /// RAG 混合检索融合 — RRF 算法融合 HNSW dense 与 FTS5 sparse 结果(Task 3)
    pub use crate::search::{hybrid_search, rrf_fuse, HybridSearchConfig, HybridSearchResult};
    pub use crate::store::WikiStore;
    pub use crate::types::{HnswConfig, WikiConfig, WikiEntry};
    /// HNSW 向量存储生产路径实现(P2-W8.1)
    pub use crate::vector::hnsw_store::HnswStore;
    /// 内存 KNN 向量存储 fallback 路径实现(P2-W8.2)
    pub use crate::vector::memory_knn_store::MemoryKnnStore;
    /// 向量索引 — 历史类型名(向后兼容)
    pub use crate::vector::VectorIndex;
}
