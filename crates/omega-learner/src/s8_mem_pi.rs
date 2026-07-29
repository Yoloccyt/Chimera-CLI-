//! S8 接缝 — Mem-π 记忆决策策略学习器（polish-v2.7 closure Stage B-6）
//!
//! 对应任务: **closure Stage B-6**（S8 接缝 Mem-π,ADR-049 决策 1 降级档收尾）
//! 对应 ADR: **ADR-031**(omega-learner 边界) + **ADR-043**(影子模式) + **ADR-049**(裁剪裁决)
//! 对应设计源: `chimera_ultimate_polish_v2.7.md` §6.1(Mem-π 生成式记忆策略)
//!
//! # S8 接缝概述
//!
//! | 字段 | 值 |
//! |------|-----|
//! | 接缝名 | S8MemPi(Mem-π 记忆决策策略) |
//! | 代码锚点 | `crates/mlc-engine/src/`(消费方)+ 本模块(学习方) |
//! | 臂 | 3 种记忆决策(Generate/Retrieve/Abstain) |
//! | 上下文 | 任务阶段(Initial/Stuck/LongRun)+ 不确定性 + 预测未来访问 + 内存压力 |
//! | 奖励 | 下游任务达成率 − 噪声占比惩罚 |
//!
//! # 与方案文档 §6.1 的降级映射(ADR-049 决策 1)
//!
//! | 方案原设计 | S8 降级实现 |
//! |---|---|
//! | DecisionPolicy 决策网络(神经) | LinUCB 上下文 bandit(3 臂) |
//! | ContentPolicy 内容生成网络 | **不实现**——内容生成属 LLM 调用,由 mlc-engine 消费方承担 |
//! | GRPO 结构化 rollout 训练 | **推迟 v3.x**(在线梯度训练受 ADR-042 治理约束) |
//! | Abstain 不确定性护栏 | 保留并硬化为**确定性护栏**(见下) |
//!
//! # Abstain 保守护栏(本接缝核心不变量)
//!
//! 方案 §6.1 的关键洞察:"Abstain:不确定时不操作(避免有害生成)"。
//! 本实现将其硬化为确定性护栏——`uncertainty > ABSTAIN_UNCERTAINTY_THRESHOLD (0.7)`
//! 时 `select()` **无条件返回 Abstain**,不经过 LinUCB。
//!
//! WHY 硬护栏而非学习偏好:有害记忆生成(幽灵记忆)是三重悖论"记忆悖论"的
//! 病理源头,护栏必须不可被学习器"探索"绕过(与 UNLEARNABLE_SECURITY_RULES
//! 同哲学:安全底线不可学习)。护栏触发不计入 LinUCB 更新,避免污染模型。
//!
//! # 上下文向量设计(7 维)
//!
//! ```text
//! x = [
//!   task_phase_one_hot(3),      // 0..2: Initial / Stuck / LongRun
//!   uncertainty,                // 3: 当前任务不确定性 ∈ [0, 1]
//!   predicted_future_access,    // 4: 预测未来访问概率 ∈ [0, 1]
//!   memory_pressure,            // 5: 内存压力 ∈ [0, 1](used / budget)
//!   bias,                       // 6: 常量 1.0(线性模型偏置项)
//! ]
//! ```
//!
//! 维度 `d = 7`,满足 LinUCB regret 上界 `O(√(T·d·ln(K·T)))` 的有界假设。
//!
//! # 奖励函数
//!
//! `reward = downstream_achievement − λ × noise_ratio`
//!
//! - `downstream_achievement ∈ [0, 1]`: 下游任务达成率(记忆决策的最终效用)
//! - `noise_ratio ∈ [0, 1]`: 本次决策引入的噪声占比(生成/召回条目中无关条目比例)
//! - `λ = 0.4`(默认):高于 S2 的 0.3——Mem-π 的核心风险是有害生成,
//!   噪声惩罚需更强以引导学习器在低置信场景偏向 Abstain
//!
//! WHY 加法形式: LinUCB 假设奖励是上下文线性函数,加法形式符合线性假设
//! (与 S2 一致,乘法形式会破坏 regret 上界)。
//!
//! # 灰度与影子模式(C4 合规,ADR-043)
//!
//! - **影子期(当前)**: `S8Learner` 仅记录决策与奖励,**不向 mlc-engine 注入策略**;
//!   上层编排器用 `shadow_mode::StrategyMetrics` 采集 S8 决策 vs 规则基线对比
//! - **解冻前置**: `CapabilityToken(SeamId::S8MemPi)` 达 Authorized
//!   + ADR-043 影子模式 2 周观察 + 三方评审
//! - **回滚**: Token 降级 Frozen + 本模块不再被编排器调用(模块本身零侵入)
//!
//! # R2 冻结声明(ADR-042)
//!
//! 本接缝为在线 LinUCB bandit(与 S1-S6 同款),**非** R2 约束 RL 路径;
//! 无梯度更新、无策略网络、不消费 ReplayPool 轨迹数据。
//!
//! # 示例
//!
//! ```
//! use omega_learner::s2_memory::TaskPhase;
//! use omega_learner::s8_mem_pi::{MemPiDecision, S8Context, S8Learner, S8Reward};
//!
//! // 1. 创建 S8 学习器(默认 α=1.0)
//! let mut learner = S8Learner::with_default_alpha().unwrap();
//!
//! // 2. 低不确定性上下文 → LinUCB 正常选择
//! let ctx = S8Context::new(TaskPhase::LongRun, 0.3, 0.8, 0.5).unwrap();
//! let decision = learner.select(&ctx).unwrap();
//!
//! // 3. 观察奖励并更新(下游达成 0.9,噪声占比 0.1)
//! let reward = S8Reward::new(0.9, 0.1).unwrap();
//! learner.update(&ctx, decision, &reward).unwrap();
//! assert_eq!(learner.total_steps(), 1);
//!
//! // 4. 高不确定性 → Abstain 护栏无条件触发(不可被学习绕过)
//! let risky = S8Context::new(TaskPhase::Stuck, 0.9, 0.5, 0.5).unwrap();
//! assert_eq!(learner.select(&risky).unwrap(), MemPiDecision::Abstain);
//! ```

use crate::arm::{ArmId, ArmIndex, DiscreteArmSet};
use crate::context::SeamContext;
use crate::error::{LearnerError, Result};
use crate::linucb::LinUCB;
use crate::s2_memory::TaskPhase;
use serde::{Deserialize, Serialize};

// ============================================================
// 常量定义
// ============================================================

/// S8 上下文维度(task_phase one-hot(3) + uncertainty + predicted_future_access
/// + memory_pressure + bias)
pub const S8_CONTEXT_DIM: usize = 7;

/// S8 臂数(3 种记忆决策)
pub const S8_ARM_COUNT: usize = 3;

/// Abstain 保守护栏阈值 — uncertainty 超过此值时无条件 Abstain
///
/// WHY 0.7: 方案 §6.1 的 abstain_prob > 0.5 是决策网络输出的概率阈值;
/// 降级实现中 uncertainty 是外部输入的任务不确定性度量,0.7 提供
/// "明显不确定才拦截"的保守边界——过低(0.5)会使学习器在中等不确定
/// 场景失去探索机会,过高(0.9)则护栏名存实亡。
pub const ABSTAIN_UNCERTAINTY_THRESHOLD: f32 = 0.7;

/// S8 默认噪声惩罚强度 λ(downstream_achievement − λ × noise_ratio)
///
/// WHY λ=0.4(高于 S2 的 0.3): Mem-π 的核心风险是有害生成(幽灵记忆),
/// 更强的噪声惩罚引导学习器在低置信场景偏向 Abstain/Retrieve 而非 Generate。
pub const DEFAULT_NOISE_PENALTY_LAMBDA: f64 = 0.4;

/// S8 默认探索强度 α(与 S1/S2/S4 保持一致,Li et al. 2010 稳健默认值)
pub const DEFAULT_S8_ALPHA: f64 = 1.0;

// ============================================================
// Mem-π 决策枚举(3 臂)
// ============================================================

/// Mem-π 记忆决策 — S8 接缝的动作空间
///
/// 对应方案 §6.1 `MemPiDecision` 的降级形态:去掉决策附带的内容载荷
/// (`Generate(MemoryChunk)` / `Retrieve(String)`),只保留决策类型本身——
/// 内容生成/检索由消费方(mlc-engine)按决策类型自行执行,
/// 学习器只负责"做什么"不负责"怎么做"(职责分离,ADR-031 学习边界)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum MemPiDecision {
    /// 生成记忆 — 将当前上下文压缩为新记忆条目(方案 §6.1 Generate)
    ///
    /// 适用:预测未来访问概率高且不确定性低的场景
    Generate = 0,
    /// 检索记忆 — 从既有记忆召回(方案 §6.1 Retrieve)
    ///
    /// 适用:任务需要历史信息且既有记忆覆盖充分的场景
    Retrieve = 1,
    /// 不操作 — 不确定时既不生成也不检索(方案 §6.1 Abstain,避免有害生成)
    ///
    /// 适用:高不确定性场景(护栏强制)或学习器判断操作收益为负
    Abstain = 2,
}

impl MemPiDecision {
    /// 返回所有决策(按枚举值升序,与臂索引一致)
    pub const ALL: [Self; 3] = [Self::Generate, Self::Retrieve, Self::Abstain];

    /// 返回决策简称(用于日志/臂 ID/持久化)
    pub const fn short_name(self) -> &'static str {
        match self {
            Self::Generate => "generate",
            Self::Retrieve => "retrieve",
            Self::Abstain => "abstain",
        }
    }
}

impl std::fmt::Display for MemPiDecision {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.short_name())
    }
}

// ============================================================
// S8 上下文
// ============================================================

/// S8 上下文 — 任务阶段 / 不确定性 / 预测未来访问 / 内存压力
///
/// 编码为 7 维特征向量供 LinUCB 消费。所有数值字段归一化到 [0, 1],
/// 满足 LinUCB regret 上界假设 `||x||` 有界(最大范数 √5 ≈ 2.24,
/// α=1.0 下仍提供合理探索,与 S2 的范数分析同理)。
///
/// # 设计决策(WHY)
/// - **复用 S2 的 TaskPhase**: 任务阶段语义与 S2 完全一致(Initial/Stuck/LongRun),
///   独立定义会造成同义类型分裂
/// - **uncertainty 独立成维**: 既是 LinUCB 特征也是 Abstain 护栏的触发信号,
///   对应方案 §6.1 `MemPiState.uncertainty`
/// - **predicted_future_access**: 对应方案 §6.1 `MemPiState.predicted_future_access`,
///   Generate 决策的核心正信号(未来会访问才值得生成)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct S8Context {
    /// 任务阶段(one-hot 编码到 3 维,复用 S2 的 TaskPhase)
    pub task_phase: TaskPhase,
    /// 当前任务不确定性 ∈ [0, 1](> 0.7 触发 Abstain 护栏)
    pub uncertainty: f32,
    /// 预测未来访问概率 ∈ [0, 1]
    pub predicted_future_access: f32,
    /// 内存压力 ∈ [0, 1](used / budget)
    pub memory_pressure: f32,
}

impl S8Context {
    /// 创建 S8 上下文
    ///
    /// # 参数
    /// - `task_phase`: 任务阶段(决定 one-hot 编码位置)
    /// - `uncertainty`: 任务不确定性 ∈ [0, 1]
    /// - `predicted_future_access`: 预测未来访问概率 ∈ [0, 1]
    /// - `memory_pressure`: 内存压力 ∈ [0, 1]
    ///
    /// # 错误
    /// - `InvalidReward`: 任一数值字段不在 [0, 1] 或非有限
    pub fn new(
        task_phase: TaskPhase,
        uncertainty: f32,
        predicted_future_access: f32,
        memory_pressure: f32,
    ) -> Result<Self> {
        // 三个数值字段统一校验(系统边界校验,越界即拒绝)
        for value in [uncertainty, predicted_future_access, memory_pressure] {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return Err(LearnerError::InvalidReward {
                    reward: value as f64,
                });
            }
        }
        Ok(Self {
            task_phase,
            uncertainty,
            predicted_future_access,
            memory_pressure,
        })
    }

    /// 是否触发 Abstain 保守护栏(uncertainty > 0.7)
    ///
    /// WHY 公开: 上层编排器与影子模式报告需要区分"护栏 Abstain"与
    /// "学习 Abstain"(前者是安全行为,后者是策略偏好)。
    pub fn triggers_abstain_guard(&self) -> bool {
        self.uncertainty > ABSTAIN_UNCERTAINTY_THRESHOLD
    }

    /// 编码为 7 维特征向量,供 LinUCB 消费
    ///
    /// 向量布局:
    /// - `[0..3]`: task_phase one-hot 编码
    /// - `[3]`: uncertainty
    /// - `[4]`: predicted_future_access
    /// - `[5]`: memory_pressure
    /// - `[6]`: bias 常量 1.0
    pub fn features(&self) -> [f32; S8_CONTEXT_DIM] {
        let mut features = [0.0f32; S8_CONTEXT_DIM];
        features[self.task_phase.one_hot_index()] = 1.0;
        features[3] = self.uncertainty;
        features[4] = self.predicted_future_access;
        features[5] = self.memory_pressure;
        features[6] = 1.0;
        features
    }

    /// 转换为 `SeamContext`(LinUCB 输入)
    pub fn to_seam_context(&self) -> Result<SeamContext> {
        SeamContext::new(self.features().to_vec())
    }
}

impl std::fmt::Display for S8Context {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "S8Context({}, uncertainty={:.2}, future={:.2}, mem={:.2})",
            self.task_phase.short_name(),
            self.uncertainty,
            self.predicted_future_access,
            self.memory_pressure
        )
    }
}

// ============================================================
// S8 奖励
// ============================================================

/// S8 奖励参数 — 控制噪声惩罚强度
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct S8RewardParams {
    /// 噪声惩罚强度 λ(downstream_achievement − λ × noise_ratio)
    pub noise_penalty_lambda: f64,
}

impl Default for S8RewardParams {
    fn default() -> Self {
        Self {
            noise_penalty_lambda: DEFAULT_NOISE_PENALTY_LAMBDA,
        }
    }
}

/// S8 奖励 — 下游任务达成率 − 噪声占比惩罚
///
/// 公式: `reward = downstream_achievement − λ × noise_ratio`
///
/// # 边界行为
/// - `noise_ratio = 0.0`: 无惩罚(reward = downstream_achievement)
/// - `downstream_achievement = 0.0 且 noise_ratio = 1.0`: reward = −λ(最强惩罚)
/// - Abstain 决策的典型观测: noise_ratio = 0(未引入任何条目),
///   reward 完全由下游达成率决定——这使 Abstain 在"不操作也能达成"的
///   场景获得正反馈,符合方案 §6.1 的保守生成哲学
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct S8Reward {
    /// 下游任务达成率 ∈ [0, 1]
    pub downstream_achievement: f64,
    /// 噪声占比 ∈ [0, 1](生成/召回条目中无关条目比例;Abstain 时为 0)
    pub noise_ratio: f64,
    /// 奖励参数
    pub params: S8RewardParams,
}

impl S8Reward {
    /// 创建 S8 奖励(使用默认参数 λ=0.4)
    ///
    /// # 错误
    /// - `InvalidReward`: downstream_achievement 或 noise_ratio 不在 [0, 1] 或非有限
    pub fn new(downstream_achievement: f64, noise_ratio: f64) -> Result<Self> {
        Self::with_params(
            downstream_achievement,
            noise_ratio,
            S8RewardParams::default(),
        )
    }

    /// 创建 S8 奖励(自定义参数)
    ///
    /// # 错误
    /// - `InvalidReward`: downstream_achievement 或 noise_ratio 不在 [0, 1] 或非有限
    pub fn with_params(
        downstream_achievement: f64,
        noise_ratio: f64,
        params: S8RewardParams,
    ) -> Result<Self> {
        for value in [downstream_achievement, noise_ratio] {
            if !value.is_finite() || !(0.0..=1.0).contains(&value) {
                return Err(LearnerError::InvalidReward { reward: value });
            }
        }
        Ok(Self {
            downstream_achievement,
            noise_ratio,
            params,
        })
    }

    /// 计算最终奖励值
    ///
    /// 公式: `reward = downstream_achievement − λ × noise_ratio`
    pub fn reward(&self) -> f64 {
        self.downstream_achievement - self.params.noise_penalty_lambda * self.noise_ratio
    }
}

// ============================================================
// S8 臂集(3 臂对应 MemPiDecision::ALL)
// ============================================================

/// 构建 S8 接缝的臂集(3 臂对应 3 种记忆决策)
///
/// 臂 ID 用决策简称字符串(与 `MemPiDecision::short_name()` 一致),
/// 便于跨版本持久化与影子模式报告对齐。
pub fn s8_arm_set() -> DiscreteArmSet {
    DiscreteArmSet::new(vec![
        ArmId::new(MemPiDecision::Generate.short_name()),
        ArmId::new(MemPiDecision::Retrieve.short_name()),
        ArmId::new(MemPiDecision::Abstain.short_name()),
    ])
}

/// ArmIndex → MemPiDecision 映射(臂顺序与 `MemPiDecision::ALL` 一致)
pub const fn arm_index_to_decision(idx: usize) -> MemPiDecision {
    match idx {
        0 => MemPiDecision::Generate,
        1 => MemPiDecision::Retrieve,
        _ => MemPiDecision::Abstain,
    }
}

/// MemPiDecision → ArmIndex 映射
pub const fn decision_to_arm_index(decision: MemPiDecision) -> usize {
    decision as usize
}

// ============================================================
// S8 学习器
// ============================================================

/// S8 学习器 — 封装 LinUCB + Abstain 保守护栏
///
/// # 设计
///
/// `S8Learner` 是 `LinUCB` 的薄封装,提供 S8 接缝特定的:
/// - 上下文编码(`S8Context` → `SeamContext`)
/// - 臂映射(`ArmIndex` → `MemPiDecision`)
/// - 奖励计算(`S8Reward` → `f64`)
/// - **Abstain 保守护栏**(uncertainty > 0.7 时绕过 LinUCB 强制 Abstain)
///
/// # 影子模式约束(ADR-043,当前状态)
///
/// S8 处于影子期:学习器只产出决策供对比记录,**不向 mlc-engine 注入策略**。
/// 解冻(CapabilityToken Authorized + 影子 2 周 + 评审)后,注入通路
/// 复用 `mlc-engine::memory_strategy_learner` 持有器模式(S2 先例),届时需新增
/// 对应 Policy 契约类型上提 L0——影子期刻意不预置该类型(YAGNI,closure §10.5 同理)。
///
/// # 线程安全
///
/// 内部 `LinUCB` 非 `Sync`(ndarray 数组无原子操作),多线程共享需
/// `Arc<Mutex<S8Learner>>`;典型用法是单线程后台任务 + mpsc 通信(与 S2 一致)。
#[derive(Debug, Clone)]
pub struct S8Learner {
    /// 内部 LinUCB 实例
    linucb: LinUCB,
    /// 最近一次选择的臂索引(用于诊断与影子报告)
    last_arm_idx: usize,
    /// 已观察到的总步数
    total_steps: u64,
    /// Abstain 护栏累计触发次数(影子模式报告的安全行为指标)
    guard_triggered_count: u64,
}

impl S8Learner {
    /// 创建 S8 学习器
    ///
    /// # 参数
    /// - `alpha`: LinUCB 探索强度(必须 > 0 且有限)
    ///
    /// # 错误
    /// - `InvalidAlpha`: alpha ≤ 0 或非有限
    pub fn new(alpha: f64) -> Result<Self> {
        let arm_set = s8_arm_set();
        let linucb = LinUCB::new(S8_CONTEXT_DIM, &arm_set, alpha)?;
        Ok(Self {
            linucb,
            last_arm_idx: decision_to_arm_index(MemPiDecision::Abstain),
            total_steps: 0,
            guard_triggered_count: 0,
        })
    }

    /// 创建 S8 学习器(使用默认 α=1.0)
    pub fn with_default_alpha() -> Result<Self> {
        Self::new(DEFAULT_S8_ALPHA)
    }

    /// 选择记忆决策 — Abstain 护栏优先,其余走 LinUCB
    ///
    /// # 算法
    /// 1. **护栏检查**: `uncertainty > 0.7` → 无条件返回 `Abstain`(不经 LinUCB,
    ///    不可被学习绕过;护栏触发计数 +1)
    /// 2. 将 `S8Context` 编码为 7 维特征向量 → `SeamContext`
    /// 3. `LinUCB::select_arm` 选择 UCB 最大的臂 → 映射回 `MemPiDecision`
    pub fn select(&mut self, context: &S8Context) -> Result<MemPiDecision> {
        if context.triggers_abstain_guard() {
            self.guard_triggered_count += 1;
            self.last_arm_idx = decision_to_arm_index(MemPiDecision::Abstain);
            return Ok(MemPiDecision::Abstain);
        }
        let seam_ctx = context.to_seam_context()?;
        let arm_idx = self.linucb.select_arm(&seam_ctx)?;
        self.last_arm_idx = arm_idx.as_usize();
        Ok(arm_index_to_decision(self.last_arm_idx))
    }

    /// 观察奖励并更新模型
    ///
    /// # 护栏语义
    /// 护栏强制的 Abstain 决策**不应**回传更新(那不是学习器的选择,
    /// 回传会把护栏行为误学为策略偏好)。调用方约定:仅对
    /// `!context.triggers_abstain_guard()` 的决策调用本方法;
    /// 若误传护栏上下文,本方法直接跳过更新并返回 Ok(防御边界仅此一处,
    /// 因为这是学习正确性约束而非输入合法性问题)。
    pub fn update(
        &mut self,
        context: &S8Context,
        decision: MemPiDecision,
        reward: &S8Reward,
    ) -> Result<()> {
        if context.triggers_abstain_guard() {
            // 护栏上下文不参与学习:静默跳过(见方法文档"护栏语义")
            return Ok(());
        }
        let seam_ctx = context.to_seam_context()?;
        let arm_idx = ArmIndex::from(decision_to_arm_index(decision));
        self.linucb.update(arm_idx, &seam_ctx, reward.reward())?;
        self.total_steps += 1;
        Ok(())
    }

    /// 返回最近一次选择的决策(影子模式报告用)
    pub fn last_decision(&self) -> MemPiDecision {
        arm_index_to_decision(self.last_arm_idx)
    }

    /// 返回已观察到的总步数(不含护栏触发)
    pub fn total_steps(&self) -> u64 {
        self.total_steps
    }

    /// 返回 Abstain 护栏累计触发次数(影子模式安全行为指标)
    pub fn guard_triggered_count(&self) -> u64 {
        self.guard_triggered_count
    }

    /// 返回内部 LinUCB 引用(用于诊断与持久化)
    pub fn linucb(&self) -> &LinUCB {
        &self.linucb
    }
}

// ============================================================
// Send + Sync 静态断言
// ============================================================

/// S8Learner 必须实现 Send + Sync(异步跨线程共享需求,与 S2 同款断言)
fn _assert_s8_learner_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<S8Learner>();
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    // ============================================================
    // MemPiDecision 测试
    // ============================================================

    #[test]
    fn test_decision_short_name() {
        assert_eq!(MemPiDecision::Generate.short_name(), "generate");
        assert_eq!(MemPiDecision::Retrieve.short_name(), "retrieve");
        assert_eq!(MemPiDecision::Abstain.short_name(), "abstain");
    }

    #[test]
    fn test_decision_all_returns_three() {
        assert_eq!(MemPiDecision::ALL.len(), 3);
        assert!(MemPiDecision::ALL.contains(&MemPiDecision::Abstain));
    }

    #[test]
    fn test_decision_arm_index_roundtrip() {
        for decision in MemPiDecision::ALL {
            let idx = decision_to_arm_index(decision);
            assert_eq!(arm_index_to_decision(idx), decision);
        }
    }

    #[test]
    fn test_decision_display() {
        assert_eq!(format!("{}", MemPiDecision::Abstain), "abstain");
    }

    // ============================================================
    // S8Context 测试
    // ============================================================

    #[test]
    fn test_context_new_valid() {
        let ctx = S8Context::new(TaskPhase::Initial, 0.5, 0.5, 0.5).unwrap();
        assert_eq!(ctx.task_phase, TaskPhase::Initial);
        assert!(!ctx.triggers_abstain_guard());
    }

    #[test]
    fn test_context_rejects_out_of_range() {
        assert!(S8Context::new(TaskPhase::Initial, 1.5, 0.5, 0.5).is_err());
        assert!(S8Context::new(TaskPhase::Initial, 0.5, -0.1, 0.5).is_err());
        assert!(S8Context::new(TaskPhase::Initial, 0.5, 0.5, f32::NAN).is_err());
    }

    #[test]
    fn test_context_features_layout() {
        let ctx = S8Context::new(TaskPhase::Stuck, 0.3, 0.7, 0.2).unwrap();
        let f = ctx.features();
        assert_eq!(f.len(), S8_CONTEXT_DIM);
        // one-hot: Stuck 在位置 1
        assert_eq!(f[0], 0.0);
        assert_eq!(f[1], 1.0);
        assert_eq!(f[2], 0.0);
        // 数值特征
        assert!((f[3] - 0.3).abs() < 1e-6);
        assert!((f[4] - 0.7).abs() < 1e-6);
        assert!((f[5] - 0.2).abs() < 1e-6);
        // bias
        assert_eq!(f[6], 1.0);
    }

    #[test]
    fn test_context_guard_boundary() {
        // 阈值边界:恰好 0.7 不触发(严格大于),0.71 触发
        let at = S8Context::new(TaskPhase::Initial, 0.7, 0.5, 0.5).unwrap();
        assert!(!at.triggers_abstain_guard());
        let above = S8Context::new(TaskPhase::Initial, 0.71, 0.5, 0.5).unwrap();
        assert!(above.triggers_abstain_guard());
    }

    // ============================================================
    // S8Reward 测试
    // ============================================================

    #[test]
    fn test_reward_no_noise_no_penalty() {
        let r = S8Reward::new(1.0, 0.0).unwrap();
        assert!((r.reward() - 1.0).abs() < 1e-9);
    }

    #[test]
    fn test_reward_full_noise_penalty() {
        // 达成 1.0,噪声 1.0 → 1.0 − 0.4 = 0.6
        let r = S8Reward::new(1.0, 1.0).unwrap();
        assert!((r.reward() - 0.6).abs() < 1e-9);
    }

    #[test]
    fn test_reward_worst_case() {
        // 达成 0,噪声 1.0 → −λ = −0.4(最强惩罚)
        let r = S8Reward::new(0.0, 1.0).unwrap();
        assert!((r.reward() + 0.4).abs() < 1e-9);
    }

    #[test]
    fn test_reward_rejects_invalid() {
        assert!(S8Reward::new(1.5, 0.0).is_err());
        assert!(S8Reward::new(0.5, f64::INFINITY).is_err());
    }

    #[test]
    fn test_reward_custom_params() {
        let params = S8RewardParams {
            noise_penalty_lambda: 0.8,
        };
        let r = S8Reward::with_params(1.0, 0.5, params).unwrap();
        assert!((r.reward() - 0.6).abs() < 1e-9);
    }

    // ============================================================
    // S8Learner 测试
    // ============================================================

    #[test]
    fn test_learner_new() {
        let learner = S8Learner::with_default_alpha().unwrap();
        assert_eq!(learner.total_steps(), 0);
        assert_eq!(learner.guard_triggered_count(), 0);
    }

    #[test]
    fn test_learner_select_returns_valid_decision() {
        let mut learner = S8Learner::with_default_alpha().unwrap();
        let ctx = S8Context::new(TaskPhase::LongRun, 0.3, 0.8, 0.5).unwrap();
        let decision = learner.select(&ctx).unwrap();
        assert!(MemPiDecision::ALL.contains(&decision));
    }

    #[test]
    fn test_learner_select_update_cycle() {
        let mut learner = S8Learner::with_default_alpha().unwrap();
        let ctx = S8Context::new(TaskPhase::Initial, 0.2, 0.9, 0.3).unwrap();
        let decision = learner.select(&ctx).unwrap();
        let reward = S8Reward::new(0.9, 0.1).unwrap();
        learner.update(&ctx, decision, &reward).unwrap();
        assert_eq!(learner.total_steps(), 1);
        assert_eq!(learner.last_decision(), decision);
    }

    #[test]
    fn test_abstain_guard_forces_abstain() {
        let mut learner = S8Learner::with_default_alpha().unwrap();
        let risky = S8Context::new(TaskPhase::Stuck, 0.9, 0.9, 0.5).unwrap();
        // 无论 LinUCB 状态如何,高不确定性必须 Abstain
        assert_eq!(learner.select(&risky).unwrap(), MemPiDecision::Abstain);
        assert_eq!(learner.guard_triggered_count(), 1);
    }

    #[test]
    fn test_guard_context_skips_update() {
        let mut learner = S8Learner::with_default_alpha().unwrap();
        let risky = S8Context::new(TaskPhase::Stuck, 0.9, 0.5, 0.5).unwrap();
        let reward = S8Reward::new(1.0, 0.0).unwrap();
        // 护栏上下文的 update 静默跳过,不污染模型
        learner
            .update(&risky, MemPiDecision::Abstain, &reward)
            .unwrap();
        assert_eq!(learner.total_steps(), 0);
    }

    #[test]
    fn test_guard_not_learnable_after_training() {
        // 核心不变量:即使训练强烈偏向 Generate,护栏仍不可绕过
        let mut learner = S8Learner::with_default_alpha().unwrap();
        let safe = S8Context::new(TaskPhase::Initial, 0.1, 0.9, 0.3).unwrap();
        // 对 Generate 灌入 50 次满分奖励,制造强 Generate 偏好
        for _ in 0..50 {
            learner
                .update(
                    &safe,
                    MemPiDecision::Generate,
                    &S8Reward::new(1.0, 0.0).unwrap(),
                )
                .unwrap();
        }
        // 高不确定性上下文仍然强制 Abstain
        let risky = S8Context::new(TaskPhase::Initial, 0.95, 0.9, 0.3).unwrap();
        assert_eq!(learner.select(&risky).unwrap(), MemPiDecision::Abstain);
    }

    #[test]
    fn test_learner_converges_to_rewarded_arm() {
        // 收敛性冒烟:持续奖励 Retrieve,学习器最终偏向 Retrieve
        let mut learner = S8Learner::with_default_alpha().unwrap();
        let ctx = S8Context::new(TaskPhase::LongRun, 0.3, 0.5, 0.5).unwrap();
        for _ in 0..100 {
            let decision = learner.select(&ctx).unwrap();
            let reward = if decision == MemPiDecision::Retrieve {
                S8Reward::new(1.0, 0.0).unwrap()
            } else {
                S8Reward::new(0.1, 0.5).unwrap()
            };
            learner.update(&ctx, decision, &reward).unwrap();
        }
        // 训练后再选 20 次,Retrieve 应占多数(UCB 收敛)
        let mut retrieve_count = 0;
        for _ in 0..20 {
            if learner.select(&ctx).unwrap() == MemPiDecision::Retrieve {
                retrieve_count += 1;
            }
        }
        assert!(
            retrieve_count >= 15,
            "expected Retrieve dominance, got {retrieve_count}/20"
        );
    }

    #[test]
    fn test_arm_set_has_three_arms() {
        assert_eq!(s8_arm_set().len(), S8_ARM_COUNT);
    }

    // ============================================================
    // proptest 属性测试(Abstain 护栏不变量)
    // ============================================================

    proptest! {
        /// 不变量 1: uncertainty > 0.7 ⇒ select() 恒为 Abstain(护栏不可绕过)
        #[test]
        fn prop_abstain_guard_invariant(
            uncertainty in 0.71f32..=1.0,
            future in 0.0f32..=1.0,
            mem in 0.0f32..=1.0,
            phase_idx in 0usize..3,
        ) {
            let mut learner = S8Learner::with_default_alpha().unwrap();
            let ctx = S8Context::new(
                TaskPhase::ALL[phase_idx],
                uncertainty,
                future,
                mem,
            ).unwrap();
            prop_assert_eq!(learner.select(&ctx).unwrap(), MemPiDecision::Abstain);
        }

        /// 不变量 2: 合法输入下 reward() 有界于 [−λ, 1](数值稳定性)
        #[test]
        fn prop_reward_bounded(
            achievement in 0.0f64..=1.0,
            noise in 0.0f64..=1.0,
        ) {
            let r = S8Reward::new(achievement, noise).unwrap();
            let value = r.reward();
            prop_assert!(value >= -DEFAULT_NOISE_PENALTY_LAMBDA - 1e-9);
            prop_assert!(value <= 1.0 + 1e-9);
        }
    }
}
