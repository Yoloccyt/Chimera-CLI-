//! HCW 上下文压缩基准测试
//!
//! 对应 SubTask 11.1:引入 criterion 基准测试框架
//!
//! 基准场景:构造 100K Token 上下文(100 个条目 × 1000 token/条),
//! 测量 `ContextCompressor::compress` 压缩到 32K 的延迟。

use std::sync::Arc;

use chrono::Utc;
use criterion::{criterion_group, criterion_main, Criterion};
use hcw_window::{ContextCompressor, ContextEntry, HcwConfig};

/// 构造 100K Token 的上下文条目列表(100 个条目 × 1000 token/条)
///
/// WHY(M-01/M-02):compress 签名改为 `&[Arc<ContextEntry>]`,
/// 此处返回 `Vec<Arc<ContextEntry>>` 以匹配签名,用 `.map(Arc::new)` 包装
fn make_100k_entries() -> Vec<Arc<ContextEntry>> {
    (0..100)
        .map(|i| {
            let mut entry =
                ContextEntry::new(format!("e-{i}"), "file-1", format!("content-{i}"), 1000);
            entry.access_count = (i % 10) as u32;
            entry.last_accessed_at = Utc::now() - chrono::Duration::milliseconds(i as i64 * 10);
            Arc::new(entry)
        })
        .collect()
}

/// 基准:100K Token 压缩到 32K
fn bench_compress_100k_to_32k(c: &mut Criterion) {
    let config = HcwConfig::default();
    c.bench_function("compress_100k_to_32k", |b| {
        b.iter_with_setup(make_100k_entries, |entries| {
            ContextCompressor::compress(&config, &entries, 32_000, None, Utc::now());
        });
    });
}

criterion_group!(benches, bench_compress_100k_to_32k);
criterion_main!(benches);
