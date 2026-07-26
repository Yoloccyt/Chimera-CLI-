//! S6 接缝 — decay-engine 衰减参数学习器（LinUCB 上下文线性 bandit）
//!
//! 对应任务: **P4-W14.4.1**（S6 接缝上下文/臂/奖励定义）
//! 对应 ADR: **ADR-031**（omega-learner 边界）
//! 对应设计源: `NEXUS-OMEGA_v5.0_系统性完整设计文档.md` §7.3 S6
//!
//! # S6 接缝概述
//!
//! | 字段 | 值 |
//! |------|-----|
//! | 接缝名 | S6DecayProfile（衰减参数档位） |
//! | 代码锚点 | `crates/decay-engine/src/` |
//! | 臂 | 4 种衰减档位（Lenient/Standard/Strict/Aggressive） |
//! | 上下文 | 操作类型 + 风险信号密度 + 历史违规率 |
//! | 奖励 | -(误拦率 × W_block + 漏拦率 × W_pass) |
//!
//! # 上下文向量设计（7 维）
//!
//! ```text
//! x = [
//!   op_type_read_only_onehot,  // 0: 操作类型 one-hot（只读）
//!   op_type_write_onehot,      // 1: 操作类型 one-hot（写）
//!   op_type_exec_onehot,       // 2: 操作类型 one-hot（执行）
//!   op_type_sandbox_onehot,    // 3: 操作类型 one-hot（沙箱）
//!   risk_signal_density,       // 4: 风险信号密度 ∈ [0, 1]
//!   historical_violation_rate, // 5: 历史违规率 ∈ [0, 1]
//!   bias,                       // 6: 常量 1.0（线性模型偏置项）
//! ]
//! ```
//!
//! 维度 `d = 7`，满足 LinUCB regret 上界 `O(√(T·d·ln(K·T)))` 的有界假设。
//!
//! WHY 7 维而非更少: decay-engine 的衰减参数需要根据"操作类型"自适应
//! （只读操作不应被快速冻结，写操作需要严格衰减），one-hot 编码保证
//! LinUCB 能学习到每种类型的独立偏好。同时"风险信号密度"与"历史违规率"
//! 提供连续信号辅助决策。
//!
//! # 臂集设计
//!
//! 4 臂对应 `DecayProfile::ALL`：
//! `Lenient` / `Standard` / `Strict` / `Aggressive`。
//! 臂 ID 用档位简称字符串（与 `DecayProfile::short_name()` 一致），
//! 便于跨版本持久化与 SpecRegistry 谱系追踪。
//!
//! # 奖励函数
//!
//! `reward = -(W_block × false_block_rate + W_pass × false_pass_rate)`
//!
//! - `false_block_rate ∈ [0, 1]`: 误拦率（合法操作被错误冻结）
//! - `false_pass_rate ∈ [0, 1]`: 漏拦率（违规操作未被冻结）
//! - `W_block = 0.3`（默认，误拦代价低，重新尝试即可）
//! - `W_pass = 0.7`（默认，漏拦代价高，安全风险）
//!
//! WHY 加法形式而非相乘: LinUCB 假设奖励是上下文线性函数，
//! 加法形式符合线性假设；乘法形式会导致 reward 非线性化，破坏 regret 上界。
//!
//! WHY 负号: 误拦与漏拦都是错误率（越大越糟），取负后最大化 reward
//! 等价于最小化加权错误率。LinUCB 的 regret 上界对奖励符号无要求。
//!
//! # 三重悖论"进化悖论"修复路径
//!
//! 三重悖论病理：验证器层级 L3（执行反馈）的"测试通过/失败"信号可被游戏化，
//! decay-engine 的衰减参数选择是典型的"验证器边界"权衡：
//! - 误拦（false_block）：合法操作被错误冻结（生产力损失）
//! - 漏拦（false_pass）：违规操作未被冻结（安全风险）
//!
//! S6 接缝通过 LinUCB 学习上下文特征 → 档位映射，使衰减强度随场景自适应，
//! 避免 Lenient 时违规操作漏拦（安全风险）与 Aggressive 时合法操作误拦
//! （生产力损失）。
//!
//! # C4 合规（能力场灰度，非运行时旗）
//!
//! `S6Learner` 输出 `DecayPolicy::Learned { version, profile }` 给上层调用方
//! （chimera-cli / quest-engine），由上层通过
//! `DecayLearnerHolder::update_policy()` 注入。`decay-engine` 本地
//! fallback 到 `DecayPolicy::Static(DecayProfile::Standard)`，
//! **无跨 crate 旗标传播**（spec.md:334）。
//!
//! # 示例
//!
//! ## 基础学习流程
//!
//! ```
//! use nexus_contracts::DecayProfile;
//! use omega_learner::s6_decay::{OperationType, S6Context, S6Learner, S6Reward};
//!
//! // 1. 创建 S6 学习器（α=1.0，默认奖励参数）
//! let mut learner = S6Learner::new(1.0).unwrap();
//!
//! // 2. 构造上下文（写操作，风险信号密度 0.7，历史违规率 0.3）
//! let ctx = S6Context::new(OperationType::Write, 0.7, 0.3).unwrap();
//!
//! // 3. 选择衰减档位
//! let profile = learner.select(&ctx).unwrap();
//! assert!(matches!(profile, DecayProfile::Lenient
//!     | DecayProfile::Standard
//!     | DecayProfile::Strict
//!     | DecayProfile::Aggressive));
//!
//! // 4. 观察奖励并更新模型（误拦率 0.1，漏拦率 0.05）
//! let reward = S6Reward::new(0.1, 0.05).unwrap();
//! learner.update(&ctx, profile, &reward).unwrap();
//! assert_eq!(learner.total_steps(), 1);
//!
//! // 5. 输出当前策略（DecayPolicy::Learned）
//! let policy = learner.current_policy(1);
//! assert!(policy.is_learned());
//! assert_eq!(policy.profile(), profile);
//! ```

use crate::arm::{ArmId, DiscreteArmSet};
use crate::context::SeamContext;
use crate::error::{LearnerError, Result};
use crate::linucb::LinUCB;
use nexus_contracts::{DecayPolicy, DecayProfile};
use serde::{Deserialize, Serialize};

// ============================================================
// 常量定义
// ============================================================

/// S6 上下文维度（4 one-hot + risk_signal_density + historical_violation_rate + bias）
pub const S6_CONTEXT_DIM: usize = 7;

/// S6 默认误拦权重 W_block（false_block_rate × W_block）
///
/// WHY W_block=0.3: 误拦代价低（合法操作被错误冻结，重新尝试即可），
/// 比 S5 的辩论成本 λ=0.5 更低（误拦 ≠ 辩论成本，仅是暂时生产力损失）。
/// - W_block=0.5 过强：learner 可能过度偏向 Lenient 导致漏拦安全风险
/// - W_block=0.1 过弱：对误拦不够敏感，可能过度偏向 Aggressive
pub const DEFAULT_FALSE_BLOCK_WEIGHT: f64 = 0.3;

/// S6 默认漏拦权重 W_pass（false_pass_rate × W_pass）
///
/// WHY W_pass=0.7: 漏拦代价高（违规操作未被冻结，安全风险），
/// 应严格惩罚。W_block + W_pass = 1.0 保证加权归一化。
pub const DEFAULT_FALSE_PASS_WEIGHT: f64 = 0.7;

/// S6 默认探索强度 α（LinUCB 探索-利用平衡）
///
/// WHY α=1.0: 与 S1/S2/S3/S4/S5 保持一致，Li et al. (2010) 推荐的稳健默认值，
/// 在合理范数假设下提供 O(√(T·d·ln(K·T))) regret 上界。
pub const DEFAULT_S6_ALPHA: f64 = 1.0;

/// S6 臂数（4 种衰减档位）
pub const S6_ARM_COUNT: usize = 4;

// ============================================================
// OperationType 枚举（上下文输入）
// ============================================================

/// 操作类型 — S6 上下文核心信号
///
/// 决定衰减参数档位的选择倾向：
/// - `ReadOnly`: 倾向 Lenient（误拦代价高，慢衰减）
/// - `Write`: 倾向 Strict（误拦代价低，快衰减）
/// - `Exec`: 倾向 Standard（中间档位）
/// - `Sandbox`: 倾向 Standard（沙箱环境已隔离，无需过严）
///
/// # 设计决策（WHY 枚举而非字符串）
/// - 编译期穷尽性检查，避免拼写错误
/// - 4 种类型有限且固定，枚举比字符串更安全
/// - one-hot 编码便于 LinUCB 消费（避免序数编码引入虚假序关系）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum OperationType {
    /// 只读操作（如读文件、查询）— 倾向 Lenient
    ReadOnly = 1,

    /// 写操作（如写文件、修改配置）— 倾向 Strict
    Write = 2,

    /// 执行操作（如运行命令、shell exec）— 倾向 Standard
    Exec = 3,

    /// 沙箱操作（如沙箱内运行）— 倾向 Standard
    Sandbox = 4,
}

impl OperationType {
    /// 所有操作类型（按枚举值升序）
    pub const ALL: [Self; 4] = [Self::ReadOnly, Self::Write, Self::Exec, Self::Sandbox];

    /// 返回 one-hot 编码（4 维向量）
    pub const fn onehot(self) -> [f32; 4] {
        match self {
            Self::ReadOnly => [1.0, 0.0, 0.0, 0.0],
            Self::Write => [0.0, 1.0, 0.0, 0.0],
            Self::Exec => [0.0, 0.0, 1.0, 0.0],
            Self::Sandbox => [0.0, 0.0, 0.0, 1.0],
        }
    }

    /// 返回简称（用于日志/调试）
    pub const fn short_name(self) -> &'static str {
        match self {
            Self::ReadOnly => "read-only",
            Self::Write => "write",
            Self::Exec => "exec",
            Self::Sandbox => "sandbox",
        }
    }
}

impl std::fmt::Display for OperationType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.short_name())
    }
}

// ============================================================
// S6 上下文
// ============================================================

/// S6 上下文 — 操作类型 / 风险信号密度 / 历史违规率
///
/// 编码为 7 维特征向量，供 LinUCB 消费。数值字段归一化到 [0, 1]，
/// 满足 LinUCB regret 上界假设 `||x||` 有界。
///
/// # 设计决策（WHY）
/// - **one-hot 编码 operation_type**: 4 种类型无序关系（ReadOnly < Write 不成立），
///   one-hot 避免序数编码引入虚假序关系
/// - **risk_signal_density 归一化**: 风险信号密度 ∈ [0, 1]，调用方负责归一化
///   （如近期违规事件数 / 总操作数）
/// - **historical_violation_rate 直接传入**: 已归一化（历史违规次数 / 总操作次数）
/// - **bias 常量 1.0**: 线性模型偏置项，允许 θ_a 学习"基础偏好"
///
/// # L2 范数分析
/// - 最小范数: 仅 bias=1.0 = 1.0
/// - 最大范数: one-hot 1.0 + 2 个特征 1.0 + bias 1.0 = √4 = 2.0
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct S6Context {
    /// 操作类型（one-hot 编码到 4 维）
    pub operation_type: OperationType,
    /// 风险信号密度 ∈ [0, 1]（高密度 → 倾向 Aggressive）
    pub risk_signal_density: f32,
    /// 历史违规率 ∈ [0, 1]（高违规率 → 倾向 Strict/Aggressive）
    pub historical_violation_rate: f32,
}

impl S6Context {
    /// 创建 S6 上下文
    ///
    /// # 参数
    /// - `operation_type`: 操作类型（ReadOnly/Write/Exec/Sandbox）
    /// - `risk_signal_density`: 风险信号密度 ∈ [0, 1]（调用方归一化）
    /// - `historical_violation_rate`: 历史违规率 ∈ [0, 1]
    ///
    /// # 错误
    /// - `InvalidReward`: 任一数值字段不在 [0, 1] 或非有限
    pub fn new(
        operation_type: OperationType,
        risk_signal_density: f32,
        historical_violation_rate: f32,
    ) -> Result<Self> {
        if !risk_signal_density.is_finite() || !(0.0..=1.0).contains(&risk_signal_density) {
            return Err(LearnerError::InvalidReward {
                reward: risk_signal_density as f64,
            });
        }
        if !historical_violation_rate.is_finite()
            || !(0.0..=1.0).contains(&historical_violation_rate)
        {
            return Err(LearnerError::InvalidReward {
                reward: historical_violation_rate as f64,
            });
        }

        Ok(Self {
            operation_type,
            risk_signal_density,
            historical_violation_rate,
        })
    }

    /// 编码为 7 维特征向量，供 LinUCB 消费
    ///
    /// 向量布局:
    /// - `[0..4]`: operation_type one-hot 编码
    /// - `[4]`: risk_signal_density
    /// - `[5]`: historical_violation_rate
    /// - `[6]`: bias 常量 1.0
    pub fn features(&self) -> [f32; S6_CONTEXT_DIM] {
        let mut features = [0.0f32; S6_CONTEXT_DIM];
        let onehot = self.operation_type.onehot();
        features[0] = onehot[0];
        features[1] = onehot[1];
        features[2] = onehot[2];
        features[3] = onehot[3];
        features[4] = self.risk_signal_density;
        features[5] = self.historical_violation_rate;
        features[6] = 1.0; // bias
        features
    }

    /// 转换为 `SeamContext`（LinUCB 输入）
    pub fn to_seam_context(&self) -> Result<SeamContext> {
        SeamContext::new(self.features().to_vec())
    }
}

impl std::fmt::Display for S6Context {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "S6Context(op={}, risk={:.2}, violation={:.2})",
            self.operation_type, self.risk_signal_density, self.historical_violation_rate
        )
    }
}

// ============================================================
// S6 奖励
// ============================================================

/// S6 奖励参数 — 控制误拦与漏拦的权重
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct S6RewardParams {
    /// 误拦权重 W_block（false_block_rate × W_block）
    pub false_block_weight: f64,
    /// 漏拦权重 W_pass（false_pass_rate × W_pass）
    pub false_pass_weight: f64,
}

impl Default for S6RewardParams {
    fn default() -> Self {
        Self {
            false_block_weight: DEFAULT_FALSE_BLOCK_WEIGHT,
            false_pass_weight: DEFAULT_FALSE_PASS_WEIGHT,
        }
    }
}

/// S6 奖励 — -(误拦率 × W_block + 漏拦率 × W_pass)
///
/// 公式: `reward = -(W_block × false_block_rate + W_pass × false_pass_rate)`
///
/// # 字段
/// - `false_block_rate ∈ [0, 1]`: 误拦率（合法操作被错误冻结）
/// - `false_pass_rate ∈ [0, 1]`: 漏拦率（违规操作未被冻结）
/// - `params`: 奖励参数（W_block, W_pass）
///
/// # 边界处理
/// - `false_block=0.0 + false_pass=0.0`（完美）: reward = 0.0（最大奖励）
/// - `false_block=1.0 + false_pass=1.0`（最差）: reward = -1.0（最小奖励）
/// - `false_block=0.0 + false_pass=1.0`（全漏拦）: reward = -W_pass = -0.7
/// - `false_block=1.0 + false_pass=0.0`（全误拦）: reward = -W_block = -0.3
///
/// # 示例
///
/// ```
/// use omega_learner::s6_decay::S6Reward;
///
/// // 完美档位（无误拦无漏拦）= 0.0 奖励
/// let r1 = S6Reward::new(0.0, 0.0).unwrap();
/// assert!(r1.reward().abs() < 1e-6);
///
/// // 全漏拦 = -0.7（默认 W_pass=0.7）
/// let r2 = S6Reward::new(0.0, 1.0).unwrap();
/// assert!((r2.reward() - (-0.7)).abs() < 1e-6);
///
/// // 全误拦 = -0.3（默认 W_block=0.3）
/// let r3 = S6Reward::new(1.0, 0.0).unwrap();
/// assert!((r3.reward() - (-0.3)).abs() < 1e-6);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct S6Reward {
    /// 误拦率 ∈ [0, 1]（合法操作被错误冻结）
    pub false_block_rate: f64,
    /// 漏拦率 ∈ [0, 1]（违规操作未被冻结）
    pub false_pass_rate: f64,
    /// 奖励参数
    pub params: S6RewardParams,
}

impl S6Reward {
    /// 创建 S6 奖励（使用默认参数）
    ///
    /// # 参数
    /// - `false_block_rate`: 误拦率 ∈ [0, 1]
    /// - `false_pass_rate`: 漏拦率 ∈ [0, 1]
    ///
    /// # 错误
    /// - `InvalidReward`: 任一字段不在 [0, 1] 或非有限
    pub fn new(false_block_rate: f64, false_pass_rate: f64) -> Result<Self> {
        Self::with_params(false_block_rate, false_pass_rate, S6RewardParams::default())
    }

    /// 创建 S6 奖励（自定义参数）
    ///
    /// # 参数
    /// - `false_block_rate`: 误拦率 ∈ [0, 1]
    /// - `false_pass_rate`: 漏拦率 ∈ [0, 1]
    /// - `params`: 奖励参数（W_block, W_pass）
    ///
    /// # 错误
    /// - `InvalidReward`: 任一字段不在 [0, 1] 或非有限
    pub fn with_params(
        false_block_rate: f64,
        false_pass_rate: f64,
        params: S6RewardParams,
    ) -> Result<Self> {
        if !false_block_rate.is_finite() || !(0.0..=1.0).contains(&false_block_rate) {
            return Err(LearnerError::InvalidReward {
                reward: false_block_rate,
            });
        }
        if !false_pass_rate.is_finite() || !(0.0..=1.0).contains(&false_pass_rate) {
            return Err(LearnerError::InvalidReward {
                reward: false_pass_rate,
            });
        }
        Ok(Self {
            false_block_rate,
            false_pass_rate,
            params,
        })
    }

    /// 计算最终奖励值
    ///
    /// 公式: `reward = -(W_block × false_block_rate + W_pass × false_pass_rate)`
    ///
    /// WHY 加法形式: LinUCB 假设奖励是上下文线性函数，
    /// 加法形式符合线性假设；乘法形式会导致 reward 非线性化，破坏 regret 上界。
    ///
    /// WHY 负号: 误拦与漏拦都是错误率（越大越糟），取负后最大化 reward
    /// 等价于最小化加权错误率。
    pub fn reward(&self) -> f64 {
        let penalty = self.params.false_block_weight * self.false_block_rate
            + self.params.false_pass_weight * self.false_pass_rate;
        -penalty
    }
}

// ============================================================
// S6 臂集（4 臂对应 DecayProfile::ALL）
// ============================================================

/// 构建 S6 接缝的臂集（4 臂对应 4 种衰减档位）
///
/// 臂 ID 用档位简称字符串（与 `DecayProfile::short_name()` 一致），
/// 便于跨版本持久化与 SpecRegistry 谱系追踪。
pub fn s6_arm_set() -> DiscreteArmSet {
    DiscreteArmSet::new(vec![
        ArmId::new(DecayProfile::Lenient.short_name()),
        ArmId::new(DecayProfile::Standard.short_name()),
        ArmId::new(DecayProfile::Strict.short_name()),
        ArmId::new(DecayProfile::Aggressive.short_name()),
    ])
}

/// ArmIndex → DecayProfile 映射
///
/// 臂顺序与 `DecayProfile::ALL` 一致
/// （Lenient/Standard/Strict/Aggressive）。
pub const fn arm_index_to_profile(idx: usize) -> DecayProfile {
    match idx {
        0 => DecayProfile::Lenient,
        1 => DecayProfile::Standard,
        2 => DecayProfile::Strict,
        _ => DecayProfile::Aggressive,
    }
}

/// DecayProfile → ArmIndex 映射
pub const fn profile_to_arm_index(profile: DecayProfile) -> usize {
    match profile {
        DecayProfile::Lenient => 0,
        DecayProfile::Standard => 1,
        DecayProfile::Strict => 2,
        DecayProfile::Aggressive => 3,
    }
}

// ============================================================
// S6 学习器
// ============================================================

/// S6 学习器 — 封装 LinUCB + S6 上下文/臂/奖励逻辑
///
/// # 设计
///
/// `S6Learner` 是 `LinUCB` 的薄封装，提供 S6 接缝特定的:
/// - 上下文编码（`S6Context` → `SeamContext`）
/// - 臂映射（`ArmIndex` → `DecayProfile`）
/// - 奖励计算（`S6Reward` → `f64`）
///
/// # C4 合规
///
/// `S6Learner` 只产出 `DecayPolicy::Learned { version, profile }`，
/// 不直接修改 `decay-engine` 状态。上层调用方负责通过
/// `DecayLearnerHolder::update_policy()` 注入。
///
/// # 线程安全
///
/// `S6Learner` 内部 `LinUCB` 非 `Sync`（ndarray 数组无原子操作），
/// 多线程共享需通过 `Arc<Mutex<S6Learner>>` 或 `Arc<RwLock<S6Learner>>`。
/// 异步学习器典型用法是单线程后台任务 + tokio::sync::mpsc 通信。
///
/// # 示例
///
/// ```
/// use nexus_contracts::DecayProfile;
/// use omega_learner::s6_decay::{OperationType, S6Context, S6Learner, S6Reward};
///
/// let mut learner = S6Learner::new(1.0).unwrap();
///
/// let ctx = S6Context::new(OperationType::Write, 0.7, 0.3).unwrap();
/// let profile = learner.select(&ctx).unwrap();
///
/// let reward = S6Reward::new(0.1, 0.05).unwrap();
/// learner.update(&ctx, profile, &reward).unwrap();
///
/// let policy = learner.current_policy(1);
/// assert!(policy.is_learned());
/// assert_eq!(policy.profile(), profile);
/// ```
#[derive(Debug, Clone)]
pub struct S6Learner {
    /// 内部 LinUCB 实例
    linucb: LinUCB,
    /// 最近一次选择的臂索引（用于 `current_policy` 输出）
    last_arm_idx: usize,
    /// 已观察到的总步数
    total_steps: u64,
}

impl S6Learner {
    /// 创建 S6 学习器
    ///
    /// # 参数
    /// - `alpha`: LinUCB 探索强度（必须 > 0 且有限）
    ///
    /// # 错误
    /// - `InvalidAlpha`: alpha ≤ 0 或非有限
    /// - `NoArms`: 内部错误（S6 固定 4 臂，不应触发）
    pub fn new(alpha: f64) -> Result<Self> {
        let arm_set = s6_arm_set();
        let linucb = LinUCB::new(S6_CONTEXT_DIM, &arm_set, alpha)?;
        Ok(Self {
            linucb,
            last_arm_idx: 0,
            total_steps: 0,
        })
    }

    /// 创建 S6 学习器（使用默认 α=1.0）
    pub fn with_default_alpha() -> Result<Self> {
        Self::new(DEFAULT_S6_ALPHA)
    }

    /// 选择衰减档位 — 基于 S6 上下文
    ///
    /// # 算法
    /// 1. 将 `S6Context` 编码为 7 维特征向量
    /// 2. 转换为 `SeamContext`（LinUCB 输入）
    /// 3. 调用 `LinUCB::select_arm` 选择 UCB 最大的臂
    /// 4. 将 `ArmIndex` 映射回 `DecayProfile`
    pub fn select(&mut self, context: &S6Context) -> Result<DecayProfile> {
        let seam_ctx = context.to_seam_context()?;
        let arm_idx = self.linucb.select_arm(&seam_ctx)?;
        self.last_arm_idx = arm_idx.as_usize();
        Ok(arm_index_to_profile(self.last_arm_idx))
    }

    /// 观察奖励并更新模型
    ///
    /// # 参数
    /// - `context`: 选择时的 S6 上下文
    /// - `profile`: 选择的衰减档位
    /// - `reward`: 观察到的奖励
    pub fn update(
        &mut self,
        context: &S6Context,
        profile: DecayProfile,
        reward: &S6Reward,
    ) -> Result<()> {
        let seam_ctx = context.to_seam_context()?;
        let arm_idx = crate::arm::ArmIndex::from(profile_to_arm_index(profile));
        let reward_value = reward.reward();
        self.linucb.update(arm_idx, &seam_ctx, reward_value)?;
        self.total_steps += 1;
        Ok(())
    }

    /// 输出当前策略（DecayPolicy::Learned）
    ///
    /// # 参数
    /// - `version`: 学习版本号（单调递增，用于 A/B 测试与回滚）
    ///
    /// # 返回
    /// `DecayPolicy::Learned { version, profile }`，
    /// profile 为最近一次 `select` 的结果。
    pub fn current_policy(&self, version: u64) -> DecayPolicy {
        DecayPolicy::learned(version, arm_index_to_profile(self.last_arm_idx))
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

/// S6Learner 必须实现 Send + Sync（异步跨线程共享需求）
fn _assert_s6_learner_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<S6Learner>();
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    // P4-W14.4 测试补丁: `arm_set.size()` 是 `ArmSet` trait 方法，trait 不在 `super::*` 范围内
    // （`super` = `s6_decay` 模块，仅 import 了 `DiscreteArmSet` 类型而非 trait），
    // 需显式导入 `ArmSet` trait 才能在测试中调用 `size()` 方法。
    // 此前 S1-S5 测试因不同原因避开了此路径，S6 新引入 `arm_set.size()` 断言时首遇。
    use crate::arm::ArmSet;

    // ============================================================
    // OperationType 测试
    // ============================================================

    #[test]
    fn test_operation_type_all_count() {
        assert_eq!(OperationType::ALL.len(), 4);
    }

    #[test]
    fn test_operation_type_short_names() {
        assert_eq!(OperationType::ReadOnly.short_name(), "read-only");
        assert_eq!(OperationType::Write.short_name(), "write");
        assert_eq!(OperationType::Exec.short_name(), "exec");
        assert_eq!(OperationType::Sandbox.short_name(), "sandbox");
    }

    #[test]
    fn test_operation_type_onehot() {
        assert_eq!(OperationType::ReadOnly.onehot(), [1.0, 0.0, 0.0, 0.0]);
        assert_eq!(OperationType::Write.onehot(), [0.0, 1.0, 0.0, 0.0]);
        assert_eq!(OperationType::Exec.onehot(), [0.0, 0.0, 1.0, 0.0]);
        assert_eq!(OperationType::Sandbox.onehot(), [0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn test_operation_type_display() {
        assert_eq!(format!("{}", OperationType::ReadOnly), "read-only");
        assert_eq!(format!("{}", OperationType::Write), "write");
    }

    // ============================================================
    // S6Context 测试
    // ============================================================

    #[test]
    fn test_s6_context_new_basic() {
        let ctx = S6Context::new(OperationType::Write, 0.7, 0.3).unwrap();
        assert_eq!(ctx.operation_type, OperationType::Write);
        assert!((ctx.risk_signal_density - 0.7).abs() < 1e-6);
        assert!((ctx.historical_violation_rate - 0.3).abs() < 1e-6);
    }

    #[test]
    fn test_s6_context_zero_values() {
        let ctx = S6Context::new(OperationType::ReadOnly, 0.0, 0.0).unwrap();
        assert!(ctx.risk_signal_density.abs() < 1e-6);
    }

    #[test]
    fn test_s6_context_max_values() {
        let ctx = S6Context::new(OperationType::Exec, 1.0, 1.0).unwrap();
        assert!((ctx.risk_signal_density - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_s6_context_invalid_risk() {
        let result = S6Context::new(OperationType::Write, 1.5, 0.3);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));
    }

    #[test]
    fn test_s6_context_invalid_violation_rate() {
        let result = S6Context::new(OperationType::Write, 0.5, -0.1);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));
    }

    #[test]
    fn test_s6_context_nan_risk() {
        let result = S6Context::new(OperationType::Write, f32::NAN, 0.3);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));
    }

    #[test]
    fn test_s6_context_features_layout() {
        let ctx = S6Context::new(OperationType::Write, 0.7, 0.3).unwrap();
        let features = ctx.features();
        assert_eq!(features.len(), S6_CONTEXT_DIM);
        // one-hot 编码
        assert!((features[0] - 0.0).abs() < 1e-6); // ReadOnly
        assert!((features[1] - 1.0).abs() < 1e-6); // Write
        assert!((features[2] - 0.0).abs() < 1e-6); // Exec
        assert!((features[3] - 0.0).abs() < 1e-6); // Sandbox
                                                   // 数值特征
        assert!((features[4] - 0.7).abs() < 1e-6);
        assert!((features[5] - 0.3).abs() < 1e-6);
        // bias
        assert!((features[6] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_s6_context_features_readonly() {
        let ctx = S6Context::new(OperationType::ReadOnly, 0.1, 0.05).unwrap();
        let features = ctx.features();
        assert!((features[0] - 1.0).abs() < 1e-6);
        assert!((features[1] - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_s6_context_features_sandbox() {
        let ctx = S6Context::new(OperationType::Sandbox, 0.2, 0.1).unwrap();
        let features = ctx.features();
        assert!((features[3] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_s6_context_to_seam_context() {
        let ctx = S6Context::new(OperationType::Write, 0.5, 0.5).unwrap();
        let seam_ctx = ctx.to_seam_context().unwrap();
        assert_eq!(seam_ctx.dim(), S6_CONTEXT_DIM);
    }

    #[test]
    fn test_s6_context_display() {
        let ctx = S6Context::new(OperationType::Write, 0.7, 0.3).unwrap();
        let s = format!("{}", ctx);
        assert!(s.contains("S6Context"));
        assert!(s.contains("op=write"));
    }

    // ============================================================
    // S6Reward 测试
    // ============================================================

    #[test]
    fn test_s6_reward_perfect() {
        // 无误拦无漏拦 = 0.0 奖励
        let r = S6Reward::new(0.0, 0.0).unwrap();
        assert!(r.reward().abs() < 1e-6);
    }

    #[test]
    fn test_s6_reward_all_false_pass() {
        // 全漏拦 = -0.7（默认 W_pass=0.7）
        let r = S6Reward::new(0.0, 1.0).unwrap();
        assert!((r.reward() - (-0.7)).abs() < 1e-6);
    }

    #[test]
    fn test_s6_reward_all_false_block() {
        // 全误拦 = -0.3（默认 W_block=0.3）
        let r = S6Reward::new(1.0, 0.0).unwrap();
        assert!((r.reward() - (-0.3)).abs() < 1e-6);
    }

    #[test]
    fn test_s6_reward_worst_case() {
        // 全误拦全漏拦 = -1.0
        let r = S6Reward::new(1.0, 1.0).unwrap();
        assert!((r.reward() - (-1.0)).abs() < 1e-6);
    }

    #[test]
    fn test_s6_reward_mixed_case() {
        // 误拦 0.2 + 漏拦 0.3 = -(0.3 × 0.2 + 0.7 × 0.3) = -(0.06 + 0.21) = -0.27
        let r = S6Reward::new(0.2, 0.3).unwrap();
        assert!((r.reward() - (-0.27)).abs() < 1e-6);
    }

    #[test]
    fn test_s6_reward_custom_weights() {
        // 自定义 W_block=0.5, W_pass=0.5
        let params = S6RewardParams {
            false_block_weight: 0.5,
            false_pass_weight: 0.5,
        };
        let r = S6Reward::with_params(0.2, 0.3, params).unwrap();
        // reward = -(0.5 × 0.2 + 0.5 × 0.3) = -0.25
        assert!((r.reward() - (-0.25)).abs() < 1e-6);
    }

    #[test]
    fn test_s6_reward_invalid_false_block() {
        let result = S6Reward::new(1.5, 0.3);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));
    }

    #[test]
    fn test_s6_reward_invalid_false_pass() {
        let result = S6Reward::new(0.3, -0.1);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));
    }

    #[test]
    fn test_s6_reward_nan_false_block() {
        let result = S6Reward::new(f64::NAN, 0.3);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));
    }

    // ============================================================
    // 臂集与映射测试
    // ============================================================

    #[test]
    fn test_s6_arm_set_count() {
        let arm_set = s6_arm_set();
        // `ArmSet::size()` 返回 `Option<usize>`（动态臂集可能无固定大小），
        // `DiscreteArmSet` 实现为 `Some(self.arms.len())`，故用 `Some(S6_ARM_COUNT)` 比对。
        assert_eq!(arm_set.size(), Some(S6_ARM_COUNT));
    }

    #[test]
    fn test_arm_index_to_profile_mapping() {
        assert_eq!(arm_index_to_profile(0), DecayProfile::Lenient);
        assert_eq!(arm_index_to_profile(1), DecayProfile::Standard);
        assert_eq!(arm_index_to_profile(2), DecayProfile::Strict);
        assert_eq!(arm_index_to_profile(3), DecayProfile::Aggressive);
        // 越界 fallback 到 Aggressive
        assert_eq!(arm_index_to_profile(99), DecayProfile::Aggressive);
    }

    #[test]
    fn test_profile_to_arm_index_mapping() {
        assert_eq!(profile_to_arm_index(DecayProfile::Lenient), 0);
        assert_eq!(profile_to_arm_index(DecayProfile::Standard), 1);
        assert_eq!(profile_to_arm_index(DecayProfile::Strict), 2);
        assert_eq!(profile_to_arm_index(DecayProfile::Aggressive), 3);
    }

    #[test]
    fn test_arm_mapping_roundtrip() {
        for profile in DecayProfile::ALL.iter() {
            let idx = profile_to_arm_index(*profile);
            let restored = arm_index_to_profile(idx);
            assert_eq!(*profile, restored, "roundtrip failed for {:?}", profile);
        }
    }

    // ============================================================
    // S6Learner 测试
    // ============================================================

    #[test]
    fn test_s6_learner_new_basic() {
        let learner = S6Learner::new(1.0).unwrap();
        assert_eq!(learner.total_steps(), 0);
    }

    #[test]
    fn test_s6_learner_with_default_alpha() {
        let learner = S6Learner::with_default_alpha().unwrap();
        assert_eq!(learner.total_steps(), 0);
    }

    #[test]
    fn test_s6_learner_invalid_alpha_zero() {
        let result = S6Learner::new(0.0);
        assert!(result.is_err());
    }

    #[test]
    fn test_s6_learner_invalid_alpha_negative() {
        let result = S6Learner::new(-1.0);
        assert!(result.is_err());
    }

    #[test]
    fn test_s6_learner_invalid_alpha_nan() {
        let result = S6Learner::new(f64::NAN);
        assert!(result.is_err());
    }

    #[test]
    fn test_s6_learner_select_returns_valid_profile() {
        let mut learner = S6Learner::new(1.0).unwrap();
        let ctx = S6Context::new(OperationType::Write, 0.5, 0.5).unwrap();
        let profile = learner.select(&ctx).unwrap();
        assert!(matches!(
            profile,
            DecayProfile::Lenient
                | DecayProfile::Standard
                | DecayProfile::Strict
                | DecayProfile::Aggressive
        ));
    }

    #[test]
    fn test_s6_learner_select_does_not_increment_step() {
        let mut learner = S6Learner::new(1.0).unwrap();
        let ctx = S6Context::new(OperationType::Write, 0.5, 0.5).unwrap();
        learner.select(&ctx).unwrap();
        assert_eq!(learner.total_steps(), 0);
    }

    #[test]
    fn test_s6_learner_update_increments_steps() {
        let mut learner = S6Learner::new(1.0).unwrap();
        let ctx = S6Context::new(OperationType::Write, 0.7, 0.3).unwrap();
        let profile = learner.select(&ctx).unwrap();
        let reward = S6Reward::new(0.1, 0.05).unwrap();
        learner.update(&ctx, profile, &reward).unwrap();
        assert_eq!(learner.total_steps(), 1);
    }

    #[test]
    fn test_s6_learner_multiple_updates() {
        let mut learner = S6Learner::new(1.0).unwrap();

        for i in 0..10 {
            let risk = (i as f32) / 10.0;
            let ctx = S6Context::new(OperationType::Write, risk, 0.3).unwrap();
            let profile = learner.select(&ctx).unwrap();
            // 模拟奖励：档位越严格，漏拦率越低但误拦率越高
            let (fb, fp) = match profile {
                DecayProfile::Lenient => (0.05, 0.30), // 低误拦高漏拦
                DecayProfile::Standard => (0.10, 0.15),
                DecayProfile::Strict => (0.20, 0.05),
                DecayProfile::Aggressive => (0.30, 0.02), // 高误拦低漏拦
            };
            let reward = S6Reward::new(fb, fp).unwrap();
            learner.update(&ctx, profile, &reward).unwrap();
        }

        assert_eq!(learner.total_steps(), 10);
    }

    #[test]
    fn test_s6_learner_current_policy_learned() {
        let mut learner = S6Learner::new(1.0).unwrap();
        let ctx = S6Context::new(OperationType::Write, 0.7, 0.4).unwrap();
        let profile = learner.select(&ctx).unwrap();

        let policy = learner.current_policy(42);
        assert!(policy.is_learned());
        assert_eq!(policy.version(), Some(42));
        assert_eq!(policy.profile(), profile);
    }

    #[test]
    fn test_s6_learner_current_policy_version_zero() {
        let learner = S6Learner::new(1.0).unwrap();
        let policy = learner.current_policy(0);
        assert!(policy.is_learned());
        assert_eq!(policy.version(), Some(0));
    }

    #[test]
    fn test_s6_learner_clone_independent() {
        let mut learner1 = S6Learner::new(1.0).unwrap();
        let ctx = S6Context::new(OperationType::Write, 0.7, 0.4).unwrap();
        let _ = learner1.select(&ctx).unwrap();

        let mut learner2 = learner1.clone();
        assert_eq!(learner1.total_steps(), learner2.total_steps());

        let reward = S6Reward::new(0.1, 0.05).unwrap();
        learner2
            .update(&ctx, DecayProfile::Strict, &reward)
            .unwrap();

        assert_eq!(learner1.total_steps(), 0);
        assert_eq!(learner2.total_steps(), 1);
    }

    #[test]
    fn test_s6_learner_linucb_access() {
        let learner = S6Learner::new(1.0).unwrap();
        let _linucb = learner.linucb();
    }

    // ============================================================
    // 端到端场景测试
    // ============================================================

    #[test]
    fn test_s6_scenario_readonly_low_risk_prefers_lenient() {
        // 只读 + 低风险场景，多轮学习后应偏向 Lenient
        let mut learner = S6Learner::new(1.0).unwrap();
        let ctx = S6Context::new(OperationType::ReadOnly, 0.1, 0.05).unwrap();

        for _ in 0..20 {
            let profile = learner.select(&ctx).unwrap();
            // Lenient 在只读低风险场景下误拦率低（合法操作不被冻结）
            let (fb, fp) = match profile {
                DecayProfile::Lenient => (0.02, 0.15),
                DecayProfile::Standard => (0.08, 0.10),
                DecayProfile::Strict => (0.20, 0.05),
                DecayProfile::Aggressive => (0.35, 0.02),
            };
            let reward = S6Reward::new(fb, fp).unwrap();
            learner.update(&ctx, profile, &reward).unwrap();
        }

        assert_eq!(learner.total_steps(), 20);
        let policy = learner.current_policy(1);
        assert!(policy.is_learned());
    }

    #[test]
    fn test_s6_scenario_write_high_risk_prefers_strict() {
        // 写操作 + 高风险场景，多轮学习后应偏向 Strict/Aggressive
        let mut learner = S6Learner::new(1.0).unwrap();
        let ctx = S6Context::new(OperationType::Write, 0.9, 0.5).unwrap();

        for _ in 0..20 {
            let profile = learner.select(&ctx).unwrap();
            // Strict/Aggressive 在写高风险场景下漏拦率低（违规操作被冻结）
            let (fb, fp) = match profile {
                DecayProfile::Lenient => (0.05, 0.40), // 漏拦率高
                DecayProfile::Standard => (0.10, 0.20),
                DecayProfile::Strict => (0.20, 0.05),
                DecayProfile::Aggressive => (0.30, 0.02), // 漏拦率最低
            };
            let reward = S6Reward::new(fb, fp).unwrap();
            learner.update(&ctx, profile, &reward).unwrap();
        }

        assert_eq!(learner.total_steps(), 20);
        let policy = learner.current_policy(1);
        assert!(policy.is_learned());
    }

    #[test]
    fn test_s6_scenario_exec_medium_risk_balanced() {
        // 执行操作 + 中等风险场景
        let mut learner = S6Learner::new(1.0).unwrap();
        let ctx = S6Context::new(OperationType::Exec, 0.5, 0.3).unwrap();

        for _ in 0..15 {
            let profile = learner.select(&ctx).unwrap();
            let reward = S6Reward::new(0.15, 0.10).unwrap();
            learner.update(&ctx, profile, &reward).unwrap();
        }

        assert_eq!(learner.total_steps(), 15);
    }

    #[test]
    fn test_s6_scenario_c4_compliance_fallback() {
        // C4 合规: learner 输出 Learned，调用方 fallback 到 Static(Standard)
        let mut learner = S6Learner::new(1.0).unwrap();
        let ctx = S6Context::new(OperationType::Write, 0.5, 0.5).unwrap();
        let _ = learner.select(&ctx).unwrap();

        let learned_policy = learner.current_policy(1);
        assert!(learned_policy.is_learned());

        // 模拟 learner panic 后本地 fallback
        let fallback = DecayPolicy::fallback();
        assert!(fallback.is_static());
        assert_eq!(fallback.profile(), DecayProfile::Standard);
    }

    #[test]
    fn test_s6_scenario_versioned_policy_for_ab_test() {
        // 版本号用于 A/B 测试与回滚
        let mut learner = S6Learner::new(1.0).unwrap();
        let ctx = S6Context::new(OperationType::Write, 0.5, 0.5).unwrap();
        learner.select(&ctx).unwrap();

        let v1 = learner.current_policy(1);
        let v2 = learner.current_policy(2);

        assert_ne!(v1.version(), v2.version());
        // 策略相同（同一次 select 结果）
        assert_eq!(v1.profile(), v2.profile());
    }

    #[test]
    fn test_s6_scenario_all_operation_types() {
        // 遍历所有操作类型验证不 panic
        let mut learner = S6Learner::new(1.0).unwrap();
        for op_type in OperationType::ALL.iter() {
            let ctx = S6Context::new(*op_type, 0.5, 0.3).unwrap();
            let profile = learner.select(&ctx).unwrap();
            let reward = S6Reward::new(0.1, 0.05).unwrap();
            learner.update(&ctx, profile, &reward).unwrap();
        }
        assert_eq!(learner.total_steps(), 4);
    }
}
