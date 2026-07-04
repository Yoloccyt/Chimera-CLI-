//! DecayEngine 集成测试
//!
//! 验收标准覆盖:
//! - 连续 [0,1] 权限流体(非离散)
//! - 冻结/解冻 API(对应 Skeptic 否决权)
//! - 5 次冻结测试
//! - 连续衰减曲线验证
//! - 时间驱动 + 事件驱动衰减

use decay_engine::types::DecayConfig;
use decay_engine::{DecayEngine, DecayError, DecayEvent};

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

// ============================================================================
// 以下为 E-MAJOR-1 补充测试:覆盖并发衰减、错误路径、边界值、restore 上限
// ============================================================================

/// WHY: DashMap 在并发"读-改-写"场景下必须保证原子性,
/// 否则并发衰减会丢失更新(lost update),导致能力值异常偏高(权限提升风险)。
#[test]
fn test_concurrent_decay_same_capability() {
    use std::sync::Arc;
    use std::thread;

    // Arrange: 8 线程各发起 10 次违规惩罚(severity=1.0,每次减 0.1)
    let engine = Arc::new(setup_engine());
    engine.register_capability("cap1", "共享能力", 1.0).unwrap();

    let num_threads = 8;
    let penalties_per_thread = 10;

    // Act
    let mut handles = Vec::new();
    for _ in 0..num_threads {
        let engine_clone = Arc::clone(&engine);
        handles.push(thread::spawn(move || {
            for _ in 0..penalties_per_thread {
                engine_clone
                    .decay(
                        "cap1",
                        DecayEvent::ViolationPenalty {
                            capability_id: "cap1".to_string(),
                            severity: 1.0,
                        },
                    )
                    .unwrap();
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    // Assert: 总惩罚 8×10×0.1=8.0 远超 1.0,能力应被 clamp 到 0 并自动冻结
    let final_level = engine.get_level("cap1").unwrap();
    assert!(
        final_level.value() <= 0.0 + 1e-6,
        "并发衰减后能力应被 clamp 到 0,实际: {}",
        final_level.value()
    );
    assert!(
        engine.is_frozen("cap1").unwrap(),
        "并发衰减到阈值以下应自动冻结"
    );
}

/// WHY: 并发衰减不同能力时,DashMap 的分片锁不应造成跨能力干扰,
/// 验证能力间隔离(一个能力的衰减不影响另一个,防止权限串扰)。
#[test]
fn test_concurrent_decay_different_capabilities() {
    use std::sync::Arc;
    use std::thread;

    // Arrange: 注册 4 个独立能力,初始均为 1.0
    let engine = Arc::new(setup_engine());
    for i in 0..4 {
        engine
            .register_capability(&format!("cap{}", i), &format!("能力{}", i), 1.0)
            .unwrap();
    }

    // Act: 每个能力在独立线程中各衰减 5 次(severity=1.0,每次减 0.1)
    let mut handles = Vec::new();
    for i in 0..4 {
        let engine_clone = Arc::clone(&engine);
        let cap_id = format!("cap{}", i);
        handles.push(thread::spawn(move || {
            for _ in 0..5 {
                engine_clone
                    .decay(
                        &cap_id,
                        DecayEvent::ViolationPenalty {
                            capability_id: cap_id.clone(),
                            severity: 1.0,
                        },
                    )
                    .unwrap();
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    // Assert: 每个能力独立衰减 5×0.1=0.5,最终值约 0.5(无串扰)
    for i in 0..4 {
        let level = engine.get_level(&format!("cap{}", i)).unwrap();
        assert!(
            (level.value() - 0.5).abs() < 1e-6,
            "能力{} 衰减后应为 0.5(互不干扰),实际: {}",
            i,
            level.value()
        );
        assert!(
            !engine.is_frozen(&format!("cap{}", i)).unwrap(),
            "能力{} 不应被冻结(0.5 > 阈值 0.05)",
            i
        );
    }
}

/// WHY: level=0.0 是权限流体的下界,衰减必须保持非负,
/// 且 0.0 <= freeze_threshold 应触发自动冻结,防止零权限能力残留可操作。
#[test]
fn test_decay_boundary_zero_capability() {
    // Arrange
    let engine = setup_engine();
    engine
        .register_capability("cap1", "零权限能力", 0.0)
        .unwrap();

    // Act: 对零权限能力应用时间衰减
    std::thread::sleep(std::time::Duration::from_millis(10));
    let new_level = engine.decay("cap1", DecayEvent::TimeDecay).unwrap();

    // Assert: 衰减后仍为 0.0(clamp 保护),且触发自动冻结
    assert!(
        new_level.value() <= 0.0 + 1e-6,
        "零权限能力衰减后应保持 0,实际: {}",
        new_level.value()
    );
    assert!(
        engine.is_frozen("cap1").unwrap(),
        "零权限能力衰减后应自动冻结(0.0 <= freeze_threshold)"
    );
}

/// WHY: level=1.0 是权限流体的上界,注册后应正确识别为满权限,
/// 衰减后应立即 < 1.0(验证上界处理无 off-by-one 或浮点越界)。
#[test]
fn test_decay_boundary_max_capability() {
    // Arrange
    let engine = setup_engine();
    engine
        .register_capability("cap1", "满权限能力", 1.0)
        .unwrap();

    // Assert: 注册后应识别为满权限
    let level = engine.get_level("cap1").unwrap();
    assert!(
        level.is_full(),
        "注册 level=1.0 应识别为满权限,实际: {}",
        level.value()
    );

    // Act: 轻微时间衰减
    std::thread::sleep(std::time::Duration::from_millis(10));
    let new_level = engine.decay("cap1", DecayEvent::TimeDecay).unwrap();

    // Assert: 衰减后应 < 1.0,不再是满权限
    assert!(
        new_level.value() < 1.0,
        "满权限衰减后应 < 1.0,实际: {}",
        new_level.value()
    );
    assert!(
        !new_level.is_full(),
        "衰减后不应再是满权限,实际: {}",
        new_level.value()
    );
}

/// WHY: Restore 操作的 clamp 上界是 1.0(满权限),不是初始注册值。
/// 多次或大量 restore 不能突破 1.0 上限,防止权限越界提升(对应尸检:权限不应自行提升)。
#[test]
fn test_restore_exceeds_limit() {
    // Arrange: 使用高 restore_rate 加速饱和,验证 clamp 而非速率
    let config = DecayConfig {
        time_decay_rate: 1.0,
        event_decay_penalty: 0.1,
        min_level: 0.0,
        freeze_threshold: 0.05,
        restore_rate: 100.0, // 每秒恢复 1000%,短 sleep 即可饱和
    };
    let engine = DecayEngine::new(config);
    engine.register_capability("cap1", "测试能力", 0.5).unwrap();

    // 先降低权限,为 restore 留出空间(0.5 - 0.1×1.0 = 0.4)
    engine
        .decay(
            "cap1",
            DecayEvent::ViolationPenalty {
                capability_id: "cap1".to_string(),
                severity: 1.0,
            },
        )
        .unwrap();
    let before_restore = engine.get_level("cap1").unwrap().value();
    assert!(
        before_restore < 0.5,
        "惩罚后 level 应降低,实际: {}",
        before_restore
    );

    // Act: sleep 50ms,restore 量 = 0.05 × 100 = 5.0,远超 1.0 上限
    std::thread::sleep(std::time::Duration::from_millis(50));
    let new_level = engine
        .decay(
            "cap1",
            DecayEvent::Restore {
                capability_id: "cap1".to_string(),
            },
        )
        .unwrap();

    // Assert: 最终值被 clamp 到 1.0,不能超过上限
    assert!(
        new_level.is_full(),
        "Restore 应被 clamp 到 1.0,实际: {}",
        new_level.value()
    );
    assert!(
        (new_level.value() - 1.0).abs() < 1e-6,
        "Restore 不能超过 1.0 上限,实际: {}",
        new_level.value()
    );
}

/// WHY: 多次 TimeDecay 应保证 level 单调不增,
/// 防止时间驱动衰减出现"恢复"异常(对应尸检:权限不应自行提升)。
/// 原有 test_continuous_decay_curve 仅覆盖 ViolationPenalty,此处补充 TimeDecay。
#[test]
fn test_decay_monotonic_decrease() {
    // Arrange
    let engine = setup_engine();
    engine.register_capability("cap1", "测试能力", 1.0).unwrap();

    let mut prev_level = 1.0f32;

    // Act & Assert: 3 次 TimeDecay,每次后验证 level 单调不增
    for i in 0..3 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        let new_level = engine.decay("cap1", DecayEvent::TimeDecay).unwrap();

        assert!(
            new_level.value() <= prev_level + 1e-6,
            "第 {} 次 TimeDecay 后 level 应单调不增,prev: {}, cur: {}",
            i + 1,
            prev_level,
            new_level.value()
        );
        prev_level = new_level.value();
    }
}

/// WHY: 能力完全衰减并冻结后,通过 unfreeze + Restore 应能恢复,
/// 验证"冻结非终态"的设计(Skeptic 否决权可解除,对应 §5.1 数据流)。
#[test]
fn test_restore_after_full_decay() {
    // Arrange
    let engine = setup_engine();
    engine.register_capability("cap1", "测试能力", 1.0).unwrap();

    // Act 1: 通过严重违规彻底冻结能力(1.0 - 0.1×10 = 0.0,触发自动冻结)
    engine
        .decay(
            "cap1",
            DecayEvent::ViolationPenalty {
                capability_id: "cap1".to_string(),
                severity: 10.0,
            },
        )
        .unwrap();
    assert!(engine.is_frozen("cap1").unwrap(), "严重违规后应自动冻结");

    // Act 2: 解冻并 restore
    engine.unfreeze("cap1").unwrap();
    let after_unfreeze = engine.get_level("cap1").unwrap().value();
    assert!(
        after_unfreeze > 0.0,
        "解冻后 level 应 > 0,实际: {}",
        after_unfreeze
    );

    std::thread::sleep(std::time::Duration::from_millis(50));
    engine
        .decay(
            "cap1",
            DecayEvent::Restore {
                capability_id: "cap1".to_string(),
            },
        )
        .unwrap();

    // Assert: restore 后 level 应高于解冻后的初始值
    let final_level = engine.get_level("cap1").unwrap().value();
    assert!(
        final_level > after_unfreeze,
        "Restore 后 level 应高于解冻初始值,after_unfreeze: {}, final: {}",
        after_unfreeze,
        final_level
    );
}

/// WHY: 对未注册能力调用任何 API 必须返回 CapabilityNotFound,
/// 而非 panic 或静默成功(防止调用方误判能力存在,导致后续操作基于错误假设)。
/// 同时验证重复注册与冻结状态错误的正确处理(幂等保护)。
#[test]
fn test_decay_invalid_input_handling() {
    let engine = setup_engine();
    engine.register_capability("cap1", "测试能力", 0.8).unwrap();

    // --- 错误路径 1: 操作不存在的能力 ---

    // decay 不存在的能力
    let result = engine.decay("nonexistent", DecayEvent::TimeDecay);
    assert!(
        matches!(result, Err(DecayError::CapabilityNotFound(_))),
        "decay 不存在能力应返回 CapabilityNotFound"
    );

    // freeze 不存在的能力
    let result = engine.freeze("nonexistent", "测试");
    assert!(
        matches!(result, Err(DecayError::CapabilityNotFound(_))),
        "freeze 不存在能力应返回 CapabilityNotFound"
    );

    // unfreeze 不存在的能力
    let result = engine.unfreeze("nonexistent");
    assert!(
        matches!(result, Err(DecayError::CapabilityNotFound(_))),
        "unfreeze 不存在能力应返回 CapabilityNotFound"
    );

    // get_level 不存在的能力
    let result = engine.get_level("nonexistent");
    assert!(
        matches!(result, Err(DecayError::CapabilityNotFound(_))),
        "get_level 不存在能力应返回 CapabilityNotFound"
    );

    // is_frozen 不存在的能力
    let result = engine.is_frozen("nonexistent");
    assert!(
        matches!(result, Err(DecayError::CapabilityNotFound(_))),
        "is_frozen 不存在能力应返回 CapabilityNotFound"
    );

    // --- 错误路径 2: 重复注册(防止覆盖已有能力) ---
    let result = engine.register_capability("cap1", "重复注册", 0.5);
    assert!(
        matches!(result, Err(DecayError::ConfigError(_))),
        "重复注册应返回 ConfigError"
    );

    // --- 错误路径 3: 冻结状态错误(幂等保护) ---

    // unfreeze 未冻结的能力
    let result = engine.unfreeze("cap1");
    assert!(
        matches!(result, Err(DecayError::NotFrozen(_))),
        "解冻未冻结能力应返回 NotFrozen"
    );

    // freeze 已冻结的能力
    engine.freeze("cap1", "首次冻结").unwrap();
    let result = engine.freeze("cap1", "再次冻结");
    assert!(
        matches!(result, Err(DecayError::AlreadyFrozen(_))),
        "冻结已冻结能力应返回 AlreadyFrozen"
    );
}
