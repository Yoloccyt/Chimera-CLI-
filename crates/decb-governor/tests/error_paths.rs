//! DECB 错误路径测试(SubTask 37.7)
//!
//! 验证 DecbError 5 个变体的触发与处理:
//! BudgetExceeded / InvalidCoefficient / DegradedModeRejected / ConfigError / 溢出检测失败。
//!
//! 对应架构层:L8 Parliament
//! 对应创新点:DECB(Dual-tier Cognitive Budget)

#![forbid(unsafe_code)]

use decb_governor::{
    BudgetCoefficient, BudgetConsumption, BudgetTier, DecbConfig, DecbError, DecbGovernor,
    OverflowDetector, QuestBudgetInput,
};

// ============================================================
// DECB 错误路径测试(5 个)
// ============================================================

/// 错误路径:BudgetExceeded — 预算超限
///
/// WHY:当前消耗超过预算上限时触发,携带预算类型、当前值与上限,
/// 便于上层(L8 Parliament)决策是否拒绝新 Quest 或触发降级链路。
#[test]
fn test_error_budget_exceeded() {
    // 场景 1:构造 BudgetExceeded 错误,验证 Display
    let err = DecbError::BudgetExceeded {
        budget_type: "token".into(),
        current: 1500,
        limit: 1000,
    };
    let msg = err.to_string();

    assert!(
        msg.contains("budget exceeded"),
        "错误消息应包含 'budget exceeded',实际: {msg}"
    );
    assert!(msg.contains("token"), "错误消息应包含预算类型 'token'");
    assert!(msg.contains("1500"), "错误消息应包含当前值 1500");
    assert!(msg.contains("1000"), "错误消息应包含上限 1000");

    // 场景 2:OverflowDetector 检测到 100% 溢出,建议 Degraded
    let config = DecbConfig::default();
    let detector = OverflowDetector::new(&config);
    let full_consumption = config.total_budget_limit;
    let suggested = detector.check_overflow(full_consumption);
    assert_eq!(
        suggested,
        Some(BudgetTier::Degraded),
        "100% 溢出应建议 Degraded"
    );

    // 场景 3:80% 溢出应建议 LowTier
    let high_consumption = config.total_budget_limit * 0.8;
    let suggested = detector.check_overflow(high_consumption);
    assert_eq!(
        suggested,
        Some(BudgetTier::LowTier),
        "80% 溢出应建议 LowTier"
    );

    // 场景 4:50% 溢出不应建议降级(仅警告)
    let warn_consumption = config.total_budget_limit * 0.5;
    let suggested = detector.check_overflow(warn_consumption);
    assert_eq!(suggested, None, "50% 溢出应仅警告,不降级");
}

/// 错误路径:InvalidCoefficient — 预算系数非法
///
/// WHY:BudgetCoefficient::try_new 对越界值(∉ [0,1] 或 NaN)返回错误,
/// 此测试验证严格校验路径。
#[test]
fn test_error_invalid_coefficient() {
    // 场景 1:超过上限的系数
    let err = BudgetCoefficient::try_new(1.5).expect_err("1.5 应被拒绝");
    let msg = err.to_string();
    assert!(
        msg.contains("invalid budget coefficient"),
        "错误消息应包含 'invalid budget coefficient',实际: {msg}"
    );
    assert!(msg.contains("1.5"), "错误消息应包含越界值 1.5");

    // 场景 2:负数系数
    let err = BudgetCoefficient::try_new(-0.1).expect_err("-0.1 应被拒绝");
    assert!(
        err.to_string().contains("-0.1"),
        "错误消息应包含越界值 -0.1"
    );

    // 场景 3:NaN 系数
    let err = BudgetCoefficient::try_new(f32::NAN).expect_err("NaN 应被拒绝");
    assert!(err.to_string().contains("NaN"), "错误消息应包含 NaN");

    // 场景 4:对比 new() 与 try_new() 的语义差异
    // new() 自动 clamp,不报错;try_new() 严格拒绝
    let clamped = BudgetCoefficient::new(1.5);
    assert!(
        (clamped.value() - 1.0).abs() < 1e-6,
        "new(1.5) 应 clamp 到 1.0"
    );
    let clamped_neg = BudgetCoefficient::new(-0.5);
    assert!(
        (clamped_neg.value() - 0.0).abs() < 1e-6,
        "new(-0.5) 应 clamp 到 0.0"
    );
    let clamped_nan = BudgetCoefficient::new(f32::NAN);
    assert!(
        (clamped_nan.value() - 0.0).abs() < 1e-6,
        "new(NaN) 应映射为 0.0"
    );

    // 场景 5:合法值通过 try_new
    assert!(BudgetCoefficient::try_new(0.0).is_ok(), "0.0 应通过");
    assert!(BudgetCoefficient::try_new(0.5).is_ok(), "0.5 应通过");
    assert!(BudgetCoefficient::try_new(1.0).is_ok(), "1.0 应通过");
}

/// 错误路径:DegradedModeRejected — 降级模式拒绝新 Quest
///
/// WHY:Degraded 是最低档位,无法继续降级。此时新 Quest 应被拒绝,
/// 携带 quest_id 便于上层追踪哪个 Quest 被拒绝。
#[test]
fn test_error_degraded_mode_rejected() {
    // 场景 1:构造 DegradedModeRejected 错误,验证 Display
    let err = DecbError::DegradedModeRejected {
        quest_id: "quest-overflow".into(),
        reason: "budget exhausted in degraded mode".into(),
    };
    let msg = err.to_string();

    assert!(
        msg.contains("degraded mode rejected quest"),
        "错误消息应包含 'degraded mode rejected quest',实际: {msg}"
    );
    assert!(msg.contains("quest-overflow"), "错误消息应包含 quest_id");
    assert!(msg.contains("budget exhausted"), "错误消息应包含拒绝原因");

    // 场景 2:模拟 Degraded 档位下预算耗尽
    let config = DecbConfig {
        tier_switch_lag_ms: 0, // 测试用:无滞后
        ..DecbConfig::default()
    };
    let governor = DecbGovernor::new(config).expect("默认配置应通过校验");

    // 耗尽预算
    let exhaust = BudgetConsumption {
        total_cost: governor.config().total_budget_limit,
        ..BudgetConsumption::zero()
    };
    governor
        .record_consumption(&exhaust)
        .expect("记录消耗不应失败");

    // 验证预算耗尽后系数为 0.0(Degraded 触发条件)
    let quest = QuestBudgetInput::simple("quest-degraded");
    let coefficient = governor.compute_budget(&quest);
    assert!(
        coefficient.abs() < 1e-6,
        "预算耗尽后系数应为 0.0(触发 Degraded),实际: {coefficient}"
    );

    // 验证档位判定为 Degraded
    let tier = governor.determine_tier(coefficient);
    assert_eq!(tier, BudgetTier::Degraded, "系数 0.0 应判定为 Degraded");
}

/// 错误路径:ConfigError — 配置错误
///
/// WHY:DecbConfig::validate 检测到非法配置时返回 ConfigError,
/// DecbGovernor::new 在构造时调用 validate,提前暴露配置错误。
#[test]
fn test_error_config_invalid() {
    // 场景 1:base_budget 越界
    let bad_config = DecbConfig {
        base_budget: 1.5,
        ..DecbConfig::default()
    };
    let err = bad_config.validate().expect_err("base_budget=1.5 应报错");
    assert!(
        err.to_string().contains("base_budget"),
        "错误消息应包含 'base_budget'"
    );

    // 场景 2:阈值倒挂(high <= low)
    let inverted_config = DecbConfig {
        high_tier_threshold: 0.3,
        low_tier_threshold: 0.6,
        ..DecbConfig::default()
    };
    let err = inverted_config.validate().expect_err("阈值倒挂应报错");
    assert!(
        err.to_string().contains("high_tier_threshold"),
        "错误消息应包含 'high_tier_threshold'"
    );

    // 场景 3:总预算为 0
    let zero_budget_config = DecbConfig {
        total_budget_limit: 0.0,
        ..DecbConfig::default()
    };
    let err = zero_budget_config
        .validate()
        .expect_err("总预算为 0 应报错");
    assert!(
        err.to_string().contains("total_budget_limit"),
        "错误消息应包含 'total_budget_limit'"
    );

    // 场景 4:复杂度因子倒挂
    let inverted_factor_config = DecbConfig {
        complexity_factor_min: 1.5,
        complexity_factor_max: 0.5,
        ..DecbConfig::default()
    };
    let err = inverted_factor_config
        .validate()
        .expect_err("复杂度因子倒挂应报错");
    assert!(
        err.to_string().contains("complexity_factor"),
        "错误消息应包含 'complexity_factor'"
    );

    // 场景 5:DecbGovernor::new 拒绝非法配置
    let result = DecbGovernor::new(bad_config);
    assert!(
        result.is_err(),
        "DecbGovernor::new 应拒绝 base_budget=1.5 的配置"
    );
    let err = result.err().expect("已校验 is_err,必含错误");
    assert!(
        matches!(err, DecbError::ConfigError { .. }),
        "应返回 ConfigError 变体,实际: {err:?}"
    );

    // 场景 6:负成本单价
    let neg_cost_config = DecbConfig {
        cost_per_token: -0.1,
        ..DecbConfig::default()
    };
    let err = neg_cost_config.validate().expect_err("负成本单价应报错");
    assert!(
        err.to_string().contains("cost_per_token"),
        "错误消息应包含 'cost_per_token'"
    );
}

/// 错误路径:溢出检测失败(边界条件)
///
/// WHY:OverflowDetector 在边界条件(0 预算、负消耗、超大消耗)下
/// 应正确处理,不 panic、不返回错误值。此测试验证边界处理。
#[test]
fn test_error_overflow_detection_failed() {
    let config = DecbConfig::default();
    let detector = OverflowDetector::new(&config);

    // 场景 1:0 消耗 → 无溢出(None)
    let result = detector.check_overflow(0.0);
    assert_eq!(result, None, "0 消耗应无溢出建议");

    // 场景 2:负消耗(异常情况)→ 无溢出(None)
    // WHY 负消耗:理论上不应发生,但检测器应容错处理
    let result = detector.check_overflow(-100.0);
    assert_eq!(result, None, "负消耗应无溢出建议(容错)");

    // 场景 3:恰好 80% 阈值 → LowTier
    let threshold_80 = config.total_budget_limit * 0.8;
    let result = detector.check_overflow(threshold_80);
    assert_eq!(result, Some(BudgetTier::LowTier), "80% 应触发 LowTier");

    // 场景 4:恰好 100% 阈值 → Degraded
    let threshold_100 = config.total_budget_limit;
    let result = detector.check_overflow(threshold_100);
    assert_eq!(result, Some(BudgetTier::Degraded), "100% 应触发 Degraded");

    // 场景 5:超过 100% → Degraded(不 panic)
    let over_100 = config.total_budget_limit * 1.5;
    let result = detector.check_overflow(over_100);
    assert_eq!(
        result,
        Some(BudgetTier::Degraded),
        "150% 应触发 Degraded(不 panic)"
    );

    // 场景 6:0 预算配置的检测器视为已溢出
    let zero_config = DecbConfig {
        total_budget_limit: 0.0,
        ..DecbConfig::default()
    };
    // WHY 0 预算配置无法通过 validate,但 OverflowDetector::new 不校验,
    // 直接构造以测试边界
    let zero_detector = OverflowDetector::new(&zero_config);
    let result = zero_detector.check_overflow(0.0);
    assert_eq!(
        result,
        Some(BudgetTier::Degraded),
        "0 预算配置应视为已溢出(Degraded)"
    );

    // 场景 7:trigger_degradation 链路验证
    let tier = detector.trigger_degradation(BudgetTier::HighTier);
    assert_eq!(tier, BudgetTier::LowTier, "HighTier 应降级到 LowTier");
    let tier = detector.trigger_degradation(BudgetTier::LowTier);
    assert_eq!(tier, BudgetTier::Degraded, "LowTier 应降级到 Degraded");
    let tier = detector.trigger_degradation(BudgetTier::Degraded);
    assert_eq!(
        tier,
        BudgetTier::Degraded,
        "Degraded 应保持不变(已最低,幂等)"
    );
}
