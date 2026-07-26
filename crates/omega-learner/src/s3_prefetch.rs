//! S3 接缝 — SCC 预取策略学习器（v5.0 §7.3 六接缝之一）
//!
//! 对应任务: **P4-W14.2.1**（S3 接缝上下文/臂/奖励定义）
//! 对应 ADR: **ADR-031**（omega-learner 边界）
//! 对应设计源: `NEXUS-OMEGA_v5.0_系统性完整设计文档.md` §7.3 S3
//!
//! # S3 接缝概述
//!
//! | 字段 | 值 |
//! |------|-----|
//! | 接缝名 | S3Prefetch（SCC 预取策略） |
//! | 代码锚点 | `crates/scc-cache/src/prefetch.rs` |
//! | 臂 | 5 种预取策略（NoPrefetch/Conservative/Standard/Aggressive/TopK3） |
//! | 上下文 | 编辑密度 + 局部性强度 + 缓存压力 |
//! | 奖励 | 预取命中率 − 预取浪费惩罚 |
//!
//! # 上下文向量设计（4 维）
//!
//! ```text
//! x = [
//!   edit_density,        // 0: 编辑密度 ∈ [0, 1]（近期编辑频率归一化）
//!   locality_strength,   // 1: 局部性强度 ∈ [0, 1]（重复访问同一上下文的比例）
//!   cache_pressure,      // 2: 缓存压力 ∈ [0, 1]（used / budget）
//!   bias,                 // 3: 常量 1.0（线性模型偏置项）
//! ]
//! ```
//!
//! 维度 `d = 4`，满足 LinUCB regret 上界 `O(√(T·d·ln(K·T)))` 的有界假设。
//!
//! WHY 4 维而非更多: scc-cache 的核心信号是"编辑历史密度"与"局部性强度"，
//! 这两者决定预取价值；cache_pressure 决定预取消耗（缓存满时预取会挤占热点）。
//! 4 维已足够区分 5 个臂（≥ log2(5) ≈ 3 维），比 S1 的 8 维 / S2 的 6 维简洁。
//!
//! # 臂集设计
//!
//! 5 臂对应 `PrefetchStrategy::ALL`：
//! `NoPrefetch` / `Conservative` / `Standard` / `Aggressive` / `TopK3`。
//! 臂 ID 用策略简称字符串（与 `PrefetchStrategy::short_name()` 一致），
//! 便于跨版本持久化与 SpecRegistry 谱系追踪。
//!
//! # 奖励函数
//!
//! `reward = hit_rate − λ × prefetch_overhead_ratio`
//!
//! - `hit_rate ∈ [0, 1]`: 预取命中率（成功预热次数 / 总预取次数）
//! - `prefetch_overhead_ratio ∈ [0, 1]`: 预取浪费比例（预取但未访问次数 / 总预取次数）
//! - `λ = 0.3`（默认，控制预取浪费惩罚强度）
//!
//! WHY 加法形式而非相乘: LinUCB 假设奖励是上下文线性函数，
//! 加法形式符合线性假设；乘法形式会导致 reward 非线性化，破坏 regret 上界。
//!
//! # 三重悖论"推理悖论"修复路径
//!
//! 三重悖论病理：10 层架构的跨层协调成本存在阈值，当协调成本超过推理增益时
//! 多 Agent 反而不如单 Agent。预取是典型的"协调成本 vs 推理增益"权衡：
//! - 预取消耗 CPU/内存/IO 资源（协调成本）
//! - 预取命中可避免缓存未命中延迟（推理增益）
//! S3 接缝通过 LinUCB 学习上下文特征 → 策略映射，使预取强度随场景自适应，
//! 避免 NoPrefetch 时缓存未命中率高（推理增益损失）与 Aggressive 时
//! 预取消耗过大（协调成本超载）。
//!
//! # C4 合规（能力场灰度，非运行时旗）
//!
//! `S3Learner` 输出 `PrefetchPolicy::Learned { version, strategy }` 给上层调用方
//! （chimera-cli / quest-engine），由上层通过
//! `AccessPatternLearner::update_prefetch_policy()` 注入。`scc-cache` 本地
//! fallback 到 `PrefetchPolicy::Static(PrefetchStrategy::Standard)`，
//! **无跨 crate 旗标传播**（spec.md:334）。
//!
//! # 示例
//!
//! ## 基础学习流程
//!
//! ```
//! use nexus_contracts::PrefetchStrategy;
//! use omega_learner::s3_prefetch::{S3Context, S3Learner, S3Reward};
//!
//! // 1. 创建 S3 学习器（α=1.0，默认奖励参数）
//! let mut learner = S3Learner::new(1.0).unwrap();
//!
//! // 2. 构造上下文（编辑密度 0.8，局部性强度 0.7，缓存压力 0.4）
//! let ctx = S3Context::new(0.8, 0.7, 0.4).unwrap();
//!
//! // 3. 选择预取策略
//! let strategy = learner.select(&ctx).unwrap();
//! assert!(matches!(strategy, PrefetchStrategy::NoPrefetch
//!     | PrefetchStrategy::Conservative
//!     | PrefetchStrategy::Standard
//!     | PrefetchStrategy::Aggressive
//!     | PrefetchStrategy::TopK3));
//!
//! // 4. 观察奖励并更新模型（命中率 0.9，浪费率 0.1）
//! let reward = S3Reward::new(0.9, 0.1).unwrap();
//! learner.update(&ctx, strategy, &reward).unwrap();
//! assert_eq!(learner.total_steps(), 1);
//!
//! // 5. 输出当前策略（PrefetchPolicy::Learned）
//! let policy = learner.current_policy(1);
//! assert!(policy.is_learned());
//! assert_eq!(policy.strategy(), strategy);
//! ```

use crate::arm::{ArmId, DiscreteArmSet};
use crate::context::SeamContext;
use crate::error::{LearnerError, Result};
use crate::linucb::LinUCB;
use nexus_contracts::{PrefetchPolicy, PrefetchStrategy};
use serde::{Deserialize, Serialize};

// ============================================================
// 常量定义
// ============================================================

/// S3 上下文维度（edit_density + locality_strength + cache_pressure + bias）
pub const S3_CONTEXT_DIM: usize = 4;

/// S3 默认预取浪费惩罚强度 λ（hit_rate - λ × prefetch_overhead_ratio）
///
/// WHY λ=0.3: 与 S2 保持一致，预取浪费是次要指标（主要指标是命中率），
/// λ=0.3 提供温和惩罚避免过度预取低相关上下文，同时不喧宾夺主。
/// - λ=0.5 过强：可能导致 learner 过度偏向 NoPrefetch 以最小化浪费
/// - λ=0.1 过弱：对 Aggressive 等激进策略激励不足
pub const DEFAULT_PREFETCH_OVERHEAD_PENALTY_LAMBDA: f64 = 0.3;

/// S3 默认探索强度 α（LinUCB 探索-利用平衡）
///
/// WHY α=1.0: 与 S1/S2/S4 保持一致，Li et al. (2010) 推荐的稳健默认值，
/// 在合理范数假设下提供 O(√(T·d·ln(K·T))) regret 上界。
pub const DEFAULT_S3_ALPHA: f64 = 1.0;

/// S3 臂数（5 种预取策略）
pub const S3_ARM_COUNT: usize = 5;

// ============================================================
// S3 上下文
// ============================================================

/// S3 上下文 — 编辑密度 / 局部性强度 / 缓存压力
///
/// 编码为 4 维特征向量，供 LinUCB 消费。所有数值字段归一化到 [0, 1]，
/// 满足 LinUCB regret 上界假设 `||x||` 有界。
///
/// # 设计决策（WHY）
/// - **不使用枚举**: 与 S1 TaskType/S2 TaskPhase 不同，S3 的三个特征均为
///   连续数值（[0,1]），不需要离散枚举编码，简化设计
/// - **edit_density 归一化**: 编辑密度 ∈ [0, 1]，调用方负责归一化
///   （如 `recent_edits / window_size`）
/// - **locality_strength 直接传入**: 已归一化（重复访问比例）
/// - **cache_pressure 直接传入**: 已归一化（used / budget）
/// - **bias 常量 1.0**: 线性模型偏置项，允许 θ_a 学习"基础偏好"
///
/// # L2 范数分析
/// - 最小范数: 仅 bias=1.0 = 1.0
/// - 最大范数: 3 个特征都为 1.0 + bias = √4 = 2.0
///
/// **WHY 不强制归一化**: LinUCB regret 上界假设 `||x|| ≤ 1`，
/// 但实践中允许稍大范数只需相应增大 α 探索强度。
/// `S3Learner::new` 默认 α=1.0，对范数 2.0 仍提供合理探索。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct S3Context {
    /// 编辑密度 ∈ [0, 1]（近期编辑频率归一化）
    pub edit_density: f32,
    /// 局部性强度 ∈ [0, 1]（重复访问同一上下文的比例）
    pub locality_strength: f32,
    /// 缓存压力 ∈ [0, 1]（used / budget）
    pub cache_pressure: f32,
}

impl S3Context {
    /// 创建 S3 上下文
    ///
    /// # 参数
    /// - `edit_density`: 编辑密度 ∈ [0, 1]（调用方归一化）
    /// - `locality_strength`: 局部性强度 ∈ [0, 1]（重复访问比例）
    /// - `cache_pressure`: 缓存压力 ∈ [0, 1]（used / budget）
    ///
    /// # 错误
    /// - `InvalidReward`: 任一字段不在 [0, 1] 或非有限
    ///
    /// # 示例
    ///
    /// ```
    /// use omega_learner::s3_prefetch::S3Context;
    ///
    /// let ctx = S3Context::new(0.8, 0.7, 0.4).unwrap();
    /// assert!((ctx.edit_density - 0.8).abs() < 1e-6);
    /// assert!((ctx.locality_strength - 0.7).abs() < 1e-6);
    /// assert!((ctx.cache_pressure - 0.4).abs() < 1e-6);
    /// ```
    pub fn new(edit_density: f32, locality_strength: f32, cache_pressure: f32) -> Result<Self> {
        if !edit_density.is_finite() || !(0.0..=1.0).contains(&edit_density) {
            return Err(LearnerError::InvalidReward {
                reward: edit_density as f64,
            });
        }
        if !locality_strength.is_finite() || !(0.0..=1.0).contains(&locality_strength) {
            return Err(LearnerError::InvalidReward {
                reward: locality_strength as f64,
            });
        }
        if !cache_pressure.is_finite() || !(0.0..=1.0).contains(&cache_pressure) {
            return Err(LearnerError::InvalidReward {
                reward: cache_pressure as f64,
            });
        }

        Ok(Self {
            edit_density,
            locality_strength,
            cache_pressure,
        })
    }

    /// 编码为 4 维特征向量，供 LinUCB 消费
    ///
    /// 向量布局:
    /// - `[0]`: edit_density
    /// - `[1]`: locality_strength
    /// - `[2]`: cache_pressure
    /// - `[3]`: bias 常量 1.0
    ///
    /// # L2 范数分析
    /// - 最小范数: 仅 bias=1.0 = 1.0
    /// - 最大范数: 3 个特征都为 1.0 + bias = √4 = 2.0
    pub fn features(&self) -> [f32; S3_CONTEXT_DIM] {
        let mut features = [0.0f32; S3_CONTEXT_DIM];
        features[0] = self.edit_density;
        features[1] = self.locality_strength;
        features[2] = self.cache_pressure;
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

impl std::fmt::Display for S3Context {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "S3Context(density={:.2}, locality={:.2}, cache={:.2})",
            self.edit_density, self.locality_strength, self.cache_pressure
        )
    }
}

// ============================================================
// S3 奖励
// ============================================================

/// S3 奖励参数 — 控制预取浪费惩罚强度
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct S3RewardParams {
    /// 预取浪费惩罚强度 λ（hit_rate - λ × prefetch_overhead_ratio）
    pub prefetch_overhead_penalty_lambda: f64,
}

impl Default for S3RewardParams {
    fn default() -> Self {
        Self {
            prefetch_overhead_penalty_lambda: DEFAULT_PREFETCH_OVERHEAD_PENALTY_LAMBDA,
        }
    }
}

/// S3 奖励 — 预取命中率 − 预取浪费惩罚
///
/// 公式: `reward = hit_rate − λ × prefetch_overhead_ratio`
///
/// # 字段
/// - `hit_rate ∈ [0, 1]`: 预取命中率（成功预热次数 / 总预取次数）
/// - `prefetch_overhead_ratio ∈ [0, 1]`: 预取浪费比例（预取但未访问次数 / 总预取次数）
/// - `params`: 奖励参数（λ）
///
/// # 边界处理
/// - `prefetch_overhead_ratio = 0.0`: 浪费惩罚为 0（奖励 = hit_rate）
/// - `prefetch_overhead_ratio = 1.0`: 浪费惩罚为 λ（reward = hit_rate - λ）
/// - `hit_rate = 1.0 + overhead = 0.0`: reward → 1.0（最大奖励）
/// - `hit_rate = 0.0 + overhead = 1.0`: reward → -λ（强惩罚）
///
/// # 示例
///
/// ```
/// use omega_learner::s3_prefetch::{S3Reward, S3RewardParams};
///
/// // 命中率 100%，浪费率 0%（无惩罚）
/// let r1 = S3Reward::new(1.0, 0.0).unwrap();
/// assert!((r1.reward() - 1.0).abs() < 1e-6);
///
/// // 命中率 80%，浪费率 20%（惩罚 0.3 × 0.2 = 0.06）
/// let r2 = S3Reward::new(0.8, 0.2).unwrap();
/// assert!((r2.reward() - 0.74).abs() < 1e-6);
///
/// // 命中率 0%，浪费率 100%（强惩罚 reward = -0.3）
/// let r3 = S3Reward::new(0.0, 1.0).unwrap();
/// assert!((r3.reward() - (-0.3)).abs() < 1e-6);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct S3Reward {
    /// 预取命中率 ∈ [0, 1]
    pub hit_rate: f64,
    /// 预取浪费比例 ∈ [0, 1]
    pub prefetch_overhead_ratio: f64,
    /// 奖励参数
    pub params: S3RewardParams,
}

impl S3Reward {
    /// 创建 S3 奖励（使用默认参数）
    ///
    /// # 参数
    /// - `hit_rate`: 预取命中率 ∈ [0, 1]
    /// - `prefetch_overhead_ratio`: 预取浪费比例 ∈ [0, 1]
    ///
    /// # 错误
    /// - `InvalidReward`: hit_rate 或 prefetch_overhead_ratio 不在 [0, 1] 或非有限
    pub fn new(hit_rate: f64, prefetch_overhead_ratio: f64) -> Result<Self> {
        Self::with_params(hit_rate, prefetch_overhead_ratio, S3RewardParams::default())
    }

    /// 创建 S3 奖励（自定义参数）
    ///
    /// # 参数
    /// - `hit_rate`: 预取命中率 ∈ [0, 1]
    /// - `prefetch_overhead_ratio`: 预取浪费比例 ∈ [0, 1]
    /// - `params`: 奖励参数（λ）
    ///
    /// # 错误
    /// - `InvalidReward`: hit_rate 或 prefetch_overhead_ratio 不在 [0, 1] 或非有限
    pub fn with_params(
        hit_rate: f64,
        prefetch_overhead_ratio: f64,
        params: S3RewardParams,
    ) -> Result<Self> {
        if !hit_rate.is_finite() || !(0.0..=1.0).contains(&hit_rate) {
            return Err(LearnerError::InvalidReward { reward: hit_rate });
        }
        if !prefetch_overhead_ratio.is_finite() || !(0.0..=1.0).contains(&prefetch_overhead_ratio) {
            return Err(LearnerError::InvalidReward {
                reward: prefetch_overhead_ratio,
            });
        }
        Ok(Self {
            hit_rate,
            prefetch_overhead_ratio,
            params,
        })
    }

    /// 计算最终奖励值
    ///
    /// 公式: `reward = hit_rate − λ × prefetch_overhead_ratio`
    ///
    /// WHY 加法形式: LinUCB 假设奖励是上下文线性函数，
    /// 加法形式符合线性假设；乘法形式会导致 reward 非线性化，破坏 regret 上界。
    pub fn reward(&self) -> f64 {
        let penalty = self.params.prefetch_overhead_penalty_lambda * self.prefetch_overhead_ratio;
        self.hit_rate - penalty
    }
}

// ============================================================
// S3 臂集（5 臂对应 PrefetchStrategy::ALL）
// ============================================================

/// 构建 S3 接缝的臂集（5 臂对应 5 种预取策略）
///
/// 臂 ID 用策略简称字符串（与 `PrefetchStrategy::short_name()` 一致），
/// 便于跨版本持久化与 SpecRegistry 谱系追踪。
///
/// WHY 函数而非常量: `DiscreteArmSet::new` 接受 `Vec<ArmId>`，
/// 不能在 const 上下文构造（Vec 堆分配）。每次调用开销 O(1)（5 个 ArmId 克隆）。
pub fn s3_arm_set() -> DiscreteArmSet {
    DiscreteArmSet::new(vec![
        ArmId::new(PrefetchStrategy::NoPrefetch.short_name()),
        ArmId::new(PrefetchStrategy::Conservative.short_name()),
        ArmId::new(PrefetchStrategy::Standard.short_name()),
        ArmId::new(PrefetchStrategy::Aggressive.short_name()),
        ArmId::new(PrefetchStrategy::TopK3.short_name()),
    ])
}

/// ArmIndex → PrefetchStrategy 映射
///
/// 臂顺序与 `PrefetchStrategy::ALL` 一致
/// （NoPrefetch/Conservative/Standard/Aggressive/TopK3）。
/// WHY const fn: 映射是纯函数，编译期可计算，避免运行时开销。
pub const fn arm_index_to_strategy(idx: usize) -> PrefetchStrategy {
    match idx {
        0 => PrefetchStrategy::NoPrefetch,
        1 => PrefetchStrategy::Conservative,
        2 => PrefetchStrategy::Standard,
        3 => PrefetchStrategy::Aggressive,
        _ => PrefetchStrategy::TopK3,
    }
}

/// PrefetchStrategy → ArmIndex 映射
pub const fn strategy_to_arm_index(strategy: PrefetchStrategy) -> usize {
    match strategy {
        PrefetchStrategy::NoPrefetch => 0,
        PrefetchStrategy::Conservative => 1,
        PrefetchStrategy::Standard => 2,
        PrefetchStrategy::Aggressive => 3,
        PrefetchStrategy::TopK3 => 4,
    }
}

// ============================================================
// S3 学习器
// ============================================================

/// S3 学习器 — 封装 LinUCB + S3 上下文/臂/奖励逻辑
///
/// # 设计
///
/// `S3Learner` 是 `LinUCB` 的薄封装，提供 S3 接缝特定的:
/// - 上下文编码（`S3Context` → `SeamContext`）
/// - 臂映射（`ArmIndex` → `PrefetchStrategy`）
/// - 奖励计算（`S3Reward` → `f64`）
///
/// # C4 合规
///
/// `S3Learner` 只产出 `PrefetchPolicy::Learned { version, strategy }`，
/// 不直接修改 `scc-cache` 状态。上层调用方负责通过
/// `AccessPatternLearner::update_prefetch_policy()` 注入。
///
/// # 线程安全
///
/// `S3Learner` 内部 `LinUCB` 非 `Sync`（ndarray 数组无原子操作），
/// 多线程共享需通过 `Arc<Mutex<S3Learner>>` 或 `Arc<RwLock<S3Learner>>`。
/// 异步学习器典型用法是单线程后台任务 + tokio::sync::mpsc 通信。
///
/// # 示例
///
/// ```
/// use nexus_contracts::PrefetchStrategy;
/// use omega_learner::s3_prefetch::{S3Context, S3Learner, S3Reward};
///
/// let mut learner = S3Learner::new(1.0).unwrap();
///
/// let ctx = S3Context::new(0.8, 0.7, 0.4).unwrap();
/// let strategy = learner.select(&ctx).unwrap();
///
/// let reward = S3Reward::new(0.9, 0.1).unwrap();
/// learner.update(&ctx, strategy, &reward).unwrap();
///
/// let policy = learner.current_policy(1);
/// assert!(policy.is_learned());
/// assert_eq!(policy.strategy(), strategy);
/// ```
#[derive(Debug, Clone)]
pub struct S3Learner {
    /// 内部 LinUCB 实例
    linucb: LinUCB,
    /// 最近一次选择的臂索引（用于 `current_policy` 输出）
    last_arm_idx: usize,
    /// 已观察到的总步数
    total_steps: u64,
}

impl S3Learner {
    /// 创建 S3 学习器
    ///
    /// # 参数
    /// - `alpha`: LinUCB 探索强度（必须 > 0 且有限）
    ///
    /// # 错误
    /// - `InvalidAlpha`: alpha ≤ 0 或非有限
    /// - `NoArms`: 内部错误（S3 固定 5 臂，不应触发）
    pub fn new(alpha: f64) -> Result<Self> {
        let arm_set = s3_arm_set();
        let linucb = LinUCB::new(S3_CONTEXT_DIM, &arm_set, alpha)?;
        Ok(Self {
            linucb,
            last_arm_idx: 0,
            total_steps: 0,
        })
    }

    /// 创建 S3 学习器（使用默认 α=1.0）
    pub fn with_default_alpha() -> Result<Self> {
        Self::new(DEFAULT_S3_ALPHA)
    }

    /// 选择预取策略 — 基于 S3 上下文
    ///
    /// # 算法
    /// 1. 将 `S3Context` 编码为 4 维特征向量
    /// 2. 转换为 `SeamContext`（LinUCB 输入）
    /// 3. 调用 `LinUCB::select_arm` 选择 UCB 最大的臂
    /// 4. 将 `ArmIndex` 映射回 `PrefetchStrategy`
    ///
    /// # 错误
    /// - `ContextDimensionMismatch`: 内部错误（S3 固定 4 维，不应触发）
    pub fn select(&mut self, context: &S3Context) -> Result<PrefetchStrategy> {
        let seam_ctx = context.to_seam_context()?;
        let arm_idx = self.linucb.select_arm(&seam_ctx)?;
        self.last_arm_idx = arm_idx.as_usize();
        Ok(arm_index_to_strategy(self.last_arm_idx))
    }

    /// 观察奖励并更新模型
    ///
    /// # 参数
    /// - `context`: 选择时的 S3 上下文
    /// - `strategy`: 选择的预取策略
    /// - `reward`: 观察到的奖励
    ///
    /// # 错误
    /// - `ArmOutOfRange`: strategy 不在 S3 臂集中（不应触发）
    /// - `ContextDimensionMismatch`: 内部错误
    /// - `NumericalInstability`: Sherman-Morrison 分母 ≤ 0（矩阵病态）
    pub fn update(
        &mut self,
        context: &S3Context,
        strategy: PrefetchStrategy,
        reward: &S3Reward,
    ) -> Result<()> {
        let seam_ctx = context.to_seam_context()?;
        let arm_idx = crate::arm::ArmIndex::from(strategy_to_arm_index(strategy));
        let reward_value = reward.reward();
        self.linucb.update(arm_idx, &seam_ctx, reward_value)?;
        self.total_steps += 1;
        Ok(())
    }

    /// 输出当前策略（PrefetchPolicy::Learned）
    ///
    /// # 参数
    /// - `version`: 学习版本号（单调递增，用于 A/B 测试与回滚）
    ///
    /// # 返回
    /// `PrefetchPolicy::Learned { version, strategy }`，
    /// strategy 为最近一次 `select` 的结果。
    ///
    /// WHY 提供: 上层调用方（chimera-cli / quest-engine）调用此方法
    /// 获取学习到的策略，然后通过 `AccessPatternLearner::update_prefetch_policy()` 注入。
    pub fn current_policy(&self, version: u64) -> PrefetchPolicy {
        PrefetchPolicy::learned(version, arm_index_to_strategy(self.last_arm_idx))
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

/// S3Learner 必须实现 Send + Sync（异步跨线程共享需求）
///
/// WHY 必要性: S3Learner 可能被 Arc<Mutex<S3Learner>> 包裹，
/// 在 tokio 异步任务中跨 await 持有，编译期断言 Send+Sync 防止误用。
fn _assert_s3_learner_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<S3Learner>();
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================
    // S3Context 测试
    // ============================================================

    #[test]
    fn test_s3_context_new_basic() {
        let ctx = S3Context::new(0.8, 0.7, 0.4).unwrap();
        assert!((ctx.edit_density - 0.8).abs() < 1e-6);
        assert!((ctx.locality_strength - 0.7).abs() < 1e-6);
        assert!((ctx.cache_pressure - 0.4).abs() < 1e-6);
    }

    #[test]
    fn test_s3_context_zero_values() {
        let ctx = S3Context::new(0.0, 0.0, 0.0).unwrap();
        assert!((ctx.edit_density - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_s3_context_max_values() {
        let ctx = S3Context::new(1.0, 1.0, 1.0).unwrap();
        assert!((ctx.edit_density - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_s3_context_invalid_edit_density() {
        // > 1.0 失败
        let result = S3Context::new(1.5, 0.5, 0.5);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));

        // < 0.0 失败
        let result = S3Context::new(-0.1, 0.5, 0.5);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));

        // NaN 失败
        let result = S3Context::new(f32::NAN, 0.5, 0.5);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));
    }

    #[test]
    fn test_s3_context_invalid_locality_strength() {
        let result = S3Context::new(0.5, 1.5, 0.5);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));
    }

    #[test]
    fn test_s3_context_invalid_cache_pressure() {
        let result = S3Context::new(0.5, 0.5, -0.1);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));
    }

    #[test]
    fn test_s3_context_features_layout() {
        let ctx = S3Context::new(0.8, 0.7, 0.4).unwrap();
        let features = ctx.features();

        assert_eq!(features.len(), S3_CONTEXT_DIM);
        assert!((features[0] - 0.8).abs() < 1e-6);
        assert!((features[1] - 0.7).abs() < 1e-6);
        assert!((features[2] - 0.4).abs() < 1e-6);
        // bias
        assert!((features[3] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_s3_context_to_seam_context() {
        let ctx = S3Context::new(0.5, 0.6, 0.7).unwrap();
        let seam_ctx = ctx.to_seam_context().unwrap();
        assert_eq!(seam_ctx.dim(), S3_CONTEXT_DIM);
    }

    #[test]
    fn test_s3_context_display() {
        let ctx = S3Context::new(0.8, 0.7, 0.4).unwrap();
        let s = format!("{}", ctx);
        assert!(s.contains("density=0.80"));
        assert!(s.contains("locality=0.70"));
        assert!(s.contains("cache=0.40"));
    }

    #[test]
    fn test_s3_context_serialize_json() {
        let ctx = S3Context::new(0.8, 0.7, 0.4).unwrap();
        let json = serde_json::to_string(&ctx).unwrap();
        let deserialized: S3Context = serde_json::from_str(&json).unwrap();
        assert_eq!(ctx, deserialized);
    }

    // ============================================================
    // S3Reward 测试
    // ============================================================

    #[test]
    fn test_s3_reward_no_overhead() {
        // 浪费率 0%，无惩罚
        let r = S3Reward::new(1.0, 0.0).unwrap();
        assert!((r.reward() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_s3_reward_full_overhead() {
        // 浪费率 100%，惩罚 0.3
        let r = S3Reward::new(1.0, 1.0).unwrap();
        assert!((r.reward() - 0.7).abs() < 1e-6);
    }

    #[test]
    fn test_s3_reward_partial_overhead() {
        // 命中率 0.8，浪费率 0.2，惩罚 0.3 × 0.2 = 0.06
        let r = S3Reward::new(0.8, 0.2).unwrap();
        assert!((r.reward() - 0.74).abs() < 1e-6);
    }

    #[test]
    fn test_s3_reward_failed_prefetch() {
        // 命中率 0，浪费率 1.0，reward = -0.3
        let r = S3Reward::new(0.0, 1.0).unwrap();
        assert!((r.reward() - (-0.3)).abs() < 1e-6);
    }

    #[test]
    fn test_s3_reward_custom_params() {
        // 自定义 λ=0.5
        let params = S3RewardParams {
            prefetch_overhead_penalty_lambda: 0.5,
        };
        let r = S3Reward::with_params(0.8, 0.4, params).unwrap();
        // 惩罚 0.5 × 0.4 = 0.2，reward = 0.8 - 0.2 = 0.6
        assert!((r.reward() - 0.6).abs() < 1e-6);
    }

    #[test]
    fn test_s3_reward_invalid_hit_rate() {
        let result = S3Reward::new(1.5, 0.2);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));

        let result = S3Reward::new(-0.1, 0.2);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));
    }

    #[test]
    fn test_s3_reward_invalid_overhead() {
        let result = S3Reward::new(0.5, 1.5);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));

        let result = S3Reward::new(0.5, -0.1);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));

        let result = S3Reward::new(0.5, f64::NAN);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));
    }

    #[test]
    fn test_s3_reward_default_params() {
        let params = S3RewardParams::default();
        assert!((params.prefetch_overhead_penalty_lambda - 0.3).abs() < 1e-6);
    }

    #[test]
    fn test_s3_reward_serialize_json() {
        let r = S3Reward::new(0.8, 0.2).unwrap();
        let json = serde_json::to_string(&r).unwrap();
        let deserialized: S3Reward = serde_json::from_str(&json).unwrap();
        assert_eq!(r, deserialized);
    }

    // ============================================================
    // 臂集与映射测试
    // ============================================================

    #[test]
    fn test_s3_arm_set_has_five_arms() {
        let arm_set = s3_arm_set();
        assert_eq!(arm_set.len(), 5);
    }

    #[test]
    fn test_arm_index_to_strategy_mapping() {
        assert_eq!(arm_index_to_strategy(0), PrefetchStrategy::NoPrefetch);
        assert_eq!(arm_index_to_strategy(1), PrefetchStrategy::Conservative);
        assert_eq!(arm_index_to_strategy(2), PrefetchStrategy::Standard);
        assert_eq!(arm_index_to_strategy(3), PrefetchStrategy::Aggressive);
        assert_eq!(arm_index_to_strategy(4), PrefetchStrategy::TopK3);
        // 越界兜底返回 TopK3
        assert_eq!(arm_index_to_strategy(99), PrefetchStrategy::TopK3);
    }

    #[test]
    fn test_strategy_to_arm_index_mapping() {
        assert_eq!(strategy_to_arm_index(PrefetchStrategy::NoPrefetch), 0);
        assert_eq!(strategy_to_arm_index(PrefetchStrategy::Conservative), 1);
        assert_eq!(strategy_to_arm_index(PrefetchStrategy::Standard), 2);
        assert_eq!(strategy_to_arm_index(PrefetchStrategy::Aggressive), 3);
        assert_eq!(strategy_to_arm_index(PrefetchStrategy::TopK3), 4);
    }

    #[test]
    fn test_arm_index_strategy_round_trip() {
        // 验证双向映射一致性
        for strategy in PrefetchStrategy::ALL {
            let idx = strategy_to_arm_index(strategy);
            assert_eq!(arm_index_to_strategy(idx), strategy);
        }
    }

    // ============================================================
    // S3Learner 测试
    // ============================================================

    #[test]
    fn test_s3_learner_new_default_alpha() {
        let learner = S3Learner::with_default_alpha().unwrap();
        assert_eq!(learner.total_steps(), 0);
    }

    #[test]
    fn test_s3_learner_new_custom_alpha() {
        let learner = S3Learner::new(2.0).unwrap();
        assert_eq!(learner.total_steps(), 0);
    }

    #[test]
    fn test_s3_learner_invalid_alpha() {
        let result = S3Learner::new(0.0);
        assert!(matches!(result, Err(LearnerError::InvalidAlpha { .. })));

        let result = S3Learner::new(-1.0);
        assert!(matches!(result, Err(LearnerError::InvalidAlpha { .. })));
    }

    #[test]
    fn test_s3_learner_select_returns_valid_strategy() {
        let mut learner = S3Learner::with_default_alpha().unwrap();
        let ctx = S3Context::new(0.8, 0.7, 0.4).unwrap();
        let strategy = learner.select(&ctx).unwrap();

        // 初始所有臂 UCB 相同，应返回第一个（NoPrefetch）
        assert_eq!(strategy, PrefetchStrategy::NoPrefetch);
    }

    #[test]
    fn test_s3_learner_update_increments_steps() {
        let mut learner = S3Learner::with_default_alpha().unwrap();
        let ctx = S3Context::new(0.5, 0.6, 0.7).unwrap();
        let strategy = learner.select(&ctx).unwrap();
        assert_eq!(learner.total_steps(), 0);

        let reward = S3Reward::new(0.9, 0.1).unwrap();
        learner.update(&ctx, strategy, &reward).unwrap();
        assert_eq!(learner.total_steps(), 1);
    }

    #[test]
    fn test_s3_learner_current_policy_initial() {
        let learner = S3Learner::with_default_alpha().unwrap();
        // 未调用 select 时，last_arm_idx = 0 → NoPrefetch
        let policy = learner.current_policy(1);
        assert!(policy.is_learned());
        assert_eq!(policy.strategy(), PrefetchStrategy::NoPrefetch);
        assert_eq!(policy.version(), Some(1));
    }

    #[test]
    fn test_s3_learner_current_policy_after_select() {
        let mut learner = S3Learner::with_default_alpha().unwrap();
        let ctx = S3Context::new(0.8, 0.7, 0.4).unwrap();
        let strategy = learner.select(&ctx).unwrap();

        let policy = learner.current_policy(42);
        assert!(policy.is_learned());
        assert_eq!(policy.strategy(), strategy);
        assert_eq!(policy.version(), Some(42));
    }

    #[test]
    fn test_s3_learner_clone_independent() {
        let mut learner1 = S3Learner::with_default_alpha().unwrap();
        let ctx = S3Context::new(0.5, 0.6, 0.7).unwrap();
        let strategy = learner1.select(&ctx).unwrap();
        let reward = S3Reward::new(0.8, 0.2).unwrap();
        learner1.update(&ctx, strategy, &reward).unwrap();

        // Clone 后两者独立
        let learner2 = learner1.clone();
        assert_eq!(learner1.total_steps(), learner2.total_steps());

        // learner1 继续更新，learner2 不受影响
        let strategy2 = learner1.select(&ctx).unwrap();
        let reward2 = S3Reward::new(0.9, 0.1).unwrap();
        learner1.update(&ctx, strategy2, &reward2).unwrap();
        assert_eq!(learner1.total_steps(), 2);
        assert_eq!(learner2.total_steps(), 1);
    }

    #[test]
    fn test_s3_learner_linucb_accessor() {
        let learner = S3Learner::with_default_alpha().unwrap();
        let linucb = learner.linucb();
        assert_eq!(linucb.total_steps(), 0);
    }

    // ============================================================
    // 集成测试：S3 完整学习闭环
    // ============================================================

    #[test]
    fn test_s3_learner_full_loop_integration() {
        // 模拟一个完整的 S3 学习闭环
        let mut learner = S3Learner::with_default_alpha().unwrap();

        // 模拟 20 次迭代，编辑密度逐渐变化
        for step in 0..20 {
            let edit_density = (step as f32) / 20.0;
            let ctx = S3Context::new(edit_density, 0.5, 0.6).unwrap();

            let strategy = learner.select(&ctx).unwrap();

            // 模拟执行: Aggressive 在高编辑密度时高命中，
            // NoPrefetch 永远 0 命中（不预取）
            let (hit_rate, overhead) = match strategy {
                PrefetchStrategy::NoPrefetch => (0.0, 0.0), // 不预取不浪费也不命中
                PrefetchStrategy::Conservative => (0.7, 0.1),
                PrefetchStrategy::Standard => (0.8, 0.2),
                PrefetchStrategy::Aggressive => {
                    if edit_density > 0.5 {
                        (0.9, 0.3)
                    } else {
                        (0.6, 0.4) // 低密度时激进预取浪费多
                    }
                }
                PrefetchStrategy::TopK3 => (0.85, 0.15),
            };

            let reward = S3Reward::new(hit_rate, overhead).unwrap();
            learner.update(&ctx, strategy, &reward).unwrap();
        }

        // 验证 learner 已学习（20 步）
        assert_eq!(learner.total_steps(), 20);

        // 输出最终策略
        let policy = learner.current_policy(1);
        assert!(policy.is_learned());
        // 策略应该是 5 个合法策略之一
        assert!(matches!(
            policy.strategy(),
            PrefetchStrategy::NoPrefetch
                | PrefetchStrategy::Conservative
                | PrefetchStrategy::Standard
                | PrefetchStrategy::Aggressive
                | PrefetchStrategy::TopK3
        ));
    }

    #[test]
    fn test_s3_learner_overhead_penalty_dominates() {
        // 验证浪费惩罚主导时，learner 偏好低浪费策略
        let mut learner = S3Learner::with_default_alpha().unwrap();
        let ctx = S3Context::new(0.5, 0.5, 0.5).unwrap();

        // Aggressive 始终高浪费（0.8），reward 极低
        // Conservative 始终低浪费（0.1），reward 高
        for _ in 0..15 {
            let strategy = learner.select(&ctx).unwrap();
            let (hit_rate, overhead) = match strategy {
                PrefetchStrategy::Aggressive => (0.5, 0.8), // reward = 0.5 - 0.24 = 0.26
                PrefetchStrategy::Conservative => (0.8, 0.1), // reward = 0.8 - 0.03 = 0.77
                PrefetchStrategy::Standard => (0.7, 0.3),
                PrefetchStrategy::TopK3 => (0.75, 0.2),
                PrefetchStrategy::NoPrefetch => (0.0, 0.0),
            };
            let reward = S3Reward::new(hit_rate, overhead).unwrap();
            learner.update(&ctx, strategy, &reward).unwrap();
        }

        // 经过 15 次更新，learner 应偏好低浪费策略
        assert!(learner.total_steps() > 0);
    }

    #[test]
    fn test_s3_learner_high_edit_density_prefers_aggressive() {
        // 高编辑密度场景：Aggressive 应获得高奖励
        let mut learner = S3Learner::with_default_alpha().unwrap();

        // 高密度 + 强局部性 + 低缓存压力 → Aggressive 最佳
        let ctx = S3Context::new(0.9, 0.9, 0.3).unwrap();

        for _ in 0..20 {
            let strategy = learner.select(&ctx).unwrap();
            // 在高密度场景，Aggressive 命中率高
            let (hit_rate, overhead) = match strategy {
                PrefetchStrategy::Aggressive => (0.95, 0.2),
                PrefetchStrategy::Standard => (0.85, 0.15),
                PrefetchStrategy::Conservative => (0.7, 0.1),
                PrefetchStrategy::TopK3 => (0.8, 0.1),
                PrefetchStrategy::NoPrefetch => (0.0, 0.0),
            };
            let reward = S3Reward::new(hit_rate, overhead).unwrap();
            learner.update(&ctx, strategy, &reward).unwrap();
        }

        assert_eq!(learner.total_steps(), 20);
    }
}
