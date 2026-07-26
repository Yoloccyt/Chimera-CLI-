//! HCW-Sparse v2.0 粗召回基准 — Project 图联合传播 → 100 模块
//!
//! 对应任务: P3-W9.1.2（spec.md P3 内环升级）
//! 验证红线: **粗召回 p95 < 10ms**（spec.md KPI 表格）
//!
//! # 基准项
//! - `coarse_recall_100_modules`: 100 模块 × 3 种子 × top_k=100（最小基线）
//! - `coarse_recall_1000_modules`: 1000 模块 × 3 种子 × top_k=100（典型场景）
//! - `coarse_recall_5000_modules`: 5000 模块 × 5 种子 × top_k=100（性能上限）
//! - `coarse_recall_p95_latency`: iter_custom 收集 p95 延迟，断言 <10ms
//!
//! # 设计说明
//! 模块图用确定性算法生成（链状 + 星型混合），保证：
//! 1. 每次运行结果一致（无随机性导致 bench 抖动）
//! 2. 图结构有足够路径让 BFS 传播（链状产生 distance 衰减）
//! 3. 节点度数适中（星型 hub 节点连接多个 leaf）
//!
//! CLV 用确定性伪随机生成（与 vector_bench.rs 一致），避免引入 rand 依赖。
//!
//! # 运行
//! ```bash
//! # 全部基准
//! cargo bench -p hcw-window --bench coarse_recall
//! # 仅 100 模块基准（快速验证）
//! cargo bench -p hcw-window --bench coarse_recall -- "100_modules"
//! # 仅 p95 延迟基准
//! cargo bench -p hcw-window --bench coarse_recall -- "p95"
//! ```

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::hint::black_box;
use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, Criterion};
use hcw_window::recall::{
    CoChangeMatrix, CoarseRecallBuilder, CoarseRecallInput, ModuleGraph, RecallWeights,
};
use nexus_core::CLV;

/// CLV 固定维度
const CLV_DIM: usize = 512;

/// 种子数量（小规模基准）
const SEED_COUNT_SMALL: usize = 3;

/// 种子数量（大规模基准）
const SEED_COUNT_LARGE: usize = 5;

/// Top-K 模块数（spec.md 要求 100）
const TOP_K: usize = 100;

/// 生成确定性伪随机 CLV
///
/// 与 `vector_bench.rs::make_vector` 同模式，确保跨基准一致性。
/// WHY 不用 `rand` crate:① 减少依赖；② 避免随机性导致 bench 抖动；
/// ③ 可复现（每次运行结果完全一致）。
fn make_clv(seed: u64) -> CLV {
    let v: Vec<f32> = (0..CLV_DIM)
        .map(|j| {
            let h = seed
                .wrapping_mul(7)
                .wrapping_add((j as u64).wrapping_mul(13))
                .wrapping_mul(31);
            (h % 100003) as f32 / 100003.0 + 0.001 // 避免零向量
        })
        .collect();
    CLV::from_vec(v).expect("CLV dimension must be 512")
}

/// 构造测试用 Project 图（链状 + 星型混合）
///
/// # 拓扑
/// - 链状: `m_0 → m_1 → m_2 → ... → m_{n-1}`（BFS 传播路径）
/// - 星型: 每 10 个节点附加 `m_{10k} → m_{10k+1..10k+5}`（hub 节点连接多个 leaf）
/// - 孤立: 5% 节点无依赖（验证候选并集逻辑）
///
/// # 复杂度
/// 节点 N，边 E ≈ 1.5N（链 N-1 + 星型 0.5N）
fn build_module_graph(module_count: usize) -> ModuleGraph {
    let mut edges: Vec<(String, String)> = Vec::with_capacity(module_count * 2);

    // 链状: m_i → m_{i+1}
    for i in 0..module_count.saturating_sub(1) {
        edges.push((format!("m_{i}"), format!("m_{}", i + 1)));
    }

    // 星型: 每 10 个节点做 hub，连接后 5 个节点
    for hub_idx in (0..module_count).step_by(10) {
        for offset in 1..=5 {
            let leaf_idx = hub_idx + offset;
            if leaf_idx < module_count {
                edges.push((format!("m_{hub_idx}"), format!("m_{leaf_idx}")));
            }
        }
    }

    // 5% 孤立节点（仅作为 module_clvs 候选，不在图中）
    let isolated_count = module_count / 20;
    let isolated_nodes: Vec<String> = (0..isolated_count).map(|i| format!("iso_{i}")).collect();

    ModuleGraph::from_edges(edges, isolated_nodes)
}

/// 构造测试用共变更矩阵（稀疏填充 5% 模块对）
///
/// WHY 5%: HCW 典型场景下大多数模块对无共变更历史（稀疏度 > 95%），
/// 稀疏矩阵能真实反映生产环境性能。
fn build_cochange_matrix(module_count: usize) -> CoChangeMatrix {
    let mut matrix = CoChangeMatrix::new();
    // 每 50 个模块对，共变更 1-3 次
    let pair_count = module_count / 50;
    for i in 0..pair_count {
        let a = format!("m_{}", (i * 7) % module_count);
        let b = format!("m_{}", (i * 13 + 3) % module_count);
        if a != b {
            for _ in 0..=(i % 3) {
                matrix.record(a.clone(), b.clone());
            }
        }
    }
    matrix
}

/// 构造测试用 module_clvs（每个模块一个 CLV）
fn build_module_clvs(module_count: usize) -> HashMap<String, CLV> {
    (0..module_count)
        .map(|i| (format!("m_{i}"), make_clv(i as u64)))
        .collect()
}

/// 构造测试用种子模块（前 SEED_COUNT 个模块）
fn build_seeds(seed_count: usize) -> Vec<String> {
    (0..seed_count).map(|i| format!("m_{i}")).collect()
}

/// 基准 1: 100 模块 × 3 种子 × top_k=100（最小基线）
fn bench_coarse_recall_100_modules(c: &mut Criterion) {
    let module_count = 100;
    let graph = build_module_graph(module_count);
    let cochange = build_cochange_matrix(module_count);
    let module_clvs = build_module_clvs(module_count);
    let seeds = build_seeds(SEED_COUNT_SMALL);
    let seed_clv = make_clv(0);

    let recall = CoarseRecallBuilder::new()
        .with_graph(graph)
        .with_cochange(cochange)
        .with_weights(RecallWeights::DEFAULT)
        .build()
        .expect("build should succeed");

    let input = CoarseRecallInput {
        seed_modules: &seeds,
        seed_clv: &seed_clv,
        module_clvs: &module_clvs,
        top_k: TOP_K,
    };

    let mut group = c.benchmark_group("coarse_recall_100_modules");
    group.sample_size(100);
    group.bench_function("recall", |b| {
        b.iter(|| {
            let output = recall
                .recall(black_box(CoarseRecallInput {
                    seed_modules: black_box(&seeds),
                    seed_clv: black_box(&seed_clv),
                    module_clvs: black_box(&module_clvs),
                    top_k: black_box(TOP_K),
                }))
                .expect("recall should succeed");
            black_box(output);
            // 引用 input 避免未使用告警
            let _ = &input;
        });
    });
    group.finish();
}

/// 基准 2: 1000 模块 × 3 种子 × top_k=100（典型场景）
fn bench_coarse_recall_1000_modules(c: &mut Criterion) {
    let module_count = 1000;
    let graph = build_module_graph(module_count);
    let cochange = build_cochange_matrix(module_count);
    let module_clvs = build_module_clvs(module_count);
    let seeds = build_seeds(SEED_COUNT_SMALL);
    let seed_clv = make_clv(0);

    let recall = CoarseRecallBuilder::new()
        .with_graph(graph)
        .with_cochange(cochange)
        .with_weights(RecallWeights::DEFAULT)
        .build()
        .expect("build should succeed");

    let mut group = c.benchmark_group("coarse_recall_1000_modules");
    group.sample_size(100);
    group.bench_function("recall", |b| {
        b.iter(|| {
            let output = recall
                .recall(black_box(CoarseRecallInput {
                    seed_modules: black_box(&seeds),
                    seed_clv: black_box(&seed_clv),
                    module_clvs: black_box(&module_clvs),
                    top_k: black_box(TOP_K),
                }))
                .expect("recall should succeed");
            black_box(output);
        });
    });
    group.finish();
}

/// 基准 3: 5000 模块 × 5 种子 × top_k=100（性能上限）
fn bench_coarse_recall_5000_modules(c: &mut Criterion) {
    let module_count = 5000;
    let graph = build_module_graph(module_count);
    let cochange = build_cochange_matrix(module_count);
    let module_clvs = build_module_clvs(module_count);
    let seeds = build_seeds(SEED_COUNT_LARGE);
    let seed_clv = make_clv(0);

    let recall = CoarseRecallBuilder::new()
        .with_graph(graph)
        .with_cochange(cochange)
        .with_weights(RecallWeights::DEFAULT)
        .build()
        .expect("build should succeed");

    let mut group = c.benchmark_group("coarse_recall_5000_modules");
    group.sample_size(50); // 5000 模块单次较慢，减少样本数
    group.bench_function("recall", |b| {
        b.iter(|| {
            let output = recall
                .recall(black_box(CoarseRecallInput {
                    seed_modules: black_box(&seeds),
                    seed_clv: black_box(&seed_clv),
                    module_clvs: black_box(&module_clvs),
                    top_k: black_box(TOP_K),
                }))
                .expect("recall should succeed");
            black_box(output);
        });
    });
    group.finish();
}

/// 基准 4: p95 延迟测量（iter_custom 收集单次延迟样本）
///
/// 验证 spec.md P3-W9.1 红线:**1000 模块 p95 < 10ms**
///
/// # 设计
/// `iter_custom` 让我们手动控制每次迭代的计时，收集每单次 `recall` 调用的延迟样本。
/// criterion 调用 `iters` 次（由 --measurement-time 决定，通常 100-3000），
/// 收集后排序取 p95，通过 `eprintln!` 输出供人工核验。
///
/// # 红线断言
/// 真正的"p95 < 10ms"红线断言不在此 bench 中（bench 不应 panic 失败 CI）。
/// 红线断言由独立测试 `tests/coarse_recall_p95_test.rs::test_coarse_recall_p95_below_10ms` 守护。
///
/// # 输出示例
/// ```text
/// [coarse_recall_p95] samples=100, mean=1.2ms, p50=1.1ms, p95=2.3ms, p99=3.8ms, p95<10ms=true
/// ```
fn bench_coarse_recall_p95_latency(c: &mut Criterion) {
    let module_count = 1000;
    let graph = build_module_graph(module_count);
    let cochange = build_cochange_matrix(module_count);
    let module_clvs = build_module_clvs(module_count);
    let seeds = build_seeds(SEED_COUNT_SMALL);
    let seed_clv = make_clv(0);

    let recall = CoarseRecallBuilder::new()
        .with_graph(graph)
        .with_cochange(cochange)
        .with_weights(RecallWeights::DEFAULT)
        .build()
        .expect("build should succeed");

    let mut group = c.benchmark_group("coarse_recall_p95_latency");
    group.sample_size(100);

    group.bench_function("p95_latency", |b| {
        b.iter_custom(|iters| {
            let mut latencies: Vec<Duration> = Vec::with_capacity(iters as usize);
            let start_total = Instant::now();
            for _ in 0..iters {
                let start = Instant::now();
                black_box(
                    recall
                        .recall(black_box(CoarseRecallInput {
                            seed_modules: black_box(&seeds),
                            seed_clv: black_box(&seed_clv),
                            module_clvs: black_box(&module_clvs),
                            top_k: black_box(TOP_K),
                        }))
                        .expect("recall should succeed"),
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
                "[coarse_recall_p95] samples={n}, mean={mean:?}, p50={p50:?}, p95={p95:?}, p99={p99:?}, \
                 p95<10ms={}",
                p95 < Duration::from_millis(10)
            );

            total
        })
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_coarse_recall_100_modules,
    bench_coarse_recall_1000_modules,
    bench_coarse_recall_5000_modules,
    bench_coarse_recall_p95_latency,
);
criterion_main!(benches);
