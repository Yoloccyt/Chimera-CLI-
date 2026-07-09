//! ACB 治理器属性测试(proptest)
//!
//! 对应任务:补齐 acb-governor 的 proptest 覆盖(§7.1 测试矩阵约定)
//!
//! # 验证的不变量
//! 1. `prop_tier_limits_strictly_increasing` — L0 < L1 < L2 < L3 token_limit 严格递增,
//!    validate() 必须拒绝非递增配置(避免预算倒挂导致级别判定异常)
//! 2. `prop_consumption_never_exceeds_budget` — 累计消耗不超过 total_budget_limit 时
//!    record_consumption 必须成功(避免预算未耗尽却误报超限)
//!
//! # 策略
//!
//! 使用 `proptest!` 宏的块状命名语法 `fn test_name(x in 0..100u32) { ... }`。
//! WHY: proptest 1.11.0 闭包形式 `|x in 0..100|` 可能解析失败,
//! 块状命名形式作为 fallback(Engineering Convention)。

#![forbid(unsafe_code)]

use acb_governor::{AcbGovernor, AcbGovernorConfig, BudgetTier};
use proptest::prelude::*;

/// 将任意可显示错误转换为 `TestCaseError`
fn fail<E: std::fmt::Display>(e: E) -> TestCaseError {
    TestCaseError::fail(format!("{e}"))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// 不变量 1:L0 < L1 < L2 < L3 token_limit 严格递增
    ///
    /// 任意四个 token_limit 值,validate() 的判定必须与"严格递增"语义一致:
    /// - 若 l0 < l1 < l2 < l3 严格递增 → validate() 必须返回 Ok(其余字段使用合法默认)
    /// - 否则(任意一对相邻非严格递增)→ validate() 必须返回 Err
    ///
    /// WHY 严格递增:级别越高,允许的单次请求 Token 上限应越大(§config.rs 的 validate
    /// 校验此不变量)。倒挂会导致 check_budget 误判:高级别反而拒绝低级别能通过的请求,
    /// 违反 ACB"级别越高资源越多"的设计前提。
    #[test]
    fn prop_tier_limits_strictly_increasing(
        l0 in 1u64..1000,
        l1 in 1u64..1000,
        l2 in 1u64..1000,
        l3 in 1u64..1000,
    ) {
        let config = AcbGovernorConfig {
            l0_token_limit: l0,
            l1_token_limit: l1,
            l2_token_limit: l2,
            l3_token_limit: l3,
            // 使用合法默认值,让 token_limit 成为唯一变量
            total_budget_limit: 1_000_000,
            degrade_threshold: 0.8,
            upgrade_threshold: 0.3,
            tier_switch_lag_ms: 1_000,
        };
        let result = config.validate();

        // 严格递增判定:必须 l0 < l1 < l2 < l3(任意相等都不算严格递增)
        let is_strictly_increasing = l0 < l1 && l1 < l2 && l2 < l3;

        if is_strictly_increasing {
            prop_assert!(
                result.is_ok(),
                "严格递增配置应通过校验: L0={} L1={} L2={} L3={}",
                l0, l1, l2, l3
            );
        } else {
            prop_assert!(
                result.is_err(),
                "非严格递增配置应被拒绝: L0={} L1={} L2={} L3={}",
                l0, l1, l2, l3
            );
        }
    }

    /// 不变量 2:累计消耗不超过总预算时 record_consumption 必须成功
    ///
    /// 任意总预算 budget(>0)和单次消耗 consume(>0),只要累计消耗 ≤ budget,
    /// record_consumption 必须返回 Ok。这保证预算未耗尽时不会误报 DegradedModeRejected。
    ///
    /// WHY 累计守恒:record_consumption 是高频写路径(§governor.rs 步骤 1 原子累加),
    /// 必须保证累加值正确且不超出预算时永不拒绝。否则会导致 L9 Quest Engine
    /// 在预算充足时被误拒,违反 ACB"自适应"语义。
    ///
    /// 注:此测试不验证降级机制 — adjust_budget 在 utilization > degrade_threshold 时
    /// 会触发降级,但 record_consumption 本身在累计 ≤ budget 时仍应 Ok(降级不等于拒绝)。
    ///
    /// 配置策略:使用默认 token_limit(L0<L1<L2<L3 严格递增,满足 validate),仅修改
    /// total_budget_limit 为 budget;阈值设为极端值避免触发降级/升级;consume 限制在
    /// ≤ L0 上限(1000)以保证 check_budget 不超限。
    #[test]
    fn prop_consumption_never_exceeds_budget(
        budget in 1_000u64..100_000,
        consume in 1u64..1_000,  // ≤ L0 默认上限(1000),保证 check_budget 不超限
        repeat_count in 1u32..50,
    ) {
        // 确保累计消耗 ≤ budget:repeat_count × consume ≤ budget
        // WHY saturating_mul + min:避免溢出,并保证测试前提成立
        let safe_repeat = (budget / consume).min(repeat_count as u64);
        let safe_repeat = safe_repeat.min(50) as u32;

        let config = AcbGovernorConfig {
            // 仅覆盖 total_budget_limit 为变量 budget,其余字段用默认值
            // (默认 token_limit L0<L1<L2<L3 严格递增,满足 validate 不变量)
            total_budget_limit: budget,
            // 阈值设为极端值避免触发降级/升级
            degrade_threshold: 1.0,  // 100% 才降级(永不到达)
            upgrade_threshold: 0.0,  // 0% 才升级(永不到达)
            tier_switch_lag_ms: 0,   // 无滞后约束
            ..Default::default()
        };
        let governor = AcbGovernor::new(config).map_err(fail)?;

        // 累计消耗 safe_repeat × consume,总和 = safe_repeat × consume ≤ budget
        let total = (safe_repeat as u64).saturating_mul(consume);
        prop_assert!(
            total <= budget,
            "测试前提:累计消耗 {} 应 ≤ budget {}",
            total, budget
        );

        // 执行 safe_repeat 次 record_consumption,每次都应 Ok
        for _ in 0..safe_repeat {
            let result = governor.record_consumption(consume);
            prop_assert!(
                result.is_ok(),
                "累计消耗未超 budget 时 record_consumption 必须成功 \
                (consume={}, budget={}, 当前累计={})",
                consume,
                budget,
                governor.get_status().total_consumption
            );
        }

        // 最终状态校验:累计消耗等于 safe_repeat × consume
        let expected_total = (safe_repeat as u64) * consume;
        let actual_total = governor.get_status().total_consumption;
        prop_assert_eq!(
            actual_total, expected_total,
            "累计消耗应为 {} 实际 {} (budget={}, consume={}, repeat={})",
            expected_total, actual_total, budget, consume, safe_repeat
        );
    }

    /// 不变量 3:BudgetTier degrade/upgrade 单调性
    ///
    /// 任意 BudgetTier(由 from_level 构造),degrade 后级别 ≤ 原级别,
    /// upgrade 后级别 ≥ 原级别。这保证 ACB 级别切换的"流体单调性" —
    /// 降级不会越级升,升级不会越级降(§types.rs BudgetTier::degrade/upgrade)。
    ///
    /// WHY 单调性:对应 §6 架构红线"竞态/抖动防护"。如果 degrade 不单调
    /// (如 L2.degrade 返回 L3),会导致级别反复横跳,违反滞后带设计。
    #[test]
    fn prop_tier_degrade_upgrade_monotonic(level in 0u8..4) {
        let tier = BudgetTier::from_level(level).map_err(fail)?;

        // degrade 单调性:降级后级别 ≤ 原级别
        let degraded = tier.degrade();
        prop_assert!(
            degraded.as_level() <= tier.as_level(),
            "degrade 后级别 {} 应 ≤ 原级别 {}",
            degraded.as_level(),
            tier.as_level()
        );

        // upgrade 单调性:升级后级别 ≥ 原级别
        let upgraded = tier.upgrade();
        prop_assert!(
            upgraded.as_level() >= tier.as_level(),
            "upgrade 后级别 {} 应 ≥ 原级别 {}",
            upgraded.as_level(),
            tier.as_level()
        );

        // 边界守恒:L0.degrade == L0(最低不可再降),L3.upgrade == L3(最高不可再升)
        if tier == BudgetTier::L0 {
            prop_assert_eq!(
                degraded, BudgetTier::L0,
                "L0 已最低,degrade 应返回 L0"
            );
        }
        if tier == BudgetTier::L3 {
            prop_assert_eq!(
                upgraded, BudgetTier::L3,
                "L3 已最高,upgrade 应返回 L3"
            );
        }
    }
}
