//! Task 3.6: RAG 混合检索(RRF) vs dense-only 延迟对比基准
//!
//! 对应 Task 3.6:验证 `hybrid_search`(RRF 融合)相比 `dense-only` 检索的延迟开销。
//!
//! # 基准场景
//!
//! - 1000 文档规模(模拟小型知识库)
//! - 10000 文档规模(模拟中型知识库)
//! - dense-only:HNSW 单路检索 + Top-K
//! - hybrid:HNSW + FTS5 双路检索 + RRF 融合 + Top-K
//!
//! # 预期结果
//!
//! RRF 融合本身是 O(N) HashMap 合并 + Top-K 选择(select_nth_unstable),
//! 在 1000/10000 规模下延迟应 < 1ms,远低于 HNSW 检索延迟(通常 5-50ms)。
//! 因此 hybrid vs dense-only 的延迟差异应 < 20%(融合开销被检索开销主导)。
//!
//! # WHY 不直接调用 HnswStore
//!
//! 基准测试聚焦 RRF 融合算法本身的延迟,而非 HNSW 检索延迟。
//! HNSW 检索延迟已由 `vector_bench.rs` 覆盖,这里用预生成的 doc_id 列表
//! 模拟检索结果,隔离 RRF 融合算法的性能特征。

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use repo_wiki::search::{hybrid_search, rrf_fuse, HybridSearchConfig};

/// 生成 N 个唯一 doc_id(模拟 dense 检索结果,按相关性降序)
fn make_dense_results(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("doc-d-{i}")).collect()
}

/// 生成 N 个 doc_id(模拟 sparse 检索结果,与 dense 有部分重叠)
///
/// WHY 部分重叠而非完全相同:真实场景中 HNSW 与 FTS5 召回的文档
/// 有交集但不完全相同(向量检索关注语义,全文检索关注关键词)。
/// 50% 重叠率是经验值(基于 TREC 实验数据)。
fn make_sparse_results(n: usize) -> Vec<String> {
    // 前 50% 与 dense 完全重叠(相同 doc_id),后 50% 是 sparse 独有
    let overlap = n / 2;
    (0..overlap)
        .map(|i| format!("doc-d-{i}"))
        .chain((0..(n - overlap)).map(|i| format!("doc-s-{i}")))
        .collect()
}

/// 基准:RRF 融合延迟(dense + sparse → Top-K)
///
/// 直接调用 `rrf_fuse`,隔离融合算法本身的延迟。
fn bench_rrf_fuse(c: &mut Criterion) {
    let config = HybridSearchConfig::default();
    let top_k = 10;

    let mut group = c.benchmark_group("rrf_fuse");
    for size in [1000, 10000].iter() {
        let dense = make_dense_results(*size);
        let sparse = make_sparse_results(*size);

        group.bench_with_input(
            BenchmarkId::from_parameter(size),
            &(&dense, &sparse),
            |b, &(d, s)| {
                b.iter(|| {
                    let results = rrf_fuse(
                        black_box(d),
                        black_box(s),
                        black_box(&config),
                        black_box(top_k),
                    );
                    black_box(results);
                });
            },
        );
    }
    group.finish();
}

/// 基准:dense-only 检索延迟(仅 dense 一路,无融合)
///
/// 模拟"仅 HNSW 单路检索"的场景,sparse 结果为空。
/// `hybrid_search` 内部会走降级路径(仅返回 dense 排名)。
fn bench_dense_only(c: &mut Criterion) {
    let config = HybridSearchConfig::default();
    let top_k = 10;

    let mut group = c.benchmark_group("dense_only");
    for size in [1000, 10000].iter() {
        let dense = make_dense_results(*size);
        let sparse: Vec<String> = Vec::new();

        group.bench_with_input(
            BenchmarkId::from_parameter(size),
            &(&dense, &sparse),
            |b, &(d, s)| {
                b.iter(|| {
                    let results = hybrid_search(
                        black_box(d),
                        black_box(s),
                        black_box(&config),
                        black_box(top_k),
                    );
                    black_box(results);
                });
            },
        );
    }
    group.finish();
}

/// 基准:hybrid 检索延迟(dense + sparse 双路 RRF 融合)
///
/// 完整的混合检索路径,两路均有结果,触发 RRF 融合。
/// 与 `bench_dense_only` 对比可评估融合开销占比。
fn bench_hybrid_search(c: &mut Criterion) {
    let config = HybridSearchConfig::default();
    let top_k = 10;

    let mut group = c.benchmark_group("hybrid_search");
    for size in [1000, 10000].iter() {
        let dense = make_dense_results(*size);
        let sparse = make_sparse_results(*size);

        group.bench_with_input(
            BenchmarkId::from_parameter(size),
            &(&dense, &sparse),
            |b, &(d, s)| {
                b.iter(|| {
                    let results = hybrid_search(
                        black_box(d),
                        black_box(s),
                        black_box(&config),
                        black_box(top_k),
                    );
                    black_box(results);
                });
            },
        );
    }
    group.finish();
}

criterion_group!(
    benches,
    bench_rrf_fuse,
    bench_dense_only,
    bench_hybrid_search
);
criterion_main!(benches);
