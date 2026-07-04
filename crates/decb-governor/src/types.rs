//! DECB 核心类型 — 预算档位、系数、消耗与统计
//!
//! 对应架构层:L8 Parliament
//! 对应创新点:DECB(Dual-tier Cognitive Budget)
//!
//! # 设计决策(WHY)
//! - `BudgetTier` 为 enum:三档(High/Low/Degraded)语义清晰,匹配 §6 架构红线的
//!   "禁止功能标志,用能力场自然进化替代"——档位是能力场的离散投影,非功能开关
//! - `BudgetCoefficient` 用 newtype:类型安全,构造时 clamp 到 `[0,1]`,防止外部
//!   传入越界值导致档位判定异常(§4.3 newtype 模式)
//! - `QuestBudgetInput` 为值对象:DECB 是 L8 层,不能向上依赖 L9 Quest Engine
//!   (§2.2 依赖铁律),通过此结构传入 Quest 信息,实现事件解耦

use serde::{Deserialize, Serialize};

use crate::error::DecbError;

// ============================================================
// 预算档位 — 三档枚举
// ============================================================

/// 预算档位 — DECB 双档 + 降级模式
///
/// - `HighTier`:高预算档,复杂/紧急 Quest 可获得更多资源
/// - `LowTier`:低预算档,常规 Quest 的默认档位
/// - `Degraded`:降级模式,预算接近耗尽时强制降级,拒绝新 Quest
///
/// WHY Copy + PartialEq:档位频繁参与比较与传递,Copy 避免克隆开销,
/// PartialEq 支持档位切换前后的相等性判断。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BudgetTier {
    /// 高预算档 — 复杂/紧急 Quest,资源充足
    HighTier,
    /// 低预算档 — 常规 Quest,资源受限
    LowTier,
    /// 降级模式 — 预算接近耗尽,拒绝新 Quest
    Degraded,
}

impl BudgetTier {
    /// 返回档位的人类可读名称
    pub fn as_str(&self) -> &'static str {
        match self {
            BudgetTier::HighTier => "high_tier",
            BudgetTier::LowTier => "low_tier",
            BudgetTier::Degraded => "degraded",
        }
    }
}

impl std::fmt::Display for BudgetTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ============================================================
// 预算系数 — newtype 模式,类型安全
// ============================================================

/// 预算系数 — 连续可调的 [0.0, 1.0] 标量
///
/// 包装 f32,构造时自动 clamp 到 [0.0, 1.0],防止外部传入越界值。
/// WHY newtype:类型安全,防止 BudgetCoefficient 与其他 f32(如门控值)混用;
/// clamp 集中在构造函数,避免各处重复 clamp 逻辑(§4.3 newtype 模式)。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct BudgetCoefficient(f32);

impl BudgetCoefficient {
    /// 创建预算系数,自动 clamp 到 [0.0, 1.0]
    ///
    /// WHY clamp 而非报错:预算系数是连续可调的,外部计算的中间值可能瞬时
    /// 越界(如浮点误差),clamp 保证最终值合法,避免调用方处处校验。
    /// NaN 视为 0.0(最保守的降级处理)。
    pub fn new(value: f32) -> Self {
        // WHY NaN 处理:NaN 参与比较结果均为 false,会导致档位判定异常,
        // 统一映射为 0.0(Degraded),保证系统安全降级
        let clamped = if value.is_nan() {
            0.0
        } else {
            value.clamp(0.0, 1.0)
        };
        Self(clamped)
    }

    /// 严格创建预算系数 — 值必须在 [0.0, 1.0] 区间,否则返回错误
    ///
    /// WHY:用于需要严格校验的场景(如配置反序列化),区分"自动 clamp"
    /// 与"拒绝越界"两种语义
    pub fn try_new(value: f32) -> Result<Self, DecbError> {
        if value.is_nan() || !(0.0..=1.0).contains(&value) {
            return Err(DecbError::InvalidCoefficient {
                value,
                reason: format!("expected [0.0, 1.0], got {value}"),
            });
        }
        Ok(Self(value))
    }

    /// 返回内部 f32 值
    pub fn value(&self) -> f32 {
        self.0
    }
}

impl Default for BudgetCoefficient {
    fn default() -> Self {
        // 默认系数 1.0:满预算,对应 HighTier(假设 base_budget 足够)
        Self(1.0)
    }
}

impl From<f32> for BudgetCoefficient {
    /// 从 f32 构造,自动 clamp(便捷构造,等价于 `new`)
    fn from(value: f32) -> Self {
        Self::new(value)
    }
}

// ============================================================
// 预算消耗 — 单次 Quest 执行的资源消耗
// ============================================================

/// 预算消耗 — 单次 Quest 执行的资源消耗记录
///
/// DECB 通过 `record_consumption` 累加消耗,计算总成本并检测溢出。
/// `total_cost` 为预计算的总成本(美分),若为 0 则由 DECB 按配置单价计算。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BudgetConsumption {
    /// Token 消耗数量
    pub token_count: u64,
    /// 工具调用次数
    pub tool_call_count: u64,
    /// 上下文加载次数(如 Wiki 检索、记忆召回)
    pub context_load_count: u64,
    /// 预计算总成本(美分);为 0 时由 DECB 按单价计算
    pub total_cost: f64,
}

impl BudgetConsumption {
    /// 创建零消耗(用于测试与初始化)
    pub fn zero() -> Self {
        Self {
            token_count: 0,
            tool_call_count: 0,
            context_load_count: 0,
            total_cost: 0.0,
        }
    }

    /// 创建新的消耗记录
    pub fn new(token_count: u64, tool_call_count: u64, context_load_count: u64) -> Self {
        Self {
            token_count,
            tool_call_count,
            context_load_count,
            total_cost: 0.0,
        }
    }
}

impl Default for BudgetConsumption {
    fn default() -> Self {
        Self::zero()
    }
}

// ============================================================
// 预算统计 — 当前预算状态快照
// ============================================================

/// 预算统计 — DECB 当前预算状态的只读快照
///
/// 用于监控、日志与事件发布(Task 37 集成 event-bus 的 `BudgetStatsReported`)。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BudgetStats {
    /// 累计消耗(美分)
    pub total_consumption: f64,
    /// 剩余预算(美分)
    pub remaining_budget: f64,
    /// 预算利用率 [0.0, 1.0]
    pub utilization_rate: f32,
    /// 当前档位
    pub current_tier: BudgetTier,
    /// 当前预算系数 [0.0, 1.0]
    pub current_coefficient: f32,
}

impl BudgetStats {
    /// 创建零统计(用于初始化)
    pub fn zero() -> Self {
        Self {
            total_consumption: 0.0,
            remaining_budget: 0.0,
            utilization_rate: 0.0,
            current_tier: BudgetTier::HighTier,
            current_coefficient: 1.0,
        }
    }
}

impl Default for BudgetStats {
    fn default() -> Self {
        Self::zero()
    }
}

// ============================================================
// Quest 预算输入 — 从 L9 Quest Engine 传入的值对象
// ============================================================

/// Quest 预算输入 — 从 L9 Quest Engine 传入的 Quest 元信息
///
/// WHY 值对象:DECB 是 L8 层,不能向上依赖 L9 Quest Engine(§2.2 依赖铁律)。
/// 通过此结构传入 Quest 的预算相关元信息,实现层间解耦。
/// `deadline` 为 `Option`:无 deadline 的 Quest 使用标准 urgency_factor。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct QuestBudgetInput {
    /// Quest 唯一标识
    pub quest_id: String,
    /// 任务数量(用于估算复杂度)
    pub task_count: usize,
    /// 依赖深度(DAG 最大深度,用于估算复杂度)
    pub dependency_depth: usize,
    /// 截止时间(UTC);None 表示无 deadline
    pub deadline: Option<chrono::DateTime<chrono::Utc>>,
    /// Quest 描述长度(用于估算上下文加载成本)
    pub description_length: usize,
}

impl QuestBudgetInput {
    /// 创建新的 Quest 预算输入
    pub fn new(
        quest_id: impl Into<String>,
        task_count: usize,
        dependency_depth: usize,
        deadline: Option<chrono::DateTime<chrono::Utc>>,
        description_length: usize,
    ) -> Self {
        Self {
            quest_id: quest_id.into(),
            task_count,
            dependency_depth,
            deadline,
            description_length,
        }
    }

    /// 创建简单 Quest 输入(无 deadline,单任务,无依赖)
    pub fn simple(quest_id: impl Into<String>) -> Self {
        Self::new(quest_id, 1, 0, None, 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============================================================
    // BudgetTier 测试
    // ============================================================

    #[test]
    fn test_budget_tier_as_str() {
        assert_eq!(BudgetTier::HighTier.as_str(), "high_tier");
        assert_eq!(BudgetTier::LowTier.as_str(), "low_tier");
        assert_eq!(BudgetTier::Degraded.as_str(), "degraded");
    }

    #[test]
    fn test_budget_tier_display() {
        assert_eq!(BudgetTier::HighTier.to_string(), "high_tier");
        assert_eq!(BudgetTier::LowTier.to_string(), "low_tier");
        assert_eq!(BudgetTier::Degraded.to_string(), "degraded");
    }

    #[test]
    fn test_budget_tier_equality() {
        assert_eq!(BudgetTier::HighTier, BudgetTier::HighTier);
        assert_ne!(BudgetTier::HighTier, BudgetTier::LowTier);
    }

    #[test]
    fn test_budget_tier_copy() {
        let tier = BudgetTier::HighTier;
        let tier_copy = tier; // Copy 语义,无需 clone
        assert_eq!(tier, tier_copy);
    }

    #[test]
    fn test_budget_tier_serde_roundtrip() {
        let tier = BudgetTier::LowTier;
        let json = serde_json::to_string(&tier).unwrap();
        let restored: BudgetTier = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, tier);
    }

    // ============================================================
    // BudgetCoefficient 测试
    // ============================================================

    #[test]
    fn test_coefficient_valid() {
        let coef = BudgetCoefficient::new(0.5);
        assert!((coef.value() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn test_coefficient_clamp_high() {
        let coef = BudgetCoefficient::new(1.5);
        assert!((coef.value() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_coefficient_clamp_low() {
        let coef = BudgetCoefficient::new(-0.5);
        assert!((coef.value() - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_coefficient_nan_becomes_zero() {
        // WHY NaN 映射为 0.0:保证系统安全降级
        let coef = BudgetCoefficient::new(f32::NAN);
        assert!((coef.value() - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_coefficient_boundary() {
        assert!((BudgetCoefficient::new(0.0).value() - 0.0).abs() < 1e-6);
        assert!((BudgetCoefficient::new(1.0).value() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_coefficient_try_new_valid() {
        assert!(BudgetCoefficient::try_new(0.5).is_ok());
        assert!(BudgetCoefficient::try_new(0.0).is_ok());
        assert!(BudgetCoefficient::try_new(1.0).is_ok());
    }

    #[test]
    fn test_coefficient_try_new_invalid() {
        assert!(BudgetCoefficient::try_new(-0.1).is_err());
        assert!(BudgetCoefficient::try_new(1.1).is_err());
        assert!(BudgetCoefficient::try_new(f32::NAN).is_err());
    }

    #[test]
    fn test_coefficient_default() {
        let coef = BudgetCoefficient::default();
        assert!((coef.value() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_coefficient_from_f32() {
        let coef = BudgetCoefficient::from(0.7);
        assert!((coef.value() - 0.7).abs() < 1e-6);
    }

    #[test]
    fn test_coefficient_serde_roundtrip() {
        let coef = BudgetCoefficient::new(0.65);
        let json = serde_json::to_string(&coef).unwrap();
        let restored: BudgetCoefficient = serde_json::from_str(&json).unwrap();
        assert!((restored.value() - coef.value()).abs() < 1e-6);
    }

    // ============================================================
    // BudgetConsumption 测试
    // ============================================================

    #[test]
    fn test_consumption_zero() {
        let c = BudgetConsumption::zero();
        assert_eq!(c.token_count, 0);
        assert_eq!(c.tool_call_count, 0);
        assert_eq!(c.context_load_count, 0);
        assert!((c.total_cost - 0.0).abs() < 1e-9);
    }

    #[test]
    fn test_consumption_new() {
        let c = BudgetConsumption::new(100, 5, 3);
        assert_eq!(c.token_count, 100);
        assert_eq!(c.tool_call_count, 5);
        assert_eq!(c.context_load_count, 3);
        assert!((c.total_cost - 0.0).abs() < 1e-9);
    }

    #[test]
    fn test_consumption_serde_roundtrip() {
        let c = BudgetConsumption::new(100, 5, 3);
        let json = serde_json::to_string(&c).unwrap();
        let restored: BudgetConsumption = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, c);
    }

    // ============================================================
    // BudgetStats 测试
    // ============================================================

    #[test]
    fn test_stats_zero() {
        let s = BudgetStats::zero();
        assert!((s.total_consumption - 0.0).abs() < 1e-9);
        assert!((s.remaining_budget - 0.0).abs() < 1e-9);
        assert!((s.utilization_rate - 0.0).abs() < 1e-6);
        assert_eq!(s.current_tier, BudgetTier::HighTier);
        assert!((s.current_coefficient - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_stats_serde_roundtrip() {
        let s = BudgetStats {
            total_consumption: 500.0,
            remaining_budget: 500.0,
            utilization_rate: 0.5,
            current_tier: BudgetTier::LowTier,
            current_coefficient: 0.4,
        };
        let json = serde_json::to_string(&s).unwrap();
        let restored: BudgetStats = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, s);
    }

    // ============================================================
    // QuestBudgetInput 测试
    // ============================================================

    #[test]
    fn test_quest_input_new() {
        let deadline = chrono::Utc::now();
        let input = QuestBudgetInput::new("quest-1", 5, 2, Some(deadline), 100);
        assert_eq!(input.quest_id, "quest-1");
        assert_eq!(input.task_count, 5);
        assert_eq!(input.dependency_depth, 2);
        assert_eq!(input.deadline, Some(deadline));
        assert_eq!(input.description_length, 100);
    }

    #[test]
    fn test_quest_input_simple() {
        let input = QuestBudgetInput::simple("quest-simple");
        assert_eq!(input.quest_id, "quest-simple");
        assert_eq!(input.task_count, 1);
        assert_eq!(input.dependency_depth, 0);
        assert!(input.deadline.is_none());
        assert_eq!(input.description_length, 0);
    }

    #[test]
    fn test_quest_input_serde_roundtrip() {
        let input = QuestBudgetInput::new("quest-1", 5, 2, None, 100);
        let json = serde_json::to_string(&input).unwrap();
        let restored: QuestBudgetInput = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, input);
    }
}
