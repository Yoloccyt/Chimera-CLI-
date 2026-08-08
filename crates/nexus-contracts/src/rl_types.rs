//! RL 共享类型契约 — RLState / RLAction / RLExperience
//!
//! 对应方案: `CHIMERA_V3_专项优化方案_v2.21基线.md` §5.1 P1（RL 共享类型补齐）
//! 对应 ADR: ADR-049 修订（原裁决"rl-types ✅ 本期"实为部分落地，VariantId/
//! BehaviorContract/ProceduralBlueprint 已落地，RLState/RLAction/RLExperience 未落地
//! ——2026-08-07 全仓库 Grep 复核，修订为 v3.0.x 补齐，即本模块）。
//!
//! # 职责
//!
//! 承载跨接缝 RL 训练所需的共享数据类型：
//! - `RLAction`: S1-S9 接缝动作的封闭枚举（每变体对应一个接缝契约类型）
//! - `RLState`: 上下文状态快照（context 向量 + 可选任务阶段/预算水位）
//! - `RLExperience`: 经验四元组（state, action, reward, next_state）
//!
//! # 硬约束（ADR-033 / ADR-042）
//!
//! - **纯类型零逻辑**: 仅 serde derive + 枚举/结构体定义（ADR-033 L0 契约层）
//! - **不含训练逻辑**: 无梯度/LinUCB/回放算法（R2 冻结面外，ADR-042）
//! - **接缝映射**: `RLAction` 变体即映射——每个变体包装一个既有接缝契约类型，
//!   `seam_id()` 返回对应 `SeamId`（Route/Custom 因 SeamId 枚举未覆盖返回 `None`）
//! - **f32 字段不 derive Eq/Hash**: context/reward 仅 `PartialEq`（浮点比较红线）

use serde::{Deserialize, Serialize};

use crate::capability_token::SeamId;
use crate::decay_profile::DecayProfile;
use crate::density::DensityTier;
use crate::memory_strategy::MemoryTaskPhase;
use crate::parliament_policy::ActivationStrategy;
use crate::prefetch::PrefetchStrategy;
use crate::recall_quota::RecallQuota;
use crate::strategy::MemoryStrategy;

// ============================================================
// RLAction — 接缝动作封闭枚举
// ============================================================

/// Mem-π 记忆决策动作 — S8 接缝三臂（Generate/Retrieve/Abstain）
///
/// 与 `omega_learner::s8_mem_pi::MemPiDecision` 语义同构（L0 纯类型，
/// 不依赖 L6；由 L6 侧做接缝转换）。Abstain 为不确定性保守护栏臂
/// （ABSTAIN_UNCERTAINTY_THRESHOLD=0.7 不可绕过，ADR-043 影子期守护）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum MemPiAction {
    /// 生成记忆 — 将当前上下文压缩为新记忆条目
    Generate = 0,
    /// 检索记忆 — 从既有记忆召回
    Retrieve = 1,
    /// 弃权 — 不确定时不操作（保守护栏，避免有害生成）
    Abstain = 2,
}

/// RL 动作 — S1-S9 接缝动作的封闭枚举（接缝映射载体）
///
/// 每个变体包装一个既有 L0 接缝契约类型；消费方（回放池/训练服务）按变体
/// 解构即可还原接缝原始动作，无需了解接缝内部细节。
///
/// # 映射表
///
/// | 变体 | 接缝 | 载荷类型 |
/// |---|---|---|
/// | `Density` | S1 | `DensityTier`（hcw-window 密度档位） |
/// | `Memory` | S2 | `MemoryStrategy`（mlc-engine 记忆策略） |
/// | `Prefetch` | S3 | `PrefetchStrategy`（scc-cache 预取策略） |
/// | `Selector` | S4 | `SelectorPolicy`（selector 权重策略） |
/// | `Parliament` | S5 | `ActivationStrategy`（Parliament 激活策略） |
/// | `Decay` | S6 | `DecayProfile`（decay-engine 衰减档位） |
/// | `RecallQuota` | S7 | `RecallQuota`（R1 召回配额，离线 RL） |
/// | `MemPi` | S8 | `MemPiAction`（Mem-π 三臂） |
/// | `Route` | S9 | `String`（provider/model/mode 组合臂串） |
/// | `Custom` | — | `String`（扩展点，供未来接缝使用） |
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RLAction {
    /// S1: 密度档位动作（hcw-window selector）
    Density(DensityTier),
    /// S2: 记忆策略动作（mlc-engine recall）
    Memory(MemoryStrategy),
    /// S3: 预取策略动作（scc-cache prefetch）
    Prefetch(PrefetchStrategy),
    /// S4: selector 权重策略动作（hcw-window selector）
    Selector(crate::policy::SelectorPolicy),
    /// S5: Parliament 激活策略动作（Fast Path）
    Parliament(ActivationStrategy),
    /// S6: 衰减档位动作（decay-engine DecayProfile）
    Decay(DecayProfile),
    /// S7: 召回配额动作（R1 离线 RL 接缝，CQL/IQL）
    RecallQuota(RecallQuota),
    /// S8: Mem-π 记忆决策动作（三臂）
    MemPi(MemPiAction),
    /// S9: 通道路由臂（provider/model/mode 组合串，RouteLLM 落点）
    Route(String),
    /// 扩展点: 未来接缝动作（保持枚举封闭性与向后兼容）
    Custom(String),
}

impl RLAction {
    /// 返回动作所属接缝标识
    ///
    /// `Route`/`Custom` 返回 `None`：SeamId 枚举的 9 号位为 `S9TokenEfficiency`
    /// （ADR-069），未覆盖 S9Route 接缝（omega-learner s9_route，RouteLLM 落点）；
    /// Custom 为预留扩展位。诚实表达而非硬塞语义错误的接缝号。
    pub const fn seam_id(&self) -> Option<SeamId> {
        match self {
            Self::Density(_) => Some(SeamId::S1Density),
            Self::Memory(_) => Some(SeamId::S2Memory),
            Self::Prefetch(_) => Some(SeamId::S3Prefetch),
            Self::Selector(_) => Some(SeamId::S4Selector),
            Self::Parliament(_) => Some(SeamId::S5Parliament),
            Self::Decay(_) => Some(SeamId::S6Decay),
            Self::RecallQuota(_) => Some(SeamId::S7RecallQuota),
            Self::MemPi(_) => Some(SeamId::S8MemPi),
            Self::Route(_) | Self::Custom(_) => None,
        }
    }
}

// ============================================================
// RLState — 上下文状态快照
// ============================================================

/// RL 状态 — 接缝上下文的状态快照（训练输入）
///
/// 纯数据容器：context 向量 + 时间戳 + 可选元数据。维度随接缝变化
/// （如 S9 路由 6 维上下文：任务复杂度/预算水位/延迟敏感度/缓存命中/风险/bias）。
///
/// # f32 约束
///
/// context 为 f32 向量，故仅 `PartialEq`（不 derive `Eq`/`Hash`——
/// 浮点比较红线，见项目记忆 f32 陷阱）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RLState {
    /// 上下文向量（维度随接缝变化，如 S1 密度 1 维 / S9 路由 6 维）
    context: Vec<f32>,
    /// 任务阶段（可选，S8 Mem-π / S9 路由使用）
    task_phase: Option<MemoryTaskPhase>,
    /// 预算水位 0.0-1.0（可选，S1 密度 / S5 Parliament 使用）
    budget_watermark: Option<f32>,
    /// 时间戳（毫秒，经验排序与衰减用）
    timestamp_ms: u64,
}

impl RLState {
    /// 创建状态快照（context + 时间戳，可选字段留空）
    pub fn new(context: Vec<f32>, timestamp_ms: u64) -> Self {
        Self {
            context,
            task_phase: None,
            budget_watermark: None,
            timestamp_ms,
        }
    }

    /// 返回上下文向量
    pub fn context(&self) -> &[f32] {
        &self.context
    }

    /// 返回时间戳（毫秒）
    pub const fn timestamp_ms(&self) -> u64 {
        self.timestamp_ms
    }

    /// 返回任务阶段（若有）
    pub const fn task_phase(&self) -> Option<&MemoryTaskPhase> {
        self.task_phase.as_ref()
    }

    /// 返回预算水位（若有）
    pub const fn budget_watermark(&self) -> Option<f32> {
        self.budget_watermark
    }

    /// 设置任务阶段（builder 模式）
    pub fn with_task_phase(mut self, phase: MemoryTaskPhase) -> Self {
        self.task_phase = Some(phase);
        self
    }

    /// 设置预算水位（builder 模式）
    pub fn with_budget_watermark(mut self, watermark: f32) -> Self {
        self.budget_watermark = Some(watermark);
        self
    }
}

// ============================================================
// RLExperience — 经验四元组
// ============================================================

/// RL 经验 — (state, action, reward, next_state) 四元组 + 元数据
///
/// 回放池（cmt-tiering rl_replay_pool / omega-learner replay_pool）的
/// 统一入池类型；`seam` 字段支持按接缝分层采样（Hot/Warm/Cold/Ice）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RLExperience {
    /// 动作前状态
    pub state: RLState,
    /// 执行的动作（接缝封闭枚举）
    pub action: RLAction,
    /// 奖励信号（f32，R2 冻结面外仅作观测）
    pub reward: f32,
    /// 动作后状态
    pub next_state: RLState,
    /// 是否终止（episode 边界）
    pub done: bool,
    /// 接缝来源（审计 / 回放池分层）
    pub seam: SeamId,
}

impl RLExperience {
    /// 创建经验四元组（done=false，seam 由动作推导）
    pub fn new(state: RLState, action: RLAction, reward: f32, next_state: RLState) -> Self {
        let seam = action
            .seam_id()
            .expect("接缝动作必须映射到 SeamId（Route/Custom 经验请显式构造 seam 字段）");
        Self {
            state,
            action,
            reward,
            next_state,
            done: false,
            seam,
        }
    }
}

// ============================================================
// 测试（TDD：先失败测试后实现）
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// RLAction seam 映射完整性：S1-S8 全覆盖
    #[test]
    fn seam_id_mapping_covers_s1_to_s8() {
        assert_eq!(
            RLAction::Density(DensityTier::default()).seam_id(),
            Some(SeamId::S1Density)
        );
        assert_eq!(
            RLAction::Memory(MemoryStrategy::StandardTopK).seam_id(),
            Some(SeamId::S2Memory)
        );
        assert_eq!(
            RLAction::Prefetch(PrefetchStrategy::default()).seam_id(),
            Some(SeamId::S3Prefetch)
        );
        assert_eq!(
            RLAction::Selector(crate::policy::SelectorPolicy::fallback()).seam_id(),
            Some(SeamId::S4Selector)
        );
        assert_eq!(
            RLAction::Parliament(ActivationStrategy::default()).seam_id(),
            Some(SeamId::S5Parliament)
        );
        assert_eq!(
            RLAction::Decay(DecayProfile::default()).seam_id(),
            Some(SeamId::S6Decay)
        );
        assert_eq!(
            RLAction::RecallQuota(RecallQuota::K10).seam_id(),
            Some(SeamId::S7RecallQuota)
        );
        assert_eq!(
            RLAction::MemPi(MemPiAction::Abstain).seam_id(),
            Some(SeamId::S8MemPi)
        );
    }

    /// RLState builder 链式设置
    #[test]
    fn rl_state_builder_chain() {
        let state = RLState::new(vec![0.1, 0.2], 42)
            .with_task_phase(MemoryTaskPhase::LongRun)
            .with_budget_watermark(0.8);
        assert_eq!(state.task_phase(), Some(&MemoryTaskPhase::LongRun));
        assert_eq!(state.budget_watermark(), Some(0.8));
        assert_eq!(state.timestamp_ms(), 42);
    }
}
