//! 稀疏化压缩收益对照基准（P2-T5，手册 W12）
//!
//! 门禁口径（T8/T9 单行采样模式）：`sparse_gain_pct` ≥ +20%——
//! 稀疏化打分截断（snip：去重 + importance-top-n）相对基线（仅去重不截断）
//! 的 token 降幅增量。数据诚实：只输出实测数字，判定由 CI/人工执行。

use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use criterion::{criterion_group, criterion_main, Criterion};
use hcw_window::pipeline::CompressionPipeline;
use hcw_window::types::ContextEntry;
use hcw_window::HcwConfig;

/// 固定时钟（Ω₇）
fn fixed_now() -> DateTime<Utc> {
    Utc.with_ymd_and_hms(2026, 8, 25, 0, 0, 0).unwrap()
}

/// 合成高冗余会话（重复内容多 → 去重收益大；评分截断需在去重后仍超预算才显效）
fn synth_entries(n: usize, distinct: usize) -> Vec<Arc<ContextEntry>> {
    (0..n)
        .map(|i| {
            let content = format!(
                "item-{}-{}：会话上下文内容片段，用于稀疏化对照基准测量",
                i % distinct,
                i,
            );
            let tokens = content.chars().count() / 3 + 1;
            Arc::new(ContextEntry::new(
                format!("e-{i}"),
                format!("f-{}", i % 5),
                content,
                tokens,
            ))
        })
        .collect()
}

/// 门禁单行采样：稀疏（snip）vs 基线（仅去重）的 token 降幅对照
fn probe_gate(
    pipeline: &CompressionPipeline,
    entries: &[Arc<ContextEntry>],
    budget: usize,
    n: usize,
) {
    // 基线路径：仅去重（dedup_by_content 语义经 snip 的早退分支——预算充足时
    // snip 只去重不截断；用大预算触发"去重即止"）
    let base_budget = usize::MAX / 2;
    let base_report = pipeline.compress_body(entries, base_budget, None, fixed_now());
    // 稀疏路径：小预算触发评分截断（importance-top-n）
    let sparse_report = pipeline.compress_body(entries, budget, None, fixed_now());
    let baseline_pct = base_report.token_reduction_pct();
    let sparse_pct = sparse_report.token_reduction_pct();
    let gain = sparse_pct - baseline_pct;
    eprintln!(
        "[sparse_gate] n={n} entries={} budget={budget} baseline_pct={baseline_pct:.2} sparse_pct={sparse_pct:.2} sparse_gain_pct={gain:.2} level={:?}",
        entries.len(),
        sparse_report.level,
    );
}

fn sparse_compare(c: &mut Criterion) {
    let pipeline = CompressionPipeline::new(HcwConfig::default());
    let entries = synth_entries(500, 10); // 500 条 / 10 去重域

    let mut group = c.benchmark_group("sparse_compare");
    group.sample_size(30);
    group.bench_function("snip_sparse_500", |b| {
        b.iter(|| criterion::black_box(pipeline.compress_body(&entries, 800, None, fixed_now())));
    });
    group.finish();

    // 门禁单行采样（iter_custom 外，固定 n=20——压缩为确定性纯函数，无需大样本）
    probe_gate(&pipeline, &entries, 800, 20);
}

criterion_group!(benches, sparse_compare);
criterion_main!(benches);
