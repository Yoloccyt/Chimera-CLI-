//! P3-W10.2.3: HCW-Sparse v2.0 增量流式红线验证
//!
//! 对应任务: P3-W10.2（spec.md P3 内环升级）
//! 验证项: 首 token p95 < 500ms（Fast）/ < 2s（Deep）红线
//!
//! # 红线验证逻辑
//! - **split() 数据就绪延迟**: 验证同步分割阶段 p95 远小于首 token 目标
//!   - Fast: split() p95 < 500ms（实际 <1ms,留 500x 余量给 LLM prefill）
//!   - Deep: split() p95 < 2000ms（实际 <1ms,留 2000x 余量给 LLM prefill）
//! - **关键块比例正确性**: Fast=10%, Deep=20%
//! - **不变量守恒**: critical + background = total（块数与 token 数）
//!
//! # WHY 用 #[ignore]
//! 性能红线测试需 release 模式运行（debug 模式开销不稳定）:
//! ```
//! cargo test --release -p hcw-window --test streaming_fill_p95_test -- --ignored --nocapture
//! ```

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::time::{Duration, Instant};

use hcw_window::recall::types::{BlockScore, FineRecallOutput};
use hcw_window::recall::{
    RerankFill, RerankFillConfig, RerankFillInput, StreamingFill, StreamingFillInput,
    StreamingFillOutput, WindowBudget,
};

// ============================================================
// 辅助函数
// ============================================================

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

fn build_fine_output(blocks: Vec<BlockScore>) -> FineRecallOutput {
    FineRecallOutput {
        blocks,
        elapsed_us: 0,
        candidate_count: 0,
    }
}

fn build_block_tokens(blocks: &[BlockScore]) -> HashMap<String, usize> {
    blocks
        .iter()
        .map(|b| (b.block_id.clone(), b.token_count))
        .collect()
}

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

/// 计算 p95 延迟
fn percentile(latencies: &[Duration], p: f64) -> Duration {
    let mut sorted = latencies.to_vec();
    sorted.sort_unstable();
    let idx = ((sorted.len() as f64 - 1.0) * p).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

// ============================================================
// 红线测试 1: Fast 模式 split() p95 < 500ms
// ============================================================

#[test]
#[ignore = "性能红线测试:需 release 模式运行"]
fn test_streaming_fast_split_p95_below_500ms() {
    const BLOCK_COUNT: usize = 500;
    const SAMPLE_COUNT: usize = 1000;
    const P95_THRESHOLD_MS: u64 = 500;

    let blocks = build_blocks(BLOCK_COUNT, 1024);
    let rerank_output = build_rerank_output(blocks, WindowBudget::L3_1M);
    let streaming = StreamingFill::fast();

    // Warmup + 采样
    let mut latencies = Vec::with_capacity(SAMPLE_COUNT);
    for _ in 0..SAMPLE_COUNT {
        let start = Instant::now();
        streaming
            .split(StreamingFillInput {
                rerank_output: &rerank_output,
            })
            .expect("split should succeed");
        latencies.push(start.elapsed());
    }

    let p95 = percentile(&latencies, 0.95);
    println!(
        "[Fast] samples={}, p95={:?}, target=500ms, ratio={}",
        latencies.len(),
        p95,
        streaming.config().effective_ratio()
    );
    assert!(
        p95 < Duration::from_millis(P95_THRESHOLD_MS),
        "P3-W10.2.3 红线违规: Fast 模式 split() {} Block p95={:?} ≥ {:?}",
        BLOCK_COUNT,
        p95,
        Duration::from_millis(P95_THRESHOLD_MS)
    );
}

// ============================================================
// 红线测试 2: Deep 模式 split() p95 < 2000ms
// ============================================================

#[test]
#[ignore = "性能红线测试:需 release 模式运行"]
fn test_streaming_deep_split_p95_below_2000ms() {
    const BLOCK_COUNT: usize = 500;
    const SAMPLE_COUNT: usize = 1000;
    const P95_THRESHOLD_MS: u64 = 2000;

    let blocks = build_blocks(BLOCK_COUNT, 1024);
    let rerank_output = build_rerank_output(blocks, WindowBudget::L3_1M);
    let streaming = StreamingFill::deep();

    let mut latencies = Vec::with_capacity(SAMPLE_COUNT);
    for _ in 0..SAMPLE_COUNT {
        let start = Instant::now();
        streaming
            .split(StreamingFillInput {
                rerank_output: &rerank_output,
            })
            .expect("split should succeed");
        latencies.push(start.elapsed());
    }

    let p95 = percentile(&latencies, 0.95);
    println!(
        "[Deep] samples={}, p95={:?}, target=2000ms, ratio={}",
        latencies.len(),
        p95,
        streaming.config().effective_ratio()
    );
    assert!(
        p95 < Duration::from_millis(P95_THRESHOLD_MS),
        "P3-W10.2.3 红线违规: Deep 模式 split() {} Block p95={:?} ≥ {:?}",
        BLOCK_COUNT,
        p95,
        Duration::from_millis(P95_THRESHOLD_MS)
    );
}

// ============================================================
// 红线测试 3: Fast 关键块比例 ≈ 10%
// ============================================================

#[test]
#[ignore = "性能红线测试:需 release 模式运行"]
fn test_streaming_fast_critical_ratio_10_percent() {
    const BLOCK_COUNT: usize = 500;
    const EXPECTED_RATIO: f32 = 0.1;
    const TOLERANCE: f32 = 0.02; // ±2% 容差（ceil 导致的舍入）

    let blocks = build_blocks(BLOCK_COUNT, 1024);
    let rerank_output = build_rerank_output(blocks, WindowBudget::L3_1M);
    let streaming = StreamingFill::fast();

    let output = streaming
        .split(StreamingFillInput {
            rerank_output: &rerank_output,
        })
        .expect("split should succeed");

    let actual_ratio = output.critical_block_ratio();
    println!(
        "[Fast ratio] critical={}, background={}, ratio={:.4}, expected≈{:.2}",
        output.critical_blocks.len(),
        output.background_blocks.len(),
        actual_ratio,
        EXPECTED_RATIO
    );
    assert!(
        (actual_ratio - EXPECTED_RATIO).abs() < TOLERANCE,
        "P3-W10.2.3 红线违规: Fast 关键块比例 = {:.4},期望 ≈ {:.2} ± {:.2}",
        actual_ratio,
        EXPECTED_RATIO,
        TOLERANCE
    );
}

// ============================================================
// 红线测试 4: Deep 关键块比例 ≈ 20%
// ============================================================

#[test]
#[ignore = "性能红线测试:需 release 模式运行"]
fn test_streaming_deep_critical_ratio_20_percent() {
    const BLOCK_COUNT: usize = 500;
    const EXPECTED_RATIO: f32 = 0.2;
    const TOLERANCE: f32 = 0.02;

    let blocks = build_blocks(BLOCK_COUNT, 1024);
    let rerank_output = build_rerank_output(blocks, WindowBudget::L3_1M);
    let streaming = StreamingFill::deep();

    let output = streaming
        .split(StreamingFillInput {
            rerank_output: &rerank_output,
        })
        .expect("split should succeed");

    let actual_ratio = output.critical_block_ratio();
    println!(
        "[Deep ratio] critical={}, background={}, ratio={:.4}, expected≈{:.2}",
        output.critical_blocks.len(),
        output.background_blocks.len(),
        actual_ratio,
        EXPECTED_RATIO
    );
    assert!(
        (actual_ratio - EXPECTED_RATIO).abs() < TOLERANCE,
        "P3-W10.2.3 红线违规: Deep 关键块比例 = {:.4},期望 ≈ {:.2} ± {:.2}",
        actual_ratio,
        EXPECTED_RATIO,
        TOLERANCE
    );
}

// ============================================================
// 红线测试 5: 不变量守恒（块数 + token 数）
// ============================================================

#[test]
#[ignore = "性能红线测试:需 release 模式运行"]
fn test_streaming_invariants_preserved() {
    const BLOCK_COUNT: usize = 500;

    let blocks = build_blocks(BLOCK_COUNT, 1024);
    let rerank_output = build_rerank_output(blocks, WindowBudget::L3_1M);

    // Fast 模式不变量
    let fast_output = StreamingFill::fast()
        .split(StreamingFillInput {
            rerank_output: &rerank_output,
        })
        .expect("fast split should succeed");
    assert_invariants(&fast_output, &rerank_output, "Fast");

    // Deep 模式不变量
    let deep_output = StreamingFill::deep()
        .split(StreamingFillInput {
            rerank_output: &rerank_output,
        })
        .expect("deep split should succeed");
    assert_invariants(&deep_output, &rerank_output, "Deep");
}

/// 验证不变量: critical + background = total
fn assert_invariants(
    output: &StreamingFillOutput,
    rerank_output: &hcw_window::recall::RerankFillOutput,
    mode: &str,
) {
    // 块数守恒
    assert_eq!(
        output.critical_blocks.len() + output.background_blocks.len(),
        rerank_output.filled_blocks.len(),
        "{} INV 违规: 关键块 + 后台块 ≠ 总块数",
        mode
    );
    // token 数守恒
    assert_eq!(
        output.critical_token_count + output.background_token_count,
        rerank_output.total_tokens,
        "{} INV 违规: 关键 token + 后台 token ≠ 总 token",
        mode
    );
    // 首 token 就绪（非空输入场景）
    assert!(
        output.first_token_ready,
        "{} INV 违规: 首 token 未就绪（关键块未分割）",
        mode
    );
}

// ============================================================
// 红线测试 6: 大规模 Block（5000 块）split 延迟
// ============================================================

#[test]
#[ignore = "性能红线测试:需 release 模式运行"]
fn test_streaming_split_5000_blocks_p95_below_500ms() {
    const BLOCK_COUNT: usize = 5000;
    const SAMPLE_COUNT: usize = 100;
    const P95_THRESHOLD_MS: u64 = 500;

    let blocks = build_blocks(BLOCK_COUNT, 1024);
    // 用 L3_1M 预算（128K / 1024 = 128 块）会限制填充,但 split 仍处理 128 块
    let rerank_output = build_rerank_output(blocks, WindowBudget::L3_1M);
    let streaming = StreamingFill::fast();

    let mut latencies = Vec::with_capacity(SAMPLE_COUNT);
    for _ in 0..SAMPLE_COUNT {
        let start = Instant::now();
        streaming
            .split(StreamingFillInput {
                rerank_output: &rerank_output,
            })
            .expect("split should succeed");
        latencies.push(start.elapsed());
    }

    let p95 = percentile(&latencies, 0.95);
    println!(
        "[5000 blocks → 128 filled] samples={}, p95={:?}, target=500ms",
        latencies.len(),
        p95
    );
    assert!(
        p95 < Duration::from_millis(P95_THRESHOLD_MS),
        "P3-W10.2.3 红线违规: 5000 Block split p95={:?} ≥ {:?}",
        p95,
        Duration::from_millis(P95_THRESHOLD_MS)
    );
}
