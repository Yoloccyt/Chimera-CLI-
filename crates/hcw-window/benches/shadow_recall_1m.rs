//! HCW-Sparse v2.0 1M 影子集召回率基准 — spec.md P3-W12.3.1 验收基准
//!
//! 对应任务: P3-W12.3.1（spec.md P3 阶段验收）
//! 验证红线: **1M 物理窗口影子集召回率 ≥95% + p95 延迟 < 200ms**
//!
//! # 基准项
//! - `shadow_recall_1m_recall_rate`: 5000 Block 影子集 + L3_1M 预算，输出召回率（≥95% 红线）
//! - `shadow_recall_1m_p95_latency`: 1000 次采样 p95 延迟（< 200ms 红线，spec.md KPI 表格）
//!
//! # 设计说明
//! - 与 `tests/shadow_recall_1m.rs` 互补:test 做硬性红线断言（panic 失败 CI），
//!   bench 输出延迟分布与召回率供人工核验（不 panic）
//! - Block 用确定性伪随机生成（与 rerank_fill.rs / fine_recall.rs 一致）
//! - 1000 次采样保证 p95 统计显著性
//!
//! # 运行
//! ```bash
//! # 召回率基准
//! cargo bench -p hcw-window --bench shadow_recall_1m -- "recall_rate"
//! # p95 延迟基准
//! cargo bench -p hcw-window --bench shadow_recall_1m -- "p95"
//! # 全部基准
//! cargo bench -p hcw-window --bench shadow_recall_1m
//! ```

#![forbid(unsafe_code)]

use std::collections::HashSet;
use std::hint::black_box;
use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, Criterion};
use hcw_window::recall::{
    BlockScore, FineRecallOutput, RerankFill, RerankFillConfig, RerankFillInput, WindowBudget,
};

/// 影子集 Block 总数（对齐 spec.md §4.2 重排填充 5000 Block 性能上限）
const SHADOW_BLOCK_COUNT: usize = 5000;

/// 模块总数（50 个模块均匀分布）
const SHADOW_MODULE_COUNT: usize = 50;

/// L3_1M 实际预算（128K = 131072 token）
const L3_1M_ACTUAL_TOKENS: usize = 128 * 1024;

/// 默认 Block token 数
const DEFAULT_BLOCK_TOKENS: usize = 1024;

/// Ground truth Block 数（128K ÷ 1024 = 128）
const GROUND_TRUTH_COUNT: usize = L3_1M_ACTUAL_TOKENS / DEFAULT_BLOCK_TOKENS;

/// 召回率阈值（spec.md KPI: ≥95%）
const RECALL_RATE_THRESHOLD: f32 = 0.95;

/// 构造确定性影子集 Block 列表（与 tests/shadow_recall_1m.rs 一致）
fn build_shadow_blocks(count: usize, module_count: usize) -> Vec<BlockScore> {
    (0..count)
        .map(|i| {
            BlockScore::new(
                format!("block-{i}"),
                1.0 - (i as f32 * 0.0001),
                1.0 - (i as f32 * 0.0001),
                format!("module-{}", i % module_count),
                DEFAULT_BLOCK_TOKENS,
            )
        })
        .collect()
}

/// 构造 block_tokens 映射
fn build_block_tokens(blocks: &[BlockScore]) -> std::collections::HashMap<String, usize> {
    blocks
        .iter()
        .map(|b| (b.block_id.clone(), b.token_count))
        .collect()
}

/// 构造 FineRecallOutput
fn build_fine_output(blocks: Vec<BlockScore>) -> FineRecallOutput {
    let candidate_count = blocks.len();
    FineRecallOutput {
        blocks,
        elapsed_us: 0,
        candidate_count,
    }
}

/// 计算 ground truth top-N Block ID 集合（按 score 降序取前 N）
fn compute_ground_truth(blocks: &[BlockScore], top_n: usize) -> HashSet<String> {
    let mut sorted = blocks.to_vec();
    sorted.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    sorted.into_iter().take(top_n).map(|b| b.block_id).collect()
}

/// 计算召回率
fn compute_recall_rate(filled: &[BlockScore], ground_truth: &HashSet<String>) -> f32 {
    let filled_ids: HashSet<String> = filled.iter().map(|b| b.block_id.clone()).collect();
    let intersection = filled_ids.intersection(ground_truth).count();
    let gt_len = ground_truth.len();
    if gt_len == 0 {
        return 0.0;
    }
    intersection as f32 / gt_len as f32
}

/// 基准 1: 1M 影子集召回率 — 5000 Block + L3_1M 预算，输出召回率供人工核验
///
/// 红线断言（≥95%）由 `tests/shadow_recall_1m.rs::test_shadow_recall_1m_basic_at_least_95_percent` 守护，
/// 此 bench 仅输出召回率到 stderr 供人工核验，不 panic。
fn bench_shadow_recall_1m_recall_rate(c: &mut Criterion) {
    let blocks = build_shadow_blocks(SHADOW_BLOCK_COUNT, SHADOW_MODULE_COUNT);
    let ground_truth = compute_ground_truth(&blocks, GROUND_TRUTH_COUNT);
    let fine_output = build_fine_output(blocks.clone());
    let block_tokens = build_block_tokens(&blocks);
    let recall = RerankFill::new(RerankFillConfig {
        window_budget: WindowBudget::L3_1M,
        diversity_alpha: 0.2,
        enable_sparse_pattern: true,
    });

    // 预先执行一次，输出召回率供人工核验（不参与 benchmark 计时）
    let preview_output = recall
        .fill(RerankFillInput {
            fine_output: &fine_output,
            block_tokens: &block_tokens,
        })
        .expect("fill should succeed");
    let preview_rate = compute_recall_rate(&preview_output.filled_blocks, &ground_truth);
    eprintln!(
        "[shadow_recall_1m] filled_blocks={}, total_tokens={}, budget_utilization={:.4}, recall_rate={:.4}, threshold={:.2}, pass={}",
        preview_output.filled_blocks.len(),
        preview_output.total_tokens,
        preview_output.budget_utilization,
        preview_rate,
        RECALL_RATE_THRESHOLD,
        preview_rate >= RECALL_RATE_THRESHOLD
    );

    let mut group = c.benchmark_group("shadow_recall_1m_recall_rate");
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

/// 基准 2: p95 延迟测量 — 1000 次采样收集 p95 延迟，断言 < 200ms
///
/// 红线断言（< 200ms）由独立测试守护（参考 rerank_fill_p95_test.rs 模式）。
/// 此 bench 仅输出 p95 延迟到 stderr 供人工核验。
fn bench_shadow_recall_1m_p95_latency(c: &mut Criterion) {
    let blocks = build_shadow_blocks(SHADOW_BLOCK_COUNT, SHADOW_MODULE_COUNT);
    let fine_output = build_fine_output(blocks.clone());
    let block_tokens = build_block_tokens(&blocks);
    let recall = RerankFill::new(RerankFillConfig {
        window_budget: WindowBudget::L3_1M,
        diversity_alpha: 0.2,
        enable_sparse_pattern: true,
    });

    let mut group = c.benchmark_group("shadow_recall_1m_p95_latency");
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

            latencies.sort_unstable();
            let n = latencies.len();
            let p50 = latencies[((n as f64 * 0.50) as usize).min(n.saturating_sub(1))];
            let p95 = latencies[((n as f64 * 0.95) as usize).min(n.saturating_sub(1))];
            let p99 = latencies[((n as f64 * 0.99) as usize).min(n.saturating_sub(1))];
            let mean = latencies.iter().sum::<Duration>() / n.max(1) as u32;

            eprintln!(
                "[shadow_recall_1m_p95] samples={n}, mean={mean:?}, p50={p50:?}, p95={p95:?}, p99={p99:?}, \
                 p95<200ms={}",
                p95 < Duration::from_millis(200)
            );

            total
        })
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_shadow_recall_1m_recall_rate,
    bench_shadow_recall_1m_p95_latency,
);
criterion_main!(benches);
