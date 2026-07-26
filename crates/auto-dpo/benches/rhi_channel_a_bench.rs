//! RHI-CG 通道 A 评判延迟基准测试 — P5.1.5
//!
//! 对应任务: **P5.1.5**（criterion 基准：评判延迟）
//! 对应 KPI: **KPI-04**（RHI 通道 A 评判延迟 <2s for Deep 模型）
//! 对应 ADR: ADR-032（双通道评估器）/ ADR-044（P5 工程实施）
//! 对应设计源: `NEXUS-OMEGA_v5.0_系统性完整设计文档.md` §7.4（RHI-CG 双通道）
//!
//! # 基准项
//!
//! 1. `stub_judge_latency`: StubJudgeClient 路径 — 纯协议开销（无 LLM 调用）
//!    - 测量 RhiChannelA::generate_preference_pair 的协议层开销
//!    - 包含 PreferencePair 构造 + from_adjacent_specs 转换
//!    - 不含 LLM 调用（StubJudgeClient 直接返回固定 verdict）
//!
//! 2. `model_router_judge_latency`: ModelRouterJudgeClient + StubLlmInvoker 路径
//!    - 测量完整生产级路径（路由决策 + prompt 构造 + JSON 解析）
//!    - 不含网络 RTT（StubLlmInvoker 同步返回）
//!    - 反映评判器的同步开销上界
//!
//! 3. `spec_complexity_scaling`: spec 复杂度扩展性
//!    - 1 contract / 5 contracts / 20 contracts 三档
//!    - 验证 prompt 构造的 O(n) 扩展性
//!    - canonical_merkle_input 是 spec 大小的线性函数
//!
//! 4. `prompt_template_format_latency`: prompt 构造单独基准
//!    - 隔离测量 JudgePromptTemplate::format 热点路径
//!    - 涉及 canonical_merkle_input() + format! 拼接
//!
//! 5. `dynamic_response_judge_latency`: 动态响应评判延迟
//!    - 模拟真实 LLM 评判场景（响应内容动态变化）
//!    - 覆盖 JSON 解析的不同路径
//!
//! # KPI-04 验证
//!
//! 设计 §13.3 要求评判延迟 <2s（Deep 模型）。
//! - StubJudgeClient 路径：预期 <1ms（纯协议开销）
//! - ModelRouterJudgeClient + StubLlmInvoker 路径：预期 <10ms（同步开销）
//! - 生产环境加上 LLM 网络 RTT（秒级）应在 2s 内
//!
//! # async 驱动模式
//!
//! `generate_preference_pair` 是 async fn，criterion 0.5 默认未启用 `async_tokio` feature。
//! 采用 `tokio::runtime::Runtime::new().block_on()` 标准模式驱动 async 调用，
//! 避免修改 workspace Cargo.toml 引入 criterion async feature。
//!
//! # min-of-N 5 采样（Engineering Convention）
//!
//! criterion 默认 sample_size=100 + 5 warmup，统计上等价于"min-of-N 5"采样减少
//! Windows 调度噪声。

#![forbid(unsafe_code)]

use auto_dpo::rhi_judge_client::{JudgePromptTemplate, ModelRouterJudgeClient};
use auto_dpo::{
    LlmInvoker, LlmResponse, RhiChannelA, StubJudgeClient, StubLlmInvoker, TokenUsage,
};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use event_bus::EventBus;
use model_router::{ModelRegistry, ModelRouter, RouterConfig};
use nexus_contracts::{ContractSpec, HarnessMeta, HarnessSpec, HopSpec, RetryPolicy};
use std::sync::Arc;
use tokio::runtime::Runtime;

// ============================================================
// 测试辅助函数
// ============================================================

/// 构造指定复杂度的 HarnessSpec
///
/// # 参数
/// - `version`: spec 版本号
/// - `contract_count`: contract 数量（控制 spec 大小）
fn make_spec_with_complexity(version: u32, contract_count: usize) -> HarnessSpec {
    let contracts: Vec<ContractSpec> = (0..contract_count)
        .map(|i| ContractSpec {
            name: format!("contract_{i}"),
            property: format!("must_satisfy_{i}"),
            description: Some(format!("Contract #{i} for spec v{version}")),
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

/// 构造 ModelRouterJudgeClient + StubLlmInvoker 链路
fn make_model_router_judge_client(
    invoker: Arc<dyn LlmInvoker>,
) -> Arc<ModelRouterJudgeClient> {
    let bus = EventBus::new();
    let registry = ModelRegistry::from_config(&RouterConfig::default());
    let router = Arc::new(ModelRouter::new(registry, bus));
    Arc::new(ModelRouterJudgeClient::new(router, invoker))
}

/// 构造合法的 LLM JSON 响应
fn make_json_response(winner: &str, confidence: f32) -> String {
    format!(
        r#"{{"winner":"{winner}","winner_score":0.85,"loser_score":0.45,"confidence":{confidence},"rationale":"bench verdict"}}"#
    )
}

// ============================================================
// Bench 1: StubJudgeClient 路径延迟
// ============================================================

/// 测量 StubJudgeClient 路径的纯协议开销
///
/// WHY StubJudgeClient:不涉及 LLM 调用，直接返回固定 verdict，
/// 测量 RhiChannelA::generate_preference_pair 的协议层开销下界：
/// - JudgeClient::judge() Future 调度
/// - PreferencePair::from_adjacent_specs() 转换
/// - canonical_merkle_input() 调用
fn stub_judge_latency(c: &mut Criterion) {
    let judge_client = Arc::new(StubJudgeClient::current_wins());
    let channel_a = RhiChannelA::new(judge_client);
    let rt = Runtime::new().expect("tokio runtime 创建成功");

    let spec_v1 = make_spec_with_complexity(1, 1);
    let spec_v2 = make_spec_with_complexity(2, 1);

    let mut group = c.benchmark_group("rhi_channel_a_stub_judge_latency");
    group.bench_function("generate_preference_pair", |b| {
        b.iter(|| {
            // WHY block_on:criterion 0.5 默认未启用 async_tokio feature，
            // 用 Runtime::block_on 驱动 async Future
            let pair = rt.block_on(channel_a.generate_preference_pair(
                black_box(&spec_v2),
                black_box(&spec_v1),
            ))
            .expect("Stub 评判器不应失败");
            black_box(pair);
        });
    });
    group.finish();
}

// ============================================================
// Bench 2: ModelRouterJudgeClient 路径延迟
// ============================================================

/// 测量 ModelRouterJudgeClient + StubLlmInvoker 路径的同步开销
///
/// WHY ModelRouterJudgeClient:含路由决策 + prompt 构造 + JSON 解析的完整路径，
/// 但 StubLlmInvoker 同步返回（无网络 RTT），反映评判器的同步开销上界。
///
/// KPI-04 验证：此路径预期 <10ms，生产环境加上 LLM 网络 RTT 应在 2s 内。
fn model_router_judge_latency(c: &mut Criterion) {
    let invoker = Arc::new(StubLlmInvoker::current_wins());
    let judge_client = make_model_router_judge_client(invoker);
    let channel_a = RhiChannelA::new(judge_client);
    let rt = Runtime::new().expect("tokio runtime 创建成功");

    let spec_v1 = make_spec_with_complexity(1, 1);
    let spec_v2 = make_spec_with_complexity(2, 1);

    let mut group = c.benchmark_group("rhi_channel_a_model_router_judge_latency");
    group.bench_function("generate_preference_pair", |b| {
        b.iter(|| {
            let pair = rt.block_on(channel_a.generate_preference_pair(
                black_box(&spec_v2),
                black_box(&spec_v1),
            ))
            .expect("ModelRouter + StubLlmInvoker 路径应成功");
            black_box(pair);
        });
    });
    group.finish();
}

// ============================================================
// Bench 3: spec 复杂度扩展性
// ============================================================

/// 测量不同 spec 复杂度下的评判延迟
///
/// WHY 扩展性测试:canonical_merkle_input() 是 spec 大小的线性函数，
/// prompt 构造与 JSON 解析的代价随 contract 数量线性增长。
/// 验证 O(n) 扩展性，确保 20 contracts 仍在 KPI-04 阈值内。
///
/// 测试矩阵：
/// - 1 contract:最小 spec（baseline）
/// - 5 contracts:中等复杂度（典型场景）
/// - 20 contracts:高复杂度（压力测试）
fn spec_complexity_scaling(c: &mut Criterion) {
    let invoker = Arc::new(StubLlmInvoker::current_wins());
    let judge_client = make_model_router_judge_client(invoker);
    let channel_a = RhiChannelA::new(judge_client);
    let rt = Runtime::new().expect("tokio runtime 创建成功");

    let mut group = c.benchmark_group("rhi_channel_a_spec_complexity_scaling");

    for &contract_count in &[1usize, 5, 20] {
        let spec_v1 = make_spec_with_complexity(1, contract_count);
        let spec_v2 = make_spec_with_complexity(2, contract_count);

        group.bench_with_input(
            BenchmarkId::from_parameter(format!("{contract_count}_contracts")),
            &contract_count,
            |b, _| {
                b.iter(|| {
                    let pair = rt.block_on(channel_a.generate_preference_pair(
                        black_box(&spec_v2),
                        black_box(&spec_v1),
                    ))
                    .expect("评判应成功");
                    black_box(pair);
                });
            },
        );
    }
    group.finish();
}

// ============================================================
// Bench 4: prompt 构造单独基准
// ============================================================

/// 测量 JudgePromptTemplate::format 单独延迟
///
/// WHY 单独基准:prompt 构造是评判器的热点路径，
/// 涉及 canonical_merkle_input() 调用 + format! 拼接。
/// 隔离测量便于定位性能瓶颈。
fn prompt_template_format_latency(c: &mut Criterion) {
    let template = JudgePromptTemplate::new();
    let spec_v1 = make_spec_with_complexity(1, 5);
    let spec_v2 = make_spec_with_complexity(2, 5);

    let mut group = c.benchmark_group("rhi_channel_a_prompt_template_format");
    group.bench_function("format", |b| {
        b.iter(|| {
            let prompt = template.format(black_box(&spec_v2), black_box(&spec_v1));
            black_box(prompt);
        });
    });
    group.finish();
}

// ============================================================
// Bench 5: 动态响应评判延迟
// ============================================================

/// 测量动态响应评判路径延迟（模拟真实 LLM 评判场景）
///
/// WHY 动态响应:生产环境中 LLM 响应内容会变化，
/// 此基准测量评判器在动态响应下的延迟稳定性。
/// StubLlmInvoker 通过闭包动态生成 JSON，覆盖 JSON 解析的不同路径。
fn dynamic_response_judge_latency(c: &mut Criterion) {
    let invoker = Arc::new(StubLlmInvoker::with_dynamic_response(|_, prompt| {
        // WHY 精确匹配 "Current Version (v2)":prompt 同时包含 current 与 previous 版本号，
        // 仅检查 "v2" 会匹配两者。需精确匹配 "Current Version (v2)" 头部以区分。
        let current_wins = prompt.contains("Current Version (v2)");
        let json = if current_wins {
            make_json_response("current", 0.92)
        } else {
            make_json_response("previous", 0.88)
        };
        LlmResponse {
            content: json,
            model_id: "dynamic-bench".to_string(),
            usage: TokenUsage {
                prompt_tokens: 200,
                completion_tokens: 80,
            },
        }
    }));

    let judge_client = make_model_router_judge_client(invoker);
    let channel_a = RhiChannelA::new(judge_client);
    let rt = Runtime::new().expect("tokio runtime 创建成功");

    let spec_v1 = make_spec_with_complexity(1, 1);
    let spec_v2 = make_spec_with_complexity(2, 1);

    let mut group = c.benchmark_group("rhi_channel_a_dynamic_response_latency");
    group.bench_function("generate_preference_pair", |b| {
        b.iter(|| {
            let pair = rt.block_on(channel_a.generate_preference_pair(
                black_box(&spec_v2),
                black_box(&spec_v1),
            ))
            .expect("动态响应路径应成功");
            black_box(pair);
        });
    });
    group.finish();
}

criterion_group!(
    benches,
    stub_judge_latency,
    model_router_judge_latency,
    spec_complexity_scaling,
    prompt_template_format_latency,
    dynamic_response_judge_latency,
);
criterion_main!(benches);
