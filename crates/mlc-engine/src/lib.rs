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

use std::sync::{OnceLock, RwLock};

// === 模块声明 ===
pub mod config;
pub mod engine;
pub mod error;
pub mod l0_working;
pub mod l1_episodic;
pub mod l2_semantic;
pub mod l3_procedural;
/// polish-v2.7 P4-3:记忆图谱(语义边 + 共现边,Top-K 近邻建边,ADR-049)
pub mod memory_graph;
/// 记忆策略学习器持有器 — S2 接缝策略异步下发 + 本地 fallback（P4-W14.1）
pub mod memory_strategy_learner;
/// polish-v2.7 P4-4:记忆 Sideagent 二次验证(四检查项加权,幽灵记忆防线,ADR-049)
pub mod sideagent;
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
// P4-W14.1: S2 接缝记忆策略学习器持有器
pub use memory_strategy_learner::MemoryStrategyLearnerHolder;
pub use storage_impl::PragmaConn;
pub use types::{
    ExecutionStats, MemoryEntry, MemoryId, MemoryTier, PatternSignature, ProceduralEntry, QuestId,
    SharedCLV,
};

// === Task 3.2: L10 TUI 跨层协同 — 全局记忆策略阶段快照 ===
// WHY 全局快照: TUI 面板(L10)无法直接访问 MlcEngine 实例(实例在 chimera-cli
// orchestrator 中),通过进程级 OnceLock<RwLock> 提供"最近已知策略"快照,
// MlcEngine 内部策略变化时同步更新,TUI 面板读取快照实现实时显示。
// 这与 MemoryStrategyLearnerHolder 的 RwLock 设计一致(§4.4 反模式 #4 已规避:
// 锁内只读 MemoryStrategy 枚举 ~10ns,不跨 await)。
pub use nexus_contracts::MemoryStrategy;
pub use nexus_contracts::MemoryStrategyPolicy;

/// 记忆策略阶段 — MemoryStrategy 的展示层别名(Task 3.2)
///
/// WHY 别名而非新枚举: MemoryStrategy(L0 nexus-contracts)已有 5 个变体
/// (MinimalRecall/StandardTopK/QueryReformulation/AggressivePruning/TimeFocused),
/// 语义与 spec 的"记忆策略阶段"完全一致,重定义会引入类型转换开销与维护负担。
pub type MemoryStage = MemoryStrategy;

/// 全局记忆策略阶段快照(进程级 OnceLock<RwLock>)
///
/// WHY OnceLock: 首次调用惰性初始化为 StandardTopK(fallback),
/// 后续 MlcEngine 策略变化时通过 `set_current_memory_stage` 更新。
/// RwLock 写入极低频(策略切换 < 1 次/分钟),读取高频(TUI 每帧),
/// 读写不对称场景 RwLock 比 Mutex 更优。
static GLOBAL_MEMORY_STAGE: OnceLock<RwLock<MemoryStage>> = OnceLock::new();

/// 返回当前记忆策略阶段(Task 3.2 跨层 Panel 数据管道)
///
/// TUI SelfAssessmentPanel 调用此函数显示"记忆策略阶段"字段。
/// 默认返回 `MemoryStage::StandardTopK`(fallback),MlcEngine 策略
/// 变化时通过 `set_current_memory_stage` 同步更新。
///
/// # 示例
///
/// ```
/// use mlc_engine::current_memory_stage;
/// use nexus_contracts::MemoryStrategy;
///
/// let stage = current_memory_stage();
/// assert!(matches!(stage,
///     MemoryStrategy::MinimalRecall
///     | MemoryStrategy::StandardTopK
///     | MemoryStrategy::QueryReformulation
///     | MemoryStrategy::AggressivePruning
///     | MemoryStrategy::TimeFocused));
/// ```
pub fn current_memory_stage() -> MemoryStage {
    let lock = GLOBAL_MEMORY_STAGE.get_or_init(|| RwLock::new(MemoryStage::StandardTopK));
    // WHY unwrap_or_default: RwLock 读锁在正常情况下不会中毒(写锁不会 panic),
    // 中毒仅发生于持有写锁时线程 panic — 此处读锁失败回退到 StandardTopK 安全值。
    lock.read()
        .map(|guard| *guard)
        .unwrap_or(MemoryStage::StandardTopK)
}

/// 更新全局记忆策略阶段快照(mlc-engine 内部 API)
///
/// WHY pub(crate): 仅限 mlc-engine 内部 MemoryStrategyLearnerHolder
/// 在策略变化时调用,外部 crate 不应直接修改全局快照(避免状态不一致)。
pub(crate) fn set_current_memory_stage(stage: MemoryStage) {
    let lock = GLOBAL_MEMORY_STAGE.get_or_init(|| RwLock::new(MemoryStage::StandardTopK));
    // WHY unwrap_or_else: 写锁中毒意味着另一线程 panic 释放锁,此处覆盖是安全恢复。
    let _ = lock.write().map(|mut guard| *guard = stage);
}

/// 预导入模块  提供最常用类型
pub mod prelude {
    pub use crate::config::MlcConfig;
    pub use crate::engine::MlcEngine;
    pub use crate::error::MlcError;
    // Task 3.2: 全局记忆策略阶段快照(TUI 跨层 Panel 数据管道)
    pub use crate::current_memory_stage;
    pub use crate::MemoryStage;
    // P4-W14.1: S2 接缝记忆策略学习器持有器
    pub use crate::memory_strategy_learner::MemoryStrategyLearnerHolder;
    pub use crate::types::{
        ExecutionStats, MemoryEntry, MemoryId, MemoryTier, PatternSignature, ProceduralEntry,
        QuestId,
    };
}
