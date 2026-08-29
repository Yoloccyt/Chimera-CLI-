//! HCW 压缩评分 ComputeBridge 并行注入基准 — 串行 vs 并行吞吐对照
//!
//! 对应任务:P1-T14（WI-34 并行化首批注入,Phase 1 地基波次,v4.0 §7.5.1 L-a）
//! 架构层:L2 Memory（HCW 四层级窗口选择 / 压缩段间）
//!
//! # 口径
//! 直测注入热点 [`ContextCompressor::compress`](hcw_window::ContextCompressor::compress)
//! 全流程 —— 统计量 → 评分（0.4 时近性 + 0.3 频次 + 0.3 CLV 余弦相关性）→
//! Top-K 选择 → 贪心保留,纯 CPU,无 IO/await/锁。评分阶段经
//! [`score_entries`](hcw_window::parallel::score_entries) 段间并行（段内保序）,
//! Top-K/贪心保留串行,结果与注入前逐位一致。
//!
//! # 场景
//! 8_000 个条目（每 3 条带 512 维 CLV + 固定任务 CLV → 含余弦相关性成本,
//! 固定种子确定性,超过 `TaskKind::CscCollapseScore` 阈值 200 → Rayon 分支）:
//! - 串行路径:`config.with_parallel_compress(false)`（回退语义）
//! - 并行路径:`HcwConfig::default()`（`parallel_compress = true`）
//!
//! # 门禁
//! 单 crate 1.5× 加速（对照 T1 基线,本机实测记录）。口径与 T8/T9 一致:
//! `iter_custom` 测量阶段零打印,测量结束后固定采样打印 P50/P99 + speedup;
//! 不达标时报告按 DONE_WITH_CONCERNS 记录实测值（基准不做断言,防采样抖动误报）。

#![forbid(unsafe_code)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::Utc;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use hcw_window::{ContextCompressor, ContextEntry, HcwConfig};
use nexus_core::CLV;

/// 条目数量 — 超过 CscCollapseScore 阈值(200)触发 Rayon 分支;
/// 8000 使评分成本显著（>2666 次 512 维余弦 + 8000 次 recency/frequency）
const N_ENTRIES: usize = 8_000;

/// 压缩目标(Token 数,远小于原始 → 触发完整 Top-K + 贪心保留流程)
const TARGET_TOKEN: usize = 2_000;

/// 构造压缩条目（固定确定性,每 3 条带 512 维 CLV 含余弦相关性路径）
fn make_entries(n: usize) -> Vec<Arc<ContextEntry>> {
    (0..n)
        .map(|i| {
            let mut entry =
                ContextEntry::new(format!("e-{i}"), "file-1", format!("content-{i}"), 1000);
            entry.access_count = (i % 7) as u32;
            entry.last_accessed_at = Utc::now() - chrono::Duration::milliseconds(i as i64 * 13);
            if i % 3 == 0 {
                let v: Vec<f32> = (0..512)
                    .map(|j| ((i * 31 + j * 7) % 1000) as f32 / 1000.0)
                    .collect();
                entry.clv = Some(CLV::from_vec(v).expect("512 维"));
            }
            Arc::new(entry)
        })
        .collect()
}

/// 固定任务 CLV（非 None,走余弦相似度相关性路径,放大评分成本）
fn make_task_clv() -> CLV {
    let v: Vec<f32> = (0..512)
        .map(|j| ((j * 11) % 1000) as f32 / 1000.0)
        .collect();
    CLV::from_vec(v).expect("512 维")
}

/// 单次压缩耗时（注入热点直测,compress 只读 entries 不消费）
fn run(
    config: &HcwConfig,
    entries: &[Arc<ContextEntry>],
    task_clv: &CLV,
    now: chrono::DateTime<Utc>,
) -> Duration {
    let start = Instant::now();
    let report = ContextCompressor::compress(config, entries, TARGET_TOKEN, Some(task_clv), now);
    criterion::black_box(report);
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

fn bench_parallel_compress(c: &mut Criterion) {
    let entries = make_entries(N_ENTRIES);
    let task_clv = make_task_clv();
    let now = Utc::now();
    let config_serial = HcwConfig::default().with_parallel_compress(false);
    let config_parallel = HcwConfig::default();

    // 预热:rayon 池线程与内存缓存就绪后采样更接近稳态
    for _ in 0..4 {
        let _ = run(&config_parallel, &entries, &task_clv, now);
        let _ = run(&config_serial, &entries, &task_clv, now);
    }

    let mut group = c.benchmark_group("hcw_parallel_compress");
    group.sample_size(10);
    group.bench_function(BenchmarkId::from_parameter("serial_8k"), |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                total += run(&config_serial, &entries, &task_clv, now);
            }
            total
        });
    });
    group.bench_function(BenchmarkId::from_parameter("parallel_8k"), |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                total += run(&config_parallel, &entries, &task_clv, now);
            }
            total
        });
    });
    group.finish();

    // 门禁采样（iter_custom 外,零干扰）
    const GATE_SAMPLES: usize = 50;
    let mut serial_samples = Vec::with_capacity(GATE_SAMPLES);
    for _ in 0..GATE_SAMPLES {
        serial_samples.push(run(&config_serial, &entries, &task_clv, now));
    }
    serial_samples.sort_unstable();
    let mut parallel_samples = Vec::with_capacity(GATE_SAMPLES);
    for _ in 0..GATE_SAMPLES {
        parallel_samples.push(run(&config_parallel, &entries, &task_clv, now));
    }
    parallel_samples.sort_unstable();

    let s_p50 = percentile(&serial_samples, 0.50);
    let s_p99 = percentile(&serial_samples, 0.99);
    let p_p50 = percentile(&parallel_samples, 0.50);
    let p_p99 = percentile(&parallel_samples, 0.99);
    let speedup = s_p50.as_secs_f64() / p_p50.as_secs_f64();
    eprintln!(
        "[hcw_parallel_compress] n_entries={N_ENTRIES} serial P50={:.2}ms P99={:.2}ms | parallel P50={:.2}ms P99={:.2}ms | speedup(P50)={:.2}× (门禁: 1.5×)",
        s_p50.as_secs_f64() * 1e3,
        s_p99.as_secs_f64() * 1e3,
        p_p50.as_secs_f64() * 1e3,
        p_p99.as_secs_f64() * 1e3,
        speedup,
    );
}

criterion_group!(benches, bench_parallel_compress);
criterion_main!(benches);
