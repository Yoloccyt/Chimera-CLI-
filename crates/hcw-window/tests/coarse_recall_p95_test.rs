//! HCW-Sparse v2.0 粗召回 p95 延迟红线守护测试
//!
//! 对应任务: P3-W9.1.2（spec.md P3 内环升级 KPI 表格）
//! 验证红线: **1000 模块 p95 < 10ms**（粗召回 SLO）
//!
//! # 设计
//! 与 `benches/coarse_recall.rs::bench_coarse_recall_p95_latency` 互补：
//! - bench 输出延迟分布供人工核验（不 panic CI）
//! - 本测试断言 p95 < 10ms 红线（CI 失败时阻断）
//!
//! # 运行
//! ```bash
//! # release 模式运行（推荐，debug 模式可能误判红线）
//! cargo test -p hcw-window --test coarse_recall_p95_test --release -- --ignored --nocapture
//! ```
//!
//! # WHY `#[ignore]`
//! 性能红线测试需在 release 模式运行，debug 模式下 HNSW/CLV 计算未优化，
//! 可能误判红线失败。`#[ignore]` 避免普通 `cargo test` 触发，需显式 `--ignored`。

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::time::{Duration, Instant};

use hcw_window::recall::{
    CoChangeMatrix, CoarseRecallBuilder, CoarseRecallInput, ModuleGraph, RecallWeights,
};
use nexus_core::CLV;

/// CLV 维度
const CLV_DIM: usize = 512;

/// 测试模块数（spec.md 典型场景）
const MODULE_COUNT: usize = 1000;

/// 采样次数（足够稳定 p95 估计）
const SAMPLE_COUNT: usize = 1000;

/// 种子模块数
const SEED_COUNT: usize = 3;

/// Top-K 模块数（spec.md 要求 100）
const TOP_K: usize = 100;

/// p95 阈值（spec.md P3-W9.1 红线：<10ms）
const P95_THRESHOLD_MS: u64 = 10;

/// 生成确定性伪随机 CLV（与 vector_bench.rs::make_vector 同模式）
fn make_clv(seed: u64) -> CLV {
    let v: Vec<f32> = (0..CLV_DIM)
        .map(|j| {
            let h = seed
                .wrapping_mul(7)
                .wrapping_add((j as u64).wrapping_mul(13))
                .wrapping_mul(31);
            (h % 100003) as f32 / 100003.0 + 0.001
        })
        .collect();
    CLV::from_vec(v).expect("CLV dimension must be 512")
}

/// 构造测试用 Project 图（与 bench 同模式：链状 + 星型混合 + 5% 孤立节点）
fn build_module_graph(module_count: usize) -> ModuleGraph {
    let mut edges: Vec<(String, String)> = Vec::with_capacity(module_count * 2);

    // 链状
    for i in 0..module_count.saturating_sub(1) {
        edges.push((format!("m_{i}"), format!("m_{}", i + 1)));
    }

    // 星型
    for hub_idx in (0..module_count).step_by(10) {
        for offset in 1..=5 {
            let leaf_idx = hub_idx + offset;
            if leaf_idx < module_count {
                edges.push((format!("m_{hub_idx}"), format!("m_{leaf_idx}")));
            }
        }
    }

    // 5% 孤立节点
    let isolated_count = module_count / 20;
    let isolated_nodes: Vec<String> = (0..isolated_count).map(|i| format!("iso_{i}")).collect();

    ModuleGraph::from_edges(edges, isolated_nodes)
}

/// 构造测试用共变更矩阵（5% 稀疏填充）
fn build_cochange_matrix(module_count: usize) -> CoChangeMatrix {
    let mut matrix = CoChangeMatrix::new();
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

/// 构造测试用 module_clvs
fn build_module_clvs(module_count: usize) -> HashMap<String, CLV> {
    (0..module_count)
        .map(|i| (format!("m_{i}"), make_clv(i as u64)))
        .collect()
}

/// 取延迟分布的百分位
fn percentile(latencies: &[Duration], p: f64) -> Duration {
    let n = latencies.len();
    if n == 0 {
        return Duration::ZERO;
    }
    let idx = ((n as f64 * p) as usize).min(n.saturating_sub(1));
    latencies[idx]
}

/// 粗召回 p95 延迟红线守护（P3-W9.1.2 spec.md 红线）
///
/// # 红线
/// 1000 模块 × 3 种子 × top_k=100 场景下,p95 < 10ms
///
/// # 测试逻辑
/// 1. 预构建 recall 引擎（ModuleGraph + CoChangeMatrix + module_clvs）
/// 2. 先做一次 warmup（HNSW/HashMap 首次访问可能触发缓存填充）
/// 3. 收集 1000 次 recall 延迟样本
/// 4. 排序取 p50/p95/p99
/// 5. 断言 p95 < 10ms
#[test]
#[ignore = "性能红线测试:需 release 模式运行,debug 模式可能误判红线 \
            (cargo test -p hcw-window --test coarse_recall_p95_test --release -- --ignored --nocapture)"]
fn test_coarse_recall_p95_below_10ms() {
    // 1. 构建 recall 引擎与输入数据
    let graph = build_module_graph(MODULE_COUNT);
    let cochange = build_cochange_matrix(MODULE_COUNT);
    let module_clvs = build_module_clvs(MODULE_COUNT);
    let seeds: Vec<String> = (0..SEED_COUNT).map(|i| format!("m_{i}")).collect();
    let seed_clv = make_clv(0);

    let recall = CoarseRecallBuilder::new()
        .with_graph(graph)
        .with_cochange(cochange)
        .with_weights(RecallWeights::DEFAULT)
        .build()
        .expect("build should succeed");

    // 2. warmup（首次 recall 可能触发 HashMap 缓存填充与分支预测器冷启动）
    let _ = recall
        .recall(CoarseRecallInput {
            seed_modules: &seeds,
            seed_clv: &seed_clv,
            module_clvs: &module_clvs,
            top_k: TOP_K,
        })
        .expect("warmup recall should succeed");

    // 3. 收集 1000 次 recall 延迟样本
    let mut latencies: Vec<Duration> = Vec::with_capacity(SAMPLE_COUNT);
    for _ in 0..SAMPLE_COUNT {
        let start = Instant::now();
        let output = recall
            .recall(CoarseRecallInput {
                seed_modules: &seeds,
                seed_clv: &seed_clv,
                module_clvs: &module_clvs,
                top_k: TOP_K,
            })
            .expect("recall should succeed");
        let elapsed = start.elapsed();
        assert_eq!(
            output.modules.len(),
            TOP_K,
            "top_k={TOP_K} 应返回 {TOP_K} 条结果(候选 {MODULE_COUNT} 模块足够)"
        );
        latencies.push(elapsed);
    }

    // 4. 排序并取百分位
    latencies.sort_unstable();
    let p50 = percentile(&latencies, 0.50);
    let p95 = percentile(&latencies, 0.95);
    let p99 = percentile(&latencies, 0.99);
    let mean = latencies.iter().sum::<Duration>() / latencies.len() as u32;
    let max = latencies[latencies.len() - 1];

    let threshold = Duration::from_millis(P95_THRESHOLD_MS);

    // 5. 输出完整延迟分布
    eprintln!(
        "[coarse_recall_p95] modules={MODULE_COUNT}, seeds={SEED_COUNT}, samples={SAMPLE_COUNT}\n\
         [coarse_recall_p95] mean={mean:?}, p50={p50:?}, p95={p95:?}, p99={p99:?}, max={max:?}\n\
         [coarse_recall_p95] threshold={threshold:?}, p95<threshold={}",
        p95 < threshold
    );

    // 6. 红线断言
    assert!(
        p95 < threshold,
        "P3-W9.1.2 红线违规:粗召回 1000 模块 p95={p95:?} ≥ {threshold:?}\n\
         延迟分布:mean={mean:?}, p50={p50:?}, p95={p95:?}, p99={p99:?}, max={max:?}\n\
         可能原因:① debug 模式未优化(应 release 模式运行);② BFS 传播深度过大;③ CLV 语义相似度计算未 SIMD 加速;④ 候选模块数过大;⑤ 图结构密集(边数过多)"
    );
}

/// 简单冒烟测试（非 ignored，CI 中快速验证）
#[test]
fn test_coarse_recall_smoke() {
    let module_count = 100; // 小规模冒烟
    let graph = build_module_graph(module_count);
    let cochange = build_cochange_matrix(module_count);
    let module_clvs = build_module_clvs(module_count);
    let seeds: Vec<String> = (0..3).map(|i| format!("m_{i}")).collect();
    let seed_clv = make_clv(0);

    let recall = CoarseRecallBuilder::new()
        .with_graph(graph)
        .with_cochange(cochange)
        .with_weights(RecallWeights::DEFAULT)
        .build()
        .expect("build should succeed");

    let output = recall
        .recall(CoarseRecallInput {
            seed_modules: &seeds,
            seed_clv: &seed_clv,
            module_clvs: &module_clvs,
            top_k: 100,
        })
        .expect("recall should succeed");

    // 100 模块全部召回
    assert_eq!(output.modules.len(), 100);
    // Top-1 应为 m_0（种子本身 dep_score=1.0）
    assert_eq!(output.modules[0].module_id, "m_0");
    // elapsed_us 应非零（实际召回消耗时间）
    // WHY 不强制 > 0:Windows 高精度计时器在某些场景下可能返回 0
    let _elapsed_us: u64 = output.elapsed_us;
}
