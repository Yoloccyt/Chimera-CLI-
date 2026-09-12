//! xts_top_k 基准 — 红线 #8 Top-K 收敛对照(WS-2 C1)
//!
//! 对照 `xts_top_k`(O(n) partial-sort + O(k log k) 局部排序)vs 原有
//! `sort_by` 全排基线(O(n log n))。随机 u64 输入,n = 10^4 与 10^5,k = n/10。
use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion};
use nexus_contracts::util::xts_top_k;

const N_SMALL: usize = 10_000;
const N_LARGE: usize = 100_000;

fn gen(len: usize) -> Vec<u64> {
    // 确定性伪随机(LGC),避免依赖 rand;使 bench 可复现
    let mut seed: u64 = 0x9E3779B97F4A7C15;
    (0..len)
        .map(|_| {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            seed
        })
        .collect()
}

fn bench_xts_top_k(c: &mut Criterion) {
    for n in [N_SMALL, N_LARGE] {
        let k = n / 10;
        let source = gen(n);

        // xts_top_k(O(n) partial-sort)
        c.bench_function(&format!("xts_top_k n={}", n), |b| {
            b.iter_batched(
                || source.clone(),
                |mut v| {
                    let top = xts_top_k(&mut v, black_box(k));
                    black_box(top.len());
                },
                BatchSize::SmallInput,
            )
        });

        // 全排基线:sort_by 降序
        c.bench_function(&format!("sort_by_full n={}", n), |b| {
            b.iter_batched(
                || source.clone(),
                |mut v| {
                    v.sort_by(|a, b| b.cmp(a));
                    black_box(v.len());
                },
                BatchSize::SmallInput,
            )
        });

        // xts_top_k 只排前段(不含 clone 成本,对比纯选择)
        c.bench_function(&format!("xts_top_k_only_top n={}", n), |b| {
            b.iter_batched(
                || source.clone(),
                |mut v| {
                    xts_top_k(&mut v, black_box(k));
                    black_box(v[..k].len());
                },
                BatchSize::SmallInput,
            )
        });
    }
}

criterion_group!(benches, bench_xts_top_k);
criterion_main!(benches);
