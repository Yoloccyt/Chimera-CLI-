//! Wiki KNN SLO Benchmark — HnswStore 100K entry p95 < 50ms 验证
//!
//! 对应 Task T7-4: wiki_knn SLO benchmark。
//! 验证 spec.md 索引 SLO 红线: **100K entry KNN 查询 p95 延迟 < 50ms**。
//!
//! # 基准项
//! - `wiki_knn_100k_p95`: 100K entry 规模下 `top_k(query, 5, "")` 的 p95 延迟
//!   使用 `iter_custom` 收集单次调用延迟样本，排序后计算 p95 并通过 `eprintln!` 输出。
//!
//! # 运行
//! ```bash
//! # 快速验证（--test 模式，不精确测量）
//! cargo bench -p repo-wiki --bench wiki_knn_slo -- --test
//! # 正式测量（release 模式推荐，100K 预填充约 30-60 秒）
//! cargo bench --release -p repo-wiki --bench wiki_knn_slo
//! ```
//!
//! # 设计说明
//! - HnswStore 为纯内存结构，基于 hnsw_rs HNSW 图算法
//! - 100K entry × 512 dim ≈ 195MB 向量 + ~390MB HNSW 图 ≈ 585MB 内存
//! - 预填充在 setup 阶段执行（不计入测量时间）
//! - `top_k` 为同步方法（`&self`），无需 spawn_blocking
//! - SLO 红线断言由 CI 性能阈值测试守护，本 benchmark 仅输出统计指标

#![forbid(unsafe_code)]

use std::time::{Duration, Instant};

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use nexus_contracts::VectorStore;
use repo_wiki::HnswStore;

/// 向量维度（与 CLV 512-dim 对齐）
const VECTOR_DIM: usize = 512;

/// KNN 返回的 Top-K 数量
const TOP_K: usize = 5;

/// SLO 目标条目数
const ENTRY_COUNT: usize = 100_000;

/// SLO 目标 p95 延迟上限
const SLO_P95_LIMIT: Duration = Duration::from_millis(50);

/// 生成确定性伪随机向量
///
/// 每个分量基于 `(id, dim_index)` 派生，保证：
/// 1. 不同 id 产生不同向量（余弦相似度有意义）
/// 2. 无零向量（避免除零导致相似度 NaN）
/// 3. 可复现（消除随机性导致的 bench 抖动）
///
/// 模数 100003 为质数，周期 = 100003 > 100K，无重复。
fn make_vector(id: u64, dim: usize) -> Vec<f32> {
    (0..dim)
        .map(|j| {
            let h = id
                .wrapping_mul(7)
                .wrapping_add((j as u64).wrapping_mul(13))
                .wrapping_mul(31);
            let v = (h % 100003) as f32 / 100003.0;
            v + 0.001 // 避免零向量
        })
        .collect()
}

/// 预填充 HNSW 向量存储（不计入测量时间）
fn prefill_hnsw(store: &HnswStore, count: usize) {
    for i in 0..count {
        let vec = make_vector(i as u64, VECTOR_DIM);
        store
            .upsert(&format!("wiki-vec-{i}"), &vec, ())
            .unwrap_or_else(|e| panic!("预填充 hnsw upsert 失败 @#{i}: {e}"));
    }
}

/// 100K entry fixture（store + query vector）
struct WikiKnn100KFixture {
    store: HnswStore,
    query: Vec<f32>,
}

impl WikiKnn100KFixture {
    fn new() -> Self {
        let store = HnswStore::with_dim(VECTOR_DIM);
        prefill_hnsw(&store, ENTRY_COUNT);
        // 查询向量与 vec-0 相同，保证命中 top1 且结果稳定
        let query = make_vector(0, VECTOR_DIM);
        Self { store, query }
    }
}

/// HnswStore 100K entry KNN p95 SLO 基准
///
/// 验证 spec.md 索引 SLO 红线：**100K entry p95 < 50ms**。
///
/// # 设计
/// `iter_custom` 手动控制每次迭代的计时，收集每单次 `top_k` 调用的延迟样本。
/// criterion 调用 `iters` 次，收集后排序取 p95，通过 `eprintln!` 输出供人工/CI 核验。
///
/// # 输出示例
/// ```text
/// [wiki_knn_100k_slo] samples=200, mean=2.5ms, p50=2.2ms, p95=5.1ms, p99=8.3ms, slo_pass=true
/// ```
fn wiki_knn_100k_p95(c: &mut Criterion) {
    let fixture = WikiKnn100KFixture::new();

    let mut group = c.benchmark_group("wiki_knn_100k_slo");
    group.sample_size(100);

    group.bench_function("p95_latency", |b| {
        b.iter_custom(|iters| {
            let mut latencies: Vec<Duration> = Vec::with_capacity(iters as usize);
            let start_total = Instant::now();
            for _ in 0..iters {
                let start = Instant::now();
                black_box(
                    fixture
                        .store
                        .top_k(black_box(&fixture.query), black_box(TOP_K), "")
                        .expect("hnsw search 失败"),
                );
                latencies.push(start.elapsed());
            }
            let total = start_total.elapsed();

            // 排序后取百分位
            latencies.sort_unstable();
            let n = latencies.len();
            let p50 = latencies[((n as f64 * 0.50) as usize).min(n.saturating_sub(1))];
            let p95 = latencies[((n as f64 * 0.95) as usize).min(n.saturating_sub(1))];
            let p99 = latencies[((n as f64 * 0.99) as usize).min(n.saturating_sub(1))];
            let mean = latencies.iter().sum::<Duration>() / n.max(1) as u32;
            let slo_pass = p95 < SLO_P95_LIMIT;

            eprintln!(
                "[wiki_knn_100k_slo] samples={n}, mean={mean:?}, p50={p50:?}, \
                 p95={p95:?}, p99={p99:?}, slo_pass={slo_pass}"
            );

            total
        })
    });
    group.finish();
}

criterion_group!(benches, wiki_knn_100k_p95);
criterion_main!(benches);
