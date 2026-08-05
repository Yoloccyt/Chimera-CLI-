//! S4 接缝 — HCW selector 权重系数学习器（v5.0 §7.3 六接缝之一）
//!
//! 对应任务: **P4-W13.3.1**（S4 接缝上下文/臂/奖励定义）
//! 对应 ADR: **ADR-031**（omega-learner 边界）
//! 对应设计源: `NEXUS-OMEGA_v5.0_系统性完整设计文档.md` §7.3 S4
//!
//! # S4 接缝概述
//!
//! | 字段 | 值 |
//! |------|-----|
//! | 接缝名 | S4SelectorWeights（HCW selector 权重系数） |
//! | 代码锚点 | `crates/hcw-window/src/selector.rs (w1/w2/w3)` |
//! | 臂 | 5 个权重向量采样（default + 3 极端 + balanced） |
//! | 上下文 | 块类型 / 访问时序 / 错误关联 |
//! | 奖励 | 1.0 − 后悔率（cache regret 取反） |
//!
//! # 上下文向量设计（8 维）
//!
//! ```text
//! x = [
//!   block_type_one_hot(4),     // 0..3: Code / Doc / Config / Test
//!   access_recency_normalized, // 4: 最近访问距离的归一化 ∈ [0, 1]
//!   access_frequency_normalized,// 5: 访问频次归一化 ∈ [0, 1]
//!   error_correlation,         // 6: 该块类型的近期错误率 ∈ [0, 1]
//!   bias,                      // 7: 常量 1.0（线性模型偏置项）
//! ]
//! ```
//!
//! 维度 `d = 8`，与 S1 一致，便于复用 LinUCB 内部参数与 regret 上界假设。
//!
//! # 臂集设计（5 臂权重向量）
//!
//! 5 个代表性权重向量，覆盖 `SelectorWeights` 三元组空间的采样:
//!
//! | 索引 | 权重 (recency, frequency, relevance) | 语义 |
//! |------|--------------------------------------|------|
//! | 0 | (0.40, 0.30, 0.30) | default — 架构手册推荐基线 |
//! | 1 | (0.60, 0.20, 0.20) | recency-heavy — 偏向时近性 |
//! | 2 | (0.20, 0.60, 0.20) | frequency-heavy — 偏向频次 |
//! | 3 | (0.20, 0.20, 0.60) | relevance-heavy — 偏向任务相关性 |
//! | 4 | (0.34, 0.33, 0.33) | balanced — 极均衡 |
//!
//! 所有权重三元组满足 `w1 + w2 + w3 = 1.0`（`SelectorWeights::is_valid`）。
//! 臂 ID 用 `w=(r,f,rel)` 字符串（如 `"w=(0.4,0.3,0.3)"`），便于跨版本持久化。
//!
//! # 奖励函数
//!
//! `reward = 1.0 − regret_rate`
//!
//! - `regret_rate ∈ [0, 1]`: 被驱逐块后被需要的比例（cache regret）
//!   - 0.0 表示无后悔（所有驱逐决策都正确）
//!   - 1.0 表示完全后悔（所有驱逐块都被再次需要）
//! - `reward ∈ [0, 1]`: 高奖励 = 低后悔（好策略）
//!
//! WHY 用 `1.0 − regret_rate` 而非 `-regret_rate`:
//! LinUCB 假设奖励为上下文线性函数，`[0, 1]` 区间更符合线性假设，
//! 避免负奖励导致 `A_a` 矩阵更新方向偏移（虽然数学上等价，但数值稳定性更好）。
//!
//! WHY 用 `1.0 − r` 而非 `-r` 或 `e^(-r)`:
//! `1.0 − r` 是仿射变换，保持线性关系；`e^(-r)` 会破坏线性假设，
//! 导致 LinUCB regret 上界 `O(√(T·d·ln(K·T)))` 失效。
//!
//! # C4 合规（能力场灰度，非运行时旗）
//!
//! `S4Learner` 输出 `SelectorPolicy::Learned { version, weights }` 给上层调用方
//! （chimera-cli / quest-engine），由上层通过 `HcwWindow::update_selector_policy()`
//! 注入。`hcw-window` 本地 fallback 到 `SelectorPolicy::Static(SelectorWeights::DEFAULT)`，
//! **无跨 crate 旗标传播**（spec.md:334）。
//!
//! # 与 S1 接缝的对称性
//!
//! S4 与 S1 共享 LinUCB 算法骨架与 8 维上下文设计，差异仅在:
//! - 上下文字段语义（任务类型 vs 块类型 / 密度提示 vs 错误关联）
//! - 臂空间（4 个 DensityTier vs 5 个 SelectorWeights 采样）
//! - 奖励语义（成功率−延迟 vs 1−后悔率）
//!
//! # 示例
//!
//! ## 基础学习流程
//!
//! ```
//! use nexus_contracts::SelectorWeights;
//! use omega_learner::s4_selector::{BlockType, S4Context, S4Learner, S4Reward};
//!
//! // 1. 创建 S4 学习器（α=1.0，默认探索强度）
//! let mut learner = S4Learner::new(1.0).unwrap();
//!
//! // 2. 构造上下文（Code 块，最近访问，频次 0.6，错误关联 0.1）
//! let ctx = S4Context::new(
//!     BlockType::Code,
//!     0.8,    // access_recency
//!     0.6,    // access_frequency
//!     0.1,    // error_correlation
//! ).unwrap();
//!
//! // 3. 选择权重向量
//! let weights = learner.select(&ctx).unwrap();
//! assert!(weights.is_valid());
//!
//! // 4. 观察后悔率（10% 驱逐块后被需要 → reward = 0.9）
//! let reward = S4Reward::new(0.1).unwrap();
//! learner.update(&ctx, weights, &reward).unwrap();
//! assert_eq!(learner.total_steps(), 1);
//!
//! // 5. 输出当前策略（SelectorPolicy::Learned）
//! let policy = learner.current_policy(1);
//! assert!(policy.is_learned());
//! assert_eq!(policy.weights(), weights);
//! ```

use crate::arm::{ArmId, ArmIndex, DiscreteArmSet};
use crate::context::SeamContext;
use crate::error::{LearnerError, Result};
use crate::linucb::LinUCB;
use nexus_contracts::{SelectorPolicy, SelectorWeights};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// ============================================================
// 常量定义
// ============================================================

/// S4 上下文维度（block_type one-hot(4) + recency + frequency + error + bias）
pub const S4_CONTEXT_DIM: usize = 8;

/// S4 默认探索强度 α（LinUCB 探索-利用平衡）
///
/// WHY α=1.0: 与 S1 保持一致，Li et al. (2010) 推荐的稳健默认值，
/// 在 `||x|| ≤ √5` 范围内仍提供合理探索。
/// 过小会导致过早收敛到 default 臂（权重外置前基线），
/// 过大会导致过度探索，无法稳定到最优权重。
pub const DEFAULT_S4_ALPHA: f64 = 1.0;

/// S4 臂数（5 个权重向量采样）
pub const S4_ARM_COUNT: usize = 29;

// ============================================================
// 块类型枚举
// ============================================================

/// 块类型 — S4 上下文的第一组特征（one-hot 编码）
///
/// WHY 枚举而非字符串: 4 种块类型有限且固定，
/// 枚举提供编译期穷尽性检查与零开销匹配。
///
/// WHY 4 种: HCW 上下文压缩主要处理的对象类型分类，
/// 不同块类型的最优权重偏好不同:
/// - `Code`: 倾向 recency-heavy（最近编辑的代码最相关）
/// - `Doc`: 倾向 relevance-heavy（文档按主题相关）
/// - `Config`: 倾向 frequency-heavy（配置常被多模块引用）
/// - `Test`: 倾向 balanced（测试代码三者均衡）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum BlockType {
    /// 代码块（倾向 recency-heavy，最近编辑最相关）
    Code = 0,
    /// 文档块（倾向 relevance-heavy，按主题相关）
    Doc = 1,
    /// 配置块（倾向 frequency-heavy，常被多模块引用）
    Config = 2,
    /// 测试块（倾向 balanced，三者均衡）
    Test = 3,
}

impl BlockType {
    /// 返回所有块类型（按枚举值升序，便于 one-hot 编码）
    pub const ALL: [Self; 4] = [Self::Code, Self::Doc, Self::Config, Self::Test];

    /// 返回 one-hot 编码的索引位置（0..3）
    pub const fn one_hot_index(self) -> usize {
        self as usize
    }

    /// 返回块类型简称（用于日志/调试）
    pub const fn short_name(self) -> &'static str {
        match self {
            Self::Code => "code",
            Self::Doc => "doc",
            Self::Config => "config",
            Self::Test => "test",
        }
    }

    /// 返回该块类型的默认权重偏好（用于初始化 LinUCB 的先验）
    ///
    /// WHY 提供: 不同块类型有不同的"好策略"先验，
    /// 此方法供调用方在 LinUCB 冷启动时注入先验（本期未启用，预留扩展点）。
    pub const fn default_weights_hint(self) -> SelectorWeights {
        match self {
            // Code: 最近编辑的代码最相关 → recency-heavy
            Self::Code => SelectorWeights::new(0.6, 0.2, 0.2),
            // Doc: 按主题相关 → relevance-heavy
            Self::Doc => SelectorWeights::new(0.2, 0.2, 0.6),
            // Config: 多模块引用 → frequency-heavy
            Self::Config => SelectorWeights::new(0.2, 0.6, 0.2),
            // Test: 三者均衡 → balanced
            Self::Test => SelectorWeights::new(0.34, 0.33, 0.33),
        }
    }
}

impl std::fmt::Display for BlockType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.short_name())
    }
}

// ============================================================
// S4 上下文
// ============================================================

/// S4 上下文 — 块类型 / 访问时序 / 错误关联
///
/// 编码为 8 维特征向量，供 LinUCB 消费。所有数值字段归一化到 [0, 1]，
/// 满足 LinUCB regret 上界假设。
///
/// # 设计决策（WHY）
/// - **one-hot 块类型**: 4 维，避免序数编码（Code < Doc 不成立）
/// - **access_recency 归一化**: 最近访问距离 / time_span，最新为 1.0，最旧为 0.0
/// - **access_frequency 归一化**: access_count / max_access_count，最高频为 1.0
/// - **error_correlation**: 该块类型的近期错误率（如测试失败率、lint 警告率）
/// - **bias 常量 1.0**: 线性模型偏置项，允许 θ_a 学习"基础偏好"
///
/// # 字段语义
/// - `access_recency = 1.0`: 块在最近一次访问窗口内
/// - `access_recency = 0.0`: 块在最旧一次访问窗口
/// - `access_frequency = 1.0`: 块是最高频访问的
/// - `access_frequency = 0.0`: 块只被访问过一次
/// - `error_correlation = 0.0`: 该块类型近期无错误
/// - `error_correlation = 1.0`: 该块类型近期全部出错
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct S4Context {
    /// 块类型（one-hot 编码到 4 维）
    pub block_type: BlockType,
    /// 访问时近性 ∈ [0, 1]（1.0 = 最近访问，0.0 = 最旧访问）
    pub access_recency: f32,
    /// 访问频次归一化 ∈ [0, 1]（1.0 = 最高频，0.0 = 最低频）
    pub access_frequency: f32,
    /// 错误关联 ∈ [0, 1]（该块类型的近期错误率）
    pub error_correlation: f32,
}

impl S4Context {
    /// 创建 S4 上下文
    ///
    /// # 参数
    /// - `block_type`: 块类型（决定 one-hot 编码位置）
    /// - `access_recency`: 访问时近性 ∈ [0, 1]（1.0 = 最近访问）
    /// - `access_frequency`: 访问频次 ∈ [0, 1]（1.0 = 最高频）
    /// - `error_correlation`: 错误关联 ∈ [0, 1]（1.0 = 全部出错）
    ///
    /// # 错误
    /// - `InvalidReward`: 任一数值字段非有限或不在 [0, 1]
    ///
    /// # 示例
    ///
    /// ```
    /// use omega_learner::s4_selector::{BlockType, S4Context};
    ///
    /// let ctx = S4Context::new(
    ///     BlockType::Code,
    ///     0.8,    // access_recency
    ///     0.6,    // access_frequency
    ///     0.1,    // error_correlation
    /// ).unwrap();
    ///
    /// assert_eq!(ctx.block_type, BlockType::Code);
    /// assert!((ctx.access_recency - 0.8).abs() < 1e-6);
    /// ```
    pub fn new(
        block_type: BlockType,
        access_recency: f32,
        access_frequency: f32,
        error_correlation: f32,
    ) -> Result<Self> {
        // 校验 access_recency
        if !access_recency.is_finite() || !(0.0..=1.0).contains(&access_recency) {
            return Err(LearnerError::InvalidReward {
                reward: access_recency as f64,
            });
        }
        // 校验 access_frequency
        if !access_frequency.is_finite() || !(0.0..=1.0).contains(&access_frequency) {
            return Err(LearnerError::InvalidReward {
                reward: access_frequency as f64,
            });
        }
        // 校验 error_correlation
        if !error_correlation.is_finite() || !(0.0..=1.0).contains(&error_correlation) {
            return Err(LearnerError::InvalidReward {
                reward: error_correlation as f64,
            });
        }

        Ok(Self {
            block_type,
            access_recency,
            access_frequency,
            error_correlation,
        })
    }

    /// 编码为 8 维特征向量，供 LinUCB 消费
    ///
    /// 向量布局:
    /// - `[0..4]`: block_type one-hot 编码（选中位置为 1.0，其余为 0.0）
    /// - `[4]`: access_recency
    /// - `[5]`: access_frequency
    /// - `[6]`: error_correlation
    /// - `[7]`: bias 常量 1.0
    ///
    /// # L2 范数分析
    /// - 最小范数: 仅 bias=1.0 + 一个 one-hot=1.0 = √2 ≈ 1.414
    /// - 最大范数: 4 个字段都为 1.0 = √5 ≈ 2.236
    ///
    /// **WHY 不强制归一化**: 与 S1 一致，允许稍大范数只需相应增大 α 探索强度。
    /// `S4Learner::new` 默认 α=1.0，对范数 √5 仍提供合理探索。
    pub fn features(&self) -> [f32; S4_CONTEXT_DIM] {
        let mut features = [0.0f32; S4_CONTEXT_DIM];
        // block_type one-hot 编码
        features[self.block_type.one_hot_index()] = 1.0;
        // 数值特征
        features[4] = self.access_recency;
        features[5] = self.access_frequency;
        features[6] = self.error_correlation;
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

impl std::fmt::Display for S4Context {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "S4Context({}, recency={:.2}, freq={:.2}, err={:.2})",
            self.block_type.short_name(),
            self.access_recency,
            self.access_frequency,
            self.error_correlation
        )
    }
}

// ============================================================
// S4 奖励
// ============================================================

/// S4 奖励 — 后悔率转换的奖励值（1.0 − regret_rate）
///
/// 公式: `reward = 1.0 − regret_rate`
///
/// # 字段
/// - `regret_rate ∈ [0, 1]`: 被驱逐块后被需要的比例（cache regret）
///   - 0.0: 无后悔（所有驱逐决策都正确）
///   - 1.0: 完全后悔（所有驱逐块都被再次需要）
///
/// # 边界处理
/// - `regret_rate = 0.0`: reward = 1.0（最大奖励，最优策略）
/// - `regret_rate = 1.0`: reward = 0.0（最小奖励，最差策略）
/// - `regret_rate = 0.5`: reward = 0.5（中性）
///
/// # 设计动机（WHY 1.0 − r 而非其他形式）
///
/// - **仿射变换保持线性**: LinUCB 假设 `E[r | x, a] = x^T θ_a`，
///   `1.0 − r` 是仿射变换，保持线性关系。
/// - **避免负奖励**: `[0, 1]` 区间避免负奖励导致 `A_a` 矩阵更新方向偏移。
/// - **直观可解释**: reward = 0.9 直接读出"10% 后悔率"。
///
/// # 示例
///
/// ```
/// use omega_learner::s4_selector::S4Reward;
///
/// // 无后悔（完美策略）
/// let r1 = S4Reward::new(0.0).unwrap();
/// assert!((r1.reward() - 1.0).abs() < 1e-6);
///
/// // 完全后悔（最差策略）
/// let r2 = S4Reward::new(1.0).unwrap();
/// assert!((r2.reward() - 0.0).abs() < 1e-6);
///
/// // 10% 后悔率
/// let r3 = S4Reward::new(0.1).unwrap();
/// assert!((r3.reward() - 0.9).abs() < 1e-6);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct S4Reward {
    /// 后悔率 ∈ [0, 1]（被驱逐块后被需要的比例）
    pub regret_rate: f64,
}

impl S4Reward {
    /// 创建 S4 奖励
    ///
    /// # 参数
    /// - `regret_rate`: 后悔率 ∈ [0, 1]
    ///
    /// # 错误
    /// - `InvalidReward`: regret_rate 非有限或不在 [0, 1]
    ///
    /// # 示例
    ///
    /// ```
    /// use omega_learner::s4_selector::S4Reward;
    ///
    /// let reward = S4Reward::new(0.2).unwrap();
    /// assert!((reward.reward() - 0.8).abs() < 1e-6);
    /// ```
    pub fn new(regret_rate: f64) -> Result<Self> {
        if !regret_rate.is_finite() || !(0.0..=1.0).contains(&regret_rate) {
            return Err(LearnerError::InvalidReward {
                reward: regret_rate,
            });
        }
        Ok(Self { regret_rate })
    }

    /// 计算最终奖励值
    ///
    /// 公式: `reward = 1.0 − regret_rate`
    ///
    /// WHY 仿射变换: LinUCB 假设奖励是上下文线性函数，
    /// 仿射变换保持线性关系；非线性变换（如 e^(-r)）会破坏 regret 上界。
    pub fn reward(&self) -> f64 {
        1.0 - self.regret_rate
    }

    /// 返回原始后悔率（便于审计与日志）
    pub fn regret_rate(&self) -> f64 {
        self.regret_rate
    }
}

// ============================================================
// S4 臂集（5 臂对应权重向量采样）
// ============================================================

/// S4 臂集对应的 5 个权重向量（按臂索引顺序）
///
/// 索引顺序与 `s4_arm_set()` 一致，便于 `arm_index_to_weights` / `weights_to_arm_index` 双向映射。
pub const S4_ARM_WEIGHTS: [SelectorWeights; S4_ARM_COUNT] = [
    // Arm 0: default (0.4, 0.3, 0.3)
    SelectorWeights::new(0.40, 0.30, 0.30),
    // Arm 1: recency-heavy (0.6, 0.2, 0.2)
    SelectorWeights::new(0.60, 0.20, 0.20),
    // Arm 2: frequency-heavy (0.2, 0.6, 0.2)
    SelectorWeights::new(0.20, 0.60, 0.20),
    // Arm 3: relevance-heavy (0.2, 0.2, 0.6)
    SelectorWeights::new(0.20, 0.20, 0.60),
    // Arm 4: balanced (0.34, 0.33, 0.33)
    SelectorWeights::new(0.34, 0.33, 0.33),
    // === PROBE P2.3: 探针配比臂（Arm 5-28 = alpha 4 x grain 3 x k 2 = 24）===
    // 权重 = (alpha, 1-alpha, 0.0)：探针分替代 relevance；update 必须用 update_last
    SelectorWeights::new(0.3, 0.7, 0.0), // arm 5 a0.3-g256-k8
    SelectorWeights::new(0.3, 0.7, 0.0), // arm 6 a0.3-g256-k16
    SelectorWeights::new(0.3, 0.7, 0.0), // arm 7 a0.3-g512-k8
    SelectorWeights::new(0.3, 0.7, 0.0), // arm 8 a0.3-g512-k16
    SelectorWeights::new(0.3, 0.7, 0.0), // arm 9 a0.3-g1024-k8
    SelectorWeights::new(0.3, 0.7, 0.0), // arm 10 a0.3-g1024-k16
    SelectorWeights::new(0.5, 0.5, 0.0), // arm 11 a0.5-g256-k8
    SelectorWeights::new(0.5, 0.5, 0.0), // arm 12 a0.5-g256-k16
    SelectorWeights::new(0.5, 0.5, 0.0), // arm 13 a0.5-g512-k8
    SelectorWeights::new(0.5, 0.5, 0.0), // arm 14 a0.5-g512-k16
    SelectorWeights::new(0.5, 0.5, 0.0), // arm 15 a0.5-g1024-k8
    SelectorWeights::new(0.5, 0.5, 0.0), // arm 16 a0.5-g1024-k16
    SelectorWeights::new(0.7, 0.3, 0.0), // arm 17 a0.7-g256-k8
    SelectorWeights::new(0.7, 0.3, 0.0), // arm 18 a0.7-g256-k16
    SelectorWeights::new(0.7, 0.3, 0.0), // arm 19 a0.7-g512-k8
    SelectorWeights::new(0.7, 0.3, 0.0), // arm 20 a0.7-g512-k16
    SelectorWeights::new(0.7, 0.3, 0.0), // arm 21 a0.7-g1024-k8
    SelectorWeights::new(0.7, 0.3, 0.0), // arm 22 a0.7-g1024-k16
    SelectorWeights::new(0.9, 0.1, 0.0), // arm 23 a0.9-g256-k8
    SelectorWeights::new(0.9, 0.1, 0.0), // arm 24 a0.9-g256-k16
    SelectorWeights::new(0.9, 0.1, 0.0), // arm 25 a0.9-g512-k8
    SelectorWeights::new(0.9, 0.1, 0.0), // arm 26 a0.9-g512-k16
    SelectorWeights::new(0.9, 0.1, 0.0), // arm 27 a0.9-g1024-k8
    SelectorWeights::new(0.9, 0.1, 0.0), // arm 28 a0.9-g1024-k16
];

/// S4 臂集对应的简称（用于 ArmId 字符串）
const S4_ARM_SHORT_NAMES: [&str; S4_ARM_COUNT] = [
    "default",
    "recency-heavy",
    "frequency-heavy",
    "relevance-heavy",
    "balanced",
    "probe-a0.3-g256-k8",
    "probe-a0.3-g256-k16",
    "probe-a0.3-g512-k8",
    "probe-a0.3-g512-k16",
    "probe-a0.3-g1024-k8",
    "probe-a0.3-g1024-k16",
    "probe-a0.5-g256-k8",
    "probe-a0.5-g256-k16",
    "probe-a0.5-g512-k8",
    "probe-a0.5-g512-k16",
    "probe-a0.5-g1024-k8",
    "probe-a0.5-g1024-k16",
    "probe-a0.7-g256-k8",
    "probe-a0.7-g256-k16",
    "probe-a0.7-g512-k8",
    "probe-a0.7-g512-k16",
    "probe-a0.7-g1024-k8",
    "probe-a0.7-g1024-k16",
    "probe-a0.9-g256-k8",
    "probe-a0.9-g256-k16",
    "probe-a0.9-g512-k8",
    "probe-a0.9-g512-k16",
    "probe-a0.9-g1024-k8",
    "probe-a0.9-g1024-k16",
];

/// 构建 S4 接缝的臂集（5 臂对应权重向量采样）
///
/// 臂 ID 用 `w=(r,f,rel)` 字符串格式（如 `"w=(0.4,0.3,0.3)"`），
/// 便于跨版本持久化与 SpecRegistry 谱系追踪。
///
/// WHY 函数而非常量: `DiscreteArmSet::new` 接受 `Vec<ArmId>`，
/// 不能在 const 上下文构造（Vec 堆分配）。每次调用开销 O(1)（5 个 ArmId 克隆）。
pub fn s4_arm_set() -> DiscreteArmSet {
    let arm_ids: Vec<ArmId> = S4_ARM_WEIGHTS
        .iter()
        .map(|w| {
            let (r, f, rel) = w.as_tuple();
            ArmId::new(format!("w=({r},{f},{rel})"))
        })
        .collect();
    DiscreteArmSet::new(arm_ids)
}

/// ArmIndex → SelectorWeights 映射
///
/// 臂顺序与 `S4_ARM_WEIGHTS` 一致（default / recency-heavy / frequency-heavy /
/// relevance-heavy / balanced）。
///
/// WHY const fn: 映射是纯函数，编译期可计算，避免运行时开销。
pub const fn arm_index_to_weights(idx: usize) -> SelectorWeights {
    // PROBE P2.3: 数组索引（29 臂）——越界返回 Arm 0（default）防 panic
    if idx < S4_ARM_COUNT {
        S4_ARM_WEIGHTS[idx]
    } else {
        S4_ARM_WEIGHTS[0]
    }
}
/// ArmIndex → 臂简称（用于日志/调试）
///
/// PROBE P2.3: 29 臂数组索引，越界返回 default
pub const fn arm_index_to_short_name(idx: usize) -> &'static str {
    // PROBE P2.3: 数组索引（29 臂）——越界返回 default 防 panic
    if idx < S4_ARM_COUNT {
        S4_ARM_SHORT_NAMES[idx]
    } else {
        S4_ARM_SHORT_NAMES[0]
    }
}
/// 在 S4 臂集中查找匹配的权重向量，返回其索引。
/// 若无精确匹配（浮点比较），返回 0（default）作为 fallback。
///
/// WHY 用浮点容差比较: f32 浮点累加可能产生微小漂移，
/// 用 1e-6 容差避免误判。
pub fn weights_to_arm_index(weights: SelectorWeights) -> usize {
    for (idx, &arm_weights) in S4_ARM_WEIGHTS.iter().enumerate() {
        let (r1, f1, rel1) = weights.as_tuple();
        let (r2, f2, rel2) = arm_weights.as_tuple();
        if (r1 - r2).abs() < 1e-6 && (f1 - f2).abs() < 1e-6 && (rel1 - rel2).abs() < 1e-6 {
            return idx;
        }
    }
    // 默认返回 0（default 臂）作为 fallback
    0
}

// ============================================================
/// 探针配比臂参数（PROBE P2.3）
///
/// Arm 5-28 的探针融合参数：(alpha, beta) 静态/探针分权重 + grain/k 检索配置。
///
/// # 字段
/// - `alpha`: 静态分权重（探针分权重 = 1 - alpha，配比互补）
/// - `beta`: 探针分权重（= 1 - alpha，保持 f32 语义对称）
/// - `grain`: 探针相似度检索粒度（token 窗口）
/// - `k`: 探针 Top-K 召回数
///
/// WHY 独立于 SelectorWeights: (alpha,beta) 是 probe.rs 融合公式参数，
/// 与 compressor 静态评分三元组正交；omega-learner 无 hcw-window 依赖，
/// 经本表在 omega-learner 侧承载，生效侧（hcw-window）按臂索引查表
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProbeArmParams {
    /// 静态分权重 [0,1]
    pub alpha: f32,
    /// 探针分权重 = 1 - alpha
    pub beta: f32,
    /// 检索粒度（token）
    pub grain: usize,
    /// Top-K 召回数
    pub k: usize,
}

/// 按臂索引查询探针配比参数（Arm 5-28；原臂 0-4 返回 None）
///
/// # 返回
/// - `Some(params)`: 探针配比臂（索引 5-28）
/// - `None`: 原 5 权重臂（0-4）或越界
pub fn probe_arm_params(idx: usize) -> Option<ProbeArmParams> {
    if !(5..S4_ARM_COUNT).contains(&idx) {
        return None;
    }
    let probe_idx = idx - 5;
    let alphas = [0.3f32, 0.5, 0.7, 0.9];
    let grains = [256usize, 512, 1024];
    let ks = [8usize, 16];
    let a = alphas[probe_idx / (3 * 2)];
    let g = grains[(probe_idx / 2) % 3];
    let k = ks[probe_idx % 2];
    Some(ProbeArmParams {
        alpha: a,
        beta: 1.0 - a,
        grain: g,
        k,
    })
}

// S4 学习器
// ============================================================

/// S4 学习器 — 封装 LinUCB + S4 上下文/臂/奖励逻辑
///
/// # 设计
///
/// `S4Learner` 是 `LinUCB` 的薄封装，提供 S4 接缝特定的:
/// - 上下文编码（`S4Context` → `SeamContext`）
/// - 臂映射（`ArmIndex` → `SelectorWeights`）
/// - 奖励计算（`S4Reward` → `f64`）
///
/// # C4 合规
///
/// `S4Learner` 只产出 `SelectorPolicy::Learned { version, weights }`，
/// 不直接修改 `hcw-window` 状态。上层调用方负责通过
/// `HcwWindow::update_selector_policy()` 注入策略。
///
/// # 线程安全
///
/// `S4Learner` 内部 `LinUCB` 非 `Sync`（ndarray 数组无原子操作），
/// 多线程共享需通过 `Arc<Mutex<S4Learner>>` 或 `Arc<RwLock<S4Learner>>`。
/// 异步学习器典型用法是单线程后台任务 + tokio::sync::mpsc 通信。
///
/// # 示例
///
/// ```
/// use nexus_contracts::SelectorWeights;
/// use omega_learner::s4_selector::{BlockType, S4Context, S4Learner, S4Reward};
///
/// let mut learner = S4Learner::new(1.0).unwrap();
///
/// let ctx = S4Context::new(BlockType::Code, 0.8, 0.6, 0.1).unwrap();
/// let weights = learner.select(&ctx).unwrap();
///
/// let reward = S4Reward::new(0.1).unwrap();
/// learner.update(&ctx, weights, &reward).unwrap();
///
/// let policy = learner.current_policy(1);
/// assert!(policy.is_learned());
/// assert_eq!(policy.weights(), weights);
/// ```
#[derive(Debug, Clone)]
pub struct S4Learner {
    /// 内部 LinUCB 实例
    linucb: LinUCB,
    /// 最近一次选择的臂索引（用于 `current_policy` 输出）
    last_arm_idx: usize,
    /// 已观察到的总步数
    total_steps: u64,
    // PROBE P2.2: 策略下发回调（编排器注入）
    policy_sink: Option<PolicySink>,
}

/// 策略下发回调类型（PROBE P2.2）
/// WHY struct 包装: S4Learner derive(Debug, Clone)，裸 Arc<dyn Fn> 不实现 Debug/Clone；
#[derive(Clone)]
pub struct PolicySink(Arc<dyn Fn(SelectorPolicy) + Send + Sync>);

impl std::fmt::Debug for PolicySink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PolicySink")
    }
}

impl PolicySink {
    /// 创建回调（编排器闭包捕获 holder）
    pub fn new<F: Fn(SelectorPolicy) + Send + Sync + 'static>(f: F) -> Self {
        Self(Arc::new(f))
    }

    /// 触发回调
    pub fn invoke(&self, policy: SelectorPolicy) {
        (self.0)(policy)
    }
}

impl S4Learner {
    /// 创建 S4 学习器
    ///
    /// # 参数
    /// - `alpha`: LinUCB 探索强度（必须 > 0 且有限）
    ///
    /// # 错误
    /// - `InvalidAlpha`: alpha ≤ 0 或非有限
    /// - `NoArms`: 内部错误（S4 固定 5 臂，不应触发）
    pub fn new(alpha: f64) -> Result<Self> {
        let arm_set = s4_arm_set();
        let linucb = LinUCB::new(S4_CONTEXT_DIM, &arm_set, alpha)?;
        Ok(Self {
            linucb,
            last_arm_idx: 0,
            total_steps: 0,
            // PROBE P2.2: 默认无回调（未接线时零行为变化）
            policy_sink: None,
        })
    }

    /// 创建 S4 学习器（使用默认 α=1.0）
    pub fn with_default_alpha() -> Result<Self> {
        Self::new(DEFAULT_S4_ALPHA)
    }

    /// 设置策略下发回调（PROBE P2.2，append-only）
    /// # 参数 sink: 策略回调；None 清除回调
    /// 选择探针配比臂并返回参数（PROBE P2.3）
    ///
    /// # 返回
    /// - `Ok(Some(params))`: 探针臂（5-28）配比参数
    /// - `Ok(None)`: 选中原权重臂（0-4）——调用方按原权重路径处理
    /// - `Err`: select 失败（上下文维度/数值异常）
    ///
    /// WHY 独立于 `select()`: 探针臂的 (alpha,beta,grain,k) 无法承载进
    /// SelectorWeights 三元组，经本方法返回结构化参数
    pub fn select_probe(&mut self, context: &S4Context) -> Result<Option<ProbeArmParams>> {
        let _weights = self.select(context)?;
        Ok(probe_arm_params(self.last_arm_idx))
    }

    /// 用最近选择的臂更新模型（PROBE P2.3，探针臂专用）
    ///
    /// # 参数
    /// - `context`: 选择时的 S4 上下文
    /// - `reward`: 观察到的奖励（后悔率转换）
    ///
    /// WHY 与 `update()` 并行: 探针臂权重非唯一（(alpha,1-alpha,0) 按 alpha 档
    /// 重复），`weights_to_arm_index` 会映射到同 alpha 档第一个臂——
    /// 探针臂必须经 last_arm_idx 精确回溯（本方法）；原 5 权重臂仍走 `update()`
    pub fn update_last(&mut self, context: &S4Context, reward: &S4Reward) -> Result<()> {
        let seam_ctx = context.to_seam_context()?;
        let arm_idx = ArmIndex::from(self.last_arm_idx);
        let reward_value = reward.reward();
        self.linucb.update(arm_idx, &seam_ctx, reward_value)?;
        self.total_steps += 1;
        // PROBE P2.2: 学习更新后自动下发当前策略（有回调时）
        self.emit_policy();
        Ok(())
    }

    /// 设置策略下发回调（PROBE P2.2，append-only）
    /// # 参数 sink: 策略回调；None 清除回调
    pub fn set_policy_sink(&mut self, sink: Option<PolicySink>) {
        self.policy_sink = sink;
    }

    /// 手动触发策略下发（PROBE P2.2）— version = total_steps 单调递增
    pub fn emit_policy(&self) {
        if let Some(sink) = &self.policy_sink {
            sink.invoke(self.current_policy(self.total_steps));
        }
    }

    /// 选择权重向量 — 基于 S4 上下文
    ///
    /// # 算法
    /// 1. 将 `S4Context` 编码为 8 维特征向量
    /// 2. 转换为 `SeamContext`（LinUCB 输入）
    /// 3. 调用 `LinUCB::select_arm` 选择 UCB 最大的臂
    /// 4. 将 `ArmIndex` 映射回 `SelectorWeights`
    ///
    /// # 错误
    /// - `ContextDimensionMismatch`: 内部错误（S4 固定 8 维，不应触发）
    pub fn select(&mut self, context: &S4Context) -> Result<SelectorWeights> {
        let seam_ctx = context.to_seam_context()?;
        let arm_idx = self.linucb.select_arm(&seam_ctx)?;
        self.last_arm_idx = arm_idx.as_usize();
        Ok(arm_index_to_weights(self.last_arm_idx))
    }

    /// 观察奖励并更新模型
    ///
    /// # 参数
    /// - `context`: 选择时的 S4 上下文
    /// - `weights`: 选择的权重向量
    /// - `reward`: 观察到的奖励（后悔率转换）
    ///
    /// # 错误
    /// - `ArmOutOfRange`: weights 不在 S4 臂集中（不应触发）
    /// - `ContextDimensionMismatch`: 内部错误
    /// - `NumericalInstability`: Sherman-Morrison 分母 ≤ 0（矩阵病态）
    pub fn update(
        &mut self,
        context: &S4Context,
        weights: SelectorWeights,
        reward: &S4Reward,
    ) -> Result<()> {
        let seam_ctx = context.to_seam_context()?;
        let arm_idx = ArmIndex::from(weights_to_arm_index(weights));
        let reward_value = reward.reward();
        self.linucb.update(arm_idx, &seam_ctx, reward_value)?;
        self.total_steps += 1;
        // PROBE P2.2: 学习更新后自动下发当前策略（有回调时）
        self.emit_policy();
        Ok(())
    }

    /// 输出当前策略（SelectorPolicy::Learned）
    ///
    /// # 参数
    /// - `version`: 学习版本号（单调递增，用于 A/B 测试与回滚）
    ///
    /// # 返回
    /// `SelectorPolicy::Learned { version, weights }`，
    /// weights 为最近一次 `select` 的结果。
    ///
    /// WHY 提供: 上层调用方（chimera-cli / quest-engine）调用此方法
    /// 获取学习到的策略，然后通过 `HcwWindow::update_selector_policy()` 注入。
    pub fn current_policy(&self, version: u64) -> SelectorPolicy {
        SelectorPolicy::learned(version, arm_index_to_weights(self.last_arm_idx))
    }

    /// 返回已观察到的总步数
    pub fn total_steps(&self) -> u64 {
        self.total_steps
    }

    /// 返回最近一次选择的臂索引
    ///
    /// WHY 提供: 便于上层调用方记录使用的臂（用于效果追踪与 A/B 测试）。
    pub fn last_arm_index(&self) -> usize {
        self.last_arm_idx
    }

    /// 返回最近一次选择的臂简称（用于日志/审计）
    pub fn last_arm_short_name(&self) -> &'static str {
        arm_index_to_short_name(self.last_arm_idx)
    }

    /// 返回内部 LinUCB 引用（用于诊断与持久化）
    pub fn linucb(&self) -> &LinUCB {
        &self.linucb
    }
}

// ============================================================
// Send + Sync 静态断言
// ============================================================

/// S4Learner 必须实现 Send + Sync（异步跨线程共享需求）
///
/// WHY 必要性: S4Learner 可能被 Arc<Mutex<S4Learner>> 包裹，
/// 在 tokio 异步任务中跨 await 持有，编译期断言 Send+Sync 防止误用。
fn _assert_s4_learner_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<S4Learner>();
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================
    // BlockType 测试
    // ============================================================

    #[test]
    fn test_block_type_one_hot_index() {
        assert_eq!(BlockType::Code.one_hot_index(), 0);
        assert_eq!(BlockType::Doc.one_hot_index(), 1);
        assert_eq!(BlockType::Config.one_hot_index(), 2);
        assert_eq!(BlockType::Test.one_hot_index(), 3);
    }

    #[test]
    fn test_block_type_short_name() {
        assert_eq!(BlockType::Code.short_name(), "code");
        assert_eq!(BlockType::Doc.short_name(), "doc");
        assert_eq!(BlockType::Config.short_name(), "config");
        assert_eq!(BlockType::Test.short_name(), "test");
    }

    #[test]
    fn test_block_type_all_returns_four() {
        let all = BlockType::ALL;
        assert_eq!(all.len(), 4);
        assert!(all.contains(&BlockType::Code));
        assert!(all.contains(&BlockType::Test));
    }

    #[test]
    fn test_block_type_display() {
        assert_eq!(format!("{}", BlockType::Code), "code");
        assert_eq!(format!("{}", BlockType::Config), "config");
    }

    #[test]
    fn test_block_type_serialize_json() {
        let json = serde_json::to_string(&BlockType::Doc).unwrap();
        let deserialized: BlockType = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, BlockType::Doc);
    }

    #[test]
    fn test_block_type_default_weights_hint_validity() {
        // 所有块类型的默认权重偏好都必须满足合法性（和 = 1.0）
        for bt in BlockType::ALL {
            assert!(
                bt.default_weights_hint().is_valid(),
                "BlockType {bt:?} default_weights_hint invalid"
            );
        }
    }

    #[test]
    fn test_block_type_default_weights_hint_distinct() {
        // 不同块类型的默认权重偏好应不同（避免 LinUCB 先验退化）
        let weights: Vec<SelectorWeights> = BlockType::ALL
            .iter()
            .map(|bt| bt.default_weights_hint())
            .collect();
        for i in 0..weights.len() {
            for j in (i + 1)..weights.len() {
                assert_ne!(
                    weights[i], weights[j],
                    "BlockType {i} and {j} have same default weights hint"
                );
            }
        }
    }

    // ============================================================
    // S4Context 测试
    // ============================================================

    #[test]
    fn test_s4_context_new_basic() {
        let ctx = S4Context::new(BlockType::Code, 0.8, 0.6, 0.1).unwrap();
        assert_eq!(ctx.block_type, BlockType::Code);
        assert!((ctx.access_recency - 0.8).abs() < 1e-6);
        assert!((ctx.access_frequency - 0.6).abs() < 1e-6);
        assert!((ctx.error_correlation - 0.1).abs() < 1e-6);
    }

    #[test]
    fn test_s4_context_boundary_zero() {
        // 边界值 0.0 合法
        let ctx = S4Context::new(BlockType::Doc, 0.0, 0.0, 0.0).unwrap();
        assert!((ctx.access_recency - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_s4_context_boundary_one() {
        // 边界值 1.0 合法
        let ctx = S4Context::new(BlockType::Config, 1.0, 1.0, 1.0).unwrap();
        assert!((ctx.access_recency - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_s4_context_invalid_recency_high() {
        let result = S4Context::new(BlockType::Code, 1.5, 0.5, 0.1);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));
    }

    #[test]
    fn test_s4_context_invalid_recency_negative() {
        let result = S4Context::new(BlockType::Code, -0.1, 0.5, 0.1);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));
    }

    #[test]
    fn test_s4_context_invalid_recency_nan() {
        let result = S4Context::new(BlockType::Code, f32::NAN, 0.5, 0.1);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));
    }

    #[test]
    fn test_s4_context_invalid_frequency_high() {
        let result = S4Context::new(BlockType::Code, 0.5, 1.5, 0.1);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));
    }

    #[test]
    fn test_s4_context_invalid_error_correlation_high() {
        let result = S4Context::new(BlockType::Code, 0.5, 0.5, 1.5);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));
    }

    #[test]
    fn test_s4_context_invalid_error_correlation_nan() {
        let result = S4Context::new(BlockType::Code, 0.5, 0.5, f32::NAN);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));
    }

    #[test]
    fn test_s4_context_features_dimensions() {
        let ctx = S4Context::new(BlockType::Code, 0.5, 0.5, 0.5).unwrap();
        let features = ctx.features();
        assert_eq!(features.len(), S4_CONTEXT_DIM);
        assert_eq!(S4_CONTEXT_DIM, 8);
    }

    #[test]
    fn test_s4_context_features_one_hot_code() {
        let ctx = S4Context::new(BlockType::Code, 0.5, 0.5, 0.5).unwrap();
        let features = ctx.features();
        // Code 在索引 0
        assert!((features[0] - 1.0).abs() < 1e-6);
        assert!((features[1] - 0.0).abs() < 1e-6);
        assert!((features[2] - 0.0).abs() < 1e-6);
        assert!((features[3] - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_s4_context_features_one_hot_doc() {
        let ctx = S4Context::new(BlockType::Doc, 0.5, 0.5, 0.5).unwrap();
        let features = ctx.features();
        // Doc 在索引 1
        assert!((features[0] - 0.0).abs() < 1e-6);
        assert!((features[1] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_s4_context_features_one_hot_config() {
        let ctx = S4Context::new(BlockType::Config, 0.5, 0.5, 0.5).unwrap();
        let features = ctx.features();
        // Config 在索引 2
        assert!((features[2] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_s4_context_features_one_hot_test() {
        let ctx = S4Context::new(BlockType::Test, 0.5, 0.5, 0.5).unwrap();
        let features = ctx.features();
        // Test 在索引 3
        assert!((features[3] - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_s4_context_features_numeric_fields() {
        let ctx = S4Context::new(BlockType::Code, 0.7, 0.4, 0.2).unwrap();
        let features = ctx.features();
        assert!((features[4] - 0.7).abs() < 1e-6); // access_recency
        assert!((features[5] - 0.4).abs() < 1e-6); // access_frequency
        assert!((features[6] - 0.2).abs() < 1e-6); // error_correlation
        assert!((features[7] - 1.0).abs() < 1e-6); // bias
    }

    #[test]
    fn test_s4_context_to_seam_context() {
        let ctx = S4Context::new(BlockType::Code, 0.7, 0.4, 0.2).unwrap();
        let seam_ctx = ctx.to_seam_context().unwrap();
        assert_eq!(seam_ctx.dim(), S4_CONTEXT_DIM);
    }

    #[test]
    fn test_s4_context_display() {
        let ctx = S4Context::new(BlockType::Code, 0.8, 0.6, 0.1).unwrap();
        let s = format!("{ctx}");
        assert!(s.contains("S4Context"));
        assert!(s.contains("code"));
    }

    #[test]
    fn test_s4_context_serialize_json() {
        let ctx = S4Context::new(BlockType::Doc, 0.5, 0.3, 0.2).unwrap();
        let json = serde_json::to_string(&ctx).unwrap();
        let deserialized: S4Context = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, ctx);
    }

    // ============================================================
    // S4Reward 测试
    // ============================================================

    #[test]
    fn test_s4_reward_new_zero_regret() {
        // 无后悔 → reward = 1.0（最大奖励）
        let r = S4Reward::new(0.0).unwrap();
        assert!((r.reward() - 1.0).abs() < 1e-6);
        assert!((r.regret_rate() - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_s4_reward_new_full_regret() {
        // 完全后悔 → reward = 0.0（最小奖励）
        let r = S4Reward::new(1.0).unwrap();
        assert!((r.reward() - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_s4_reward_new_half_regret() {
        // 50% 后悔 → reward = 0.5
        let r = S4Reward::new(0.5).unwrap();
        assert!((r.reward() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn test_s4_reward_new_10_percent_regret() {
        // 10% 后悔 → reward = 0.9
        let r = S4Reward::new(0.1).unwrap();
        assert!((r.reward() - 0.9).abs() < 1e-6);
    }

    #[test]
    fn test_s4_reward_invalid_high() {
        let result = S4Reward::new(1.5);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));
    }

    #[test]
    fn test_s4_reward_invalid_negative() {
        let result = S4Reward::new(-0.1);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));
    }

    #[test]
    fn test_s4_reward_invalid_nan() {
        let result = S4Reward::new(f64::NAN);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));
    }

    #[test]
    fn test_s4_reward_invalid_infinity() {
        let result = S4Reward::new(f64::INFINITY);
        assert!(matches!(result, Err(LearnerError::InvalidReward { .. })));
    }

    #[test]
    fn test_s4_reward_copy_semantics() {
        let r1 = S4Reward::new(0.3).unwrap();
        let r2 = r1; // Copy
        assert_eq!(r1, r2); // r1 仍可用（Copy 非 Move）
    }

    #[test]
    fn test_s4_reward_serialize_json() {
        let r = S4Reward::new(0.25).unwrap();
        let json = serde_json::to_string(&r).unwrap();
        let deserialized: S4Reward = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, r);
    }

    // ============================================================
    // S4 臂集测试
    // ============================================================

    #[test]
    fn test_s4_arm_set_count() {
        let arm_set = s4_arm_set();
        assert_eq!(arm_set.len(), S4_ARM_COUNT);
        assert_eq!(S4_ARM_COUNT, 29);
    }

    #[test]
    fn test_s4_arm_weights_all_valid() {
        // 所有臂的权重三元组必须满足合法性（和 = 1.0）
        for &weights in S4_ARM_WEIGHTS.iter() {
            assert!(weights.is_valid(), "arm weights {weights:?} invalid");
        }
    }

    #[test]
    fn test_s4_arm_weights_default_is_first() {
        // Arm 0 应是 default (0.4, 0.3, 0.3)
        let default = arm_index_to_weights(0);
        let (r, f, rel) = default.as_tuple();
        assert!((r - 0.4).abs() < 1e-6);
        assert!((f - 0.3).abs() < 1e-6);
        assert!((rel - 0.3).abs() < 1e-6);
    }

    #[test]
    fn test_s4_arm_weights_recency_heavy() {
        // Arm 1 应是 recency-heavy (0.6, 0.2, 0.2)
        let weights = arm_index_to_weights(1);
        let (r, f, rel) = weights.as_tuple();
        assert!((r - 0.6).abs() < 1e-6);
        assert!((f - 0.2).abs() < 1e-6);
        assert!((rel - 0.2).abs() < 1e-6);
    }

    #[test]
    fn test_s4_arm_weights_frequency_heavy() {
        // Arm 2 应是 frequency-heavy (0.2, 0.6, 0.2)
        let weights = arm_index_to_weights(2);
        let (r, f, rel) = weights.as_tuple();
        assert!((r - 0.2).abs() < 1e-6);
        assert!((f - 0.6).abs() < 1e-6);
        assert!((rel - 0.2).abs() < 1e-6);
    }

    #[test]
    fn test_s4_arm_weights_relevance_heavy() {
        // Arm 3 应是 relevance-heavy (0.2, 0.2, 0.6)
        let weights = arm_index_to_weights(3);
        let (r, f, rel) = weights.as_tuple();
        assert!((r - 0.2).abs() < 1e-6);
        assert!((f - 0.2).abs() < 1e-6);
        assert!((rel - 0.6).abs() < 1e-6);
    }

    #[test]
    fn test_s4_arm_weights_balanced() {
        // Arm 4 应是 balanced (0.34, 0.33, 0.33)
        let weights = arm_index_to_weights(4);
        let (r, f, rel) = weights.as_tuple();
        assert!((r - 0.34).abs() < 1e-6);
        assert!((f - 0.33).abs() < 1e-6);
        assert!((rel - 0.33).abs() < 1e-6);
    }

    #[test]
    fn test_s4_arm_weights_out_of_range_falls_back_to_default() {
        // 越界（>28）应 fallback 到 Arm 0 (default)，数组索引防 panic
        let weights = arm_index_to_weights(100);
        let (r, f, rel) = weights.as_tuple();
        assert!((r - 0.4).abs() < 1e-6);
        assert!((f - 0.3).abs() < 1e-6);
        assert!((rel - 0.3).abs() < 1e-6);
    }

    #[test]
    fn test_probe_arm_params_full_coverage() {
        // 24 探针臂全覆盖：alpha 4 档 x grain 3 档 x k 2 档
        let mut seen = std::collections::HashSet::new();
        for idx in 5..S4_ARM_COUNT {
            let params = probe_arm_params(idx).expect("探针臂应有参数");
            assert!(
                (params.alpha + params.beta - 1.0).abs() < 1e-6,
                "alpha+beta=1"
            );
            assert!(params.grain > 0 && params.k > 0);
            assert!((0.3..=0.9).contains(&params.alpha));
            seen.insert((params.alpha.to_bits(), params.grain, params.k));
        }
        assert_eq!(seen.len(), 24, "24 种配比组合应互异");
    }

    #[test]
    fn test_probe_arm_params_original_arms_none() {
        // 原 5 权重臂（0-4）与越界应返回 None
        for idx in 0..5 {
            assert!(probe_arm_params(idx).is_none(), "原臂 {idx} 无探针参数");
        }
        assert!(probe_arm_params(29).is_none());
        assert!(probe_arm_params(usize::MAX).is_none());
    }

    #[test]
    fn test_probe_arm_params_layout_matches_arm_set() {
        // 索引布局与 S4_ARM_WEIGHTS 生成顺序一致: alpha 外循环 x grain 中循环 x k 内循环
        let p0 = probe_arm_params(5).unwrap();
        assert_eq!(p0.alpha, 0.3);
        assert_eq!(p0.grain, 256);
        assert_eq!(p0.k, 8);
        let p1 = probe_arm_params(6).unwrap();
        assert_eq!(p1.k, 16, "k 为内循环");
        let p2 = probe_arm_params(7).unwrap();
        assert_eq!(p2.grain, 512, "grain 为中循环");
        let p_last = probe_arm_params(28).unwrap();
        assert_eq!(p_last.alpha, 0.9);
        assert_eq!(p_last.grain, 1024);
        assert_eq!(p_last.k, 16);
    }

    #[test]
    fn test_select_probe_and_update_last() {
        // 探针臂链路: select_probe → update_last（last_arm_idx 回溯，权重非唯一安全）
        let mut learner = S4Learner::with_default_alpha().unwrap();
        let ctx = S4Context::new(BlockType::Code, 0.8, 0.5, 0.1).unwrap();
        let reward = S4Reward::new(0.2).unwrap();
        let mut saw_probe = false;
        for _ in 0..40 {
            if learner.select_probe(&ctx).unwrap().is_some() {
                saw_probe = true;
                learner.update_last(&ctx, &reward).unwrap();
            } else {
                // 原臂路径仍走既有 update
                let weights = learner.select(&ctx).unwrap();
                learner.update(&ctx, weights, &reward).unwrap();
            }
        }
        assert!(
            saw_probe,
            "40 次选择应至少命中一次探针臂（29 臂中 24 个探针）"
        );
        assert_eq!(learner.total_steps(), 40);
    }

    #[test]
    fn test_s4_arm_short_names() {
        assert_eq!(arm_index_to_short_name(0), "default");
        assert_eq!(arm_index_to_short_name(1), "recency-heavy");
        assert_eq!(arm_index_to_short_name(2), "frequency-heavy");
        assert_eq!(arm_index_to_short_name(3), "relevance-heavy");
        assert_eq!(arm_index_to_short_name(4), "balanced");
    }

    #[test]
    fn test_weights_to_arm_index_round_trip() {
        // PROBE P2.3: 仅原 5 权重臂可 round-trip（探针臂权重非唯一，走 update_last 不依赖本映射）
        for idx in 0..5 {
            let weights = arm_index_to_weights(idx);
            let recovered_idx = weights_to_arm_index(weights);
            assert_eq!(idx, recovered_idx, "round-trip failed for arm {idx}");
        }
    }

    #[test]
    fn test_weights_to_arm_index_unknown_falls_back_to_default() {
        // 未知权重（不在 S4 臂集中）应 fallback 到 0 (default)
        let unknown = SelectorWeights::new(0.5, 0.4, 0.1); // 和 = 1.0 但不在臂集
        assert!(unknown.is_valid()); // 合法但不匹配任何臂
        let idx = weights_to_arm_index(unknown);
        assert_eq!(idx, 0);
    }

    #[test]
    fn test_weights_to_arm_index_default() {
        // default 权重应映射到 Arm 0
        let idx = weights_to_arm_index(SelectorWeights::DEFAULT);
        assert_eq!(idx, 0);
    }

    // ============================================================
    // S4Learner 测试
    // ============================================================

    #[test]
    fn test_s4_learner_new_basic() {
        let learner = S4Learner::new(1.0).unwrap();
        assert_eq!(learner.total_steps(), 0);
        assert_eq!(learner.last_arm_index(), 0);
        assert_eq!(learner.last_arm_short_name(), "default");
    }

    #[test]
    fn test_s4_learner_with_default_alpha() {
        let learner = S4Learner::with_default_alpha().unwrap();
        assert_eq!(learner.total_steps(), 0);
    }

    #[test]
    fn test_s4_learner_new_invalid_alpha_zero() {
        let result = S4Learner::new(0.0);
        assert!(matches!(result, Err(LearnerError::InvalidAlpha { .. })));
    }

    #[test]
    fn test_s4_learner_new_invalid_alpha_negative() {
        let result = S4Learner::new(-1.0);
        assert!(matches!(result, Err(LearnerError::InvalidAlpha { .. })));
    }

    #[test]
    fn test_s4_learner_new_invalid_alpha_nan() {
        let result = S4Learner::new(f64::NAN);
        assert!(matches!(result, Err(LearnerError::InvalidAlpha { .. })));
    }

    #[test]
    fn test_s4_learner_select_returns_valid_weights() {
        let mut learner = S4Learner::new(1.0).unwrap();
        let ctx = S4Context::new(BlockType::Code, 0.7, 0.5, 0.1).unwrap();
        let weights = learner.select(&ctx).unwrap();
        // 选出的权重必须满足合法性（和 = 1.0）
        assert!(weights.is_valid());
    }

    #[test]
    fn test_s4_learner_select_updates_last_arm_idx() {
        let mut learner = S4Learner::new(1.0).unwrap();
        let ctx = S4Context::new(BlockType::Doc, 0.5, 0.5, 0.5).unwrap();
        let _ = learner.select(&ctx).unwrap();
        // select 后 last_arm_idx 应更新（具体值依赖 LinUCB 初始状态）
        assert!(learner.last_arm_index() < S4_ARM_COUNT);
    }

    #[test]
    fn test_s4_learner_update_increments_steps() {
        let mut learner = S4Learner::new(1.0).unwrap();
        let ctx = S4Context::new(BlockType::Code, 0.7, 0.5, 0.1).unwrap();

        // 先 select 再 update
        let weights = learner.select(&ctx).unwrap();
        let reward = S4Reward::new(0.1).unwrap();
        learner.update(&ctx, weights, &reward).unwrap();

        assert_eq!(learner.total_steps(), 1);
    }

    #[test]
    fn test_s4_learner_update_multiple_times() {
        let mut learner = S4Learner::new(1.0).unwrap();

        // 模拟 10 次学习迭代
        for step in 0..10 {
            let ctx = S4Context::new(BlockType::Code, 0.5 + step as f32 * 0.04, 0.5, 0.1).unwrap();
            let weights = learner.select(&ctx).unwrap();
            let reward = S4Reward::new(0.05 + step as f64 * 0.01).unwrap();
            learner.update(&ctx, weights, &reward).unwrap();
        }

        assert_eq!(learner.total_steps(), 10);
    }

    #[test]
    fn test_s4_learner_current_policy_initial() {
        // 初始策略（未 select 过）应是版本号传入的 Learned
        let learner = S4Learner::new(1.0).unwrap();
        let policy = learner.current_policy(1);
        assert!(policy.is_learned());
        assert_eq!(policy.version(), Some(1));
        // last_arm_idx 初始 = 0，所以 weights 应是 default
        assert_eq!(policy.weights(), arm_index_to_weights(0));
    }

    #[test]
    fn test_s4_learner_current_policy_after_select() {
        let mut learner = S4Learner::new(1.0).unwrap();
        let ctx = S4Context::new(BlockType::Code, 0.7, 0.5, 0.1).unwrap();
        let weights = learner.select(&ctx).unwrap();

        let policy = learner.current_policy(42);
        assert!(policy.is_learned());
        assert_eq!(policy.version(), Some(42));
        assert_eq!(policy.weights(), weights);
    }

    #[test]
    fn test_s4_learner_current_policy_version_monotonic() {
        // 版本号应单调递增（调用方负责）
        let learner = S4Learner::new(1.0).unwrap();
        let p1 = learner.current_policy(1);
        let p2 = learner.current_policy(2);
        let p3 = learner.current_policy(3);
        assert_eq!(p1.version(), Some(1));
        assert_eq!(p2.version(), Some(2));
        assert_eq!(p3.version(), Some(3));
    }

    #[test]
    fn test_s4_learner_last_arm_short_name() {
        let mut learner = S4Learner::new(1.0).unwrap();
        let ctx = S4Context::new(BlockType::Code, 0.7, 0.5, 0.1).unwrap();
        let _ = learner.select(&ctx).unwrap();

        let short_name = learner.last_arm_short_name();
        // 必须是 5 个臂简称之一
        assert!(matches!(
            short_name,
            "default" | "recency-heavy" | "frequency-heavy" | "relevance-heavy" | "balanced"
        ));
    }

    #[test]
    fn test_s4_learner_linucb_ref() {
        let learner = S4Learner::new(1.0).unwrap();
        let linucb = learner.linucb();
        // LinUCB 引用应有效
        assert_eq!(linucb.total_steps(), 0);
    }

    #[test]
    fn test_s4_learner_clone_independent() {
        let mut learner1 = S4Learner::new(1.0).unwrap();
        let ctx = S4Context::new(BlockType::Code, 0.7, 0.5, 0.1).unwrap();
        let _ = learner1.select(&ctx).unwrap();
        let reward = S4Reward::new(0.1).unwrap();
        learner1
            .update(&ctx, arm_index_to_weights(0), &reward)
            .unwrap();

        // 克隆后两者独立演化（learner2 不需 mut，仅做快照对比）
        let learner2 = learner1.clone();
        assert_eq!(learner1.total_steps(), learner2.total_steps());

        // 修改 learner1 不影响 learner2
        let _ = learner1.select(&ctx).unwrap();
        learner1
            .update(&ctx, arm_index_to_weights(1), &reward)
            .unwrap();
        assert_eq!(learner1.total_steps(), 2);
        assert_eq!(learner2.total_steps(), 1); // 保持原值
    }

    // ============================================================
    // 集成场景测试
    // ============================================================

    #[test]
    fn test_s4_seam_full_learning_flow() {
        // 模拟 S4 接缝完整学习流程:
        // 1. 创建 learner
        // 2. 构造 Code 块上下文（最近访问、高频、低错误）
        // 3. select 权重向量
        // 4. 模拟观察后悔率（5% 后悔 → reward = 0.95）
        // 5. update 模型
        // 6. current_policy 输出 Learned 策略

        let mut learner = S4Learner::new(1.0).unwrap();

        // Step 2: 构造 Code 块上下文
        let ctx = S4Context::new(BlockType::Code, 0.9, 0.8, 0.05).unwrap();

        // Step 3: select 权重向量
        let weights = learner.select(&ctx).unwrap();
        assert!(weights.is_valid());

        // Step 4 & 5: 观察后悔率并更新（5% 后悔）
        let reward = S4Reward::new(0.05).unwrap();
        learner.update(&ctx, weights, &reward).unwrap();
        assert_eq!(learner.total_steps(), 1);

        // Step 6: 输出 Learned 策略
        let policy = learner.current_policy(1);
        assert!(policy.is_learned());
        assert_eq!(policy.version(), Some(1));
        assert_eq!(policy.weights(), weights);
    }

    #[test]
    fn test_s4_seam_block_type_specific_learning() {
        // 模拟不同块类型的最优策略学习:
        // - Code 块倾向 recency-heavy
        // - Doc 块倾向 relevance-heavy
        // - Config 块倾向 frequency-heavy
        // - Test 块倾向 balanced
        //
        // 通过模拟不同后悔率反馈，验证 learner 能区分不同上下文

        let mut learner = S4Learner::new(1.0).unwrap();

        // 对 Code 块给 recency-heavy 臂低后悔率（高奖励）
        let code_ctx = S4Context::new(BlockType::Code, 0.8, 0.5, 0.1).unwrap();
        for _ in 0..5 {
            let _ = learner.select(&code_ctx).unwrap();
            // 模拟: 给 recency-heavy 低后悔率
            let reward = S4Reward::new(0.05).unwrap();
            learner
                .update(&code_ctx, arm_index_to_weights(1), &reward)
                .unwrap();
        }

        // 对 Doc 块给 relevance-heavy 臂低后悔率
        let doc_ctx = S4Context::new(BlockType::Doc, 0.5, 0.5, 0.1).unwrap();
        for _ in 0..5 {
            let _ = learner.select(&doc_ctx).unwrap();
            let reward = S4Reward::new(0.05).unwrap();
            learner
                .update(&doc_ctx, arm_index_to_weights(3), &reward)
                .unwrap();
        }

        // learner 应已积累 10 步
        assert_eq!(learner.total_steps(), 10);
    }

    #[test]
    fn test_s4_seam_c4_compliance_local_fallback() {
        // C4 合规: learner 输出 SelectorPolicy::Learned，
        // 调用方未注入时 hcw-window 使用 SelectorPolicy::default() = Static(DEFAULT)
        let mut learner = S4Learner::new(1.0).unwrap();
        let ctx = S4Context::new(BlockType::Code, 0.7, 0.5, 0.1).unwrap();
        let _ = learner.select(&ctx).unwrap();

        // learner 输出 Learned 策略
        let learned_policy = learner.current_policy(1);
        assert!(learned_policy.is_learned());

        // 调用方本地 fallback（模拟 learner panic 后）
        let fallback = SelectorPolicy::fallback();
        assert!(fallback.is_static());
        assert_eq!(fallback.weights(), SelectorWeights::DEFAULT);

        // fallback 与 learned 不同（除非 learner 巧好学到 DEFAULT，初始不太可能）
        // 这里只验证 fallback 始终是 Static(DEFAULT)，与 learner 输出独立
    }

    #[test]
    fn test_s4_seam_ab_test_scenario() {
        // 模拟 A/B 测试场景:
        // 版本 1 用 default 臂，版本 2 用 recency-heavy 臂，
        // 对比两个版本的策略效果

        let learner_v1 = S4Learner::new(1.0).unwrap();
        let learner_v2 = S4Learner::new(1.0).unwrap();

        // 两个 learner 初始状态相同
        assert_eq!(learner_v1.total_steps(), learner_v2.total_steps());

        // 输出不同版本的策略
        let policy_v1 = learner_v1.current_policy(1);
        let policy_v2 = learner_v2.current_policy(2);

        // 版本号不同
        assert_ne!(policy_v1.version(), policy_v2.version());
        // 但初始 weights 相同（都是 default 臂）
        assert_eq!(policy_v1.weights(), policy_v2.weights());
    }

    #[test]
    fn test_s4_seam_reward_range() {
        // 验证 S4 奖励值范围 ∈ [0, 1]
        // 后悔率 0.0 → reward 1.0
        // 后悔率 1.0 → reward 0.0

        let r_min = S4Reward::new(1.0).unwrap();
        assert_eq!(r_min.reward(), 0.0);

        let r_max = S4Reward::new(0.0).unwrap();
        assert_eq!(r_max.reward(), 1.0);

        // 中间值
        for regret_rate in [0.1, 0.25, 0.5, 0.75, 0.9] {
            let r = S4Reward::new(regret_rate).unwrap();
            let expected = 1.0 - regret_rate;
            assert!((r.reward() - expected).abs() < 1e-9);
            assert!(r.reward() >= 0.0 && r.reward() <= 1.0);
        }
    }
}
