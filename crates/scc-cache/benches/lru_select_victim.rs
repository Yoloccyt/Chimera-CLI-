//! SCC LRU select_victim 规模标定 — ADR-289 登记项的数据驱动裁决(R11 W2e)
//!
//! ADR-289 登记「scc LRU O(n) 全表扫描 × 每候选 2 次分片锁」为不实施项,
//! 自辩理由是「256 条目规模下 ≈1µs」(lru.rs 头注)。本 bench 把该自辩变成
//! criterion 机证:256(默认容量)与 4096(扩容场景)两档实测 select_victim,
//! 为「维持现状 vs 升级 O(1) 结构」提供裁决数据。
//!
//! 运行:`cargo bench -p scc-cache --bench lru_select_victim`

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code

use std::sync::Arc;

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use dashmap::DashMap;
use scc_cache::lru::select_victim;
use scc_cache::{ContextEntry, ContextId};

/// 构造 n 条目的 (access_order, entries) 双表(全可驱逐,strong_count == 1)
fn make_maps(
    n: usize,
) -> (
    DashMap<ContextId, u64>,
    DashMap<ContextId, Arc<ContextEntry>>,
) {
    let access: DashMap<ContextId, u64> = DashMap::new();
    let entries: DashMap<ContextId, Arc<ContextEntry>> = DashMap::new();
    for i in 0..n {
        let name = format!("ctx-{i}");
        let id = ContextId::new(name.as_str());
        access.insert(id.clone(), i as u64);
        entries.insert(
            id,
            Arc::new(ContextEntry::new(name.as_str(), "benchmark content")),
        );
    }
    (access, entries)
}

fn bench_select_victim(c: &mut Criterion) {
    let mut group = c.benchmark_group("scc_lru_select_victim");
    // 256 = SccConfig 默认容量;4096 = 扩容场景的 O(n) 上界探针
    for n in [256usize, 4096] {
        let (access, entries) = make_maps(n);
        group.bench_function(format!("{n}_entries"), |b| {
            b.iter(|| black_box(select_victim(black_box(&access), black_box(&entries))));
        });
    }
    group.finish();
}

criterion_group!(benches, bench_select_victim);
criterion_main!(benches);
