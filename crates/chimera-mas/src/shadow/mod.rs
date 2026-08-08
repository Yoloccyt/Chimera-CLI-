//! 影子模式子系统 — R2 解冻阶段③ 的编排层(ADR-053 备忘录 §五 B-4/B-5)
//!
//! 对应架构层: L9 Quest(chimera-mas 子模块)
//! 对应 ADR: ADR-053-rev4(权威版,阶段② 治理签署 2026-07-29)
//!
//! # 模块分工(条款 ↔ 模块一一对应,可审计)
//!
//! | 模块 | 职责 | 对应 ADR 条款 |
//! |------|------|--------------|
//! | [`config`] | 治理签署 + 参数校验(fail-closed 构造) | rev4 s_min 签署档 / rev3 3A″-P |
//! | [`stats`] | Wilson / 游程哨兵 / bootstrap 纯函数 | rev4 3A′-P2(单调 fail-closed) |
//! | [`evidence_gate`] | 资格层硬门 + 排名层加权 | rev4 3A″-P2 / rev3 3C-P |
//! | [`batch`] | 批次账本 + 检查点门控 | rev3 3A′-P(防 optional stopping) |
//! | [`orchestrator`] | 熔断/范围/证据/胜率组合裁决 + AHIRT 接线 | 备忘录 §五 B-4/B-5 |
//!
//! # R2 冻结合规(WHY 本子系统不违反冻结)
//!
//! 全子系统零 RL 训练路径、零被禁标识符;编排器无治理签署配置不可
//! 实例化,终判仅产出建议——**实现 ≠ 解冻**,真正解冻须阶段③ 实跑
//! 满门 + ADR-054 三方复核 + 用户治理签署(详见 orchestrator 模块文档)。

pub mod batch;
pub mod config;
pub mod evidence_gate;
pub mod orchestrator;
// Milestone C-5 前置: R2 解冻就绪检查（ADR-053 rev4 阶段③ 自动化载体）
pub mod readiness;
pub mod stats;

// === 关键类型重导出(适度导出:裁决入口 + 证据类型 + 审计分解) ===
pub use batch::{BatchRecord, Checkpoint, BASE_BATCHES, EXTENDED_BATCHES, EXTENSION_BAND};
pub use config::{EvidenceWeights, GovernanceSignoff, ShadowModeConfig};
pub use evidence_gate::{
    AhirtBatchEvidence, AhirtCategoryStats, BatchEvidence, BatchVerdict, DimensionScores,
    EvidenceGate,
};
pub use orchestrator::{
    AhirtEvidenceCollector, PromotionAdvice, ShadowModeOrchestrator, Stage3Prerequisites,
};
pub use stats::{
    effective_lower_bound, moving_block_bootstrap_lower, runs_sentinel_one_sided,
    wilson_lower_bound, EffectiveLowerBound, SentinelVerdict,
};
