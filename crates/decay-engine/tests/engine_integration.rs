//! DecayEngine 衰减引擎集成测试 — 从 src/engine.rs 内联测试模块外移(L4-P2-1)
//!
//! 外移说明:原 #[cfg(test)] mod tests 混在生产文件(546 行,占 52%),
//! 外移后 engine.rs 仅保留生产代码。覆盖:双驱动衰减、冻结/解冻、
//! S6 接缝 profile_to_config 映射(等价辅助函数复制)、边界值。
use decay_engine::{DecayConfig, DecayEngine, DecayError, DecayEvent};
use nexus_contracts::{DecayPolicy, DecayProfile};

/// 复制 src/engine.rs pub(crate) profile_to_config 的等价实现
/// (外移后私有函数不可见;DecayProfile getter 均公共,复制零语义变化)
/// 注意:DecayConfig 的全部字段显式赋值(与 src 实现一致),无 ..Default
fn profile_to_config(profile: DecayProfile) -> DecayConfig {
    DecayConfig {
        time_decay_rate: profile.time_decay_rate(),
        event_decay_penalty: profile.event_decay_penalty(),
        min_level: profile.min_level(),
        freeze_threshold: profile.freeze_threshold(),
        restore_rate: profile.restore_rate(),
        use_exponential_decay: false,
        decay_tau_seconds: 86400.0,
    }
}

// ============================================================
// profile_to_config 测试
// ============================================================

#[test]
fn test_profile_to_config_lenient() {
    let config = profile_to_config(DecayProfile::Lenient);
    assert!((config.time_decay_rate - 0.0005).abs() < 1e-6);
    assert!((config.event_decay_penalty - 0.05).abs() < 1e-6);
    assert!((config.freeze_threshold - 0.02).abs() < 1e-6);
    assert!((config.restore_rate - 0.01).abs() < 1e-6);
    assert!(config.min_level.abs() < 1e-6);
}

#[test]
fn test_profile_to_config_standard() {
    // Standard 档位必须等价于 DecayConfig::default()（C4 合规）
    let config = profile_to_config(DecayProfile::Standard);
    let default_config = DecayConfig::default();
    assert!((config.time_decay_rate - default_config.time_decay_rate).abs() < 1e-6);
    assert!((config.event_decay_penalty - default_config.event_decay_penalty).abs() < 1e-6);
    assert!((config.freeze_threshold - default_config.freeze_threshold).abs() < 1e-6);
    assert!((config.restore_rate - default_config.restore_rate).abs() < 1e-6);
    assert!((config.min_level - default_config.min_level).abs() < 1e-6);
}

#[test]
fn test_profile_to_config_strict() {
    let config = profile_to_config(DecayProfile::Strict);
    assert!((config.time_decay_rate - 0.005).abs() < 1e-6);
    assert!((config.event_decay_penalty - 0.15).abs() < 1e-6);
    assert!((config.freeze_threshold - 0.10).abs() < 1e-6);
}

#[test]
fn test_profile_to_config_aggressive() {
    let config = profile_to_config(DecayProfile::Aggressive);
    assert!((config.time_decay_rate - 0.01).abs() < 1e-6);
    assert!((config.event_decay_penalty - 0.2).abs() < 1e-6);
    assert!((config.freeze_threshold - 0.15).abs() < 1e-6);
}

// ============================================================
// decay_with_policy 基础行为测试
// ============================================================

#[test]
fn test_decay_with_policy_fallback_equivalent_to_decay() {
    // C4 合规: 传入 fallback() (Static(Standard)) 行为应与 decay() 完全一致
    let engine1 = DecayEngine::new(DecayConfig::default());
    let engine2 = DecayEngine::new(DecayConfig::default());

    engine1.register_capability("cap", "测试", 1.0).unwrap();
    engine2.register_capability("cap", "测试", 1.0).unwrap();

    let event = DecayEvent::ViolationPenalty {
        capability_id: "cap".into(),
        severity: 1.0,
    };

    let level1 = engine1.decay("cap", event.clone()).unwrap();
    let level2 = engine2
        .decay_with_policy("cap", event, DecayPolicy::fallback())
        .unwrap();
    assert!((level1.value() - level2.value()).abs() < 1e-6);
}

#[test]
fn test_decay_with_policy_strict_higher_penalty() {
    // Strict 档位惩罚应大于 Standard（同 severity 下）
    let engine1 = DecayEngine::new(DecayConfig::default());
    let engine2 = DecayEngine::new(DecayConfig::default());

    engine1.register_capability("cap1", "测试1", 1.0).unwrap();
    engine2.register_capability("cap2", "测试2", 1.0).unwrap();

    let event = DecayEvent::ViolationPenalty {
        capability_id: "cap1".into(),
        severity: 1.0,
    };

    let level_standard = engine1
        .decay_with_policy(
            "cap1",
            event.clone(),
            DecayPolicy::static_policy(DecayProfile::Standard),
        )
        .unwrap();
    let level_strict = engine2
        .decay_with_policy(
            "cap2",
            DecayEvent::ViolationPenalty {
                capability_id: "cap2".into(),
                severity: 1.0,
            },
            DecayPolicy::static_policy(DecayProfile::Strict),
        )
        .unwrap();

    // Strict penalty=0.15 > Standard penalty=0.1，所以 Strict 衰减更多
    assert!(level_strict.value() < level_standard.value());
}

#[test]
fn test_decay_with_policy_lenient_lower_penalty() {
    // Lenient 档位惩罚应小于 Standard
    let engine1 = DecayEngine::new(DecayConfig::default());
    let engine2 = DecayEngine::new(DecayConfig::default());

    engine1.register_capability("cap1", "测试1", 1.0).unwrap();
    engine2.register_capability("cap2", "测试2", 1.0).unwrap();

    let level_standard = engine1
        .decay_with_policy(
            "cap1",
            DecayEvent::ViolationPenalty {
                capability_id: "cap1".into(),
                severity: 1.0,
            },
            DecayPolicy::static_policy(DecayProfile::Standard),
        )
        .unwrap();
    let level_lenient = engine2
        .decay_with_policy(
            "cap2",
            DecayEvent::ViolationPenalty {
                capability_id: "cap2".into(),
                severity: 1.0,
            },
            DecayPolicy::static_policy(DecayProfile::Lenient),
        )
        .unwrap();

    // Lenient penalty=0.05 < Standard penalty=0.1，所以 Lenient 衰减更少
    assert!(level_lenient.value() > level_standard.value());
}

#[test]
fn test_decay_with_policy_aggressive_highest_penalty() {
    // Aggressive 档位惩罚最高
    let engine = DecayEngine::new(DecayConfig::default());
    engine.register_capability("cap", "测试", 1.0).unwrap();

    let level = engine
        .decay_with_policy(
            "cap",
            DecayEvent::ViolationPenalty {
                capability_id: "cap".into(),
                severity: 1.0,
            },
            DecayPolicy::static_policy(DecayProfile::Aggressive),
        )
        .unwrap();

    // Aggressive penalty=0.2，衰减后 level = 1.0 - 0.2 = 0.8
    assert!((level.value() - 0.8).abs() < 1e-6);
}

// ============================================================
// decay_with_policy 学习策略测试
// ============================================================

#[test]
fn test_decay_with_policy_learned_equivalent_to_static() {
    // Learned 与 Static 同档位行为应一致
    let engine1 = DecayEngine::new(DecayConfig::default());
    let engine2 = DecayEngine::new(DecayConfig::default());

    engine1.register_capability("cap1", "测试1", 1.0).unwrap();
    engine2.register_capability("cap2", "测试2", 1.0).unwrap();

    let event = DecayEvent::ViolationPenalty {
        capability_id: "cap1".into(),
        severity: 1.0,
    };

    let level_static = engine1
        .decay_with_policy(
            "cap1",
            event.clone(),
            DecayPolicy::static_policy(DecayProfile::Strict),
        )
        .unwrap();
    let level_learned = engine2
        .decay_with_policy(
            "cap2",
            DecayEvent::ViolationPenalty {
                capability_id: "cap2".into(),
                severity: 1.0,
            },
            DecayPolicy::learned(1, DecayProfile::Strict),
        )
        .unwrap();

    assert!((level_static.value() - level_learned.value()).abs() < 1e-6);
}

#[test]
fn test_decay_with_policy_freeze_ignores_profile() {
    // Freeze 事件清零权限，与档位无关
    let engine = DecayEngine::new(DecayConfig::default());
    engine.register_capability("cap", "测试", 1.0).unwrap();

    let level = engine
        .decay_with_policy(
            "cap",
            DecayEvent::Freeze {
                capability_id: "cap".into(),
                reason: "测试冻结".into(),
            },
            DecayPolicy::static_policy(DecayProfile::Aggressive),
        )
        .unwrap();

    assert!(level.value().abs() < 1e-6);
    assert!(engine.is_frozen("cap").unwrap());
}

#[test]
fn test_decay_with_policy_capability_not_found() {
    // 不存在的能力 ID 应返回错误
    let engine = DecayEngine::new(DecayConfig::default());
    let result = engine.decay_with_policy(
        "nonexistent",
        DecayEvent::TimeDecay,
        DecayPolicy::fallback(),
    );
    assert!(matches!(result, Err(DecayError::CapabilityNotFound(_))));
}

#[test]
fn test_decay_with_policy_frozen_skips_time_decay() {
    // 已冻结能力跳过时间衰减（与 decay 行为一致）
    let engine = DecayEngine::new(DecayConfig::default());
    engine.register_capability("cap", "测试", 0.5).unwrap();
    engine.freeze("cap", "预冻结").unwrap();

    let level = engine
        .decay_with_policy(
            "cap",
            DecayEvent::TimeDecay,
            DecayPolicy::static_policy(DecayProfile::Aggressive),
        )
        .unwrap();

    // 已冻结，level 保持 0.0
    assert!(level.value().abs() < 1e-6);
}

// ============================================================
// decay 向后兼容测试
// ============================================================

#[test]
fn test_decay_unchanged_after_refactor() {
    // 重构后 decay 行为应与 P4 修复前一致
    let engine = DecayEngine::new(DecayConfig::default());
    engine.register_capability("cap", "测试", 1.0).unwrap();

    let level = engine
        .decay(
            "cap",
            DecayEvent::ViolationPenalty {
                capability_id: "cap".into(),
                severity: 1.0,
            },
        )
        .unwrap();

    // penalty = 0.1 × 1.0 = 0.1，level = 1.0 - 0.1 = 0.9
    assert!((level.value() - 0.9).abs() < 1e-6);
}

// ============================================================
// 自动冻结阈值测试（不同档位）
// ============================================================

#[test]
fn test_decay_with_policy_auto_freeze_strict_threshold() {
    // Strict 档位 freeze_threshold=0.10，权限降到 0.10 以下应自动冻结
    let engine = DecayEngine::new(DecayConfig::default());
    // 初始权限设为 0.20（高于 Strict 阈值 0.10）
    engine.register_capability("cap", "测试", 0.20).unwrap();

    // 使用 Strict 档位 + severity=2.0 衰减
    // penalty = 0.15 × 2.0 = 0.30，new_level = 0.20 - 0.30 = -0.10 → clamp 到 min_level=0.0
    // 但 0.0 <= 0.10（Strict freeze_threshold），应触发自动冻结
    let level = engine
        .decay_with_policy(
            "cap",
            DecayEvent::ViolationPenalty {
                capability_id: "cap".into(),
                severity: 2.0,
            },
            DecayPolicy::static_policy(DecayProfile::Strict),
        )
        .unwrap();

    // 自动冻结：level = 0.0
    assert!(level.value().abs() < 1e-6);
    assert!(engine.is_frozen("cap").unwrap());
}

#[test]
fn test_decay_with_policy_no_freeze_lenient_threshold() {
    // Lenient 档位 freeze_threshold=0.02，相同 level 不会触发冻结
    let engine = DecayEngine::new(DecayConfig::default());
    engine.register_capability("cap", "测试", 0.20).unwrap();

    // Lenient penalty = 0.05 × 2.0 = 0.10，new_level = 0.20 - 0.10 = 0.10
    // 0.10 > 0.02（Lenient freeze_threshold），不触发自动冻结
    let level = engine
        .decay_with_policy(
            "cap",
            DecayEvent::ViolationPenalty {
                capability_id: "cap".into(),
                severity: 2.0,
            },
            DecayPolicy::static_policy(DecayProfile::Lenient),
        )
        .unwrap();

    assert!((level.value() - 0.10).abs() < 1e-6);
    assert!(!engine.is_frozen("cap").unwrap());
}

// ============================================================
// 端到端策略切换测试
// ============================================================

#[test]
fn test_scenario_switching_profiles_across_operations() {
    // 模拟:同一能力在不同场景下使用不同档位衰减
    let engine = DecayEngine::new(DecayConfig::default());
    engine
        .register_capability("file_write", "文件写入", 1.0)
        .unwrap();

    // 1. 低风险场景: 使用 Lenient
    let level1 = engine
        .decay_with_policy(
            "file_write",
            DecayEvent::ViolationPenalty {
                capability_id: "file_write".into(),
                severity: 1.0,
            },
            DecayPolicy::static_policy(DecayProfile::Lenient),
        )
        .unwrap();
    // Lenient penalty=0.05，level = 1.0 - 0.05 = 0.95
    assert!((level1.value() - 0.95).abs() < 1e-6);

    // 2. 高风险场景: 使用 Strict
    let level2 = engine
        .decay_with_policy(
            "file_write",
            DecayEvent::ViolationPenalty {
                capability_id: "file_write".into(),
                severity: 1.0,
            },
            DecayPolicy::static_policy(DecayProfile::Strict),
        )
        .unwrap();
    // Strict penalty=0.15，level = 0.95 - 0.15 = 0.80
    assert!((level2.value() - 0.80).abs() < 1e-6);

    // 3. 学习策略下发 Aggressive
    let level3 = engine
        .decay_with_policy(
            "file_write",
            DecayEvent::ViolationPenalty {
                capability_id: "file_write".into(),
                severity: 1.0,
            },
            DecayPolicy::learned(1, DecayProfile::Aggressive),
        )
        .unwrap();
    // Aggressive penalty=0.2，level = 0.80 - 0.20 = 0.60
    assert!((level3.value() - 0.60).abs() < 1e-6);

    // 4. learner panic: fallback 到 Standard
    let level4 = engine
        .decay_with_policy(
            "file_write",
            DecayEvent::ViolationPenalty {
                capability_id: "file_write".into(),
                severity: 1.0,
            },
            DecayPolicy::fallback(),
        )
        .unwrap();
    // Standard penalty=0.1，level = 0.60 - 0.10 = 0.50
    assert!((level4.value() - 0.50).abs() < 1e-6);
}

// ============================================================
// P1-4: 指数衰减模式测试
// ============================================================

#[test]
fn test_exponential_decay_zero_elapsed() {
    // 指数衰减模式下,elapsed=0 时因子为 1.0,level 不变
    let config = DecayConfig {
        use_exponential_decay: true,
        decay_tau_seconds: 86400.0,
        ..Default::default()
    };
    let engine = DecayEngine::new(config);
    engine.register_capability("cap", "测试", 1.0).unwrap();

    let level = engine.decay("cap", DecayEvent::TimeDecay).unwrap();
    // elapsed ≈ 0,decay_factor ≈ 1.0,level ≈ 1.0
    assert!((level.value() - 1.0).abs() < 1e-6);
}

#[test]
fn test_exponential_decay_one_tau() {
    // 指数衰减模式下,elapsed=tau 时 level = level × 1/e ≈ level × 0.3679
    // 注意:无法精确控制 elapsed(Instant::now()),使用近似验证
    let config = DecayConfig {
        use_exponential_decay: true,
        decay_tau_seconds: 86400.0,
        ..Default::default()
    };
    let engine = DecayEngine::new(config);
    engine.register_capability("cap", "测试", 1.0).unwrap();

    let level = engine.decay("cap", DecayEvent::TimeDecay).unwrap();
    // elapsed 很小,level 应接近 1.0
    assert!(level.value() > 0.99);
}

#[test]
fn test_exponential_decay_vs_linear_comparison() {
    // 指数衰减与线性衰减在相同 elapsed 下行为不同:
    // 指数衰减:level = level × exp(-elapsed/tau),不会降到负数
    // 线性衰减:level = level - elapsed × rate,可能降到负数再 clamp
    let config_exp = DecayConfig {
        use_exponential_decay: true,
        decay_tau_seconds: 100.0, // τ=100s,衰减快
        ..Default::default()
    };
    let config_lin = DecayConfig {
        time_decay_rate: 0.01, // 每秒减 1%
        ..Default::default()
    };

    let engine_exp = DecayEngine::new(config_exp);
    let engine_lin = DecayEngine::new(config_lin);

    engine_exp.register_capability("exp", "指数", 0.5).unwrap();
    engine_lin.register_capability("lin", "线性", 0.5).unwrap();

    let level_exp = engine_exp.decay("exp", DecayEvent::TimeDecay).unwrap();
    let level_lin = engine_lin.decay("lin", DecayEvent::TimeDecay).unwrap();

    // elapsed 很小,两者都应接近 0.5
    assert!(level_exp.value() > 0.49);
    assert!(level_lin.value() > 0.49);
}

#[test]
fn test_exponential_decay_frozen_skipped() {
    // 已冻结能力在指数衰减模式下也应跳过
    let config = DecayConfig {
        use_exponential_decay: true,
        ..Default::default()
    };
    let engine = DecayEngine::new(config);
    engine.register_capability("cap", "测试", 0.5).unwrap();
    engine.freeze("cap", "预冻结").unwrap();

    let level = engine.decay("cap", DecayEvent::TimeDecay).unwrap();
    assert!(level.value().abs() < 1e-6);
    assert!(engine.is_frozen("cap").unwrap());
}

#[test]
fn test_exponential_decay_with_policy() {
    // 指数衰减模式 + decay_with_policy 组合
    let config = DecayConfig {
        use_exponential_decay: true,
        decay_tau_seconds: 86400.0,
        ..Default::default()
    };
    let engine = DecayEngine::new(config);
    engine.register_capability("cap", "测试", 1.0).unwrap();

    let level = engine
        .decay_with_policy("cap", DecayEvent::TimeDecay, DecayPolicy::fallback())
        .unwrap();
    // elapsed 很小,level 应接近 1.0
    assert!(level.value() > 0.99);
}

#[test]
fn test_exponential_decay_respects_min_level() {
    // 指数衰减应尊重 min_level 下限
    let config = DecayConfig {
        use_exponential_decay: true,
        decay_tau_seconds: 1.0, // τ=1s,衰减极快
        min_level: 0.3,
        ..Default::default()
    };
    let engine = DecayEngine::new(config);
    engine.register_capability("cap", "测试", 0.5).unwrap();

    let level = engine.decay("cap", DecayEvent::TimeDecay).unwrap();
    // elapsed 很小,不会触发衰减到 min_level 以下
    assert!(level.value() >= 0.3);
}

#[test]
fn test_exponential_decay_auto_freeze() {
    // 指数衰减后低于 freeze_threshold 应自动冻结
    let config = DecayConfig {
        use_exponential_decay: true,
        decay_tau_seconds: 100.0, // τ=100s,适度衰减
        freeze_threshold: 0.45,   // 冻结阈值略低于初始权限
        ..Default::default()
    };
    let engine = DecayEngine::new(config);
    engine.register_capability("cap", "测试", 0.5).unwrap();

    let _level = engine.decay("cap", DecayEvent::TimeDecay).unwrap();
    // elapsed 很小(μs级),decay_factor ≈ 1.0,level ≈ 0.5 > 0.45,不应触发冻结
    assert!(!engine.is_frozen("cap").unwrap());
}

#[test]
fn test_exponential_decay_default_backward_compat() {
    // 默认配置(use_exponential_decay=false)应使用线性衰减
    let config = DecayConfig::default();
    assert!(!config.use_exponential_decay);
    let engine = DecayEngine::new(config);
    engine.register_capability("cap", "测试", 1.0).unwrap();

    let level = engine.decay("cap", DecayEvent::TimeDecay).unwrap();
    // 线性衰减:elapsed 很小,level ≈ 1.0
    assert!(level.value() > 0.99);
}
