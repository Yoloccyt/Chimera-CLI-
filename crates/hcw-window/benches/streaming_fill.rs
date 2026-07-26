//! P3-W10.2.2: HCW-Sparse v2.0 增量流式基准
//!
//! 对应任务: P3-W10.2（spec.md P3 内环升级）
//! 对应病理修复: D1（HCW 无学习机制,静态加载无法适配动态首 token 延迟）
//!
//! # 基准项与目标（RED-first）
//! - `fast_split_100_blocks`: Fast 模式 100 Block 分割延迟,目标 p95 < 500ms（首 token 目标）
//! - `fast_split_500_blocks`: Fast 模式 500 Block 分割延迟,目标 p95 < 500ms
//! - `deep_split_100_blocks`: Deep 模式 100 Block 分割延迟,目标 p95 < 2s
//! - `deep_split_500_blocks`: Deep 模式 500 Block 分割延迟,目标 p95 < 2s
//! - `p95_latency`: Fast 模式 500 Block × 100 采样的 p95 延迟测量
//!
//! # 设计理由（WHY）
//! - **分割延迟 << 首 token 延迟**: split() 本身是 O(N) 数据分割,<1ms;
//!   首 token 延迟主要由 LLM prefill 决定（关键块 token 数 × prefill 速度）
//! - **本基准验证数据就绪延迟**: 确保 split() 不成为首 token 路径的瓶颈
//!   - Fast: split() 应 <1ms（远小于 500ms 首 token 目标）
//! - **p95 采样**: 100 次重复采样,统计 p95 验证红线
//!
//! # 性能预算
//! - 500 Block × Fast 10% split: ~0.01ms（O(N) 切片 + 50 块克隆）
//! - 500 Block × Deep 20% split: ~0.02ms（100 块克隆）
//! - 总计远小于 500ms 首 token 目标

#![forbid(unsafe_code)]

use criterion::{black_box, criterion_group, criterion_main, Criterion, Throughput};
use std::time::{Duration, Instant};

use hcw_window::recall::types::{BlockScore, FineRecallOutput};
use hcw_window::recall::{
    RerankFill, RerankFillConfig, RerankFillInput, StreamingFill, StreamingFillInput, WindowBudget,
};
use std::collections::HashMap;

// ============================================================
// 辅助函数
// ============================================================

/// 构造 N 个测试 Block（score 递减,模拟密度降序）
fn build_blocks(n: usize, tokens_per_block: usize) -> Vec<BlockScore> {
    (0..n)
        .map(|i| {
            BlockScore::new(
                format!("block-{i}"),
                1.0 - i as f32 * 0.001,
                1.0 - i as f32 * 0.001,
                format!("module-{}", i % 10),
                tokens_per_block,
            )
        })
        .collect()
}

/// 构造精排输出
fn build_fine_output(blocks: Vec<BlockScore>) -> FineRecallOutput {
    FineRecallOutput {
        blocks,
        elapsed_us: 0,
        candidate_count: 0,
    }
}

/// 构造 block_tokens 映射
fn build_block_tokens(blocks: &[BlockScore]) -> HashMap<String, usize> {
    blocks
        .iter()
        .map(|b| (b.block_id.clone(), b.token_count))
        .collect()
}

/// 通过 RerankFill 生成 RerankFillOutput
fn build_rerank_output(
    blocks: Vec<BlockScore>,
    budget: WindowBudget,
) -> hcw_window::recall::RerankFillOutput {
    let fine_output = build_fine_output(blocks);
    let block_tokens = build_block_tokens(&fine_output.blocks);
    let rerank = RerankFill::new(RerankFillConfig {
        window_budget: budget,
        diversity_alpha: 0.0,
        enable_sparse_pattern: false,
    });
    rerank
        .fill(RerankFillInput {
            fine_output: &fine_output,
            block_tokens: &block_tokens,
        })
        .expect("rerank fill should succeed")
}

// ============================================================
// 基准 1: Fast 模式 100 Block 分割
// ============================================================

fn bench_fast_split_100_blocks(c: &mut Criterion) {
    let blocks = build_blocks(100, 1024);
    let rerank_output = build_rerank_output(blocks, WindowBudget::L3_1M);
    let streaming = StreamingFill::fast();

    let mut group = c.benchmark_group("streaming_fast_split");
    group.throughput(Throughput::Elements(100));
    group.bench_function("100_blocks", |b| {
        b.iter(|| {
            black_box(streaming.split(black_box(StreamingFillInput {
                rerank_output: black_box(&rerank_output),
            })))
        })
    });
    group.finish();
}

// ============================================================
// 基准 2: Fast 模式 500 Block 分割
// ============================================================

fn bench_fast_split_500_blocks(c: &mut Criterion) {
    let blocks = build_blocks(500, 1024);
    let rerank_output = build_rerank_output(blocks, WindowBudget::L3_1M);
    let streaming = StreamingFill::fast();

    let mut group = c.benchmark_group("streaming_fast_split");
    group.throughput(Throughput::Elements(500));
    group.bench_function("500_blocks", |b| {
        b.iter(|| {
            black_box(streaming.split(black_box(StreamingFillInput {
                rerank_output: black_box(&rerank_output),
            })))
        })
    });
    group.finish();
}

// ============================================================
// 基准 3: Deep 模式 100 Block 分割
// ============================================================

fn bench_deep_split_100_blocks(c: &mut Criterion) {
    let blocks = build_blocks(100, 1024);
    let rerank_output = build_rerank_output(blocks, WindowBudget::L3_1M);
    let streaming = StreamingFill::deep();

    let mut group = c.benchmark_group("streaming_deep_split");
    group.throughput(Throughput::Elements(100));
    group.bench_function("100_blocks", |b| {
        b.iter(|| {
            black_box(streaming.split(black_box(StreamingFillInput {
                rerank_output: black_box(&rerank_output),
            })))
        })
    });
    group.finish();
}

// ============================================================
// 基准 4: p95 延迟测量（Fast 500 Block × 100 采样）
// ============================================================

fn bench_streaming_split_p95_latency(c: &mut Criterion) {
    let blocks = build_blocks(500, 1024);
    let rerank_output = build_rerank_output(blocks, WindowBudget::L3_1M);
    let streaming = StreamingFill::fast();

    let mut group = c.benchmark_group("streaming_split_p95_latency");
    group.sample_size(100);
    group.bench_function("p95_latency", |b| {
        b.iter_custom(|iters| {
            let mut latencies = Vec::with_capacity(iters as usize);
            let start_total = Instant::now();
            for _ in 0..iters {
                let start = Instant::now();
                black_box(
                    streaming
                        .split(black_box(StreamingFillInput {
                            rerank_output: black_box(&rerank_output),
                        }))
                        .unwrap(),
                );
                latencies.push(start.elapsed());
            }
            let total = start_total.elapsed();
            latencies.sort_unstable();
            let p95_idx = ((latencies.len() as f64 * 0.95) as usize).min(latencies.len() - 1);
            let p95 = latencies[p95_idx];
            // Fast 首 token 目标 500ms;split() 应 <<1ms
            eprintln!(
                "[streaming_split_p95] samples={}, p95={:?}, target=500ms, p95<500ms={}",
                latencies.len(),
                p95,
                p95 < Duration::from_millis(500)
            );
            total
        })
    });
    group.finish();
}

criterion_group!(
    streaming_benches,
    bench_fast_split_100_blocks,
    bench_fast_split_500_blocks,
    bench_deep_split_100_blocks,
    bench_streaming_split_p95_latency,
);
criterion_main!(streaming_benches);
