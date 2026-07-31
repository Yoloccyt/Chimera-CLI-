//! 归档语义去重性能基准 — L9 优化 2.4(dedup_semantic 配对降常数证伪)
//!
//! 三档规模(200/500/1000 条目)× 跨 Agent 分布:
//! - 每条目 128 维随机 embedding(贴近真实 CLV 维度量级)
//! - 条目均匀分配到 4 个 Agent,保证大量跨 Agent 配对(去重核心场景)
//!
//! 基线用途:优化前记录 O(n²) 配对基线,优化后接入 bench_check.yml 守护
//! (1000 条目 dedup < 50ms)。全排序 O(n² log n) 保留(贪心正确性所需)。

use chimera_mas::archive::{DedupEngine, DedupEntry};
use chrono::Utc;
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};

/// 构造 `n` 条目:4 Agent 轮流,embedding 含少量重复簇(触发语义去重路径)
fn make_entries(n: usize) -> Vec<DedupEntry> {
    (0..n)
        .map(|i| {
            // 每 10 条共享一个基向量方向 → 制造语义相似簇(跨 Agent 可去重)
            let cluster = i / 10;
            let mut embedding = vec![0.01_f32; 128];
            embedding[cluster % 128] = 1.0;
            embedding[(cluster + 1) % 128] = 0.5;
            DedupEntry::new(
                format!("entry-{i}"),
                format!("agent-{}", i % 4),
                embedding,
                // content_hash=0 表示未计算,跳过精确去重直入语义去重路径
                0,
                0.5 + (i % 5) as f32 * 0.1,
                Utc::now(),
            )
        })
        .collect()
}

fn bench_dedup(c: &mut Criterion) {
    let engine = DedupEngine::new();
    let mut group = c.benchmark_group("dedup_semantic");
    for &n in &[200usize, 500, 1000] {
        let entries = make_entries(n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &entries, |b, entries| {
            b.iter(|| engine.dedup(black_box(entries)));
        });
    }
    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(20);
    targets = bench_dedup
}
criterion_main!(benches);
