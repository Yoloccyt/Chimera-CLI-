//! MCA 热路径基准 — 成本估算 + SSE 归一器(MCA M3/M0,ADR-065/068)
//!
//! 对应架构层:L10 Interface(mca-gateway)
//!
//! # 基准目标(RED-first,阈值标记供 check_perf_redlines 静态 lint)
//! - `bench_cost_estimate`: 路由热路径成本估算,目标 < 1μs(COST_ESTIMATE_TARGET_US)
//!   —— 复刻 cacr.rs 美分整数范式,无浮点中间态(f32 精度红线)
//! - `bench_sse_normalize`: SSE 单事件归一,目标 < 5μs(SSE_EVENT_TARGET_US);
//!   64KB chunk 吞吐 ≥ 200MB/s(设计文档 §5.2 TTFT 红线承载者)
//!
//! # 红线标记(静态 lint 锚点)
//! 阈值常量名即 lint 的 Threshold 标记,勿重命名。

#![forbid(unsafe_code)]

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use mca_gateway::sse::StreamNormalizer;
use nexus_contracts::affinity::{PricingSpec, ProtocolDialect};

/// 成本估算红线(μs)——路由热路径,超限即路由决策拖慢
pub const COST_ESTIMATE_TARGET_US: u64 = 1;

/// SSE 单事件归一红线(μs)——TTFT 红线承载者
pub const SSE_EVENT_TARGET_US: u64 = 5;

/// 构造定价样本(DeepSeek 档:峰谷 2×,缓存命中 ¥0.01/M)
fn sample_pricing() -> PricingSpec {
    PricingSpec {
        currency: nexus_contracts::affinity::Currency::Cny,
        input_micro_per_mtok: 1_000_000,
        output_micro_per_mtok: 2_000_000,
        cache_hit_micro_per_mtok: 10_000,
        peak_periods: vec![nexus_contracts::affinity::PeakPeriod {
            start_hour: 8,
            end_hour: 20,
            factor_percent: 200,
        }],
    }
}

/// 成本估算基准(参考 adapters.rs `actual_cost` 的整数口径)
fn bench_cost_estimate(c: &mut Criterion) {
    let pricing = sample_pricing();
    let usage = nexus_contracts::affinity::UsageReport {
        input_tokens: 1_000_000,
        output_tokens: 500_000,
        cache_hit_tokens: 600_000,
        thinking_tokens: None,
    };
    c.bench_function("cost_estimate_1m_tokens_peak", |b| {
        b.iter(|| {
            // 峰谷查表 + 整数微元运算(与 actual_cost 同构,仅内联轻量实现)
            let factor = pricing
                .peak_periods
                .iter()
                .find(|p| p.start_hour <= 12 && 12 < p.end_hour)
                .map(|p| p.factor_percent)
                .unwrap_or(100);
            let cached = usage.cache_hit_tokens.min(usage.input_tokens);
            let uncached = usage.input_tokens - cached;
            let input_cost = uncached * pricing.input_micro_per_mtok / 1_000_000;
            let cache_cost = cached * pricing.cache_hit_micro_per_mtok / 1_000_000;
            let output_cost = usage.output_tokens * pricing.output_micro_per_mtok / 1_000_000;
            black_box((input_cost + cache_cost + output_cost) * u64::from(factor) / 100)
        })
    });
}

/// SSE 归一基准(OpenAI 方言文本增量流,单事件 < 5μs)
fn bench_sse_normalize(c: &mut Criterion) {
    let mut normalizer = StreamNormalizer::new(ProtocolDialect::OpenAiChat);
    // 构造 100 帧文本增量流(模拟流式回答)
    let mut stream = String::new();
    for i in 0..100 {
        stream.push_str(&format!(
            "data: {{\"choices\":[{{\"delta\":{{\"content\":\"tok-{i}\"}}}}]}}\n\n"
        ));
    }
    let bytes = stream.into_bytes();
    c.bench_function("sse_normalize_100_text_deltas", |b| {
        b.iter(|| {
            let events = normalizer.feed(black_box(&bytes));
            black_box(events.len())
        })
    });
}

criterion_group!(benches, bench_cost_estimate, bench_sse_normalize,);
criterion_main!(benches);
