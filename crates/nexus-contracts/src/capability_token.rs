//! 能力场令牌契约 — 学习策略灰度授权载体（P4-W14.5 C4 合规）
//!
//! 对应架构层: **L0 Contracts**（nexus-core 之下，跨层共享类型）
//! 对应 ADR: **ADR-037**（能力场灰度 C4 合规，提议中）
//! 对应设计源: `NEXUS-OMEGA_v5.0_系统性完整设计文档.md` §C4 + §7.4
//! 对应任务: **P4-W14.5.1**（CapabilityToken 类型设计）
//!
//! # 核心职责
//!
//! 承载学习策略（`*Policy::Learned`）的灰度授权载体，使编排器
//! （chimera-cli / quest-engine）能在注入 Learned 策略前查询授权等级，
//! 实现 C4 合规要求的"运行时灰度走能力场，而非散落运行时布尔旗"。
//!
//! # C4 合规三层 fallback
//!
//! 1. **默认值层**: `CapabilityToken::new()` 初始化为
//!    `status = Provisional` + `authorized_level = INITIAL_LEVEL = 0.2`
//!    （低于 `ACTIVATION_THRESHOLD = 0.3`，灰度未激活）
//! 2. **异常回退层**: `trigger_asa_intervention()` 触发后进入 `Cooldown`，
//!    `allows_learned_policy()` 返回 false，编排器本地 fallback 到 Static
//! 3. **熔断入口层**: 连续 ASA 触发达 `ASA_FREEZE_THRESHOLD = 3` 自动 `Frozen`，
//!    `fallback_to_static()` 不受 token 约束（C4 合规第三层不可阻塞）
//!
//! # 渐进授权算法（EWMA + 自适应步长）
//!
//! - **EWMA α=0.1**: 指数加权移动平均更新成功率，对非平稳奖励分布适应性强
//!   （学术支撑: Token Budgets, Khan 2026）
//! - **自适应步长**: `step = PROMOTION_STEP_BASE × (1.0 - current_level)`
//!   - 低 level（0.2）时步长 = 0.08，快速激活
//!   - 高 level（0.8）时步长 = 0.02，谨慎逼近
//! - **激活阈值**: `authorized_level >= ACTIVATION_THRESHOLD (0.3)` 才允许 Learned
//!
//! # AsaIntervention 安全闭环
//!
//! ```text
//! AsaIntervention 事件 → trigger_asa_intervention(now)
//!   ├─ 1. authorized_level -= DECAY_ON_ASA (0.2)
//!   ├─ 2. status = Cooldown, cooldown_until = now + 60s
//!   ├─ 3. consecutive_asa_count += 1
//!   └─ 4. 若 consecutive_asa_count >= 3 → status = Frozen（自动冻结）
//! ```
//!
//! 冷却期结束后（`now >= cooldown_until`），状态恢复为 `Authorized`（若 level >= 阈值）
//! 或 `Provisional`（若 level < 阈值）。`Frozen` 状态只能通过手动 `unfreeze()` 恢复。
//!
//! # 设计决策（WHY）
//!
//! - **L0 定义**: CapabilityToken 需被 L4 decay-engine（Registry 嵌入）+ L6 omega-learner
//!   （记录 outcome）+ L9 编排器（查询激活）共同消费，定义在 L0 避免跨层依赖
//! - **i64 时间戳**: 与 `temporal::TemporalMeta` 一致，UTC 秒时间戳，调用方传入 `now`
//!   （L0 禁止依赖 chrono，保持零 crate 依赖）
//! - **SeamId 独立定义**: 与 `omega_learner::SeamId` 语义对齐，未来 P4-W14.6 统一上提
//!
//! # 示例
//!
//! ## 基础灰度授权流程
//!
//! ```
//! use nexus_contracts::{CapabilityToken, SeamId};
//!
//! // 1. 新策略注册（初始低能力）
//! let mut token = CapabilityToken::new("s6-decay-v1", SeamId::S6Decay);
//! assert!(!token.allows_learned_policy(0));  // 未达激活阈值
//!
//! // 2. 多次成功 outcome 推动渐进授权
//! for _ in 0..10 {
//!     token.record_outcome(true);
//!     token.maybe_promote();
//! }
//! assert!(token.authorized_level() > 0.2);
//!
//! // 3. 达到激活阈值后允许 Learned 策略
//! while !token.allows_learned_policy(0) {
//!     token.record_outcome(true);
//!     token.maybe_promote();
//! }
//! assert!(token.allows_learned_policy(0));
//! ```

use serde::{Deserialize, Serialize};

// ============================================================
// SeamId 枚举（六接缝标识）
// ============================================================

/// 八接缝标识 — 学习策略灰度授权的目标接缝
///
/// WHY 独立定义（与 `omega_learner::SeamId` 语义对齐）:
/// - L0 nexus-contracts 禁止依赖 L6 omega-learner（依赖铁律向上禁止）
/// - 当前任务 P4-W14.5 聚焦 CapabilityToken，SeamId 统一上提作为 P4-W14.6 后续任务
/// - 物理独立但语义对齐：8 变体一一对应，未来上提时可直接替换
///
/// WHY 用枚举而非字符串:
/// - 编译期穷尽性检查（match 必须覆盖所有变体）
/// - 避免 typo 错误
/// - IDE 自动补全
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum SeamId {
    /// S1: DDR/HCW 密度档位（hcw-window selector）
    S1Density = 1,
    /// S2: 记忆策略选择（mlc-engine recall）
    S2Memory = 2,
    /// S3: SCC 预取策略（scc-cache prefetch）
    S3Prefetch = 3,
    /// S4: selector 权重系数（hcw-window selector w1/w2/w3）
    S4Selector = 4,
    /// S5: Parliament 激活策略（parliament Fast Path）
    S5Parliament = 5,
    /// S6: 衰减参数档位（decay-engine DecayProfile）
    S6Decay = 6,
    /// S7: 召回配额档位（omega-learner r1_recall_quota，P4-W16.2.2）
    ///
    /// R1 离线 RL 接缝：CQL/IQL 算法学习召回配额 k ∈ {5,10,20,50,100}，
    /// 在 FormalVerifier 落地前需满足影子模式 2 周前置（ADR-043）。
    /// 与 S1-S6 在线 bandit 不同，S7 使用离线 RL（CQL/IQL），
    /// 从 `ReplayPool<RecallQuotaTransition>` 采样训练。
    S7RecallQuota = 7,

    /// S8: Mem-π 记忆决策策略（omega-learner s8_mem_pi，polish-v2.7 closure Stage B-6）
    ///
    /// Mem-π 接缝：LinUCB 学习记忆操作决策（Generate/Retrieve/Abstain 三档），
    /// 对应方案文档 §6.1 Mem-π 两阶段决策的规则化降级（ADR-049 决策 1）。
    /// 高不确定性时强制 Abstain（保守护栏，避免有害生成）；
    /// 解冻前置 = CapabilityToken 灰度 + ADR-043 影子模式 2 周。
    S8MemPi = 8,
}

impl SeamId {
    /// 返回接缝编号（1-8）
    pub const fn number(self) -> u8 {
        self as u8
    }

    /// 返回接缝简称（用于日志/调试）
    pub const fn short_name(self) -> &'static str {
        match self {
            Self::S1Density => "S1-density",
            Self::S2Memory => "S2-memory",
            Self::S3Prefetch => "S3-prefetch",
            Self::S4Selector => "S4-selector",
            Self::S5Parliament => "S5-parliament",
            Self::S6Decay => "S6-decay",
            Self::S7RecallQuota => "S7-recall-quota",
            Self::S8MemPi => "S8-mem-pi",
        }
    }

    /// 返回所有八接缝（用于遍历初始化）
    ///
    /// WHY 8 而非 7: S8 为 polish-v2.7 closure Stage B-6 新增 Mem-π 接缝
    pub const fn all() -> [SeamId; 8] {
        [
            Self::S1Density,
            Self::S2Memory,
            Self::S3Prefetch,
            Self::S4Selector,
            Self::S5Parliament,
            Self::S6Decay,
            Self::S7RecallQuota,
            Self::S8MemPi,
        ]
    }
}

impl std::fmt::Display for SeamId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.short_name())
    }
}

// ============================================================
// CapabilityTokenStatus 枚举
// ============================================================

/// 能力令牌状态 — 灰度授权生命周期四态
///
/// WHY 四态而非二态:
/// - `Provisional` 与 `Authorized` 区分"未达阈值"与"已达阈值"，便于编排器决策
/// - `Cooldown` 与 `Frozen` 区分"临时冷却"与"永久冻结"，对应不同恢复路径
///
/// # 状态转换图
///
/// ```text
///   ┌─────────────┐  maybe_promote()  ┌──────────────┐
///   │ Provisional │ ────────────────▶ │  Authorized  │
///   │  level<0.3  │ ◀──────────────── │  level≥0.3   │
///   └─────────────┘   decay(asg=0.2)  └──────────────┘
///         │                                  │
///         │           trigger_asa           │
///         │       ┌──────────────┐          │
///         └──────▶│   Cooldown   │◀─────────┘
///                 │ 60s 冷却期   │
///                 └──────────────┘
///                         │
///                         │ consecutive_asa_count >= 3
///                         ▼
///                 ┌──────────────┐
///                 │    Frozen    │
///                 │ 永久冻结      │
///                 └──────────────┘
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CapabilityTokenStatus {
    /// 临时状态 — 已注册但未达激活阈值
    ///
    /// `authorized_level < ACTIVATION_THRESHOLD (0.3)`
    /// `allows_learned_policy()` 返回 false
    Provisional,

    /// 已授权 — 达到激活阈值，允许 Learned 策略
    ///
    /// `authorized_level >= ACTIVATION_THRESHOLD (0.3)`
    /// `allows_learned_policy()` 返回 true
    Authorized,

    /// 冷却期 — AsaIntervention 触发后临时禁用
    ///
    /// `now < cooldown_until`
    /// `allows_learned_policy()` 返回 false
    /// 冷却期结束后恢复为 Provisional 或 Authorized
    Cooldown,

    /// 已冻结 — 连续 ASA 触发达阈值或手动冻结
    ///
    /// `allows_learned_policy()` 返回 false
    /// 只能通过 `unfreeze()` 手动恢复
    Frozen,
}

impl CapabilityTokenStatus {
    /// 返回状态名称（用于日志/事件 payload）
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Provisional => "Provisional",
            Self::Authorized => "Authorized",
            Self::Cooldown => "Cooldown",
            Self::Frozen => "Frozen",
        }
    }

    /// 是否允许 Learned 策略（不依赖 token level，仅状态判断）
    ///
    /// WHY 提供: 便于编排器快速过滤，无需读取 level 字段
    pub fn allows_learned(self) -> bool {
        matches!(self, Self::Authorized)
    }
}

impl std::fmt::Display for CapabilityTokenStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ============================================================
// CapabilityToken 结构体
// ============================================================

/// 能力场令牌 — 学习策略灰度授权载体
///
/// 承载单个接缝（SeamId）的灰度授权状态，由 `CapabilityTokenRegistry`
/// （L4 decay-engine）集中管理。编排器在注入 `*Policy::Learned` 前
/// 查询 `allows_learned_policy(now)`，未授权则本地 fallback 到 Static。
///
/// # 字段说明
///
/// - `token_id`: 唯一标识（如 "s6-decay-v1"）
/// - `seam`: 所属接缝（六接缝之一）
/// - `authorized_level`: 授权等级 [0.0, 1.0]，连续流体（非离散 0/1）
/// - `status`: 生命周期状态（Provisional/Authorized/Cooldown/Frozen）
/// - `bound_policy_version`: 绑定的 Learned 策略版本号
/// - `success_ewma`: EWMA 成功率（α=0.1）
/// - `sample_count`: 累计样本数（用于评估统计显著性）
/// - `cooldown_until`: 冷却期结束时间（UTC 秒，None 表示未在冷却）
/// - `consecutive_asa_count`: 连续 ASA 触发次数（达阈值自动冻结）
///
/// # 设计决策（WHY）
///
/// - **i64 时间戳**: 与 `temporal::TemporalMeta` 一致，UTC 秒，调用方传入 `now`
/// - **f32 而非 f64**: 与 `CapabilityLevel` / `DensityTier` 等既有类型一致，避免精度膨胀
///   （§4.4 反模式 6: f32 禁止隐式转 f64 比较）
/// - **bound_policy_version**: 用于 A/B 测试与回滚（与 `*Policy::Learned(version)` 对齐）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CapabilityToken {
    /// 令牌唯一标识（如 "s6-decay-v1"）
    pub token_id: String,

    /// 所属接缝
    pub seam: SeamId,

    /// 授权等级 [0.0, 1.0]
    pub authorized_level: f32,

    /// 生命周期状态
    pub status: CapabilityTokenStatus,

    /// 绑定的 Learned 策略版本号（None 表示未绑定）
    pub bound_policy_version: Option<u64>,

    /// EWMA 成功率 [0.0, 1.0]
    pub success_ewma: f32,

    /// 累计样本数
    pub sample_count: u64,

    /// 冷却期结束时间（UTC 秒，None 表示未在冷却）
    pub cooldown_until: Option<i64>,

    /// 连续 ASA 触发次数
    pub consecutive_asa_count: u32,
}

// ============================================================
// 常量定义
// ============================================================

impl CapabilityToken {
    /// 初始授权等级 — 新策略起步低于激活阈值
    ///
    /// WHY 0.2 而非 0.0:
    /// - 0.0 会导致 `maybe_promote` 步长 = 0.1（最大），过度激进
    /// - 0.2 给出步长 = 0.08，4 次成功 outcome 即可达 0.3 阈值
    /// - 与 `DecayProfile::Lenient` 的 `freeze_threshold = 0.02` 区分语义
    pub const INITIAL_LEVEL: f32 = 0.2;

    /// 激活阈值 — 达到此值才允许 Learned 策略
    ///
    /// WHY 0.3:
    /// - 高于 INITIAL_LEVEL (0.2)，需要至少 2 次成功 outcome 才能激活
    /// - 低于 0.5，避免激活阈值过高导致灰度推进过慢
    /// - 与 `DecayProfile::Standard` 的 `freeze_threshold = 0.05` 区分语义
    pub const ACTIVATION_THRESHOLD: f32 = 0.3;

    /// 渐进授权步长基数 — 自适应步长 = base × (1 - current_level)
    ///
    /// WHY 0.1:
    /// - 步长 = 0.1 × (1 - 0.2) = 0.08（首次提升）
    /// - 步长 = 0.1 × (1 - 0.5) = 0.05（中段提升）
    /// - 步长 = 0.1 × (1 - 0.9) = 0.01（高段谨慎）
    /// - 与 EWMA α=0.1 对齐，避免步长过大破坏 EWMA 收敛性
    pub const PROMOTION_STEP_BASE: f32 = 0.1;

    /// EWMA 平滑系数 — α=0.1 适应非平稳奖励分布
    ///
    /// WHY 0.1（学术支撑: Token Budgets, Khan 2026）:
    /// - α=0.1 对历史样本加权 90%，对当前样本加权 10%
    /// - 比 sliding window 更适应非平稳分布（任务阶段切换时奖励分布漂移）
    /// - 比 α=0.5 更稳定，避免单次失败导致 EWMA 崩塌
    pub const EWMA_ALPHA: f32 = 0.1;

    /// 冷却期时长（秒）— AsaIntervention 触发后临时禁用
    ///
    /// WHY 60 秒:
    /// - 足够长以让编排器观察策略回退效果
    /// - 足够短以避免长时间无法学习（影响 Ω-Evolve）
    pub const COOLDOWN_DURATION_SECS: i64 = 60;

    /// 连续 ASA 触发自动冻结阈值
    ///
    /// WHY 3 次:
    /// - 1 次 ASA 可能是偶发（噪声）
    /// - 2 次 ASA 表明策略有问题
    /// - 3 次 ASA 确认策略有系统性问题，自动冻结
    /// - 与 `chimera-mas` `StabilityGuard` 的 3 次熔断阈值对齐
    pub const ASA_FREEZE_THRESHOLD: u32 = 3;

    /// AsaIntervention 触发时的衰减量
    ///
    /// WHY 0.2:
    /// - 单次 ASA 衰减 0.2，3 次累计 0.6，足以将 level 从 1.0 降到 0.4
    /// - 与 `DecayEvent::ViolationPenalty` 的 `severity = 2.0` × `penalty = 0.1` = 0.2 对齐
    pub const DECAY_ON_ASA: f32 = 0.2;
}

// ============================================================
// CapabilityToken 方法
// ============================================================

impl CapabilityToken {
    /// 创建新的能力令牌（初始低能力 + Provisional 状态）
    ///
    /// # 参数
    /// - `token_id`: 唯一标识（如 "s6-decay-v1"）
    /// - `seam`: 所属接缝
    ///
    /// # 初始状态
    /// - `authorized_level = INITIAL_LEVEL (0.2)`
    /// - `status = Provisional`
    /// - `success_ewma = 0.5`（中性先验，避免初始偏置）
    /// - `sample_count = 0`
    /// - `bound_policy_version = None`
    /// - `cooldown_until = None`
    /// - `consecutive_asa_count = 0`
    ///
    /// # 示例
    ///
    /// ```
    /// use nexus_contracts::{CapabilityToken, SeamId};
    ///
    /// let token = CapabilityToken::new("s6-decay-v1", SeamId::S6Decay);
    /// assert_eq!(token.authorized_level(), 0.2);
    /// assert_eq!(token.status(), nexus_contracts::CapabilityTokenStatus::Provisional);
    /// assert!(!token.allows_learned_policy(0));
    /// ```
    pub fn new(token_id: impl Into<String>, seam: SeamId) -> Self {
        Self {
            token_id: token_id.into(),
            seam,
            authorized_level: Self::INITIAL_LEVEL,
            status: CapabilityTokenStatus::Provisional,
            bound_policy_version: None,
            success_ewma: 0.5, // 中性先验
            sample_count: 0,
            cooldown_until: None,
            consecutive_asa_count: 0,
        }
    }

    /// 返回授权等级
    pub fn authorized_level(&self) -> f32 {
        self.authorized_level
    }

    /// 返回生命周期状态
    pub fn status(&self) -> CapabilityTokenStatus {
        self.status
    }

    /// 返回所属接缝
    pub fn seam(&self) -> SeamId {
        self.seam
    }

    /// 返回令牌 ID
    pub fn token_id(&self) -> &str {
        &self.token_id
    }

    /// 返回绑定策略版本号
    pub fn bound_policy_version(&self) -> Option<u64> {
        self.bound_policy_version
    }

    /// 返回 EWMA 成功率
    pub fn success_ewma(&self) -> f32 {
        self.success_ewma
    }

    /// 返回累计样本数
    pub fn sample_count(&self) -> u64 {
        self.sample_count
    }

    /// 返回连续 ASA 触发次数
    pub fn consecutive_asa_count(&self) -> u32 {
        self.consecutive_asa_count
    }

    /// 绑定 Learned 策略版本号
    ///
    /// WHY 提供: 编排器在注入 Learned 策略时调用，便于回溯
    pub fn bind_policy_version(&mut self, version: u64) {
        self.bound_policy_version = Some(version);
    }

    /// 判断是否在冷却期
    ///
    /// # 参数
    /// - `now`: 当前 UTC 秒时间戳
    ///
    /// # 返回
    /// - `true`: status == Cooldown && now < cooldown_until
    /// - `false`: 其他情况
    pub fn is_in_cooldown(&self, now: i64) -> bool {
        match self.status {
            CapabilityTokenStatus::Cooldown => match self.cooldown_until {
                Some(until) => now < until,
                None => false, // 无 cooldown_until 表示已过冷却期
            },
            _ => false,
        }
    }

    /// 判断是否允许 Learned 策略（C4 合规核心查询）
    ///
    /// 编排器在调用 `holder.update_policy(Learned)` 前必须查询此方法。
    /// 返回 false 时，编排器应本地 fallback 到 Static（C4 合规第三层）。
    ///
    /// # 参数
    /// - `now`: 当前 UTC 秒时间戳
    ///
    /// # 返回
    /// - `true`: status == Authorized && authorized_level >= ACTIVATION_THRESHOLD
    /// - `false`: 其他情况（Provisional/Cooldown/Frozen 或 level 不足）
    ///
    /// # 示例
    ///
    /// ```
    /// use nexus_contracts::{CapabilityToken, SeamId};
    ///
    /// let mut token = CapabilityToken::new("s1-density-v1", SeamId::S1Density);
    /// // 初始状态：Provisional，不允许 Learned
    /// assert!(!token.allows_learned_policy(0));
    ///
    /// // 模拟多次成功 outcome 推动渐进授权
    /// for _ in 0..10 {
    ///     token.record_outcome(true);
    ///     token.maybe_promote();
    /// }
    /// // 达到激活阈值后允许 Learned
    /// assert!(token.allows_learned_policy(0));
    /// ```
    pub fn allows_learned_policy(&self, now: i64) -> bool {
        // Frozen 状态永远不允许（C4 合规第三层：熔断入口不可阻塞 fallback_to_static）
        if self.status == CapabilityTokenStatus::Frozen {
            return false;
        }

        // Cooldown 状态：检查是否仍在冷却期
        if self.status == CapabilityTokenStatus::Cooldown {
            // 仍在冷却期内：不允许 Learned
            if self.is_in_cooldown(now) {
                return false;
            }
            // 冷却期已结束但状态字段未显式恢复：根据 level 隐式判断
            //
            // WHY 隐式判断而非显式恢复:
            // - `allows_learned_policy` 是查询方法（`&self`），不能调用
            //   `maybe_recover_from_cooldown`（`&mut self`）修改状态字段
            // - 编排器应定期调用 `maybe_recover_from_cooldown` 显式同步状态字段
            // - 但查询时若冷却期已结束且 level 达标，应允许 Learned
            //   （避免不必要的 fallback 到 Static，影响 Ω-Evolve 学习效率）
            return self.authorized_level >= Self::ACTIVATION_THRESHOLD;
        }

        // Provisional / Authorized 状态：双重检查（状态 + level）
        self.status == CapabilityTokenStatus::Authorized
            && self.authorized_level >= Self::ACTIVATION_THRESHOLD
    }

    /// 记录一次策略执行结果（EWMA 更新）
    ///
    /// # 参数
    /// - `success`: true 表示成功（奖励 +1），false 表示失败（奖励 0）
    ///
    /// # EWMA 更新公式
    /// - `success_ewma = (1 - α) × success_ewma + α × reward`
    /// - `reward = 1.0 if success else 0.0`
    /// - `α = EWMA_ALPHA (0.1)`
    ///
    /// # 设计决策（WHY 不在此方法提升 level）
    ///
    /// - `record_outcome` 仅更新 EWMA 与样本数，不修改 level
    /// - level 提升由 `maybe_promote` 独立负责，便于编排器控制提升时机
    /// - 分离关注点：观察（record_outcome）vs 决策（maybe_promote）
    pub fn record_outcome(&mut self, success: bool) {
        let reward = if success { 1.0_f32 } else { 0.0_f32 };
        // EWMA 更新: ewma = (1-α) × ewma + α × reward
        self.success_ewma =
            (1.0 - Self::EWMA_ALPHA) * self.success_ewma + Self::EWMA_ALPHA * reward;
        self.sample_count += 1;
    }

    /// 尝试渐进授权提升（自适应步长）
    ///
    /// # 提升规则
    /// - 仅在 `Authorized` 状态下提升（Provisional 需先达到阈值）
    /// - 仅在 EWMA 成功率 >= 0.7 时提升（避免奖励黑客）
    /// - 步长 = `PROMOTION_STEP_BASE × (1.0 - current_level)`
    /// - 提升后若 `authorized_level >= ACTIVATION_THRESHOLD`，状态转为 `Authorized`
    ///
    /// # 返回
    /// - `true`: level 实际提升
    /// - `false`: level 未提升（状态不符或 EWMA 不足）
    ///
    /// # 示例
    ///
    /// ```
    /// use nexus_contracts::{CapabilityToken, SeamId};
    ///
    /// let mut token = CapabilityToken::new("s2-memory-v1", SeamId::S2Memory);
    /// let initial_level = token.authorized_level();
    ///
    /// // EWMA 不足，不提升
    /// token.maybe_promote();
    /// assert_eq!(token.authorized_level(), initial_level);
    ///
    /// // 多次成功 outcome 提升 EWMA
    /// for _ in 0..10 {
    ///     token.record_outcome(true);
    /// }
    /// token.maybe_promote();
    /// assert!(token.authorized_level() > initial_level);
    /// ```
    pub fn maybe_promote(&mut self) -> bool {
        // 仅在 Provisional 或 Authorized 状态下可提升
        // Cooldown/Frozen 状态不提升（需先恢复）
        match self.status {
            CapabilityTokenStatus::Provisional | CapabilityTokenStatus::Authorized => {}
            CapabilityTokenStatus::Cooldown | CapabilityTokenStatus::Frozen => return false,
        }

        // EWMA 成功率门槛：避免奖励黑客（WHY 0.7 而非 0.5）
        // - 0.7 表明策略在 70% 的场景下表现良好
        // - 0.5 仅略高于随机，不足以证明策略有效
        if self.success_ewma < 0.7 {
            return false;
        }

        // 自适应步长：低 level 快速提升，高 level 谨慎逼近
        let step = Self::PROMOTION_STEP_BASE * (1.0 - self.authorized_level);
        let new_level = (self.authorized_level + step).min(1.0);
        if new_level <= self.authorized_level {
            return false; // 已达上限 1.0
        }

        self.authorized_level = new_level;

        // 达到激活阈值时状态转为 Authorized
        if self.authorized_level >= Self::ACTIVATION_THRESHOLD
            && self.status == CapabilityTokenStatus::Provisional
        {
            self.status = CapabilityTokenStatus::Authorized;
        }

        true
    }

    /// 应用衰减（AsaIntervention 触发或手动衰减）
    ///
    /// # 参数
    /// - `amount`: 衰减量（正数，level 减少量）
    ///
    /// # 效果
    /// - `authorized_level -= amount`，clamp 到 [0.0, 1.0]
    /// - 若 level 降到 `ACTIVATION_THRESHOLD` 以下，状态转为 `Provisional`
    pub fn decay(&mut self, amount: f32) {
        let amount = amount.max(0.0); // 确保非负
                                      // clamp 保证 level 在 [0.0, 1.0] 范围内（clippy::manual_clamp）
        self.authorized_level = (self.authorized_level - amount).clamp(0.0, 1.0);

        // level 降到阈值以下，状态降级为 Provisional
        if self.authorized_level < Self::ACTIVATION_THRESHOLD
            && self.status == CapabilityTokenStatus::Authorized
        {
            self.status = CapabilityTokenStatus::Provisional;
        }
    }

    /// 触发 AsaIntervention 安全闭环
    ///
    /// 编排器在收到 `AsaIntervention` 事件时调用此方法。
    ///
    /// # 效果
    /// 1. `authorized_level -= DECAY_ON_ASA (0.2)`
    /// 2. `status = Cooldown`
    /// 3. `cooldown_until = now + COOLDOWN_DURATION_SECS (60)`
    /// 4. `consecutive_asa_count += 1`
    /// 5. 若 `consecutive_asa_count >= ASA_FREEZE_THRESHOLD (3)`，状态转为 `Frozen`
    ///
    /// # 参数
    /// - `now`: 当前 UTC 秒时间戳
    ///
    /// # 返回
    /// - `true`: 触发了自动冻结（consecutive_asa_count 达阈值）
    /// - `false`: 仅进入冷却期
    ///
    /// # 示例
    ///
    /// ```
    /// use nexus_contracts::{CapabilityToken, SeamId, CapabilityTokenStatus};
    ///
    /// let mut token = CapabilityToken::new("s5-parliament-v1", SeamId::S5Parliament);
    ///
    /// // 第一次 ASA 触发：进入冷却期
    /// let frozen = token.trigger_asa_intervention(1000);
    /// assert!(!frozen);
    /// assert_eq!(token.status(), CapabilityTokenStatus::Cooldown);
    /// assert!(token.is_in_cooldown(1000));
    ///
    /// // 冷却期结束后恢复
    /// assert!(!token.is_in_cooldown(1061)); // 60s 后
    /// ```
    pub fn trigger_asa_intervention(&mut self, now: i64) -> bool {
        // 1. 衰减 level
        self.decay(Self::DECAY_ON_ASA);

        // 2. 进入冷却期
        self.status = CapabilityTokenStatus::Cooldown;
        self.cooldown_until = Some(now + Self::COOLDOWN_DURATION_SECS);

        // 3. 累计 ASA 次数
        self.consecutive_asa_count += 1;

        // 4. 达阈值自动冻结
        if self.consecutive_asa_count >= Self::ASA_FREEZE_THRESHOLD {
            self.status = CapabilityTokenStatus::Frozen;
            self.cooldown_until = None; // Frozen 不需要 cooldown_until
            return true;
        }

        false
    }

    /// 检查并恢复冷却期结束后的状态
    ///
    /// 编排器定期调用此方法（如每秒），将冷却期结束的 token 恢复到正常状态。
    ///
    /// # 参数
    /// - `now`: 当前 UTC 秒时间戳
    ///
    /// # 返回
    /// - `true`: 状态实际恢复（从 Cooldown 转为 Provisional/Authorized）
    /// - `false`: 状态未变化（仍在冷却期或非 Cooldown 状态）
    pub fn maybe_recover_from_cooldown(&mut self, now: i64) -> bool {
        if self.status != CapabilityTokenStatus::Cooldown {
            return false;
        }

        // 检查冷却期是否结束
        let in_cooldown = match self.cooldown_until {
            Some(until) => now < until,
            None => false,
        };

        if in_cooldown {
            return false;
        }

        // 冷却期结束，根据 level 恢复状态
        if self.authorized_level >= Self::ACTIVATION_THRESHOLD {
            self.status = CapabilityTokenStatus::Authorized;
        } else {
            self.status = CapabilityTokenStatus::Provisional;
        }
        self.cooldown_until = None;
        // 连续 ASA 计数在冷却期结束后重置（WHY: 给策略重新证明的机会）
        self.consecutive_asa_count = 0;

        true
    }

    /// 手动冻结令牌（编排器强制熔断）
    ///
    /// WHY 提供: 当学习策略导致严重后果（如 quest 失败）时，
    /// 编排器可立即冻结，无需等待 3 次 ASA 累积。
    pub fn freeze(&mut self) {
        self.status = CapabilityTokenStatus::Frozen;
        self.cooldown_until = None;
        self.authorized_level = 0.0; // 冻结时 level 清零
    }

    /// 手动解冻令牌（重置为初始状态）
    ///
    /// WHY 提供: 冻结后需手动恢复，避免自动恢复导致策略再次激活
    /// 解冻后回到 `Provisional` 状态，需重新累积 EWMA 才能再次激活
    pub fn unfreeze(&mut self) {
        self.status = CapabilityTokenStatus::Provisional;
        self.authorized_level = Self::INITIAL_LEVEL;
        self.success_ewma = 0.5; // 重置为中性先验
        self.sample_count = 0;
        self.consecutive_asa_count = 0;
        self.cooldown_until = None;
        self.bound_policy_version = None;
    }
}

impl Default for CapabilityToken {
    /// 默认实现：S1Density 接缝的初始 token
    ///
    /// WHY 提供 Default: 便于测试与快速构造
    fn default() -> Self {
        Self::new("default", SeamId::S1Density)
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================
    // SeamId 测试
    // ============================================================

    #[test]
    fn test_seam_id_number() {
        assert_eq!(SeamId::S1Density.number(), 1);
        assert_eq!(SeamId::S2Memory.number(), 2);
        assert_eq!(SeamId::S3Prefetch.number(), 3);
        assert_eq!(SeamId::S4Selector.number(), 4);
        assert_eq!(SeamId::S5Parliament.number(), 5);
        assert_eq!(SeamId::S6Decay.number(), 6);
        // P4-W16.2.2: S7 召回配额（R1 离线 RL 接缝）
        assert_eq!(SeamId::S7RecallQuota.number(), 7);
        // closure Stage B-6: S8 Mem-π 记忆决策接缝
        assert_eq!(SeamId::S8MemPi.number(), 8);
    }

    #[test]
    fn test_seam_id_short_name() {
        assert_eq!(SeamId::S1Density.short_name(), "S1-density");
        assert_eq!(SeamId::S2Memory.short_name(), "S2-memory");
        assert_eq!(SeamId::S3Prefetch.short_name(), "S3-prefetch");
        assert_eq!(SeamId::S4Selector.short_name(), "S4-selector");
        assert_eq!(SeamId::S5Parliament.short_name(), "S5-parliament");
        assert_eq!(SeamId::S6Decay.short_name(), "S6-decay");
        // P4-W16.2.2: S7 简称
        assert_eq!(SeamId::S7RecallQuota.short_name(), "S7-recall-quota");
        // closure Stage B-6: S8 简称
        assert_eq!(SeamId::S8MemPi.short_name(), "S8-mem-pi");
    }

    #[test]
    fn test_seam_id_all_returns_eight() {
        let all = SeamId::all();
        assert_eq!(all.len(), 8);
        assert!(all.contains(&SeamId::S1Density));
        assert!(all.contains(&SeamId::S6Decay));
        // P4-W16.2.2: S7 必须在 all() 中
        assert!(all.contains(&SeamId::S7RecallQuota));
        // closure Stage B-6: S8 必须在 all() 中
        assert!(all.contains(&SeamId::S8MemPi));
    }

    #[test]
    fn test_seam_id_all_unique() {
        let all = SeamId::all();
        let mut seen = std::collections::HashSet::new();
        for seam in all.iter() {
            assert!(seen.insert(seam.number()), "duplicate seam number");
        }
    }

    #[test]
    fn test_seam_id_display() {
        assert_eq!(format!("{}", SeamId::S1Density), "S1-density");
        assert_eq!(format!("{}", SeamId::S4Selector), "S4-selector");
    }

    #[test]
    fn test_seam_id_serialize_json() {
        let seam = SeamId::S4Selector;
        let json = serde_json::to_string(&seam).unwrap();
        let deserialized: SeamId = serde_json::from_str(&json).unwrap();
        assert_eq!(seam, deserialized);
    }

    #[test]
    fn test_seam_id_repr_u8() {
        // 验证 #[repr(u8)]: 内存中占 1 字节
        assert_eq!(std::mem::size_of::<SeamId>(), 1);
    }

    // ============================================================
    // CapabilityTokenStatus 测试
    // ============================================================

    #[test]
    fn test_status_as_str() {
        assert_eq!(CapabilityTokenStatus::Provisional.as_str(), "Provisional");
        assert_eq!(CapabilityTokenStatus::Authorized.as_str(), "Authorized");
        assert_eq!(CapabilityTokenStatus::Cooldown.as_str(), "Cooldown");
        assert_eq!(CapabilityTokenStatus::Frozen.as_str(), "Frozen");
    }

    #[test]
    fn test_status_allows_learned() {
        assert!(!CapabilityTokenStatus::Provisional.allows_learned());
        assert!(CapabilityTokenStatus::Authorized.allows_learned());
        assert!(!CapabilityTokenStatus::Cooldown.allows_learned());
        assert!(!CapabilityTokenStatus::Frozen.allows_learned());
    }

    #[test]
    fn test_status_display() {
        assert_eq!(
            format!("{}", CapabilityTokenStatus::Provisional),
            "Provisional"
        );
        assert_eq!(
            format!("{}", CapabilityTokenStatus::Authorized),
            "Authorized"
        );
    }

    #[test]
    fn test_status_serialize_json() {
        let status = CapabilityTokenStatus::Authorized;
        let json = serde_json::to_string(&status).unwrap();
        let deserialized: CapabilityTokenStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(status, deserialized);
    }

    // ============================================================
    // CapabilityToken 基础行为测试
    // ============================================================

    #[test]
    fn test_new_initial_state() {
        let token = CapabilityToken::new("s6-decay-v1", SeamId::S6Decay);
        assert_eq!(token.token_id(), "s6-decay-v1");
        assert_eq!(token.seam(), SeamId::S6Decay);
        assert!((token.authorized_level() - 0.2).abs() < 1e-6);
        assert_eq!(token.status(), CapabilityTokenStatus::Provisional);
        assert_eq!(token.bound_policy_version(), None);
        assert!((token.success_ewma() - 0.5).abs() < 1e-6);
        assert_eq!(token.sample_count(), 0);
        assert_eq!(token.consecutive_asa_count(), 0);
    }

    #[test]
    fn test_new_does_not_allow_learned() {
        // C4 合规: 初始状态不允许 Learned 策略
        let token = CapabilityToken::new("s1-density-v1", SeamId::S1Density);
        assert!(!token.allows_learned_policy(0));
    }

    #[test]
    fn test_default_is_s1_density() {
        let token = CapabilityToken::default();
        assert_eq!(token.seam(), SeamId::S1Density);
        assert_eq!(token.token_id(), "default");
    }

    // ============================================================
    // record_outcome 测试
    // ============================================================

    #[test]
    fn test_record_outcome_success_increases_ewma() {
        let mut token = CapabilityToken::new("test", SeamId::S1Density);
        let initial_ewma = token.success_ewma();

        token.record_outcome(true);
        // EWMA: 0.9 × 0.5 + 0.1 × 1.0 = 0.45 + 0.1 = 0.55
        assert!((token.success_ewma() - 0.55).abs() < 1e-6);
        assert!(token.success_ewma() > initial_ewma);
        assert_eq!(token.sample_count(), 1);
    }

    #[test]
    fn test_record_outcome_failure_decreases_ewma() {
        let mut token = CapabilityToken::new("test", SeamId::S1Density);
        let initial_ewma = token.success_ewma();

        token.record_outcome(false);
        // EWMA: 0.9 × 0.5 + 0.1 × 0.0 = 0.45
        assert!((token.success_ewma() - 0.45).abs() < 1e-6);
        assert!(token.success_ewma() < initial_ewma);
        assert_eq!(token.sample_count(), 1);
    }

    #[test]
    fn test_record_outcome_multiple_samples() {
        let mut token = CapabilityToken::new("test", SeamId::S1Density);

        // 10 次成功 outcome
        for _ in 0..10 {
            token.record_outcome(true);
        }
        assert_eq!(token.sample_count(), 10);
        // EWMA 应趋近 1.0
        assert!(token.success_ewma() > 0.6);
    }

    #[test]
    fn test_record_outcome_does_not_modify_level() {
        // WHY: record_outcome 仅更新 EWMA，不修改 level
        let mut token = CapabilityToken::new("test", SeamId::S1Density);
        let initial_level = token.authorized_level();

        token.record_outcome(true);
        token.record_outcome(true);
        token.record_outcome(true);

        assert_eq!(token.authorized_level(), initial_level);
    }

    // ============================================================
    // maybe_promote 测试
    // ============================================================

    #[test]
    fn test_maybe_promote_no_promotion_when_ewma_low() {
        let mut token = CapabilityToken::new("test", SeamId::S1Density);
        let initial_level = token.authorized_level();

        // EWMA = 0.5（初始），低于 0.7 门槛
        token.maybe_promote();
        assert_eq!(token.authorized_level(), initial_level);
    }

    #[test]
    fn test_maybe_promote_promotion_when_ewma_high() {
        let mut token = CapabilityToken::new("test", SeamId::S1Density);
        let initial_level = token.authorized_level();

        // 10 次成功 outcome 提升 EWMA 到 ~0.7+
        for _ in 0..10 {
            token.record_outcome(true);
        }
        assert!(token.success_ewma() >= 0.7);

        let promoted = token.maybe_promote();
        assert!(promoted);
        assert!(token.authorized_level() > initial_level);

        // 验证自适应步长: 0.1 × (1 - 0.2) = 0.08
        let expected_step = 0.1 * (1.0 - initial_level);
        assert!((token.authorized_level() - (initial_level + expected_step)).abs() < 1e-6);
    }

    #[test]
    fn test_maybe_promote_status_transitions_to_authorized() {
        let mut token = CapabilityToken::new("test", SeamId::S1Density);
        assert_eq!(token.status(), CapabilityTokenStatus::Provisional);

        // 多次成功 outcome + 提升，直到达到 ACTIVATION_THRESHOLD
        for _ in 0..20 {
            token.record_outcome(true);
            token.maybe_promote();
        }

        assert!(token.authorized_level() >= CapabilityToken::ACTIVATION_THRESHOLD);
        assert_eq!(token.status(), CapabilityTokenStatus::Authorized);
    }

    #[test]
    fn test_maybe_promote_no_promotion_in_cooldown() {
        let mut token = CapabilityToken::new("test", SeamId::S1Density);

        // 先提升到 Authorized
        for _ in 0..20 {
            token.record_outcome(true);
            token.maybe_promote();
        }
        assert_eq!(token.status(), CapabilityTokenStatus::Authorized);

        // 触发 ASA 进入冷却期
        token.trigger_asa_intervention(1000);
        assert_eq!(token.status(), CapabilityTokenStatus::Cooldown);

        let level_before = token.authorized_level();
        token.maybe_promote(); // 不应提升
        assert_eq!(token.authorized_level(), level_before);
    }

    #[test]
    fn test_maybe_promote_no_promotion_when_frozen() {
        let mut token = CapabilityToken::new("test", SeamId::S1Density);
        token.freeze();
        assert_eq!(token.status(), CapabilityTokenStatus::Frozen);

        let level_before = token.authorized_level();
        token.maybe_promote();
        assert_eq!(token.authorized_level(), level_before);
    }

    // ============================================================
    // allows_learned_policy 测试
    // ============================================================

    #[test]
    fn test_allows_learned_policy_provisional_returns_false() {
        let token = CapabilityToken::new("test", SeamId::S1Density);
        assert!(!token.allows_learned_policy(0));
    }

    #[test]
    fn test_allows_learned_policy_authorized_returns_true() {
        let mut token = CapabilityToken::new("test", SeamId::S1Density);
        for _ in 0..20 {
            token.record_outcome(true);
            token.maybe_promote();
        }
        assert!(token.allows_learned_policy(0));
    }

    #[test]
    fn test_allows_learned_policy_cooldown_returns_false() {
        let mut token = CapabilityToken::new("test", SeamId::S1Density);
        for _ in 0..20 {
            token.record_outcome(true);
            token.maybe_promote();
        }
        assert!(token.allows_learned_policy(0));

        token.trigger_asa_intervention(1000);
        assert!(!token.allows_learned_policy(1000)); // 在冷却期
    }

    #[test]
    fn test_allows_learned_policy_frozen_returns_false() {
        let mut token = CapabilityToken::new("test", SeamId::S1Density);
        token.freeze();
        assert!(!token.allows_learned_policy(0));
    }

    #[test]
    fn test_allows_learned_policy_cooldown_expired_still_authorized() {
        // 冷却期结束后，若 level 仍 >= 阈值，恢复 Authorized
        let mut token = CapabilityToken::new("test", SeamId::S1Density);
        for _ in 0..20 {
            token.record_outcome(true);
            token.maybe_promote();
        }
        assert!(token.allows_learned_policy(0));

        token.trigger_asa_intervention(1000);
        assert!(!token.allows_learned_policy(1000));

        // 冷却期结束后（1061s）
        assert!(token.allows_learned_policy(1061));
    }

    // ============================================================
    // is_in_cooldown 测试
    // ============================================================

    #[test]
    fn test_is_in_cooldown_during_cooldown() {
        let mut token = CapabilityToken::new("test", SeamId::S1Density);
        token.trigger_asa_intervention(1000);
        assert!(token.is_in_cooldown(1000));
        assert!(token.is_in_cooldown(1059));
        assert!(!token.is_in_cooldown(1060)); // cooldown_until = 1060
    }

    #[test]
    fn test_is_in_cooldown_not_in_cooldown() {
        let token = CapabilityToken::new("test", SeamId::S1Density);
        assert!(!token.is_in_cooldown(0));
    }

    // ============================================================
    // trigger_asa_intervention 测试
    // ============================================================

    #[test]
    fn test_trigger_asa_intervention_enters_cooldown() {
        let mut token = CapabilityToken::new("test", SeamId::S1Density);
        let initial_level = token.authorized_level();

        let frozen = token.trigger_asa_intervention(1000);
        assert!(!frozen); // 第一次未冻结
        assert_eq!(token.status(), CapabilityTokenStatus::Cooldown);
        assert_eq!(token.consecutive_asa_count(), 1);
        // level 衰减 0.2
        assert!((token.authorized_level() - (initial_level - 0.2)).abs() < 1e-6);
    }

    #[test]
    fn test_trigger_asa_intervention_auto_freeze_after_three() {
        let mut token = CapabilityToken::new("test", SeamId::S1Density);

        // 第一次 ASA
        let f1 = token.trigger_asa_intervention(1000);
        assert!(!f1);
        assert_eq!(token.status(), CapabilityTokenStatus::Cooldown);

        // 第二次 ASA（冷却期内的 ASA 调用，模拟连续触发）
        let f2 = token.trigger_asa_intervention(1100);
        assert!(!f2);
        assert_eq!(token.consecutive_asa_count(), 2);

        // 第三次 ASA → 自动冻结
        let f3 = token.trigger_asa_intervention(1200);
        assert!(f3);
        assert_eq!(token.status(), CapabilityTokenStatus::Frozen);
    }

    #[test]
    fn test_trigger_asa_intervention_decay_amount() {
        let mut token = CapabilityToken::new("test", SeamId::S1Density);
        let initial_level = token.authorized_level();

        token.trigger_asa_intervention(1000);
        // DECAY_ON_ASA = 0.2
        assert!((token.authorized_level() - (initial_level - 0.2)).abs() < 1e-6);
    }

    // ============================================================
    // maybe_recover_from_cooldown 测试
    // ============================================================

    #[test]
    fn test_recover_from_cooldown_after_expiry() {
        let mut token = CapabilityToken::new("test", SeamId::S1Density);
        for _ in 0..20 {
            token.record_outcome(true);
            token.maybe_promote();
        }
        assert_eq!(token.status(), CapabilityTokenStatus::Authorized);

        token.trigger_asa_intervention(1000);
        assert_eq!(token.status(), CapabilityTokenStatus::Cooldown);

        // 冷却期内不恢复
        assert!(!token.maybe_recover_from_cooldown(1059));

        // 冷却期结束后恢复
        assert!(token.maybe_recover_from_cooldown(1061));
        // level 仍 >= 阈值，恢复为 Authorized
        assert_eq!(token.status(), CapabilityTokenStatus::Authorized);
        // consecutive_asa_count 重置
        assert_eq!(token.consecutive_asa_count(), 0);
    }

    #[test]
    fn test_recover_from_cooldown_level_below_threshold() {
        let mut token = CapabilityToken::new("test", SeamId::S1Density);
        // 不提升 level，保持 0.2

        token.trigger_asa_intervention(1000);
        assert_eq!(token.status(), CapabilityTokenStatus::Cooldown);

        // 冷却期结束后，level = 0.2 - 0.2 = 0.0 < 0.3，恢复为 Provisional
        assert!(token.maybe_recover_from_cooldown(1061));
        assert_eq!(token.status(), CapabilityTokenStatus::Provisional);
    }

    #[test]
    fn test_recover_from_cooldown_non_cooldown_returns_false() {
        let mut token = CapabilityToken::new("test", SeamId::S1Density);
        assert!(!token.maybe_recover_from_cooldown(0));

        token.freeze();
        assert!(!token.maybe_recover_from_cooldown(0));
    }

    // ============================================================
    // freeze / unfreeze 测试
    // ============================================================

    #[test]
    fn test_freeze_sets_status_and_level() {
        let mut token = CapabilityToken::new("test", SeamId::S1Density);
        token.freeze();
        assert_eq!(token.status(), CapabilityTokenStatus::Frozen);
        assert!(token.authorized_level().abs() < 1e-6);
    }

    #[test]
    fn test_unfreeze_resets_to_initial() {
        let mut token = CapabilityToken::new("test", SeamId::S1Density);
        for _ in 0..20 {
            token.record_outcome(true);
            token.maybe_promote();
        }
        token.freeze();

        token.unfreeze();
        assert_eq!(token.status(), CapabilityTokenStatus::Provisional);
        assert!((token.authorized_level() - CapabilityToken::INITIAL_LEVEL).abs() < 1e-6);
        assert!((token.success_ewma() - 0.5).abs() < 1e-6);
        assert_eq!(token.sample_count(), 0);
        assert_eq!(token.consecutive_asa_count(), 0);
    }

    // ============================================================
    // decay 测试
    // ============================================================

    #[test]
    fn test_decay_reduces_level() {
        let mut token = CapabilityToken::new("test", SeamId::S1Density);
        for _ in 0..20 {
            token.record_outcome(true);
            token.maybe_promote();
        }
        let level_before = token.authorized_level();

        token.decay(0.1);
        assert!((token.authorized_level() - (level_before - 0.1)).abs() < 1e-6);
    }

    #[test]
    fn test_decay_below_threshold_transitions_to_provisional() {
        let mut token = CapabilityToken::new("test", SeamId::S1Density);
        for _ in 0..20 {
            token.record_outcome(true);
            token.maybe_promote();
        }
        assert_eq!(token.status(), CapabilityTokenStatus::Authorized);

        // 衰减到阈值以下
        let level = token.authorized_level();
        token.decay(level - 0.01); // 衰减到略低于阈值
        assert!(token.authorized_level() < CapabilityToken::ACTIVATION_THRESHOLD);
        assert_eq!(token.status(), CapabilityTokenStatus::Provisional);
    }

    #[test]
    fn test_decay_clamps_to_zero() {
        let mut token = CapabilityToken::new("test", SeamId::S1Density);
        token.decay(1.0); // 衰减量超过当前 level
        assert!(token.authorized_level().abs() < 1e-6);
    }

    // ============================================================
    // bind_policy_version 测试
    // ============================================================

    #[test]
    fn test_bind_policy_version() {
        let mut token = CapabilityToken::new("test", SeamId::S1Density);
        assert_eq!(token.bound_policy_version(), None);

        token.bind_policy_version(42);
        assert_eq!(token.bound_policy_version(), Some(42));
    }

    // ============================================================
    // 端到端场景测试
    // ============================================================

    #[test]
    fn test_scenario_full_lifecycle() {
        // 模拟完整生命周期: Provisional → Authorized → Cooldown → Recover → Frozen
        let mut token = CapabilityToken::new("s6-decay-v1", SeamId::S6Decay);

        // 1. 初始 Provisional，不允许 Learned
        assert!(!token.allows_learned_policy(0));

        // 2. 累积成功 outcome + 提升，达到 Authorized
        for _ in 0..20 {
            token.record_outcome(true);
            token.maybe_promote();
        }
        assert!(token.allows_learned_policy(0));
        assert_eq!(token.status(), CapabilityTokenStatus::Authorized);

        // 3. 绑定策略版本
        token.bind_policy_version(1);

        // 4. 触发 ASA 进入冷却期
        token.trigger_asa_intervention(1000);
        assert!(!token.allows_learned_policy(1000));
        assert_eq!(token.status(), CapabilityTokenStatus::Cooldown);

        // 5. 冷却期结束恢复
        assert!(token.maybe_recover_from_cooldown(1061));
        assert_eq!(token.consecutive_asa_count(), 0);

        // 6. 连续 3 次 ASA 自动冻结
        token.trigger_asa_intervention(2000);
        token.trigger_asa_intervention(2100);
        let frozen = token.trigger_asa_intervention(2200);
        assert!(frozen);
        assert_eq!(token.status(), CapabilityTokenStatus::Frozen);

        // 7. 手动解冻，回到初始状态
        token.unfreeze();
        assert_eq!(token.status(), CapabilityTokenStatus::Provisional);
        assert!(!token.allows_learned_policy(0));
    }

    #[test]
    fn test_scenario_adaptive_step_decreases_at_high_level() {
        // 验证自适应步长: 高 level 时步长更小（谨慎逼近）
        let mut token = CapabilityToken::new("test", SeamId::S1Density);

        // 提升到较高 level
        for _ in 0..100 {
            token.record_outcome(true);
            token.maybe_promote();
        }
        let high_level = token.authorized_level();
        assert!(high_level > 0.5);

        // 记录步长
        let step_high = {
            let level_before = token.authorized_level();
            token.maybe_promote();
            token.authorized_level() - level_before
        };

        // 与低 level 时的步长比较
        let mut token_low = CapabilityToken::new("test2", SeamId::S1Density);
        for _ in 0..10 {
            token_low.record_outcome(true);
            token_low.maybe_promote();
        }
        let level_before_low = token_low.authorized_level();
        token_low.maybe_promote();
        let step_low = token_low.authorized_level() - level_before_low;

        // 高 level 时步长应小于低 level 时步长
        assert!(step_high < step_low);
    }

    #[test]
    fn test_scenario_ewma_convergence_with_mixed_outcomes() {
        // 验证 EWMA 对混合结果的收敛性
        let mut token = CapabilityToken::new("test", SeamId::S1Density);

        // 70% 成功 + 30% 失败 → EWMA 应趋近 0.7
        //
        // WHY 使用 `(i * 7) % 10 < 7` 而非 `i % 10 < 7`:
        // - `i % 10 < 7` 将 3 次 failure 聚集在周期末尾(位置 7,8,9),
        //   导致 EWMA 在周期末尾处于低点(~0.584),偏离 0.7 达 0.116
        //   (数学验证: 稳态 E = 0.3487 × E + 0.3803 → E ≈ 0.584)
        // - `(i * 7) % 10 < 7` 将 3 次 failure 均匀分布在周期内
        //   (位置 1,4,7),使 EWMA 稳态接近真实均值 0.7,波动幅度 < 0.05
        // - 两种模式都恰好 70% 成功率(每 10 样本 7 成功 3 失败),
        //   但分布方式影响 EWMA 在特定采样点的瞬时值
        for i in 0..100 {
            let success = (i * 7) % 10 < 7; // 70% 成功,均匀分布
            token.record_outcome(success);
        }
        // EWMA 应接近 0.7(均匀分布模式下稳态波动 < 0.05)
        assert!(
            (token.success_ewma() - 0.7).abs() < 0.05,
            "EWMA 应收敛到 0.7 附近,实际值: {}",
            token.success_ewma()
        );
        assert_eq!(token.sample_count(), 100);
    }

    // ============================================================
    // 序列化测试
    // ============================================================

    #[test]
    fn test_token_serialize_deserialize_json() {
        let mut token = CapabilityToken::new("s6-decay-v1", SeamId::S6Decay);
        token.record_outcome(true);
        token.maybe_promote();
        token.bind_policy_version(42);

        let json = serde_json::to_string(&token).unwrap();
        let deserialized: CapabilityToken = serde_json::from_str(&json).unwrap();

        assert_eq!(deserialized.token_id(), "s6-decay-v1");
        assert_eq!(deserialized.seam(), SeamId::S6Decay);
        assert!((deserialized.authorized_level() - token.authorized_level()).abs() < 1e-6);
        assert_eq!(deserialized.status(), token.status());
        assert_eq!(deserialized.bound_policy_version(), Some(42));
        assert_eq!(deserialized.sample_count(), 1);
    }

    #[test]
    fn test_token_clone_independent() {
        // 验证 Clone 后两个 token 状态独立（修改一方不影响另一方）
        let mut token = CapabilityToken::new("test", SeamId::S1Density);

        // WHY 10 次 record_outcome(true):
        // - 初始 EWMA = 0.5,单次成功后 EWMA = 0.55 < 0.7（maybe_promote 门槛）
        // - 10 次成功后 EWMA = 1 - 0.9^10 × 0.5 ≈ 0.974 >= 0.7,满足提升条件
        // - 不满足条件时 maybe_promote 不提升 level,导致 clone 前后 level 相同,
        //   assert_ne! 失败
        for _ in 0..10 {
            token.record_outcome(true);
        }
        let cloned = token.clone();

        // 修改原 token: maybe_promote 实际提升 level
        token.maybe_promote();

        // 克隆 token 的 level 保持不变（Clone 语义独立性）
        assert_ne!(
            token.authorized_level(),
            cloned.authorized_level(),
            "Clone 后修改原 token 应不影响克隆副本"
        );
    }
}
