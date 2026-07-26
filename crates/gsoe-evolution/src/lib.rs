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

/// P5.2.1: RHI-CG 通道 B CI 执行门(CiGate trait + CargoCiGate + MockCiGate)
///
/// 对应 ADR: ADR-044 决策 5(CI 执行门接口设计)
pub mod ci_gate;
pub mod config;
pub mod engine;
pub mod error;
pub mod policy;
/// P5.2.2: RHI-CG 通道 B 显著性检测(单尾二项检验 + SignificanceDetector)
///
/// 对应 ADR: ADR-044 决策 6(显著性检测算法选型)+ ADR-045 决策 8(否决证据检查独立化)
pub mod significance;
/// P4-W15.1.2: HarnessSpec 加载器（TOML 反序列化 + 字段校验 + 不可进化面检查）
pub mod spec_loader;
/// P4-W15.2.1: HarnessSpec 版本化注册表（谱系追踪 + A/B 测试 + 一键回滚 + 不可进化面守护）
pub mod spec_registry;
pub mod types;

// === 关键类型重导出,简化外部导入 ===
// P5.2.1: CiGate 公开 API 重导出
pub use ci_gate::{
    check_inv9_delegation_acyclic, CargoCiGate, CiFailure, CiFailureKind, CiGate, CiGateError,
    CiGateResult, DelegationEdge, MockCiGate,
};
pub use config::GsoeConfig;
pub use engine::GsoeEvolutionEngine;
pub use error::GsoeError;
pub use policy::fitness::{evaluate_fitness, evaluate_population};
pub use policy::grpo::{compute_advantage, sample_rollouts};
pub use policy::mutation::{apply_mutation, mutate};
// P5.2.2: SignificanceDetector 公开 API 重导出
pub use significance::{
    check_veto_evidence, SignificanceDetector, NULL_HYPOTHESIS_P, SIGNIFICANCE_THRESHOLD,
    VETO_STREAK_THRESHOLD,
};
// P4-W15.1.2: SpecLoader 公开 API 重导出
pub use spec_loader::{SpecLoader, SpecLoaderError};
// P4-W15.2.1: SpecRegistry 公开 API 重导出
pub use spec_registry::{SpecRegistry, SpecRegistryError};
pub use types::{
    EvolutionPolicy, EvolutionResult, FitnessReport, GrpoRollout, MutationCandidate, MutationType,
};

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    // P5.2.1: CiGate 加入 prelude
    pub use crate::ci_gate::{
        CargoCiGate, CiGate, CiGateError, CiGateResult, DelegationEdge, MockCiGate,
    };
    pub use crate::config::GsoeConfig;
    pub use crate::engine::GsoeEvolutionEngine;
    pub use crate::error::GsoeError;
    // P5.2.2: SignificanceDetector 加入 prelude
    pub use crate::significance::{
        check_veto_evidence, SignificanceDetector, SIGNIFICANCE_THRESHOLD, VETO_STREAK_THRESHOLD,
    };
    // P4-W15.1.2: SpecLoader 加入 prelude
    pub use crate::spec_loader::{SpecLoader, SpecLoaderError};
    // P4-W15.2.1: SpecRegistry 加入 prelude
    pub use crate::spec_registry::{SpecRegistry, SpecRegistryError};
    pub use crate::types::{
        EvolutionPolicy, EvolutionResult, FitnessReport, GrpoRollout, MutationCandidate,
        MutationType,
    };
}
