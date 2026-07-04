//! CLV 编码与余弦相似性基准测试
//!
//! 对应任务:Week 8 Task 18 SubTask 18.2
//! 架构层:L1 Core
//!
//! # 基准场景
//! - CLV::from_vec 构造延迟(512 维向量)
//! - CLV::cosine_similarity 计算延迟(相同向量 / 正交向量 / 一般向量)
//!
//! # criterion 0.5
//! 使用 `criterion_group!` + `criterion_main!` 宏注册基准。
//! `harness = false` 在 Cargo.toml 中声明,禁用 libtest 默认 harness。

#![forbid(unsafe_code)]

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use nexus_core::CLV;

/// 基准:CLV::from_vec 构造延迟
///
/// 测量从 `Vec<f32>` 构造 CLV 的延迟,验证零拷贝路径(Array1::from_vec)
/// 在 512 维度下的性能特征。生成 3 种向量模式:零向量、全 1 向量、随机值向量。
fn bench_clv_from_vec(c: &mut Criterion) {
    let mut group = c.benchmark_group("clv_from_vec");

    let cases: Vec<(&str, Vec<f32>)> = vec![
        ("zero", vec![0.0f32; CLV::DIMENSION]),
        ("ones", vec![1.0f32; CLV::DIMENSION]),
        (
            "ramp",
            (0..CLV::DIMENSION).map(|i| i as f32 * 0.001).collect(),
        ),
    ];

    for (name, vec) in cases {
        group.bench_with_input(BenchmarkId::from_parameter(name), &vec, |b, v| {
            b.iter(|| {
                CLV::from_vec(v.clone()).expect("CLV 构造应成功");
            });
        });
    }

    group.finish();
}

/// 基准:CLV::cosine_similarity 计算延迟
///
/// 测量余弦相似度计算的延迟,覆盖 3 种几何关系:
/// - identical:相同向量,相似度 = 1.0
/// - orthogonal:正交向量(前半非零 vs 后半非零),相似度 = 0.0
/// - general:一般向量,相似度 in (0, 1)
fn bench_clv_cosine_similarity(c: &mut Criterion) {
    // 准备基准数据
    let identical = CLV::from_vec(vec![1.0f32; CLV::DIMENSION]).unwrap();

    let mut v_orth_a = vec![0.0f32; CLV::DIMENSION];
    let mut v_orth_b = vec![0.0f32; CLV::DIMENSION];
    for i in 0..(CLV::DIMENSION / 2) {
        v_orth_a[i] = 1.0;
        v_orth_b[CLV::DIMENSION / 2 + i] = 1.0;
    }
    let orth_a = CLV::from_vec(v_orth_a).unwrap();
    let orth_b = CLV::from_vec(v_orth_b).unwrap();

    let general_a = CLV::from_vec((0..CLV::DIMENSION).map(|i| (i as f32).sin()).collect()).unwrap();
    let general_b = CLV::from_vec(
        (0..CLV::DIMENSION)
            .map(|i| (i as f32 * 0.5).cos())
            .collect(),
    )
    .unwrap();

    let mut group = c.benchmark_group("clv_cosine_similarity");

    let cases: Vec<(&str, &CLV, &CLV)> = vec![
        ("identical", &identical, &identical),
        ("orthogonal", &orth_a, &orth_b),
        ("general", &general_a, &general_b),
    ];

    for (name, a, b) in cases {
        group.bench_with_input(
            BenchmarkId::from_parameter(name),
            &(a, b),
            |bencher, (a, b)| {
                bencher.iter(|| {
                    let _sim = a.cosine_similarity(b);
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_clv_from_vec, bench_clv_cosine_similarity);
criterion_main!(benches);
