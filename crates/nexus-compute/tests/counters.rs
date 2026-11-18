//! `KindCounters` 归因契约 — 定长原子数组按 `TaskKind` 计数、并发守恒、口径隔离
//!
//! 对应 ADR-197 / ADR-222 D3:计数器是有状态遥测真值源,热路径零锁零分配。
//! 本文件从 crate 外验证:计数落点正确（不串槽）、并发自增不丢、批粒度与项粒度
//! 两个口径不可互推,以及归因决胜规则的确定性。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code

use std::sync::Arc;

use nexus_compute::{KindCounters, TaskKind};

#[test]
fn counters_attribute_each_kind_without_cross_talk() {
    // 契约:计数落点由 kind_index 决定 —— 三类各自独立,未计数的类保持 0
    let counters = KindCounters::default();
    for _ in 0..3 {
        counters.record_spawn(TaskKind::OsaMask);
    }
    counters.record_spawn(TaskKind::KnnSearch);
    counters.record_failure(TaskKind::KnnSearch);

    assert_eq!(counters.spawns_of(TaskKind::OsaMask), 3);
    assert_eq!(counters.spawns_of(TaskKind::KnnSearch), 1);
    assert_eq!(counters.failures_of(TaskKind::KnnSearch), 1);
    assert_eq!(
        counters.failures_of(TaskKind::OsaMask),
        0,
        "spawn 不得串到 failures"
    );
    assert_eq!(counters.spawns_of(TaskKind::Generic), 0, "未计数类必须为 0");
}

#[test]
fn concurrent_spawn_increments_are_exactly_conserved() {
    // 契约:并发自增零丢失（Relaxed 原子计数足够——这里验证的是总量守恒,不是顺序）
    const THREADS: usize = 8;
    const PER_THREAD: usize = 500;
    let counters = Arc::new(KindCounters::default());
    let handles: Vec<_> = (0..THREADS)
        .map(|_| {
            let c = Arc::clone(&counters);
            std::thread::spawn(move || {
                for _ in 0..PER_THREAD {
                    c.record_spawn(TaskKind::DecayBatch);
                }
            })
        })
        .collect();
    for h in handles {
        h.join().expect("计数线程应正常退出");
    }
    let total = (THREADS * PER_THREAD) as u64;
    assert_eq!(
        counters.spawns_of(TaskKind::DecayBatch),
        total,
        "并发自增总量必须精确守恒"
    );
    let snapshot_sum: u64 = counters.snapshot().iter().map(|s| s.spawns).sum();
    assert_eq!(snapshot_sum, total, "快照求和必须等于总增量");
}

#[test]
fn snapshot_has_exactly_one_row_per_registered_kind() {
    // 契约:快照按 ALL 顺序一 kind 一行 —— 导出面不得重复或遗漏
    let counters = KindCounters::default();
    counters.record_spawn(TaskKind::ExpertScore);
    let snap = counters.snapshot();
    assert_eq!(snap.len(), TaskKind::COUNT);
    for (idx, kind) in TaskKind::ALL.iter().enumerate() {
        assert_eq!(snap[idx].kind, *kind, "快照第 {idx} 行顺序必须与 ALL 一致");
        assert_eq!(
            snap.iter().filter(|s| s.kind == *kind).count(),
            1,
            "{kind:?} 必须恰好出现一次"
        );
    }
}

#[test]
fn hottest_kind_is_deterministic_and_unknown_without_samples() {
    // 契约:决胜规则确定化（同计数取 ALL 靠前者）;零采样不伪造归因
    let counters = KindCounters::default();
    assert_eq!(counters.hottest_kind(), "Unknown", "无采样不得伪造归因");

    counters.record_spawn(TaskKind::ClvSimilarity);
    counters.record_spawn(TaskKind::OsaMask);
    assert_eq!(
        counters.hottest_kind(),
        "ClvSimilarity",
        "同计数必须取 ALL 中靠前者（确定性决胜）"
    );

    counters.record_spawn(TaskKind::OsaMask);
    assert_eq!(counters.hottest_kind(), "OsaMask", "计数更高者必须胜出");
}

#[test]
fn degradation_batch_and_item_grains_are_independent() {
    // 契约:degradations 是批粒度（一批 +1）,degraded_items 是项粒度（一批 n 项 +n）
    // —— 两个口径不可互推,故必须分别计数
    let counters = KindCounters::default();
    counters.record_degradation();
    counters.record_degraded_items(5);
    counters.record_degradation();
    counters.record_degraded_items(3);
    assert_eq!(counters.degradations(), 2, "批粒度两次降级必须为 2");
    assert_eq!(
        counters.degraded_items_total(),
        8,
        "项粒度为 5+3=8,与批粒度 2 不同源"
    );
}

#[test]
fn saturation_and_rejection_counters_are_monotone_and_separate() {
    // 契约:超阈观测（三策略均增）与严格拒绝（仅 strict）是两条独立曲线,只增不减
    let counters = KindCounters::default();
    assert_eq!(counters.saturations(), 0);
    assert_eq!(counters.rejections(), 0);
    for expected in 1..=3u64 {
        counters.record_saturation();
        assert_eq!(counters.saturations(), expected);
        assert_eq!(
            counters.rejections(),
            0,
            "Observe/Degrade 下不得产生拒绝计数"
        );
    }
    counters.record_rejection();
    assert_eq!(counters.rejections(), 1);
    assert_eq!(counters.saturations(), 3, "拒绝不得污染观测计数");
}
