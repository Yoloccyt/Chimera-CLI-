//! S7 接缝 — R1 召回配额离线 RL 学习器（v5.0 §7.5 离线 RL 两接缝之一）
//!
//! 对应任务: **P4-W16.2.2**（R1 召回配额 CQL/IQL 算法设计与实现）
//! 对应 ADR: **ADR-042**（R2 冻结）+ **ADR-043**（R1 影子模式）+ **ADR-037**（CapabilityToken 四态）
//! 对应设计源: `NEXUS-OMEGA_v5.0_系统性完整设计文档.md` §7.5（离线 RL 两接缝）
//!
//! # R1 接缝概述
//!
//! | 字段 | 值 |
//! |------|-----|
//! | 接缝名 | S7RecallQuota（召回配额档位） |
//! | 代码锚点 | `crates/omega-learner/src/r1_recall_quota.rs` |
//! | 算法 | CQL（默认）/ IQL（备选），线性函数近似 |
//! | 臂 | 5 档 k 值 ∈ {5, 10, 20, 50, 100}（`RecallQuota` 枚举） |
//! | 上下文 | 任务阶段 one-hot(3) + 任务复杂度 + 内存压力 + bias（6 维） |
//! | 奖励 | `recall_rate − 0.5×false_block_rate − 0.3×latency_penalty` |
//! | 数据源 | `ReplayPool<RecallQuotaTransition>` 离线采样（≥10K 轨迹，P4-W16.2.1） |
//!
//! # 与 S1-S6 在线 bandit 的差异
//!
//! | 维度 | S1-S6（LinUCB） | S7（CQL/IQL） |
//! |------|-----------------|---------------|
//! | 学习范式 | 在线 bandit | 离线 RL |
//! | 数据源 | 实时观察 reward | 从 ReplayPool 采样 |
//! | 样本格式 | 三元组 (context, arm, reward) | 四元组 (state, action, reward, next_state) |
//! | 探索 | UCB 上界 | 无（离线学习） |
//! | 解冻条件 | EWMA ≥ 0.3 阈值 | 影子模式 2 周 + EWMA ≥ 0.7 + 胜率 ≥ 71.4% + 无 ASA |
//!
//! # C4 合规（ADR-037 + ADR-043）
//!
//! `RecallQuotaLearner` 输出 `RecallQuotaPolicy::Learned { version, quota }` 给上层调用方
//! （chimera-cli / quest-engine），由上层通过 `CapabilityToken::Provisional → Authorized`
//! 灰度授权后注入。影子模式期间（Provisional），编排器查询 token 未授权 → fallback 到
//! `RecallQuotaPolicy::Static(K10)`（C4 合规第三层 fallback）。
//!
//! # 算法选择（WHY CQL 默认）
//!
//! - **CQL（Conservative Q-Learning, Kumar et al. 2020 NeurIPS）**：
//!   保守惩罚 `α·[logΣexp(Q(s,·)) − Q(s,a_data)]` 压低 OOD 动作 Q 值，
//!   K=5 动作空间小，log-sum-exp 保守惩罚直观。**默认算法**。
//! - **IQL（Implicit Q-Learning, Kostrikov et al. 2022 ICLR）**：
//!   V 函数 + expectile 回归，不查询 OOD 动作（用 V(s') 替代 max_a' Q(s',a')）。
//!   备选算法，若影子模式对比显示 CQL 退化则切换。
//!
//! # R2 冻结声明（ADR-042）
//!
//! 本文件仅实现 R1（召回配额 CQL/IQL）路径，**不涉及 R2（GSOE×AutoDPO 约束 RL）**。
//! R2 路径在 FormalVerifier 落地前无条件冻结（ADR-042），本文件无需 R2 冻结声明。
//!
//! # 示例
//!
//! ## CQL 训练与推理流程
//!
//! ```
//! use nexus_contracts::RecallQuota;
//! use omega_learner::r1_recall_quota::{
//!     R1Context, RecallQuotaLearner, RecallQuotaTransition,
//! };
//! use omega_learner::replay_pool::ReplayPool;
//! use omega_learner::s2_memory::TaskPhase;
//! use rand::thread_rng;
//!
//! // 1. 创建回放池并填充轨迹
//! let pool: ReplayPool<RecallQuotaTransition> = ReplayPool::new();
//! let ctx = R1Context::new(TaskPhase::LongRun, 0.7, 0.4).unwrap();
//! let next_ctx = R1Context::new(TaskPhase::LongRun, 0.8, 0.5).unwrap();
//! for _ in 0..200 {
//!     pool.push(RecallQuotaTransition::new(
//!         &ctx, RecallQuota::K20, 0.75, &next_ctx, false, "q-1",
//!     ).unwrap());
//! }
//!
//! // 2. 创建 CQL 学习器并训练（default_cql 内部 validate 可能失败，返回 Result）
//! let mut learner = RecallQuotaLearner::default_cql().unwrap();
//! let mut rng = thread_rng();
//! learner.train(&pool, &mut rng).unwrap();
//! assert!(learner.train_steps() > 0);
//!
//! // 3. 推理：选择召回配额
//! let quota = learner.select_quota(&ctx).unwrap();
//! assert!(matches!(quota, RecallQuota::K5 | RecallQuota::K10
//!     | RecallQuota::K20 | RecallQuota::K50 | RecallQuota::K100));
//!
//! // 4. 输出当前策略（RecallQuotaPolicy::Learned）
//! let policy = learner.current_policy(1, &ctx);
//! assert!(policy.is_learned());
//! ```

use crate::error::{LearnerError, Result};
use crate::replay_pool::ReplayPool;
use crate::s2_memory::TaskPhase;
use crate::seam::SeamId;
use ndarray::{Array1, Array2};
use nexus_contracts::{RecallQuota, RecallQuotaPolicy};
use rand::Rng;
use serde::{Deserialize, Serialize};

// ============================================================
// 常量定义
// ============================================================

/// R1 上下文维度（task_phase one-hot(3) + task_complexity + memory_pressure + bias）
///
/// WHY 6 维: 与 `S2Context` 维度对齐，task_phase / complexity / memory_pressure
/// 共享语义（记忆相关接缝），便于跨接缝对照分析。
pub const R1_CONTEXT_DIM: usize = 6;

/// R1 动作空间大小（5 档 k 值）
pub const R1_ARM_COUNT: usize = 5;

/// R1 默认折扣因子 γ（future reward discount）
///
/// WHY 0.95: 与 RL 文献标准值对齐（Sutton & Barto, 2018），
/// 平衡短期收益与长期收益，避免过小导致 myopic / 过大导致 credit assignment 困难。
pub const DEFAULT_GAMMA: f64 = 0.95;

/// R1 默认 CQL 保守惩罚强度 α
///
/// WHY 1.0: Kumar et al. (2020) 推荐的稳健默认值，
/// α=1.0 提供适度保守性，避免 OOD 动作 Q 值过高估计。
pub const DEFAULT_CQL_ALPHA: f64 = 1.0;

/// R1 默认 IQL expectile 参数 τ
///
/// WHY 0.7: Kostrikov et al. (2022) 推荐 τ ∈ [0.5, 0.9]，
/// τ=0.7 跟踪上分位（upper expectile），避免过度悲观。
pub const DEFAULT_IQL_TAU: f64 = 0.7;

/// R1 默认学习率
///
/// WHY 0.01: 与 SGD 文献标准值对齐，平衡收敛速度与稳定性。
/// 过大（0.1）会导致梯度爆炸；过小（0.001）会导致收敛过慢。
pub const DEFAULT_LR: f64 = 0.01;

/// R1 默认 L2 正则化系数
///
/// WHY 0.001: 防止过拟合，与 ML 文献标准值对齐。
/// λ=0.001 提供温和正则化，避免模型复杂度过高。
pub const DEFAULT_L2_REG: f64 = 0.001;

/// R1 默认 mini-batch 大小
///
/// WHY 64: 与 RL 文献标准值对齐（Sutton & Barto, 2018），
/// 平衡梯度估计方差与计算效率。
pub const DEFAULT_BATCH_SIZE: usize = 64;

/// R1 默认训练迭代次数
///
/// WHY 100: 10K 轨迹 + batch=64 → 每 epoch ~156 步，
/// 100 迭代 ≈ 0.64 epoch，足够线性 Q 函数收敛。
pub const DEFAULT_TRAIN_ITERS: usize = 100;

/// R1 默认最小回放池容量（训练前置条件）
///
/// WHY 200: 测试与小型部署可用，生产环境推荐 ≥10K（与 spec "≥10K 轨迹"对齐）。
pub const DEFAULT_MIN_POOL_SIZE: usize = 200;

/// R1 默认梯度裁剪阈值（防止梯度爆炸）
///
/// WHY 10.0: 与 RL 文献标准值对齐，超过此范数的梯度将被裁剪。
pub const DEFAULT_GRAD_CLIP: f64 = 10.0;

// ============================================================
// R1Context
// ============================================================

/// R1 上下文 — 任务阶段 / 任务复杂度 / 内存压力（6 维特征）
///
/// 与 `s2_memory::S2Context` 字段完全相同（WHY: 都是记忆相关接缝，共享上下文语义），
/// 但独立定义避免 S2 修改时影响 R1。
///
/// # 编码
///
/// ```text
/// x = [
///   task_phase_one_hot(3),   // 0..2: Initial / Stuck / LongRun
///   task_complexity,         // 3: 任务复杂度 ∈ [0, 1]（归一化）
///   memory_pressure,         // 4: 内存压力 ∈ [0, 1]（used / budget）
///   bias,                    // 5: 常量 1.0（线性模型偏置项）
/// ]
/// ```
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct R1Context {
    /// 任务阶段（one-hot 编码到 3 维）
    pub task_phase: TaskPhase,
    /// 任务复杂度 ∈ [0, 1]（已归一化）
    pub task_complexity: f32,
    /// 内存压力 ∈ [0, 1]（used / budget）
    pub memory_pressure: f32,
}

impl R1Context {
    /// 创建 R1 上下文
    ///
    /// # 参数
    /// - `task_phase`: 任务阶段（决定 one-hot 编码位置）
    /// - `task_complexity`: 任务复杂度 ∈ [0, 1]（调用方归一化）
    /// - `memory_pressure`: 内存压力 ∈ [0, 1]（used / budget）
    ///
    /// # 错误
    /// - `InvalidReward`: task_complexity 或 memory_pressure 不在 [0, 1] 或非有限
    pub fn new(task_phase: TaskPhase, task_complexity: f32, memory_pressure: f32) -> Result<Self> {
        if !task_complexity.is_finite() || !(0.0..=1.0).contains(&task_complexity) {
            return Err(LearnerError::InvalidReward {
                reward: task_complexity as f64,
            });
        }
        if !memory_pressure.is_finite() || !(0.0..=1.0).contains(&memory_pressure) {
            return Err(LearnerError::InvalidReward {
                reward: memory_pressure as f64,
            });
        }
        Ok(Self {
            task_phase,
            task_complexity,
            memory_pressure,
        })
    }

    /// 编码为 6 维特征向量，供 CQL/IQL 消费
    ///
    /// 向量布局:
    /// - `[0..3]`: task_phase one-hot 编码
    /// - `[3]`: task_complexity
    /// - `[4]`: memory_pressure
    /// - `[5]`: bias 常量 1.0
    pub fn features(&self) -> [f32; R1_CONTEXT_DIM] {
        let mut features = [0.0f32; R1_CONTEXT_DIM];
        features[self.task_phase.one_hot_index()] = 1.0;
        features[3] = self.task_complexity;
        features[4] = self.memory_pressure;
        features[5] = 1.0; // bias
        features
    }

    /// 返回 6 维特征向量的 f64 拷贝（供 ndarray 计算）
    pub fn features_f64(&self) -> [f64; R1_CONTEXT_DIM] {
        let mut features = [0.0f64; R1_CONTEXT_DIM];
        let f = self.features();
        for (i, &v) in f.iter().enumerate() {
            features[i] = v as f64;
        }
        features
    }
}

impl std::fmt::Display for R1Context {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "R1Context({}, complexity={:.2}, mem={:.2})",
            self.task_phase.short_name(),
            self.task_complexity,
            self.memory_pressure
        )
    }
}

// ============================================================
// RecallQuotaTransition — 离线 RL 四元组
// ============================================================

/// R1 离线 RL 转移四元组 — (state, action, reward, next_state)
///
/// WHY 不复用 `ReplaySample`: 在线 bandit 样本是三元组 (context, arm, reward)，
/// 离线 RL 需要四元组 (state, action, reward, next_state) 用于 TD 学习。
/// 强行扩展 ReplaySample 会破坏 S1-S6 既有 34 个测试。
///
/// # 字段
/// - `state`: 当前状态（6 维 R1 上下文）
/// - `action`: 执行的动作（选择的 k 值）
/// - `reward`: 奖励信号 [-0.5, 1.0]（来自 L3 执行反馈）
/// - `next_state`: 下一状态（6 维 R1 上下文）
/// - `done`: 是否为终止状态（Quest 完成或失败）
/// - `quest_id`: 所属 Quest ID（跨模块追踪键）
/// - `timestamp`: 时间戳（UTC）
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecallQuotaTransition {
    /// 当前状态（6 维 R1 上下文）
    pub state: [f32; R1_CONTEXT_DIM],
    /// 执行的动作（选择的 k 值）
    pub action: RecallQuota,
    /// 奖励信号 [-0.5, 1.0]
    pub reward: f32,
    /// 下一状态（6 维 R1 上下文）
    pub next_state: [f32; R1_CONTEXT_DIM],
    /// 是否为终止状态
    pub done: bool,
    /// 所属 Quest ID
    pub quest_id: String,
    /// 时间戳（UTC）
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

impl RecallQuotaTransition {
    /// 创建 R1 转移四元组（自动记录当前 UTC 时间戳）
    ///
    /// # 参数
    /// - `state_ctx`: 当前状态上下文
    /// - `action`: 执行的动作
    /// - `reward`: 奖励信号 [-0.5, 1.0]（非有限值返回 Err）
    /// - `next_state_ctx`: 下一状态上下文
    /// - `done`: 是否为终止状态
    /// - `quest_id`: 所属 Quest ID
    pub fn new(
        state_ctx: &R1Context,
        action: RecallQuota,
        reward: f32,
        next_state_ctx: &R1Context,
        done: bool,
        quest_id: impl Into<String>,
    ) -> Result<Self> {
        if !reward.is_finite() {
            return Err(LearnerError::InvalidReward {
                reward: reward as f64,
            });
        }
        Ok(Self {
            state: state_ctx.features(),
            action,
            reward,
            next_state: next_state_ctx.features(),
            done,
            quest_id: quest_id.into(),
            timestamp: chrono::Utc::now(),
        })
    }

    /// 创建 R1 转移四元组（显式时间戳，便于测试确定性）
    pub fn with_timestamp(
        state_ctx: &R1Context,
        action: RecallQuota,
        reward: f32,
        next_state_ctx: &R1Context,
        done: bool,
        quest_id: impl Into<String>,
        timestamp: chrono::DateTime<chrono::Utc>,
    ) -> Result<Self> {
        if !reward.is_finite() {
            return Err(LearnerError::InvalidReward {
                reward: reward as f64,
            });
        }
        Ok(Self {
            state: state_ctx.features(),
            action,
            reward,
            next_state: next_state_ctx.features(),
            done,
            quest_id: quest_id.into(),
            timestamp,
        })
    }

    /// 返回状态特征向量（f64，供 ndarray 计算）
    pub fn state_f64(&self) -> [f64; R1_CONTEXT_DIM] {
        let mut arr = [0.0f64; R1_CONTEXT_DIM];
        for (i, &v) in self.state.iter().enumerate() {
            arr[i] = v as f64;
        }
        arr
    }

    /// 返回下一状态特征向量（f64，供 ndarray 计算）
    pub fn next_state_f64(&self) -> [f64; R1_CONTEXT_DIM] {
        let mut arr = [0.0f64; R1_CONTEXT_DIM];
        for (i, &v) in self.next_state.iter().enumerate() {
            arr[i] = v as f64;
        }
        arr
    }
}

// ============================================================
// R1Reward
// ============================================================

/// R1 奖励参数 — 控制误杀率与延迟惩罚的权重
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct R1RewardParams {
    /// 误杀率权重（recall_rate − 0.5×false_block_rate 中的 0.5）
    ///
    /// WHY 0.5: 误杀（false_block）是 R1 接缝的核心反指标，
    /// 权重 0.5 提供温和惩罚避免过度激进召回。
    pub false_block_weight: f64,
    /// 延迟惩罚权重（recall_rate − 0.5×false_block_rate − 0.3×latency_penalty 中的 0.3）
    ///
    /// WHY 0.3: 延迟是 R1 接缝的代价指标，
    /// 权重 0.3 比误杀率权重低，反映"宁可延迟也要召回正确条目"的优先级。
    pub latency_penalty_weight: f64,
}

impl Default for R1RewardParams {
    fn default() -> Self {
        Self {
            false_block_weight: 0.5,
            latency_penalty_weight: 0.3,
        }
    }
}

/// R1 奖励 — `recall_rate − 0.5×false_block_rate − 0.3×latency_penalty`
///
/// # 字段
/// - `recall_rate ∈ [0, 1]`: 召回率（R1 接缝的核心指标）
/// - `false_block_rate ∈ [0, 1]`: 误杀率（反指标，越低越好）
/// - `latency_penalty ∈ [0, 1]`: 延迟惩罚（归一化）
///
/// # 边界
/// - 全部最优: reward → 1.0
/// - 全部最差: recall=0 + false_block=1 + latency=1 → reward = -0.8
/// - 实际范围: [-0.8, 1.0]，与 `RecallQuotaTransition.reward` 字段 [-0.5, 1.0] 略宽，
///   通过 `R1Reward::new` 校验保证 ∈ [-1.0, 1.0]。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct R1Reward {
    /// 召回率 ∈ [0, 1]
    pub recall_rate: f64,
    /// 误杀率 ∈ [0, 1]
    pub false_block_rate: f64,
    /// 延迟惩罚 ∈ [0, 1]
    pub latency_penalty: f64,
    /// 奖励参数
    pub params: R1RewardParams,
}

impl R1Reward {
    /// 创建 R1 奖励（使用默认参数）
    pub fn new(recall_rate: f64, false_block_rate: f64, latency_penalty: f64) -> Result<Self> {
        Self::with_params(
            recall_rate,
            false_block_rate,
            latency_penalty,
            R1RewardParams::default(),
        )
    }

    /// 创建 R1 奖励（自定义参数）
    pub fn with_params(
        recall_rate: f64,
        false_block_rate: f64,
        latency_penalty: f64,
        params: R1RewardParams,
    ) -> Result<Self> {
        for (name, v) in [
            ("recall_rate", recall_rate),
            ("false_block_rate", false_block_rate),
            ("latency_penalty", latency_penalty),
        ] {
            if !v.is_finite() || !(0.0..=1.0).contains(&v) {
                let _ = name;
                return Err(LearnerError::InvalidReward { reward: v });
            }
        }
        Ok(Self {
            recall_rate,
            false_block_rate,
            latency_penalty,
            params,
        })
    }

    /// 计算最终奖励值
    pub fn reward(&self) -> f64 {
        self.recall_rate
            - self.params.false_block_weight * self.false_block_rate
            - self.params.latency_penalty_weight * self.latency_penalty
    }
}

// ============================================================
// R1Algorithm + RecallQuotaConfig
// ============================================================

/// R1 算法选择枚举
///
/// WHY 枚举而非 trait object:
/// - 编译期穷尽性检查
/// - 避免 `Box<dyn Policy>` 的运行时开销
/// - 与 `RecallQuotaLearner` 的 enum dispatch 对齐
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum R1Algorithm {
    /// CQL — Conservative Q-Learning (Kumar et al., 2020 NeurIPS)
    Cql,
    /// IQL — Implicit Q-Learning (Kostrikov et al., 2022 ICLR)
    Iql,
}

/// R1 学习器配置 — CQL/IQL 共享超参
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecallQuotaConfig {
    /// 折扣因子 γ ∈ (0, 1)
    pub gamma: f64,
    /// CQL 保守惩罚强度 α ≥ 0
    pub cql_alpha: f64,
    /// IQL expectile 参数 τ ∈ (0.5, 1.0]
    pub iql_tau: f64,
    /// 学习率 > 0
    pub lr: f64,
    /// L2 正则化系数 ≥ 0
    pub l2_reg: f64,
    /// mini-batch 大小 ≥ 1
    pub batch_size: usize,
    /// 训练迭代次数 ≥ 1
    pub train_iters: usize,
    /// 最小回放池容量（训练前置条件）
    pub min_pool_size: usize,
    /// 梯度裁剪阈值（L2 范数）
    pub grad_clip: f64,
    /// 算法选择
    pub algorithm: R1Algorithm,
}

impl Default for RecallQuotaConfig {
    fn default() -> Self {
        Self {
            gamma: DEFAULT_GAMMA,
            cql_alpha: DEFAULT_CQL_ALPHA,
            iql_tau: DEFAULT_IQL_TAU,
            lr: DEFAULT_LR,
            l2_reg: DEFAULT_L2_REG,
            batch_size: DEFAULT_BATCH_SIZE,
            train_iters: DEFAULT_TRAIN_ITERS,
            min_pool_size: DEFAULT_MIN_POOL_SIZE,
            grad_clip: DEFAULT_GRAD_CLIP,
            algorithm: R1Algorithm::Cql,
        }
    }
}

impl RecallQuotaConfig {
    /// 创建 CQL 默认配置
    pub fn cql_default() -> Self {
        Self {
            algorithm: R1Algorithm::Cql,
            ..Self::default()
        }
    }

    /// 创建 IQL 默认配置
    pub fn iql_default() -> Self {
        Self {
            algorithm: R1Algorithm::Iql,
            ..Self::default()
        }
    }

    /// 校验配置合法性
    pub fn validate(&self) -> Result<()> {
        if !self.gamma.is_finite() || !(0.0..=1.0).contains(&self.gamma) {
            return Err(LearnerError::InvalidConfig {
                field: "gamma",
                value: self.gamma.to_string(),
            });
        }
        if !self.cql_alpha.is_finite() || self.cql_alpha < 0.0 {
            return Err(LearnerError::InvalidConfig {
                field: "cql_alpha",
                value: self.cql_alpha.to_string(),
            });
        }
        if !self.iql_tau.is_finite() || !(0.5..=1.0).contains(&self.iql_tau) {
            return Err(LearnerError::InvalidConfig {
                field: "iql_tau",
                value: self.iql_tau.to_string(),
            });
        }
        if !self.lr.is_finite() || self.lr <= 0.0 {
            return Err(LearnerError::InvalidConfig {
                field: "lr",
                value: self.lr.to_string(),
            });
        }
        if !self.l2_reg.is_finite() || self.l2_reg < 0.0 {
            return Err(LearnerError::InvalidConfig {
                field: "l2_reg",
                value: self.l2_reg.to_string(),
            });
        }
        if self.batch_size == 0 {
            return Err(LearnerError::InvalidConfig {
                field: "batch_size",
                value: "0".to_string(),
            });
        }
        if self.train_iters == 0 {
            return Err(LearnerError::InvalidConfig {
                field: "train_iters",
                value: "0".to_string(),
            });
        }
        if !self.grad_clip.is_finite() || self.grad_clip <= 0.0 {
            return Err(LearnerError::InvalidConfig {
                field: "grad_clip",
                value: self.grad_clip.to_string(),
            });
        }
        Ok(())
    }
}

// ============================================================
// CqlPolicy — 保守 Q-Learning 线性函数近似
// ============================================================

/// CQL 策略 — 保守 Q-Learning 的线性函数近似实现
///
/// # 算法 (Kumar et al., 2020, NeurIPS)
///
/// `Q(s,a) = φ(s)^T · θ_a`（线性函数近似，每个动作一个 `θ_a ∈ R^d`）
///
/// 损失函数:
/// ```text
/// L_CQL = α · E_s [log Σ_a exp(Q(s,a)) − Q(s,a_data)]   (保守惩罚)
///       + E_{s,a,r,s'} [(Q(s,a) − (r + γ · max_a' Q(s',a')))²]   (标准 TD)
///       + λ · ||θ||²   (L2 正则)
/// ```
///
/// # 数值稳定性
///
/// log-sum-exp 实现先减去 `max_q` 避免 exp 溢出:
/// `log Σ exp(Q_i) = max_q + log Σ exp(Q_i − max_q)`
///
/// # 字段
/// - `theta`: Q 函数参数,`θ ∈ R^{K × d}`（K 个动作,每个 d 维）
/// - `config`: 配置
/// - `train_steps`: 训练步数（用于诊断与持久化）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CqlPolicy {
    /// Q 函数参数:θ ∈ R^{K × d}（K=R1_ARM_COUNT, d=R1_CONTEXT_DIM）
    theta: Array2<f64>,
    /// 配置
    config: RecallQuotaConfig,
    /// 训练步数
    train_steps: u64,
}

impl CqlPolicy {
    /// 创建 CQL 策略（零初始化 θ）
    pub fn new(config: RecallQuotaConfig) -> Result<Self> {
        config.validate()?;
        Ok(Self {
            theta: Array2::zeros((R1_ARM_COUNT, R1_CONTEXT_DIM)),
            config,
            train_steps: 0,
        })
    }

    /// 计算所有动作的 Q 值: `Q(s, :) = θ · φ(s)`
    ///
    /// 返回长度 K=5 的向量。
    pub fn q_values(&self, state: &[f64; R1_CONTEXT_DIM]) -> [f64; R1_ARM_COUNT] {
        let state_arr = Array1::from(state.to_vec());
        let q = self.theta.dot(&state_arr);
        let mut result = [0.0f64; R1_ARM_COUNT];
        for (i, &v) in q.iter().enumerate() {
            result[i] = v;
        }
        result
    }

    /// 选择 Q 值最高的动作（greedy，无探索）
    ///
    /// WHY 无探索: 离线 RL 训练阶段无探索，部署阶段直接 greedy。
    /// 探索在数据收集阶段已完成（通过 S1-S6 在线 bandit 产生轨迹）。
    pub fn select_action(&self, state: &[f64; R1_CONTEXT_DIM]) -> RecallQuota {
        let q = self.q_values(state);
        let mut best_idx = 0;
        let mut best_q = q[0];
        for (i, &v) in q.iter().enumerate() {
            if v > best_q {
                best_q = v;
                best_idx = i;
            }
        }
        RecallQuota::from_index(best_idx).unwrap_or(RecallQuota::DEFAULT_FALLBACK)
    }

    /// 单步训练: 采样 mini-batch → 计算 TD target + CQL 保守惩罚 → 梯度下降
    ///
    /// # 参数
    /// - `batch`: mini-batch 转移四元组
    /// - `rng`: 随机数生成器（保留接口用于未来探索性扰动）
    pub fn train_step<R: Rng>(
        &mut self,
        batch: &[RecallQuotaTransition],
        _rng: &mut R,
    ) -> Result<()> {
        if batch.is_empty() {
            return Err(LearnerError::InsufficientSamples {
                required: 1,
                actual: 0,
            });
        }

        let lr = self.config.lr;
        let gamma = self.config.gamma;
        let cql_alpha = self.config.cql_alpha;
        let l2_reg = self.config.l2_reg;
        let grad_clip = self.config.grad_clip;

        // 累计梯度（K × d）
        let mut grad = Array2::<f64>::zeros((R1_ARM_COUNT, R1_CONTEXT_DIM));
        let n = batch.len() as f64;

        for tr in batch.iter() {
            let s = Array1::from(tr.state_f64().to_vec());
            let s_next = Array1::from(tr.next_state_f64().to_vec());

            // 1. TD target: y = r + γ · max_a' Q(s', a') (done 时 y = r)
            let q_next = if tr.done {
                0.0
            } else {
                let q_next_all = self.theta.dot(&s_next);
                q_next_all.iter().cloned().fold(f64::NEG_INFINITY, f64::max)
            };
            let td_target = tr.reward as f64 + gamma * q_next;

            // 2. 当前动作的 Q 值
            let a_idx = tr.action.index();
            let q_a = self.theta.row(a_idx).dot(&s);

            // 3. TD 误差梯度: ∂(Q(s,a) − y)²/∂θ_a = 2(Q(s,a) − y) · φ(s)
            let td_error = q_a - td_target;
            let td_grad_a = 2.0 * td_error / n;
            for j in 0..R1_CONTEXT_DIM {
                grad[(a_idx, j)] += td_grad_a * s[j];
            }

            // 4. CQL 保守惩罚梯度
            // L_cql = α · [log Σ_a exp(Q(s, a)) − Q(s, a_data)]
            // ∂L_cql/∂Q(s,a') = α · [softmax(Q(s,:))_a' − 1{a' == a_data}]
            // ∂L_cql/∂θ_a' = α · [softmax_a' − 1{a' == a_data}] · φ(s)
            let q_all = self.theta.dot(&s);
            let max_q = q_all.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
            let mut exp_sum = 0.0;
            let mut softmax = [0.0f64; R1_ARM_COUNT];
            for (i, &q) in q_all.iter().enumerate() {
                let e = (q - max_q).exp();
                softmax[i] = e;
                exp_sum += e;
            }
            if !exp_sum.is_finite() || exp_sum <= 0.0 {
                return Err(LearnerError::R1NumericalInstability {
                    detail: "log-sum-exp overflow in CQL",
                });
            }
            for val in softmax.iter_mut() {
                *val /= exp_sum;
            }
            let cql_scale = cql_alpha / n;
            for (i, mut row) in grad.rows_mut().into_iter().enumerate() {
                let indicator = if i == a_idx { 1.0 } else { 0.0 };
                let coef = cql_scale * (softmax[i] - indicator);
                for (j, g) in row.iter_mut().enumerate() {
                    *g += coef * s[j];
                }
            }

            // 5. L2 正则梯度: ∂(λ||θ||²)/∂θ = 2λ · θ
            // (在梯度累加后统一处理，避免重复计算)
        }

        // L2 正则梯度（全局）
        for (i, mut row) in grad.rows_mut().into_iter().enumerate() {
            for (j, g) in row.iter_mut().enumerate() {
                *g += 2.0 * l2_reg * self.theta[(i, j)] / n;
            }
        }

        // 6. 梯度裁剪（L2 范数）
        let grad_norm: f64 = grad.iter().map(|&v| v * v).sum::<f64>().sqrt();
        if grad_norm.is_finite() && grad_norm > grad_clip {
            let scale = grad_clip / grad_norm;
            grad.mapv_inplace(|v| v * scale);
        } else if !grad_norm.is_finite() {
            return Err(LearnerError::R1NumericalInstability {
                detail: "gradient norm non-finite",
            });
        }

        // 7. 参数更新: θ ← θ − lr · grad
        self.theta = &self.theta - &(grad.mapv(|v| v * lr));

        self.train_steps += 1;
        Ok(())
    }

    /// 返回训练步数
    pub fn train_steps(&self) -> u64 {
        self.train_steps
    }

    /// 返回配置引用
    pub fn config(&self) -> &RecallQuotaConfig {
        &self.config
    }
}

// ============================================================
// IqlPolicy — 隐式 Q-Learning
// ============================================================

/// IQL 策略 — 隐式 Q-Learning 线性函数近似实现
///
/// # 算法 (Kostrikov et al., 2022, ICLR)
///
/// - V 函数: `V(s) = ψ^T · φ(s)`, ψ ∈ R^d
/// - Q 函数: `Q(s,a) = θ_a^T · φ(s)`, θ ∈ R^{K × d}
///
/// 训练流程:
/// 1. V 通过 expectile 回归学习（τ=0.7 跟踪上分位）
/// 2. Q 用 V(s') 替代 max_a' Q(s',a')，避免查询 OOD 动作
///
/// # Expectile 权重
///
/// 残差 `δ = Q(s,a) − V(s)`:
/// - δ > 0（Q > V）: 权重 = τ（上分位）
/// - δ < 0（Q < V）: 权重 = (1 − τ)（下分位）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IqlPolicy {
    /// V 函数参数:ψ ∈ R^d
    psi: Array1<f64>,
    /// Q 函数参数:θ ∈ R^{K × d}
    theta: Array2<f64>,
    /// 配置
    config: RecallQuotaConfig,
    /// 训练步数
    train_steps: u64,
}

impl IqlPolicy {
    /// 创建 IQL 策略（零初始化 ψ 和 θ）
    pub fn new(config: RecallQuotaConfig) -> Result<Self> {
        config.validate()?;
        Ok(Self {
            psi: Array1::zeros(R1_CONTEXT_DIM),
            theta: Array2::zeros((R1_ARM_COUNT, R1_CONTEXT_DIM)),
            config,
            train_steps: 0,
        })
    }

    /// 计算 V(s) = ψ^T · φ(s)
    pub fn v_value(&self, state: &[f64; R1_CONTEXT_DIM]) -> f64 {
        let s = Array1::from(state.to_vec());
        self.psi.dot(&s)
    }

    /// 计算所有动作的 Q 值
    pub fn q_values(&self, state: &[f64; R1_CONTEXT_DIM]) -> [f64; R1_ARM_COUNT] {
        let s = Array1::from(state.to_vec());
        let q = self.theta.dot(&s);
        let mut result = [0.0f64; R1_ARM_COUNT];
        for (i, &v) in q.iter().enumerate() {
            result[i] = v;
        }
        result
    }

    /// 选择 Q 值最高的动作
    pub fn select_action(&self, state: &[f64; R1_CONTEXT_DIM]) -> RecallQuota {
        let q = self.q_values(state);
        let mut best_idx = 0;
        let mut best_q = q[0];
        for (i, &v) in q.iter().enumerate() {
            if v > best_q {
                best_q = v;
                best_idx = i;
            }
        }
        RecallQuota::from_index(best_idx).unwrap_or(RecallQuota::DEFAULT_FALLBACK)
    }

    /// 单步训练: expectile 回归更新 V → TD with V target 更新 Q
    pub fn train_step<R: Rng>(
        &mut self,
        batch: &[RecallQuotaTransition],
        _rng: &mut R,
    ) -> Result<()> {
        if batch.is_empty() {
            return Err(LearnerError::InsufficientSamples {
                required: 1,
                actual: 0,
            });
        }

        let lr = self.config.lr;
        let gamma = self.config.gamma;
        let tau = self.config.iql_tau;
        let l2_reg = self.config.l2_reg;
        let grad_clip = self.config.grad_clip;

        // 1. V 梯度（expectile 回归）
        let mut v_grad = Array1::<f64>::zeros(R1_CONTEXT_DIM);
        // 2. Q 梯度（TD with V target）
        let mut q_grad = Array2::<f64>::zeros((R1_ARM_COUNT, R1_CONTEXT_DIM));
        let n = batch.len() as f64;

        for tr in batch.iter() {
            let s = Array1::from(tr.state_f64().to_vec());
            let s_next = Array1::from(tr.next_state_f64().to_vec());

            // 1. V(s) expectile 回归目标: V(s) → expectile_τ(Q(s, a_data))
            let a_idx = tr.action.index();
            let q_a = self.theta.row(a_idx).dot(&s);
            let v_s = self.psi.dot(&s);
            let delta = q_a - v_s;
            // expectile 权重: δ > 0 用 τ, δ < 0 用 (1-τ)
            let weight = if delta > 0.0 { tau } else { 1.0 - tau };
            let v_loss_grad = 2.0 * weight * delta / n; // ∂(weight·δ²)/∂V = -2·weight·δ
            for j in 0..R1_CONTEXT_DIM {
                v_grad[j] -= v_loss_grad * s[j]; // 负号因为 δ = Q - V, ∂δ/∂V = -1
            }

            // 2. Q(s, a) TD with V target
            // y = r + γ · V(s') (done 时 y = r)
            let v_next = if tr.done { 0.0 } else { self.psi.dot(&s_next) };
            let td_target = tr.reward as f64 + gamma * v_next;
            let q_a_cur = self.theta.row(a_idx).dot(&s);
            let td_error = q_a_cur - td_target;
            let td_grad_a = 2.0 * td_error / n;
            for j in 0..R1_CONTEXT_DIM {
                q_grad[(a_idx, j)] += td_grad_a * s[j];
            }
        }

        // L2 正则
        for j in 0..R1_CONTEXT_DIM {
            v_grad[j] += 2.0 * l2_reg * self.psi[j] / n;
        }
        for i in 0..R1_ARM_COUNT {
            for j in 0..R1_CONTEXT_DIM {
                q_grad[(i, j)] += 2.0 * l2_reg * self.theta[(i, j)] / n;
            }
        }

        // 梯度裁剪（合并 V 与 Q 梯度的范数）
        let mut total_norm_sq = v_grad.iter().map(|&v| v * v).sum::<f64>();
        total_norm_sq += q_grad.iter().map(|&v| v * v).sum::<f64>();
        let total_norm = total_norm_sq.sqrt();
        if !total_norm.is_finite() {
            return Err(LearnerError::R1NumericalInstability {
                detail: "gradient norm non-finite in IQL",
            });
        }
        if total_norm > grad_clip {
            let scale = grad_clip / total_norm;
            v_grad.mapv_inplace(|v| v * scale);
            q_grad.mapv_inplace(|v| v * scale);
        }

        // 参数更新
        self.psi = &self.psi - &(v_grad.mapv(|v| v * lr));
        self.theta = &self.theta - &(q_grad.mapv(|v| v * lr));

        self.train_steps += 1;
        Ok(())
    }

    /// 返回训练步数
    pub fn train_steps(&self) -> u64 {
        self.train_steps
    }

    /// 返回配置引用
    pub fn config(&self) -> &RecallQuotaConfig {
        &self.config
    }
}

// ============================================================
// RecallQuotaLearner — 统一学习器（enum dispatch）
// ============================================================

/// R1 召回配额统一学习器 — CQL/IQL enum dispatch
///
/// WHY enum dispatch 而非 trait object:
/// - 与 §4.1 "避免 Box<dyn Trait>,优先 impl Trait 或 enum dispatch" 对齐
/// - 编译期穷尽性，避免运行时虚函数调用开销
/// - 便于序列化（Cql/Iql Policy 均派生 Serialize/Deserialize）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RecallQuotaLearner {
    /// CQL 学习器
    Cql(CqlPolicy),
    /// IQL 学习器
    Iql(IqlPolicy),
}

impl RecallQuotaLearner {
    /// 创建默认 CQL 学习器
    pub fn default_cql() -> Result<Self> {
        Ok(Self::Cql(CqlPolicy::new(RecallQuotaConfig::cql_default())?))
    }

    /// 创建默认 IQL 学习器
    pub fn default_iql() -> Result<Self> {
        Ok(Self::Iql(IqlPolicy::new(RecallQuotaConfig::iql_default())?))
    }

    /// 创建指定配置的学习器
    pub fn new(config: RecallQuotaConfig) -> Result<Self> {
        config.validate()?;
        Ok(match config.algorithm {
            R1Algorithm::Cql => Self::Cql(CqlPolicy::new(config)?),
            R1Algorithm::Iql => Self::Iql(IqlPolicy::new(config)?),
        })
    }

    /// 从回放池采样训练多步
    ///
    /// # 参数
    /// - `pool`: 经验回放池（≥ min_pool_size 条轨迹）
    /// - `rng`: 随机数生成器
    ///
    /// # 错误
    /// - `EmptyReplayPool`: 池为空
    /// - `InsufficientSamples`: 池中样本数 < batch_size
    pub fn train<R: Rng>(
        &mut self,
        pool: &ReplayPool<RecallQuotaTransition>,
        rng: &mut R,
    ) -> Result<()> {
        let (batch_size, train_iters, min_pool_size) = match self {
            Self::Cql(p) => (
                p.config().batch_size,
                p.config().train_iters,
                p.config().min_pool_size,
            ),
            Self::Iql(p) => (
                p.config().batch_size,
                p.config().train_iters,
                p.config().min_pool_size,
            ),
        };

        let pool_len = pool.len();
        if pool_len == 0 {
            return Err(LearnerError::EmptyReplayPool);
        }
        if pool_len < min_pool_size {
            return Err(LearnerError::InsufficientSamples {
                required: min_pool_size,
                actual: pool_len,
            });
        }

        // 实际 batch 大小不超过池大小
        let actual_batch = batch_size.min(pool_len);

        for _ in 0..train_iters {
            let batch = pool.sample(actual_batch, rng);
            if batch.is_empty() {
                return Err(LearnerError::InsufficientSamples {
                    required: actual_batch,
                    actual: 0,
                });
            }
            match self {
                Self::Cql(p) => p.train_step(&batch, rng)?,
                Self::Iql(p) => p.train_step(&batch, rng)?,
            }
        }
        Ok(())
    }

    /// 选择召回配额（greedy，无探索）
    pub fn select_quota(&self, ctx: &R1Context) -> Result<RecallQuota> {
        let state = ctx.features_f64();
        Ok(match self {
            Self::Cql(p) => p.select_action(&state),
            Self::Iql(p) => p.select_action(&state),
        })
    }

    /// 输出当前策略（`RecallQuotaPolicy::Learned`）
    ///
    /// # 参数
    /// - `version`: 策略版本号（与 `CapabilityToken::bound_policy_version` 对齐）
    /// - `ctx`: 当前上下文（用于选择动作）
    pub fn current_policy(&self, version: u64, ctx: &R1Context) -> RecallQuotaPolicy {
        let quota = self
            .select_quota(ctx)
            .unwrap_or(RecallQuota::DEFAULT_FALLBACK);
        RecallQuotaPolicy::Learned { version, quota }
    }

    /// 返回训练步数
    pub fn train_steps(&self) -> u64 {
        match self {
            Self::Cql(p) => p.train_steps(),
            Self::Iql(p) => p.train_steps(),
        }
    }

    /// 返回所属接缝（S7RecallQuota）
    pub fn seam(&self) -> SeamId {
        SeamId::S7RecallQuota
    }

    /// 返回算法类型
    pub fn algorithm(&self) -> R1Algorithm {
        match self {
            Self::Cql(_) => R1Algorithm::Cql,
            Self::Iql(_) => R1Algorithm::Iql,
        }
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use rand::thread_rng;

    // ----- R1Context 测试 -----

    #[test]
    fn test_r1_context_new_valid() {
        let ctx = R1Context::new(TaskPhase::LongRun, 0.8, 0.6).unwrap();
        assert_eq!(ctx.task_phase, TaskPhase::LongRun);
        assert!((ctx.task_complexity - 0.8).abs() < 1e-6);
        assert!((ctx.memory_pressure - 0.6).abs() < 1e-6);
    }

    #[test]
    fn test_r1_context_new_invalid_complexity() {
        let err = R1Context::new(TaskPhase::Initial, 1.5, 0.5).unwrap_err();
        assert!(matches!(err, LearnerError::InvalidReward { .. }));
    }

    #[test]
    fn test_r1_context_new_invalid_pressure() {
        let err = R1Context::new(TaskPhase::Initial, 0.5, f32::NAN).unwrap_err();
        assert!(matches!(err, LearnerError::InvalidReward { .. }));
    }

    #[test]
    fn test_r1_context_features_layout() {
        let ctx = R1Context::new(TaskPhase::Stuck, 0.7, 0.3).unwrap();
        let f = ctx.features();
        assert_eq!(f.len(), R1_CONTEXT_DIM);
        // Stuck = 1, one-hot 位置 1
        assert_eq!(f[0], 0.0);
        assert_eq!(f[1], 1.0);
        assert_eq!(f[2], 0.0);
        assert!((f[3] - 0.7).abs() < 1e-6);
        assert!((f[4] - 0.3).abs() < 1e-6);
        assert_eq!(f[5], 1.0); // bias
    }

    #[test]
    fn test_r1_context_features_f64() {
        let ctx = R1Context::new(TaskPhase::LongRun, 0.5, 0.5).unwrap();
        let f = ctx.features_f64();
        assert_eq!(f.len(), R1_CONTEXT_DIM);
        for &v in f.iter() {
            assert!(v.is_finite());
        }
    }

    // ----- RecallQuotaTransition 测试 -----

    #[test]
    fn test_transition_new_basic() {
        let ctx = R1Context::new(TaskPhase::Initial, 0.5, 0.3).unwrap();
        let next = R1Context::new(TaskPhase::Stuck, 0.6, 0.4).unwrap();
        let tr =
            RecallQuotaTransition::new(&ctx, RecallQuota::K10, 0.8, &next, false, "q-1").unwrap();
        assert_eq!(tr.action, RecallQuota::K10);
        assert!((tr.reward - 0.8).abs() < 1e-6);
        assert!(!tr.done);
        assert_eq!(tr.quest_id, "q-1");
    }

    #[test]
    fn test_transition_new_invalid_reward() {
        let ctx = R1Context::new(TaskPhase::Initial, 0.5, 0.3).unwrap();
        let next = R1Context::new(TaskPhase::Stuck, 0.6, 0.4).unwrap();
        let err = RecallQuotaTransition::new(&ctx, RecallQuota::K10, f32::NAN, &next, false, "q-1")
            .unwrap_err();
        assert!(matches!(err, LearnerError::InvalidReward { .. }));
    }

    #[test]
    fn test_transition_state_f64_round_trip() {
        let ctx = R1Context::new(TaskPhase::LongRun, 0.7, 0.4).unwrap();
        let next = R1Context::new(TaskPhase::LongRun, 0.8, 0.5).unwrap();
        let tr =
            RecallQuotaTransition::new(&ctx, RecallQuota::K20, 0.75, &next, false, "q-1").unwrap();
        let s = tr.state_f64();
        let n = tr.next_state_f64();
        assert_eq!(s.len(), R1_CONTEXT_DIM);
        assert_eq!(n.len(), R1_CONTEXT_DIM);
    }

    // ----- R1Reward 测试 -----

    #[test]
    fn test_r1_reward_full_score() {
        // 全部最优: recall=1, false_block=0, latency=0 → reward=1.0
        let r = R1Reward::new(1.0, 0.0, 0.0).unwrap();
        assert!((r.reward() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn test_r1_reward_worst_case() {
        // 全部最差: recall=0, false_block=1, latency=1 → reward = -0.8
        let r = R1Reward::new(0.0, 1.0, 1.0).unwrap();
        assert!((r.reward() - (-0.8)).abs() < 1e-9);
    }

    #[test]
    fn test_r1_reward_mid_case() {
        // recall=0.9, false_block=0.1, latency=0.2
        // reward = 0.9 - 0.5*0.1 - 0.3*0.2 = 0.9 - 0.05 - 0.06 = 0.79
        let r = R1Reward::new(0.9, 0.1, 0.2).unwrap();
        assert!((r.reward() - 0.79).abs() < 1e-9);
    }

    #[test]
    fn test_r1_reward_invalid_values() {
        // recall > 1
        let err = R1Reward::new(1.5, 0.0, 0.0).unwrap_err();
        assert!(matches!(err, LearnerError::InvalidReward { .. }));
        // false_block < 0
        let err = R1Reward::new(0.5, -0.1, 0.0).unwrap_err();
        assert!(matches!(err, LearnerError::InvalidReward { .. }));
        // latency = NaN
        let err = R1Reward::new(0.5, 0.0, f64::NAN).unwrap_err();
        assert!(matches!(err, LearnerError::InvalidReward { .. }));
    }

    // ----- RecallQuotaConfig 测试 -----

    #[test]
    fn test_config_default_valid() {
        let cfg = RecallQuotaConfig::default();
        assert!(cfg.validate().is_ok());
        assert_eq!(cfg.algorithm, R1Algorithm::Cql);
    }

    #[test]
    fn test_config_cql_default() {
        let cfg = RecallQuotaConfig::cql_default();
        assert_eq!(cfg.algorithm, R1Algorithm::Cql);
        assert!((cfg.gamma - DEFAULT_GAMMA).abs() < 1e-9);
    }

    #[test]
    fn test_config_iql_default() {
        let cfg = RecallQuotaConfig::iql_default();
        assert_eq!(cfg.algorithm, R1Algorithm::Iql);
    }

    #[test]
    fn test_config_invalid_gamma() {
        let cfg = RecallQuotaConfig {
            gamma: 1.5,
            ..RecallQuotaConfig::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(matches!(err, LearnerError::InvalidConfig { field, .. } if field == "gamma"));
    }

    #[test]
    fn test_config_invalid_tau() {
        let cfg = RecallQuotaConfig {
            iql_tau: 0.3,
            ..RecallQuotaConfig::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(matches!(err, LearnerError::InvalidConfig { field, .. } if field == "iql_tau"));
    }

    #[test]
    fn test_config_invalid_lr() {
        let cfg = RecallQuotaConfig {
            lr: 0.0,
            ..RecallQuotaConfig::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(matches!(err, LearnerError::InvalidConfig { field, .. } if field == "lr"));
    }

    #[test]
    fn test_config_invalid_batch_size() {
        let cfg = RecallQuotaConfig {
            batch_size: 0,
            ..RecallQuotaConfig::default()
        };
        let err = cfg.validate().unwrap_err();
        assert!(matches!(err, LearnerError::InvalidConfig { field, .. } if field == "batch_size"));
    }

    // ----- CqlPolicy 测试 -----

    #[test]
    fn test_cql_policy_new_zero_theta() {
        let cfg = RecallQuotaConfig::cql_default();
        let policy = CqlPolicy::new(cfg).unwrap();
        let ctx = R1Context::new(TaskPhase::LongRun, 0.5, 0.5).unwrap();
        let state = ctx.features_f64();
        let q = policy.q_values(&state);
        // 零初始化: 所有 Q 值应为 0
        for &v in q.iter() {
            assert!(v.abs() < 1e-9);
        }
    }

    #[test]
    fn test_cql_policy_select_action_zero_init() {
        let cfg = RecallQuotaConfig::cql_default();
        let policy = CqlPolicy::new(cfg).unwrap();
        let ctx = R1Context::new(TaskPhase::LongRun, 0.5, 0.5).unwrap();
        let state = ctx.features_f64();
        // 零初始化: 所有 Q 值相等, select_action 返回第一个（K5）
        let q = policy.select_action(&state);
        assert_eq!(q, RecallQuota::K5);
    }

    #[test]
    fn test_cql_policy_train_step_smoke() {
        let cfg = RecallQuotaConfig {
            batch_size: 8,
            train_iters: 1,
            min_pool_size: 8,
            ..RecallQuotaConfig::cql_default()
        };
        let mut policy = CqlPolicy::new(cfg).unwrap();
        let mut rng = thread_rng();

        // 构造 mini-batch
        let ctx = R1Context::new(TaskPhase::LongRun, 0.7, 0.4).unwrap();
        let next = R1Context::new(TaskPhase::LongRun, 0.8, 0.5).unwrap();
        let batch: Vec<RecallQuotaTransition> = (0..8)
            .map(|_| {
                RecallQuotaTransition::new(&ctx, RecallQuota::K20, 0.75, &next, false, "q-1")
                    .unwrap()
            })
            .collect();
        policy.train_step(&batch, &mut rng).unwrap();
        assert_eq!(policy.train_steps(), 1);
    }

    #[test]
    fn test_cql_policy_train_step_empty_batch() {
        let cfg = RecallQuotaConfig::cql_default();
        let mut policy = CqlPolicy::new(cfg).unwrap();
        let mut rng = thread_rng();
        let err = policy.train_step(&[], &mut rng).unwrap_err();
        assert!(matches!(err, LearnerError::InsufficientSamples { .. }));
    }

    // ----- IqlPolicy 测试 -----

    #[test]
    fn test_iql_policy_new_zero_init() {
        let cfg = RecallQuotaConfig::iql_default();
        let policy = IqlPolicy::new(cfg).unwrap();
        let ctx = R1Context::new(TaskPhase::LongRun, 0.5, 0.5).unwrap();
        let state = ctx.features_f64();
        let v = policy.v_value(&state);
        assert!(v.abs() < 1e-9);
        let q = policy.q_values(&state);
        for &v in q.iter() {
            assert!(v.abs() < 1e-9);
        }
    }

    #[test]
    fn test_iql_policy_train_step_smoke() {
        let cfg = RecallQuotaConfig {
            batch_size: 8,
            train_iters: 1,
            min_pool_size: 8,
            ..RecallQuotaConfig::iql_default()
        };
        let mut policy = IqlPolicy::new(cfg).unwrap();
        let mut rng = thread_rng();
        let ctx = R1Context::new(TaskPhase::LongRun, 0.7, 0.4).unwrap();
        let next = R1Context::new(TaskPhase::LongRun, 0.8, 0.5).unwrap();
        let batch: Vec<RecallQuotaTransition> = (0..8)
            .map(|_| {
                RecallQuotaTransition::new(&ctx, RecallQuota::K20, 0.75, &next, false, "q-1")
                    .unwrap()
            })
            .collect();
        policy.train_step(&batch, &mut rng).unwrap();
        assert_eq!(policy.train_steps(), 1);
    }

    // ----- RecallQuotaLearner 测试 -----

    #[test]
    fn test_learner_default_cql() {
        let learner = RecallQuotaLearner::default_cql().unwrap();
        assert_eq!(learner.algorithm(), R1Algorithm::Cql);
        assert_eq!(learner.seam(), SeamId::S7RecallQuota);
        assert_eq!(learner.train_steps(), 0);
    }

    #[test]
    fn test_learner_default_iql() {
        let learner = RecallQuotaLearner::default_iql().unwrap();
        assert_eq!(learner.algorithm(), R1Algorithm::Iql);
    }

    #[test]
    fn test_learner_train_cql_full() {
        let cfg = RecallQuotaConfig {
            batch_size: 32,
            train_iters: 5,
            min_pool_size: 100,
            ..RecallQuotaConfig::cql_default()
        };
        let mut learner = RecallQuotaLearner::new(cfg).unwrap();
        let pool: ReplayPool<RecallQuotaTransition> = ReplayPool::new();
        let ctx = R1Context::new(TaskPhase::LongRun, 0.7, 0.4).unwrap();
        let next = R1Context::new(TaskPhase::LongRun, 0.8, 0.5).unwrap();
        for _ in 0..200 {
            pool.push(
                RecallQuotaTransition::new(&ctx, RecallQuota::K20, 0.75, &next, false, "q-1")
                    .unwrap(),
            );
        }
        let mut rng = thread_rng();
        learner.train(&pool, &mut rng).unwrap();
        assert!(learner.train_steps() >= 5);
    }

    #[test]
    fn test_learner_train_iql_full() {
        let cfg = RecallQuotaConfig {
            batch_size: 32,
            train_iters: 5,
            min_pool_size: 100,
            ..RecallQuotaConfig::iql_default()
        };
        let mut learner = RecallQuotaLearner::new(cfg).unwrap();
        let pool: ReplayPool<RecallQuotaTransition> = ReplayPool::new();
        let ctx = R1Context::new(TaskPhase::LongRun, 0.7, 0.4).unwrap();
        let next = R1Context::new(TaskPhase::LongRun, 0.8, 0.5).unwrap();
        for _ in 0..200 {
            pool.push(
                RecallQuotaTransition::new(&ctx, RecallQuota::K20, 0.75, &next, false, "q-1")
                    .unwrap(),
            );
        }
        let mut rng = thread_rng();
        learner.train(&pool, &mut rng).unwrap();
        assert!(learner.train_steps() >= 5);
    }

    #[test]
    fn test_learner_train_empty_pool() {
        let mut learner = RecallQuotaLearner::default_cql().unwrap();
        let pool: ReplayPool<RecallQuotaTransition> = ReplayPool::new();
        let mut rng = thread_rng();
        let err = learner.train(&pool, &mut rng).unwrap_err();
        assert!(matches!(err, LearnerError::EmptyReplayPool));
    }

    #[test]
    fn test_learner_train_insufficient_samples() {
        let cfg = RecallQuotaConfig {
            min_pool_size: 1000,
            ..RecallQuotaConfig::cql_default()
        };
        let mut learner = RecallQuotaLearner::new(cfg).unwrap();
        let pool: ReplayPool<RecallQuotaTransition> = ReplayPool::new();
        let ctx = R1Context::new(TaskPhase::LongRun, 0.7, 0.4).unwrap();
        let next = R1Context::new(TaskPhase::LongRun, 0.8, 0.5).unwrap();
        for _ in 0..50 {
            pool.push(
                RecallQuotaTransition::new(&ctx, RecallQuota::K20, 0.75, &next, false, "q-1")
                    .unwrap(),
            );
        }
        let mut rng = thread_rng();
        let err = learner.train(&pool, &mut rng).unwrap_err();
        assert!(matches!(err, LearnerError::InsufficientSamples { .. }));
    }

    #[test]
    fn test_learner_select_quota() {
        let learner = RecallQuotaLearner::default_cql().unwrap();
        let ctx = R1Context::new(TaskPhase::LongRun, 0.7, 0.4).unwrap();
        let q = learner.select_quota(&ctx).unwrap();
        // 零初始化: 返回第一个动作（K5）
        assert!(matches!(q, RecallQuota::K5));
    }

    #[test]
    fn test_learner_current_policy_learned() {
        let learner = RecallQuotaLearner::default_cql().unwrap();
        let ctx = R1Context::new(TaskPhase::LongRun, 0.7, 0.4).unwrap();
        let policy = learner.current_policy(42, &ctx);
        assert!(policy.is_learned());
        assert_eq!(policy.version(), Some(42));
    }

    #[test]
    fn test_learner_seam_returns_s7() {
        let learner = RecallQuotaLearner::default_cql().unwrap();
        assert_eq!(learner.seam(), SeamId::S7RecallQuota);
    }

    #[test]
    fn test_learner_serde_round_trip() {
        let learner = RecallQuotaLearner::default_cql().unwrap();
        let json = serde_json::to_string(&learner).unwrap();
        let de: RecallQuotaLearner = serde_json::from_str(&json).unwrap();
        assert_eq!(de.algorithm(), R1Algorithm::Cql);
        assert_eq!(de.train_steps(), 0);
    }
}
