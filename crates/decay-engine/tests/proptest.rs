//! decay-engine 属性测试 — 能力衰减曲线不变量
//!
//! 对应架构层:L4 Security
//! 对应 SubTask 13.6:v1.5.0-omega 发布就绪差距闭合
//!
//! # 测试覆盖的不变量
//! 1. CapabilityLevel::new 拒绝越界值(< 0 或 > 1),接受 [0, 1]
//! 2. CapabilityLevel::is_frozen(level <= 0.0) 与 is_full(level >= 1.0)
//! 3. DecayEngine:register_capability 后 get_level 返回初始等级
//! 4. decay(ViolationPenalty) 降低等级(或冻结时保持不变)
//! 5. freeze 后 is_frozen == true,重复 freeze 返回 AlreadyFrozen
//!
//! # 设计要点
//! - 使用整数策略生成 f32 level(避免 NaN/Inf)
//! - block-named 语法(§4.1 规范)
//! - 256 cases(proptest 默认)

#![forbid(unsafe_code)]

use decay_engine::{DecayConfig, DecayEngine, DecayError, DecayEvent};
use proptest::prelude::*;

/// 生成 [0, 1] 区间的 f32 level(精度 1/1000)
fn level_strategy() -> impl Strategy<Value = f32> {
    (0u32..=1000).prop_map(|x| x as f32 / 1000.0)
}

proptest! {
    /// 不变量 1:CapabilityLevel::new 拒绝越界值,接受 [0, 1]
    ///
    /// - level ∈ [0, 1] → Ok
    /// - level < 0 或 level > 1 → Err(InvalidLevel)
    ///
    /// WHY 边界校验:权限流体模型要求 [0, 1] 范围,
    /// 越界值会破坏衰减数学与冻结语义(§6.1 尸检红线)。
    #[test]
    fn prop_capability_level_boundary(level_milli in 0u32..=2000) {
        let level = level_milli as f32 / 1000.0; // [0.0, 2.0]
        let result = decay_engine::CapabilityLevel::new(level);

        if (0.0..=1.0).contains(&level) {
            prop_assert!(
                result.is_ok(),
                "level={} ∈ [0, 1] 应被接受",
                level
            );
            let cap_level = result.unwrap();
            prop_assert!((cap_level.value() - level).abs() < 1e-6);
        } else {
            prop_assert!(
                matches!(result, Err(DecayError::InvalidLevel(_))),
                "level={} 越界应返回 InvalidLevel",
                level
            );
        }
    }

    /// 不变量 2:is_frozen(level <= 0.0) 与 is_full(level >= 1.0)
    ///
    /// - level == 0.0 → is_frozen == true
    /// - level > 0.0 → is_frozen == false
    /// - level == 1.0 → is_full == true
    /// - level < 1.0 → is_full == false
    #[test]
    fn prop_capability_level_frozen_full(level_milli in 0u32..=1000) {
        let level = level_milli as f32 / 1000.0;
        let cap_level = decay_engine::CapabilityLevel::new(level)
            .expect("level ∈ [0, 1] 应被接受");

        // is_frozen: level <= 0.0
        if level <= 0.0 {
            prop_assert!(cap_level.is_frozen(), "level={} <= 0.0 应 is_frozen", level);
        } else {
            prop_assert!(!cap_level.is_frozen(), "level={} > 0.0 不应 is_frozen", level);
        }

        // is_full: level >= 1.0
        if level >= 1.0 {
            prop_assert!(cap_level.is_full(), "level={} >= 1.0 应 is_full", level);
        } else {
            prop_assert!(!cap_level.is_full(), "level={} < 1.0 不应 is_full", level);
        }
    }

    /// 不变量 3:register_capability 后 get_level 返回初始等级
    ///
    /// 注册任意合法初始等级,get_level 应返回相同值。
    /// 验证注册表与查询的一致性。
    #[test]
    fn prop_register_then_get_level(initial_level in level_strategy()) {
        let engine = DecayEngine::new(DecayConfig::default());
        let result = engine.register_capability("cap-1", "测试能力", initial_level);
        prop_assert!(result.is_ok(), "注册应成功: {:?}", result);

        let got = engine.get_level("cap-1");
        prop_assert!(got.is_ok(), "get_level 应成功");
        let level = got.unwrap();
        prop_assert!(
            (level.value() - initial_level).abs() < 1e-6,
            "get_level 应返回初始等级 {},实际 {}",
            initial_level,
            level.value()
        );
    }

    /// 不变量 4:decay(ViolationPenalty) 降低等级(或冻结时保持不变)
    ///
    /// 对未冻结能力施加 ViolationPenalty:
    /// - 等级应降低(或降至 min_level 后保持)
    /// - 降低幅度 = event_decay_penalty × severity(默认 0.1 × severity)
    /// - 冻结能力:保持不变(返回当前 level)
    ///
    /// WHY 仅测试 ViolationPenalty:TimeDecay 依赖 elapsed 时间,
    /// 难以在 proptest 中精确控制;ViolationPenalty 的衰减量仅依赖 severity,可精确验证。
    #[test]
    fn prop_violation_penalty_reduces_level(
        initial_level_milli in 100u32..=1000,  // 初始等级 [0.1, 1.0](避免起点过低立即冻结)
        severity_milli in 100u32..=2000,        // severity [0.1, 2.0]
    ) {
        let initial_level = initial_level_milli as f32 / 1000.0;
        let severity = severity_milli as f32 / 1000.0;
        let engine = DecayEngine::new(DecayConfig::default());
        engine
            .register_capability("cap-1", "测试能力", initial_level)
            .expect("注册应成功");

        let result = engine.decay(
            "cap-1",
            DecayEvent::ViolationPenalty {
                capability_id: "cap-1".into(),
                severity,
            },
        );
        prop_assert!(result.is_ok(), "decay 应成功: {:?}", result);
        let new_level = result.unwrap().value();

        // 预期衰减量
        let penalty = 0.1 * severity; // event_decay_penalty × severity
        let expected_lower_bound = (initial_level - penalty).max(0.0);
        // 新等级应 <= 初始等级(单调下降)
        prop_assert!(
            new_level <= initial_level + 1e-6,
            "ViolationPenalty 后等级 {} 应 <= 初始等级 {}",
            new_level,
            initial_level
        );
        // 新等级应 >= max(0, initial - penalty)(衰减量不超过 penalty)
        // 注意:若触发自动冻结(freeze_threshold=0.05),new_level 可能为 0.0
        prop_assert!(
            new_level >= expected_lower_bound - 1e-6 || new_level == 0.0,
            "新等级 {} 应 >= {} 或为 0.0(自动冻结)",
            new_level,
            expected_lower_bound
        );
    }

    /// 不变量 5:freeze 后 is_frozen == true,重复 freeze 返回 AlreadyFrozen
    ///
    /// - 首次 freeze 成功,is_frozen == true,get_level == 0.0
    /// - 再次 freeze 返回 AlreadyFrozen 错误
    /// - freeze 是幂等安全:重复 freeze 不会修改状态
    #[test]
    fn prop_freeze_idempotent(initial_level in level_strategy()) {
        let engine = DecayEngine::new(DecayConfig::default());
        engine
            .register_capability("cap-1", "测试能力", initial_level)
            .expect("注册应成功");

        // 首次 freeze
        let result = engine.freeze("cap-1", "测试冻结");
        prop_assert!(result.is_ok(), "首次 freeze 应成功: {:?}", result);
        prop_assert!(engine.is_frozen("cap-1").unwrap_or(false), "freeze 后应 is_frozen");
        let level = engine.get_level("cap-1").unwrap();
        prop_assert!(
            (level.value() - 0.0).abs() < 1e-6,
            "freeze 后 level 应为 0.0,实际 {}",
            level.value()
        );

        // 再次 freeze 应返回 AlreadyFrozen
        let dup = engine.freeze("cap-1", "重复冻结");
        prop_assert!(
            matches!(dup, Err(DecayError::AlreadyFrozen(_))),
            "重复 freeze 应返回 AlreadyFrozen,实际 {:?}",
            dup
        );
        // 状态保持不变
        prop_assert!(engine.is_frozen("cap-1").unwrap_or(false));
    }
}
