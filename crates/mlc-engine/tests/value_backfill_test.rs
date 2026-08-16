//! MSCE 双信号价值回填集成测试 — L0 AtomicMemoryCard 协同（v3.4.0 §7.5）
//!
//! 覆盖: 顶层 API 可达性 / 逆向价值传播全链路 / L0 AtomicMemoryCard.value 回填 /
//! α 边界 / proptest 价值传播不变量

#![forbid(unsafe_code)]

use mlc_engine::{DualSignalBackfill, L1Trace, ReflectionScorer};
use nexus_contracts::memory_pyramid::{AtomicCardType, AtomicMemoryCard};
use proptest::prelude::*;

fn trace(reflection: Option<&str>, feedback: Option<f32>) -> L1Trace {
    L1Trace {
        reflection: reflection.map(String::from),
        environmental_feedback: feedback,
        value: None,
    }
}

// ----------------------------------------------------------
// 顶层 API 可达性
// ----------------------------------------------------------

#[test]
fn top_level_api_accessible() {
    use mlc_engine::prelude::*;
    let backfill = DualSignalBackfill::new(0.9);
    let scorer = ReflectionScorer::new();
    let mut traces = vec![trace(Some("反思"), Some(1.0))];
    backfill.backfill_values(&mut traces);
    assert!(traces[0].value.is_some());
    assert!(scorer.score("因为所以") > 0.0);
}

// ----------------------------------------------------------
// 逆向价值传播全链路
// ----------------------------------------------------------

#[test]
fn value_propagation_backward_chain() {
    let backfill = DualSignalBackfill::new(0.9);
    let mut traces = vec![
        trace(None, Some(0.0)), // t0（最早）
        trace(None, Some(0.0)), // t1
        trace(None, Some(1.0)), // t2（最晚，环境奖励）
    ];
    backfill.backfill_values(&mut traces);
    // 全部回填
    for t in &traces {
        assert!(t.value.is_some());
    }
    // 折减累积: 越早价值越低
    let v0 = traces[0].value.unwrap();
    let v1 = traces[1].value.unwrap();
    let v2 = traces[2].value.unwrap();
    assert!(v2 >= v1, "尾部价值 ≥ 中部");
    assert!(v1 >= v0, "中部价值 ≥ 头部");
    assert!(v2 > v0, "尾部价值 > 头部（折减累积）");
}

// ----------------------------------------------------------
// L0 AtomicMemoryCard.value 回填协同
// ----------------------------------------------------------

#[test]
fn backfill_to_l0_atomic_card_value() {
    // 从 L0 AtomicMemoryCard 的 reflection 构造轨迹 → 回填 → 写回 value
    let card = AtomicMemoryCard::new(
        "c1",
        AtomicCardType::Trace,
        100,
        "scene",
        "内容",
        None,
        None,
        Some("因为类型错误，所以学到类型标注"), // reflection
        None,                                   // value 待回填
        1_700_000_000_000,
    );
    let backfill = DualSignalBackfill::new(0.9);
    let mut traces = vec![L1Trace {
        reflection: card.reflection.as_ref().map(|r| r.to_string()),
        environmental_feedback: Some(1.0),
        value: None,
    }];
    backfill.backfill_values(&mut traces);
    let backfilled = traces[0].value.expect("已回填");
    assert!(backfilled > 0.0, "有反思+环境反馈应产生正价值");
    // 回填值可写回 L0 卡片的 value 字段（通过构造新卡片，铁律3 不可变）
    let updated = AtomicMemoryCard::new(
        "c1",
        AtomicCardType::Trace,
        100,
        "scene",
        "内容",
        None,
        None,
        Some("因为类型错误，所以学到类型标注"),
        Some(backfilled),
        1_700_000_000_000,
    );
    assert_eq!(updated.value, Some(backfilled));
}

// ----------------------------------------------------------
// α 边界
// ----------------------------------------------------------

#[test]
fn alpha_boundaries_environment_vs_propagation() {
    let scorer = ReflectionScorer::new();
    // 强反思 → α≈1（主要依赖环境反馈）
    let strong = "因为A所以B，学到C，改进D，原因E，应该F，总结G，反思H，下次I，导致J";
    assert!((scorer.score(strong) - 1.0).abs() < 1e-6);
    // 空反思 → α=0（纯价值传播）
    assert_eq!(scorer.score(""), 0.0);
}

#[test]
fn no_reflection_uses_conservative_default_alpha() {
    let backfill = DualSignalBackfill::new(1.0);
    let mut traces = vec![trace(None, Some(1.0))];
    backfill.backfill_values(&mut traces);
    // α=default_alpha=0.3, γ=1, Vt+1=0 → Vt=0.3
    let v = traces[0].value.unwrap();
    assert!((v - 0.3).abs() < 1e-6);
}

// ----------------------------------------------------------
// proptest: 价值传播不变量
// ----------------------------------------------------------

proptest! {
    /// 任意 γ∈[0,1]，单轨迹 Vt = α·Rt（Vt+1=0），值域 [0, max(Rt,0)]
    #[test]
    fn single_trace_value_bounded(
        gamma in 0.0f32..1.0,
        reward in 0.0f32..1.0,
    ) {
        let backfill = DualSignalBackfill::new(gamma);
        let mut traces = vec![trace(None, Some(reward))];
        backfill.backfill_values(&mut traces);
        let v = traces[0].value.prop_test_unwrap();
        // α∈[0,1], Rt=reward ≥0, Vt+1=0 → Vt = α·reward ∈ [0, reward]
        prop_assert!(v >= 0.0);
        prop_assert!(v <= reward + 1e-6);
    }

    /// 任意长度轨迹，回填后全部 value 有值且有限
    #[test]
    fn all_traces_filled_finite(
        n in 1usize..10,
    ) {
        let backfill = DualSignalBackfill::new(0.9);
        let mut traces: Vec<L1Trace> = (0..n)
            .map(|i| trace(None, Some(i as f32 * 0.1)))
            .collect();
        backfill.backfill_values(&mut traces);
        for t in &traces {
            let v = t.value.prop_test_unwrap();
            prop_assert!(v.is_finite());
        }
    }
}

/// proptest 辅助: 解包 Option<f32>
trait PropTestUnwrap {
    fn prop_test_unwrap(&self) -> f32;
}
impl PropTestUnwrap for Option<f32> {
    fn prop_test_unwrap(&self) -> f32 {
        self.expect("已回填")
    }
}
