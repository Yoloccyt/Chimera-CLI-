//! DecayEngine 集成测试
//!
//! 验收标准覆盖:
//! - 连续 [0,1] 权限流体(非离散)
//! - 冻结/解冻 API(对应 Skeptic 否决权)
//! - 5 次冻结测试
//! - 连续衰减曲线验证
//! - 时间驱动 + 事件驱动衰减

use decay_engine::types::DecayConfig;
use decay_engine::{DecayEngine, DecayEvent};

/// 测试用引擎配置:衰减速率激进,便于快速验证
/// (生产环境应使用 `default_config()`,time_decay_rate ≈ 0.001)
fn setup_engine() -> DecayEngine {
    let config = DecayConfig {
        time_decay_rate: 1.0, // 每秒衰减 100%(测试用,生产应为 0.001)
        event_decay_penalty: 0.1,
        min_level: 0.0,
        freeze_threshold: 0.05,
        restore_rate: 1.0, // 每秒恢复 100%
    };
    DecayEngine::new(config)
}

#[test]
fn test_capability_registration() {
    let engine = setup_engine();
    engine.register_capability("cap1", "文件读写", 0.8).unwrap();

    let level = engine.get_level("cap1").unwrap();
    assert!((level.value() - 0.8).abs() < 1e-6);
    assert!(!engine.is_frozen("cap1").unwrap());
}

#[test]
fn test_time_decay() {
    let engine = setup_engine();
    engine.register_capability("cap1", "测试能力", 1.0).unwrap();

    // sleep 确保时间流逝,elapsed > 0(避免零时长衰减无法验证)
    std::thread::sleep(std::time::Duration::from_millis(50));

    let new_level = engine.decay("cap1", DecayEvent::TimeDecay).unwrap();
    assert!(
        new_level.value() < 1.0,
        "时间衰减后 level 应递减,实际: {}",
        new_level.value()
    );
}

#[test]
fn test_violation_penalty() {
    let engine = setup_engine();
    engine.register_capability("cap1", "测试能力", 1.0).unwrap();

    let new_level = engine
        .decay(
            "cap1",
            DecayEvent::ViolationPenalty {
                capability_id: "cap1".to_string(),
                severity: 2.0,
            },
        )
        .unwrap();

    // 期望:1.0 - 0.1 × 2.0 = 0.8
    assert!(
        (new_level.value() - 0.8).abs() < 1e-6,
        "违规惩罚后 level 应为 0.8,实际: {}",
        new_level.value()
    );
}

#[test]
fn test_freeze_unfreeze() {
    let engine = setup_engine();
    engine.register_capability("cap1", "测试能力", 0.9).unwrap();

    // 5 次冻结/解冻循环(对应验收标准:5 次冻结测试)
    for i in 0..5 {
        engine
            .freeze("cap1", &format!("第 {} 次冻结", i + 1))
            .unwrap();
        assert!(
            engine.is_frozen("cap1").unwrap(),
            "第 {} 次冻结后应处于冻结状态",
            i + 1
        );
        assert!(engine.get_level("cap1").unwrap().is_frozen());

        engine.unfreeze("cap1").unwrap();
        assert!(
            !engine.is_frozen("cap1").unwrap(),
            "第 {} 次解冻后应处于非冻结状态",
            i + 1
        );
    }
}

#[test]
fn test_auto_freeze_below_threshold() {
    let engine = setup_engine();
    engine.register_capability("cap1", "测试能力", 0.1).unwrap();

    // 通过违规惩罚将 level 降到阈值以下(0.1 - 0.1 = 0.0 < 0.05)
    engine
        .decay(
            "cap1",
            DecayEvent::ViolationPenalty {
                capability_id: "cap1".to_string(),
                severity: 1.0,
            },
        )
        .unwrap();

    assert!(engine.is_frozen("cap1").unwrap(), "低于阈值应自动冻结");
}

#[test]
fn test_restore() {
    let engine = setup_engine();
    engine.register_capability("cap1", "测试能力", 0.5).unwrap();

    engine.freeze("cap1", "测试冻结").unwrap();
    engine.unfreeze("cap1").unwrap();

    // sleep 确保恢复有时间累积(elapsed > 0)
    std::thread::sleep(std::time::Duration::from_millis(50));

    let new_level = engine
        .decay(
            "cap1",
            DecayEvent::Restore {
                capability_id: "cap1".to_string(),
            },
        )
        .unwrap();

    assert!(
        new_level.value() > 0.0,
        "恢复后 level 应大于 0,实际: {}",
        new_level.value()
    );
}

#[test]
fn test_invalid_level_rejected() {
    let engine = setup_engine();

    let result = engine.register_capability("cap1", "测试能力", 1.5);
    assert!(result.is_err(), "level > 1.0 应被拒绝");

    let result = engine.register_capability("cap2", "测试能力", -0.1);
    assert!(result.is_err(), "level < 0.0 应被拒绝");
}

#[test]
fn test_continuous_decay_curve() {
    let engine = setup_engine();
    engine.register_capability("cap1", "测试能力", 1.0).unwrap();

    let mut prev_level = 1.0f32;

    // 多次违规衰减,验证 level 单调递减(连续衰减曲线)
    // 每次衰减 0.1 × 0.1 = 0.01:1.0 → 0.99 → 0.98 → 0.97 → 0.96 → 0.95
    for i in 0..5 {
        let new_level = engine
            .decay(
                "cap1",
                DecayEvent::ViolationPenalty {
                    capability_id: "cap1".to_string(),
                    severity: 0.1,
                },
            )
            .unwrap();

        assert!(
            new_level.value() <= prev_level + 1e-6,
            "第 {} 次衰减后 level 应单调递减,prev: {}, cur: {}",
            i + 1,
            prev_level,
            new_level.value()
        );
        prev_level = new_level.value();
    }
}

#[test]
fn test_freeze_blocks_decay() {
    let engine = setup_engine();
    engine.register_capability("cap1", "测试能力", 0.8).unwrap();

    engine.freeze("cap1", "测试冻结").unwrap();
    let frozen_level = engine.get_level("cap1").unwrap().value();

    // 冻结后尝试时间衰减,level 不应变
    std::thread::sleep(std::time::Duration::from_millis(50));
    let new_level = engine.decay("cap1", DecayEvent::TimeDecay).unwrap();

    assert!(
        (new_level.value() - frozen_level).abs() < 1e-6,
        "冻结后 level 不应变化,frozen: {}, after_decay: {}",
        frozen_level,
        new_level.value()
    );
    assert!(engine.is_frozen("cap1").unwrap());
}
