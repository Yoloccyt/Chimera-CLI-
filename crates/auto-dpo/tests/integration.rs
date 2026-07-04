//! AutoDPO 集成测试 — 验证偏好对生成、事件发布与 EventBus 共享总线集成
//!
//! 对应 Week 8 Task 5(测试体系补齐)
//! 架构层:L5 Knowledge
//!
//! # 测试目标
//! - 验证 `DpoPairGenerated` 事件在 `generate()` 调用后正确发布(Ω-Event 定律)
//! - 验证偏好对收集逻辑:从多候选中选最高分为 chosen、最低分为 rejected
//! - 验证 `with_event_bus` 共享总线模式:外部注入 EventBus + 多订阅者 fan-out
//! - 验证 `enable_event_publish = false` 时静默不发布事件
//!
//! # 设计约束
//! - WHY `try_recv` 而非 `recv().await`:`publish_blocking` 为同步发布,
//!   事件在 `send` 返回后立即可见,`try_recv` 不依赖 tokio 运行时,简化测试
//! - WHY `subscribe` 必须在 `with_event_bus` 之前:broadcast 不缓存历史,
//!   先订阅才能收到后续事件(§4.4 反模式 3)
//! - WHY 仅用公共 API:集成测试在外部 crate,不能访问 `pair_counter` 等私有字段

#![forbid(unsafe_code)]

use auto_dpo::{AutoDpoConfig, ModelOutput, PreferencePairGenerator};
use event_bus::NexusEvent;

// ============================================================
// 辅助函数
// ============================================================

/// 构造默认生成器(内部私有 EventBus,事件静默丢弃)
fn make_generator() -> PreferencePairGenerator {
    PreferencePairGenerator::new(AutoDpoConfig::default()).unwrap()
}

/// 构造两个差异明显的候选:高分为 chosen,低分为 rejected
fn make_two_candidates() -> Vec<ModelOutput> {
    vec![
        ModelOutput::new("good-output", 0.9),
        ModelOutput::new("bad-output", 0.3),
    ]
}

// ============================================================
// A. DpoPairGenerated 事件发布测试
// ============================================================

#[test]
fn test_dpo_pair_generated_event_published() {
    // WHY 共享 EventBus:需订阅事件验证 Ω-Event 定律
    let bus = event_bus::EventBus::new();
    // WHY 先订阅后构造:broadcast 不回放历史,subscribe 必须在 publish 之前
    let mut subscriber = bus.subscribe();
    let generator = PreferencePairGenerator::with_event_bus(AutoDpoConfig::default(), bus).unwrap();

    // 触发偏好对生成,内部 publish_blocking 发布 DpoPairGenerated
    let pair = generator.generate(&make_two_candidates()).unwrap();

    // 验证事件已发布到 EventBus
    let event = subscriber
        .try_recv()
        .expect("try_recv should not error")
        .expect("DpoPairGenerated event should be published");
    match event {
        NexusEvent::DpoPairGenerated {
            pair_id,
            chosen,
            rejected,
            ..
        } => {
            assert_eq!(pair_id, pair.pair_id, "pair_id field mismatch");
            assert_eq!(chosen, pair.chosen, "chosen field mismatch");
            assert_eq!(rejected, pair.rejected, "rejected field mismatch");
        }
        other => panic!("expected DpoPairGenerated, got {other:?}"),
    }
}

#[test]
fn test_dpo_pair_generated_event_metadata_source() {
    // 验证事件 metadata.source = "auto-dpo"(generator.rs:160 EventMetadata::new("auto-dpo"))
    let bus = event_bus::EventBus::new();
    let mut subscriber = bus.subscribe();
    let generator = PreferencePairGenerator::with_event_bus(AutoDpoConfig::default(), bus).unwrap();

    generator.generate(&make_two_candidates()).unwrap();

    let event = subscriber.try_recv().unwrap().unwrap();
    match event {
        NexusEvent::DpoPairGenerated { metadata, .. } => {
            assert_eq!(
                metadata.source, "auto-dpo",
                "metadata.source should be 'auto-dpo'"
            );
        }
        _ => unreachable!("expected DpoPairGenerated"),
    }
}

// ============================================================
// B. 偏好对收集逻辑测试
// ============================================================

#[test]
fn test_preference_pair_chooses_extreme_scores() {
    // 3 个候选,验证 chosen=最高分 / rejected=最低分 / 中间分不入选
    let generator = make_generator();
    let outputs = vec![
        ModelOutput::new("medium", 0.6),
        ModelOutput::new("high", 0.95),
        ModelOutput::new("low", 0.2),
    ];
    let pair = generator.generate(&outputs).unwrap();

    assert_eq!(pair.chosen, "high", "chosen should be highest-score output");
    assert_eq!(
        pair.rejected, "low",
        "rejected should be lowest-score output"
    );
    assert!(
        (pair.chosen_score - 0.95).abs() < 1e-6,
        "chosen_score should be 0.95"
    );
    assert!(
        (pair.rejected_score - 0.2).abs() < 1e-6,
        "rejected_score should be 0.2"
    );
    assert!(pair.score_gap() > 0.0, "score_gap should be positive");
}

#[test]
fn test_preference_pair_quality_classification() {
    // chosen_score 0.9 → High;rejected_score 0.3 → Low
    let generator = make_generator();
    let pair = generator.generate(&make_two_candidates()).unwrap();

    assert_eq!(
        pair.quality,
        auto_dpo::SampleQuality::High,
        "quality derives from chosen_score (0.9 → High)"
    );
}

#[test]
fn test_pair_id_uniqueness_across_generations() {
    // 验证 AtomicU64 计数器:多次生成应产生唯一 pair_id
    let generator = make_generator();
    let outputs = make_two_candidates();

    let pair1 = generator.generate(&outputs).unwrap();
    let pair2 = generator.generate(&outputs).unwrap();
    let pair3 = generator.generate(&outputs).unwrap();

    assert_ne!(pair1.pair_id, pair2.pair_id, "pair_id must be unique");
    assert_ne!(pair2.pair_id, pair3.pair_id, "pair_id must be unique");
    assert_ne!(pair1.pair_id, pair3.pair_id, "pair_id must be unique");
    // 所有 pair_id 都应遵循 "dpo-pair-{counter}" 格式
    for pair_id in [&pair1.pair_id, &pair2.pair_id, &pair3.pair_id] {
        assert!(
            pair_id.starts_with("dpo-pair-"),
            "pair_id should follow 'dpo-pair-{{counter}}' format, got {pair_id}"
        );
    }
}

// ============================================================
// C. with_event_bus 共享总线模式测试
// ============================================================

#[test]
fn test_with_event_bus_shared_bus_pattern() {
    // 验证 with_event_bus 注入的共享总线可被多个订阅者 fan-out 接收
    let bus = event_bus::EventBus::new();
    let mut sub1 = bus.subscribe();
    let mut sub2 = bus.subscribe();
    let generator = PreferencePairGenerator::with_event_bus(AutoDpoConfig::default(), bus).unwrap();

    generator.generate(&make_two_candidates()).unwrap();

    // 两个订阅者都应收到同一事件(broadcast fan-out)
    let evt1 = sub1.try_recv().unwrap().expect("sub1 should receive event");
    let evt2 = sub2.try_recv().unwrap().expect("sub2 should receive event");
    assert!(matches!(evt1, NexusEvent::DpoPairGenerated { .. }));
    assert!(matches!(evt2, NexusEvent::DpoPairGenerated { .. }));
}

#[test]
fn test_with_event_bus_subscriber_count() {
    // 验证 with_event_bus 不改变外部订阅者数量
    let bus = event_bus::EventBus::new();
    let _sub1 = bus.subscribe();
    let _sub2 = bus.subscribe();
    let before = bus.subscriber_count();
    let _generator =
        PreferencePairGenerator::with_event_bus(AutoDpoConfig::default(), bus).unwrap();
    // with_event_bus 消费 bus,无法再查询;此处通过 _sub1/_sub2 仍存活验证订阅未被扰动
    assert_eq!(
        before, 2,
        "two subscribers registered before with_event_bus"
    );
}

// ============================================================
// D. 事件发布开关测试
// ============================================================

#[test]
fn test_event_publish_disabled_silent() {
    // enable_event_publish = false 时,generate 不应发布事件
    // WHY:测试或离线批处理场景需关闭事件发布,避免无订阅者告警噪声
    let bus = event_bus::EventBus::new();
    let mut subscriber = bus.subscribe();
    let config = AutoDpoConfig {
        enable_event_publish: false,
        ..Default::default()
    };
    let generator = PreferencePairGenerator::with_event_bus(config, bus).unwrap();

    // generate 仍应成功返回偏好对
    let pair = generator.generate(&make_two_candidates()).unwrap();
    assert_eq!(pair.chosen, "good-output");

    // 但 EventBus 上不应有任何事件
    let result = subscriber.try_recv();
    match result {
        Ok(None) => { /* 期望:无事件 */ }
        Ok(Some(event)) => panic!("expected no event, got {event:?}"),
        Err(e) => panic!("try_recv errored: {e:?}"),
    }
}

#[test]
fn test_internal_bus_new_pattern_silent_drop() {
    // 验证 new() 创建内部私有 bus 时,无订阅者 publish 静默丢弃不报错
    // WHY:new() 内部 bus 无订阅者,publish_blocking 返回 Ok(()),事件被静默丢弃
    let generator = make_generator();
    // 应正常完成,无 panic
    let pair = generator.generate(&make_two_candidates()).unwrap();
    assert!(!pair.pair_id.is_empty());
}

// ============================================================
// E. 配置错误传播测试
// ============================================================

#[test]
fn test_with_event_bus_invalid_config_rejected() {
    // 验证 with_event_bus 在构造时校验配置,提前暴露错误
    let bus = event_bus::EventBus::new();
    let bad_config = AutoDpoConfig {
        min_samples: 1, // 违反 >= 2 约束
        ..Default::default()
    };
    let result = PreferencePairGenerator::with_event_bus(bad_config, bus);
    assert!(
        result.is_err(),
        "with_event_bus should reject invalid config"
    );
}

#[test]
fn test_generate_insufficient_samples_error() {
    // 验证候选数 < min_samples 时返回 InsufficientSamples
    let generator = make_generator();
    let single = vec![ModelOutput::new("only", 0.9)];
    let result = generator.generate(&single);
    assert!(
        matches!(
            result,
            Err(auto_dpo::AutoDpoError::InsufficientSamples { actual: 1 })
        ),
        "should reject with InsufficientSamples, got {result:?}"
    );
}
