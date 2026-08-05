//! 超窗兜底路径性能基准（PROBE P5）
//!
//! # 基准场景
//!
//! - **set_corpus_600k**: 60 万 token 语料 chunk 化 + kvbsr 聚类构成量化
//!   （chunking O(字符数) + 每块 512 维 CLV 生成 + Union-Find 聚类）
//! - **provider_select_2344**: 2344 块 dense（CLV 余弦）+ sparse（关键词命中）
//!   + RRF 融合全链路（当前规模）
//! - **provider_select_24k**: 24K 块规模化对照（P3 部分选择优化判据）
//!
//! # 约定
//!
//! 只输出分布不 panic（对齐 probe_select.rs 约定）。

use chimera_cli::overwindow_bridge::OverWindowBridge;
use criterion::{black_box, criterion_group, criterion_main, Criterion};

/// 构造指定规模语料（重复段填充；token 估算 = 字符数 / 4）
fn make_corpus(target_tokens: usize) -> String {
    let base = "模块A 处理请求路由与鉴权，模块B 负责缓存失效与回写，模块C 执行语义检索。";
    // 每段约 30 字符 ≈ 7.5 token → 段数 = target_tokens * 4 / 30
    let segments = (target_tokens * 4 / 30).max(1);
    let mut corpus = String::with_capacity(target_tokens * 4);
    for i in 0..segments {
        corpus.push_str(&format!("{base} 段{i} "));
    }
    corpus
}

fn bench_set_corpus_600k(c: &mut Criterion) {
    let bus = event_bus::EventBus::new();
    let bridge = OverWindowBridge::new(bus).unwrap();
    let corpus = make_corpus(600_000);

    c.bench_function("overwindow_set_corpus_600k_tokens", |b| {
        // set_corpus 返回 unit——black_box 包裹会触发 clippy::unit_arg（对齐 linucb_select 约定）
        b.iter(|| bridge.set_corpus(black_box(&corpus)))
    });
}

fn bench_provider_select_2344(c: &mut Criterion) {
    let bus = event_bus::EventBus::new();
    let bridge = OverWindowBridge::new(bus).unwrap();
    bridge.set_corpus(&make_corpus(600_000)); // ≈ 2344 块（256 token/块）
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("overwindow_provider_select_2344", |b| {
        b.iter(|| {
            // 超窗触发（1M 宣称 → 折减 600K < 70 万 token 语料）
            black_box(rt.block_on(bridge.run("语义检索", 700_000, 600_000)))
        })
    });
}

fn bench_provider_select_24k(c: &mut Criterion) {
    let bus = event_bus::EventBus::new();
    let bridge = OverWindowBridge::new(bus).unwrap();
    bridge.set_corpus(&make_corpus(6_000_000)); // ≈ 24K 块（规模化对照）
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("overwindow_provider_select_24k", |b| {
        b.iter(|| black_box(rt.block_on(bridge.run("语义检索", 6_000_000, 600_000))))
    });
}

criterion_group!(
    benches,
    bench_set_corpus_600k,
    bench_provider_select_2344,
    bench_provider_select_24k,
);
criterion_main!(benches);
