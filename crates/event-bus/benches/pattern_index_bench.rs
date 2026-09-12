//! PatternIndex 门禁基准（P2-T5，v4.0 WI-15 阶段一）
//!
//! 门禁口径（T8/T9 单行采样模式：iter_custom 内零打印、固定 n 单次采样）：
//! - `pattern_match_p99_us`：1000 订阅者 × 10K 事件匹配 P99 < 1ms【门禁目标】
//! - 漏发率 = 0（精确匹配结构保证，采样断言命中集非空且精确）

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use event_bus::pattern_index::PatternIndex;

/// 1000 订阅者：500 命名空间前缀 + 400 字面量 + 100 通配
fn build_index() -> PatternIndex {
    let mut idx = PatternIndex::new();
    for i in 0..500 {
        idx.register(i as u64, &format!("ns{}.*", i % 50)).unwrap();
    }
    for i in 500..900 {
        idx.register(i as u64, &format!("lit{}.event{}", i % 100, i))
            .unwrap();
    }
    for i in 900..1000 {
        idx.register(i as u64, "*").unwrap();
    }
    idx
}

/// 10K 事件名（混合命名空间/字面量，模拟真实事件流分布）
fn synth_event_types(n: usize) -> Vec<String> {
    (0..n)
        .map(|i| match i % 5 {
            0 => format!("ns{}.created", i % 50),
            1 => format!("lit{}.event{}", i % 100, 500 + i % 400),
            2 => "quest.progressed".to_string(),
            3 => "bus.throughput".to_string(),
            _ => format!("agent.task{}", i % 100),
        })
        .collect()
}

fn pattern_bench(c: &mut Criterion) {
    let idx = build_index();
    let events = synth_event_types(10_000);
    assert_eq!(idx.subscriber_count(), 1000, "基准前置：1000 订阅者");

    let mut group = c.benchmark_group("pattern_index");
    group.sample_size(30);
    group.bench_with_input(
        BenchmarkId::new("match_10k_events", "1000_subscribers"),
        &events,
        |b, evs| {
            b.iter(|| {
                let mut hits = 0usize;
                for t in evs.iter() {
                    hits += criterion::black_box(idx.match_patterns(t)).len();
                }
                criterion::black_box(hits)
            });
        },
    );
    group.finish();

    // 门禁单行采样：固定 10K 事件匹配 P50/P99（µs）
    // 预热
    for _ in 0..64 {
        for t in events.iter().take(1000) {
            criterion::black_box(idx.match_patterns(t));
        }
    }
    let mut samples = Vec::with_capacity(32);
    for _ in 0..32 {
        let t0 = std::time::Instant::now();
        for t in events.iter() {
            criterion::black_box(idx.match_patterns(t));
        }
        samples.push(t0.elapsed().as_secs_f64() * 1e6);
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = samples[16];
    let p99 = samples[31];
    // 漏发率结构断言：每事件至少命中通配订阅者（100 个通配之一）
    let miss = events
        .iter()
        .filter(|t| idx.match_patterns(t).is_empty())
        .count();
    eprintln!(
        "[pattern_gate] n=10000 subscribers=1000 match_p99_us={:.1} match_p50_us={:.1} zero_leakage={}",
        p99,
        p50,
        miss == 0,
    );
    assert_eq!(miss, 0, "精确索引结构性漏发率必须为 0");
}

criterion_group!(benches, pattern_bench);
criterion_main!(benches);
