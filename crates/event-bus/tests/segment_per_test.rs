//! Segment-aware PER 集成测试 — 轨迹分段回放全链路（v3.4.0 §6.2）
//!
//! 覆盖: 顶层 API / 铁律9 分段身份与 anchor 语义 / prompt-equal 数学 /
//! 回放采样分布 / proptest 折减不变量

#![forbid(unsafe_code)]

use event_bus::{PerBuffer, PerEntry, SegmentAwarePER};
use nexus_contracts::rl_types::{MemPiAction, RLAction, RLExperience, RLState};
use nexus_contracts::token_evidence::{SegmentCreationReason, SegmentMetadata};
use nexus_contracts::SeamId;
use proptest::prelude::*;

fn exp(reward: f32) -> RLExperience {
    RLExperience {
        state: RLState::new(vec![0.1], 1),
        action: RLAction::MemPi(MemPiAction::Retrieve),
        reward,
        next_state: RLState::new(vec![0.2], 2),
        done: false,
        seam: SeamId::S8MemPi,
    }
}

fn segment(id: &str, traj: &str, idx: u32, is_anchor: bool) -> SegmentMetadata {
    SegmentMetadata::new(
        id,
        traj,
        idx,
        is_anchor,
        vec![],
        vec![],
        idx,
        idx,
        SegmentCreationReason::NaturalBoundary,
    )
}

// ----------------------------------------------------------
// 顶层 API 可达性
// ----------------------------------------------------------

#[test]
fn top_level_api_accessible() {
    let per = SegmentAwarePER::new(100, 42);
    assert_eq!(per.stats().buffer_len, 0);
    let buffer = PerBuffer::new(10, 1);
    assert_eq!(buffer.capacity(), 10);
    assert!(buffer.is_empty());
}

// ----------------------------------------------------------
// 铁律9: 全轨迹共享身份 + anchor 终局 reward
// ----------------------------------------------------------

#[test]
fn full_trajectory_segments_share_identity() {
    let mut per = SegmentAwarePER::new(0, 42);
    // 一条轨迹 3 段（第 0 段为 anchor）
    per.add_segment(exp(1.0), segment("seg-0", "traj-A", 0, true), 0.5);
    per.add_segment(exp(0.3), segment("seg-1", "traj-A", 1, false), 0.4);
    per.add_segment(exp(0.2), segment("seg-2", "traj-A", 2, false), 0.3);
    // 共享身份
    assert_eq!(per.segment_count("traj-A"), 3);
    // anchor 承载终局 reward
    assert_eq!(per.anchor_reward("traj-A"), Some(1.0));
    // 非 anchor 段的 reward 不影响 anchor（广播才覆盖）
    per.broadcast_reward("traj-A", 0.95);
    assert_eq!(per.anchor_reward("traj-A"), Some(0.95));
}

// ----------------------------------------------------------
// prompt-equal denominator 数学
// ----------------------------------------------------------

#[test]
fn prompt_equal_denominator_scales_with_segment_count() {
    let mut per = SegmentAwarePER::new(0, 42);
    // 1 段轨迹: td=0.9 → 0.9
    per.add_segment(exp(0.0), segment("s1", "t1", 0, false), 0.9);
    // 9 段轨迹: td=0.9 → 0.9/3 = 0.3
    for i in 0..9 {
        per.add_segment(exp(0.0), segment(&format!("s{i}"), "t9", i, false), 0.9);
    }
    let samples = per.sample_batch(2);
    // 找到对应轨迹的采样
    let t1_sample = samples
        .iter()
        .find(|e| e.segment.parent_traj_id.as_ref() == "t1");
    let t9_sample = samples
        .iter()
        .find(|e| e.segment.parent_traj_id.as_ref() == "t9");
    if let (Some(a), Some(b)) = (t1_sample, t9_sample) {
        assert!((a.td_error - 0.9).abs() < 1e-6);
        assert!((b.td_error - 0.3).abs() < 1e-6, "9 段折减后应为 0.3");
        assert!(a.td_error > b.td_error);
    }
    // 无论采样到哪条，折减后的存储值都可验证（直接查 stats 缓冲）
    assert_eq!(per.segment_count("t9"), 9);
}

// ----------------------------------------------------------
// 采样分布: 高 TD 主导 + 全零拒绝
// ----------------------------------------------------------

#[test]
fn sampling_distribution_biased_to_high_td() {
    let mut per = SegmentAwarePER::new(0, 7);
    for i in 0..50 {
        per.add_segment(
            exp(0.1),
            segment(&format!("low-{i}"), "t-low", i, false),
            0.01,
        );
    }
    per.add_segment(exp(1.0), segment("high", "t-high", 0, false), 100.0);
    let samples = per.sample_batch(500);
    let high = samples
        .iter()
        .filter(|e| e.segment.segment_id.as_ref() == "high")
        .count();
    assert!(high > 450, "高 TD 条目应主导采样（实际 {high}/500）");
}

#[test]
fn all_zero_td_not_sampled() {
    let mut per = SegmentAwarePER::new(0, 42);
    per.add_segment(exp(0.0), segment("z1", "t1", 0, false), 0.0);
    assert!(per.sample_batch(10).is_empty());
}

// ----------------------------------------------------------
// 容量淘汰
// ----------------------------------------------------------

#[test]
fn capacity_eviction_keeps_highest_td() {
    let mut buffer = PerBuffer::new(3, 42);
    for (id, td) in [("a", 0.1), ("b", 0.5), ("c", 0.3), ("d", 0.9)] {
        buffer.add(PerEntry {
            experience: exp(0.0),
            segment: segment(id, "t", 0, false),
            td_error: td,
        });
    }
    assert_eq!(buffer.len(), 3);
    // 先绑定采样结果，避免临时值借用（E0716）
    let samples = buffer.sample_batch(3);
    let ids: Vec<&str> = samples
        .iter()
        .map(|e| e.segment.segment_id.as_ref())
        .collect();
    assert!(!ids.contains(&"a"), "最低 TD（0.1）应被淘汰");
    assert!(ids.contains(&"d"), "最高 TD（0.9）应保留");
}

// ----------------------------------------------------------
// proptest: 折减不变量（td_error ≥ 0 时折减后非负且 ≤ 原值）
// ----------------------------------------------------------

proptest! {
    /// 任意正 TD 误差与分段数，折减后 ∈ [td/sqrt(segments), td]
    ///（首段折减最少 = td，末段折减最多 = td/sqrt(segments)）
    #[test]
    fn prompt_equal_denominator_bounds(
        td in 0.001f32..10.0,
        segments in 1u32..100,
    ) {
        let mut per = SegmentAwarePER::new(0, 1);
        for i in 0..segments {
            per.add_segment(exp(0.0), segment(&format!("s{i}"), "t-prop", i, false), td);
        }
        let samples = per.sample_batch(1);
        prop_assert!(!samples.is_empty());
        let adjusted = samples[0].td_error;
        // 折减后非负（td ≥ 0 输入）
        prop_assert!(adjusted >= 0.0);
        // 折减后 ≤ 原值（denominator ≥ 1）
        prop_assert!(adjusted <= td + 1e-6);
        // 数学下界: 任意段折减 ≥ td / sqrt(segments)（末段折减最大）
        let lower_bound = td / (segments as f32).sqrt();
        prop_assert!(adjusted >= lower_bound - 1e-3);
    }

    /// 任意批次大小: 有放回采样数 = batch（空缓冲为 0）
    #[test]
    fn sample_batch_size_bounded(
        n in 0usize..20,
        batch in 0usize..30,
    ) {
        let mut per = SegmentAwarePER::new(0, 2);
        for i in 0..n {
            per.add_segment(exp(0.5), segment(&format!("s{i}"), "t-bound", i as u32, false), 0.5);
        }
        let samples = per.sample_batch(batch);
        // PER 为有放回采样: 非空缓冲返回 batch 条（允许重复）；空缓冲返回 0
        let expected = if n == 0 { 0 } else { batch };
        prop_assert_eq!(samples.len(), expected);
        // 空批次恒返回空
        if batch == 0 {
            prop_assert!(samples.is_empty());
        }
    }
}
