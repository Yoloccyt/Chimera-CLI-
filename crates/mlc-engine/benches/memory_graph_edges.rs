//! 记忆图谱建边与召回性能基准(closure Stage C-12 + Phase 8.3 HNSW 改造)
//!
//! 对应架构层: L2 Memory(mlc-engine)
//! 对应 ADR: ADR-049 决策 6(图谱建边拒绝 O(n²) 全对比较)
//! 对应验收门禁: **1 万节点建边 < 1s**(实施计划 P4-9)
//!
//! Phase 8.3:build_semantic_edges 按节点数路由——≤1K 精确路径、
//! 超过 1K 走 HNSW 候选路径(O(n·log n))。本 bench 覆盖 1K(精确)/ 2K / 10K
//! (HNSW),实证 10K 建边 <1s 门禁(此前精确路径平方增长外推 50s+ 不可达)。
//!
//! # 运行
//!
//! ```powershell
//! cargo bench -p mlc-engine --bench memory_graph_edges
//! ```

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use mlc_engine::memory_graph::{MemoryGraph, MemoryNode, MemoryNodeType};
use nexus_core::CLV;

/// 确定性伪随机嵌入(索引驱动的正弦分布,避免引入 rand 依赖)
///
/// WHY 正弦而非常数:常数嵌入使所有相似度恒为 1,Top-K 选择退化;
/// 正弦分布让节点间相似度差异化,建边路径覆盖阈值过滤分支。
fn synthetic_embedding(seed: usize) -> CLV {
    let v: Vec<f32> = (0..512)
        .map(|d| ((seed * 31 + d) as f32 * 0.01).sin())
        .collect();
    CLV::from_vec(v).expect("512 维向量构造必然成功")
}

/// 构建 n 节点图谱(未建边)
fn graph_with_nodes(n: usize) -> MemoryGraph {
    let mut graph = MemoryGraph::new();
    for i in 0..n {
        graph.insert_node(MemoryNode {
            node_id: format!("node-{i}"),
            content: format!("memory content {i}"),
            embedding: synthetic_embedding(i),
            node_type: MemoryNodeType::CodeSnippet,
            success_associated: i % 3 == 0,
        });
    }
    graph
}

/// 语义建边规模曲线(1K 精确 / 2K、10K HNSW)— 实证 10K <1s 门禁
///
/// Phase 8.3:1K 走精确路径(基线 464ms);2K、10K 走 HNSW 路径
/// (O(n·log n)),验证 10K 建边跌破 <1s 门禁(此前精确平方增长不可达)。
fn build_semantic_edges_scale(c: &mut Criterion) {
    let mut group = c.benchmark_group("memory_graph/build_semantic_edges");
    // bench 内重复建边开销大,降低采样量保证总时长可控
    group.sample_size(10);
    for &n in &[1_000usize, 2_000, 10_000] {
        group.bench_function(BenchmarkId::from_parameter(n), |b| {
            b.iter_batched(
                || graph_with_nodes(n),
                |mut graph| {
                    graph.build_semantic_edges(black_box(0.3));
                    black_box(graph.edge_count());
                },
                criterion::BatchSize::LargeInput,
            );
        });
    }
    group.finish();
}

/// 图谱召回延迟(2K 节点已建边,种子 + BFS depth=2,验收 <10ms)
fn recall_with_graph(c: &mut Criterion) {
    let mut graph = graph_with_nodes(2_000);
    graph.build_semantic_edges(0.3);
    let query = synthetic_embedding(777);
    c.bench_function("memory_graph/recall_2k_depth2", |b| {
        b.iter(|| {
            let hits = graph.recall_with_graph(black_box(&query), black_box(2), black_box(10));
            black_box(hits);
        });
    });
}

criterion_group!(benches, build_semantic_edges_scale, recall_with_graph);
criterion_main!(benches);
