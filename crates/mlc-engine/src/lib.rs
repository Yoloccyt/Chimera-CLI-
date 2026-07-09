//! 四级潜在记忆引擎  L0-L3 神经形态记忆分级存储与检索
//!
//! 对应架构层:L2 Memory
//! 对应创新点:MLC(Multi-Level Context,四级神经形态记忆)
//!
//! # 核心职责
//! - 实现 L0 WorkingMemory(DashMap + LRU,容量 64,延迟 < 1μs)
//! - 实现 L1 EpisodicMemory(BTreeMap 时间索引 + HashMap Quest 索引,容量 1024)
//! - 实现 L2 SemanticMemory(Vec + 线性扫描 KNN,容量 4096,Top-10 召回 < 5ms)
//! - 实现 L3 ProceduralMemory(SQLite 持久化,模式签名匹配)
//! - 通过 MlcEngine 统一接口聚合 L0-L3,自动路由与层级迁移
//! - 集成 EventBus,发布 MemoryMetricsReported/MemoryTiered 事件
//!
//! # 架构红线
//! - 所有跨层通信走 EventBus(2.2 依赖铁律)
//! - 单函数  200 行,禁止 unwrap()/expect()
//! - DashMap 写锁释放后再调用 async 方法(避免死锁)
//! - 函数参数/返回值类型严格控制
//! # 快速示例
//! ```no_run
//! use mlc_engine::{MlcEngine, MlcConfig, MemoryEntry, MemoryTier};
//! use event_bus::EventBus;
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let bus = EventBus::new();
//! let engine = MlcEngine::with_default_config(bus)?;
//!
//! let entry = MemoryEntry::new("m-1", "内容", MemoryTier::L0Working);
//! engine.store(entry).await?;
//!
//! let recalled = engine.recall("m-1").await?;
//! assert!(recalled.is_some());
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

// === 模块声明 ===
pub mod config;
pub mod engine;
pub mod error;
pub mod l0_working;
pub mod l1_episodic;
pub mod l2_semantic;
pub mod l3_procedural;
pub mod storage_impl;
pub mod types;

// === 关键类型重导出,简化外部导入 ===
pub use config::MlcConfig;
pub use engine::MlcEngine;
pub use error::MlcError;
pub use l0_working::WorkingMemory;
pub use l1_episodic::EpisodicMemory;
pub use l2_semantic::SemanticMemory;
pub use l3_procedural::ProceduralMemory;
pub use storage_impl::PragmaConn;
pub use types::{
    ExecutionStats, MemoryEntry, MemoryId, MemoryTier, PatternSignature, ProceduralEntry, QuestId,
    SharedCLV,
};

/// 预导入模块  提供最常用类型
pub mod prelude {
    pub use crate::config::MlcConfig;
    pub use crate::engine::MlcEngine;
    pub use crate::error::MlcError;
    pub use crate::types::{
        ExecutionStats, MemoryEntry, MemoryId, MemoryTier, PatternSignature, ProceduralEntry,
        QuestId,
    };
}
