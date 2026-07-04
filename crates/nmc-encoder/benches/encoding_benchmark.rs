//! 文本编码延迟基准测试
//!
//! 对应 SubTask 4.5:验证文本感知器编码延迟
//! 验收标准:p95 ≤ 30ms
//!
//! # 基准场景
//! - 短文本(100 字符)编码延迟
//! - 中等文本(1KB)编码延迟
//! - 长文本(10KB)编码延迟

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use nmc_encoder::{NmcConfig, NmcEncoder, PerceptionInput};

/// 基准:不同长度文本的编码延迟
fn bench_text_encoding(c: &mut Criterion) {
    let encoder = NmcEncoder::new(NmcConfig::default()).expect("编码器构造应成功");

    let mut group = c.benchmark_group("text_encoding");

    for (name, size) in [
        ("short_100b", 100),
        ("medium_1kb", 1_000),
        ("long_10kb", 10_000),
    ] {
        let text = "a".repeat(size);
        group.bench_with_input(BenchmarkId::from_parameter(name), &text, |b, text| {
            b.iter(|| {
                encoder
                    .perceive(PerceptionInput::Text(text.clone()))
                    .expect("编码应成功");
            });
        });
    }

    group.finish();
}

/// 基准:Desktop 模态编码延迟(对比文本)
fn bench_desktop_encoding(c: &mut Criterion) {
    let encoder = NmcEncoder::new(NmcConfig::default()).expect("编码器构造应成功");

    c.bench_function("desktop_encoding", |b| {
        b.iter(|| {
            encoder
                .perceive(PerceptionInput::Desktop(nmc_encoder::DesktopCapture::new(
                    1920,
                    1080,
                    "code editor with syntax highlighting",
                )))
                .expect("编码应成功");
        });
    });
}

criterion_group!(benches, bench_text_encoding, bench_desktop_encoding);
criterion_main!(benches);
