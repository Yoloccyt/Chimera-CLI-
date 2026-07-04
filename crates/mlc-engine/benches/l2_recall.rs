//! L2 SemanticMemory 召回基准测试
//!
//! 对应 SubTask 11.1:引入 criterion 基准测试框架
//!
//! 基准场景:
//! - 100 条目 Top-10 召回:验证 criterion 框架能正常运行(冒烟基准)
//! - 4096 条目 Top-10 召回:验证设计目标 < 200ms(满容量场景)

use criterion::{criterion_group, criterion_main, Criterion};
use mlc_engine::{MemoryEntry, MemoryTier, SemanticMemory};
use nexus_core::CLV;

/// 构造测试用 CLV(512 维,非零向量)
fn make_clv(seed: f32) -> CLV {
    let mut v = vec![0.0_f32; CLV::DIMENSION];
    v[0] = seed;
    v[1] = 1.0; // 确保非零向量,避免零向量导致相似度未定义
    CLV::from_vec(v).expect("CLV 构造应成功")
}

/// 构造已填充 100 条目的 SemanticMemory(冒烟基准)
fn make_filled_memory() -> SemanticMemory {
    let mem = SemanticMemory::new(4096);
    for i in 0..100 {
        let mut v = vec![0.0_f32; CLV::DIMENSION];
        v[0] = (i as f32) * 0.01; // dim_0 递增,确保条目间有区分度
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

/// 构造已填充 4096 条目的 SemanticMemory(满容量,验证设计目标 < 200ms)
///
/// WHY 4096 规模:L2 SemanticMemory 设计容量为 4096,需验证满容量下
/// Top-10 线性扫描 KNN 召回延迟 < 200ms(架构手册 §L2 性能基准)。
fn make_filled_memory_4096() -> SemanticMemory {
    let mem = SemanticMemory::new(4096);
    for i in 0..4096 {
        let mut v = vec![0.0_f32; CLV::DIMENSION];
        // dim_0 在 [0, 1.0) 均匀分布,确保 4096 条目间有区分度
        v[0] = (i as f32) / 4096.0;
        v[1] = 1.0; // 确保非零向量
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

/// 基准:100 条目 Top-10 召回(冒烟基准)
fn bench_l2_recall(c: &mut Criterion) {
    let mem = make_filled_memory();
    let query = make_clv(0.5); // 与中间的条目最相似

    c.bench_function("l2_recall_top10_100_entries", |b| {
        b.iter(|| {
            mem.recall_by_clv(&query, 10).expect("召回应成功");
        });
    });
}

/// 基准:4096 条目 Top-10 召回(验证设计目标 < 200ms)
///
/// 4096 条目 × 512 维线性扫描,理论计算量 ~2M 次乘加,
/// 现代 CPU 单核 < 10ms 即可完成,criterion 会输出实际延迟供确认。
fn bench_l2_recall_4096(c: &mut Criterion) {
    let mem = make_filled_memory_4096();
    let query = make_clv(0.5); // 与中间的条目最相似

    c.bench_function("l2_recall_top10_4096_entries", |b| {
        b.iter(|| {
            mem.recall_by_clv(&query, 10).expect("召回应成功");
        });
    });
}

criterion_group!(benches, bench_l2_recall, bench_l2_recall_4096);
criterion_main!(benches);
