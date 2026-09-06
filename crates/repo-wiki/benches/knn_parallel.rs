//! 批量 KNN 检索 ComputeBridge 并行注入基准 — 串行 vs 并行吞吐对照
//!
//! 对应任务:P1-T14（WI-34 并行化第 2 批补全,v4.0 §7.5.1 L-a）
//! 架构层:L5 Knowledge（内存 KNN 降级路径 + HNSW 生产路径的上游批量检索）
//! 对应注入表:W8-9「repo-wiki KNN 并行;扫描成本降 ≥85%」
//!
//! # 口径
//! 直测注入热点 [`VectorIndex::search_batch`](repo_wiki::vector::VectorIndex::search_batch)
//! 批量 KNN 全流程 —— 快照分离（读锁一次 + 按 id 排序）→ 计算
//! （`TaskKind::KnnSearch`,阈值 5,000,`n_items = q × v` 总相似度计算数）→
//! 结果保序返回。128 queries × 512 vectors × 64 dim = 65,536 次余弦计算
//! （≥ 阈值 → Rayon 分支）,确定性伪随机向量（同 wiki_knn_slo bench 口径）,无 IO。
//!
//! # 场景
//! - 串行路径:`idx.with_parallel_search(false)`（回退语义）
//! - 并行路径:`VectorIndex::new(dim)`（`parallel_search = true` 默认）
//!
//! # 门禁
//! 扫描成本降 ≥85%（≈ ≥6.7× 加速,对应任务目标 3-6× 上沿）。口径与 T8/T9 一致:
//! `iter_custom` 测量阶段零打印,测量结束后固定采样打印 P50/P99 + speedup;
//! 不达标时报告按 DONE_WITH_CONCERNS 记录实测值（基准不做断言,防采样抖动误报）。

#![forbid(unsafe_code)]

use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use repo_wiki::vector::VectorIndex;

/// 向量维度（降维控制 setup 成本;余弦计算量与维度线性相关,64 维已足够显著）
const VECTOR_DIM: usize = 64;

/// 查询数 × 向量数 = 65,536 次余弦计算 ≥ KnnSearch 阈值 5,000 → Rayon 分支
const N_QUERIES: usize = 128;
const N_VECTORS: usize = 512;

/// KNN 返回的 Top-K 数量
const TOP_K: usize = 5;

/// 确定性伪随机向量（同 wiki_knn_slo bench 口径:分量 ∈ (0,1),无零向量）
fn make_vector(id: u64, dim: usize) -> Vec<f32> {
    (0..dim)
        .map(|j| {
            let h = id
                .wrapping_mul(7)
                .wrapping_add((j as u64).wrapping_mul(13))
                .wrapping_mul(31);
            let v = (h % 100003) as f32 / 100003.0;
            v + 0.001
        })
        .collect()
}

/// 单次批量检索耗时（注入热点直测,search_batch 只读不消费）
fn run(idx: &VectorIndex, queries: &[Vec<f32>]) -> Duration {
    let start = Instant::now();
    let out = idx.search_batch(queries, TOP_K);
    let _ = criterion::black_box(out);
    start.elapsed()
}

// ADR-159 决策 3 三态登记:dev-only(历史副本,新 bench 请用 nexus_contracts::util::percentile_sorted)
/// 取分位数（样本需已排序）
fn percentile(sorted: &[Duration], p: f64) -> Duration {
    assert!(
        !sorted.is_empty(),
        "percentile: 样本为空,无法计算 p={p} 分位数"
    );
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx]
}

fn bench_knn_parallel(c: &mut Criterion) {
    // 两索引同一初始数据;串行/并行各自独立,只读检索 → 对比公平
    let idx_serial = VectorIndex::new(VECTOR_DIM).with_parallel_search(false);
    let idx_parallel = VectorIndex::new(VECTOR_DIM);
    let queries: Vec<Vec<f32>> = (0..N_QUERIES)
        .map(|i| make_vector(i as u64, VECTOR_DIM))
        .collect();

    // 预填充（setup,不计时）
    for i in 0..N_VECTORS {
        let vec = make_vector(i as u64, VECTOR_DIM);
        idx_serial.upsert(&format!("vec-{i:05}"), &vec).unwrap();
        idx_parallel.upsert(&format!("vec-{i:05}"), &vec).unwrap();
    }

    // 预热:rayon 池线程与内存缓存就绪后采样更接近稳态
    for _ in 0..4 {
        let _ = run(&idx_serial, &queries);
        let _ = run(&idx_parallel, &queries);
    }

    let mut group = c.benchmark_group("knn_parallel");
    group.sample_size(10);
    group.bench_function(BenchmarkId::from_parameter("serial_128x512"), |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                total += run(&idx_serial, &queries);
            }
            total
        });
    });
    group.bench_function(BenchmarkId::from_parameter("parallel_128x512"), |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                total += run(&idx_parallel, &queries);
            }
            total
        });
    });
    group.finish();

    // 门禁采样（iter_custom 外,零干扰）
    const GATE_SAMPLES: usize = 50;
    let mut serial_samples = Vec::with_capacity(GATE_SAMPLES);
    for _ in 0..GATE_SAMPLES {
        serial_samples.push(run(&idx_serial, &queries));
    }
    serial_samples.sort_unstable();
    let mut parallel_samples = Vec::with_capacity(GATE_SAMPLES);
    for _ in 0..GATE_SAMPLES {
        parallel_samples.push(run(&idx_parallel, &queries));
    }
    parallel_samples.sort_unstable();

    let s_p50 = percentile(&serial_samples, 0.50);
    let s_p99 = percentile(&serial_samples, 0.99);
    let p_p50 = percentile(&parallel_samples, 0.50);
    let p_p99 = percentile(&parallel_samples, 0.99);
    let speedup = s_p50.as_secs_f64() / p_p50.as_secs_f64();
    eprintln!(
        "[knn_parallel] queries={N_QUERIES} vectors={N_VECTORS} dim={VECTOR_DIM} serial P50={:.2}ms P99={:.2}ms | parallel P50={:.2}ms P99={:.2}ms | speedup(P50)={:.2}× (扫描成本降≥85%,目标 3-6×)",
        s_p50.as_secs_f64() * 1e3,
        s_p99.as_secs_f64() * 1e3,
        p_p50.as_secs_f64() * 1e3,
        p_p99.as_secs_f64() * 1e3,
        speedup,
    );
}

criterion_group!(benches, bench_knn_parallel);
criterion_main!(benches);
