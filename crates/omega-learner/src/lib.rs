//! NEXUS-OMEGA L6 学习层 — omega-learner Bandit 六接缝
//!
//! 对应架构层: **L6 Router**（与 OSA/KVBSR/FAAE/SESA 同层）
//! 对应 ADR: **ADR-031**（Harness-as-Spec + omega-learner 边界）
//! 对应设计源: `NEXUS-OMEGA_v5.0_系统性完整设计文档.md` §7.3
//! 对应任务: **P4-W13.1**（新建 omega-learner crate）
//!
//! # 核心职责
//!
//! 承载 v5.0 设计文档 §7.3 定义的"环外学习"层: 通过 LinUCB 上下文线性 bandit
//! 学习六个接缝(S1-S6)的最优策略参数,异步下发给调用方(不在推理关键路径同步调用)。
//!
//! ## 六接缝概览
//!
//! | # | 接缝 | 锚点 | 臂 | 奖励 | 状态 |
//! |---|------|------|-----|------|------|
//! | S1 | DDR/HCW 密度档位 | hcw-window selector | ρ∈{0.5,2,5,10} | 成功率 − 延迟惩罚 | ✅ P4-W13.2 |
//! | S2 | 记忆策略选择 | mlc-engine recall | 5 种策略 | 阶段目标达成率 | ✅ P4-W14.1 |
//! | S3 | SCC 预取 | scc-cache prefetch | 5 种预取策略 | 命中率 − 浪费惩罚 | ✅ P4-W14.2 |
//! | S4 | selector 权重系数 | hcw-window selector w1/w2/w3 | 权重向量 | 后悔率 | ✅ P4-W13.3 |
//! | S5 | Parliament 激活 | parliament Fast Path | 跳过/精简/完整 | 推翻率 × 辩论成本 | ✅ P4-W14.3 |
//! | S6 | 衰减参数 | decay-engine DecayProfile | profile 参数 | 误拦率 vs 漏拦率 | ✅ P4-W14.4 |
//! | S7 | 召回配额（R1） | omega-learner r1_recall_quota | k∈{5,10,20,50,100} | 召回率 − 误杀 − 延迟 | ✅ P4-W16.2.2 |
//! | S8 | Mem-π 记忆决策 | omega-learner s8_mem_pi | Generate/Retrieve/Abstain | 下游达成率 − 噪声惩罚 | ✅ closure-B6（影子期） |
//!
//! # 设计约束(ADR-031)
//!
//! - **学习永不在关键路径**: 调用方本地执行 + 本地 fallback 到 `SelectorPolicy::Static`(C4 合规)
//! - **异步下发策略**: learner 通过 `SelectorPolicy::Learned` 值注入,不通过 event-bus 同步广播
//! - **regret 上界保证**: LinUCB 提供 O(√(T·d·ln(K·T))) regret 上界(Li et al., 2010)
//! - **依赖铁律合规**: L6 → L0(nexus-contracts)/L1(event-bus) 向下依赖,无向上依赖
//!
//! # 算法选择(WHY LinUCB)
//!
//! - **regret 上界可证**: Li et al. (2010) 证明在 `||x|| ≤ 1` 假设下收敛
//! - **实现轻量**: 仅维护每个臂的 d×d 矩阵与 d 向量,ndarray 已在 workspace
//! - **无外部线性代数依赖**: Sherman-Morrison 公式增量更新,避免 BLAS/LAPACK(保持 forbid(unsafe_code) 哲学)
//! - **神经升级路径**: NeuralUCB 列为长期选项(本期不实现,ADR-031 边界)
//!
//! # 示例
//!
//! ## 基础 LinUCB 流程
//!
//! ```
//! use omega_learner::arm::{ArmId, DiscreteArmSet};
//! use omega_learner::context::SeamContext;
//! use omega_learner::linucb::LinUCB;
//! use omega_learner::seam::SeamId;
//!
//! // 1. 构造臂集(S1 接缝: 4 档密度)
//! let arm_set = DiscreteArmSet::new(vec![
//!     ArmId::new("rho=0.5"),
//!     ArmId::new("rho=2"),
//!     ArmId::new("rho=5"),
//!     ArmId::new("rho=10"),
//! ]);
//!
//! // 2. 创建 LinUCB(3 维上下文, α=1.0)
//! let mut linucb = LinUCB::new(3, &arm_set, 1.0).unwrap();
//!
//! // 3. 选择臂
//! let ctx = SeamContext::new(vec![0.5, 0.3, 0.2]).unwrap();
//! let arm = linucb.select_arm(&ctx).unwrap();
//!
//! // 4. 观察奖励并更新模型
//! linucb.update(arm, &ctx, 0.85).unwrap();
//! assert_eq!(linucb.total_steps(), 1);
//!
//! // 5. 通过 SeamId 引用接缝(用于日志/诊断)
//! let seam = SeamId::S1Density;
//! assert_eq!(seam.short_name(), "S1-density");
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]
#![doc(html_root_url = "https://docs.rs/omega-learner")]

// ============================================================
// 模块声明
// ============================================================

/// 臂定义 — LinUCB 离散动作空间
pub mod arm;

/// 上下文特征向量 — LinUCB 输入
pub mod context;

/// 错误类型 — 库层 thiserror enum
pub mod error;

/// FormalVerifier M1 — 学习单调性形式化验证（P7-T5,ADR-047 Property #5）
///
/// 验证观测轨迹（步数/奖励/后悔率）而非 LinUCB 内部矩阵状态,
/// 类型复用 `nexus_contracts::formal_props`（L0 契约层）。
pub mod formal;

/// LinUCB 算法核心 — 上下文线性 bandit
pub mod linucb;

/// S1 接缝 — DDR/HCW 密度档位学习器（P4-W13.2）
pub mod s1_density;

/// S2 接缝 — mlc-engine 记忆策略选择学习器（P4-W14.1）
pub mod s2_memory;

/// S3 接缝 — scc-cache 预取策略学习器（P4-W14.2）
pub mod s3_prefetch;

/// S4 接缝 — HCW selector 权重系数学习器（P4-W13.3）
pub mod s4_selector;

/// S5 接缝 — Parliament 激活策略学习器（P4-W14.3）
pub mod s5_parliament;

/// S6 接缝 — decay-engine 衰减参数学习器（P4-W14.4）
pub mod s6_decay;

/// S8 接缝 — Mem-π 记忆决策策略学习器（polish-v2.7 closure Stage B-6）
///
/// 影子期约束（ADR-043）：仅记录决策与奖励，不向 mlc-engine 注入策略；
/// Abstain 保守护栏（uncertainty > 0.7）不可被学习绕过。
pub mod s8_mem_pi;

/// 接缝标识 — v5.0 §7.3 学习接缝枚举
pub mod seam;

/// S9 接缝 — 通道路由学习器(MCA M3,RouteLLM 落点,ADR-065)
///
/// LinUCB 臂空间 = provider × model × thinking_mode(~40 臂),影子模式合规;
/// 不触 R2 冻结面(不 import gsoe-evolution/auto-dpo,无 evolve/RL 训练路径)。
pub mod s9_route;

/// S9 会话集成 — 将 StreamSessionCompleted 事件桥接到 S9RouteLearner(P1-2)
///
/// 启动后台任务订阅 `StreamSessionCompleted` 事件,解析 `route_key` 为 arm_id
/// 并调用 `learner.observe()` 更新 LinUCB 模型。不引入 mca-gateway(L10) 依赖,
/// 仅消费 event-bus(L1) 事件,符合 §2.2 依赖铁律。
pub mod s9_integration;

/// P4-W16.2.1: 经验回放池 — off-policy RL 训练的轨迹存储与采样基础设施
pub mod replay_pool;

/// polish-v2.7 P4-1: PER 优先经验回放 — SumTree O(log n) 优先级采样(ADR-049 决策 4)
///
/// R2 冻结声明(ADR-042):经验数据仅供 R1 路径,禁止 R2 约束 RL。
pub mod per_buffer;

/// P4-W16.2.2: R1 召回配额离线 RL（CQL/IQL）— S7 接缝
pub mod r1_recall_quota;

/// P4-W16.2.2: R1 影子模式 — 2 周观察期对比报告与解冻条件评估（ADR-043）
pub mod shadow_mode;

/// R2 解冻阶段③ 前置 1 — 后悔率自动采集管线（ADR-052 待办 1,纯可观测性）
///
/// 滑动窗口聚合后悔率观测,复用 `verify_regret_non_increasing` 产出趋势
/// `VerificationResult`,喂给 decay-engine `ShadowModeCircuitBreaker`（前置1→前置3
/// 信号链）。仅记录观测,无 RL 训练,不含 R2 扫描关键词。
pub mod regret_pipeline;

// ============================================================
// 公开 API 导出
// ============================================================

pub use arm::{ArmId, ArmIndex, ArmSet, DiscreteArmSet};
// polish-v2.7 P4-1: PER 公开 API 重导出
pub use context::SeamContext;
pub use error::{LearnerError, Result};
// FormalVerifier M1:学习单调性验证器重导出（P7-T5）
pub use formal::LearningMonotonicityChecker;
// R2 解冻阶段③ 前置 1:后悔率采集管线重导出
pub use linucb::LinUCB;
pub use per_buffer::{PerBuffer, PerSample};
pub use regret_pipeline::{
    RegretCollector, RegretSample, DEFAULT_CAPACITY, DEFAULT_TREND_TOLERANCE, DEFAULT_TREND_WINDOW,
};
// P4-W16.2.1: 经验回放池类型重导出(供 L9/L10 上层实例化与填充)
pub use replay_pool::{ReplayPool, ReplayPoolStats, ReplaySample, TrajectorySource};
// P4-W16.2.2: R1 召回配额离线 RL（CQL/IQL）类型重导出
pub use r1_recall_quota::{
    CqlPolicy, IqlPolicy, R1Algorithm, R1Context, R1Reward, R1RewardParams, RecallQuotaConfig,
    RecallQuotaLearner, RecallQuotaTransition, DEFAULT_BATCH_SIZE, DEFAULT_CQL_ALPHA,
    DEFAULT_GAMMA, DEFAULT_GRAD_CLIP, DEFAULT_IQL_TAU, DEFAULT_L2_REG, DEFAULT_LR,
    DEFAULT_MIN_POOL_SIZE, DEFAULT_TRAIN_ITERS, R1_ARM_COUNT, R1_CONTEXT_DIM,
};
// P4-W16.2.2: R1 影子模式类型重导出
pub use shadow_mode::{
    ComparisonResult, PromotionReadiness, RollbackSignal, ShadowComparisonReport, ShadowModeError,
    ShadowModeTracker, StrategyMetrics, DEFAULT_OBSERVATION_DAYS, DEFAULT_WIN_RATE_THRESHOLD,
    EWMA_COLLAPSE_THRESHOLD, EWMA_PROMOTION_THRESHOLD, RECALL_DROP_THRESHOLD,
    REGRESSION_STREAK_THRESHOLD,
};
// P4-W13.2: S1 接缝（DDR/HCW 密度档位）学习器
pub use s1_density::{
    arm_index_to_tier, s1_arm_set, tier_to_arm_index, S1Context, S1Learner, S1Reward,
    S1RewardParams, TaskType, DEFAULT_S1_ALPHA,
};
// P4-W14.1: S2 接缝（mlc-engine 记忆策略选择）学习器
// Task 2: S2StrategyAdapter 桥接 S2Learner 到 OSA MemoryStrategyProvider trait
pub use s2_memory::{
    arm_index_to_strategy, s2_arm_set, strategy_to_arm_index, S2Context, S2Learner, S2Reward,
    S2RewardParams, S2StrategyAdapter, TaskPhase, DEFAULT_S2_ALPHA, S2_ARM_COUNT, S2_CONTEXT_DIM,
};
// P4-W14.2: S3 接缝（scc-cache 预取策略）学习器
pub use s3_prefetch::{
    arm_index_to_strategy as arm_index_to_prefetch_strategy, s3_arm_set,
    strategy_to_arm_index as prefetch_strategy_to_arm_index, S3Context, S3Learner, S3Reward,
    S3RewardParams, DEFAULT_S3_ALPHA, S3_ARM_COUNT, S3_CONTEXT_DIM,
};
// P4-W13.3: S4 接缝（HCW selector 权重系数）学习器
pub use s4_selector::{
    arm_index_to_short_name, arm_index_to_weights, s4_arm_set, weights_to_arm_index, BlockType,
    S4Context, S4Learner, S4Reward, DEFAULT_S4_ALPHA, S4_ARM_COUNT, S4_ARM_WEIGHTS,
};
// P4-W14.3: S5 接缝（Parliament 激活策略）学习器
//
// WHY 重命名导入: `arm_index_to_strategy` 与 S2/S3 同名，必须 `as` 重命名避免歧义
// （Rust 的 `pub use` 不允许同名符号共存，否则 E0252 冲突）
pub use s5_parliament::{
    arm_index_to_strategy as arm_index_to_activation_strategy, s5_arm_set,
    strategy_to_arm_index as activation_strategy_to_arm_index, S5Context, S5Learner, S5Reward,
    S5RewardParams, DEFAULT_DEBATE_COST_PENALTY_LAMBDA, DEFAULT_S5_ALPHA, S5_ARM_COUNT,
    S5_CONTEXT_DIM,
};
// P4-W14.4: S6 接缝（decay-engine 衰减参数）学习器
pub use s6_decay::{
    arm_index_to_profile, profile_to_arm_index, s6_arm_set, OperationType, S6Context, S6Learner,
    S6Reward, S6RewardParams, DEFAULT_FALSE_BLOCK_WEIGHT, DEFAULT_FALSE_PASS_WEIGHT,
    DEFAULT_S6_ALPHA, S6_ARM_COUNT, S6_CONTEXT_DIM,
};
// closure Stage B-6: S8 接缝（Mem-π 记忆决策）学习器
//
// WHY 重命名导入: `arm_index_to_decision` 等映射函数名与其他接缝模式一致，
// S8 的决策映射无同名冲突可直接导出
pub use s8_mem_pi::{
    arm_index_to_decision, decision_to_arm_index, s8_arm_set, MemPiDecision, S8Context, S8Learner,
    S8Reward, S8RewardParams, ABSTAIN_UNCERTAINTY_THRESHOLD, DEFAULT_NOISE_PENALTY_LAMBDA,
    DEFAULT_S8_ALPHA, S8_ARM_COUNT, S8_CONTEXT_DIM,
};
pub use s9_integration::S9SessionIntegration;
pub use s9_route::{S9Context, S9Reward, S9RouteLearner, S9_CONTEXT_DIM};
pub use seam::SeamId;

// ============================================================
// Send + Sync 静态断言
// ============================================================
//
// WHY 必要性: omega-learner 的 LinUCB 实例可能被多线程共享(如 async 任务中
// 跨 await 持有),编译期断言 Send+Sync 防止误用。
//
// 实现模式(惰性断言函数,非 const context):
//  - Rust 1.71+ const context 不能调用非 const fn(project_memory 教训)
//  - 用 `fn _assert_xxx_send_sync()` 惰性断言,编译期 dead_code 分析识别,运行时零成本

/// LinUCB 必须实现 Send + Sync(异步跨线程共享需求)
fn _assert_linucb_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<LinUCB>();
}

/// SeamContext 必须实现 Send + Sync(跨 await 持有需求)
fn _assert_seam_context_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<SeamContext>();
}

/// DiscreteArmSet 必须实现 Send + Sync
fn _assert_discrete_arm_set_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<DiscreteArmSet>();
}

// P4-W14.3: S5Learner 必须实现 Send + Sync（异步跨线程共享需求）
//
// WHY 必要性: Parliament 是 L8 层关键路径组件，可能在 async 任务中跨 await 持有
// S5Learner。编译期断言 Send+Sync 防止误用（如内含 Rc<T> 等非 Send 类型）。
fn _assert_s5_learner_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<s5_parliament::S5Learner>();
}

// P4-W14.4: S6Learner 必须实现 Send + Sync（异步跨线程共享需求）
//
// WHY 必要性: decay-engine 是 L4 层安全组件，可能在 async 任务中跨 await 持有
// S6Learner（如 Skeptic 异步审议触发衰减决策）。编译期断言 Send+Sync 防止误用。
fn _assert_s6_learner_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<s6_decay::S6Learner>();
}

// P4-W16.2.2: R1 离线 RL 类型必须实现 Send + Sync（跨 async 任务共享需求）
//
// WHY 必要性: CqlPolicy / IqlPolicy / RecallQuotaLearner 可能被 chimera-cli /
// quest-engine 在 async 任务中跨 await 持有（如影子模式每日对比报告生成）。
// 内部仅含 ndarray Array1/Array2（Send + Sync），编译期断言验证不误用 Rc 等非 Send 类型。
fn _assert_r1_learners_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<r1_recall_quota::CqlPolicy>();
    assert_send_sync::<r1_recall_quota::IqlPolicy>();
    assert_send_sync::<r1_recall_quota::RecallQuotaLearner>();
    assert_send_sync::<r1_recall_quota::R1Context>();
    assert_send_sync::<r1_recall_quota::RecallQuotaTransition>();
}

/// 预导出模块 — 常用类型的便捷导入
///
/// # 示例
///
/// ```
/// use omega_learner::prelude::*;
///
/// let arm_set = DiscreteArmSet::new(vec![ArmId::new("a"), ArmId::new("b")]);
/// let mut linucb = LinUCB::new(2, &arm_set, 1.0).unwrap();
/// let ctx = SeamContext::new(vec![0.5, 0.5]).unwrap();
/// let arm = linucb.select_arm(&ctx).unwrap();
/// linucb.update(arm, &ctx, 0.5).unwrap();
/// ```
pub mod prelude {
    pub use crate::arm::{ArmId, ArmIndex, ArmSet, DiscreteArmSet};
    pub use crate::context::SeamContext;
    pub use crate::error::{LearnerError, Result};
    pub use crate::linucb::LinUCB;
    // P4-W16.2.1: 经验回放池类型
    pub use crate::replay_pool::{ReplayPool, ReplayPoolStats, ReplaySample, TrajectorySource};
    // P4-W16.2.2: R1 召回配额离线 RL（CQL/IQL）类型
    pub use crate::r1_recall_quota::{
        CqlPolicy, IqlPolicy, R1Algorithm, R1Context, R1Reward, R1RewardParams, RecallQuotaConfig,
        RecallQuotaLearner, RecallQuotaTransition, DEFAULT_BATCH_SIZE, DEFAULT_CQL_ALPHA,
        DEFAULT_GAMMA, DEFAULT_GRAD_CLIP, DEFAULT_IQL_TAU, DEFAULT_L2_REG, DEFAULT_LR,
        DEFAULT_MIN_POOL_SIZE, DEFAULT_TRAIN_ITERS, R1_ARM_COUNT, R1_CONTEXT_DIM,
    };
    // P4-W13.2: S1 接缝（DDR/HCW 密度档位）学习器
    pub use crate::s1_density::{
        arm_index_to_tier, s1_arm_set, tier_to_arm_index, S1Context, S1Learner, S1Reward,
        S1RewardParams, TaskType, DEFAULT_S1_ALPHA,
    };
    // P4-W14.1: S2 接缝（mlc-engine 记忆策略选择）学习器
    pub use crate::s2_memory::{
        arm_index_to_strategy, s2_arm_set, strategy_to_arm_index, S2Context, S2Learner, S2Reward,
        S2RewardParams, TaskPhase, DEFAULT_S2_ALPHA, S2_ARM_COUNT, S2_CONTEXT_DIM,
    };
    // P4-W14.2: S3 接缝（scc-cache 预取策略）学习器
    pub use crate::s3_prefetch::{
        arm_index_to_strategy as arm_index_to_prefetch_strategy, s3_arm_set,
        strategy_to_arm_index as prefetch_strategy_to_arm_index, S3Context, S3Learner, S3Reward,
        S3RewardParams, DEFAULT_S3_ALPHA, S3_ARM_COUNT, S3_CONTEXT_DIM,
    };
    // P4-W13.3: S4 接缝（HCW selector 权重系数）学习器
    pub use crate::s4_selector::{
        arm_index_to_short_name, arm_index_to_weights, s4_arm_set, weights_to_arm_index, BlockType,
        S4Context, S4Learner, S4Reward, DEFAULT_S4_ALPHA, S4_ARM_COUNT, S4_ARM_WEIGHTS,
    };
    // P4-W14.3: S5 接缝（Parliament 激活策略）学习器
    pub use crate::s5_parliament::{
        arm_index_to_strategy as arm_index_to_activation_strategy, s5_arm_set,
        strategy_to_arm_index as activation_strategy_to_arm_index, S5Context, S5Learner, S5Reward,
        S5RewardParams, DEFAULT_DEBATE_COST_PENALTY_LAMBDA, DEFAULT_S5_ALPHA, S5_ARM_COUNT,
        S5_CONTEXT_DIM,
    };
    // P4-W14.4: S6 接缝（decay-engine 衰减参数）学习器
    pub use crate::s6_decay::{
        arm_index_to_profile, profile_to_arm_index, s6_arm_set, OperationType, S6Context,
        S6Learner, S6Reward, S6RewardParams, DEFAULT_FALSE_BLOCK_WEIGHT, DEFAULT_FALSE_PASS_WEIGHT,
        DEFAULT_S6_ALPHA, S6_ARM_COUNT, S6_CONTEXT_DIM,
    };
    pub use crate::seam::SeamId;
}
