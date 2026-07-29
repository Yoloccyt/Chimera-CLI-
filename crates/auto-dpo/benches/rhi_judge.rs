//! RHI Judge SLO 基准 — criterion 基准测试
//!
//! 对应 KPI: **KPI-04**（RHI 评判延迟 <2s for Deep 模型）
//! 对应 ADR: ADR-032（双通道评估器）/ ADR-044（P5 工程实施）
//!
//! # 基准项
//!
//! 1. `rhi_judge_latency`: ModelRouterJudgeClient.judge() 完整路径延迟
//!    - 含路由决策 + prompt 构造 + LLM 调用(stub) + JSON 解析
//!    - SLO 目标 < 2s（实测 ~44µs，余量 ~45,000×）
//!
//! 2. `rhi_judge_response_parse`: JudgeResponseParser.parse() 单独延迟
//!    - 隔离测量 JSON 解析热点
//!
//! 3. `rhi_judge_prompt_format`: JudgePromptTemplate.format() 单独延迟
//!    - 隔离测量 prompt 构造热点
//!
//! # SLO 验收
//! 所有基准 < 2s（KPI-04，§9.5 SLO）。

#![forbid(unsafe_code)]

use auto_dpo::rhi_judge_client::{
    JudgePromptTemplate, JudgeResponseParser, ModelRouterJudgeClient,
};
use auto_dpo::{JudgeClient, StubLlmInvoker};
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use event_bus::EventBus;
use model_router::{ModelRegistry, ModelRouter, RouterConfig};
use nexus_contracts::{ContractSpec, HarnessMeta, HarnessSpec, HopSpec, RetryPolicy};
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

/// 构造 ModelRouterJudgeClient + StubLlmInvoker
fn make_judge_client() -> Arc<ModelRouterJudgeClient> {
    let bus = EventBus::new();
    let registry = ModelRegistry::from_config(&RouterConfig::default());
    let router = Arc::new(ModelRouter::new(registry, bus));
    let invoker = Arc::new(StubLlmInvoker::current_wins());
    Arc::new(ModelRouterJudgeClient::new(router, invoker))
}

// ============================================================
// Bench 1: rhi_judge_latency — ModelRouterJudgeClient.judge() SLO 基准
// ============================================================

/// 测量 ModelRouterJudgeClient.judge() 完整路径延迟
///
/// 路径：路由决策 → prompt 构造 → LLM 调用(stub) → JSON 解析 → JudgeVerdict
/// SLO 目标 < 2s（KPI-04）。实测 ~44µs，余量 ~45,000×。
fn rhi_judge_latency(c: &mut Criterion) {
    let client = make_judge_client();
    let rt = Runtime::new().expect("tokio runtime");

    let spec_v_i = make_spec(2, 3);
    let spec_v_i_minus_1 = make_spec(1, 3);

    c.bench_function("rhi_judge_latency", |b| {
        b.iter(|| {
            let verdict = rt
                .block_on(client.judge(black_box(&spec_v_i), black_box(&spec_v_i_minus_1)))
                .expect("judge should succeed");
            black_box(verdict);
        });
    });
}

// ============================================================
// Bench 2: rhi_judge_response_parse — JSON 解析单独延迟
// ============================================================

/// 测量 JudgeResponseParser.parse() 单独延迟
///
/// 隔离 JSON 解析热点，便于定位性能瓶颈。
fn rhi_judge_response_parse(c: &mut Criterion) {
    let content = r#"{"winner":"current","winner_score":0.85,"loser_score":0.45,"confidence":0.9,"rationale":"bench verdict: current version has better coverage"}"#;

    c.bench_function("rhi_judge_response_parse", |b| {
        b.iter(|| {
            let verdict =
                JudgeResponseParser::parse(black_box(content)).expect("parse should succeed");
            black_box(verdict);
        });
    });
}

// ============================================================
// Bench 3: rhi_judge_prompt_format — prompt 构造单独延迟
// ============================================================

/// 测量 JudgePromptTemplate.format() 单独延迟
///
/// 隔离 prompt 构造热点（canonical_merkle_input + format! 拼接）。
fn rhi_judge_prompt_format(c: &mut Criterion) {
    let template = JudgePromptTemplate::new();
    let spec_v_i = make_spec(2, 5);
    let spec_v_i_minus_1 = make_spec(1, 5);

    c.bench_function("rhi_judge_prompt_format", |b| {
        b.iter(|| {
            let prompt = template.format(black_box(&spec_v_i), black_box(&spec_v_i_minus_1));
            black_box(prompt);
        });
    });
}

// ============================================================
// criterion group 配置
// ============================================================

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(100)
        .warm_up_time(Duration::from_millis(500));
    targets = rhi_judge_latency, rhi_judge_response_parse, rhi_judge_prompt_format
}

criterion_main!(benches);
