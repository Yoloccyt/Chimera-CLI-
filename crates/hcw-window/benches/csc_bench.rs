//! CSC 四级压缩链门禁基准（P2-T4，手册 W11）
//!
//! 门禁口径（T8/T9 单行采样模式：iter_custom 内零打印、固定 n 单次采样）：
//! - `csc_p99_ms`：压缩尾延迟 P99 < 300ms【门禁目标】
//! - `token_reduction_pct`：100 轮合成会话压缩后 token 降 ≥ 40%【门禁目标】
//! - `thinking_complete_pct`：thinking 链完整率恒 100%（T-02）
//!
//! 数据诚实：本基准只输出实测数字，阈值判定由 CI/人工按门禁口径执行。

use std::sync::Arc;
use std::time::Instant;

use chrono::{DateTime, TimeZone, Utc};
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use hcw_window::pipeline::CompressionPipeline;
use hcw_window::preserve::{ConversationContext, ThinkingBlock};
use hcw_window::types::ContextEntry;
use hcw_window::HcwConfig;

/// 固定时钟（Ω₇：确定性注入，禁真实 SystemTime 依赖）
fn fixed_now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 24, 0, 0, 0).unwrap()
}

/// 合成 100 轮会话：每轮 10 条目 × ~40 token + 1 个 thinking 块
fn synth_context(rounds: usize) -> ConversationContext {
    let mut body = Vec::new();
    for r in 0..rounds {
        for i in 0..10 {
            let content = format!(
                "round-{r}-item-{i}: 会话上下文内容片段，用于压缩链路基准测量，包含若干可去重与可聚类的信息",
            );
            // ~40 token 估算：中文 3 字 ≈ 1 token
            let tokens = content.chars().count() / 3 + 1;
            body.push(Arc::new(ContextEntry::new(
                format!("e-{r}-{i}"),
                format!("file-{}", i % 3),
                content,
                tokens,
            )));
        }
    }
    let thinking = (0..8)
        .map(|i| ThinkingBlock::new(i as u64, format!("thinking-trace-{i}: 推理痕迹保留验证")))
        .collect();
    ConversationContext::new("static-prefix-unchanged", body, thinking)
}

/// 固定 n 单次采样：压缩 P50/P99（µs）与 token 降幅与 thinking 完整率
fn probe_gate_metrics(pipeline: &CompressionPipeline, ctx: &ConversationContext, n: usize) {
    // 预热（64 次，规避首轮分配）
    for _ in 0..64 {
        criterion::black_box(pipeline.compress(criterion::black_box(ctx), 2000, None, fixed_now()));
    }
    let mut samples = Vec::with_capacity(n);
    for _ in 0..n {
        let t0 = Instant::now();
        let out = pipeline.compress(ctx, 2000, None, fixed_now());
        samples.push(t0.elapsed().as_secs_f64() * 1e6);
        // thinking 完整率：输出与输入逐字节一致
        assert_eq!(
            out.context.thinking.len(),
            ctx.thinking.len(),
            "thinking 块数必须守恒"
        );
        for (o, i) in out.context.thinking.iter().zip(ctx.thinking.iter()) {
            assert_eq!(
                o.content.as_bytes(),
                i.content.as_bytes(),
                "thinking 逐字节一致"
            );
        }
        // from 模式：前缀逐字节不变
        assert_eq!(
            out.context.prefix.as_bytes(),
            ctx.prefix.as_bytes(),
            "前缀必须逐字节不变(缓存前缀不失效)"
        );
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = samples[n / 2];
    let p99 = samples[(n as f64 * 0.99) as usize];
    let report = pipeline.compress(ctx, 2000, None, fixed_now()).report;
    eprintln!(
        "[csc_gate] n={n} csc_p99_ms={:.3} csc_p50_ms={:.3} token_reduction_pct={:.2} level={:?} thinking_complete_pct=100.0",
        p99 / 1000.0,
        p50 / 1000.0,
        report.token_reduction_pct(),
        report.level,
    );
}

fn csc_bench(c: &mut Criterion) {
    let pipeline = CompressionPipeline::new(HcwConfig::default());
    let ctx100 = synth_context(100);

    let mut group = c.benchmark_group("csc");
    group.sample_size(30);
    // 迭代基准：100 轮会话压缩耗时（criterion 均值统计）
    group.bench_with_input(
        BenchmarkId::new("compress_100_rounds", "default"),
        &ctx100,
        |b, ctx| {
            b.iter(|| {
                criterion::black_box(pipeline.compress(
                    criterion::black_box(ctx),
                    2000,
                    None,
                    fixed_now(),
                ))
            });
        },
    );
    group.finish();

    // 门禁单行采样（iter_custom 外，固定 n=200）
    probe_gate_metrics(&pipeline, &ctx100, 200);
}

criterion_group!(benches, csc_bench);
criterion_main!(benches);
