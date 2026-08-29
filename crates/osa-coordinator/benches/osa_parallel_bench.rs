//! OSA 批量五维掩码 ComputeBridge 并行注入基准 — 串行 vs 并行吞吐对照
//!
//! 对应任务:P1-T14（WI-34 并行化首批注入,Phase 1 地基波次,v4.0 §7.5.5 W6-7）
//! 架构层:L6 Router
//!
//! # 场景
//! 240 个 TaskProfile（固定种子确定性生成 ≈ 1200 个五维掩码任务,超过
//! `TaskKind::OsaMask` 阈值 100 → ComputeBridge 路由到 Rayon）:
//! - 串行路径:`OsaConfig::with_parallel_masks(false)`（回退语义,与注入前一致）
//! - 并行路径:默认配置（`parallel_masks = true` → `spawn_compute_batch` 批量并行,
//!   每任务 = 一个 profile 的完整五维,闭包捕获 Arc 共享容器零 clone）
//!
//! # 规模说明（P1-T14 实测校准）
//! 初版 60 profiles 实测 0.65×（微任务 + profile clone 反超）;修复后 1.17×。
//! 端到端测量含固定串行成本（输入 to_vec 复制 / validate / hash / 事件发布 / 日志,
//! 两路径均摊）——大 N 时任务数多、并行利用率高,并行收益显著;
//! 240 profiles 为批量场景的代表规模（真实系统一次可聚合数百任务）。
//!
//! # 门禁
//! 单 crate 1.5× 加速（对照 T1 基线,本机实测记录）。口径与 T8/T9 一致:
//! `iter_custom` 测量阶段零打印,测量结束后固定采样打印 P50/P99 + speedup;
//! 不达标时报告按 DONE_WITH_CONCERNS 记录实测值（基准不做断言,防采样抖动误报）。

#![forbid(unsafe_code)]

use std::sync::Arc;
use std::time::{Duration, Instant};

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use event_bus::EventBus;
use osa_coordinator::{
    AffectedScope, FileId, MemoryId, OsaConfig, OmniSparseCoordinator, OperationId, RiskLevel,
    TaskId, TaskProfile, TaskType, TimePressure, ToolId,
};
use tokio::runtime::Runtime;

/// 构造一批固定种子 TaskProfile（LCG 伪随机但确定性,与 parallel.rs 测试同源）
fn make_profiles(n: usize) -> Vec<TaskProfile> {
    (0..n)
        .map(|i| {
            let seed = i as u64;
            // 固定种子 LCG — 伪随机但确定性,不引入 rand 依赖
            let mut state = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let mut next = move || {
                state = state
                    .wrapping_mul(6364136223846793005)
                    .wrapping_add(1442695040888963407);
                (state >> 33) as usize
            };
            let tool_n = 8 + next() % 50;
            let file_n = 50 + next() % 2000;
            let mem_n = 8 + next() % 50;
            let op_n = 20 + next() % 120;
            let task_n = 4 + next() % 12;
            let complexity = ((i * 37 + 13) % 1000) as f32 / 1000.0;
            let risk = match i % 4 {
                0 => RiskLevel::Low,
                1 => RiskLevel::Medium,
                2 => RiskLevel::High,
                _ => RiskLevel::Critical,
            };
            TaskProfile {
                task_id: format!("t-{seed}").into(),
                task_type: TaskType::Read,
                complexity_score: complexity,
                risk_level: risk,
                time_pressure: TimePressure::Low,
                affected_scope: AffectedScope::Local,
                available_tools: (0..tool_n).map(|i| ToolId::new(format!("tool-{i}"))).collect(),
                available_files: (0..file_n).map(|i| FileId::new(format!("file-{i}"))).collect(),
                available_memories: (0..mem_n).map(|i| MemoryId::new(format!("mem-{i}"))).collect(),
                recent_operations: (0..op_n).map(|i| OperationId::new(format!("op-{i}"))).collect(),
                active_tasks: (0..task_n).map(|i| TaskId::new(format!("task-{i}"))).collect(),
                routing_scores: None,
                context_scores: None,
                memory_scores: None,
                task_phase: None,
            }
        })
        .collect()
}

/// 单次批量计算耗时（含事件发布/快照,与真实端到端一致）
///
/// `profiles` 以 Arc 共享传入,每次迭代 `Arc::clone`（原子增计数,零输入复制）
/// —— 消除批量输入 to_vec 的测量伪影（注入后 API 即 Arc 语义,调用方复用容器）。
fn run_batch(
    rt: &Runtime,
    coord: &OmniSparseCoordinator,
    profiles: &Arc<Vec<TaskProfile>>,
) -> Duration {
    let start = Instant::now();
    let out = rt.block_on(coord.compute_all_masks_batch(Arc::clone(profiles)));
    criterion::black_box(out.expect("批量掩码计算应成功"));
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

fn bench_osa_masks_batch(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime 构建");
    let profiles = Arc::new(make_profiles(240));
    let coord_serial = OmniSparseCoordinator::with_config(
        EventBus::new(),
        OsaConfig::default().with_parallel_masks(false),
    );
    let coord_parallel = OmniSparseCoordinator::new(EventBus::new());

    // 预热:rayon 池线程与内存缓存就绪后采样更接近稳态
    for _ in 0..3 {
        let _ = run_batch(&rt, &coord_parallel, &profiles);
        let _ = run_batch(&rt, &coord_serial, &profiles);
    }

    let mut group = c.benchmark_group("osa_masks_batch");
    group.sample_size(10);
    group.bench_function(BenchmarkId::from_parameter("serial_240p"), |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                total += run_batch(&rt, &coord_serial, &profiles);
            }
            total
        });
    });
    group.bench_function(BenchmarkId::from_parameter("parallel_240p"), |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                total += run_batch(&rt, &coord_parallel, &profiles);
            }
            total
        });
    });
    group.finish();

    // 门禁采样:固定次数,仅打印一次 P50/P99 + speedup（不在 iter_custom 内,防重复输出）
    const GATE_SAMPLES: usize = 100;
    let mut serial_samples = Vec::with_capacity(GATE_SAMPLES);
    for _ in 0..GATE_SAMPLES {
        serial_samples.push(run_batch(&rt, &coord_serial, &profiles));
    }
    serial_samples.sort_unstable();
    let mut parallel_samples = Vec::with_capacity(GATE_SAMPLES);
    for _ in 0..GATE_SAMPLES {
        parallel_samples.push(run_batch(&rt, &coord_parallel, &profiles));
    }
    parallel_samples.sort_unstable();

    let s_p50 = percentile(&serial_samples, 0.50);
    let s_p99 = percentile(&serial_samples, 0.99);
    let p_p50 = percentile(&parallel_samples, 0.50);
    let p_p99 = percentile(&parallel_samples, 0.99);
    let speedup = s_p50.as_secs_f64() / p_p50.as_secs_f64();
    eprintln!(
        "[osa_masks_batch] n_profiles=240 serial P50={:.2}ms P99={:.2}ms | parallel P50={:.2}ms P99={:.2}ms | speedup(P50)={:.2}× (门禁: 1.5×)",
        s_p50.as_secs_f64() * 1e3,
        s_p99.as_secs_f64() * 1e3,
        p_p50.as_secs_f64() * 1e3,
        p_p99.as_secs_f64() * 1e3,
        speedup,
    );
}

criterion_group!(benches, bench_osa_masks_batch);
criterion_main!(benches);
