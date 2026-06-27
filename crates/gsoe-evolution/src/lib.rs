//! 在线进化引擎 — GRPO 风格的引导式自组织在线进化
//!
//! 对应架构层:L5 Knowledge
//! 对应创新点:GSOE(Guided Self-Organizing Evolution)
//!
//! 设计来源:DeepSeek V4 GRPO + ADR-025
//!
//! # 核心机制
//! GRPO 风格的在线强化学习,基于议会共识与红队审计生成策略更新。
//! 订阅 `ConsensusReached`(议会共识,作为进化奖励)与 `RedTeamAudit`
//! (红队审计,作为对抗进化信号),驱动策略参数的变异与选择。
//!
//! # 快速示例
//! ```no_run
//! use gsoe_evolution::{GsoeEvolutionEngine, GsoeConfig};
//!
//! # async fn run() -> anyhow::Result<()> {
//! let mut engine = GsoeEvolutionEngine::new(GsoeConfig::default());
//! let result = engine.evolve_once().await?;
//! println!("世代 {} 改进 {:.4}", result.generation, result.improvement);
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

pub mod config;
pub mod engine;
pub mod error;
pub mod policy;
pub mod types;

// === 关键类型重导出,简化外部导入 ===
pub use config::GsoeConfig;
pub use engine::GsoeEvolutionEngine;
pub use error::GsoeError;
pub use policy::fitness::{evaluate_fitness, evaluate_population};
pub use policy::grpo::{compute_advantage, sample_rollouts};
pub use policy::mutation::{apply_mutation, mutate};
pub use types::{
    EvolutionPolicy, EvolutionResult, FitnessReport, GrpoRollout, MutationCandidate, MutationType,
};

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::config::GsoeConfig;
    pub use crate::engine::GsoeEvolutionEngine;
    pub use crate::error::GsoeError;
    pub use crate::types::{
        EvolutionPolicy, EvolutionResult, FitnessReport, GrpoRollout, MutationCandidate,
        MutationType,
    };
}
