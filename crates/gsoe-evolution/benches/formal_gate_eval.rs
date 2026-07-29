//! 形式化验证器 CI 门禁聚合性能基准 — R2 解冻阶段③ 前置 2
//!
//! 对应架构层: L5 gsoe-evolution
//! 对应 ADR: ADR-052 待办 2 / ADR-049 决策 6(性能可证伪)
//!
//! # SLO 目标
//!
//! CI 门禁在每次进化提交时调用,聚合 7~N 个验证结果必须低开销:
//! - `evaluate`:聚合 7 属性 SLO < 10µs(CI 路径,非热路径但需快速反馈)
//!
//! # 运行
//! ```powershell
//! cargo bench -p gsoe-evolution --bench formal_gate_eval
//! ```

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use gsoe_evolution::formal_gate::{FormalVerifierGate, NamedPropertyResult};
use nexus_contracts::formal_props::VerificationResult;

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

criterion_group!(benches, bench_evaluate);
criterion_main!(benches);
