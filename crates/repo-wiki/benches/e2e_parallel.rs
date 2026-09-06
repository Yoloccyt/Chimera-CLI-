//! e2e_parallel_bench — repo-wiki 端到端并行基准（P3-T14，手册 W19 门禁）
//!
//! 对应架构层: L5 Knowledge（repo-wiki）
//! 对应任务: **P3-T14**（手册 W19:8 核加速 ≥6× / 内存峰值 ≤ 基线 ×1.2）
//!
//! # 场景（端到端:检索 + 沉淀全链路）
//! 批量查询检索（`VectorIndex::search_batch` 经 ComputeBridge 并行注入）
//! → 结果沉淀（命中结果写入索引模拟,纯 CPU）;
//! 对比:串行（`parallel_search = false`）vs 并行（默认 true）。
//!
//! # 诚实数据
//! 本机为 16 核（池 = num_cpus-2 = 14 线程）;手册门禁「8 核 ≥6×」按基准机
//! 口径,实测加速比随核数缩放记录（speedup 标注核数）;内存峰值为计算规模
//! 推算（向量字节数 ×2 上限,不实测 RSS——标注近似口径）。
//!
//! # 运行
//! `cargo bench -p repo-wiki --bench e2e_parallel`（release 模式）;
//! 静默态单跑取样（µs 级 bench 防负载假回归）。

#![forbid(unsafe_code)]

use std::hint::black_box;
use std::time::Instant;

use criterion::{criterion_group, criterion_main, Criterion};
use repo_wiki::vector::VectorIndex;

/// 向量维度（与 knn_parallel 一致,64 维）
const VECTOR_DIM: usize = 64;
/// 查询数（批处理规模;16,384 × 256 = 4.2M 相似度/查询 >> 阈值 → Rayon 分支）
const N_QUERIES: usize = 256;
/// 向量数（放大使并行检索主导,公共成本占比 <10%）
const N_VECTORS: usize = 32_768;
/// Top-K
const TOP_K: usize = 5;

/// 构造伪随机向量数据（固定种子,确定性）
fn make_vectors(n: usize) -> Vec<Vec<f32>> {
    let mut state = 42u64;
    let mut rng = move || {
        state = state
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (state >> 33) as f32 / u32::MAX as f32
    };
    (0..n)
        .map(|_| (0..VECTOR_DIM).map(|_| rng()).collect())
        .collect()
}

/// 串行端到端 — 检索 + 沉淀（全链路基线）
fn e2e_serial(vectors: &[Vec<f32>], queries: &[Vec<f32>]) -> usize {
    let idx = VectorIndex::new(VECTOR_DIM).with_parallel_search(false);
    for (i, v) in vectors.iter().enumerate() {
        idx.upsert(&format!("v{i}"), v).expect("upsert 维度匹配");
    }
    let mut hits = 0usize;
    for q in queries {
        let result = idx.search(q, TOP_K).expect("检索成功");
        hits += result.len();
    }
    black_box(hits)
}

/// 并行端到端 — 检索批量经 ComputeBridge + 沉淀（默认 parallel_search = true）
fn e2e_parallel(vectors: &[Vec<f32>], queries: &[Vec<f32>]) -> usize {
    let idx = VectorIndex::new(VECTOR_DIM);
    for (i, v) in vectors.iter().enumerate() {
        idx.upsert(&format!("v{i}"), v).expect("upsert 维度匹配");
    }
    // search_batch 内部经 ComputeBridge::spawn_compute_batch（TaskKind::KnnSearch）
    let results = idx.search_batch(queries, TOP_K).expect("批量检索成功");
    black_box(results.iter().map(|r| r.len()).sum::<usize>())
}

fn bench_e2e(c: &mut Criterion) {
    let vectors = make_vectors(N_VECTORS);
    let queries = make_vectors(N_QUERIES);
    let cores = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1);

    // 单次预热（JIT/缓存冷启动）
    e2e_serial(&vectors, &queries);
    e2e_parallel(&vectors, &queries);

    let mut group = c.benchmark_group("e2e_parallel");
    group.sample_size(10);

    // 串行基线（单次取样,静默态）
    let serial_start = Instant::now();
    let serial_hits = e2e_serial(&vectors, &queries);
    let serial_us = serial_start.elapsed().as_micros() as f64;
    assert_eq!(serial_hits, N_QUERIES * TOP_K);

    // 并行（单次取样）
    let par_start = Instant::now();
    let par_hits = e2e_parallel(&vectors, &queries);
    let par_us = par_start.elapsed().as_micros() as f64;
    assert_eq!(par_hits, N_QUERIES * TOP_K);
    let speedup = serial_us / par_us;
    // 内存近似:向量数据（输入 + 索引内副本上限）
    let mem_bytes = (N_VECTORS * VECTOR_DIM * 4 + N_QUERIES * VECTOR_DIM * 4) as f64;

    println!(
        "E2E_PARALLEL cores={} serial_us={:.0} parallel_us={:.0} speedup={:.2}x mem_approx_kb={:.0}",
        cores, serial_us, par_us, speedup, mem_bytes / 1024.0
    );
    // 门禁记录（不硬断言——release 模式差异;报告层判定）
    group.bench_function("serial_baseline", |b| {
        b.iter(|| e2e_serial(&vectors, &queries));
    });
    group.bench_function("parallel_injected", |b| {
        b.iter(|| e2e_parallel(&vectors, &queries));
    });
    group.finish();
}

criterion_group!(benches, bench_e2e);
criterion_main!(benches);
