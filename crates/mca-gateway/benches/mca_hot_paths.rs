//! MCA 热路径基准 — 成本估算 + SSE 归一器 + 协商引擎 + 语义指纹
//!
//! 对应架构层:L10 Interface(mca-gateway)
//!
//! # 基准目标(RED-first,阈值标记供 check_perf_redlines 静态 lint)
//! - `bench_cost_estimate`: 路由热路径成本估算,目标 < 1μs(COST_ESTIMATE_TARGET_US)
//! - `bench_sse_normalize`: SSE 单事件归一,目标 < 5μs(SSE_EVENT_TARGET_US)
//! - `bench_negotiate_full`: 能力协商全路径,目标 < 100ns(NEGOTIATE_TARGET_NS)
//! - `bench_negotiate_budget_deep`: Deep 档预算协商 + 成本护栏,目标 < 100ns
//! - `bench_semantic_fingerprint_10msg`: 10 消息×3 工具指纹,目标 < 10μs(FINGERPRINT_TARGET_US)
//!
//! # 红线标记(静态 lint 锚点)
//! 阈值常量名即 lint 的 Threshold 标记,勿重命名。

#![forbid(unsafe_code)]

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use mca_gateway::capability::{negotiate, negotiate_budget};
use mca_gateway::semantic_fingerprint::semantic_fingerprint;
use mca_gateway::sse::StreamNormalizer;
use nexus_contracts::affinity::{
    AffinityMessage, AffinityOverrides, AffinityRequest, CapabilitySet, ContentBlock, MessageRole,
    OutputFormat, PricingSpec, ProtocolDialect, SamplingParams, ThinkingPreference,
    ThinkingSupport, ToolDecl,
};

/// 成本估算红线(μs)——路由热路径,超限即路由决策拖慢
pub const COST_ESTIMATE_TARGET_US: u64 = 1;

/// SSE 单事件归一红线(μs)——TTFT 红线承载者
pub const SSE_EVENT_TARGET_US: u64 = 5;

/// 协商引擎红线(ns)——纯算术 + match,无堆分配
pub const NEGOTIATE_TARGET_NS: u64 = 100;

/// 语义指纹红线(μs)——10 条消息 × 3 工具,~5000 字符
pub const FINGERPRINT_TARGET_US: u64 = 10;

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

/// 能力协商全路径基准(三态 × 三档 = 9 组合,纯算术 O(1))
fn bench_negotiate_full(c: &mut Criterion) {
    let mut caps = CapabilitySet::minimal_text(131_072, 131_072);
    caps.thinking = ThinkingSupport::EffortLevels(vec![
        "none".into(),
        "low".into(),
        "medium".into(),
        "high".into(),
        "max".into(),
    ]);
    let request = AffinityRequest {
        intent_id: "bench".into(),
        messages: vec![AffinityMessage {
            role: MessageRole::User,
            blocks: vec![ContentBlock::Text {
                text: "hello".into(),
            }],
        }],
        tools: vec![ToolDecl {
            name: "read".into(),
            description: "read file".into(),
            parameters_schema: "{}".into(),
        }],
        thinking_pref: ThinkingPreference::Deep,
        budget_hint_micro: None,
        overrides: AffinityOverrides::default(),
        sampling: SamplingParams::default(),
        output_format: OutputFormat::default(),
    };
    c.bench_function("negotiate_full_deep_with_tools", |b| {
        b.iter(|| black_box(negotiate(black_box(&caps), black_box(&request))))
    });
}

/// Deep 档预算协商 + 成本护栏路径基准
fn bench_negotiate_budget_deep(c: &mut Criterion) {
    let mut caps = CapabilitySet::minimal_text(131_072, 65_536);
    caps.thinking = ThinkingSupport::OnOff;
    c.bench_function("negotiate_budget_deep_k3_with_hint", |b| {
        b.iter(|| {
            black_box(negotiate_budget(
                black_box(&caps),
                ThinkingPreference::Deep,
                Some(500_000),
            ))
        })
    });
}

/// 语义指纹基准(10 条消息 × 3 工具,模拟典型开发会话)
fn bench_semantic_fingerprint_10msg(c: &mut Criterion) {
    let messages: Vec<AffinityMessage> = (0..10)
        .map(|i| AffinityMessage {
            role: if i % 2 == 0 {
                MessageRole::User
            } else {
                MessageRole::Assistant
            },
            blocks: vec![ContentBlock::Text {
                text: format!(
                    "这是第{i}条消息,包含一些中文和 English 混合内容,用于测试指纹计算性能。"
                )
                .into(),
            }],
        })
        .collect();
    let tools: Vec<ToolDecl> = (0..3)
        .map(|i| ToolDecl {
            name: format!("tool_{i}").into(),
            description: format!("Tool {i} description with some text").into(),
            parameters_schema: r#"{"type":"object","properties":{"path":{"type":"string"}}}"#
                .into(),
        })
        .collect();
    c.bench_function("semantic_fingerprint_10msg_3tools", |b| {
        b.iter(|| {
            black_box(semantic_fingerprint(
                black_box(&messages),
                black_box(&tools),
            ))
        })
    });
}

criterion_group!(
    benches,
    bench_cost_estimate,
    bench_sse_normalize,
    bench_negotiate_full,
    bench_negotiate_budget_deep,
    bench_semantic_fingerprint_10msg,
);
criterion_main!(benches);
