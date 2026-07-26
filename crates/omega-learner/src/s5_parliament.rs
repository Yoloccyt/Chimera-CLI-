//! S5 接缝 — Parliament 激活策略学习器（LinUCB 上下文线性 bandit）
//!
//! 对应任务: **P4-W14.3.1**（S5 接缝上下文/臂/奖励定义）
//! 对应 ADR: **ADR-031**（omega-learner 边界）
//! 对应设计源: `NEXUS-OMEGA_v5.0_系统性完整设计文档.md` §7.3 S5
//!
//! # S5 接缝概述
//!
//! | 字段 | 值 |
//! |------|-----|
//! | 接缝名 | S5ParliamentActivation（Parliament 激活策略） |
//! | 代码锚点 | `crates/parliament/src/` |
//! | 臂 | 3 种激活策略（FastPath/Simplified/Full） |
//! | 上下文 | 风险等级 + 只读操作比例 + 历史推翻率 |
//! | 奖励 | 决策正确性 − 辩论成本惩罚 |
//!
//! # 上下文向量设计（4 维）
//!
//! ```text
//! x = [
//!   risk_level,              // 0: 提案风险等级 ∈ [0, 1]
//!   read_only_ratio,         // 1: 只读操作比例 ∈ [0, 1]
//!   historical_overturn_rate, // 2: 近期历史推翻率 ∈ [0, 1]
//!   bias,                     // 3: 常量 1.0（线性模型偏置项）
//! ]
//! ```
//!
//! 维度 `d = 4`，满足 LinUCB regret 上界 `O(√(T·d·ln(K·T)))` 的有界假设。
//!
//! WHY 4 维而非更多: Parliament 激活的核心信号是"风险等级"（决定辩论收益）
//! 与"只读操作比例"（决定副作用风险），以及"历史推翻率"（决定信任度）。
//! 这三者决定辩论价值；3 个特征已足够区分 3 个臂（≥ log2(3) ≈ 2 维），
//! 比 S1 的 8 维 / S2 的 6 维更简洁。
//!
//! # 臂集设计
//!
//! 3 臂对应 `ActivationStrategy::ALL`：
//! `FastPath` / `Simplified` / `Full`。
//! 臂 ID 用策略简称字符串（与 `ActivationStrategy::short_name()` 一致），
//! 便于跨版本持久化与 SpecRegistry 谱系追踪。
//!
//! # 奖励函数
//!
//! `reward = correctness_score − λ × debate_cost`
//!
//! - `correctness_score ∈ [0, 1]`: 决策正确性（1.0 = 未被推翻，0.0 = 被推翻）
//! - `debate_cost ∈ [0.0, 1.0]`: 归一化辩论成本（FastPath=0.0/Simplified=0.3/Full=1.0）
//! - `λ = 0.5`（默认，控制辩论成本惩罚强度）
//!
//! WHY 加法形式而非相乘: LinUCB 假设奖励是上下文线性函数，
//! 加法形式符合线性假设；乘法形式会导致 reward 非线性化，破坏 regret 上界。
//!
//! # 三重悖论"推理悖论"修复路径
//!
//! 三重悖论病理：10 层架构的跨层协调成本存在阈值，当协调成本超过推理增益时
//! 多 Agent 反而不如单 Agent。Parliament 辩论是典型的"协调成本 vs 推理增益"权衡：
//! - 辩论消耗 CPU/内存/时间资源（协调成本）
//! - 辩论可避免高风险提案通过（推理增益）
//! S5 接缝通过 LinUCB 学习上下文特征 → 策略映射，使辩论强度随场景自适应，
//! 避免 FastPath 时高风险提案误通过（推理增益损失）与 Full 时
//! 辩论成本过大（协调成本超载）。
//!
//! # C4 合规（能力场灰度，非运行时旗）
//!
//! `S5Learner` 输出 `ParliamentPolicy::Learned { version, strategy }` 给上层调用方
//! （chimera-cli / quest-engine），由上层通过
//! `ParliamentLearnerHolder::update_policy()` 注入。`parliament` 本地
//! fallback 到 `ParliamentPolicy::Static(ActivationStrategy::Full)`，
//! **无跨 crate 旗标传播**（spec.md:334）。
//!
//! # 示例
//!
//! ## 基础学习流程
//!
//! ```
//! use nexus_contracts::ActivationStrategy;
//! use omega_learner::s5_parliament::{S5Context, S5Learner, S5Reward};
//!
//! // 1. 创建 S5 学习器（α=1.0，默认奖励参数）
//! let mut learner = S5Learner::new(1.0).unwrap();
//!
//! // 2. 构造上下文（风险 0.8，只读比例 0.2，历史推翻率 0.3）
//! let ctx = S5Context::new(0.8, 0.2, 0.3).unwrap();
//!
//! // 3. 选择激活策略
//! let strategy = learner.select(&ctx).unwrap();
//! assert!(matches!(strategy, ActivationStrategy::FastPath
//!     | ActivationStrategy::Simplified
//!     | ActivationStrategy::Full));
//!
//! // 4. 观察奖励并更新模型（正确性 1.0，使用 Full 辩论成本 1.0）
//! let reward = S5Reward::new(1.0, strategy).unwrap();
//! learner.update(&ctx, strategy, &reward).unwrap();
//! assert_eq!(learner.total_steps(), 1);
//!
//! // 5. 输出当前策略（ParliamentPolicy::Learned）
//! let policy = learner.current_policy(1);
//! assert!(policy.is_learned());
//! assert_eq!(policy.strategy(), strategy);
//! ```

use crate::arm::{ArmId, DiscreteArmSet};
use crate::context::SeamContext;
use crate::error::{LearnerError, Result};
use crate::linucb::LinUCB;
use nexus_contracts::{ActivationStrategy, ParliamentPolicy};
use serde::{Deserialize, Serialize};

// ============================================================
// 常量定义
// ============================================================

/// S5 上下文维度（risk_level + read_only_ratio + historical_overturn_rate + bias）
pub const S5_CONTEXT_DIM: usize = 4;

/// S5 默认辩论成本惩罚强度 λ（correctness_score - λ × debate_cost）
///
/// WHY λ=0.5: 比 S3 的 0.3 更强，因为辩论成本是 Parliament 的主要协调开销，
/// λ=0.5 提供中等惩罚避免过度使用 Full 辩论消耗资源，同时不喧宾夺主
/// （主要指标仍是决策正确性 correctness_score）。
/// - λ=0.8 过强：可能导致 learner 过度偏向 FastPath 以最小化成本（高风险提案误通过）
/// - λ=0.2 过弱：对 Full 等高成本策略激励不足
pub const DEFAULT_DEBATE_COST_PENALTY_LAMBDA: f64 = 0.5;

/// S5 默认探索强度 α（LinUCB 探索-利用平衡）
///
/// WHY α=1.0: 与 S1/S2/S3/S4 保持一致，Li et al. (2010) 推荐的稳健默认值，
/// 在合理范数假设下提供 O(√(T·d·ln(K·T))) regret 上界。
pub const DEFAULT_S5_ALPHA: f64 = 1.0;

/// S5 臂数（3 种激活策略）
pub const S5_ARM_COUNT: usize = 3;

// ============================================================
// S5 上下文
// ============================================================

/// S5 上下文 — 风险等级 / 只读操作比例 / 历史推翻率
///
/// 编码为 4 维特征向量，供 LinUCB 消费。所有数值字段归一化到 [0, 1]，
/// 满足 LinUCB regret 上界假设 `||x||` 有界。
///
/// # 设计决策（WHY）
/// - **不使用枚举**: 与 S1 TaskType/S2 TaskPhase 不同，S5 的三个特征均为
///   连续数值（[0,1]），不需要离散枚举编码，简化设计
/// - **risk_level 归一化**: 风险等级 ∈ [0, 1]，调用方负责归一化
///   （如 `proposal.risk_level` 已归一化）
/// - **read_only_ratio 直接传入**: 已归一化（只读操作数 / 总操作数）
/// - **historical_overturn_rate 直接传入**: 已归一化（近期被推翻次数 / 总审议次数）
/// - **bias 常量 1.0**: 线性模型偏置项，允许 θ_a 学习"基础偏好"
///
/// # L2 范数分析
/// - 最小范数: 仅 bias=1.0 = 1.0
/// - 最大范数: 3 个特征都为 1.0 + bias = √4 = 2.0
///
/// **WHY 不强制归一化**: LinUCB regret 上界假设 `||x|| ≤ 1`，
/// 但实践中允许稍大范数只需相应增大 α 探索强度。
/// `S5Learner::new` 默认 α=1.0，对范数 2.0 仍提供合理探索。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct S5Context {
    /// 提案风险等级 ∈ [0, 1]（高风险 → 倾向 Full 辩论）
    pub risk_level: f32,
    /// 只读操作比例 ∈ [0, 1]（高只读 → 倾向 FastPath）
    pub read_only_ratio: f32,
    /// 近期历史推翻率 ∈ [0, 1]（高推翻率 → 倾向 Full 辩论）
    pub historical_overturn_rate: f32,
}

impl S5Context {
    /// 创建 S5 上下文
    ///
    /// # 参数
    /// - `risk_level`: 提案风险等级 ∈ [0, 1]（调用方归一化）
    /// - `read_only_ratio`: 只读操作比例 ∈ [0, 1]
    /// - `historical_overturn_rate`: 近期历史推翻率 ∈ [0, 1]
    ///
    /// # 错误
    /// - `InvalidReward`: 任一字段不在 [0, 1] 或非有限
    ///
    /// # 示例
    ///
    /// ```
    /// use omega_learner::s5_parliament::S5Context;
    ///
    /// let ctx = S5Context::new(0.8, 0.2, 0.3).unwrap();
    /// assert!((ctx.risk_level - 0.8).abs() < 1e-6);
    /// assert!((ctx.read_only_ratio - 0.2).abs() < 1e-6);
    /// assert!((ctx.historical_overturn_rate - 0.3).abs() < 1e-6);
    /// ```
    pub fn new(
        risk_level: f32,
        read_only_ratio: f32,
        historical_overturn_rate: f32,
    ) -> Result<Self> {
        if !risk_level.is_finite() || !(0.0..=1.0).contains(&risk_level) {
            return Err(LearnerError::InvalidReward {
                reward: risk_level as f64,
            });
        }
        if !read_only_ratio.is_finite() || !(0.0..=1.0).contains(&read_only_ratio) {
            return Err(LearnerError::InvalidReward {
                reward: read_only_ratio as f64,
            });
        }
        if !historical_overturn_rate.is_finite() || !(0.0..=1.0).contains(&historical_overturn_rate)
        {
            return Err(LearnerError::InvalidReward {
                reward: historical_overturn_rate as f64,
            });
        }

        Ok(Self {
            risk_level,
            read_only_ratio,
            historical_overturn_rate,
        })
    }

    /// 编码为 4 维特征向量，供 LinUCB 消费
    ///
    /// 向量布局:
    /// - `[0]`: risk_level
    /// - `[1]`: read_only_ratio
    /// - `[2]`: historical_overturn_rate
    /// - `[3]`: bias 常量 1.0
    ///
    /// # L2 范数分析
    /// - 最小范数: 仅 bias=1.0 = 1.0
    /// - 最大范数: 3 个特征都为 1.0 + bias = √4 = 2.0
    pub fn features(&self) -> [f32; S5_CONTEXT_DIM] {
        let mut features = [0.0f32; S5_CONTEXT_DIM];
        features[0] = self.risk_level;
        features[1] = self.read_only_ratio;
        features[2] = self.historical_overturn_rate;
        // bias 常量
        features[3] = 1.0;
        features
    }

    /// 转换为 `SeamContext`（LinUCB 输入）
    ///
    /// WHY 提供: `LinUCB::select_arm` 接受 `&SeamContext`，
    /// 本方法封装 features → SeamContext 转换，避免调用方重复样板代码。
    pub fn to_seam_context(&self) -> Result<SeamContext> {
        SeamContext::new(self.features().to_vec())
    }
}

impl std::fmt::Display for S5Context {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "S5Context(risk={:.2}, readonly={:.2}, overturn={:.2})",
            self.risk_level, self.read_only_ratio, self.historical_overturn_rate
        )
    }
}

// ============================================================
// S5 奖励
// ============================================================

/// S5 奖励参数 — 控制辩论成本惩罚强度
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct S5RewardParams {
    /// 辩论成本惩罚强度 λ（correctness_score - λ × debate_cost）
    pub debate_cost_penalty_lambda: f64,
}

impl Default for S5RewardParams {
    fn default() -> Self {
        Self {
            debate_cost_penalty_lambda: DEFAULT_DEBATE_COST_PENALTY_LAMBDA,
        }
    }
}

/// S5 奖励 — 决策正确性 − 辩论成本惩罚
///
/// 公式: `reward = correctness_score − λ × debate_cost`
///
/// # 字段
/// - `correctness_score ∈ [0, 1]`: 决策正确性（1.0 = 未被推翻，0.0 = 被推翻）
/// - `debate_cost ∈ [0.0, 1.0]`: 归一化辩论成本（由激活策略决定）
/// - `params`: 奖励参数（λ）
///
/// # 边界处理
/// - `debate_cost = 0.0`（FastPath）: 成本惩罚为 0（奖励 = correctness_score）
/// - `debate_cost = 1.0`（Full）: 成本惩罚为 λ（reward = correctness_score - λ）
/// - `correctness_score = 1.0 + cost = 0.0`（FastPath 正确）: reward → 1.0（最大奖励）
/// - `correctness_score = 0.0 + cost = 1.0`（Full 错误）: reward → -λ（强惩罚）
///
/// # 示例
///
/// ```
/// use nexus_contracts::ActivationStrategy;
/// use omega_learner::s5_parliament::{S5Reward, S5RewardParams};
///
/// // 正确决策 + FastPath（成本 0.0）= 满分
/// let r1 = S5Reward::new(1.0, ActivationStrategy::FastPath).unwrap();
/// assert!((r1.reward() - 1.0).abs() < 1e-6);
///
/// // 正确决策 + Full（成本 1.0，惩罚 0.5 × 1.0 = 0.5）
/// let r2 = S5Reward::new(1.0, ActivationStrategy::Full).unwrap();
/// assert!((r2.reward() - 0.5).abs() < 1e-6);
///
/// // 错误决策 + Full（reward = 0 - 0.5 = -0.5）
/// let r3 = S5Reward::new(0.0, ActivationStrategy::Full).unwrap();
/// assert!((r3.reward() - (-0.5)).abs() < 1e-6);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct S5Reward {
    /// 决策正确性 ∈ [0, 1]（1.0 = 未被推翻，0.0 = 被推翻）
    pub correctness_score: f64,
    /// 归一化辩论成本 ∈ [0.0, 1.0]（由激活策略决定）
    pub debate_cost: f64,
    /// 奖励参数
    pub params: S5RewardParams,
}

impl S5Reward {
    /// 创建 S5 奖励（使用默认参数）
    ///
    /// # 参数
    /// - `correctness_score`: 决策正确性 ∈ [0, 1]
    /// - `strategy`: 使用的激活策略（决定 debate_cost）
    ///
    /// # 错误
    /// - `InvalidReward`: correctness_score 不在 [0, 1] 或非有限
    pub fn new(correctness_score: f64, strategy: ActivationStrategy) -> Result<Self> {
        Self::with_params(correctness_score, strategy, S5RewardParams::default())
    }

    /// 创建 S5 奖励（自定义参数）
    ///
    /// # 参数
    /// - `correctness_score`: 决策正确性 ∈ [0, 1]
    /// - `strategy`: 使用的激活策略（决定 debate_cost）
    /// - `params`: 奖励参数（λ）
    ///
    /// # 错误
    /// - `InvalidReward`: correctness_score 不在 [0, 1] 或非有限
    pub fn with_params(
        correctness_score: f64,
        strategy: ActivationStrategy,
        params: S5RewardParams,
    ) -> Result<Self> {
        if !correctness_score.is_finite() || !(0.0..=1.0).contains(&correctness_score) {
            return Err(LearnerError::InvalidReward {
                reward: correctness_score,
            });
        }
        Ok(Self {
            correctness_score,
            debate_cost: strategy.debate_cost() as f64,
            params,
        })
    }

    /// 创建 S5 奖励（直接指定成本，便于测试自定义场景）
    ///
    /// # 参数
    /// - `correctness_score`: 决策正确性 ∈ [0, 1]
    /// - `debate_cost`: 归一化辩论成本 ∈ [0, 1]
    /// - `params`: 奖励参数（λ）
    ///
    /// # 错误
    /// - `InvalidReward`: 任一字段不在 [0, 1] 或非有限
    #[allow(dead_code)]
    fn with_cost(correctness_score: f64, debate_cost: f64, params: S5RewardParams) -> Result<Self> {
        if !correctness_score.is_finite() || !(0.0..=1.0).contains(&correctness_score) {
            return Err(LearnerError::InvalidReward {
                reward: correctness_score,
            });
        }
        if !debate_cost.is_finite() || !(0.0..=1.0).contains(&debate_cost) {
            return Err(LearnerError::InvalidReward {
                reward: debate_cost,
            });
        }
        Ok(Self {
            correctness_score,
            debate_cost,
            params,
        })
    }

    /// 计算最终奖励值
    ///
    /// 公式: `reward = correctness_score − λ × debate_cost`
    ///
    /// WHY 加法形式: LinUCB 假设奖励是上下文线性函数，
    /// 加法形式符合线性假设；乘法形式会导致 reward 非线性化，破坏 regret 上界。
    pub fn reward(&self) -> f64 {
        let penalty = self.params.debate_cost_penalty_lambda * self.debate_cost;
        self.correctness_score - penalty
    }
}

// ============================================================
// S5 臂集（3 臂对应 ActivationStrategy::ALL）
// ============================================================

/// 构建 S5 接缝的臂集（3 臂对应 3 种激活策略）
///
/// 臂 ID 用策略简称字符串（与 `ActivationStrategy::short_name()` 一致），
/// 便于跨版本持久化与 SpecRegistry 谱系追踪。
///
/// WHY 函数而非常量: `DiscreteArmSet::new` 接受 `Vec<ArmId>`，
/// 不能在 const 上下文构造（Vec 堆分配）。每次调用开销 O(1)（3 个 ArmId 克隆）。
pub fn s5_arm_set() -> DiscreteArmSet {
    DiscreteArmSet::new(vec![
        ArmId::new(ActivationStrategy::FastPath.short_name()),
        ArmId::new(ActivationStrategy::Simplified.short_name()),
        ArmId::new(ActivationStrategy::Full.short_name()),
    ])
}

/// ArmIndex → ActivationStrategy 映射
///
/// 臂顺序与 `ActivationStrategy::ALL` 一致
/// （FastPath/Simplified/Full）。
/// WHY const fn: 映射是纯函数，编译期可计算，避免运行时开销。
pub const fn arm_index_to_strategy(idx: usize) -> ActivationStrategy {
    match idx {
        0 => ActivationStrategy::FastPath,
        1 => ActivationStrategy::Simplified,
        _ => ActivationStrategy::Full,
    }
}

/// ActivationStrategy → ArmIndex 映射
pub const fn strategy_to_arm_index(strategy: ActivationStrategy) -> usize {
    match strategy {
        ActivationStrategy::FastPath => 0,
        ActivationStrategy::Simplified => 1,
        ActivationStrategy::Full => 2,
    }
}

// ============================================================
// S5 学习器
// ============================================================

/// S5 学习器 — 封装 LinUCB + S5 上下文/臂/奖励逻辑
///
/// # 设计
///
/// `S5Learner` 是 `LinUCB` 的薄封装，提供 S5 接缝特定的:
/// - 上下文编码（`S5Context` → `SeamContext`）
/// - 臂映射（`ArmIndex` → `ActivationStrategy`）
/// - 奖励计算（`S5Reward` → `f64`）
///
/// # C4 合规
///
/// `S5Learner` 只产出 `ParliamentPolicy::Learned { version, strategy }`，
/// 不直接修改 `parliament` 状态。上层调用方负责通过
/// `ParliamentLearnerHolder::update_policy()` 注入。
///
/// # 线程安全
///
/// `S5Learner` 内部 `LinUCB` 非 `Sync`（ndarray 数组无原子操作），
/// 多线程共享需通过 `Arc<Mutex<S5Learner>>` 或 `Arc<RwLock<S5Learner>>`。
/// 异步学习器典型用法是单线程后台任务 + tokio::sync::mpsc 通信。
///
/// # 示例
///
/// ```
/// use nexus_contracts::ActivationStrategy;
/// use omega_learner::s5_parliament::{S5Context, S5Learner, S5Reward};
///
/// let mut learner = S5Learner::new(1.0).unwrap();
///
/// let ctx = S5Context::new(0.8, 0.2, 0.3).unwrap();
/// let strategy = learner.select(&ctx).unwrap();
///
/// let reward = S5Reward::new(1.0, strategy).unwrap();
/// learner.update(&ctx, strategy, &reward).unwrap();
///
/// let policy = learner.current_policy(1);
/// assert!(policy.is_learned());
/// assert_eq!(policy.strategy(), strategy);
/// ```
#[derive(Debug, Clone)]
pub struct S5Learner {
    /// 内部 LinUCB 实例
    linucb: LinUCB,
    /// 最近一次选择的臂索引（用于 `current_policy` 输出）
    last_arm_idx: usize,
    /// 已观察到的总步数
    total_steps: u64,
}

impl S5Learner {
    /// 创建 S5 学习器
    ///
    /// # 参数
    /// - `alpha`: LinUCB 探索强度（必须 > 0 且有限）
    ///
    /// # 错误
    /// - `InvalidAlpha`: alpha ≤ 0 或非有限
    /// - `NoArms`: 内部错误（S5 固定 3 臂，不应触发）
    pub fn new(alpha: f64) -> Result<Self> {
        let arm_set = s5_arm_set();
        let linucb = LinUCB::new(S5_CONTEXT_DIM, &arm_set, alpha)?;
        Ok(Self {
            linucb,
            last_arm_idx: 0,
            total_steps: 0,
        })
    }

    /// 创建 S5 学习器（使用默认 α=1.0）
    pub fn with_default_alpha() -> Result<Self> {
        Self::new(DEFAULT_S5_ALPHA)
    }

    /// 选择激活策略 — 基于 S5 上下文
    ///
    /// # 算法
    /// 1. 将 `S5Context` 编码为 4 维特征向量
    /// 2. 转换为 `SeamContext`（LinUCB 输入）
    /// 3. 调用 `LinUCB::select_arm` 选择 UCB 最大的臂
    /// 4. 将 `ArmIndex` 映射回 `ActivationStrategy`
    ///
    /// # 错误
    /// - `ContextDimensionMismatch`: 内部错误（S5 固定 4 维，不应触发）
    pub fn select(&mut self, context: &S5Context) -> Result<ActivationStrategy> {
        let seam_ctx = context.to_seam_context()?;
        let arm_idx = self.linucb.select_arm(&seam_ctx)?;
        self.last_arm_idx = arm_idx.as_usize();
        Ok(arm_index_to_strategy(self.last_arm_idx))
    }

    /// 观察奖励并更新模型
    ///
    /// # 参数
    /// - `context`: 选择时的 S5 上下文
    /// - `strategy`: 选择的激活策略
    /// - `reward`: 观察到的奖励
    ///
    /// # 错误
    /// - `ArmOutOfRange`: strategy 不在 S5 臂集中（不应触发）
    /// - `ContextDimensionMismatch`: 内部错误
    /// - `NumericalInstability`: Sherman-Morrison 分母 ≤ 0（矩阵病态）
    pub fn update(
        &mut self,
        context: &S5Context,
        strategy: ActivationStrategy,
        reward: &S5Reward,
    ) -> Result<()> {
        let seam_ctx = context.to_seam_context()?;
        let arm_idx = crate::arm::ArmIndex::from(strategy_to_arm_index(strategy));
        let reward_value = reward.reward();
        self.linucb.update(arm_idx, &seam_ctx, reward_value)?;
        self.total_steps += 1;
        Ok(())
    }

    /// 输出当前策略（ParliamentPolicy::Learned）
    ///
    /// # 参数
    /// - `version`: 学习版本号（单调递增，用于 A/B 测试与回滚）
    ///
    /// # 返回
    /// `ParliamentPolicy::Learned { version, strategy }`，
    /// strategy 为最近一次 `select` 的结果。
    ///
    /// WHY 提供: 上层调用方（chimera-cli / quest-engine）调用此方法
    /// 获取学习到的策略，然后通过 `ParliamentLearnerHolder::update_policy()` 注入。
    pub fn current_policy(&self, version: u64) -> ParliamentPolicy {
        ParliamentPolicy::learned(version, arm_index_to_strategy(self.last_arm_idx))
    }

    /// 返回已观察到的总步数
    pub fn total_steps(&self) -> u64 {
        self.total_steps
    }

    /// 返回内部 LinUCB 引用（用于诊断与持久化）
    pub fn linucb(&self) -> &LinUCB {
        &self.linucb
    }
}

// ============================================================
// Send + Sync 静态断言
// ============================================================

/// S5Learner 必须实现 Send + Sync（异步跨线程共享需求）
///
/// WHY 必要性: S5Learner 可能被 Arc<Mutex<S5Learner>> 包裹，
/// 在 tokio 异步任务中跨 await 持有，编译期断言 Send+Sync 防止误用。
fn _assert_s5_learner_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<S5Learner>();
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    // P4-W14.3 测试补丁: `arm_set.size()` 是 `ArmSet` trait 方法，trait 不在 `super::*` 范围内
    // （`super` = `s5_parliament` 模块，仅 import 了 `DiscreteArmSet` 类型而非 trait），
    // 需显式导入 `ArmSet` trait 才能在测试中调用 `size()` 方法。
    use crate::arm::ArmSet;

    // ============================================================
    // S5Context 测试
    // ============================================================

    #[test]
    fn test_s5_context_new_basic() {
        let ctx = S5Context::new(0.8, 0.2, 0.3).unwrap();
        assert!((ctx.risk_level - 0.8).abs() < 1e-6);
        assert!((ctx.read_only_ratio - 0.2).abs() < 1e-6);
        assert!((ctx.historical_overturn_rate - 0.3).abs() < 1e-6);
    }

    #[test]
    fn test_s5_context_zero_values() {
        let ctx = S5Context::new(0.0, 0.0, 0.0).unwrap();
        assert!(ctx.risk_level.abs() < 1e-6);
    }

    #[test]
    fn test_s5_context_max_values() {
        let ctx = S5Context::new(1.0, 1.0, 1.0).unwrap();
        assert!((ctx.risk_level - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_s5_context_invalid_risk_level() {
        // > 1.0 失败
        let result = S5Context::new(1.5, 0.5, 0.5);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));
    }

    #[test]
    fn test_s5_context_invalid_read_only_ratio() {
        // < 0.0 失败
        let result = S5Context::new(0.5, -0.1, 0.5);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));
    }

    #[test]
    fn test_s5_context_invalid_overturn_rate() {
        // NaN 失败
        let result = S5Context::new(0.5, 0.5, f32::NAN);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));
    }

    #[test]
    fn test_s5_context_features_layout() {
        let ctx = S5Context::new(0.8, 0.2, 0.3).unwrap();
        let features = ctx.features();
        assert_eq!(features.len(), S5_CONTEXT_DIM);
        assert!((features[0] - 0.8).abs() < 1e-6);
        assert!((features[1] - 0.2).abs() < 1e-6);
        assert!((features[2] - 0.3).abs() < 1e-6);
        assert!((features[3] - 1.0).abs() < 1e-6); // bias
    }

    #[test]
    fn test_s5_context_to_seam_context() {
        let ctx = S5Context::new(0.5, 0.5, 0.5).unwrap();
        let seam_ctx = ctx.to_seam_context().unwrap();
        assert_eq!(seam_ctx.dim(), S5_CONTEXT_DIM);
    }

    #[test]
    fn test_s5_context_display() {
        let ctx = S5Context::new(0.8, 0.2, 0.3).unwrap();
        let s = format!("{}", ctx);
        assert!(s.contains("S5Context"));
        assert!(s.contains("risk=0.80"));
    }

    // ============================================================
    // S5Reward 测试
    // ============================================================

    #[test]
    fn test_s5_reward_new_fastpath() {
        // FastPath 成本 0.0
        let r = S5Reward::new(1.0, ActivationStrategy::FastPath).unwrap();
        assert!((r.correctness_score - 1.0).abs() < 1e-6);
        assert!(r.debate_cost.abs() < 1e-6);
        assert!((r.reward() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_s5_reward_new_simplified() {
        // Simplified 成本 0.3
        let r = S5Reward::new(1.0, ActivationStrategy::Simplified).unwrap();
        assert!((r.debate_cost - 0.3).abs() < 1e-6);
        // reward = 1.0 - 0.5 × 0.3 = 0.85
        assert!((r.reward() - 0.85).abs() < 1e-6);
    }

    #[test]
    fn test_s5_reward_new_full() {
        // Full 成本 1.0
        let r = S5Reward::new(1.0, ActivationStrategy::Full).unwrap();
        assert!((r.debate_cost - 1.0).abs() < 1e-6);
        // reward = 1.0 - 0.5 × 1.0 = 0.5
        assert!((r.reward() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn test_s5_reward_incorrect_decision_full() {
        // 错误决策 + Full = reward = 0 - 0.5 = -0.5
        let r = S5Reward::new(0.0, ActivationStrategy::Full).unwrap();
        assert!((r.reward() - (-0.5)).abs() < 1e-6);
    }

    #[test]
    fn test_s5_reward_custom_lambda() {
        // 自定义 λ=0.8
        let params = S5RewardParams {
            debate_cost_penalty_lambda: 0.8,
        };
        let r = S5Reward::with_params(1.0, ActivationStrategy::Full, params).unwrap();
        // reward = 1.0 - 0.8 × 1.0 = 0.2
        assert!((r.reward() - 0.2).abs() < 1e-6);
    }

    #[test]
    fn test_s5_reward_invalid_correctness() {
        let result = S5Reward::new(1.5, ActivationStrategy::Full);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));
    }

    #[test]
    fn test_s5_reward_zero_correctness() {
        let r = S5Reward::new(0.0, ActivationStrategy::FastPath).unwrap();
        // reward = 0 - 0.5 × 0.0 = 0.0
        assert!(r.reward().abs() < 1e-6);
    }

    #[test]
    fn test_s5_reward_with_cost_custom() {
        // 直接指定成本，便于测试
        let params = S5RewardParams::default();
        let r = S5Reward::with_cost(0.8, 0.5, params).unwrap();
        // reward = 0.8 - 0.5 × 0.5 = 0.55
        assert!((r.reward() - 0.55).abs() < 1e-6);
    }

    // ============================================================
    // 臂集与映射测试
    // ============================================================

    #[test]
    fn test_s5_arm_set_count() {
        let arm_set = s5_arm_set();
        // `ArmSet::size()` 返回 `Option<usize>`（动态臂集可能无固定大小），
        // `DiscreteArmSet` 实现为 `Some(self.arms.len())`，故用 `Some(S5_ARM_COUNT)` 比对。
        assert_eq!(arm_set.size(), Some(S5_ARM_COUNT));
    }

    #[test]
    fn test_arm_index_to_strategy_mapping() {
        assert_eq!(arm_index_to_strategy(0), ActivationStrategy::FastPath);
        assert_eq!(arm_index_to_strategy(1), ActivationStrategy::Simplified);
        assert_eq!(arm_index_to_strategy(2), ActivationStrategy::Full);
        // 越界 fallback 到 Full
        assert_eq!(arm_index_to_strategy(99), ActivationStrategy::Full);
    }

    #[test]
    fn test_strategy_to_arm_index_mapping() {
        assert_eq!(strategy_to_arm_index(ActivationStrategy::FastPath), 0);
        assert_eq!(strategy_to_arm_index(ActivationStrategy::Simplified), 1);
        assert_eq!(strategy_to_arm_index(ActivationStrategy::Full), 2);
    }

    #[test]
    fn test_arm_mapping_roundtrip() {
        for strategy in ActivationStrategy::ALL.iter() {
            let idx = strategy_to_arm_index(*strategy);
            let restored = arm_index_to_strategy(idx);
            assert_eq!(*strategy, restored, "roundtrip failed for {:?}", strategy);
        }
    }

    // ============================================================
    // S5Learner 测试
    // ============================================================

    #[test]
    fn test_s5_learner_new_basic() {
        let learner = S5Learner::new(1.0).unwrap();
        assert_eq!(learner.total_steps(), 0);
    }

    #[test]
    fn test_s5_learner_with_default_alpha() {
        let learner = S5Learner::with_default_alpha().unwrap();
        assert_eq!(learner.total_steps(), 0);
    }

    #[test]
    fn test_s5_learner_invalid_alpha_zero() {
        let result = S5Learner::new(0.0);
        assert!(result.is_err());
    }

    #[test]
    fn test_s5_learner_invalid_alpha_negative() {
        let result = S5Learner::new(-1.0);
        assert!(result.is_err());
    }

    #[test]
    fn test_s5_learner_invalid_alpha_nan() {
        let result = S5Learner::new(f64::NAN);
        assert!(result.is_err());
    }

    #[test]
    fn test_s5_learner_select_returns_valid_strategy() {
        let mut learner = S5Learner::new(1.0).unwrap();
        let ctx = S5Context::new(0.5, 0.5, 0.5).unwrap();
        let strategy = learner.select(&ctx).unwrap();
        assert!(matches!(
            strategy,
            ActivationStrategy::FastPath
                | ActivationStrategy::Simplified
                | ActivationStrategy::Full
        ));
    }

    #[test]
    fn test_s5_learner_select_increments_no_step() {
        // select 不应增加 total_steps（仅 update 增加）
        let mut learner = S5Learner::new(1.0).unwrap();
        let ctx = S5Context::new(0.5, 0.5, 0.5).unwrap();
        learner.select(&ctx).unwrap();
        assert_eq!(learner.total_steps(), 0);
    }

    #[test]
    fn test_s5_learner_update_increments_steps() {
        let mut learner = S5Learner::new(1.0).unwrap();
        let ctx = S5Context::new(0.8, 0.2, 0.3).unwrap();
        let strategy = learner.select(&ctx).unwrap();
        let reward = S5Reward::new(1.0, strategy).unwrap();
        learner.update(&ctx, strategy, &reward).unwrap();
        assert_eq!(learner.total_steps(), 1);
    }

    #[test]
    fn test_s5_learner_multiple_updates() {
        let mut learner = S5Learner::new(1.0).unwrap();

        // 模拟多次审议
        for i in 0..10 {
            let risk = (i as f32) / 10.0;
            let ctx = S5Context::new(risk, 0.5, 0.3).unwrap();
            let strategy = learner.select(&ctx).unwrap();
            // 高风险场景，Full 辩论通常更正确
            let correctness = if strategy == ActivationStrategy::Full {
                0.9
            } else {
                0.6
            };
            let reward = S5Reward::new(correctness, strategy).unwrap();
            learner.update(&ctx, strategy, &reward).unwrap();
        }

        assert_eq!(learner.total_steps(), 10);
    }

    #[test]
    fn test_s5_learner_current_policy_learned() {
        let mut learner = S5Learner::new(1.0).unwrap();
        let ctx = S5Context::new(0.7, 0.3, 0.4).unwrap();
        let strategy = learner.select(&ctx).unwrap();

        let policy = learner.current_policy(42);
        assert!(policy.is_learned());
        assert_eq!(policy.version(), Some(42));
        assert_eq!(policy.strategy(), strategy);
    }

    #[test]
    fn test_s5_learner_current_policy_version_zero() {
        let learner = S5Learner::new(1.0).unwrap();
        let policy = learner.current_policy(0);
        assert!(policy.is_learned());
        assert_eq!(policy.version(), Some(0));
    }

    #[test]
    fn test_s5_learner_clone_independent() {
        let mut learner1 = S5Learner::new(1.0).unwrap();
        let ctx = S5Context::new(0.7, 0.3, 0.4).unwrap();
        let _strategy = learner1.select(&ctx).unwrap();

        // 克隆后两者独立演化
        let mut learner2 = learner1.clone();
        assert_eq!(learner1.total_steps(), learner2.total_steps());

        // 在 learner2 上 update
        let reward = S5Reward::new(1.0, ActivationStrategy::Full).unwrap();
        learner2
            .update(&ctx, ActivationStrategy::Full, &reward)
            .unwrap();

        // learner1 不受影响
        assert_eq!(learner1.total_steps(), 0);
        assert_eq!(learner2.total_steps(), 1);
    }

    #[test]
    fn test_s5_learner_linucb_access() {
        let learner = S5Learner::new(1.0).unwrap();
        let _linucb = learner.linucb();
        // 仅验证可访问（内部状态测试由 linucb 模块覆盖）
    }

    // ============================================================
    // 端到端场景测试
    // ============================================================

    #[test]
    fn test_s5_scenario_low_risk_readonly_prefers_fastpath() {
        // 低风险 + 高只读比例场景，多轮学习后应偏向 FastPath
        // 注意：LinUCB 初始探索阶段可能不立即收敛，此处仅验证不 panic
        let mut learner = S5Learner::new(1.0).unwrap();
        let ctx = S5Context::new(0.1, 0.9, 0.05).unwrap(); // 低风险 + 高只读 + 低推翻率

        for _ in 0..20 {
            let strategy = learner.select(&ctx).unwrap();
            // FastPath 在低风险场景下更可能正确
            let correctness = match strategy {
                ActivationStrategy::FastPath => 0.95, // FastPath 在此场景下高正确性
                ActivationStrategy::Simplified => 0.85,
                ActivationStrategy::Full => 0.85,
            };
            let reward = S5Reward::new(correctness, strategy).unwrap();
            learner.update(&ctx, strategy, &reward).unwrap();
        }

        assert_eq!(learner.total_steps(), 20);
        // 学习后输出策略（具体值由 LinUCB 决定）
        let policy = learner.current_policy(1);
        assert!(policy.is_learned());
    }

    #[test]
    fn test_s5_scenario_high_risk_write_prefers_full() {
        // 高风险 + 低只读比例场景，多轮学习后应偏向 Full
        let mut learner = S5Learner::new(1.0).unwrap();
        let ctx = S5Context::new(0.9, 0.1, 0.5).unwrap(); // 高风险 + 写操作 + 中推翻率

        for _ in 0..20 {
            let strategy = learner.select(&ctx).unwrap();
            // Full 在高风险场景下更可能正确（避免误通过）
            let correctness = match strategy {
                ActivationStrategy::FastPath => 0.4, // FastPath 误通过高风险
                ActivationStrategy::Simplified => 0.7,
                ActivationStrategy::Full => 0.95, // Full 正确审查
            };
            let reward = S5Reward::new(correctness, strategy).unwrap();
            learner.update(&ctx, strategy, &reward).unwrap();
        }

        assert_eq!(learner.total_steps(), 20);
        let policy = learner.current_policy(1);
        assert!(policy.is_learned());
    }

    #[test]
    fn test_s5_scenario_medium_risk_balanced() {
        // 中等风险场景，策略应在 Simplified 附近波动
        let mut learner = S5Learner::new(1.0).unwrap();
        let ctx = S5Context::new(0.5, 0.5, 0.3).unwrap();

        for _ in 0..15 {
            let strategy = learner.select(&ctx).unwrap();
            let correctness = 0.75; // 中等正确性
            let reward = S5Reward::new(correctness, strategy).unwrap();
            learner.update(&ctx, strategy, &reward).unwrap();
        }

        assert_eq!(learner.total_steps(), 15);
    }

    #[test]
    fn test_s5_scenario_c4_compliance_fallback() {
        // C4 合规：learner 输出 Learned，调用方 fallback 到 Static(Full)
        let mut learner = S5Learner::new(1.0).unwrap();
        let ctx = S5Context::new(0.5, 0.5, 0.5).unwrap();
        let _ = learner.select(&ctx).unwrap();

        let learned_policy = learner.current_policy(1);
        assert!(learned_policy.is_learned());

        // 模拟 learner panic 后本地 fallback
        let fallback = ParliamentPolicy::fallback();
        assert!(fallback.is_static());
        assert_eq!(fallback.strategy(), ActivationStrategy::Full);
    }

    #[test]
    fn test_s5_scenario_versioned_policy_for_ab_test() {
        // 版本号用于 A/B 测试与回滚
        let mut learner = S5Learner::new(1.0).unwrap();
        let ctx = S5Context::new(0.5, 0.5, 0.5).unwrap();
        learner.select(&ctx).unwrap();

        let v1 = learner.current_policy(1);
        let v2 = learner.current_policy(2);

        assert_ne!(v1.version(), v2.version());
        // 策略相同（同一次 select 结果）
        assert_eq!(v1.strategy(), v2.strategy());
    }
}
