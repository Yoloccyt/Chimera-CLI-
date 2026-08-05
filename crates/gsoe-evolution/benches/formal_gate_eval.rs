//! 形式化验证器 CI 门禁聚合性能基准 — R2 解冻阶段③ 前置 2
//! 以及 P1-2 M0-Critic 守卫性能基准
//!
//! 对应架构层: L5 gsoe-evolution
//! 对应 ADR: ADR-052 待办 2 / ADR-049 决策 6(性能可证伪)
//!
//! # SLO 目标
//!
//! CI 门禁在每次进化提交时调用,聚合 7~N 个验证结果必须低开销:
//! - `evaluate`:聚合 7 属性 SLO < 10µs(CI 路径,非热路径但需快速反馈)
//! - `run_m0_guard`:M0-Critic 守卫 P1-2 SLO < 1µs(纯计算,无 I/O)
//!
//! # 运行
//! ```powershell
//! cargo bench -p gsoe-evolution --bench formal_gate_eval
//! ```

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use gsoe_evolution::aegis::AegisCritic;
use gsoe_evolution::formal_gate::{FormalVerifierGate, NamedPropertyResult};
use nexus_contracts::formal_props::VerificationResult;
use nexus_contracts::{HarnessMeta, HarnessSpec, RetryPolicy};

/// 构造 n 个 Satisfied 具名结果(模拟 n 个验证器全通过)
fn satisfied_results(n: usize) -> Vec<NamedPropertyResult> {
    (0..n)
        .map(|i| {
            NamedPropertyResult::new(
                format!("property-{i}"),
                VerificationResult::Satisfied {
                    samples_tested: 100,
                },
            )
        })
        .collect()
}

/// 门禁聚合开销(不同属性数规模:7 = 当前属性矩阵,64/256 = 未来扩展)
fn bench_evaluate(c: &mut Criterion) {
    let gate = FormalVerifierGate::new();
    let mut group = c.benchmark_group("formal_gate/evaluate");
    for &n in &[7usize, 64, 256] {
        let results = satisfied_results(n);
        group.bench_function(BenchmarkId::from_parameter(n), |b| {
            b.iter(|| {
                black_box(gate.evaluate(black_box(&results)));
            });
        });
    }
    group.finish();
}

/// M0-Critic 守卫开销: 单次 run_m0_guard 调用（含分数派生 + 三项验证）
///
/// SLO: < 1µs（纯计算，无 I/O）
fn bench_m0_critic_guard(c: &mut Criterion) {
    let critic = AegisCritic::with_config(true, 2.0);

    let base = HarnessSpec {
        meta: HarnessMeta {
            name: "bench-m0-guard".into(),
            version: 1,
            immutable: false,
            parent: None,
            task_type: None,
        },
        contracts: vec![],
        hops: vec![],
        retry: RetryPolicy::default(), // max_attempts=5
        auxiliary: None,
    };
    let mut candidate = base.clone();
    candidate.meta.version = 2;
    candidate.meta.parent = Some(1);
    candidate.retry.max_attempts = 3; // 改进: 5→3, 分数提升

    let mut group = c.benchmark_group("m0_critic_guard");
    group.bench_function("run_m0_guard_improving", |b| {
        b.iter(|| {
            let result = critic.run_m0_guard(black_box(&candidate), black_box(&base));
            let _ = black_box(result);
        });
    });
    group.finish();
}

criterion_group!(benches, bench_evaluate, bench_m0_critic_guard);
criterion_main!(benches);
