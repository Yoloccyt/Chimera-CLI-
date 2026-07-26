//! HCW-Sparse v2.0 召回流水线 — 三级召回（粗召回 / 精排 / 重排填充）
//!
//! 对应架构层: L2 Memory
//! 对应任务: P3-W9.1 ~ P3-W10.1（spec.md §P3 内环升级）
//! 对应病理修复: D1（HCW selector 权重手写、OSA 静态掩码无学习机制）
//!
//! # 召回流水线（继承 HCW-Sparse v2.0）
//! 1. **粗召回**（<10ms）：Project 图联合传播（依赖 40% + 语义 30% + 共变更 30%）→ 100 模块
//! 2. **精排**（<50ms）：HNSW + 精确 CLV 重排 → 500 Block（P3-W9.2）
//! 3. **重排填充**（<100ms）：多目标密度贪心 → 1M 等效窗口（P3-W10.1）
//! 4. **增量流式**（P3-W10.2）：Top-10% 关键块同步加载即放行，剩余后台补满
//!
//! # 设计动机（D1 修复）
//! v5.0 设计文档 §2.1 D1 病理：「HCW selector 权重手写（`score = w1·recency + w2·frequency + w3·relevance`）、
//! OSA 五维掩码静态、scc 一阶马尔可夫、decay 参数固定，无任何学习机制」。
//!
//! HCW-Sparse v2.0 通过引入"Project 图联合传播"将静态规则替换为基于图结构 + 语义 + 共变更历史的
//! 多信号融合召回，为后续 P3-W10.3 selector 权重外置（SelectorPolicy）奠基。
//!
//! # 模块组织
//! - `coarse`: 粗召回实现（Project 图联合传播 → 100 模块）
//! - `fine`: 精排实现（HNSW + 精确 CLV 重排 → 500 Block）
//! - `rerank`: 重排填充实现（多目标密度贪心 → 1M 等效窗口 + 二次稀疏）
//! - `streaming`: 增量流式实现（Top-10% 关键块同步放行,剩余后台补满）
//! - `types`: 召回流水线共享类型

pub mod coarse;
pub mod fine;
pub mod rerank;
pub mod streaming;
pub mod types;

// === 公开类型重导出（简化外部导入）===
pub use coarse::{CoarseRecall, CoarseRecallBuilder};
pub use fine::{FineRecall, FineRecallInput};
pub use rerank::{
    RerankFill, RerankFillConfig, RerankFillInput, RerankFillOutput, SparseAttentionPattern,
    WindowBudget, DEFAULT_BLOCK_TOKENS,
};
pub use streaming::{
    StreamingFill, StreamingFillConfig, StreamingFillInput, StreamingFillOutput, StreamingMode,
    DEEP_CRITICAL_RATIO, DEEP_FIRST_TOKEN_TARGET_MS, FAST_CRITICAL_RATIO,
    FAST_FIRST_TOKEN_TARGET_MS,
};
pub use types::{
    BlockId, BlockScore, CoChangeMatrix, CoarseRecallInput, CoarseRecallOutput, FineRecallConfig,
    FineRecallOutput, ModuleGraph, ModuleId, ModuleScore, RecallError, RecallWeights,
};

/// 预导入模块 — 提供最常用类型
pub mod prelude {
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
}
