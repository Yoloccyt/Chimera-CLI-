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
/// Phase 2 §7.2: 双层经验库（MemoHarness 案例级→全局蒸馏，ADR-049 内嵌）
pub mod dual_experience_bank;
pub mod engine;
pub mod error;
/// Phase 2 §7.1: 经验卡片系统（OpenMLE 案例级 + 全局板 + 三因子父本选择，ADR-049 内嵌）
pub mod experience_card_system;
pub mod l0_working;
pub mod l1_episodic;
pub mod l2_semantic;
pub mod l3_procedural;
/// P2-8 MemCon 自适应控制器 — 幽灵记忆检测与策略自适应调整
pub mod mem_con;
/// polish-v2.7 P4-3:记忆图谱(语义边 + 共现边,Top-K 近邻建边,ADR-049)
pub mod memory_graph;
/// 记忆策略学习器持有器 — S2 接缝策略异步下发 + 本地 fallback（P4-W14.1）
pub mod memory_strategy_learner;
/// Phase 2 §7.1: 按需记忆合成（OpenMLE 懒加载祖先/兄弟 + 算子差异化上下文）
pub mod on_demand_synthesizer;
/// Phase 2 §7.3: 记忆金字塔（MSCE + TencentDB 四层 + 检索三方式 + 注入策略 + 降级链）
pub mod pyramid;
/// P2-T6: RSB 跨轮事件残留系统（手册 T-09 + v4.0 WI-20,三层缓冲 + 相位门控）
///
/// 50+ 轮后早期决策召回保持（与深层 Transformer 梯度消失同构的残差解法）;
/// 注入公式 context' = context + α·residual,门控矩阵 Exploration[0.8,0.6,0.4] /
/// Execution[0.3,0.2,0.1] / Debugging[0.9,0.7,0.5] / Planning[0.5,0.8,0.9]。
pub mod residual;
/// polish-v2.7 P4-4:记忆 Sideagent 二次验证(四检查项加权,幽灵记忆防线,ADR-049)
pub mod sideagent;
pub mod storage_impl;
pub mod types;
/// Phase 2 §7.5: MSCE 双信号价值回填（Vt = αt·Rt + (1−αt)·γ·Vt+1）
pub mod value_backfill;

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
// P2-8: MemCon 自适应控制器
pub use mem_con::{MemConConfig, MemConController};
pub use storage_impl::PragmaConn;
pub use types::{
    assert_archive_monotonicity, ExecutionStats, MemoryEntry, MemoryId, MemoryTier,
    PatternSignature, ProceduralEntry, QuestId, SharedCLV,
};

// L2-P1-1 事件驱动化: 原 Task 3.2 的进程级全局快照(GLOBAL_MEMORY_STAGE /
// current_memory_stage / set_current_memory_stage / MemoryStage 别名)已移除。
// 记忆策略阶段的可观测传播改走 Ω₄-Event:策略变更(update_memory_strategy_policy /
// fallback_memory_strategy_to_static)发布 MemConStrategyAdjusted 事件,
// 消费方(TUI SelfAssessmentPanel)从 latest_events 事件流过滤派生展示,
// 消除跨层隐式全局状态与测试间竞态(原 TUI 测试 L11-14 自证偶发失败)。
pub use nexus_contracts::MemoryStrategy;
pub use nexus_contracts::MemoryStrategyPolicy;

// Phase 2 L2 记忆层五大组件重导出（§7.1-7.5）
pub use dual_experience_bank::{
    CaseExperience, DualExperienceBank, FailurePattern, GlobalExperience, RetrievedExperiences,
    StrategyRecord, SuccessPattern, TaskQuery,
};
pub use experience_card_system::{ExperienceCardSystem, GlobalExperienceBoard, MethodStatistics};
pub use on_demand_synthesizer::{OnDemandSynthesizer, SynthesizedMemory};
pub use pyramid::{
    DegradationChain, HybridRanker, LiteralSearcher, MemoryPyramid, PyramidL1Entry, RawLogEntry,
    RetrievalResult, RetrievalSource, RetrieveStrategy, SemanticSearcher,
};
pub use value_backfill::{DualSignalBackfill, L1Trace, ReflectionScorer};

/// 预导入模块  提供最常用类型
pub mod prelude {
    pub use crate::config::MlcConfig;
    pub use crate::engine::MlcEngine;
    pub use crate::error::MlcError;
    // P4-W14.1: S2 接缝记忆策略学习器持有器
    pub use crate::memory_strategy_learner::MemoryStrategyLearnerHolder;
    // P2-8: MemCon 自适应控制器
    pub use crate::mem_con::{MemConConfig, MemConController};
    pub use crate::types::{
        assert_archive_monotonicity, ExecutionStats, MemoryEntry, MemoryId, MemoryTier,
        PatternSignature, ProceduralEntry, QuestId,
    };
    // Phase 2 L2 记忆层五大组件（§7.1-7.5，与顶层导出同集）
    pub use crate::dual_experience_bank::{
        CaseExperience, DualExperienceBank, FailurePattern, GlobalExperience, RetrievedExperiences,
        StrategyRecord, SuccessPattern, TaskQuery,
    };
    pub use crate::experience_card_system::{
        ExperienceCardSystem, GlobalExperienceBoard, MethodStatistics,
    };
    pub use crate::on_demand_synthesizer::{OnDemandSynthesizer, SynthesizedMemory};
    pub use crate::pyramid::{
        DegradationChain, HybridRanker, LiteralSearcher, MemoryPyramid, PyramidL1Entry,
        RawLogEntry, RetrievalResult, RetrievalSource, RetrieveStrategy, SemanticSearcher,
    };
    pub use crate::value_backfill::{DualSignalBackfill, L1Trace, ReflectionScorer};
}
