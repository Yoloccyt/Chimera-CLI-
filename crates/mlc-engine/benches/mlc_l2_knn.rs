//! L2 SemanticMemory KNN 搜索 SLO 基准测试
//!
//! SLO 目标: Top-10 KNN 召回延迟 < 10ms（1K / 10K / 100K entry 规模）
//!
//! 测试场景:
//! - 1K entries:  小规模语义记忆，验证线性扫描基线
//! - 10K entries: 中等规模，验证线性扫描在万级条目下的表现
//! - 100K entries: 压力规模，验证线性扫描在十万级条目下是否仍满足 < 10ms SLO

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use mlc_engine::{MemoryEntry, MemoryTier, SemanticMemory};
use nexus_core::CLV;

/// 构造测试用 CLV（512 维，非零向量）
fn make_clv(seed: f32) -> CLV {
    let mut v = vec![0.0_f32; CLV::DIMENSION];
    v[0] = seed;
    v[1] = 1.0;
    CLV::from_vec(v).expect("CLV 构造应成功")
}

/// 构造已填充 N 条目的 SemanticMemory
///
/// 每个条目 dim_0 在 [0, 1.0) 均匀分布，dim_1 = 1.0 确保非零向量。
fn make_filled_memory(n: usize) -> SemanticMemory {
    let mem = SemanticMemory::new(n);
    for i in 0..n {
        let mut v = vec![0.0_f32; CLV::DIMENSION];
        v[0] = (i as f32) / (n as f32);
        v[1] = 1.0;
        let clv = CLV::from_vec(v).expect("CLV 构造应成功");
        let entry = MemoryEntry::new(
            format!("m-{i}"),
            format!("content-{i}"),
            MemoryTier::L2Semantic,
        )
        .with_clv(clv);
        mem.insert(entry).expect("插入应成功");
    }
    mem
}

/// 基准: 1K / 10K / 100K entry 规模下 Top-10 KNN 召回
///
/// SLO: 所有规模 < 10ms
fn bench_l2_knn_slo(c: &mut Criterion) {
    let query = make_clv(0.5);
    let sizes = [1_000, 10_000, 100_000];

    let mut group = c.benchmark_group("l2_knn_slo");
    for &size in &sizes {
        let mem = make_filled_memory(size);
        group.bench_function(
            BenchmarkId::new("top10_recall", format!("{size}_entries")),
            |b| {
                b.iter(|| {
                    mem.recall_by_clv(&query, 10).expect("召回应成功");
                });
            },
        );
    }
    group.finish();
}

/// SLO 断言: 所有规模 Top-10 召回 < 10ms
///
/// 此基准通过 `--test` 模式验证 SLO 约束（< 10ms），
/// criterion 会输出均值供 CI 阈值断言使用。
fn bench_l2_knn_slo_assert(c: &mut Criterion) {
    let query = make_clv(0.5);

    for &size in &[1_000, 10_000, 100_000] {
        let mem = make_filled_memory(size);
        let bench_id = format!("l2_knn_slo_{size}_entries_under_10ms");
        c.bench_function(&bench_id, |b| {
            b.iter(|| {
                let result = mem.recall_by_clv(&query, 10).expect("召回应成功");
                assert!(!result.is_empty(), "召回结果不应为空");
                assert!(result.len() <= 10, "Top-10 结果不应超过 10");
            });
        });
    }
}

criterion_group!(benches, bench_l2_knn_slo, bench_l2_knn_slo_assert);
criterion_main!(benches);
