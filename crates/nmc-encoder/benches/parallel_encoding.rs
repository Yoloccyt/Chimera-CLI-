//! NMC 批量感知编码 ComputeBridge 并行注入基准 — 串行 vs 并行吞吐对照
//!
//! 对应任务:P1-T14（WI-34 并行化首批注入,Phase 1 地基波次,v4.0 §7.5.1 L-a）
//! 架构层:L2 Memory（NMC 原生多模态上下文编码）
//!
//! # 口径
//! 直测注入热点 [`perceive_batch`](nmc_encoder::perceive_batch):
//! 批量感知编码（SHA256 + 256 桶字节频率 + 融合为 512 维 CLV）,纯 CPU,
//! 无 IO/await/锁 —— ComputeBridge 注入收益的真实体现（§7.5.1 L-a 预估 2-4×）。
//!
//! # 场景
//! 1_200 个文本输入（~4KB/个,固定种子确定性,超过 `TaskKind::ClvSimilarity`
//! 阈值 1_000 → ComputeBridge 路由到 Rayon）:
//! - 串行路径:`parallel_enabled = false`（回退语义,配置/env 关闭等价）
//! - 并行路径:`parallel_enabled = true` → `spawn_compute_batch`,
//!   CHUNK=64 分组,闭包捕获 `Arc<NmcEncoder>` + `Arc<Vec<PerceptionInput>>`
//!   + 索引范围,零输入复制;事件发布留主线程（IO 侧不上 rayon）
//!
//! # 门禁
//! 单 crate 1.5× 加速（对照 T1 基线,本机实测记录）。口径与 T8/T9 一致:
//! `iter_custom` 测量阶段零打印,测量结束后固定采样打印 P50/P99 + speedup;
//! 不达标时报告按 DONE_WITH_CONCERNS 记录实测值（基准不做断言,防采样抖动误报）。

#![forbid(unsafe_code)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use nmc_encoder::{perceive_batch, NmcConfig, NmcEncoder, PerceptionInput};

/// 输入数量 — 超过 ClvSimilarity 阈值(1_000)触发 Rayon 分支;
/// 1200 使任务波次充足（19 chunk / 池线程数,负载均衡良好）
const N_INPUTS: usize = 1_200;

/// 构造批量文本输入（固定确定性,~4KB/个放大 SHA256 + 字节频率计算成本）
fn make_inputs(n: usize) -> Vec<PerceptionInput> {
    let content = "Parallel batch encoding bench content with deterministic seed. ".repeat(64);
    (0..n)
        .map(|i| PerceptionInput::Text(format!("input-{i:04}:{content}")))
        .collect()
}

/// 单次批量编码耗时（注入热点直测）
fn run(
    encoder: &Arc<NmcEncoder>,
    inputs: &Arc<Vec<PerceptionInput>>,
    parallel: bool,
) -> Duration {
    let start = Instant::now();
    let out = perceive_batch(encoder, inputs, parallel);
    criterion::black_box(out);
    start.elapsed()
}

/// 取分位数（样本需已排序）
fn percentile(sorted: &[Duration], p: f64) -> Duration {
    assert!(
        !sorted.is_empty(),
        "percentile: 样本为空,无法计算 p={p} 分位数"
    );
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx]
}

fn bench_parallel_encoding(c: &mut Criterion) {
    let encoder = Arc::new(NmcEncoder::new(NmcConfig::default()).expect("编码器构造应成功"));
    let inputs = Arc::new(make_inputs(N_INPUTS));

    // 预热:rayon 池线程与内存缓存就绪后采样更接近稳态
    for _ in 0..4 {
        let _ = run(&encoder, &inputs, true);
        let _ = run(&encoder, &inputs, false);
    }

    let mut group = c.benchmark_group("nmc_parallel_encoding");
    group.sample_size(10);
    group.bench_function(BenchmarkId::from_parameter("serial_1200"), |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                total += run(&encoder, &inputs, false);
            }
            total
        });
    });
    group.bench_function(BenchmarkId::from_parameter("parallel_1200"), |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                total += run(&encoder, &inputs, true);
            }
            total
        });
    });
    group.finish();

    // 门禁采样（iter_custom 外,零干扰）
    const GATE_SAMPLES: usize = 50;
    let mut serial_samples = Vec::with_capacity(GATE_SAMPLES);
    for _ in 0..GATE_SAMPLES {
        serial_samples.push(run(&encoder, &inputs, false));
    }
    serial_samples.sort_unstable();
    let mut parallel_samples = Vec::with_capacity(GATE_SAMPLES);
    for _ in 0..GATE_SAMPLES {
        parallel_samples.push(run(&encoder, &inputs, true));
    }
    parallel_samples.sort_unstable();

    let s_p50 = percentile(&serial_samples, 0.50);
    let s_p99 = percentile(&serial_samples, 0.99);
    let p_p50 = percentile(&parallel_samples, 0.50);
    let p_p99 = percentile(&parallel_samples, 0.99);
    let speedup = s_p50.as_secs_f64() / p_p50.as_secs_f64();
    eprintln!(
        "[nmc_parallel_encoding] n_inputs={N_INPUTS} serial P50={:.2}ms P99={:.2}ms | parallel P50={:.2}ms P99={:.2}ms | speedup(P50)={:.2}× (门禁: 1.5×)",
        s_p50.as_secs_f64() * 1e3,
        s_p99.as_secs_f64() * 1e3,
        p_p50.as_secs_f64() * 1e3,
        p_p99.as_secs_f64() * 1e3,
        speedup,
    );
}

criterion_group!(benches, bench_parallel_encoding);
criterion_main!(benches);
