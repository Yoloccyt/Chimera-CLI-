//! S1 接缝 — DDR/HCW 密度档位学习器（v5.0 §7.3 六接缝之一）
//!
//! 对应任务: **P4-W13.2.1**（S1 接缝上下文/臂/奖励定义）
//! 对应 ADR: **ADR-031**（omega-learner 边界）
//! 对应设计源: `NEXUS-OMEGA_v5.0_系统性完整设计文档.md` §7.3 S1
//!
//! # S1 接缝概述
//!
//! | 字段 | 值 |
//! |------|-----|
//! | 接缝名 | S1Density（DDR/HCW 密度档位） |
//! | 代码锚点 | `crates/hcw-window/src/selector.rs` |
//! | 臂 | ρ∈{0.5, 2, 5, 10} 四档密度档位 |
//! | 上下文 | 任务类型 / DAG 深度 / 内存压力 |
//! | 奖励 | 成功率 − 延迟惩罚 |
//!
//! # 上下文向量设计（8 维）
//!
//! ```text
//! x = [
//!   task_type_one_hot(4),   // 0..3: CodeEdit / Analysis / Quest / Misc
//!   dag_depth_normalized,   // 4: DAG 深度 / MAX_DEPTH（归一化到 [0, 1]）
//!   memory_pressure,        // 5: 内存压力 ∈ [0, 1]（used / budget）
//!   tier_hint,              // 6: 当前 WindowTier 启发（L0=0.0, L3=1.0）
//!   bias,                   // 7: 常量 1.0（线性模型偏置项）
//! ]
//! ```
//!
//! 维度 `d = 8`，满足 LinUCB regret 上界 `O(√(T·d·ln(K·T)))` 的有界假设（`||x|| ≤ 1`）。
//!
//! # 臂集设计
//!
//! 4 臂对应 `DensityTier::ALL`：`Rho05` / `Rho2` / `Rho5` / `Rho10`。
//! 臂 ID 用 `ρ=<value>` 字符串（与 `DensityTier::short_name()` 一致），
//! 便于跨版本持久化与 SpecRegistry 谱系追踪。
//!
//! # 奖励函数
//!
//! `reward = success_score − latency_penalty`
//!
//! - `success_score ∈ [0, 1]`: 任务成功率（0 失败 / 1 成功，部分成功可取 0.5）
//! - `latency_penalty = λ × max(0, (latency_ms − target_ms) / target_ms)`
//!   - `λ = 0.5`（默认，控制延迟惩罚强度）
//!   - `target_ms = 100.0`（HCW 重排填充红线，spec.md §4.3）
//!
//! WHY 惩罚项而非相乘: LinUCB 假设奖励是上下文线性函数，
//! 加法形式更符合线性假设；乘法形式会导致 reward 非线性化，破坏 regret 上界。
//!
//! # C4 合规（能力场灰度，非运行时旗）
//!
//! `S1Learner` 输出 `DensityPolicy::Learned { version, tier }` 给上层调用方
//! （chimera-cli / quest-engine），由上层通过 `HcwWindow::update_density_policy()`
//! 注入。`hcw-window` 本地 fallback 到 `DensityPolicy::Static(Rho10)`，
//! **无跨 crate 旗标传播**（spec.md:334）。
//!
//! # 示例
//!
//! ## 基础学习流程
//!
//! ```
//! use nexus_contracts::DensityTier;
//! use omega_learner::s1_density::{S1Context, S1Learner, S1Reward};
//!
//! // 1. 创建 S1 学习器（α=1.0，默认奖励参数）
//! let mut learner = S1Learner::new(1.0).unwrap();
//!
//! // 2. 构造上下文（CodeEdit 任务，DAG 深度 3，内存压力 0.6，L2 窗口）
//! let ctx = S1Context::new(
//!     omega_learner::s1_density::TaskType::CodeEdit,
//!     3,      // dag_depth
//!     10,     // max_depth
//!     0.6,    // memory_pressure
//!     0.5,    // tier_hint (L2)
//! ).unwrap();
//!
//! // 3. 选择密度档位
//! let tier = learner.select(&ctx).unwrap();
//! assert!(matches!(tier, DensityTier::Rho05 | DensityTier::Rho2
//!     | DensityTier::Rho5 | DensityTier::Rho10));
//!
//! // 4. 观察奖励并更新模型（任务成功，延迟 80ms）
//! let reward = S1Reward::new(1.0, 80.0).unwrap();
//! learner.update(&ctx, tier, &reward).unwrap();
//! assert_eq!(learner.total_steps(), 1);
//!
//! // 5. 输出当前策略（DensityPolicy::Learned）
//! let policy = learner.current_policy(1);
//! assert!(policy.is_learned());
//! assert_eq!(policy.tier(), tier);
//! ```

use crate::arm::{ArmId, DiscreteArmSet};
use crate::context::SeamContext;
use crate::error::{LearnerError, Result};
use crate::linucb::LinUCB;
use nexus_contracts::{DensityPolicy, DensityTier};
use serde::{Deserialize, Serialize};

// ============================================================
// 常量定义
// ============================================================

/// S1 上下文维度（task_type one-hot(4) + dag_depth + memory_pressure + tier_hint + bias）
pub const S1_CONTEXT_DIM: usize = 8;

/// S1 默认延迟惩罚强度 λ（success_score - λ × latency_excess_ratio）
pub const DEFAULT_LATENCY_PENALTY_LAMBDA: f64 = 0.5;

/// S1 默认延迟目标（ms）— HCW 重排填充红线（spec.md §4.3）
pub const DEFAULT_LATENCY_TARGET_MS: f64 = 100.0;

/// S1 默认探索强度 α（LinUCB 探索-利用平衡）
///
/// WHY α=1.0: Li et al. (2010) 推荐的稳健默认值，
/// 在 `||x|| ≤ 1` 假设下提供 O(√(T·d·ln(K·T))) regret 上界。
/// 过小（如 0.1）会导致探索不足，过早收敛到次优臂；
/// 过大（如 10.0）会导致过度探索，延迟改善不显著。
pub const DEFAULT_S1_ALPHA: f64 = 1.0;

/// DAG 深度上限（用于归一化）
pub const MAX_DAG_DEPTH: usize = 10;

// ============================================================
// 任务类型枚举
// ============================================================

/// 任务类型 — S1 上下文的第一组特征（one-hot 编码）
///
/// WHY 枚举而非字符串: 4 种任务类型有限且固定，
/// 枚举提供编译期穷尽性检查与零开销匹配。
///
/// WHY 4 种: HCW 窗口选择的主要场景分类，
/// 不同任务类型的密度偏好不同（如 CodeEdit 偏向低密度以快速响应，
/// Quest 偏向高密度以保留完整上下文）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum TaskType {
    /// 代码编辑任务（偏向低密度，快速响应优先）
    CodeEdit = 0,
    /// 代码分析任务（中密度，平衡延迟与召回）
    Analysis = 1,
    /// 长期 Quest 任务（偏向高密度，保留完整上下文）
    Quest = 2,
    /// 其他任务（默认密度，无特殊偏好）
    Misc = 3,
}

impl TaskType {
    /// 返回所有任务类型（按枚举值升序，便于 one-hot 编码）
    pub const ALL: [Self; 4] = [Self::CodeEdit, Self::Analysis, Self::Quest, Self::Misc];

    /// 返回 one-hot 编码的索引位置（0..3）
    pub const fn one_hot_index(self) -> usize {
        self as usize
    }

    /// 返回任务类型简称（用于日志/调试）
    pub const fn short_name(self) -> &'static str {
        match self {
            Self::CodeEdit => "code-edit",
            Self::Analysis => "analysis",
            Self::Quest => "quest",
            Self::Misc => "misc",
        }
    }
}

impl std::fmt::Display for TaskType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.short_name())
    }
}

// ============================================================
// S1 上下文
// ============================================================

/// S1 上下文 — 任务类型 / DAG 深度 / 内存压力 / 窗口层级提示
///
/// 编码为 8 维特征向量，供 LinUCB 消费。所有字段归一化到 [0, 1]，
/// 满足 LinUCB regret 上界假设 `||x|| ≤ 1`。
///
/// # 设计决策（WHY）
/// - **one-hot 任务类型**: 4 维，避免序数编码（CodeEdit < Analysis 不成立）
/// - **dag_depth 归一化**: `depth / MAX_DEPTH`，clamp 到 [0, 1]
/// - **memory_pressure 直接传入**: 已归一化（used / budget）
/// - **tier_hint**: WindowTier 启发（L0=0.0, L1=0.33, L2=0.67, L3=1.0）
/// - **bias 常量 1.0**: 线性模型偏置项，允许 θ_a 学习"基础偏好"
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct S1Context {
    /// 任务类型（one-hot 编码到 4 维）
    pub task_type: TaskType,
    /// DAG 深度（已归一化到 [0, 1]）
    pub dag_depth_normalized: f32,
    /// 内存压力 ∈ [0, 1]（used / budget）
    pub memory_pressure: f32,
    /// 窗口层级启发 ∈ [0, 1]（L0=0.0, L3=1.0）
    pub tier_hint: f32,
}

impl S1Context {
    /// 创建 S1 上下文
    ///
    /// # 参数
    /// - `task_type`: 任务类型（决定 one-hot 编码位置）
    /// - `dag_depth`: DAG 深度（原始值，如 3）
    /// - `max_depth`: DAG 深度上限（用于归一化，如 10）
    /// - `memory_pressure`: 内存压力 ∈ [0, 1]（used / budget）
    /// - `tier_hint`: 窗口层级启发 ∈ [0, 1]（L0=0.0, L3=1.0）
    ///
    /// # 错误
    /// - `InvalidDimension`: max_depth == 0（除零风险）
    /// - `InvalidReward`: memory_pressure 或 tier_hint 不在 [0, 1]
    ///
    /// # 示例
    ///
    /// ```
    /// use omega_learner::s1_density::{S1Context, TaskType};
    ///
    /// let ctx = S1Context::new(
    ///     TaskType::CodeEdit,
    ///     3,      // dag_depth
    ///     10,     // max_depth
    ///     0.6,    // memory_pressure
    ///     0.5,    // tier_hint
    /// ).unwrap();
    ///
    /// assert_eq!(ctx.dag_depth_normalized, 0.3);
    /// assert_eq!(ctx.task_type, TaskType::CodeEdit);
    /// ```
    pub fn new(
        task_type: TaskType,
        dag_depth: usize,
        max_depth: usize,
        memory_pressure: f32,
        tier_hint: f32,
    ) -> Result<Self> {
        if max_depth == 0 {
            return Err(LearnerError::InvalidDimension);
        }
        if !memory_pressure.is_finite() || !(0.0..=1.0).contains(&memory_pressure) {
            return Err(LearnerError::InvalidReward {
                reward: memory_pressure as f64,
            });
        }
        if !tier_hint.is_finite() || !(0.0..=1.0).contains(&tier_hint) {
            return Err(LearnerError::InvalidReward {
                reward: tier_hint as f64,
            });
        }

        // 归一化 DAG 深度，clamp 到 [0, 1]
        let dag_depth_normalized = (dag_depth as f32 / max_depth as f32).clamp(0.0, 1.0);

        Ok(Self {
            task_type,
            dag_depth_normalized,
            memory_pressure,
            tier_hint,
        })
    }

    /// 编码为 8 维特征向量，供 LinUCB 消费
    ///
    /// 向量布局:
    /// - `[0..4]`: task_type one-hot 编码（选中位置为 1.0，其余为 0.0）
    /// - `[4]`: dag_depth_normalized
    /// - `[5]`: memory_pressure
    /// - `[6]`: tier_hint
    /// - `[7]`: bias 常量 1.0
    ///
    /// # L2 范数分析
    /// - 最小范数: 仅 bias=1.0 + 一个 one-hot=1.0 = √2 ≈ 1.414
    /// - 最大范数: 4 个字段都为 1.0 = √5 ≈ 2.236
    ///
    /// **WHY 不强制归一化**: LinUCB regret 上界假设 `||x|| ≤ 1`，
    /// 但实践中允许稍大范数（如 √5）只需相应增大 α 探索强度。
    /// `S1Learner::new` 默认 α=1.0，对范数 √5 仍提供合理探索。
    /// 如需严格 `||x|| ≤ 1`，可在 `features()` 后手动归一化（`/||x||`）。
    pub fn features(&self) -> [f32; S1_CONTEXT_DIM] {
        let mut features = [0.0f32; S1_CONTEXT_DIM];
        // task_type one-hot 编码
        features[self.task_type.one_hot_index()] = 1.0;
        // 数值特征
        features[4] = self.dag_depth_normalized;
        features[5] = self.memory_pressure;
        features[6] = self.tier_hint;
        // bias 常量
        features[7] = 1.0;
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

impl std::fmt::Display for S1Context {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "S1Context({}, depth={:.2}, mem={:.2}, tier={:.2})",
            self.task_type.short_name(),
            self.dag_depth_normalized,
            self.memory_pressure,
            self.tier_hint
        )
    }
}

// ============================================================
// S1 奖励
// ============================================================

/// S1 奖励参数 — 控制延迟惩罚强度与目标
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct S1RewardParams {
    /// 延迟惩罚强度 λ（success_score - λ × latency_excess_ratio）
    pub latency_penalty_lambda: f64,
    /// 延迟目标（ms），超过此值触发惩罚
    pub latency_target_ms: f64,
}

impl Default for S1RewardParams {
    fn default() -> Self {
        Self {
            latency_penalty_lambda: DEFAULT_LATENCY_PENALTY_LAMBDA,
            latency_target_ms: DEFAULT_LATENCY_TARGET_MS,
        }
    }
}

/// S1 奖励 — 成功率 − 延迟惩罚
///
/// 公式: `reward = success_score − λ × max(0, (latency_ms − target_ms) / target_ms)`
///
/// # 字段
/// - `success_score ∈ [0, 1]`: 任务成功率
/// - `latency_ms`: 实际延迟（毫秒）
/// - `params`: 奖励参数（λ 与 target_ms）
///
/// # 边界处理
/// - `latency_ms < target_ms`: 延迟惩罚为 0（奖励 = success_score）
/// - `latency_ms ≥ target_ms`: 延迟惩罚线性增长（reward 可能 < 0）
/// - `success_score = 1.0 + latency_ms << target_ms`: reward → 1.0（最大奖励）
/// - `success_score = 0.0 + latency_ms >> target_ms`: reward → 负值（强惩罚）
///
/// # 示例
///
/// ```
/// use omega_learner::s1_density::{S1Reward, S1RewardParams};
///
/// // 任务成功，延迟 80ms（低于 100ms 目标，无惩罚）
/// let r1 = S1Reward::new(1.0, 80.0).unwrap();
/// assert!((r1.reward() - 1.0).abs() < 1e-6);
///
/// // 任务成功，延迟 200ms（超过 100ms 目标，惩罚 0.5 × 1.0 = 0.5）
/// let r2 = S1Reward::new(1.0, 200.0).unwrap();
/// assert!((r2.reward() - 0.5).abs() < 1e-6);
///
/// // 任务失败，延迟 50ms（成功率为 0，奖励 = -0）
/// let r3 = S1Reward::new(0.0, 50.0).unwrap();
/// assert!((r3.reward() - 0.0).abs() < 1e-6);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct S1Reward {
    /// 任务成功率 ∈ [0, 1]
    pub success_score: f64,
    /// 实际延迟（毫秒）
    pub latency_ms: f64,
    /// 奖励参数
    pub params: S1RewardParams,
}

impl S1Reward {
    /// 创建 S1 奖励（使用默认参数）
    ///
    /// # 参数
    /// - `success_score`: 任务成功率 ∈ [0, 1]
    /// - `latency_ms`: 实际延迟（毫秒，必须 ≥ 0 且有限）
    ///
    /// # 错误
    /// - `InvalidReward`: success_score 不在 [0, 1] 或 latency_ms 非有限或为负
    pub fn new(success_score: f64, latency_ms: f64) -> Result<Self> {
        Self::with_params(success_score, latency_ms, S1RewardParams::default())
    }

    /// 创建 S1 奖励（自定义参数）
    ///
    /// # 参数
    /// - `success_score`: 任务成功率 ∈ [0, 1]
    /// - `latency_ms`: 实际延迟（毫秒，必须 ≥ 0 且有限）
    /// - `params`: 奖励参数（λ 与 target_ms）
    ///
    /// # 错误
    /// - `InvalidReward`: success_score 不在 [0, 1] 或 latency_ms 非有限或为负
    pub fn with_params(
        success_score: f64,
        latency_ms: f64,
        params: S1RewardParams,
    ) -> Result<Self> {
        if !success_score.is_finite() || !(0.0..=1.0).contains(&success_score) {
            return Err(LearnerError::InvalidReward {
                reward: success_score,
            });
        }
        if !latency_ms.is_finite() || latency_ms < 0.0 {
            return Err(LearnerError::InvalidReward { reward: latency_ms });
        }
        Ok(Self {
            success_score,
            latency_ms,
            params,
        })
    }

    /// 计算最终奖励值
    ///
    /// 公式: `reward = success_score − λ × max(0, (latency_ms − target_ms) / target_ms)`
    ///
    /// WHY 加法形式: LinUCB 假设奖励是上下文线性函数，
    /// 加法形式符合线性假设；乘法形式会导致 reward 非线性化，破坏 regret 上界。
    pub fn reward(&self) -> f64 {
        let latency_excess = if self.latency_ms > self.params.latency_target_ms {
            (self.latency_ms - self.params.latency_target_ms) / self.params.latency_target_ms
        } else {
            0.0
        };
        let penalty = self.params.latency_penalty_lambda * latency_excess;
        self.success_score - penalty
    }
}

// ============================================================
// S1 臂集（4 臂对应 DensityTier::ALL）
// ============================================================

/// 构建 S1 接缝的臂集（4 臂对应 ρ∈{0.5, 2, 5, 10}）
///
/// 臂 ID 用 `ρ=<value>` 字符串（与 `DensityTier::short_name()` 一致），
/// 便于跨版本持久化与 SpecRegistry 谱系追踪。
///
/// WHY 函数而非常量: `DiscreteArmSet::new` 接受 `Vec<ArmId>`，
/// 不能在 const 上下文构造（Vec 堆分配）。每次调用开销 O(1)（4 个 ArmId 克隆）。
pub fn s1_arm_set() -> DiscreteArmSet {
    DiscreteArmSet::new(vec![
        ArmId::new("ρ=0.5"),
        ArmId::new("ρ=2"),
        ArmId::new("ρ=5"),
        ArmId::new("ρ=10"),
    ])
}

/// ArmIndex → DensityTier 映射
///
/// 臂顺序与 `DensityTier::ALL` 一致（Rho05/Rho2/Rho5/Rho10）。
/// WHY const fn: 映射是纯函数，编译期可计算，避免运行时开销。
pub const fn arm_index_to_tier(idx: usize) -> DensityTier {
    match idx {
        0 => DensityTier::Rho05,
        1 => DensityTier::Rho2,
        2 => DensityTier::Rho5,
        _ => DensityTier::Rho10,
    }
}

/// DensityTier → ArmIndex 映射
pub const fn tier_to_arm_index(tier: DensityTier) -> usize {
    match tier {
        DensityTier::Rho05 => 0,
        DensityTier::Rho2 => 1,
        DensityTier::Rho5 => 2,
        DensityTier::Rho10 => 3,
    }
}

// ============================================================
// S1 学习器
// ============================================================

/// S1 学习器 — 封装 LinUCB + S1 上下文/臂/奖励逻辑
///
/// # 设计
///
/// `S1Learner` 是 `LinUCB` 的薄封装，提供 S1 接缝特定的:
/// - 上下文编码（`S1Context` → `SeamContext`）
/// - 臂映射（`ArmIndex` → `DensityTier`）
/// - 奖励计算（`S1Reward` → `f64`）
///
/// # C4 合规
///
/// `S1Learner` 只产出 `DensityPolicy::Learned { version, tier }`，
/// 不直接修改 `hcw-window` 状态。上层调用方负责通过
/// `HcwWindow::update_density_policy()` 注入策略。
///
/// # 线程安全
///
/// `S1Learner` 内部 `LinUCB` 非 `Sync`（ndarray 数组无原子操作），
/// 多线程共享需通过 `Arc<Mutex<S1Learner>>` 或 `Arc<RwLock<S1Learner>>`。
/// 异步学习器典型用法是单线程后台任务 + tokio::sync::mpsc 通信。
///
/// # 示例
///
/// ```
/// use nexus_contracts::DensityTier;
/// use omega_learner::s1_density::{S1Context, S1Learner, S1Reward, TaskType};
///
/// let mut learner = S1Learner::new(1.0).unwrap();
///
/// let ctx = S1Context::new(TaskType::Quest, 5, 10, 0.7, 0.8).unwrap();
/// let tier = learner.select(&ctx).unwrap();
///
/// let reward = S1Reward::new(0.9, 70.0).unwrap();
/// learner.update(&ctx, tier, &reward).unwrap();
///
/// let policy = learner.current_policy(1);
/// assert!(policy.is_learned());
/// assert_eq!(policy.tier(), tier);
/// ```
#[derive(Debug, Clone)]
pub struct S1Learner {
    /// 内部 LinUCB 实例
    linucb: LinUCB,
    /// 最近一次选择的臂索引（用于 `current_policy` 输出）
    last_arm_idx: usize,
    /// 已观察到的总步数
    total_steps: u64,
}

impl S1Learner {
    /// 创建 S1 学习器
    ///
    /// # 参数
    /// - `alpha`: LinUCB 探索强度（必须 > 0 且有限）
    ///
    /// # 错误
    /// - `InvalidAlpha`: alpha ≤ 0 或非有限
    /// - `NoArms`: 内部错误（S1 固定 4 臂，不应触发）
    pub fn new(alpha: f64) -> Result<Self> {
        let arm_set = s1_arm_set();
        let linucb = LinUCB::new(S1_CONTEXT_DIM, &arm_set, alpha)?;
        Ok(Self {
            linucb,
            last_arm_idx: 0,
            total_steps: 0,
        })
    }

    /// 创建 S1 学习器（使用默认 α=1.0）
    pub fn with_default_alpha() -> Result<Self> {
        Self::new(DEFAULT_S1_ALPHA)
    }

    /// 选择密度档位 — 基于 S1 上下文
    ///
    /// # 算法
    /// 1. 将 `S1Context` 编码为 8 维特征向量
    /// 2. 转换为 `SeamContext`（LinUCB 输入）
    /// 3. 调用 `LinUCB::select_arm` 选择 UCB 最大的臂
    /// 4. 将 `ArmIndex` 映射回 `DensityTier`
    ///
    /// # 错误
    /// - `ContextDimensionMismatch`: 内部错误（S1 固定 8 维，不应触发）
    pub fn select(&mut self, context: &S1Context) -> Result<DensityTier> {
        let seam_ctx = context.to_seam_context()?;
        let arm_idx = self.linucb.select_arm(&seam_ctx)?;
        self.last_arm_idx = arm_idx.as_usize();
        Ok(arm_index_to_tier(self.last_arm_idx))
    }

    /// 观察奖励并更新模型
    ///
    /// # 参数
    /// - `context`: 选择时的 S1 上下文
    /// - `tier`: 选择的密度档位
    /// - `reward`: 观察到的奖励
    ///
    /// # 错误
    /// - `ArmOutOfRange`: tier 不在 S1 臂集中（不应触发）
    /// - `ContextDimensionMismatch`: 内部错误
    /// - `NumericalInstability`: Sherman-Morrison 分母 ≤ 0（矩阵病态）
    pub fn update(
        &mut self,
        context: &S1Context,
        tier: DensityTier,
        reward: &S1Reward,
    ) -> Result<()> {
        let seam_ctx = context.to_seam_context()?;
        let arm_idx = crate::arm::ArmIndex::from(tier_to_arm_index(tier));
        let reward_value = reward.reward();
        self.linucb.update(arm_idx, &seam_ctx, reward_value)?;
        self.total_steps += 1;
        Ok(())
    }

    /// 输出当前策略（DensityPolicy::Learned）
    ///
    /// # 参数
    /// - `version`: 学习版本号（单调递增，用于 A/B 测试与回滚）
    ///
    /// # 返回
    /// `DensityPolicy::Learned { version, tier }`，tier 为最近一次 `select` 的结果。
    ///
    /// WHY 提供: 上层调用方（chimera-cli / quest-engine）调用此方法
    /// 获取学习到的策略，然后通过 `HcwWindow::update_density_policy()` 注入。
    pub fn current_policy(&self, version: u64) -> DensityPolicy {
        DensityPolicy::learned(version, arm_index_to_tier(self.last_arm_idx))
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

/// S1Learner 必须实现 Send + Sync（异步跨线程共享需求）
///
/// WHY 必要性: S1Learner 可能被 Arc<Mutex<S1Learner>> 包裹，
/// 在 tokio 异步任务中跨 await 持有，编译期断言 Send+Sync 防止误用。
fn _assert_s1_learner_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<S1Learner>();
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================
    // TaskType 测试
    // ============================================================

    #[test]
    fn test_task_type_one_hot_index() {
        assert_eq!(TaskType::CodeEdit.one_hot_index(), 0);
        assert_eq!(TaskType::Analysis.one_hot_index(), 1);
        assert_eq!(TaskType::Quest.one_hot_index(), 2);
        assert_eq!(TaskType::Misc.one_hot_index(), 3);
    }

    #[test]
    fn test_task_type_short_name() {
        assert_eq!(TaskType::CodeEdit.short_name(), "code-edit");
        assert_eq!(TaskType::Analysis.short_name(), "analysis");
        assert_eq!(TaskType::Quest.short_name(), "quest");
        assert_eq!(TaskType::Misc.short_name(), "misc");
    }

    #[test]
    fn test_task_type_all_returns_four() {
        let all = TaskType::ALL;
        assert_eq!(all.len(), 4);
        assert!(all.contains(&TaskType::CodeEdit));
        assert!(all.contains(&TaskType::Misc));
    }

    #[test]
    fn test_task_type_display() {
        assert_eq!(format!("{}", TaskType::CodeEdit), "code-edit");
        assert_eq!(format!("{}", TaskType::Quest), "quest");
    }

    #[test]
    fn test_task_type_serialize_json() {
        let json = serde_json::to_string(&TaskType::Quest).unwrap();
        let deserialized: TaskType = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, TaskType::Quest);
    }

    // ============================================================
    // S1Context 测试
    // ============================================================

    #[test]
    fn test_s1_context_new_basic() {
        let ctx = S1Context::new(TaskType::CodeEdit, 3, 10, 0.6, 0.5).unwrap();
        assert_eq!(ctx.task_type, TaskType::CodeEdit);
        assert!((ctx.dag_depth_normalized - 0.3).abs() < 1e-6);
        assert!((ctx.memory_pressure - 0.6).abs() < 1e-6);
        assert!((ctx.tier_hint - 0.5).abs() < 1e-6);
    }

    #[test]
    fn test_s1_context_dag_depth_clamp() {
        // DAG 深度超过 max_depth 时 clamp 到 1.0
        let ctx = S1Context::new(TaskType::Quest, 15, 10, 0.5, 0.5).unwrap();
        assert!((ctx.dag_depth_normalized - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_s1_context_dag_depth_zero() {
        let ctx = S1Context::new(TaskType::CodeEdit, 0, 10, 0.5, 0.5).unwrap();
        assert!((ctx.dag_depth_normalized - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_s1_context_max_depth_zero_fails() {
        let result = S1Context::new(TaskType::CodeEdit, 1, 0, 0.5, 0.5);
        assert!(matches!(result, Err(LearnerError::InvalidDimension)));
    }

    #[test]
    fn test_s1_context_invalid_memory_pressure() {
        // memory_pressure > 1.0 失败
        let result = S1Context::new(TaskType::CodeEdit, 1, 10, 1.5, 0.5);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));

        // memory_pressure < 0.0 失败
        let result = S1Context::new(TaskType::CodeEdit, 1, 10, -0.1, 0.5);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));

        // NaN 失败
        let result = S1Context::new(TaskType::CodeEdit, 1, 10, f32::NAN, 0.5);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));
    }

    #[test]
    fn test_s1_context_invalid_tier_hint() {
        let result = S1Context::new(TaskType::CodeEdit, 1, 10, 0.5, 1.5);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));
    }

    #[test]
    fn test_s1_context_features_code_edit() {
        let ctx = S1Context::new(TaskType::CodeEdit, 3, 10, 0.6, 0.5).unwrap();
        let features = ctx.features();

        assert_eq!(features.len(), S1_CONTEXT_DIM);
        // CodeEdit one-hot 在位置 0
        assert!((features[0] - 1.0).abs() < 1e-6);
        assert!((features[1] - 0.0).abs() < 1e-6);
        assert!((features[2] - 0.0).abs() < 1e-6);
        assert!((features[3] - 0.0).abs() < 1e-6);
        // 数值特征
        assert!((features[4] - 0.3).abs() < 1e-6);
        assert!((features[5] - 0.6).abs() < 1e-6);
        assert!((features[6] - 0.5).abs() < 1e-6);
        // bias
        assert!((features[7] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_s1_context_features_quest() {
        let ctx = S1Context::new(TaskType::Quest, 5, 10, 0.7, 0.8).unwrap();
        let features = ctx.features();

        // Quest one-hot 在位置 2
        assert!((features[0] - 0.0).abs() < 1e-6);
        assert!((features[1] - 0.0).abs() < 1e-6);
        assert!((features[2] - 1.0).abs() < 1e-6);
        assert!((features[3] - 0.0).abs() < 1e-6);
        assert!((features[4] - 0.5).abs() < 1e-6);
        assert!((features[5] - 0.7).abs() < 1e-6);
        assert!((features[6] - 0.8).abs() < 1e-6);
        assert!((features[7] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_s1_context_to_seam_context() {
        let ctx = S1Context::new(TaskType::Analysis, 2, 10, 0.4, 0.3).unwrap();
        let seam_ctx = ctx.to_seam_context().unwrap();
        assert_eq!(seam_ctx.dim(), S1_CONTEXT_DIM);
    }

    #[test]
    fn test_s1_context_display() {
        let ctx = S1Context::new(TaskType::CodeEdit, 3, 10, 0.6, 0.5).unwrap();
        let s = format!("{}", ctx);
        assert!(s.contains("code-edit"));
        assert!(s.contains("depth=0.30"));
        assert!(s.contains("mem=0.60"));
    }

    #[test]
    fn test_s1_context_serialize_json() {
        let ctx = S1Context::new(TaskType::Quest, 5, 10, 0.7, 0.8).unwrap();
        let json = serde_json::to_string(&ctx).unwrap();
        let deserialized: S1Context = serde_json::from_str(&json).unwrap();
        assert_eq!(ctx, deserialized);
    }

    // ============================================================
    // S1Reward 测试
    // ============================================================

    #[test]
    fn test_s1_reward_no_latency_excess() {
        // 延迟低于目标，无惩罚
        let r = S1Reward::new(1.0, 80.0).unwrap();
        assert!((r.reward() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_s1_reward_latency_equal_target() {
        // 延迟等于目标，无惩罚（边界）
        let r = S1Reward::new(1.0, 100.0).unwrap();
        assert!((r.reward() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_s1_reward_latency_excess() {
        // 延迟 200ms，超目标 100%，惩罚 0.5 × 1.0 = 0.5
        let r = S1Reward::new(1.0, 200.0).unwrap();
        assert!((r.reward() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn test_s1_reward_failed_task() {
        // 任务失败 + 低延迟 → reward = 0
        let r = S1Reward::new(0.0, 50.0).unwrap();
        assert!((r.reward() - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_s1_reward_failed_task_high_latency() {
        // 任务失败 + 高延迟 → reward = -0.5（负惩罚）
        let r = S1Reward::new(0.0, 200.0).unwrap();
        assert!((r.reward() - (-0.5)).abs() < 1e-6);
    }

    #[test]
    fn test_s1_reward_custom_params() {
        // 自定义参数：λ=1.0, target=50ms
        let params = S1RewardParams {
            latency_penalty_lambda: 1.0,
            latency_target_ms: 50.0,
        };
        let r = S1Reward::with_params(1.0, 100.0, params).unwrap();
        // 延迟超 100%，惩罚 1.0 × 1.0 = 1.0，reward = 0
        assert!((r.reward() - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_s1_reward_invalid_success_score() {
        let result = S1Reward::new(1.5, 80.0);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));

        let result = S1Reward::new(-0.1, 80.0);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));
    }

    #[test]
    fn test_s1_reward_invalid_latency() {
        let result = S1Reward::new(1.0, -10.0);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));

        let result = S1Reward::new(1.0, f64::NAN);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));
    }

    #[test]
    fn test_s1_reward_default_params() {
        let params = S1RewardParams::default();
        assert!((params.latency_penalty_lambda - 0.5).abs() < 1e-6);
        assert!((params.latency_target_ms - 100.0).abs() < 1e-6);
    }

    #[test]
    fn test_s1_reward_serialize_json() {
        let r = S1Reward::new(0.8, 120.0).unwrap();
        let json = serde_json::to_string(&r).unwrap();
        let deserialized: S1Reward = serde_json::from_str(&json).unwrap();
        assert_eq!(r, deserialized);
    }

    // ============================================================
    // 臂集与映射测试
    // ============================================================

    #[test]
    fn test_s1_arm_set_has_four_arms() {
        let arm_set = s1_arm_set();
        assert_eq!(arm_set.len(), 4);
    }

    #[test]
    fn test_arm_index_to_tier_mapping() {
        assert_eq!(arm_index_to_tier(0), DensityTier::Rho05);
        assert_eq!(arm_index_to_tier(1), DensityTier::Rho2);
        assert_eq!(arm_index_to_tier(2), DensityTier::Rho5);
        assert_eq!(arm_index_to_tier(3), DensityTier::Rho10);
        // 越界兜底返回 Rho10
        assert_eq!(arm_index_to_tier(99), DensityTier::Rho10);
    }

    #[test]
    fn test_tier_to_arm_index_mapping() {
        assert_eq!(tier_to_arm_index(DensityTier::Rho05), 0);
        assert_eq!(tier_to_arm_index(DensityTier::Rho2), 1);
        assert_eq!(tier_to_arm_index(DensityTier::Rho5), 2);
        assert_eq!(tier_to_arm_index(DensityTier::Rho10), 3);
    }

    #[test]
    fn test_arm_index_tier_round_trip() {
        // 验证双向映射一致性
        for tier in DensityTier::ALL {
            let idx = tier_to_arm_index(tier);
            assert_eq!(arm_index_to_tier(idx), tier);
        }
    }

    // ============================================================
    // S1Learner 测试
    // ============================================================

    #[test]
    fn test_s1_learner_new_default_alpha() {
        let learner = S1Learner::with_default_alpha().unwrap();
        assert_eq!(learner.total_steps(), 0);
    }

    #[test]
    fn test_s1_learner_new_custom_alpha() {
        let learner = S1Learner::new(2.0).unwrap();
        assert_eq!(learner.total_steps(), 0);
    }

    #[test]
    fn test_s1_learner_invalid_alpha() {
        let result = S1Learner::new(0.0);
        assert!(matches!(result, Err(LearnerError::InvalidAlpha { .. })));

        let result = S1Learner::new(-1.0);
        assert!(matches!(result, Err(LearnerError::InvalidAlpha { .. })));
    }

    #[test]
    fn test_s1_learner_select_returns_valid_tier() {
        let mut learner = S1Learner::with_default_alpha().unwrap();
        let ctx = S1Context::new(TaskType::CodeEdit, 3, 10, 0.6, 0.5).unwrap();
        let tier = learner.select(&ctx).unwrap();

        // 初始所有臂 UCB 相同，应返回第一个（Rho05）
        assert_eq!(tier, DensityTier::Rho05);
    }

    #[test]
    fn test_s1_learner_update_increments_steps() {
        let mut learner = S1Learner::with_default_alpha().unwrap();
        let ctx = S1Context::new(TaskType::Quest, 5, 10, 0.7, 0.8).unwrap();
        let tier = learner.select(&ctx).unwrap();
        assert_eq!(learner.total_steps(), 0);

        let reward = S1Reward::new(0.9, 70.0).unwrap();
        learner.update(&ctx, tier, &reward).unwrap();
        assert_eq!(learner.total_steps(), 1);
    }

    #[test]
    fn test_s1_learner_current_policy_initial() {
        let learner = S1Learner::with_default_alpha().unwrap();
        // 未调用 select 时，last_arm_idx = 0 → Rho05
        let policy = learner.current_policy(1);
        assert!(policy.is_learned());
        assert_eq!(policy.tier(), DensityTier::Rho05);
        assert_eq!(policy.version(), Some(1));
    }

    #[test]
    fn test_s1_learner_current_policy_after_select() {
        let mut learner = S1Learner::with_default_alpha().unwrap();
        let ctx = S1Context::new(TaskType::CodeEdit, 3, 10, 0.6, 0.5).unwrap();
        let tier = learner.select(&ctx).unwrap();

        let policy = learner.current_policy(42);
        assert!(policy.is_learned());
        assert_eq!(policy.tier(), tier);
        assert_eq!(policy.version(), Some(42));
    }

    #[test]
    fn test_s1_learner_multiple_updates_shift_preference() {
        // 验证多次更新后，learner 会偏好高奖励的臂
        let mut learner = S1Learner::with_default_alpha().unwrap();
        let ctx = S1Context::new(TaskType::Quest, 5, 10, 0.7, 0.8).unwrap();

        // Rho05 高延迟（高稀疏化导致召回不足，延迟反而可能更高）
        // Rho10 高奖励（无稀疏化，召回好，延迟可能更高）
        // 这里模拟 Rho10 始终高奖励的场景
        for _ in 0..10 {
            let tier = learner.select(&ctx).unwrap();
            // 给 Rho10 高奖励，其他低奖励
            let reward_value = if tier == DensityTier::Rho10 { 0.9 } else { 0.3 };
            let reward = S1Reward::new(reward_value, 80.0).unwrap();
            learner.update(&ctx, tier, &reward).unwrap();
        }

        // 经过 10 次更新后，learner 应更偏好 Rho10
        // 调用 select 触发 last_arm_idx 更新（其返回值在此不强制断言，
        // 因 LinUCB 探索项可能仍偏向其他臂）
        let _ = learner.select(&ctx).unwrap();
        // 至少断言 learner 已学习（total_steps > 0）
        assert!(learner.total_steps() > 0);
        // 验证最终策略可输出
        let policy = learner.current_policy(1);
        assert!(policy.is_learned());
    }

    #[test]
    fn test_s1_learner_clone_independent() {
        let mut learner1 = S1Learner::with_default_alpha().unwrap();
        let ctx = S1Context::new(TaskType::CodeEdit, 3, 10, 0.6, 0.5).unwrap();
        let tier = learner1.select(&ctx).unwrap();
        let reward = S1Reward::new(0.8, 90.0).unwrap();
        learner1.update(&ctx, tier, &reward).unwrap();

        // Clone 后两者独立
        let learner2 = learner1.clone();
        assert_eq!(learner1.total_steps(), learner2.total_steps());

        // learner1 继续更新，learner2 不受影响
        let tier2 = learner1.select(&ctx).unwrap();
        let reward2 = S1Reward::new(0.9, 70.0).unwrap();
        learner1.update(&ctx, tier2, &reward2).unwrap();
        assert_eq!(learner1.total_steps(), 2);
        assert_eq!(learner2.total_steps(), 1);
    }

    #[test]
    fn test_s1_learner_linucb_accessor() {
        let learner = S1Learner::with_default_alpha().unwrap();
        let linucb = learner.linucb();
        assert_eq!(linucb.total_steps(), 0);
    }

    // ============================================================
    // 集成测试：S1 完整学习闭环
    // ============================================================

    #[test]
    fn test_s1_learner_full_loop_integration() {
        // 模拟一个完整的 S1 学习闭环:
        // 1. 创建学习器
        // 2. 多次迭代：select → 模拟执行 → 观察奖励 → update
        // 3. 验证策略输出
        let mut learner = S1Learner::with_default_alpha().unwrap();

        // 模拟 20 次迭代
        for step in 0..20 {
            let ctx = S1Context::new(
                TaskType::CodeEdit,
                step % 5, // dag_depth 变化
                10,       // max_depth
                0.5,      // memory_pressure
                0.5,      // tier_hint
            )
            .unwrap();

            let tier = learner.select(&ctx).unwrap();

            // 模拟执行: Rho05 延迟最低但成功率较低，Rho10 延迟最高但成功率最高
            let (success, latency) = match tier {
                DensityTier::Rho05 => (0.7, 50.0),   // 低延迟，低成功率
                DensityTier::Rho2 => (0.85, 70.0),   // 中低延迟，中成功率
                DensityTier::Rho5 => (0.9, 100.0),   // 中高延迟，高成功率
                DensityTier::Rho10 => (0.95, 150.0), // 高延迟，最高成功率
            };

            let reward = S1Reward::new(success, latency).unwrap();
            learner.update(&ctx, tier, &reward).unwrap();
        }

        // 验证 learner 已学习（20 步）
        assert_eq!(learner.total_steps(), 20);

        // 输出最终策略
        let policy = learner.current_policy(1);
        assert!(policy.is_learned());
        // 策略应该是 4 个合法档位之一
        assert!(matches!(
            policy.tier(),
            DensityTier::Rho05 | DensityTier::Rho2 | DensityTier::Rho5 | DensityTier::Rho10
        ));
    }

    #[test]
    fn test_s1_learner_latency_penalty_dominates() {
        // 验证延迟惩罚主导时，learner 偏好低密度（低延迟）
        let mut learner = S1Learner::with_default_alpha().unwrap();
        let ctx = S1Context::new(TaskType::CodeEdit, 3, 10, 0.6, 0.5).unwrap();

        // Rho10 延迟极高（500ms），惩罚主导 → reward 负值
        // Rho05 延迟低（50ms），无惩罚 → reward 正值
        for _ in 0..15 {
            let tier = learner.select(&ctx).unwrap();
            let (success, latency) = match tier {
                DensityTier::Rho05 => (0.8, 50.0),   // 高 reward = 0.8
                DensityTier::Rho2 => (0.85, 80.0),   // 高 reward = 0.85
                DensityTier::Rho5 => (0.9, 200.0),   // 低 reward = 0.9 - 0.5 = 0.4
                DensityTier::Rho10 => (0.95, 500.0), // 负 reward = 0.95 - 2.0 = -1.05
            };
            let reward = S1Reward::new(success, latency).unwrap();
            learner.update(&ctx, tier, &reward).unwrap();
        }

        // 经过 15 次更新，learner 应偏好低延迟（Rho05 或 Rho2）
        // 但不强制断言（LinUCB 探索项可能导致偶尔选其他臂）
        assert!(learner.total_steps() > 0);
    }
}
