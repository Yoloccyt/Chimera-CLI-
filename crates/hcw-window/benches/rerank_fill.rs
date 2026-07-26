//! HCW-Sparse v2.0 重排填充基准 — 多目标密度贪心 → 1M 等效窗口
//!
//! 对应任务: P3-W10.1.2（spec.md P3 内环升级）
//! 验证红线: **重排填充 p95 < 100ms**（spec.md KPI 表格）
//!
//! # 基准项
//! - `rerank_fill_100_blocks`: 100 Block × L2_256K 预算（最小基线）
//! - `rerank_fill_500_blocks`: 500 Block × L2_256K 预算（典型场景，spec.md 要求）
//! - `rerank_fill_5000_blocks`: 5000 Block × L3_1M 预算（性能上限）
//! - `rerank_fill_p95_latency`: iter_custom 收集 p95 延迟，断言 <100ms
//!
//! # 设计说明
//! Block 用确定性伪随机生成（与 coarse_recall.rs / fine_recall.rs 一致），
//! 避免引入 rand 依赖，保证基准可复现。
//!
//! # 运行
//! ```bash
//! # 全部基准
//! cargo bench -p hcw-window --bench rerank_fill
//! # 仅 500 Block 基准（典型场景）
//! cargo bench -p hcw-window --bench rerank_fill -- "500_blocks"
//! # 仅 p95 延迟基准
//! cargo bench -p hcw-window --bench rerank_fill -- "p95"
//! ```

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::hint::black_box;
use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, Criterion};
use hcw_window::recall::{
    BlockScore, FineRecallOutput, RerankFill, RerankFillConfig, RerankFillInput, WindowBudget,
};

/// 构造测试用 BlockScore 列表（确定性生成，避免 rand 依赖）
///
/// # 参数
/// - `count`: Block 数量
/// - `module_count`: 模块数量（Block 均匀分布到各模块）
///
/// # 生成规则
/// - `block_id`: `block-{i}` 确定性
/// - `score`: `1.0 - i * 0.0001`（递减，保证排序确定性）
/// - `source_module`: `module-{i % module_count}`（均匀分布）
/// - `token_count`: 1024（固定，简化密度计算）
fn build_blocks(count: usize, module_count: usize) -> Vec<BlockScore> {
    (0..count)
        .map(|i| {
            BlockScore::new(
                format!("block-{i}"),
                1.0 - (i as f32 * 0.0001),
                1.0 - (i as f32 * 0.0001),
                format!("module-{}", i % module_count),
                1024,
            )
        })
        .collect()
}

/// 构造 block_tokens 映射
fn build_block_tokens(blocks: &[BlockScore]) -> HashMap<String, usize> {
    blocks
        .iter()
        .map(|b| (b.block_id.clone(), b.token_count))
        .collect()
}

/// 构造精排输出（从 Block 列表）
fn build_fine_output(blocks: Vec<BlockScore>) -> FineRecallOutput {
    FineRecallOutput {
        blocks,
        elapsed_us: 0,
        candidate_count: 0,
    }
}

/// 基准 1: 100 Block × L2_256K 预算（最小基线）
fn bench_rerank_fill_100_blocks(c: &mut Criterion) {
    let blocks = build_blocks(100, 10);
    let fine_output = build_fine_output(blocks.clone());
    let block_tokens = build_block_tokens(&blocks);
    let recall = RerankFill::new(RerankFillConfig {
        window_budget: WindowBudget::L2_256K,
        diversity_alpha: 0.2,
        enable_sparse_pattern: true,
    });

    let mut group = c.benchmark_group("rerank_fill_100_blocks");
    group.sample_size(100);
    group.bench_function("fill", |b| {
        b.iter(|| {
            let output = recall
                .fill(black_box(RerankFillInput {
                    fine_output: black_box(&fine_output),
                    block_tokens: black_box(&block_tokens),
                }))
                .expect("fill should succeed");
            black_box(output);
        });
    });
    group.finish();
}

/// 基准 2: 500 Block × L2_256K 预算（典型场景，spec.md 要求）
fn bench_rerank_fill_500_blocks(c: &mut Criterion) {
    let blocks = build_blocks(500, 20);
    let fine_output = build_fine_output(blocks.clone());
    let block_tokens = build_block_tokens(&blocks);
    let recall = RerankFill::new(RerankFillConfig {
        window_budget: WindowBudget::L2_256K,
        diversity_alpha: 0.2,
        enable_sparse_pattern: true,
    });

    let mut group = c.benchmark_group("rerank_fill_500_blocks");
    group.sample_size(100);
    group.bench_function("fill", |b| {
        b.iter(|| {
            let output = recall
                .fill(black_box(RerankFillInput {
                    fine_output: black_box(&fine_output),
                    block_tokens: black_box(&block_tokens),
                }))
                .expect("fill should succeed");
            black_box(output);
        });
    });
    group.finish();
}

/// 基准 3: 5000 Block × L3_1M 预算（性能上限）
fn bench_rerank_fill_5000_blocks(c: &mut Criterion) {
    let blocks = build_blocks(5000, 50);
    let fine_output = build_fine_output(blocks.clone());
    let block_tokens = build_block_tokens(&blocks);
    let recall = RerankFill::new(RerankFillConfig {
        window_budget: WindowBudget::L3_1M, // 1M 等效（128K 实际 × 8x 压缩）
        diversity_alpha: 0.2,
        enable_sparse_pattern: true,
    });

    let mut group = c.benchmark_group("rerank_fill_5000_blocks");
    group.sample_size(50); // 5000 Block 单次较慢，减少样本数
    group.bench_function("fill", |b| {
        b.iter(|| {
            let output = recall
                .fill(black_box(RerankFillInput {
                    fine_output: black_box(&fine_output),
                    block_tokens: black_box(&block_tokens),
                }))
                .expect("fill should succeed");
            black_box(output);
        });
    });
    group.finish();
}

/// 基准 4: p95 延迟测量（iter_custom 收集单次延迟样本）
///
/// 验证 spec.md P3-W10.1 红线:**500 Block p95 < 100ms**
///
/// # 设计
/// `iter_custom` 让我们手动控制每次迭代的计时，收集每单次 `fill` 调用的延迟样本。
/// criterion 调用 `iters` 次（由 --measurement-time 决定，通常 100-3000），
/// 收集后排序取 p95，通过 `eprintln!` 输出供人工核验。
///
/// # 红线断言
/// 真正的"p95 < 100ms"红线断言不在此 bench 中（bench 不应 panic 失败 CI）。
/// 红线断言由独立测试 `tests/rerank_fill_p95_test.rs::test_rerank_fill_p95_below_100ms` 守护。
///
/// # 输出示例
/// ```text
/// [rerank_fill_p95] samples=100, mean=0.5ms, p50=0.4ms, p95=1.2ms, p99=2.8ms, p95<100ms=true
/// ```
fn bench_rerank_fill_p95_latency(c: &mut Criterion) {
    let blocks = build_blocks(500, 20);
    let fine_output = build_fine_output(blocks.clone());
    let block_tokens = build_block_tokens(&blocks);
    let recall = RerankFill::new(RerankFillConfig {
        window_budget: WindowBudget::L2_256K,
        diversity_alpha: 0.2,
        enable_sparse_pattern: true,
    });

    let mut group = c.benchmark_group("rerank_fill_p95_latency");
    group.sample_size(100);

    group.bench_function("p95_latency", |b| {
        b.iter_custom(|iters| {
            let mut latencies: Vec<Duration> = Vec::with_capacity(iters as usize);
            let start_total = Instant::now();
            for _ in 0..iters {
                let start = Instant::now();
                black_box(
                    recall
                        .fill(black_box(RerankFillInput {
                            fine_output: black_box(&fine_output),
                            block_tokens: black_box(&block_tokens),
                        }))
                        .expect("fill should succeed"),
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
                "[rerank_fill_p95] samples={n}, mean={mean:?}, p50={p50:?}, p95={p95:?}, p99={p99:?}, \
                 p95<100ms={}",
                p95 < Duration::from_millis(100)
            );

            total
        })
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_rerank_fill_100_blocks,
    bench_rerank_fill_500_blocks,
    bench_rerank_fill_5000_blocks,
    bench_rerank_fill_p95_latency,
);
criterion_main!(benches);
