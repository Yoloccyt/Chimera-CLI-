//! HCW-Sparse v2.0 精排基准 — HNSW + 精确 CLV 重排 → 500 Block
//!
//! 对应任务: P3-W9.2.2
//! 红线: 精排 p95 <50ms（spec.md §P3 验收标准）
//!
//! # 基准场景
//! 1. 100 Block（轻量场景）— 验证小规模延迟
//! 2. 1000 Block（典型场景）— 验证中等规模延迟
//! 3. 10000 Block（大规模场景）— 验证生产规模延迟
//! 4. p95 延迟测量（1000 Block × 1000 次采样）— 红线守护

use std::collections::HashMap;
use std::hint::black_box;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, Criterion};
use hcw_window::recall::{CoarseRecallOutput, FineRecall, FineRecallConfig, FineRecallInput};
use nexus_contracts::{VectorHit, VectorStore};
use nexus_core::CLV;

// ============================================================
// 测试用 VectorStore 实现（bench 独立，不依赖 #[cfg(test)] mock）
// ============================================================

/// Bench 用内存 VectorStore — 真实存储向量并实现精确 KNN
///
/// WHY RwLock 而非 RefCell: criterion bench 可能多线程采样，
/// RwLock 保证线程安全（RefCell 非 Send + Sync）。
/// O(n) 遍历 + 精确余弦相似度，比 HNSW 慢，
/// 若此实现满足 <50ms 红线，真实 HnswStore 一定能满足。
struct BenchVectorStore {
    dim: usize,
    vectors: RwLock<HashMap<String, Vec<f32>>>,
}

impl BenchVectorStore {
    fn with_dim(dim: usize) -> Self {
        Self {
            dim,
            vectors: RwLock::new(HashMap::new()),
        }
    }

    fn insert(&self, id: &str, vector: Vec<f32>) {
        self.vectors
            .write()
            .expect("rwlock not poisoned")
            .insert(id.to_string(), vector);
    }
}

impl VectorStore for BenchVectorStore {
    type Meta = ();
    type Error = String;

    fn upsert(&self, id: &str, vector: &[f32], _meta: Self::Meta) -> Result<(), Self::Error> {
        if vector.len() != self.dim {
            return Err(format!(
                "dimension mismatch: expected {}, got {}",
                self.dim,
                vector.len()
            ));
        }
        self.vectors
            .write()
            .expect("rwlock not poisoned")
            .insert(id.to_string(), vector.to_vec());
        Ok(())
    }

    fn top_k(&self, query: &[f32], k: usize, _ns: &str) -> Result<Vec<VectorHit>, Self::Error> {
        if query.len() != self.dim {
            return Err(format!(
                "query dimension mismatch: expected {}, got {}",
                self.dim,
                query.len()
            ));
        }
        let vectors = self.vectors.read().expect("rwlock not poisoned");
        let mut scored: Vec<VectorHit> = vectors
            .iter()
            .map(|(id, vec)| VectorHit::new(id.clone(), cosine_similarity(query, vec)))
            .collect();
        // Top-K 用 select_nth_unstable_by（O(n)），符合工程约定
        if k < scored.len() {
            scored.select_nth_unstable_by(k, |a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        scored.truncate(k);
        // 最终降序排序（K log K）
        scored.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        Ok(scored)
    }

    fn remove(&self, id: &str) -> Result<(), Self::Error> {
        self.vectors
            .write()
            .expect("rwlock not poisoned")
            .remove(id);
        Ok(())
    }

    fn default() -> Self {
        Self {
            dim: 512,
            vectors: RwLock::new(HashMap::new()),
        }
    }
}

/// 简化版余弦相似度（bench 用）
fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }
    (dot / (norm_a * norm_b)).max(0.0)
}

// ============================================================
// 测试数据构建
// ============================================================

/// 构造确定性 CLV:基于 seed 生成 512-dim 向量
fn make_clv(seed: u64) -> CLV {
    let v: Vec<f32> = (0..CLV::DIMENSION)
        .map(|j| ((seed.wrapping_add(j as u64)) % 100) as f32 / 100.0)
        .collect();
    CLV::from_vec(v).expect("CLV dimension should be 512")
}

/// 构建 Block 向量索引:插入 block_count 个 Block
fn build_vector_store(block_count: usize) -> BenchVectorStore {
    let store = BenchVectorStore::with_dim(512);
    for i in 0..block_count {
        let clv = make_clv(i as u64);
        store.insert(&format!("block-{i}"), clv.as_slice().to_vec());
    }
    store
}

/// 构建空的粗召回输出（精排逻辑不直接使用，仅占位）
fn empty_coarse_output() -> CoarseRecallOutput {
    CoarseRecallOutput {
        modules: Vec::new(),
        elapsed_us: 0,
    }
}

// ============================================================
// 基准测试
// ============================================================

/// 基准 1: 100 Block 精排（轻量场景）
fn bench_fine_recall_100_blocks(c: &mut Criterion) {
    let store = build_vector_store(100);
    let coarse = empty_coarse_output();
    let seed_clv = make_clv(0);
    let recall = FineRecall::with_default_config();

    c.bench_function("fine_recall_100_blocks", |b| {
        b.iter(|| {
            black_box(
                recall
                    .rank(black_box(FineRecallInput {
                        coarse_output: black_box(&coarse),
                        seed_clv: black_box(&seed_clv),
                        vector_store: black_box(&store),
                        block_clvs: black_box(None),
                        top_k: black_box(500),
                    }))
                    .expect("rank should succeed"),
            )
        })
    });
}

/// 基准 2: 1000 Block 精排（典型场景）
fn bench_fine_recall_1000_blocks(c: &mut Criterion) {
    let store = build_vector_store(1000);
    let coarse = empty_coarse_output();
    let seed_clv = make_clv(0);
    let recall = FineRecall::with_default_config();

    c.bench_function("fine_recall_1000_blocks", |b| {
        b.iter(|| {
            black_box(
                recall
                    .rank(black_box(FineRecallInput {
                        coarse_output: black_box(&coarse),
                        seed_clv: black_box(&seed_clv),
                        vector_store: black_box(&store),
                        block_clvs: black_box(None),
                        top_k: black_box(500),
                    }))
                    .expect("rank should succeed"),
            )
        })
    });
}

/// 基准 3: 10000 Block 精排（大规模场景）
fn bench_fine_recall_10000_blocks(c: &mut Criterion) {
    let store = build_vector_store(10000);
    let coarse = empty_coarse_output();
    let seed_clv = make_clv(0);
    let recall = FineRecall::with_default_config();

    c.bench_function("fine_recall_10000_blocks", |b| {
        b.iter(|| {
            black_box(
                recall
                    .rank(black_box(FineRecallInput {
                        coarse_output: black_box(&coarse),
                        seed_clv: black_box(&seed_clv),
                        vector_store: black_box(&store),
                        block_clvs: black_box(None),
                        top_k: black_box(500),
                    }))
                    .expect("rank should succeed"),
            )
        })
    });
}

/// 基准 4: p95 延迟测量（1000 Block × 100 次采样）
///
/// 红线: p95 <50ms（spec.md §P3 验收标准）
fn bench_fine_recall_p95_latency(c: &mut Criterion) {
    let block_count = 1000;
    let store = build_vector_store(block_count);
    let coarse = empty_coarse_output();
    let seed_clv = make_clv(0);
    let recall = FineRecall::with_default_config();

    let mut group = c.benchmark_group("fine_recall_p95_latency");
    group.sample_size(100);

    group.bench_function("p95_latency", |b| {
        b.iter_custom(|iters| {
            let mut latencies: Vec<Duration> = Vec::with_capacity(iters as usize);
            let start_total = Instant::now();
            for _ in 0..iters {
                let start = Instant::now();
                black_box(
                    recall
                        .rank(black_box(FineRecallInput {
                            coarse_output: black_box(&coarse),
                            seed_clv: black_box(&seed_clv),
                            vector_store: black_box(&store),
                            block_clvs: black_box(None),
                            top_k: black_box(500),
                        }))
                        .expect("rank should succeed"),
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

            eprintln!(
                "[fine_recall_p95] samples={n}, mean={mean:?}, p50={p50:?}, p95={p95:?}, p99={p99:?}, \
                 p95<50ms={}",
                p95 < Duration::from_millis(50)
            );

            total
        })
    });
    group.finish();
}

/// 基准 5: 精确 CLV 重排 vs 无重排性能对比
///
/// 验证精确重排的性能开销（block_clvs 查找 + CLV::cosine_similarity）
fn bench_fine_recall_precise_vs_no_rerank(c: &mut Criterion) {
    let block_count = 1000;
    let store = build_vector_store(block_count);

    // 构建 block_clvs 映射
    let mut block_clvs = HashMap::new();
    for i in 0..block_count {
        block_clvs.insert(format!("block-{i}"), make_clv(i as u64));
    }

    let coarse = empty_coarse_output();
    let seed_clv = make_clv(0);

    let mut group = c.benchmark_group("fine_recall_precise_vs_no_rerank");

    // 无精确重排（仅用 HNSW score）
    group.bench_function("no_precise_rerank", |b| {
        let recall = FineRecall::new(FineRecallConfig {
            overfetch_factor: 2.0,
            precise_rerank: false,
        });
        b.iter(|| {
            black_box(
                recall
                    .rank(black_box(FineRecallInput {
                        coarse_output: black_box(&coarse),
                        seed_clv: black_box(&seed_clv),
                        vector_store: black_box(&store),
                        block_clvs: black_box(None),
                        top_k: black_box(500),
                    }))
                    .expect("rank should succeed"),
            )
        })
    });

    // 精确重排（用 block_clvs）
    group.bench_function("with_precise_rerank", |b| {
        let recall = FineRecall::new(FineRecallConfig {
            overfetch_factor: 2.0,
            precise_rerank: true,
        });
        b.iter(|| {
            black_box(
                recall
                    .rank(black_box(FineRecallInput {
                        coarse_output: black_box(&coarse),
                        seed_clv: black_box(&seed_clv),
                        vector_store: black_box(&store),
                        block_clvs: black_box(Some(&block_clvs)),
                        top_k: black_box(500),
                    }))
                    .expect("rank should succeed"),
            )
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_fine_recall_100_blocks,
    bench_fine_recall_1000_blocks,
    bench_fine_recall_10000_blocks,
    bench_fine_recall_p95_latency,
    bench_fine_recall_precise_vs_no_rerank,
);
criterion_main!(benches);
