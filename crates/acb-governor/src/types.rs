//! ACB 核心类型 — 预算级别、状态、分配与请求
//!
//! 对应架构层:L8 Parliament
//! 对应创新点:ACB(Adaptive Cognitive Budget)
//!
//! # 设计决策(WHY)
//! - `BudgetTier` 为 enum:四级(L0/L1/L2/L3)语义清晰,级别递增预算上限递增,
//!   匹配 §6 架构红线的"禁止功能标志,用能力场自然进化替代"——级别是能力场的
//!   离散投影,非功能开关
//! - `BudgetStatus` 为快照结构体:用于监控、日志与事件发布(Ω-Event 定律)
//! - `BudgetRequest` 为值对象:ACB 是 L8 层,不能向上依赖 L9 Quest Engine
//!   (§2.2 依赖铁律),通过此结构传入请求信息,实现层间解耦

use serde::{Deserialize, Serialize};

use crate::error::AcbError;

// ============================================================
// 预算级别 — L0-L3 四级枚举
// ============================================================

/// 预算级别 — ACB 四级自适应认知预算
///
/// - `L0`:降级模式,预算接近耗尽,拒绝新请求(最低)
/// - `L1`:基础级别,资源受限,仅允许低消耗请求
/// - `L2`:标准级别,常规 Quest 的默认级别
/// - `L3`:充足级别,复杂/紧急 Quest 可获得更多资源(最高)
///
/// WHY Copy + PartialEq:级别频繁参与比较与传递,Copy 避免克隆开销,
/// PartialEq 支持级别切换前后的相等性判断。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BudgetTier {
    /// L0 降级模式 — 预算接近耗尽,拒绝新请求
    L0,
    /// L1 基础级别 — 资源受限,仅允许低消耗请求
    L1,
    /// L2 标准级别 — 常规 Quest 的默认级别
    L2,
    /// L3 充足级别 — 复杂/紧急 Quest,资源充足
    L3,
}

impl BudgetTier {
    /// 返回级别的数值表示(0-3,用于配置与序列化)
    pub fn as_level(&self) -> u8 {
        match self {
            BudgetTier::L0 => 0,
            BudgetTier::L1 => 1,
            BudgetTier::L2 => 2,
            BudgetTier::L3 => 3,
        }
    }

    /// 返回级别的人类可读名称
    pub fn as_str(&self) -> &'static str {
        match self {
            BudgetTier::L0 => "L0_degraded",
            BudgetTier::L1 => "L1_basic",
            BudgetTier::L2 => "L2_standard",
            BudgetTier::L3 => "L3_abundant",
        }
    }

    /// 从数值构造级别,值必须在 [0, 3] 区间
    ///
    /// WHY:用于配置反序列化与外部传入值的校验,非法值返回错误而非 panic
    pub fn from_level(level: u8) -> Result<Self, AcbError> {
        match level {
            0 => Ok(BudgetTier::L0),
            1 => Ok(BudgetTier::L1),
            2 => Ok(BudgetTier::L2),
            3 => Ok(BudgetTier::L3),
            other => Err(AcbError::InvalidTier {
                tier: other,
                reason: format!("level must be in [0, 3], got {other}"),
            }),
        }
    }

    /// 返回更低一级(降级),L0 已是最低则返回 L0
    pub fn degrade(&self) -> BudgetTier {
        match self {
            BudgetTier::L0 => BudgetTier::L0,
            BudgetTier::L1 => BudgetTier::L0,
            BudgetTier::L2 => BudgetTier::L1,
            BudgetTier::L3 => BudgetTier::L2,
        }
    }

    /// 返回更高一级(升级),L3 已是最高则返回 L3
    pub fn upgrade(&self) -> BudgetTier {
        match self {
            BudgetTier::L0 => BudgetTier::L1,
            BudgetTier::L1 => BudgetTier::L2,
            BudgetTier::L2 => BudgetTier::L3,
            BudgetTier::L3 => BudgetTier::L3,
        }
    }
}

impl std::fmt::Display for BudgetTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ============================================================
// 预算分配 — 单个级别的预算配置
// ============================================================

/// 预算分配 — 单个级别的预算上限配置
///
/// WHY 独立结构体:每级预算上限独立配置,便于精细化调控。
/// `token_limit` 为该级别单次请求的最大 Token 消耗。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BudgetAllocation {
    /// 级别
    pub tier: BudgetTier,
    /// 该级别单次请求的 Token 上限
    pub token_limit: u64,
}

impl BudgetAllocation {
    /// 创建新的预算分配
    pub fn new(tier: BudgetTier, token_limit: u64) -> Self {
        Self { tier, token_limit }
    }
}

// ============================================================
// 预算状态 — 当前预算快照
// ============================================================

/// 预算状态 — ACB 当前预算状态的只读快照
///
/// 用于监控、日志与事件发布(Ω-Event 定律)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BudgetStatus {
    /// 当前级别
    pub current_tier: BudgetTier,
    /// 累计消耗(Token)
    pub total_consumption: u64,
    /// 剩余预算(Token)
    pub remaining_budget: u64,
    /// 预算利用率 [0.0, 1.0]
    pub utilization_rate: f32,
}

impl BudgetStatus {
    /// 创建零状态(用于初始化,默认 L3 充足级别)
    pub fn zero() -> Self {
        Self {
            current_tier: BudgetTier::L3,
            total_consumption: 0,
            remaining_budget: 0,
            utilization_rate: 0.0,
        }
    }
}

impl Default for BudgetStatus {
    fn default() -> Self {
        Self::zero()
    }
}

// ============================================================
// 预算请求 — 从 L9 Quest Engine 传入的值对象
// ============================================================

/// 预算请求 — 从 L9 Quest Engine 传入的请求信息
///
/// WHY 值对象:ACB 是 L8 层,不能向上依赖 L9 Quest Engine(§2.2 依赖铁律)。
/// 通过此结构传入请求的预算相关信息,实现层间解耦。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BudgetRequest {
    /// Quest 唯一标识
    pub quest_id: String,
    /// 请求的 Token 消耗量
    pub requested_tokens: u64,
}

impl BudgetRequest {
    /// 创建新的预算请求
    pub fn new(quest_id: impl Into<String>, requested_tokens: u64) -> Self {
        Self {
            quest_id: quest_id.into(),
            requested_tokens,
        }
    }
}

// ============================================================
// 级别切换结果 — 切换操作的返回值
// ============================================================

/// 级别切换结果 — 记录切换前后的级别
///
/// WHY 独立结构体:便于调用方与事件发布携带切换上下文,
/// `switched` 为 false 表示因滞后或相同级别未实际切换。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TierSwitchResult {
    /// 切换前级别
    pub from_tier: BudgetTier,
    /// 切换后级别
    pub to_tier: BudgetTier,
    /// 是否实际发生切换(false = 相同级别或被滞后机制阻止)
    pub switched: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================
    // BudgetTier 测试
    // ============================================================

    #[test]
    fn test_budget_tier_as_level() {
        assert_eq!(BudgetTier::L0.as_level(), 0);
        assert_eq!(BudgetTier::L1.as_level(), 1);
        assert_eq!(BudgetTier::L2.as_level(), 2);
        assert_eq!(BudgetTier::L3.as_level(), 3);
    }

    #[test]
    fn test_budget_tier_as_str() {
        assert_eq!(BudgetTier::L0.as_str(), "L0_degraded");
        assert_eq!(BudgetTier::L1.as_str(), "L1_basic");
        assert_eq!(BudgetTier::L2.as_str(), "L2_standard");
        assert_eq!(BudgetTier::L3.as_str(), "L3_abundant");
    }

    #[test]
    fn test_budget_tier_display() {
        assert_eq!(BudgetTier::L0.to_string(), "L0_degraded");
        assert_eq!(BudgetTier::L3.to_string(), "L3_abundant");
    }

    #[test]
    fn test_budget_tier_from_level_valid() {
        assert_eq!(BudgetTier::from_level(0).unwrap(), BudgetTier::L0);
        assert_eq!(BudgetTier::from_level(1).unwrap(), BudgetTier::L1);
        assert_eq!(BudgetTier::from_level(2).unwrap(), BudgetTier::L2);
        assert_eq!(BudgetTier::from_level(3).unwrap(), BudgetTier::L3);
    }

    #[test]
    fn test_budget_tier_from_level_invalid() {
        assert!(BudgetTier::from_level(4).is_err());
        assert!(BudgetTier::from_level(255).is_err());
    }

    #[test]
    fn test_budget_tier_degrade() {
        assert_eq!(BudgetTier::L3.degrade(), BudgetTier::L2);
        assert_eq!(BudgetTier::L2.degrade(), BudgetTier::L1);
        assert_eq!(BudgetTier::L1.degrade(), BudgetTier::L0);
        // L0 已是最低,degrade 返回自身
        assert_eq!(BudgetTier::L0.degrade(), BudgetTier::L0);
    }

    #[test]
    fn test_budget_tier_upgrade() {
        assert_eq!(BudgetTier::L0.upgrade(), BudgetTier::L1);
        assert_eq!(BudgetTier::L1.upgrade(), BudgetTier::L2);
        assert_eq!(BudgetTier::L2.upgrade(), BudgetTier::L3);
        // L3 已是最高,upgrade 返回自身
        assert_eq!(BudgetTier::L3.upgrade(), BudgetTier::L3);
    }

    #[test]
    fn test_budget_tier_equality() {
        assert_eq!(BudgetTier::L2, BudgetTier::L2);
        assert_ne!(BudgetTier::L1, BudgetTier::L2);
    }

    #[test]
    fn test_budget_tier_serde_roundtrip() {
        let tier = BudgetTier::L2;
        let json = serde_json::to_string(&tier).unwrap();
        let restored: BudgetTier = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, tier);
    }

    // ============================================================
    // BudgetAllocation 测试
    // ============================================================

    #[test]
    fn test_allocation_new() {
        let alloc = BudgetAllocation::new(BudgetTier::L2, 10000);
        assert_eq!(alloc.tier, BudgetTier::L2);
        assert_eq!(alloc.token_limit, 10000);
    }

    #[test]
    fn test_allocation_serde_roundtrip() {
        let alloc = BudgetAllocation::new(BudgetTier::L3, 50000);
        let json = serde_json::to_string(&alloc).unwrap();
        let restored: BudgetAllocation = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, alloc);
    }

    // ============================================================
    // BudgetStatus 测试
    // ============================================================

    #[test]
    fn test_status_zero() {
        let s = BudgetStatus::zero();
        assert_eq!(s.current_tier, BudgetTier::L3);
        assert_eq!(s.total_consumption, 0);
        assert_eq!(s.remaining_budget, 0);
        assert!((s.utilization_rate - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_status_serde_roundtrip() {
        let s = BudgetStatus {
            current_tier: BudgetTier::L1,
            total_consumption: 5000,
            remaining_budget: 5000,
            utilization_rate: 0.5,
        };
        let json = serde_json::to_string(&s).unwrap();
        let restored: BudgetStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, s);
    }

    // ============================================================
    // BudgetRequest 测试
    // ============================================================

    #[test]
    fn test_request_new() {
        let req = BudgetRequest::new("quest-1", 1000);
        assert_eq!(req.quest_id, "quest-1");
        assert_eq!(req.requested_tokens, 1000);
    }

    #[test]
    fn test_request_serde_roundtrip() {
        let req = BudgetRequest::new("quest-2", 2500);
        let json = serde_json::to_string(&req).unwrap();
        let restored: BudgetRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, req);
    }

    // ============================================================
    // TierSwitchResult 测试
    // ============================================================

    #[test]
    fn test_switch_result_serde_roundtrip() {
        let r = TierSwitchResult {
            from_tier: BudgetTier::L3,
            to_tier: BudgetTier::L2,
            switched: true,
        };
        let json = serde_json::to_string(&r).unwrap();
        let restored: TierSwitchResult = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, r);
    }
}
