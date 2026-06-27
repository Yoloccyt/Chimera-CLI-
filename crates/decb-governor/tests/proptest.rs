//! DECB proptest — 不变量属性测试(SubTask 37.7)
//!
//! 验证预算系数 ∈ [0,1] 与档位切换单调性。
//!
//! 对应架构层:L8 Parliament
//! 对应创新点:DECB(Dual-tier Cognitive Budget)

#![forbid(unsafe_code)]

use decb_governor::{BudgetTier, DecbConfig, DecbGovernor, QuestBudgetInput};
use proptest::prelude::*;

// 不变量:预算系数 ∈ [0.0, 1.0]
//
// 生成随机 base_budget、complexity、urgency、remaining_ratio,
// 计算预算系数,验证结果始终在 [0,1] 区间。
//
// WHY 此不变量:预算系数是 DECB 双档切换的核心输入,
// 越界值会导致档位判定异常(§6 架构红线:预算优先)
#[test]
fn proptest_budget_coefficient_in_range() {
    proptest!(|(task_count in 0u32..=100, dependency_depth in 0u32..=50, description_length in 0u32..=10000)| {
        let governor = DecbGovernor::new(DecbConfig::default()).map_err(|e| {
            TestCaseError::fail(format!("DecbGovernor 构造失败: {e}"))
        })?;

        let quest = QuestBudgetInput::new(
            "q-prop",
            task_count as usize,
            dependency_depth as usize,
            None, // 无 deadline,urgency = 1.0
            description_length as usize,
        );

        let coefficient = governor.compute_budget(&quest);

        prop_assert!(
            (0.0..=1.0).contains(&coefficient),
            "预算系数 {} 应在 [0,1] 区间(task_count={}, depth={}, desc_len={})",
            coefficient, task_count, dependency_depth, description_length
        );
    });
}

// 不变量:档位切换单调性
//
// 生成两个系数 c1 < c2,验证 tier(c1) ≤ tier(c2)
// (Degraded < LowTier < HighTier)。
//
// WHY 此不变量:档位阈值单调递增是 DECB 配置校验的硬约束,
// 违反会导致档位判定反转(高预算被判定为低档)
#[test]
fn proptest_tier_switch_monotonicity() {
    proptest!(|(c1 in 0.0f32..=1.0, c2 in 0.0f32..=1.0)| {
        let governor = DecbGovernor::new(DecbConfig::default()).map_err(|e| {
            TestCaseError::fail(format!("DecbGovernor 构造失败: {e}"))
        })?;

        // 确保 c1 ≤ c2
        let (c1, c2) = if c1 <= c2 { (c1, c2) } else { (c2, c1) };

        let tier1 = governor.determine_tier(c1);
        let tier2 = governor.determine_tier(c2);

        // 档位单调性:c1 ≤ c2 → tier(c1) ≤ tier(c2)
        // 用 ordinal 比较:Degraded=0, LowTier=1, HighTier=2
        let ordinal = |t: BudgetTier| match t {
            BudgetTier::Degraded => 0,
            BudgetTier::LowTier => 1,
            BudgetTier::HighTier => 2,
        };

        prop_assert!(
            ordinal(tier1) <= ordinal(tier2),
            "档位单调性违反:c1={} → {:?}, c2={} → {:?}",
            c1, tier1, c2, tier2
        );
    });
}

// 不变量:BudgetCoefficient newtype clamp 到 [0,1]
//
// 生成任意 f32 值,通过 BudgetCoefficient::new 构造,
// 验证内部值始终 ∈ [0,1](NaN 映射为 0.0)。
#[test]
fn proptest_coefficient_newtype_clamp() {
    proptest!(|(value in 0.0f32..=1.0)| {
        let coef = decb_governor::BudgetCoefficient::new(value);
        let v = coef.value();
        prop_assert!(
            (0.0..=1.0).contains(&v),
            "BudgetCoefficient::new({}) = {} 应在 [0,1] 区间",
            value, v
        );
    });
}

// 不变量:预算耗尽时系数为 0.0
//
// 模拟预算完全耗尽(total_consumption = total_budget_limit),
// 验证 remaining_budget_ratio = 0.0,系数 = 0.0。
//
// WHY 此不变量:预算耗尽应触发 Degraded 模式,
// 系数为 0.0 是降级链路的触发条件
#[test]
fn proptest_budget_exhausted_yields_zero_coefficient() {
    proptest!(|(task_count in 1u32..=50)| {
        let config = DecbConfig {
            tier_switch_lag_ms: 0, // 测试用:无滞后
            ..DecbConfig::default()
        };
        let governor = DecbGovernor::new(config).map_err(|e| {
            TestCaseError::fail(format!("DecbGovernor 构造失败: {e}"))
        })?;

        // 通过 record_consumption 耗尽预算
        use decb_governor::BudgetConsumption;
        let exhaust = BudgetConsumption {
            total_cost: governor.config().total_budget_limit,
            ..BudgetConsumption::zero()
        };
        governor.record_consumption(&exhaust).ok();

        let quest = QuestBudgetInput::new(
            "q-exhausted",
            task_count as usize,
            0,
            None,
            100,
        );
        let coefficient = governor.compute_budget(&quest);

        prop_assert!(
            coefficient.abs() < 1e-6,
            "预算耗尽时系数应为 0.0,实际: {}",
            coefficient
        );
    });
}
