//! DecayEngine 大规模批量衰减基准 — P1-T14 并行注入候选工作量(手册 §7.5.1 / v4.0 §7.5.2)
//!
//! 对应任务:P1-T14(WI-34 并行化第 2 批,v4.0 §7.5.1 L-a)
//! 对应架构层:L4 Security
//!
//! # 重建说明
//! ⚠️ 本文件于 **2026-08-28 磁盘 ENOSPC 事故中数据丢失**,现基于源码
//! (`parallel.rs` / `engine.rs`) 公共 API 做功能重建。
//!
//! # 背景
//! `parallel.rs` 的批量衰减注入器对能力注册表内全部能力应用衰减:核心语义为
//! **快照分离、计算并行、串行提交**。其在 `DecayEngine::parallel_batch`
//! 内经 `bridge().route(TaskKind::Generic, n)` 判派发 —— 以 **Generic 阈值
//! 10,000** 为界:n < 阈值走 Inline 串行,n ≥ 阈值经 ComputeBridge Rayon 并行。
//!
//! # 基准场景(直接度量并行注入器)
//! - `parallel_batch/{n}`:跨 Generic 阈值两侧(9_000 < 阈值 / 12_000 ≥ 阈值)
//!   批量 `TimeDecay` 的 [`DecayEngine::parallel_batch`] 吞吐 —— 同一工作负载,
//!   规模决定其触发 Inline / Rayon 派发,延迟 / 吞吐差异即并行注入决策的直接观察。
//!
//! > 语法结构参照同目录 `decay_bench.rs` / `decay_compute.rs`。

#![forbid(unsafe_code)]

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use decay_engine::{DecayConfig, DecayEngine, DecayEvent};

/// 批量规模梯度 — 分跨 Generic 阈值 10,000(并行注入的 Inline/Rayon 分界)
const SIZES: &[usize] = &[9_000, 12_000];

/// `DecayEngine::parallel_batch` 批量并行衰减吞吐(直接度量并行注入器)
///
/// WHY 跨阈值测量:9_000 < Generic 阈值 10,000(注入器判 Inline 串行),
/// 12_000 ≥ 阈值(注入器判 Rayon 并行)。经 `parallel_batch` 度量同一工作负载
/// 在不同规模下的吞吐,反映并行派发决策对大规模批量衰减的实际影响。
fn decay_parallel_batch(c: &mut Criterion) {
    let mut group = c.benchmark_group("parallel_batch");
    for &size in SIZES {
        group.throughput(Throughput::Elements(size as u64));

        // 预填充能力注册表(setup,不计入测量)
        let engine = DecayEngine::new(DecayConfig::default());
        for i in 0..size {
            engine
                .register_capability(&format!("cap-{i}"), "bench capability", 0.8)
                .expect("register_capability 失败");
        }
        // 预构建 items(模拟周期性全量衰减),避免 iter 中 format 干扰测量
        let ids: Vec<String> = (0..size).map(|i| format!("cap-{i}")).collect();
        let items: Vec<(&str, DecayEvent)> = ids
            .iter()
            .map(|id| (id.as_str(), DecayEvent::TimeDecay))
            .collect();

        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &_| {
            b.iter(|| {
                let out = engine
                    .parallel_batch(black_box(&items), true)
                    .expect("parallel_batch 失败");
                black_box(out);
            });
        });
    }
    group.finish();
}

criterion_group!(benches, decay_parallel_batch);
criterion_main!(benches);
