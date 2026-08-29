//! 分层上下文窗口 - 4K/32K/128K/1M 四级上下文窗口管理
//!
//! 对应架构层:L2 Memory
//! 对应创新点:HCW(Hierarchical Context Window,分层上下文窗口)
//!
//! # 核心职责
//! - 按 `complexity` 自动选择窗口层级(L0=4K/L1=32K/L2=128K/L3=1M 等效)
//! - 窗口溢出时自动升级 tier(L0->L1->L2->L3 降级链,可逆)
//! - 应用 OSA context_mask 稀疏化(仅加载活跃文件上下文)
//! - 基于重要性评分压缩上下文(0.4*时近性 + 0.3*频次 + 0.3*任务相关性)
//! - 发布 `ContextWindowSwitched`/`ContextCompressed` 事件
//! - 订阅 `OmniSparseMasksComputed` 事件(修正 V1 违规:不直接 import OSA)
//!
//! # V1 违规修正
//! 原架构:OSA(L6)直接 import HCW(L2) -> 向上依赖违规
//! 修正后:OSA 发布 `OmniSparseMasksComputed` 事件,HCW 订阅消费,
//! HCW 不持有 OSA 的引用,仅通过 EventBus 接收掩码信息(依赖铁律)
//!
//! # 1M 等效实现(架构红线)
//! L3 的 1M 等效通过"分层 + 稀疏化"实现,而非暴力加载:
//! - 实际加载容量 = l3_capacity / 8 = 128K
//! - 通过 OSA 稀疏化(8x 压缩比)跳过 87.5% 内容
//! - 实现 1M 等效,避免内存爆炸(架构红线:禁止 1M 暴力加载)
//!
//! # 快速示例
//! ```no_run
//! use hcw_window::{HcwWindow, HcwConfig, ContextEntry, WindowTier};
//! use event_bus::EventBus;
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let bus = EventBus::new();
//! let window = HcwWindow::with_default_config(bus)?;
//!
//! let entry = ContextEntry::new("e-1", "file-1", "content", 100);
//! window.insert(entry).await?;
//!
//! let tier = window.select_window(0.6).await?; // 选择 L2 窗口
//! assert_eq!(tier, WindowTier::L2);
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

// === 模块声明 ===
/// MCA P5 窗口亲和折减 — HCW 分层窗口按模型实际上限折减(ADR-065/066)
pub mod affinity;
pub mod compressor;
pub mod config;
/// P4-W13.2.2: 密度学习器持有器 — S1 接缝策略异步下发 + 本地 fallback（C4 合规）
pub mod density_learner;
pub mod error;
/// Phase 2 §7.4: HiLS-Attention 分层稀疏注意力（chunk-mass surrogate + 两级 softmax，ADR-049 内嵌）
pub mod hils;
/// P1-T14: 压缩评分 ComputeBridge 段间并行注入(env CHIMERA_NO_PARALLEL_HCW 回退)
pub mod parallel;
/// P2-T4: CSC 四级渐进压缩链 + from 模式保前缀 + 分组截断重试(ADR-119/v4.0 WI-12)
pub mod pipeline;
/// P2-T4: ThinkingPreserve 推理痕迹保留(T-02/Ω₉,压缩全程不触碰 thinking 块)
pub mod preserve;
/// PROBE P1.2: 查询探针打分（ProbeWeights / score_with_probe / mix_probe / probe_health）
pub mod probe;
pub mod recall;
pub mod selector;
/// P4-W13.3.2: 选择器学习器持有器 — S4 接缝策略异步下发 + 本地 fallback（C4 合规）
pub mod selector_learner;
/// P2-T4: SharedSemanticIndex 跨层共享语义索引(GLM IndexShare 迁移,符号/决策/错误三类)
pub mod semantic_index;
pub mod types;
pub mod window;

// === 关键类型重导出,简化外部导入 ===
pub use affinity::{FoldResult, WindowAffinity};
pub use compressor::ContextCompressor;
pub use density_learner::DensityLearnerHolder;
pub use error::HcwError;
// Phase 2 §7.4: HiLS-Attention 分层稀疏注意力（HCW 集成接口）
pub use hils::{AttentionOutput, Chunk, HiLSAttention, HiLSWindowSelector};
// PROBE P1.2/P1.6: 查询探针打分 + 增量重打分缓存公开 API
pub use probe::{mix_probe, probe_health, score_with_probe, ProbeHealth, ProbeWeights, ScoreCache};
pub use selector::WindowSelector;
// P4-W13.3.2: S4 接缝（HCW selector 权重系数）学习器持有器
pub use selector_learner::SelectorLearnerHolder;
pub use types::{CompressionReport, ContextEntry, HcwConfig, HcwState, WindowTier};
pub use window::HcwWindow;

/// 预导入模块 - 提供最常用类型
///
/// 使用方式:`use hcw_window::prelude::*;`
pub mod prelude {
    pub use crate::compressor::ContextCompressor;
    // P4-W13.2.2: 密度学习器持有器（S1 接缝异步策略下发 + 本地 fallback）
    pub use crate::density_learner::DensityLearnerHolder;
    pub use crate::error::HcwError;
    // Phase 2 §7.4: HiLS-Attention 分层稀疏注意力（与顶层导出同集）
    pub use crate::hils::{AttentionOutput, Chunk, HiLSAttention, HiLSWindowSelector};
    pub use crate::recall::coarse::{CoarseRecall, CoarseRecallBuilder};
    pub use crate::recall::fine::{FineRecall, FineRecallInput};
    pub use crate::recall::rerank::{
        RerankFill, RerankFillConfig, RerankFillInput, RerankFillOutput, SparseAttentionPattern,
        WindowBudget,
    };
    pub use crate::recall::streaming::{
        StreamingFill, StreamingFillConfig, StreamingFillInput, StreamingFillOutput, StreamingMode,
    };
    pub use crate::recall::types::{
        BlockId, BlockScore, CoChangeMatrix, CoarseRecallInput, CoarseRecallOutput,
        FineRecallConfig, FineRecallOutput, ModuleGraph, ModuleId, ModuleScore, RecallError,
        RecallWeights,
    };
    pub use crate::selector::WindowSelector;
    // P4-W13.3.2: 选择器学习器持有器（S4 接缝异步策略下发 + 本地 fallback）
    pub use crate::selector_learner::SelectorLearnerHolder;
    pub use crate::types::{CompressionReport, ContextEntry, HcwConfig, HcwState, WindowTier};
    pub use crate::window::HcwWindow;
}
