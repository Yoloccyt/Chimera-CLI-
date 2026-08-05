//! P1-4: LLM Judge 响应解析重试与降级策略 — criterion 基准测试
//!
//! 对应架构层: L5 Knowledge
//! 对应任务: P1-4（LLM Judge 响应解析降级策略）
//!
//! # 基准项
//!
//! 1. `judge_retry_success`: 重试成功场景（第 1 次解析失败，第 2 次成功）
//!    - 测量重试循环的额外延迟（含 1 次重试等待 + 2 次 LLM 调用 + 2 次解析）
//! 2. `judge_retry_fallback`: 重试耗尽降级场景（全部 3 次尝试失败）
//!    - 测量重试全部耗尽后的降级裁决延迟
//! 3. `judge_no_retry`: 无重试场景（max_retries=0，直接解析失败）
//!    - 基线：测量无重试时的错误路径延迟
//!
//! # SLO 验收
//!
//! 重试逻辑增加的延迟上限：
//! - 重试成功：≤ (max_retries × LLM 调用延迟 + 重试等待时间)
//! - 重试降级：≤ (max_retries × LLM 调用延迟 + 重试等待时间)
//! - 重试逻辑本身（无重试时）增加延迟 < 1µs

#![forbid(unsafe_code)]

use auto_dpo::rhi_judge_client::{
    JudgeClientConfig, JudgePromptTemplate, LlmResponse, ModelRouterJudgeClient, StubLlmInvoker,
    TokenUsage,
};
use auto_dpo::JudgeClient;
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use event_bus::EventBus;
use model_router::{ModelRegistry, ModelRouter, RouterConfig};
use nexus_contracts::{ContractSpec, HarnessMeta, HarnessSpec, HopSpec, RetryPolicy};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::runtime::Runtime;

// ============================================================
// 测试辅助
// ============================================================

/// 构造最小合法 HarnessSpec
fn make_spec(version: u32, contract_count: usize) -> HarnessSpec {
    let contracts: Vec<ContractSpec> = (0..contract_count)
        .map(|i| ContractSpec {
            name: format!("contract_{i}"),
            property: format!("must_satisfy_{i}"),
            description: Some(format!("Contract #{i} for bench")),
            from: None,
            to: None,
            fields: Vec::new(),
        })
        .collect();

    HarnessSpec {
        meta: HarnessMeta {
            name: format!("bench-spec-v{version}"),
            version,
            immutable: false,
            parent: if version > 1 { Some(version - 1) } else { None },
            task_type: Some("code_refactor".to_string()),
        },
        contracts,
        hops: vec![HopSpec {
            name: "execute".to_string(),
            input_type: None,
            output_type: None,
            contracts: Vec::new(),
            description: None,
            order: Vec::new(),
            on_veto: None,
            fallback: None,
        }],
        retry: RetryPolicy::default(),
        auxiliary: None,
    }
}

/// 构造一个可重试的 LLM 调用器，前 `fail_count` 次调用返回非法 JSON
fn make_retryable_invoker(fail_count: u32) -> Arc<StubLlmInvoker> {
    let call_counter = Arc::new(AtomicU32::new(0));
    let fail_count_arc = fail_count;

    Arc::new(StubLlmInvoker::with_dynamic_response(
        move |_model_id, _prompt| {
            let current = call_counter.fetch_add(1, Ordering::SeqCst);
            if current < fail_count_arc {
                LlmResponse {
                    content: "invalid json".to_string(),
                    model_id: "bench-model".to_string(),
                    usage: TokenUsage {
                        prompt_tokens: 100,
                        completion_tokens: 10,
                    },
                }
            } else {
                LlmResponse {
                content: r#"{"winner":"current","winner_score":0.85,"loser_score":0.45,"confidence":0.9,"rationale":"bench verdict"}"#.to_string(),
                model_id: "bench-model".to_string(),
                usage: TokenUsage {
                    prompt_tokens: 100,
                    completion_tokens: 50,
                },
            }
            }
        },
    ))
}

/// 构造一个始终返回非法 JSON 的 LLM 调用器
fn make_failing_invoker() -> Arc<StubLlmInvoker> {
    Arc::new(StubLlmInvoker::with_fixed_response(
        "invalid json forever",
        "bench-model",
    ))
}

/// 构造 ModelRouter + 自定义配置
fn make_judge_client(
    invoker: Arc<StubLlmInvoker>,
    config: JudgeClientConfig,
) -> Arc<ModelRouterJudgeClient> {
    let bus = EventBus::new();
    let registry = ModelRegistry::from_config(&RouterConfig::default());
    let router = Arc::new(ModelRouter::new(registry, bus));
    Arc::new(ModelRouterJudgeClient::with_config(
        router,
        invoker,
        JudgePromptTemplate::default(),
        config,
    ))
}

// ============================================================
// Bench 1: judge_no_retry — 无重试基线（max_retries=0）
// ============================================================

/// 无重试场景：解析失败直接返回错误
/// 基线值，用于对比重试逻辑的开销
fn bench_judge_no_retry(c: &mut Criterion) {
    let invoker = make_failing_invoker();
    let config = JudgeClientConfig {
        max_retries: 0,
        fallback_on_parse_failure: false,
        retry_delay_ms: 0,
        ..Default::default()
    };
    let client = make_judge_client(invoker, config);
    let rt = Runtime::new().expect("tokio runtime");

    let spec_v_i = make_spec(2, 3);
    let spec_v_i_minus_1 = make_spec(1, 3);

    c.bench_function("judge_no_retry", |b| {
        b.to_async(&rt)
            .iter(|| client.judge(black_box(&spec_v_i), black_box(&spec_v_i_minus_1)))
    });
}

// ============================================================
// Bench 2: judge_retry_success — 重试成功场景
// ============================================================

/// 重试成功：第 1 次解析失败，第 2 次成功
/// 测量重试循环的额外延迟（含 1 次重试等待 + 2 次 LLM 调用 + 2 次解析）
fn bench_judge_retry_success(c: &mut Criterion) {
    let invoker = make_retryable_invoker(1);
    let config = JudgeClientConfig {
        max_retries: 2,
        fallback_on_parse_failure: true,
        retry_delay_ms: 1, // 使用 1ms 避免测试过慢
        ..Default::default()
    };
    let client = make_judge_client(invoker, config);
    let rt = Runtime::new().expect("tokio runtime");

    let spec_v_i = make_spec(2, 3);
    let spec_v_i_minus_1 = make_spec(1, 3);

    c.bench_function("judge_retry_success", |b| {
        b.to_async(&rt)
            .iter(|| client.judge(black_box(&spec_v_i), black_box(&spec_v_i_minus_1)))
    });
}

// ============================================================
// Bench 3: judge_retry_fallback — 重试耗尽降级场景
// ============================================================

/// 重试耗尽：全部 3 次尝试都失败，使用降级默认裁决
/// 测量重试全部耗尽 + 降级裁决的延迟
fn bench_judge_retry_fallback(c: &mut Criterion) {
    let invoker = make_failing_invoker();
    let config = JudgeClientConfig {
        max_retries: 2,
        fallback_on_parse_failure: true,
        retry_delay_ms: 1, // 使用 1ms 避免测试过慢
        ..Default::default()
    };
    let client = make_judge_client(invoker, config);
    let rt = Runtime::new().expect("tokio runtime");

    let spec_v_i = make_spec(2, 3);
    let spec_v_i_minus_1 = make_spec(1, 3);

    c.bench_function("judge_retry_fallback", |b| {
        b.to_async(&rt)
            .iter(|| client.judge(black_box(&spec_v_i), black_box(&spec_v_i_minus_1)))
    });
}

// ============================================================
// Criterion 配置
// ============================================================

criterion_group!(
    name = judge_retry;
    config = Criterion::default()
        .significance_level(0.05)
        .sample_size(10)
        .measurement_time(Duration::from_secs(3));
    targets = bench_judge_no_retry, bench_judge_retry_success, bench_judge_retry_fallback
);

criterion_main!(judge_retry);
