//! 奖励函数统一框架契约 — RewardSpec / RewardSignal（Milestone C-1）
//!
//! 对应方案: `CHIMERA_V3_专项优化方案_v2.21基线.md` §5.1 P3 / §6 C-1 / §7.1
//! 对应设计: 根目录设计文档 §17（八维度奖励权重表）
//!
//! # 职责
//!
//! 统一各层奖励：L0 `RewardSpec` 契约（层权重 + 维度组件）+ `RewardSignal`
//! （EventBus 奖励信号流载荷），将 S1-S9 接缝奖励与 §17 八维度权重表 1:1 对齐。
//!
//! # 激活分层（R2 冻结声明，ADR-042）
//!
//! - **R1 数据面**（当前）：奖励信号流可接入（观测/回放池分层采样）
//! - **R2 训练面**（解冻后）：RewardSpec 权重参与训练损失
//! - **L4 安全奖励仅观测**：安全事件 → 奖励映射表（§8.1）在任何情况下
//!   不降低安全底线（UNLEARNABLE_SECURITY_RULES 优先，见 seccore）
//!
//! # 硬约束（ADR-033）
//!
//! 纯类型 + 纯函数（零逻辑零 IO）；f32 字段仅 `PartialEq`（浮点比较红线）。

use serde::{Deserialize, Serialize};

/// 十层架构层标识（奖励权重表 §17 对齐）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum RewardLayer {
    /// L0 Contracts（权重 0.6）
    L0 = 0,
    /// L1 Core（权重 0.3）
    L1 = 1,
    /// L2 Memory（权重 0.7）
    L2 = 2,
    /// L3 Storage（权重 0.5）
    L3 = 3,
    /// L4 Security（权重 0.5，仅观测）
    L4 = 4,
    /// L5 Knowledge（权重 1.2）
    L5 = 5,
    /// L6 Router（权重 0.8）
    L6 = 6,
    /// L7 Execution（权重 0.8）
    L7 = 7,
    /// L8 Parliament（权重 0.9）
    L8 = 8,
    /// L9 Quest（权重 0.9）
    L9 = 9,
    /// L10 Interface（权重 1.0）
    L10 = 10,
}

impl RewardLayer {
    /// 全部层（按枚举值升序，权重表完整性校验用）
    pub const ALL: [Self; 11] = [
        Self::L0,
        Self::L1,
        Self::L2,
        Self::L3,
        Self::L4,
        Self::L5,
        Self::L6,
        Self::L7,
        Self::L8,
        Self::L9,
        Self::L10,
    ];
}

/// 层权重表（设计 §17，1:1 映射）
///
/// L5 1.2x / L10 1.0x / L9·L8 0.9x / L7·L6 0.8x / L2 0.7x / L0 0.6x /
/// L4·L3 0.5x / L1 0.3x。WHY L5 最高：知识复用是长期主义核心信号；
/// L1 最低：基础设施层奖励噪声大，权重压低防过度优化。
pub const LAYER_WEIGHTS: [(RewardLayer, f32); 11] = [
    (RewardLayer::L0, 0.6),
    (RewardLayer::L1, 0.3),
    (RewardLayer::L2, 0.7),
    (RewardLayer::L3, 0.5),
    (RewardLayer::L4, 0.5),
    (RewardLayer::L5, 1.2),
    (RewardLayer::L6, 0.8),
    (RewardLayer::L7, 0.8),
    (RewardLayer::L8, 0.9),
    (RewardLayer::L9, 0.9),
    (RewardLayer::L10, 1.0),
];

/// 查询层权重（§17 表）；缺失层返回 0.0（防御，正常不应发生）
///
/// WHY match 而非迭代比较：const fn 中枚举 `==`（derive PartialEq）非 const 可用，
/// 模式匹配是 const 安全的（E0015 修复，2026-08-08）。
pub const fn reward_layer_weight(layer: RewardLayer) -> f32 {
    match layer {
        RewardLayer::L0 => 0.6,
        RewardLayer::L1 => 0.3,
        RewardLayer::L2 => 0.7,
        RewardLayer::L3 => 0.5,
        RewardLayer::L4 => 0.5,
        RewardLayer::L5 => 1.2,
        RewardLayer::L6 => 0.8,
        RewardLayer::L7 => 0.8,
        RewardLayer::L8 => 0.9,
        RewardLayer::L9 => 0.9,
        RewardLayer::L10 => 1.0,
    }
}

/// 五档安全严重度（L4 奖励映射输入，§8.1）
///
/// WHY 独立枚举：EventSeverity（L0 event_payload）为三档投递语义
/// （Normal/Info/Critical）；本枚举为安全事件严重度分档，两域不混用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum SecuritySeverity {
    /// 信息级（奖励 +0.1）
    Info = 0,
    /// 低危（奖励 -0.5）
    Low = 1,
    /// 中危（奖励 -2.0）
    Medium = 2,
    /// 高危（奖励 -5.0）
    High = 3,
    /// 严重（奖励 -10.0）
    Critical = 4,
}

/// L4 安全事件 → 奖励映射（§8.1，仅观测）
///
/// # 硬约束
/// 映射结果仅作观测信号（`RewardSignal::security_observation`），
/// 任何情况下不降低安全底线（UNLEARNABLE_SECURITY_RULES 优先）。
pub const fn security_event_reward(severity: SecuritySeverity) -> f32 {
    match severity {
        SecuritySeverity::Critical => -10.0,
        SecuritySeverity::High => -5.0,
        SecuritySeverity::Medium => -2.0,
        SecuritySeverity::Low => -0.5,
        SecuritySeverity::Info => 0.1,
    }
}

/// 奖励规格 — 层权重 + 维度组件的可审计契约
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RewardSpec {
    /// 规格 ID（"rs-" 前缀 + 组件名，如 "rs-s9_route"）
    pub spec_id: String,
    /// 归属层（决定权重）
    pub layer: RewardLayer,
    /// 维度组件（如 "s9_route" / "mem_pi" / "process_score" / "skill_reuse"）
    pub component: String,
    /// 组件内缩放（默认 1.0；同一层内多组件差异化时使用）
    pub scale: f32,
}

impl RewardSpec {
    /// 创建规格（scale=1.0）
    pub fn new(
        spec_id: impl Into<String>,
        layer: RewardLayer,
        component: impl Into<String>,
        scale: f32,
    ) -> Self {
        Self {
            spec_id: spec_id.into(),
            layer,
            component: component.into(),
            scale,
        }
    }

    /// 加权奖励：raw × 层权重 × 组件缩放
    ///
    /// # 返回
    /// 加权后的奖励（供 RewardSignal 载荷 / R1 数据面聚合）
    pub fn apply(&self, raw_reward: f32) -> f32 {
        raw_reward * reward_layer_weight(self.layer) * self.scale
    }
}

/// 奖励信号 — EventBus 奖励信号流载荷
///
/// R1 数据面可经 EventBus 传输（回放池分层采样）；R2 训练面解冻后
/// 由训练服务消费（reward 信号 → 损失梯度）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RewardSignal {
    /// 来源规格 ID
    pub spec_id: String,
    /// 原始奖励（接缝观测值）
    pub raw_reward: f32,
    /// 加权奖励（raw × 层权重 × 缩放；L4 观测 = raw 原值）
    pub weighted_reward: f32,
    /// 时间戳（毫秒，经验排序用）
    pub timestamp_ms: u64,
    /// L4 安全观测标记（true = 仅观测不参与训练）
    pub is_security_observation: bool,
}

impl RewardSignal {
    /// 创建普通奖励信号（加权 = raw × 层权重 × 缩放）
    pub fn new(spec: &RewardSpec, raw_reward: f32, timestamp_ms: u64) -> Self {
        Self {
            spec_id: spec.spec_id.clone(),
            raw_reward,
            weighted_reward: spec.apply(raw_reward),
            timestamp_ms,
            is_security_observation: false,
        }
    }

    /// 创建 L4 安全观测信号（§8.1 映射；不乘权重，仅观测语义）
    pub fn security_observation(spec: &RewardSpec, raw_reward: f32, timestamp_ms: u64) -> Self {
        Self {
            spec_id: spec.spec_id.clone(),
            raw_reward,
            weighted_reward: raw_reward,
            timestamp_ms,
            is_security_observation: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weight_table_has_eleven_layers() {
        assert_eq!(LAYER_WEIGHTS.len(), 11);
        assert_eq!(RewardLayer::ALL.len(), 11);
    }

    #[test]
    fn weight_query_roundtrip() {
        for (layer, w) in LAYER_WEIGHTS {
            assert_eq!(reward_layer_weight(layer), w);
        }
    }
}
