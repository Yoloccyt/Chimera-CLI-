//! 预算档位契约 — DECB 三档枚举上提(ADR-054 决策 3,P9-T3)
//!
//! 对应架构层: **L0 Contracts**(从 L8 `decb-governor` 上提,消除 L9→L8 生产违规边)
//! 对应 ADR: **ADR-054 决策 3**(BudgetTier 纯枚举上提 L0,decb-governor 改 re-export)
//!
//! # 核心职责
//!
//! 承载 DECB(Dual-tier Cognitive Budget)预算档位枚举(3 变体:HighTier/LowTier/Degraded)。
//! 原定义于 `decb-governor/src/types.rs`,被 L9 `quest-engine`(TTG/Arbitration)跨层引用,
//! 构成 L9→L8 生产依赖(违反 §2.2 依赖铁律的层间方向约束)。上提到 L0 共享契约层后:
//! - L9 `quest-engine` 直接依赖 L0(`L(N) → L(0)` 恒允许),不再依赖 L8 `decb-governor`
//! - L8 `decb-governor` 保留 re-export,对外 API 零破坏
//!
//! # 设计约束(ADR-033)
//!
//! - **纯类型 + 零逻辑**: 仅枚举定义 + 字符串转换辅助方法,不含业务逻辑
//! - **零 crate 依赖**(serde derive 例外): 与 L0 其余模块一致,仅依赖 serde derive
//!
//! # 语义对齐(WHY)
//!
//! `as_str()` / `Display` 输出与 decb-governor 原实现**逐字一致**:
//! HighTier → "high_tier" / LowTier → "low_tier" / Degraded → "degraded"。
//! 该字符串契约被 EventBus `BudgetAdjusted.new_tier` 事件消费端依赖
//! (quest-engine `parse_decb_tier` 按名解析),任何改动都会破坏运行时兼容,
//! 因此此处为**语义冻结**迁移,仅移动定义位置。

use serde::{Deserialize, Serialize};

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
    ///
    /// 字符串契约被 `BudgetAdjusted.new_tier` 事件消费端依赖(如 quest-engine
    /// `parse_decb_tier` 按名解析),语义已冻结,禁止改动。
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 全部变体清单 — 供遍历式测试复用,避免遗漏新增变体
    const ALL_TIERS: [BudgetTier; 3] = [
        BudgetTier::HighTier,
        BudgetTier::LowTier,
        BudgetTier::Degraded,
    ];

    /// proptest 策略: 全变体空间任意 `BudgetTier`
    ///
    /// WHY 用 `prop::sample::select` 显式覆盖三档,而非为纯枚举实现 `Arbitrary`:
    /// 保持 L0 零逻辑约束,测试专用策略不进入生产 API(ADR-033)。
    fn any_tier() -> impl proptest::strategy::Strategy<Value = BudgetTier> {
        proptest::sample::select(vec![
            BudgetTier::HighTier,
            BudgetTier::LowTier,
            BudgetTier::Degraded,
        ])
    }

    /// 序列化往返: 每个变体 serde_json 序列化 → 反序列化后与原值相等
    #[test]
    fn test_budget_tier_serde_json_roundtrip_all_variants() {
        for tier in ALL_TIERS {
            let json = serde_json::to_string(&tier).unwrap();
            let restored: BudgetTier = serde_json::from_str(&json).unwrap();
            assert_eq!(restored, tier, "变体 {tier:?} 序列化往返失败");
        }
    }

    /// as_str 语义: 三档的人类可读名称逐字对齐 decb-governor 原契约
    #[test]
    fn test_budget_tier_as_str_contract() {
        assert_eq!(BudgetTier::HighTier.as_str(), "high_tier");
        assert_eq!(BudgetTier::LowTier.as_str(), "low_tier");
        assert_eq!(BudgetTier::Degraded.as_str(), "degraded");
    }

    /// as_str / Display 一致性: `tier.to_string() == tier.as_str()`
    #[test]
    fn test_budget_tier_display_matches_as_str() {
        for tier in ALL_TIERS {
            assert_eq!(
                tier.to_string(),
                tier.as_str(),
                "变体 {tier:?} Display 与 as_str 不一致"
            );
        }
    }

    // proptest 属性: 任意档位 `as_str()` 输出 ∈ {high_tier, low_tier, degraded}
    // 且 `to_string() == as_str()`(覆盖全变体空间的不变量)
    //
    // WHY 用普通注释而非 doc comment:proptest! 宏会为 #[test] fn 生成包装,
    // 宏外部的 doc comment 无法附着到生成项,会触发 unused_doc_comments 警告。
    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig::with_cases(256))]

        /// 全变体空间不变量: as_str 输出合法 + Display 与 as_str 一致
        #[test]
        fn prop_budget_tier_ascii_invariant(tier in any_tier()) {
            let s = tier.as_str();
            assert!(
                matches!(s, "high_tier" | "low_tier" | "degraded"),
                "非法档位字符串: {s}"
            );
            assert_eq!(tier.to_string(), s, "Display 与 as_str 不一致");
        }
    }
}
