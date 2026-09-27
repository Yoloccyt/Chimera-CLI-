//! L2 分层检索 tiered 档基准 — ADR-192 §5 验收判据取数源
//!
//! 判据(静默态单跑取样,记忆红线:µs/ms 级门槛须静默态防负载假回归):
//! - `l2_knn_tiered/top10_recall/10000_tiered` **≤ 2.0 ms**(现状 4.117 ms,≥2× 且退出 redline 带)
//! - `l2_knn_tiered/top10_recall/100000_tiered` **≤ 8 ms**(现状 42.136 ms,≥5×)
//! - `l2_knn_tiered/top10_recall/4096_exact` 不回退(精确层红线 4096 < 200ms 原样)
//! - `l2_knn_tiered/recall_quality/*` 近似层 recall@10 质量采样(分数制三档)
//!
//! WHY 同时测 `_exact` 对照:同一进程同一语料形态下给出改善倍数,避免"跨 session
//! 负载漂移"被当作收益(亦即 ADR-192 §5 的 ≥2×/≥5× 判据需同批可比数据)。
//! 内存安全与 CI 时长:每档先测对照实例并 **drop**,再建近似层实例,避免 100k 档
//! 「主表 + 图副本 + 对照」三份向量同时驻留把内存压爆(该路径有 OOM 前科);
//! 100k 档另需 `CHIMERA_HNSW_BENCH_FULL=1` 显式开启(见 `bench_sizes` 注)。

#![allow(clippy::unwrap_used, clippy::expect_used)] // bench code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use mlc_engine::l2_semantic::HNSW_MIN_ENTRIES;
use mlc_engine::{MemoryEntry, MemoryTier, SemanticMemory};
use nexus_contracts::util::xts_top_k_by;
use nexus_core::CLV;

/// 本轮跑的规模档
///
/// WHY 默认只跑 10k(受门守护的立项判据档):100k 档除填充外还要建图(十秒级)
/// 与全量 ground-truth 质量扫描,不宜进每一次 CI 日跑;
/// 其现值由 `CHIMERA_HNSW_BENCH_FULL=1` 静默态单跑取数并登记在
/// `docs/reports/perf-slo-2026-09-25.md` §1.1 落地状态与 ADR-192 §5。
fn bench_sizes() -> Vec<usize> {
    if std::env::var("CHIMERA_HNSW_BENCH_FULL").as_deref() == Ok("1") {
        vec![10_000, 100_000]
    } else {
        vec![10_000]
    }
}

/// 固定种子 LCG(与 tests/hnsw_tiered.rs 同形态语料,便于交叉印证)
fn lcg_next(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *state >> 33
}

fn lcg_f32(state: &mut u64) -> f32 {
    let raw = lcg_next(state) as f64;
    (raw / (1u64 << 31) as f64) as f32 - 1.0
}

fn make_centers(n: usize, rng: &mut u64) -> Vec<Vec<f32>> {
    (0..n)
        .map(|c| {
            let mut v = vec![0.0_f32; CLV::DIMENSION];
            v[c % CLV::DIMENSION] = 1.0;
            for _ in 0..16 {
                let d = (lcg_next(rng) as usize) % CLV::DIMENSION;
                v[d] += lcg_f32(rng) * 0.2;
            }
            v
        })
        .collect()
}

/// n 条 64 簇语料(簇心 0.85 + 噪声 0.15)
fn make_corpus(n: usize) -> (Vec<Vec<f32>>, Vec<String>) {
    let mut rng: u64 = 0x5eed_2026_0926;
    let centers = make_centers(64, &mut rng);
    let mut vectors = Vec::with_capacity(n);
    let mut ids = Vec::with_capacity(n);
    for i in 0..n {
        let c = &centers[i % centers.len()];
        vectors.push(
            c.iter()
                .map(|&x| 0.85 * x + 0.15 * lcg_f32(&mut rng))
                .collect(),
        );
        ids.push(format!("m-{i}"));
    }
    (vectors, ids)
}

fn make_query(seed_offset: u64) -> Vec<f32> {
    let mut rng: u64 = 0x0a11_ce00_0000 ^ seed_offset;
    let centers = make_centers(64, &mut rng);
    let c = &centers[(seed_offset as usize) % centers.len()];
    c.iter()
        .map(|&x| 0.95 * x + 0.05 * lcg_f32(&mut rng))
        .collect()
}

/// 已填充实例(`hnsw_threshold = 0` 即关闭近似层,作对照档)
fn build_mem(vectors: &[Vec<f32>], ids: &[String], hnsw_threshold: usize) -> SemanticMemory {
    let mem = SemanticMemory::with_hnsw_threshold(vectors.len().max(1), hnsw_threshold);
    for (raw, id) in vectors.iter().zip(ids.iter()) {
        let clv = CLV::from_vec(raw.clone()).expect("CLV 构造应成功");
        mem.insert(
            MemoryEntry::new(id.as_str(), format!("content-{id}"), MemoryTier::L2Semantic)
                .with_clv(clv),
        )
        .expect("插入应成功");
    }
    mem
}

/// 一次性建图并返回耗时(近似层为读路径惰性建图,预热即测量点)
fn warm_up_build(mem: &SemanticMemory, query: &CLV) -> std::time::Duration {
    let started = std::time::Instant::now();
    mem.recall_by_clv(query, 10).expect("召回应成功");
    let elapsed = started.elapsed();
    let snap = mem.hnsw_tier_snapshot().expect("快照可读");
    eprintln!(
        "[hnsw_tiered] 建图+首召回 {:?} (built={} live={} tombstones={} threshold={})",
        elapsed, snap.built, snap.live_points, snap.tombstones, snap.threshold
    );
    elapsed
}

/// 基准:10k / 100k 双档 × {tiered, exact} 对照 + 4096 精确层不回退
fn bench_tiered_vs_exact(c: &mut Criterion) {
    let mut group = c.benchmark_group("l2_knn_tiered");
    // 100k 档样本数下调(每次 iter 持 205MB 级驻留,避免 CI runner OOM)
    group.sample_size(30);

    for &size in &bench_sizes() {
        let (vectors, ids) = make_corpus(size);
        let raw_query = make_query(3);
        let query = CLV::from_vec(raw_query.clone()).unwrap();

        // 1) 对照档:关闭近似层(= 现线性扫描,ADR-005 降级后的行为)
        {
            let exact = build_mem(&vectors, &ids, 0);
            group.bench_function(
                BenchmarkId::new("top10_recall", format!("{size}_exact")),
                |b| {
                    b.iter(|| {
                        let hits = exact.recall_by_clv(black_box(&query), 10).unwrap();
                        black_box(hits.len());
                    });
                },
            );
        } // drop 后再建近似层实例,避免两份向量同时驻留

        // 2) 分层档:达阈走近似层(预热即触发一次性建图,不混入采样)
        let tiered = build_mem(&vectors, &ids, HNSW_MIN_ENTRIES);
        warm_up_build(&tiered, &query);
        group.bench_function(
            BenchmarkId::new("top10_recall", format!("{size}_tiered")),
            |b| {
                b.iter(|| {
                    let hits = tiered.recall_by_clv(black_box(&query), 10).unwrap();
                    black_box(hits.len());
                });
            },
        );

        // 3) recall@10 质量采样(分数制三档,判据见文件头)
        quality_probe(size, &tiered, &vectors, &ids);

        // 4) 图侧新增驻留估算(诚实登记 ADR-192 §4 代价 1)
        let snap = tiered.hnsw_tier_snapshot().unwrap();
        eprintln!(
            "[hnsw_tiered] size={size} 图侧估算:向量副本 ≈ {:.1} MB,侧表 ≈ {:.1} MB(live={})",
            snap.live_points as f64 * (CLV::DIMENSION as f64 * 4.0) / 1_048_576.0,
            snap.live_points as f64 * 96.0 / 1_048_576.0,
            snap.live_points
        );
    }

    // 5) 4096 精确层不回退(阈值 0 实例 = 现行为,守既有 <200ms 红线)
    let (vectors, ids) = make_corpus(HNSW_MIN_ENTRIES);
    let query = CLV::from_vec(make_query(5)).unwrap();
    let exact_4096 = build_mem(&vectors, &ids, 0);
    group.bench_function(BenchmarkId::new("top10_recall", "4096_exact"), |b| {
        b.iter(|| {
            let hits = exact_4096.recall_by_clv(black_box(&query), 10).unwrap();
            black_box(hits.len());
        });
    });
    group.finish();
}

/// 近似层 recall@10 质量采样(分数制三档)
///
/// WHY 不走 `b.iter`：每 query 需逐条全量打分作 ground truth（100k 档 5 次≈秒级），
/// 入采样循环会拖成小时级；此处只取一次质量数据，不进延迟基准。
fn quality_probe(size: usize, tiered: &SemanticMemory, vectors: &[Vec<f32>], ids: &[String]) {
    let mut id_hits = 0usize;
    let mut tol_hits = 0usize;
    let mut sum_got = 0f64;
    let mut sum_gt = 0f64;
    let queries = 5_u64;
    for q in 0..queries {
        let raw_query = black_box(make_query(q));
        let query = CLV::from_vec(raw_query.clone()).unwrap();
        let got = tiered.recall_by_clv(&query, 10).unwrap();
        let mut scored: Vec<(usize, f32)> = vectors
            .iter()
            .zip(ids.iter())
            .enumerate()
            .map(|(i, (v, _))| {
                (
                    i,
                    nexus_core::cosine_similarity_slices(v, &raw_query).clamp(0.0, 1.0),
                )
            })
            .collect();
        // 红线 R8 合规范式取 Top-10（L0 `xts_top_k_by`），与测试侧同型：
        // ground-truth 的独立性在「逐条全量打分」，不靠多引入一个全排序站点
        let gt = xts_top_k_by(&mut scored, 10, |a, b| {
            b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
        })
        .to_vec();
        let gt_min = gt.iter().map(|(_, s)| *s).fold(f32::MAX, f32::min);
        sum_gt += gt.iter().map(|(_, s)| *s as f64).sum::<f64>();
        for (id, score) in &got {
            let idx: usize = id
                .as_str()
                .strip_prefix("m-")
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            let exact_score =
                nexus_core::cosine_similarity_slices(&vectors[idx], &raw_query).clamp(0.0, 1.0);
            sum_got += exact_score as f64;
            if gt.iter().any(|(i, _)| *i == idx) {
                id_hits += 1;
            }
            if exact_score >= gt_min - 1e-4 {
                tol_hits += 1;
            }
            black_box(score);
        }
    }
    let samples = (queries as usize * 10) as f64;
    eprintln!(
        "[hnsw_tiered] size={size} id-recall={:.4} tol-recall={:.4} quality={:.6}",
        id_hits as f64 / samples,
        tol_hits as f64 / samples,
        sum_got / sum_gt
    );
    assert!(
        sum_got / sum_gt >= 0.99,
        "ADR-192 召回质量比低于 0.99(size={size})"
    );
    assert!(
        tol_hits as f64 / samples >= 0.95,
        "ADR-192 分数容差 recall@10 低于 0.95(size={size})"
    );
}

criterion_group!(benches, bench_tiered_vs_exact);
criterion_main!(benches);
