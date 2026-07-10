//! KV 块语义路由器 — 两级块路由的键值缓存语义检索
//!
//! 对应架构层:L6 Router
//! 对应创新点:KVBSR(KV-Block Semantic Router)
//!
//! # 核心职责
//! - 两级路由:第一级选 Top-N 块,第二级在块内选 Top-K 工具
//! - 语义块构建:基于工具共现频率聚类(Union-Find),块向量 = 工具向量加权平均
//! - 自动重平衡:每 N 次路由重新分析共现频率,原子切换块列表
//! - 事件发布:路由完成发布 `ToolsRouted`,重平衡完成发布 `BlocksRebalanced`
//!
//! # 两级路由流程
//! 1. 将 CLV(512 维)截取前 64 维作为查询向量
//! 2. 第一级(块级):计算查询向量与各 block_vector 的余弦相似度,选 Top-3 块
//! 3. 第二级(块内):在选中块的并集工具集内,选 Top-8 工具
//! 4. 发布 ToolsRouted 事件,返回 RoutingResult
//!
//! # 性能基准(300 工具规模)
//! - 路由延迟 < 2ms
//! - 块数量 10-30 个
//! - 路由准确率 > 85%(20 条标注用例)
//! - 两级路由加速比 > 10×(相比全量扫描)
//!
//! # 架构红线
//! - 所有跨层通信走 EventBus(§2.2 依赖铁律)
//! - 单函数 ≤ 200 行,禁止 unwrap()/expect()
//! - 所有 async fn 满足 Send 约束
//! - 持锁状态下不可 await,避免死锁
//!
//! # 快速示例
//! ```no_run
//! use kvbsr_router::{KVBlockSemanticRouter, ToolVector, CoOccurrenceMatrix};
//! use event_bus::EventBus;
//! use nexus_core::CLV;
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let bus = EventBus::new();
//! let router = KVBlockSemanticRouter::new(bus.clone());
//!
//! let tools = vec![ToolVector::new("t1", vec![1.0; 64], 100)];
//! let co = CoOccurrenceMatrix::new();
//! router.build_blocks(tools, co).await?;
//!
//! let clv = CLV::zero();
//! let result = router.route(&clv).await?;
//! println!("选中 {} 个工具,延迟 {:.2}ms", result.routed_count(), result.latency_ms);
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

// === 模块声明 ===
pub mod blocks;
pub mod clv_projector;
pub mod config;
pub mod dynamic_chunker;
pub mod error;
pub mod rebalancer;
pub mod router;
pub mod types;

// === 关键类型重导出,简化外部导入 ===
pub use blocks::BlockBuilder;
pub use clv_projector::{ClvProjector, ProjectionMethod};
pub use config::KvbsrConfig;
pub use dynamic_chunker::DynamicChunker;
pub use error::KvbsrError;
pub use rebalancer::Rebalancer;
pub use router::KVBlockSemanticRouter;
pub use types::{
    CoOccurrenceMatrix, RoutingRequest, RoutingResult, SemanticBlock, ToolId, ToolVector,
};

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::blocks::BlockBuilder;
    pub use crate::clv_projector::{ClvProjector, ProjectionMethod};
    pub use crate::config::KvbsrConfig;
    pub use crate::dynamic_chunker::DynamicChunker;
    pub use crate::error::KvbsrError;
    pub use crate::rebalancer::Rebalancer;
    pub use crate::router::KVBlockSemanticRouter;
    pub use crate::types::{
        CoOccurrenceMatrix, RoutingRequest, RoutingResult, SemanticBlock, ToolId, ToolVector,
    };
}
