//! HCW-Sparse v2.0 精排 p95 延迟红线守护测试
//!
//! 对应任务: P3-W9.2.2
//! 红线: 精排 p95 <50ms（spec.md §P3 验收标准）
//!
//! # 运行方式
//! 此测试标记 `#[ignore]`，仅在 release 模式下手动运行:
//! ```powershell
//! cargo test -p hcw-window --release --test fine_recall_p95_test -- --ignored --nocapture
//! ```
//!
//! # 设计决策（WHY）
//! - `#[ignore]`: 性能红线测试需 release 模式，debug 模式可能误判红线
//! - 1000 次采样: 统计意义足够，p95 百分位在 1000 样本时误差 <5%
//! - 用 InMemoryVectorStore（O(n) KNN）而非真实 HnswStore:
//!   hcw-window (L2) 不能依赖 repo-wiki (L5) 的 HnswStore（生产依赖方向禁止），
//!   InMemoryVectorStore 比 HNSW 慢，若此实现满足 <50ms 红线，真实 HnswStore 一定能满足

use std::collections::HashMap;
use std::sync::RwLock;
use std::time::{Duration, Instant};

use hcw_window::recall::{CoarseRecallOutput, FineRecall, FineRecallConfig, FineRecallInput};
use nexus_contracts::{VectorHit, VectorStore};
use nexus_core::CLV;
// 统一使用 nexus-core 权威实现,避免多副本优化不一致
use nexus_core::cosine_similarity_slices;

// ============================================================
// 测试参数
// ============================================================

/// Block 数量（典型生产规模）
const BLOCK_COUNT: usize = 1000;

/// 采样次数（统计意义足够的样本量）
const SAMPLE_COUNT: usize = 1000;

/// Top-K 返回数（spec.md 要求 500）
const TOP_K: usize = 500;

/// p95 延迟阈值（spec.md 红线: <50ms）
const P95_THRESHOLD_MS: u64 = 50;

// ============================================================
// 测试用 VectorStore 实现
// ============================================================

/// 测试用内存 VectorStore — O(n) 遍历 + 精确余弦相似度
struct InMemoryVectorStore {
    dim: usize,
    vectors: RwLock<HashMap<String, Vec<f32>>>,
}

impl InMemoryVectorStore {
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

impl VectorStore for InMemoryVectorStore {
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
            .map(|(id, vec)| VectorHit::new(id.clone(), cosine_similarity_slices(query, vec)))
            .collect();
        if k < scored.len() {
            scored.select_nth_unstable_by(k, |a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        scored.truncate(k);
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

// ============================================================
// 辅助函数
// ============================================================

/// 构造确定性 CLV
fn make_clv(seed: u64) -> CLV {
    let v: Vec<f32> = (0..CLV::DIMENSION)
        .map(|j| ((seed.wrapping_add(j as u64)) % 100) as f32 / 100.0)
        .collect();
    CLV::from_vec(v).expect("CLV dimension should be 512")
}

/// 构建 Block 向量索引
fn build_vector_store(block_count: usize) -> InMemoryVectorStore {
    let store = InMemoryVectorStore::with_dim(512);
    for i in 0..block_count {
        let clv = make_clv(i as u64);
        store.insert(&format!("block-{i}"), clv.as_slice().to_vec());
    }
    store
}

/// 计算延迟百分位
fn percentile(sorted_latencies: &[Duration], p: f64) -> Duration {
    let n = sorted_latencies.len();
    if n == 0 {
        return Duration::ZERO;
    }
    let idx = ((n as f64 - 1.0) * p).round() as usize;
    sorted_latencies[idx.min(n - 1)]
}

// ============================================================
// p95 延迟红线守护测试
// ============================================================

/// 精排 p95 延迟红线守护（P3-W9.2.2 spec.md 红线）
///
/// 用 InMemoryVectorStore（O(n) KNN，比 HNSW 慢）验证精排逻辑性能。
/// 若此实现 p95 <50ms，真实 HnswStore 一定能满足。
#[test]
#[ignore = "性能红线测试:需 release 模式运行,debug 模式可能误判红线"]
fn test_fine_recall_p95_below_50ms() {
    // 1. 构建精排引擎与输入数据
    let store = build_vector_store(BLOCK_COUNT);
    let coarse = CoarseRecallOutput {
        modules: Vec::new(),
        elapsed_us: 0,
    };
    let seed_clv = make_clv(0);
    let recall = FineRecall::new(FineRecallConfig {
        overfetch_factor: 2.0,
        precise_rerank: true,
    });

    // 2. warmup（预热缓存，避免首次运行冷启动偏差）
    for _ in 0..10 {
        let _ = recall
            .rank(FineRecallInput {
                coarse_output: &coarse,
                seed_clv: &seed_clv,
                vector_store: &store,
                block_clvs: None,
                top_k: TOP_K,
            })
            .expect("warmup rank should succeed");
    }

    // 3. 收集 1000 次 rank() 延迟样本
    let mut latencies: Vec<Duration> = Vec::with_capacity(SAMPLE_COUNT);
    for _ in 0..SAMPLE_COUNT {
        let start = Instant::now();
        let output = recall
            .rank(FineRecallInput {
                coarse_output: &coarse,
                seed_clv: &seed_clv,
                vector_store: &store,
                block_clvs: None,
                top_k: TOP_K,
            })
            .expect("rank should succeed");
        let elapsed = start.elapsed();
        latencies.push(elapsed);

        // 验证输出正确性（首次迭代）
        if latencies.len() == 1 {
            assert_eq!(output.blocks.len(), TOP_K, "精排应返回 {TOP_K} 个 Block");
        }
    }

    // 4. 排序并取百分位
    latencies.sort_unstable();
    let p50 = percentile(&latencies, 0.50);
    let p95 = percentile(&latencies, 0.95);
    let p99 = percentile(&latencies, 0.99);
    let mean = latencies.iter().sum::<Duration>() / latencies.len() as u32;
    let max = latencies[latencies.len() - 1];

    // 5. 输出延迟分布（用于诊断）
    eprintln!(
        "[fine_recall_p95] blocks={BLOCK_COUNT}, samples={SAMPLE_COUNT}, top_k={TOP_K}\n\
         mean={mean:?}, p50={p50:?}, p95={p95:?}, p99={p99:?}, max={max:?}\n\
         p95<50ms={} (threshold={P95_THRESHOLD_MS}ms)",
        p95 < Duration::from_millis(P95_THRESHOLD_MS)
    );

    // 6. 红线断言
    assert!(
        p95 < Duration::from_millis(P95_THRESHOLD_MS),
        "P3-W9.2.2 红线违规:精排 {BLOCK_COUNT} Block p95={p95:?} ≥ {threshold:?} \
         (InMemoryVectorStore O(n) KNN，真实 HnswStore 应更快)",
        threshold = Duration::from_millis(P95_THRESHOLD_MS)
    );
}

/// 精确 CLV 重排 vs 无重排延迟对比测试
///
/// 验证精确重排的性能开销可接受（<10ms 增量）
#[test]
#[ignore = "性能对比测试:需 release 模式运行"]
fn test_precise_rerank_overhead_acceptable() {
    let store = build_vector_store(BLOCK_COUNT);
    let coarse = CoarseRecallOutput {
        modules: Vec::new(),
        elapsed_us: 0,
    };
    let seed_clv = make_clv(0);

    // 构建 block_clvs 映射
    let mut block_clvs = HashMap::new();
    for i in 0..BLOCK_COUNT {
        block_clvs.insert(format!("block-{i}"), make_clv(i as u64));
    }

    // 无精确重排
    let recall_no_rerank = FineRecall::new(FineRecallConfig {
        overfetch_factor: 2.0,
        precise_rerank: false,
    });
    // 有精确重排
    let recall_with_rerank = FineRecall::new(FineRecallConfig {
        overfetch_factor: 2.0,
        precise_rerank: true,
    });

    // warmup
    for _ in 0..10 {
        let _ = recall_no_rerank
            .rank(FineRecallInput {
                coarse_output: &coarse,
                seed_clv: &seed_clv,
                vector_store: &store,
                block_clvs: None,
                top_k: TOP_K,
            })
            .expect("warmup should succeed");
        let _ = recall_with_rerank
            .rank(FineRecallInput {
                coarse_output: &coarse,
                seed_clv: &seed_clv,
                vector_store: &store,
                block_clvs: Some(&block_clvs),
                top_k: TOP_K,
            })
            .expect("warmup should succeed");
    }

    // 测量无重排延迟
    let mut no_rerank_latencies: Vec<Duration> = Vec::with_capacity(100);
    for _ in 0..100 {
        let start = Instant::now();
        let _ = recall_no_rerank
            .rank(FineRecallInput {
                coarse_output: &coarse,
                seed_clv: &seed_clv,
                vector_store: &store,
                block_clvs: None,
                top_k: TOP_K,
            })
            .expect("rank should succeed");
        no_rerank_latencies.push(start.elapsed());
    }
    no_rerank_latencies.sort_unstable();
    let no_rerank_p95 = percentile(&no_rerank_latencies, 0.95);

    // 测量有重排延迟
    let mut with_rerank_latencies: Vec<Duration> = Vec::with_capacity(100);
    for _ in 0..100 {
        let start = Instant::now();
        let _ = recall_with_rerank
            .rank(FineRecallInput {
                coarse_output: &coarse,
                seed_clv: &seed_clv,
                vector_store: &store,
                block_clvs: Some(&block_clvs),
                top_k: TOP_K,
            })
            .expect("rank should succeed");
        with_rerank_latencies.push(start.elapsed());
    }
    with_rerank_latencies.sort_unstable();
    let with_rerank_p95 = percentile(&with_rerank_latencies, 0.95);

    let overhead = with_rerank_p95.saturating_sub(no_rerank_p95);
    eprintln!(
        "[precise_rerank_overhead] no_rerank_p95={no_rerank_p95:?}, \
         with_rerank_p95={with_rerank_p95:?}, overhead={overhead:?}"
    );

    // 精确重排开销应 <10ms（1000 Block × 512-dim cosine_similarity ≈ 1-2ms）
    assert!(
        overhead < Duration::from_millis(10),
        "精确重排开销过大: overhead={overhead:?} ≥ 10ms"
    );
}
