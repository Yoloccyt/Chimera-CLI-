//! DecayEngine proptest 属性测试 — 衰减函数不变量验证
//!
//! 对应任务: T6-6 proptest 属性测试集成
//! 架构层: L4 Security (decay-engine)
//!
//! # 验证的不变量
//! 1. ViolationPenalty 单调性 — 连续违规惩罚,能力值单调不增
//! 2. 衰减有界性 — 能力值始终在 [0.0, 1.0] 范围内
//! 3. Freeze 语义 — 冻结后 level 为 0.0 且 frozen 为 true
//! 4. ViolationPenalty 惩罚量正确性 — level 减少量 = penalty × severity
//!
//! # 语法约束(§4.4)
//! proptest 1.11+ 用 block-named 语法: `fn name(arg in strategy) { body }`

#![forbid(unsafe_code)]

use decay_engine::types::DecayConfig;
use decay_engine::{DecayEngine, DecayEvent};
use proptest::prelude::*;

/// 创建测试用 DecayEngine(可控配置)
fn make_engine(penalty: f32, min_level: f32) -> DecayEngine {
    let config = DecayConfig {
        time_decay_rate: 0.0, // 禁用时间衰减,专注测试 ViolationPenalty
        event_decay_penalty: penalty,
        min_level,
        freeze_threshold: 0.0, // 禁用自动冻结(避免干扰单调性验证)
        restore_rate: 0.0,
    };
    DecayEngine::new(config)
}

proptest! {
    /// 不变量 1: ViolationPenalty 单调性 — 连续违规惩罚下能力值单调不增
    ///
    /// 固定 penalty × severity > 0,连续施加 3 次惩罚,
    /// 断言每次衰减后 level <= 前一次 level。
    ///
    /// WHY: 权限单调不增是安全基石——若衰减后权限反而升高,
    /// 等同于"越违规权限越大",违反最小权限原则。
    #[test]
    fn prop_violation_monotonicity(
        initial in 0.1f32..1.0f32,
        penalty in 0.01f32..0.5f32,
        severity in 0.1f32..5.0f32,
    ) {
        let engine = make_engine(penalty, 0.0);
        engine.register_capability("cap", "test", initial)?;

        let mut prev = initial;
        for _ in 0..3 {
            let level = engine.decay("cap", DecayEvent::ViolationPenalty {
                capability_id: "cap".into(),
                severity,
            })?;
            let v = level.value();
            prop_assert!(
                v <= prev + 1e-6,
                "level {} should be <= prev {} (monotonicity)",
                v, prev
            );
            prev = v;
        }
    }

    /// 不变量 2: 衰减有界性 — ViolationPenalty 后 level 始终在 [0.0, 1.0]
    ///
    /// 任意初始值、惩罚系数和严重度的组合下,level 不能越界。
    ///
    /// WHY: 连续 [0,1] 流体模型的核心约束。越界意味着权限异常:
    /// < 0 表示"负权限"(无意义),> 1 表示"超权限"(安全漏洞)。
    #[test]
    fn prop_decay_bounded(
        initial in 0.0f32..1.0f32,
        penalty in 0.01f32..1.0f32,
        severity in 0.1f32..10.0f32,
    ) {
        let engine = make_engine(penalty, 0.0);
        engine.register_capability("cap", "test", initial)?;

        let level = engine.decay("cap", DecayEvent::ViolationPenalty {
            capability_id: "cap".into(),
            severity,
        })?;
        let v = level.value();
        prop_assert!(
            v >= 0.0 && v <= 1.0,
            "level {} out of [0.0, 1.0] (initial={}, penalty={}, severity={})",
            v, initial, penalty, severity
        );
    }

    /// 不变量 3: Freeze 语义 — 冻结后 level 精确为 0.0
    ///
    /// 任意初始值下,freeze 后 level 必须为 0.0(完全剥夺权限)。
    ///
    /// WHY: Freeze 对应 Skeptic 否决权,是安全最后防线。
    /// 若冻结后 level > 0,残留权限可能被利用进行权限提升攻击。
    #[test]
    fn prop_freeze_sets_level_to_zero(
        initial in 0.0f32..1.0f32,
    ) {
        let engine = make_engine(0.1, 0.0);
        engine.register_capability("cap", "test", initial)?;
        engine.freeze("cap", "proptest freeze")?;

        let level = engine.get_level("cap")?;
        prop_assert_eq!(level.value(), 0.0, "freeze must set level to exactly 0.0");
        prop_assert!(engine.is_frozen("cap")?, "freeze must set frozen to true");
    }

    /// 不变量 4: ViolationPenalty 惩罚量正确性 —
    /// level 减少量 = min(penalty × severity, level - min_level)
    ///
    /// 在不触发 clamp 的范围内(即 penalty × severity <= level - min_level),
    /// 实际减少量应精确等于 penalty × severity。
    ///
    /// WHY: 惩罚量正确性保证衰减模型的可预测性——
    /// 安全策略制定者需要精确知道违规的代价。
    #[test]
    fn prop_violation_penalty_amount(
        initial in 0.5f32..1.0f32,
        penalty in 0.01f32..0.2f32,
        severity in 0.1f32..2.0f32,
    ) {
        let expected_drop = penalty * severity;
        // 仅在预期降量不超过初始值时验证精确性(避免 clamp 干扰)
        if expected_drop <= initial {
            let engine = make_engine(penalty, 0.0);
            engine.register_capability("cap", "test", initial)?;

            let level = engine.decay("cap", DecayEvent::ViolationPenalty {
                capability_id: "cap".into(),
                severity,
            })?;
            let expected = initial - expected_drop;
            let actual = level.value();
            prop_assert!(
                (actual - expected).abs() < 1e-4,
                "expected level ~{} but got {} (initial={}, penalty={}, severity={})",
                expected, actual, initial, penalty, severity
            );
        }
    }

    /// 不变量 5: 不同能力的衰减隔离性 — 衰减一个能力不影响另一个
    ///
    /// 注册两个独立能力,衰减其中一个,另一个的 level 保持不变。
    ///
    /// WHY: DashMap 分片锁若存在跨分片干扰(如全局状态污染),
    /// 会导致能力间权限串扰,违反最小权限隔离原则。
    #[test]
    fn prop_decay_isolation(
        initial_a in 0.2f32..1.0f32,
        initial_b in 0.2f32..1.0f32,
        penalty in 0.01f32..0.3f32,
        severity in 0.1f32..3.0f32,
    ) {
        let engine = make_engine(penalty, 0.0);
        engine.register_capability("a", "cap_a", initial_a)?;
        engine.register_capability("b", "cap_b", initial_b)?;

        // 衰减能力 a
        let _ = engine.decay("a", DecayEvent::ViolationPenalty {
            capability_id: "a".into(),
            severity,
        })?;

        // 能力 b 应保持不变
        let level_b = engine.get_level("b")?;
        prop_assert!(
            (level_b.value() - initial_b).abs() < 1e-6,
            "decaying 'a' should not affect 'b': expected {}, got {}",
            initial_b, level_b.value()
        );
    }

    /// 不变量 6: 注册后立即查询 — level 等于初始值
    ///
    /// register_capability 后 get_level 返回的值应与注册的初始值一致。
    ///
    /// WHY: 注册是衰减链的起点。若注册后 level 即偏离初始值,
    /// 整个衰减模型的基准就不正确。
    #[test]
    fn prop_register_then_get_preserves_initial(
        initial in 0.0f32..1.0f32,
    ) {
        let engine = make_engine(0.1, 0.0);
        engine.register_capability("cap", "test", initial)?;
        let level = engine.get_level("cap")?;
        prop_assert!(
            (level.value() - initial).abs() < 1e-6,
            "get_level after register should equal initial {}, got {}",
            initial, level.value()
        );
    }

    /// 不变量 7: Freeze 隔离性 — 冻结一个能力不影响另一个
    ///
    /// 注册两个能力,冻结其中一个,另一个的 level 保持不变。
    ///
    /// WHY: DashMap 分片锁若存在跨分片干扰,
    /// freeze 操作可能误伤无关能力,违反最小权限隔离原则。
    #[test]
    fn prop_freeze_isolation(
        initial_a in 0.1f32..1.0f32,
        initial_b in 0.1f32..1.0f32,
    ) {
        let engine = make_engine(0.1, 0.0);
        engine.register_capability("a", "cap_a", initial_a)?;
        engine.register_capability("b", "cap_b", initial_b)?;

        engine.freeze("a", "proptest freeze isolation")?;

        let level_b = engine.get_level("b")?;
        prop_assert!(
            (level_b.value() - initial_b).abs() < 1e-6,
            "freezing 'a' should not affect 'b': expected {}, got {}",
            initial_b, level_b.value()
        );
        prop_assert!(
            !engine.is_frozen("b")?,
            "'b' should not be frozen when only 'a' was frozen"
        );
    }
}
