//! cosine_similarity_slices 多维度基准测试
//!
//! 覆盖 64 / 128 / 256 / 512 / 1024 维 f32 向量,
//! 测试零向量 / 相同向量 / 随机向量三种场景。
//! 使用 `std::hint::black_box` 防止编译器过度优化。

#![forbid(unsafe_code)]

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use nexus_core::cosine_similarity_slices;

/// 生成确定性伪随机 f32 向量（seed 简单线性同余）
fn make_random_vec(len: usize, seed: u64) -> Vec<f32> {
    let mut state = seed;
    (0..len)
        .map(|_| {
            // 简单 LCG: state = state * 6364136223846793005 + 1
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1);
            // 取高 32 位映射到 [-1, 1]
            let bits = (state >> 32) as u32;
            (bits as f32 / u32::MAX as f32) * 2.0 - 1.0
        })
        .collect()
}

/// 基准: cosine_similarity_slices — 不同维度 × 不同场景
fn bench_cosine_similarity_slices(c: &mut Criterion) {
    let dims = [64, 128, 256, 512, 1024];

    for dim in dims {
        let zero = vec![0.0f32; dim];
        let ones = vec![1.0f32; dim];
        let rand_a = make_random_vec(dim, 42);
        let rand_b = make_random_vec(dim, 1337);

        let mut group = c.benchmark_group(format!("cosine_slices_{dim}d"));

        // 零向量 vs 非零向量
        group.bench_with_input(BenchmarkId::new("zero", dim), &dim, |b, _| {
            b.iter(|| {
                let _ = cosine_similarity_slices(black_box(&zero), black_box(&ones));
            });
        });

        // 相同向量（相似度 = 1.0）
        group.bench_with_input(BenchmarkId::new("identical", dim), &dim, |b, _| {
            b.iter(|| {
                let _ = cosine_similarity_slices(black_box(&ones), black_box(&ones));
            });
        });

        // 随机向量
        group.bench_with_input(BenchmarkId::new("random", dim), &dim, |b, _| {
            b.iter(|| {
                let _ = cosine_similarity_slices(black_box(&rand_a), black_box(&rand_b));
            });
        });

        group.finish();
    }
}

criterion_group!(benches, bench_cosine_similarity_slices);
criterion_main!(benches);
