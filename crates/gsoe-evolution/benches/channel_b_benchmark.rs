//! RHI-CG 通道 B criterion 基准 — P5.2.5
//!
//! 对应架构层: L5 Knowledge(gsoe-evolution)
//! 对应 ADR: ADR-044 决策 5/6 + ADR-045 决策 8
//! 对应任务: P5.2.5(3 项 criterion 基准)
//!
//! # 基准清单
//!
//! ## 1. ci_gate_execute — CiGate 执行性能
//!
//! 测量通道 B CI 执行门的延迟,覆盖三条路径:
//! - `ci_gate_mock_execute`:MockCiGate(永远通过)— 测量 trait dispatch + Future 调度开销
//! - `ci_gate_cargo_dag_no_subprocess`:CargoCiGate + DAG 委托图,禁用子进程 — 测量 INV-9 通过路径
//! - `ci_gate_cargo_cyclic_no_subprocess`:CargoCiGate + 带环委托图,禁用子进程 — 测量 INV-9 违反路径
//!
//! 验收标准:MockCiGate < 100µs,CargoCiGate INV-9 路径 < 1ms(DFS 三色标记法 O(V+E))
//!
//! ## 2. significance_detection — 显著性检测性能
//!
//! 测量 SignificanceDetector 的二项检验计算延迟,覆盖四个场景:
//! - `significance_p_value_zero_streak`:streak=0, n=0(边界,立即返回 1.0)
//! - `significance_p_value_three_streak`:streak=3, n=3(关键值 0.125)
//! - `significance_is_veto_justified_three_streak`:streak=3 完整否决检查
//! - `significance_is_veto_justified_five_streak`:streak=5 完整否决检查(显著)
//!
//! 验收标准:单次 p_value() < 1µs(N <= 10 时无对数空间需求)
//!
//! ## 3. spec_registry_register — SpecRegistry 注册性能
//!
//! 测量 SpecRegistry::register_with_source 的延迟,覆盖两条路径:
//! - `spec_registry_register_no_bus`:无 EventBus(纯注册开销)
//! - `spec_registry_register_with_bus`:带 EventBus(含 SpecRegistered 事件发布)
//!
//! 验收标准:无 bus < 50µs,带 bus < 200µs(含 broadcast 发布开销)
//!
//! # 基准配置
//!
//! - warmup: 500ms(与 evolution_benchmark 一致)
//! - sample_size: 20(小样本快速验证)
//! - 测量时间:标准 criterion 默认(约 5 秒/项)
//!
//! # 学习不在关键路径(ADR-031 决策 4)
//!
//! CiGate::execute / SignificanceDetector::p_value / SpecRegistry::register 均在
//! 通道 B 后台执行,不阻塞推理路径。基准数据用于验证 P5.2 实施的延迟预算。

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use event_bus::EventBus;
use gsoe_evolution::{
    CargoCiGate, CiGate, DelegationEdge, MockCiGate, SignificanceDetector, SpecRegistry,
};
use nexus_contracts::{ContractSpec, HarnessMeta, HarnessSpec, HopSpec, RetryPolicy};
use std::time::Duration;

// ============================================================
// 测试辅助 — 构造最小合法 HarnessSpec
// ============================================================

/// 构造最小合法 HarnessSpec 用于基准测试
///
/// WHY 复用 integration.rs 的同款构造:确保 spec 通过 validate(),
/// 避免注册路径因校验失败提前返回而偏离测量目标。
fn make_bench_spec(name: &str, version: u32, parent: Option<u32>) -> HarnessSpec {
    HarnessSpec {
        meta: HarnessMeta {
            name: name.to_string(),
            version,
            immutable: false,
            parent,
            task_type: None,
        },
        contracts: vec![ContractSpec {
            name: "no_panic".to_string(),
            property: "fuzz_target_must_not_panic".to_string(),
            description: None,
            from: None,
            to: None,
            fields: vec![],
        }],
        hops: vec![HopSpec {
            name: "generate_input".to_string(),
            input_type: None,
            output_type: None,
            contracts: vec!["no_panic".to_string()],
            description: None,
            order: vec!["Architect.propose".to_string()],
            on_veto: None,
            fallback: None,
        }],
        retry: RetryPolicy::default(),
        auxiliary: Some(
            "acceptance_gates = [\"tests_pass\", \"bench_no_regression\", \"invariants_clean\", \"redline_scan_clean\"]"
                .to_string(),
        ),
    }
}

// ============================================================
// 基准 1: ci_gate_execute — CiGate 执行性能
// ============================================================

/// 基准:MockCiGate 执行(永远通过)— 测量 trait dispatch + Future 调度开销
///
/// 这是通道 B 的延迟下界,代表"CI 永远通过"的纯开销。
/// 验收标准:< 100µs
fn bench_ci_gate_mock_execute(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let gate = MockCiGate::with_passing_result();
    let spec = make_bench_spec("bench-spec", 1, None);

    c.bench_function("ci_gate_mock_execute", |b| {
        b.iter(|| {
            let spec_ref = black_box(&spec);
            rt.block_on(gate.execute(spec_ref))
                .expect("MockCiGate 不应失败")
        });
    });
}

/// 基准:CargoCiGate + DAG 委托图,禁用子进程 — 测量 INV-9 通过路径
///
/// 构造 5 节点 DAG(root→a, root→b, a→c, a→d),验证无环路径开销。
/// DFS 三色标记法 O(V+E),预期 < 1ms。
fn bench_ci_gate_cargo_dag_no_subprocess(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let edges = vec![
        DelegationEdge::new("root", "agent-a"),
        DelegationEdge::new("root", "agent-b"),
        DelegationEdge::new("agent-a", "agent-c"),
        DelegationEdge::new("agent-a", "agent-d"),
    ];
    let gate = CargoCiGate::new(edges).with_subprocess_enabled(false);
    let spec = make_bench_spec("bench-spec", 1, None);

    c.bench_function("ci_gate_cargo_dag_no_subprocess", |b| {
        b.iter(|| {
            let spec_ref = black_box(&spec);
            rt.block_on(gate.execute(spec_ref)).expect("DAG 应通过")
        });
    });
}

/// 基准:CargoCiGate + 带环委托图,禁用子进程 — 测量 INV-9 违反路径
///
/// 构造 5 节点带环图(A→B→C→A 三节点环 + D→E 无环分量),
/// 验证 DFS 检测到环后的失败聚合开销。预期 < 1ms。
fn bench_ci_gate_cargo_cyclic_no_subprocess(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let edges = vec![
        DelegationEdge::new("agent-a", "agent-b"),
        DelegationEdge::new("agent-b", "agent-c"),
        DelegationEdge::new("agent-c", "agent-a"), // 三节点环 A→B→C→A
        DelegationEdge::new("agent-d", "agent-e"), // 无环分量
    ];
    let gate = CargoCiGate::new(edges).with_subprocess_enabled(false);
    let spec = make_bench_spec("bench-spec", 1, None);

    c.bench_function("ci_gate_cargo_cyclic_no_subprocess", |b| {
        b.iter(|| {
            let spec_ref = black_box(&spec);
            // 注意:返回 Ok(CiGateResult { passed: false, ... }),非 Err
            let _result = rt
                .block_on(gate.execute(spec_ref))
                .expect("CargoCiGate 执行本身不应失败(返回 passed=false)");
        });
    });
}

// ============================================================
// 基准 2: significance_detection — 显著性检测性能
// ============================================================

/// 基准:SignificanceDetector p_value() — streak=0, n=0(边界)
///
/// 测量 binomial_sf(0, 0, 0.5) 的立即返回路径(k=0 时直接返回 1.0)。
/// 预期 < 100ns。
fn bench_significance_p_value_zero_streak(c: &mut Criterion) {
    let detector = SignificanceDetector::new();
    assert_eq!(detector.regression_streak(), 0);
    assert_eq!(detector.observed_runs(), 0);

    c.bench_function("significance_p_value_zero_streak", |b| {
        b.iter(|| black_box(detector.p_value()));
    });
}

/// 基准:SignificanceDetector p_value() — streak=3, n=3(关键值 0.125)
///
/// 测量 binomial_sf(3, 3, 0.5) 的计算路径(单次 PMF 求和)。
/// 这是 ADR-044 决策 6 的关键阈值场景。预期 < 1µs。
fn bench_significance_p_value_three_streak(c: &mut Criterion) {
    let mut detector = SignificanceDetector::new();
    detector.record_regression();
    detector.record_regression();
    detector.record_regression();
    assert_eq!(detector.regression_streak(), 3);
    assert_eq!(detector.observed_runs(), 3);

    c.bench_function("significance_p_value_three_streak", |b| {
        b.iter(|| black_box(detector.p_value()));
    });
}

/// 基准:SignificanceDetector is_veto_justified() — streak=3(不显著,p=0.125)
///
/// 完整否决检查路径:p_value() + check_veto_evidence(组合两个条件)。
/// 预期 < 2µs(含 p_value() + 短路评估)。
fn bench_significance_is_veto_justified_three_streak(c: &mut Criterion) {
    let mut detector = SignificanceDetector::new();
    detector.record_regression();
    detector.record_regression();
    detector.record_regression();

    c.bench_function("significance_is_veto_justified_three_streak", |b| {
        b.iter(|| black_box(detector.is_veto_justified()));
    });
}

/// 基准:SignificanceDetector is_veto_justified() — streak=5(显著,p=0.03125)
///
/// 完整否决检查路径(显著场景):p_value() + check_veto_evidence 均通过。
/// 这是通道 B 触发否决的关键路径。预期 < 2µs。
fn bench_significance_is_veto_justified_five_streak(c: &mut Criterion) {
    let mut detector = SignificanceDetector::new();
    for _ in 0..5 {
        detector.record_regression();
    }

    c.bench_function("significance_is_veto_justified_five_streak", |b| {
        b.iter(|| black_box(detector.is_veto_justified()));
    });
}

// ============================================================
// 基准 3: spec_registry_register — SpecRegistry 注册性能
// ============================================================

/// 基准:SpecRegistry::register_with_source 无 EventBus — 纯注册开销
///
/// 测量 validate() + HashMap 插入 + 谱系更新路径。
/// 每次迭代使用唯一 name 避免版本冲突,确保测量的是"新 spec 注册"路径。
/// 预期 < 50µs。
fn bench_spec_registry_register_no_bus(c: &mut Criterion) {
    c.bench_function("spec_registry_register_no_bus", |b| {
        let mut counter: u64 = 0;
        b.iter(|| {
            let mut registry = SpecRegistry::new();
            let name = format!("bench-spec-{counter}");
            counter = counter.wrapping_add(1);
            let spec = make_bench_spec(&name, 1, None);
            black_box(registry.register_with_source(spec, "channel-b-bench")).expect("注册应成功")
        });
    });
}

/// 基准:SpecRegistry::register_with_source 带 EventBus — 含事件发布开销
///
/// 测量完整路径:validate() + HashMap 插入 + SpecRegistered 事件构造 + publish_blocking。
/// 每次迭代创建新 EventBus(避免 subscriber 缓冲溢出影响测量)。
/// 预期 < 200µs(含 broadcast channel 发布开销)。
///
/// WHY 不订阅消费:本基准测量"发布开销",不测量"订阅者消费开销"。
///    broadcast channel 内部 Arc 克隆 + 缓冲推送是发布主要开销,与订阅无关。
///    若订阅者缓冲满,publish_blocking 会返回 Err SlowConsumerDropped,
///    但注册本身仍成功(register_with_source 内部仅 warn 日志)。
fn bench_spec_registry_register_with_bus(c: &mut Criterion) {
    c.bench_function("spec_registry_register_with_bus", |b| {
        let mut counter: u64 = 0;
        b.iter(|| {
            let bus = EventBus::new();
            // 创建一个订阅者但不消费,仅确保 bus 有订阅者 ID(避免空 bus 优化)
            let _rx = bus.subscribe();
            let mut registry = SpecRegistry::with_event_bus(bus);
            let name = format!("bench-spec-{counter}");
            counter = counter.wrapping_add(1);
            let spec = make_bench_spec(&name, 1, None);
            black_box(registry.register_with_source(spec, "channel-b-bench")).expect("注册应成功")
        });
    });
}

// ============================================================
// criterion 入口
// ============================================================

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(20)
        .warm_up_time(Duration::from_millis(500));
    targets = bench_ci_gate_mock_execute,
        bench_ci_gate_cargo_dag_no_subprocess,
        bench_ci_gate_cargo_cyclic_no_subprocess,
        bench_significance_p_value_zero_streak,
        bench_significance_p_value_three_streak,
        bench_significance_is_veto_justified_three_streak,
        bench_significance_is_veto_justified_five_streak,
        bench_spec_registry_register_no_bus,
        bench_spec_registry_register_with_bus
}

criterion_main!(benches);
