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
//! # 文档-代码偏差记录(《最新版》§10 对齐,P1-3 深度优化)
//! - **AEGIS 落点偏差**:文档 §10.3.1/§20 规划独立 `aegis-engine` crate,
//!   实际落地为本 crate 的 `aegis` 子模块(AEGIS-lite 降级设计,
//!   ADR-050 + ADR-049 决策 1 否决新建 crate)。
//! - **变体隔离落点偏差**:文档 §10.3.2 规划 L5 `variant-pool` crate,
//!   实际落地 L8 parliament(与文档 §13 L8 变体审议一致),L5 侧经
//!   L0 VariantId 共享类型协作。
//! - **formal/ 跨层职责**:`formal` 模块标注 L4 FormalVerifier 职责
//!   (文档 §19 L5 防御 = AEGIS Critic 奖励欺骗检测),形式化验证器
//!   按 ADR-047/052 裁决分布于本 crate 与 parliament/omega-learner/
//!   decay-engine 四 crate,属历史裁决,记录而非重构。
//! - **P1-3 增量**(2026-08):`spec_dag_snapshot` 接入 SpecRegistry 真实谱系
//!   (原空快照 TODO 关闭);`checkpoint_preserver`(文档 §10.2 问题 3,
//!   RSIBench 保留历史最佳)与 `self_improvement`(文档 §10.2 问题 5,
//!   PenguinHarness 四步降级实现)均已落地。
//!
//! # 快速示例
//! ```no_run
//! use gsoe_evolution::{GsoeEvolutionEngine, GsoeConfig};
//!
//! # async fn run() -> Result<(), Box<dyn std::error::Error>> {
//! let mut engine = GsoeEvolutionEngine::new(GsoeConfig::default());
//! let result = engine.evolve_once().await?;
//! println!("世代 {} 改进 {:.4}", result.generation, result.improvement);
//! # Ok(())
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

/// polish-v2.7 Phase 2: AEGIS-lite 四阶段进化流水线(Digester→Planner→Evolver→Critic)
///
/// 对应 ADR: ADR-050(AEGIS-lite 降级设计)+ ADR-049 决策 1(落点裁决)
/// R2 冻结声明(ADR-042):规则/统计驱动,无 RL 参数更新
pub mod aegis;
/// P1-3(计划 Task 4): CheckpointPreserver 保留历史最佳 checkpoint(RSIBench,文档 §10.3.3)
///
/// 纯逻辑模块(无 IO/无事件/无全局状态),按 task_type 隔离历史最佳,
/// 停止策略 attempts > 10 且有最佳时建议停止(RSIBench 78.26% 发现)。
pub mod checkpoint_preserver;
/// P5.2.1: RHI-CG 通道 B CI 执行门(CiGate trait + CargoCiGate + MockCiGate)
///
/// 对应 ADR: ADR-044 决策 5(CI 执行门接口设计)
pub mod ci_gate;
pub mod config;
pub mod engine;
/// P3-T13b: EPTS 快照沙箱评测流水线（v4.0 WI-31:Extractor→Generator→Judge）
pub mod epts;
pub mod error;
/// P2-T13: 变体适应度批量评估并行化（v4.0 注入表 W13-14,Shadow 限定）
///
/// ComputeBridge rayon 并行（GsoeEvaluate 阈值已登记）;串行回退 env 开关;
/// R2 约束:仅加速评估计算,不改变策略写入路径（转正须议会审批 ADR-142）。
pub mod fitness_parallel;
/// 形式化验证模块 — AEGIS Critic 单调性等不变量的形式化保证
///
/// 对应架构层: L4 FormalVerifier
pub mod formal;
/// R2 解冻阶段③ 前置 2 — 形式化验证器 CI 门禁化(ADR-052 待办 2)
///
/// 聚合 7 个验证器的 VerificationResult 为 CiGateResult,复用既有 CI 门禁基础设施。
/// 消费切片(不持有验证器实例)以规避 L5→L6 向上依赖;不含 R2 扫描关键词。
pub mod formal_gate;
/// Phase 5 §10.1: 四套原子算子（OpenMLE Draft/Improve/Debug/Crossover，ADR-049 内嵌）
pub mod four_operators;
/// polish-v2.7 closure Stage B-8: Meta-Agent 适配器(外部 Harness 描述规范化,ADR-049 降级档)
///
/// 规则式规范化:外部描述 → HarnessSpec TOML → SpecLoader 全量校验(强制门自动注入)。
/// R2 冻结声明(ADR-042):纯文本规范化,无学习/训练路径。
pub mod meta_adapter;
pub mod policy;
/// P2-T10: RTL 运行时策略复盘 Shadow（v4.0 WI-30,R2 限定）
///
/// 零 Python/零梯度/零权重更新;Shadow 只读(转正须议会审批 ADR-142);
/// 可验证奖励纯 Rust;反馈仅写影子表 + 周度报告供议会审阅。
pub mod rtl_shadow;
/// P1-3(计划 Task 6): 自我改进流水线(PenguinHarness 四步,文档 §10.3.4)
///
/// 降级实现(ADR-042 合规):四步语义保留但规则/统计驱动,
/// Optimizer 复用 AEGIS-lite 四阶段编排。
pub mod self_improvement;
/// P5.2.2: RHI-CG 通道 B 显著性检测(单尾二项检验 + SignificanceDetector)
///
/// 对应 ADR: ADR-044 决策 6(显著性检测算法选型)+ ADR-045 决策 8(否决证据检查独立化)
pub mod significance;
/// P4-W15.1.2: HarnessSpec 加载器（TOML 反序列化 + 字段校验 + 不可进化面检查）
pub mod spec_loader;
/// P4-W15.2.1: HarnessSpec 版本化注册表（谱系追踪 + A/B 测试 + 一键回滚 + 不可进化面守护）
pub mod spec_registry;
/// Phase 5 §10.2: 三因子父本选择器（UCB + Softmax + 冷却，OpenMLE，ADR-049 内嵌）
pub mod three_factor_selector;
pub mod types;
/// R2 解冻统一决策闸门 — 4 项前置的组合封套(capstone)
///
/// 组合范围(WHAT,UnfreezeScope)与验证(WHETHER,调用方传入)为单一 fail-closed
/// 决策入口,Denied 指明失败维度;审计计数。决策闸门 ≠ 解冻,不含 R2 扫描关键词。
pub mod unfreeze_governor;
/// R2 解冻阶段③ 前置 4 — 解冻范围界定守卫(ADR-052 待办 4)
///
/// fail-closed 白名单:未显式纳入范围的 RL 更新目标一律拒绝(默认全冻结)。
/// 把"解冻范围"从文档承诺升级为运行时可强制。纯策略判定,不含 R2 扫描关键词。
pub mod unfreeze_scope;

// === 关键类型重导出,简化外部导入 ===
// polish-v2.7 Phase 2: AEGIS-lite 公开 API 重导出
pub use aegis::{
    AdaptationDirection, AdaptationPlan, AdaptationPlanner, AegisCritic, AegisPipeline,
    CriticVerdict, DigestedTrajectories, FailurePattern, RejectedCandidate, SpecCandidate,
    SpecEvolver, TrajectoryDigester, TrajectoryOutcome,
};
// P1-3(计划 Task 4): CheckpointPreserver 公开 API 重导出
pub use checkpoint_preserver::{
    Checkpoint, CheckpointPreserver, PreserveDecision, StopDecision, MAX_ATTEMPTS_BEFORE_STOP,
};
// P5.2.1: CiGate 公开 API 重导出
pub use ci_gate::{
    check_inv9_delegation_acyclic, CargoCiGate, CiFailure, CiFailureKind, CiGate, CiGateError,
    CiGateResult, DelegationEdge, MockCiGate,
};
// R2 解冻阶段③ 前置 2:FormalVerifierGate 公开 API 重导出
pub use config::GsoeConfig;
pub use engine::GsoeEvolutionEngine;
pub use error::GsoeError;
pub use formal_gate::{
    FormalGateSummary, FormalVerifierGate, NamedPropertyResult, DEFAULT_MIN_SATISFIED,
};
// R2 解冻阶段③ 前置 4:UnfreezeScope 公开 API 重导出
pub use unfreeze_scope::{RlUpdateTarget, ScopeVerdict, UnfreezeScope};
// P1-3(计划 Task 6): 自我改进流水线公开 API 重导出
pub use self_improvement::{
    AgentCreator, AgentEvaluator, Benchmark, BenchmarkDesigner, BenchmarkScenario,
    ImprovementRequirements, ImprovementResult, RuleBasedEvaluator, RuleBenchmarkDesigner,
    SelfImprovementPipeline, TemplateCreator,
};
// R2 解冻统一决策闸门:UnfreezeGovernor 公开 API 重导出
pub use unfreeze_governor::{DenialDimension, UnfreezeDecision, UnfreezeGovernor};
// polish-v2.7 closure Stage B-8: Meta-Agent 适配器公开 API 重导出
pub use meta_adapter::{
    ExternalHarnessDescriptor, ExternalStep, MetaAdapterError, MetaAgentAdapter,
};
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
// Phase 5 §10.1: 四套原子算子公开 API 重导出
pub use four_operators::{
    AtomicOperatorTrait, CardQuery, CrossoverOperator, DebugOperator, DraftOperator,
    ImproveOperator, OperatorContext, OperatorError, OperatorResult, ResourceCost,
};
// Phase 5 §10.2: 三因子父本选择器公开 API 重导出
pub use three_factor_selector::ThreeFactorSelector;
pub use types::{
    EvolutionPolicy, EvolutionResult, FitnessReport, GrpoRollout, MutationCandidate, MutationType,
};

// === Task 3.5: L10 TUI 跨层协同 — 谱系 DAG 快照 ===

/// 谱系节点 — Spec DAG 中的单个规范版本节点(Task 3.5)
///
/// WHY 独立 struct 而非复用 SpecRegistry: SpecRegistry 是运行时变异结构,
/// TUI 面板只需只读快照,用简单 struct 避免持有锁或引用复杂状态。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecNode {
    /// 节点唯一标识(如 spec id)
    pub id: String,
    /// 版本号(单调递增)
    pub version: u64,
    /// 状态简述(如 "active"/"deprecated"/"experimental")
    pub status: String,
}

/// 谱系边 — Spec DAG 中的有向关系边(Task 3.5)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpecEdge {
    /// 源节点 id
    pub from: String,
    /// 目标节点 id
    pub to: String,
    /// 关系类型(如 "evolves"/"deprecates"/"forks")
    pub relation: String,
}

/// 谱系 DAG 快照 — 规范版本演化有向无环图(Task 3.5)
///
/// TUI DagVizPanel 调用 `spec_dag_snapshot()` 获取节点/边计数,
/// 展示谱系演化规模。P1-3 起由 SpecRegistry 注册/回滚路径真实更新,
/// 不再返回空快照。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpecDagSnapshot {
    /// 谱系节点列表
    pub nodes: Vec<SpecNode>,
    /// 谱系边列表
    pub edges: Vec<SpecEdge>,
}

/// P1-3:全进程谱系 DAG 快照缓存
///
/// WHY 全局只读缓存:spec_dag_snapshot() 是同步纯渲染路径(TUI dag_viz
/// 面板),无法访问任意 SpecRegistry 实例;SpecRegistry 可能存在多个实例
/// (with_event_bus / new),全局快照是它们的并集(全进程谱系累积)。
///
/// WHY 不用 event-bus:dag_viz 面板消费函数是同步纯渲染路径,SpecRegistered
/// 事件管道改造 TUI 状态成本高于收益;快照为 append-only 只读缓存,
/// 唯一写入点 = SpecRegistry 注册/回滚路径,无隐式可变状态。
static SPEC_DAG: std::sync::LazyLock<std::sync::RwLock<SpecDagSnapshot>> =
    std::sync::LazyLock::new(|| std::sync::RwLock::new(SpecDagSnapshot::default()));

/// 返回谱系 DAG 快照(Task 3.5 跨层 Panel 数据管道;P1-3 接入真实数据)
///
/// TUI DagVizPanel 调用此函数显示"谱系"节点/边计数。
/// 快照由 `SpecRegistry::register` / `rollback` 路径增量更新,
/// 跨多个 SpecRegistry 实例累积(全进程谱系)。
///
/// # 示例
///
/// ```
/// use gsoe_evolution::spec_dag_snapshot;
///
/// let snapshot = spec_dag_snapshot();
/// // 快照为全进程谱系累积:未注册时为 0,注册后为实际节点/边数
/// let _total = snapshot.nodes.len() + snapshot.edges.len();
/// ```
pub fn spec_dag_snapshot() -> SpecDagSnapshot {
    SPEC_DAG
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .clone()
}

/// 重置全局谱系快照 — 仅测试隔离用(生产代码不得调用)
///
/// WHY:全局快照跨测试累积会污染断言,测试需可重置;用 #[doc(hidden)]
/// 标注避免误入公共 API 文档。
#[doc(hidden)]
pub fn reset_spec_dag() {
    *SPEC_DAG
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = SpecDagSnapshot::default();
}

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
// P3-T13b: EPTS 公开 API（WI-31）
pub use epts::{
    EptsPipeline, EptsStatus, JudgeVerdict, SynthesizedTask, TaskExtractor, TaskGenerator,
    TaskJudge, TaskTemplate, JUDGE_PASS_GATE, WEEKLY_TARGET,
};
