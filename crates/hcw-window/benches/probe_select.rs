//! PROBE P1.7 探针装窗基准 — criterion 只输出分布不 panic（红线由独立测试守护）
//!
//! 对应任务: PROBE P1 实施计划 T7（P1.7 双性能红线）
//! 对应红线断言: `tests/probe_select_p95_test.rs`（#[ignore] release，p95<10ms + 吞吐 ≥10K 块/秒）
//!
//! # 设计（对齐既有 bench 范式）
//! - bench 只 `eprintln!` 输出分布，**不 panic**（`benches/coarse_recall.rs` L258-260 约定）
//! - 红线断言在独立 `#[ignore]` release 测试中（测试可失败，bench 不失败 CI）
//!
//! # 基准组
//! - `probe_window_select`: 256 块探针装窗全链路（快照→打分→Top-K→组装）
//! - `probe_score_throughput`: 打分吞吐（块/秒），1000 块全量 cosine
//! - `score_cache_hit_vs_recompute`: 增量重打分命中 vs 重算对照（查询期零计算收益）

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::hint::black_box;

use criterion::{criterion_group, criterion_main, Criterion};
use hcw_window::recall::types::BlockId;
use hcw_window::{score_with_probe, ProbeWeights, ScoreCache};
use nexus_core::CLV;

/// 窗口档块数（128K 档 = 256 块 × 512 token）
const BLOCK_COUNT: usize = 256;
/// 吞吐基准块数（1000 块，超窗语料模拟）
const THROUGHPUT_BLOCKS: usize = 1000;

/// 构造确定性 CLV（SplitMix64，与 recall/eval 同模式）
fn make_clv(seed: u64) -> CLV {
    let v: Vec<f32> = (0..512)
        .map(|j| {
            let mut z = seed.wrapping_add((j as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            ((z >> 11) as f32) / (1u64 << 53) as f32 * 2.0 - 1.0
        })
        .collect();
    CLV::from_vec(v).expect("512 dims")
}

/// 构造语料（块 CLV 与主题 60% 混合）
fn build_corpus(count: usize) -> (CLV, Vec<(BlockId, CLV)>) {
    let topic = make_clv(0x5EED_CAFE);
    let blocks: Vec<(BlockId, CLV)> = (0..count)
        .map(|i| {
            let noise = make_clv(1000 + i as u64);
            let t = topic.as_slice();
            let n = noise.as_slice();
            let v: Vec<f32> = (0..512).map(|j| 0.6 * t[j] + 0.4 * n[j]).collect();
            (format!("b{i:03}"), CLV::from_vec(v).expect("512 dims"))
        })
        .collect();
    (topic, blocks)
}

/// 探针装窗单趟（打分 + 融合 + Top-K 组装）
fn probe_window_pass(
    probe: &CLV,
    blocks: &[(BlockId, CLV)],
    weights: ProbeWeights,
) -> Vec<BlockId> {
    // 打分（f32 全程）+ 静态分融合（score_with_probe 共享公式）
    let mut scored: Vec<(f32, &BlockId)> = blocks
        .iter()
        .map(|(id, clv)| {
            let probe_score = probe.cosine_similarity(clv).max(0.0);
            let static_score = 0.5; // 中性静态分（无 recency/frequency 上下文时）
            (score_with_probe(static_score, probe_score, weights), id)
        })
        .collect();
    // Top-K（select_nth_unstable_by 红线）
    let k = 128.min(scored.len());
    let nth = k - 1;
    scored.select_nth_unstable_by(nth, |a, b| {
        b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(k);
    scored.into_iter().map(|(_, id)| id.clone()).collect()
}

fn bench_probe_window_select(c: &mut Criterion) {
    let (probe, blocks) = build_corpus(BLOCK_COUNT);
    let weights = ProbeWeights::DEFAULT;
    let mut group = c.benchmark_group("probe_window_select");
    group.sample_size(100);
    group.bench_function("256_blocks", |b| {
        b.iter(|| {
            let selected = probe_window_pass(black_box(&probe), black_box(&blocks), weights);
            black_box(selected);
        })
    });
    group.finish();
}

fn bench_probe_score_throughput(c: &mut Criterion) {
    let (probe, blocks) = build_corpus(THROUGHPUT_BLOCKS);
    let weights = ProbeWeights::new(0.5, 0.5);
    // 吞吐按 iter_custom 计时（块/秒）
    let mut group = c.benchmark_group("probe_score_throughput");
    group.sample_size(50);
    group.bench_function("1000_blocks", |b| {
        b.iter_custom(|iters| {
            let start = std::time::Instant::now();
            for _ in 0..iters {
                black_box(probe_window_pass(black_box(&probe), black_box(&blocks), weights));
            }
            let elapsed = start.elapsed();
            let per_pass = elapsed.as_secs_f64() / iters as f64;
            let throughput = THROUGHPUT_BLOCKS as f64 / per_pass;
            eprintln!(
                "[probe_score_throughput] per_pass={:.3}ms throughput={:.0} blocks/s (redline >= 10000)",
                per_pass * 1000.0,
                throughput
            );
            elapsed
        })
    });
    group.finish();
}

fn bench_score_cache_hit_vs_recompute(c: &mut Criterion) {
    let (probe, blocks) = build_corpus(BLOCK_COUNT);
    let hash = ScoreCache::probe_fingerprint(&probe);
    let mut group = c.benchmark_group("score_cache_hit_vs_recompute");
    group.sample_size(100);

    group.bench_function("cache_hit", |b| {
        let mut cache = ScoreCache::new();
        let scores: HashMap<BlockId, f32> = blocks
            .iter()
            .map(|(id, clv)| (id.clone(), probe.cosine_similarity(clv).max(0.0)))
            .collect();
        cache.put(hash, 1, scores);
        b.iter(|| {
            let hit = cache.try_hit(black_box(hash), 1);
            black_box(hit.is_some());
        })
    });

    group.bench_function("recompute", |b| {
        let mut cache = ScoreCache::new();
        b.iter(|| {
            let scores: HashMap<BlockId, f32> = blocks
                .iter()
                .map(|(id, clv)| (id.clone(), probe.cosine_similarity(clv).max(0.0)))
                .collect();
            cache.put(hash, 1, scores);
        })
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_probe_window_select,
    bench_probe_score_throughput,
    bench_score_cache_hit_vs_recompute,
);
criterion_main!(benches);
