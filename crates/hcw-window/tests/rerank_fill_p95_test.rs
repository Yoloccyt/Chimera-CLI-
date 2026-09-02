//! HCW-Sparse v2.0 重排填充 p95 延迟红线守护测试
//!
//! 对应任务: P3-W10.1.2（spec.md P3 内环升级）
//! 验证红线: **重排填充 500 Block p95 < 100ms**（spec.md KPI 表格）
//!
//! # 运行方式
//! ```bash
//! # release 模式运行（debug 模式可能误判红线）
//! cargo test -p hcw-window --release --test rerank_fill_p95_test -- --ignored --nocapture
//! ```
//!
//! # 设计说明
//! - 1000 次采样收集延迟分布，取 p95 百分位断言 <100ms
//! - 用 `#[ignore]` 标记：仅在 release 模式 + 显式 `--ignored` 时运行，
//!   避免 debug 模式下因优化不足导致的误判红线（CI 失败）
//! - 与 `benches/rerank_fill.rs::bench_rerank_fill_p95_latency` 互补：
//!   bench 输出延迟分布供人工核验，test 做硬性红线断言

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::time::{Duration, Instant};

use hcw_window::recall::{
    BlockScore, FineRecallOutput, RerankFill, RerankFillConfig, RerankFillInput, WindowBudget,
};

/// Block 数量（spec.md 要求 500）
const BLOCK_COUNT: usize = 500;

/// 采样次数（1000 次保证 p95 统计显著性）
const SAMPLE_COUNT: usize = 1000;

/// p95 延迟阈值（spec.md 红线：< 100ms）
const P95_THRESHOLD_MS: u64 = 100;

/// 断言用阈值 = 契约常量 × CI 缩放旋钮（`CHIMERA_PERF_SCALE`，缺省 1.0）
///
/// 失败消息里打印的 `{threshold:?}` 因此是**生效阈值**而非契约常量，
/// 避免“scale=4 时报告里写着 100ms 却实际按 400ms 判”这种误导。
fn p95_threshold() -> Duration {
    Duration::from_millis(nexus_contracts::util::perf_scale_ms(P95_THRESHOLD_MS))
}

/// 构造测试用 BlockScore 列表（确定性生成）
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

/// 计算延迟百分位(委托共享工具)
///
// 口径变更:原 `trunc(n*p)` 统一为 `round((n-1)*p)`,两者索引差 ≤1 个样本,
// 对 p95 红线测得值无实质影响(已由本文件红线断言回归验证)。
use nexus_contracts::util::percentile_sorted;
fn percentile(sorted: &[Duration], p: f64) -> Duration {
    percentile_sorted(sorted, p).unwrap_or(Duration::ZERO)
}

/// 重排填充 p95 延迟红线守护（P3-W10.1.2 spec.md 红线）
///
/// # 红线
/// 500 Block × L2_256K 预算 × 1000 次采样，p95 < 100ms
///
/// # 算法复杂度
/// - 密度计算 + 排序: O(N log N) ≈ 0.1ms（500 Block）
/// - 贪心填充: O(N) ≈ 0.01ms
/// - 二次稀疏构建: O(N) ≈ 0.5ms
/// - 总计: < 1ms，预算 <100ms 极充足
#[test]
#[ignore = "性能红线测试:需 release 模式运行,debug 模式可能误判红线"]
fn test_rerank_fill_p95_below_100ms() {
    // 1. 构建重排填充引擎与输入数据
    let blocks = build_blocks(BLOCK_COUNT, 20);
    let fine_output = FineRecallOutput {
        blocks,
        elapsed_us: 0,
        candidate_count: 0,
    };
    let block_tokens = build_block_tokens(&fine_output.blocks);
    let recall = RerankFill::new(RerankFillConfig {
        window_budget: WindowBudget::L2_256K,
        diversity_alpha: 0.2,
        enable_sparse_pattern: true,
    });

    // 2. warmup（预热缓存，避免首次运行的冷启动偏差）
    for _ in 0..10 {
        let _ = recall
            .fill(RerankFillInput {
                fine_output: &fine_output,
                block_tokens: &block_tokens,
            })
            .expect("warmup fill should succeed");
    }

    // 3. 收集 1000 次 fill() 延迟样本
    let mut latencies: Vec<Duration> = Vec::with_capacity(SAMPLE_COUNT);
    for _ in 0..SAMPLE_COUNT {
        let start = Instant::now();
        let output = recall
            .fill(RerankFillInput {
                fine_output: &fine_output,
                block_tokens: &block_tokens,
            })
            .expect("fill should succeed");
        let elapsed = start.elapsed();
        latencies.push(elapsed);

        // 验证填充结果正确性（不应因性能优化牺牲正确性）
        assert!(
            !output.filled_blocks.is_empty(),
            "fill should return blocks"
        );
        assert!(output.total_tokens > 0, "total_tokens should be > 0");
    }

    // 4. 排序并取百分位
    latencies.sort_unstable();
    let p50 = percentile(&latencies, 0.50);
    let p95 = percentile(&latencies, 0.95);
    let p99 = percentile(&latencies, 0.99);
    let mean = latencies.iter().sum::<Duration>() / latencies.len() as u32;
    let max = latencies[latencies.len() - 1];

    // 5. 输出延迟分布（供人工核验）
    eprintln!(
        "[rerank_fill_p95_test] blocks={BLOCK_COUNT}, samples={SAMPLE_COUNT}, \
         mean={mean:?}, p50={p50:?}, p95={p95:?}, p99={p99:?}, max={max:?}, \
         p95<{P95_THRESHOLD_MS}ms={}",
        p95 < p95_threshold()
    );

    // 6. 红线断言: p95 < 100ms
    assert!(
        p95 < p95_threshold(),
        "P3-W10.1.2 红线违规:重排填充 {BLOCK_COUNT} Block p95={p95:?} ≥ {threshold:?} \
         (密度贪心 O(N log N) + 二次稀疏 O(N)，理论 <1ms)",
        threshold = p95_threshold()
    );
}

/// 1M 等效窗口 p95 延迟测试（架构红线验证）
///
/// # 红线
/// 5000 Block × L3_1M 预算 × 100 次采样，p95 < 100ms
///
/// # 验证点
/// - 1M 等效窗口（128K 实际 × 8x 压缩）重排填充延迟 <100ms
/// - 大规模 Block（5000）下密度贪心性能
#[test]
#[ignore = "性能红线测试:需 release 模式运行,验证 1M 等效窗口性能"]
fn test_rerank_fill_1m_window_p95_below_100ms() {
    const LARGE_BLOCK_COUNT: usize = 5000;
    const LARGE_SAMPLE_COUNT: usize = 100;

    let blocks = build_blocks(LARGE_BLOCK_COUNT, 50);
    let fine_output = FineRecallOutput {
        blocks,
        elapsed_us: 0,
        candidate_count: 0,
    };
    let block_tokens = build_block_tokens(&fine_output.blocks);
    let recall = RerankFill::new(RerankFillConfig {
        window_budget: WindowBudget::L3_1M, // 1M 等效（128K 实际 × 8x 压缩）
        diversity_alpha: 0.2,
        enable_sparse_pattern: true,
    });

    // warmup
    for _ in 0..5 {
        let _ = recall
            .fill(RerankFillInput {
                fine_output: &fine_output,
                block_tokens: &block_tokens,
            })
            .expect("warmup fill should succeed");
    }

    let mut latencies: Vec<Duration> = Vec::with_capacity(LARGE_SAMPLE_COUNT);
    for _ in 0..LARGE_SAMPLE_COUNT {
        let start = Instant::now();
        let output = recall
            .fill(RerankFillInput {
                fine_output: &fine_output,
                block_tokens: &block_tokens,
            })
            .expect("fill should succeed");
        latencies.push(start.elapsed());

        // 验证 1M 等效窗口只加载 128K（架构红线：禁止 1M 暴力加载）
        assert!(
            output.total_tokens <= 128 * 1024,
            "1M 等效应只加载 128K 实际 tokens,实际 {}",
            output.total_tokens
        );
    }

    latencies.sort_unstable();
    let p95 = percentile(&latencies, 0.95);
    let mean = latencies.iter().sum::<Duration>() / latencies.len() as u32;

    eprintln!(
        "[rerank_fill_1m_p95] blocks={LARGE_BLOCK_COUNT}, samples={LARGE_SAMPLE_COUNT}, \
         mean={mean:?}, p95={p95:?}, p95<100ms={}",
        p95 < p95_threshold()
    );

    assert!(
        p95 < p95_threshold(),
        "P3-W10.1.2 1M 等效窗口红线违规:重排填充 {LARGE_BLOCK_COUNT} Block p95={p95:?} ≥ {threshold:?}",
        threshold = p95_threshold()
    );
}
