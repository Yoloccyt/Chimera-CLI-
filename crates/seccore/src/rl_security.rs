//! 可学习安全策略（Milestone D-2b，设计 §8.1 目标形态）
//!
//! 安全事件 → RL 奖励 + 审计频率/沙箱严格度微调。
//!
//! R2 冻结（ADR-042）+ ADR-049 降级裁决 + 安全绝对红线（§8.3）：
//! - RL 微调仅作用于**审计频率**（非拦截决策），因子 clamp [0.5, 2.0]——
//!   审计不会关闭（下限）也不会失控（上限）
//! - **沙箱严格度不可降底线**：`sandbox_strictness` 仅由基础策略（规则式）
//!   决策，RL 微调不参与（Maximum/High 不可因微调降低，UNLEARNABLE_SECURITY_RULES 优先）
//! - 奖励映射复用 `nexus_contracts::reward::security_event_reward`（§8.1 五档，
//!   仅观测信号——`RewardSignal::security_observation`，不触 R1/R2 数据面）
//!
//! 依赖方向：L4 seccore 内部模块（nexus-contracts 既有依赖，0 新增 crate）。

use nexus_contracts::reward::SecuritySeverity;
use std::collections::HashMap;

/// 微调因子下限（审计间隔最短 = base × 0.5——审计不会被关闭）
pub const DEFAULT_ADJUSTMENT_LOWER_BOUND: f32 = 0.5;
/// 微调因子上限（审计间隔最长 = base × 2.0——审计不会失控）
pub const DEFAULT_ADJUSTMENT_UPPER_BOUND: f32 = 2.0;

/// 沙箱严格度（设计 §8.1 `SandboxLevel`）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SandboxLevel {
    /// 最大严格度（gVisor 全隔离）——不可降
    Maximum,
    /// 高严格度——不可降
    High,
    /// 中严格度（seccomp 过滤）
    Medium,
    /// 低严格度（进程级，Windows 降级）
    Low,
}

impl SandboxLevel {
    /// 严格度排序值（越大越严格；排序测试用）
    pub fn rank(self) -> u8 {
        match self {
            SandboxLevel::Maximum => 4,
            SandboxLevel::High => 3,
            SandboxLevel::Medium => 2,
            SandboxLevel::Low => 1,
        }
    }

    /// 层名（日志/审计）
    pub fn as_str(self) -> &'static str {
        match self {
            SandboxLevel::Maximum => "Maximum",
            SandboxLevel::High => "High",
            SandboxLevel::Medium => "Medium",
            SandboxLevel::Low => "Low",
        }
    }
}

/// 规则式基础安全策略（专家先验，RL 微调域之外）
#[derive(Debug, Clone, Copy, Default)]
pub struct BaseSecurityPolicy {}

impl BaseSecurityPolicy {
    /// 审计间隔（毫秒）：风险越高间隔越小（审计越频繁）
    ///
    /// 规则式映射：risk ∈ [0,1] → 间隔 1000ms（低风险）线性降至 100ms（高风险）。
    pub fn audit_frequency_ms(&self, risk_level: f32) -> u64 {
        let risk = risk_level.clamp(0.0, 1.0);
        (1000.0 * (1.0 - risk * 0.9)) as u64
    }

    /// 沙箱严格度阈值映射（规则式）：
    /// risk ≥ 0.8 → Maximum；≥ 0.5 → High；≥ 0.2 → Medium；否则 Low
    pub fn sandbox_strictness(&self, task_risk: f32) -> SandboxLevel {
        match task_risk {
            r if r >= 0.8 => SandboxLevel::Maximum,
            r if r >= 0.5 => SandboxLevel::High,
            r if r >= 0.2 => SandboxLevel::Medium,
            _ => SandboxLevel::Low,
        }
    }
}

/// 可学习安全策略（RL 微调域）
///
/// `adjustments`: task_type → 审计频率微调因子（如 +0.5 = 间隔 ×1.5）。
/// 微调注入源（外部决策/规则）经 `set_adjustment` 更新——R2 冻结下由
/// 规则式来源驱动；解冻后可由训练面输出，接口不变。
#[derive(Debug, Clone)]
pub struct RLSecurityPolicy {
    base_policy: BaseSecurityPolicy,
    adjustments: HashMap<String, f32>,
}

impl RLSecurityPolicy {
    /// 构造：基础策略 + 初始微调表
    pub fn new(base_policy: BaseSecurityPolicy, adjustments: HashMap<String, f32>) -> Self {
        Self {
            base_policy,
            adjustments,
        }
    }

    /// 审计频率（毫秒）：base × clamp(1 + 微调, 0.5, 2.0)
    ///
    /// 仅影响审计节奏，不影响任何拦截/放行决策（安全红线 §8.3）。
    pub fn audit_frequency_ms(&self, risk_level: f32, task_type: &str) -> u64 {
        let base = self.base_policy.audit_frequency_ms(risk_level);
        let factor = (1.0 + self.adjustments.get(task_type).copied().unwrap_or(0.0)).clamp(
            DEFAULT_ADJUSTMENT_LOWER_BOUND,
            DEFAULT_ADJUSTMENT_UPPER_BOUND,
        );
        (base as f32 * factor) as u64
    }

    /// 沙箱严格度：仅由基础策略决策——RL 微调**不可降低底线**（§8.3）
    pub fn sandbox_strictness(&self, task_risk: f32) -> SandboxLevel {
        self.base_policy.sandbox_strictness(task_risk)
    }

    /// 注入/更新某任务类型的审计频率微调因子（R2：规则式来源驱动）
    pub fn set_adjustment(&mut self, task_type: &str, adjustment: f32) {
        self.adjustments.insert(task_type.to_string(), adjustment);
    }
}

/// 安全严重度 → 奖励（§8.1 五档；转发 L0 契约，保证单一事实源）
///
/// WHY 转发而非重实现：`nexus_contracts::reward::security_event_reward` 为
/// 唯一权威映射（C-1 RewardSpec），seccore 侧仅作消费方。
pub fn security_event_to_reward(severity: SecuritySeverity) -> f32 {
    nexus_contracts::reward::security_event_reward(severity)
}
