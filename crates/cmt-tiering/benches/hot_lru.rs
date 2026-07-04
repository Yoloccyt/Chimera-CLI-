//! CMT Hot 层 LRU 驱逐基准测试
//!
//! 对应 SubTask 11.1:引入 criterion 基准测试框架
//!
//! 基准场景:Hot 层容量 256,插入 256 条目后,测量第 257 次插入触发 LRU 驱逐的延迟。

use cmt_tiering::{CapabilityEntry, HotTier, Tier};
use criterion::{criterion_group, criterion_main, Criterion};

/// 构造已填满 256 条目的 HotTier
fn make_filled_hot_tier() -> HotTier {
    let tier = HotTier::new(256);
    for i in 0..256 {
        let entry = CapabilityEntry::new(format!("cap-{i}"), format!("content-{i}"), Tier::Hot);
        tier.insert(entry).expect("插入应成功");
    }
    tier
}

/// 基准:Hot 层 LRU 驱逐(插入第 257 个条目触发驱逐)
fn bench_hot_lru_eviction(c: &mut Criterion) {
    c.bench_function("hot_lru_eviction_257th_insert", |b| {
        b.iter_batched(
            make_filled_hot_tier,
            |tier| {
                let entry = CapabilityEntry::new("cap-256", "content-256", Tier::Hot);
                tier.insert(entry).expect("插入应成功");
            },
            criterion::BatchSize::SmallInput,
        );
    });
}

criterion_group!(benches, bench_hot_lru_eviction);
criterion_main!(benches);
