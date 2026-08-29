//! FaaE 批量专家评分 ComputeBridge 并行注入基准 — 串行 vs 并行吞吐对照
//!
//! 对应任务:P1-T14（WI-34 并行化首批注入,Phase 1 地基波次,v4.0 §7.5.5 W3-8 段）
//! 架构层:L6 Router
//!
//! # 双口径对照
//! - **热点层（主对照,与 v4.0 §7.5.1 预估对齐）**:直接测量注入热点
//!   [`score_experts_batch`](faae_router::parallel::score_experts_batch)
//!   —— 纯 CPU 批量评分（512 维余弦 × priority）,无 IO/await/锁。
//!   这是 ComputeBridge 注入收益的真实体现（§7.5.1 L-a 预估 3-6× 即此口径）。
//! - **端到端层（真实路径如实记录）**:`FaaeRouter::route` 全流程
//!   —— 含快照读锁收集（24000 次 async 读锁 + expert_vector 全量 clone,
//!   注入边界外的异步前置,禁入 rayon）等固定成本,结构性稀释注入收益,
//!   实测显著低于热点层（本机 1.37-1.42×,低于门禁 1.5×）,报告如实记录。
//!
//! # 场景
//! 24_000 个输入（固定种子确定性生成,超过 `TaskKind::Generic` 阈值
//! 10_000 → ComputeBridge 路由到 Rayon）:
//! - 串行路径:`parallel_enabled = false`（回退语义,配置/env 关闭等价）
//! - 并行路径:`parallel_enabled = true` → `spawn_compute_batch`,
//!   CHUNK=64 分组,闭包捕获 Arc 共享容器 + 索引范围,零输入复制
//!
//! 专家向量与 CLV 均为 **512 维**（对齐 router 文档口径:"clv 为上下文潜在向量
//! 512 维,内部截取前 64 维与 expert_vector 对齐"——64 维场景下评分仅 ~1.2ms,
//! 被 12000 次 tokio 快照读锁（~5ms 固定,两路径均摊）淹没,并行收益不可见;512 维
//! 使评分成本回到真实占比）。
//!
//! 规模说明（P1-T14 实测校准）:初版 64 维/12k 实测 0.76×（chunk.to_vec 全量
//! 输入复制 ~5ms 反超,已改为 Arc 共享零复制）→ 修复后 1.08×;512 维/12k 端到端
//! → 1.42×;512 维/24k 端到端 → 1.37×（快照收集随 N 线性,稀释加剧）;
//! 热点层 512 维/24k → 本档。
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
use faae_router::parallel::{score_experts_batch, ScoreInput};
use faae_router::{ExpertProfile, FaaeConfig, FaaeRouter, ToolId};
use tokio::runtime::Runtime;

/// 专家数量 — 超过 Generic 阈值(10_000)触发 Rayon 分支;24k 使任务波次充足,
/// 并行负载均衡更优（12k 时 188 任务/14 线程仅 13 波次,尾效应显著）
const N_EXPERTS: usize = 24_000;

/// 专家向量维度 — 对齐 router 文档口径的 CLV 512 维（评分成本占真实占比）
const VEC_DIM: usize = 512;

/// 构造专家画像（固定种子确定性,向量唯一以区分评分）
fn make_experts(n: usize) -> Vec<ExpertProfile> {
    (0..n)
        .map(|i| {
            let mut v = vec![0.0f32; VEC_DIM];
            v[i % VEC_DIM] = 1.0 + (i % 3) as f32 * 0.1;
            ExpertProfile::new(format!("tool-{i}"), v, vec![], 0.5 + (i % 10) as f32 / 10.0)
        })
        .collect()
}

/// 构造候选工具 ID 列表
fn make_candidates(n: usize) -> Vec<ToolId> {
    (0..n).map(|i| ToolId::new(format!("tool-{i}"))).collect()
}

/// 构造固定种子 CLV（512 维,与专家向量对齐,真实评分占比）
fn make_clv() -> Vec<f32> {
    (0..VEC_DIM)
        .map(|i| ((i * 13) % 1000) as f32 / 1000.0)
        .collect()
}

/// 构造热点层评分输入快照（固定种子确定性,与专家画像同分布）
fn make_score_inputs(n: usize) -> Vec<ScoreInput> {
    (0..n)
        .map(|i| {
            let mut v = vec![0.0f32; VEC_DIM];
            v[i % VEC_DIM] = 1.0 + (i % 3) as f32 * 0.1;
            ScoreInput {
                tool_id: ToolId::new(format!("tool-{i}")),
                expert_vector: v,
                priority: 0.5 + (i % 10) as f32 / 10.0,
            }
        })
        .collect()
}

/// 单次路由耗时（端到端:快照读锁 + 批量评分 + Top-K + usage 更新 + 事件发布）
fn run_route(rt: &Runtime, router: &FaaeRouter, clv: &[f32], candidates: &[ToolId]) -> Duration {
    let start = Instant::now();
    let out = rt.block_on(router.route(clv, candidates));
    criterion::black_box(out.expect("路由应成功"));
    start.elapsed()
}

/// 单次热点评分耗时（纯 CPU 批量评分,无 IO/await/锁）
fn run_hot(clv: &[f32], inputs: &Arc<Vec<ScoreInput>>, parallel: bool) -> Duration {
    let start = Instant::now();
    let out = score_experts_batch(clv, inputs, parallel);
    criterion::black_box(out.expect("批量评分应成功"));
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

fn bench_faae_scoring(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio runtime 构建");
    // ExpertProfile 不实现 Clone:为两个 router 各生成一份,按所有权移入注册
    let candidates = make_candidates(N_EXPERTS);
    let clv = make_clv();
    let inputs = Arc::new(make_score_inputs(N_EXPERTS));

    // ========== 热点层:直测评分核心（注入收益真实体现） ==========
    let mut hot_group = c.benchmark_group("faae_hot_scoring");
    hot_group.sample_size(10);
    hot_group.bench_function(BenchmarkId::from_parameter("serial_24k"), |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                total += run_hot(&clv, &inputs, false);
            }
            total
        });
    });
    hot_group.bench_function(BenchmarkId::from_parameter("parallel_24k"), |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                total += run_hot(&clv, &inputs, true);
            }
            total
        });
    });
    hot_group.finish();

    // 热点层门禁采样（iter_custom 外,零干扰）
    const GATE_SAMPLES: usize = 50;
    let mut hot_serial = Vec::with_capacity(GATE_SAMPLES);
    for _ in 0..GATE_SAMPLES {
        hot_serial.push(run_hot(&clv, &inputs, false));
    }
    hot_serial.sort_unstable();
    let mut hot_parallel = Vec::with_capacity(GATE_SAMPLES);
    for _ in 0..GATE_SAMPLES {
        hot_parallel.push(run_hot(&clv, &inputs, true));
    }
    hot_parallel.sort_unstable();

    let hs_p50 = percentile(&hot_serial, 0.50);
    let hs_p99 = percentile(&hot_serial, 0.99);
    let hp_p50 = percentile(&hot_parallel, 0.50);
    let hp_p99 = percentile(&hot_parallel, 0.99);
    let hot_speedup = hs_p50.as_secs_f64() / hp_p50.as_secs_f64();
    eprintln!(
        "[faae_hot_scoring] n_experts={N_EXPERTS} serial P50={:.2}ms P99={:.2}ms | parallel P50={:.2}ms P99={:.2}ms | speedup(P50)={:.2}× (门禁: 1.5×)",
        hs_p50.as_secs_f64() * 1e3,
        hs_p99.as_secs_f64() * 1e3,
        hp_p50.as_secs_f64() * 1e3,
        hp_p99.as_secs_f64() * 1e3,
        hot_speedup,
    );

    // ========== 端到端层:route() 真实路径（含注入边界外前置,如实记录） ==========
    let router_serial = FaaeRouter::with_config(
        EventBus::new(),
        FaaeConfig::default()
            .with_balance_enabled(false)
            .with_parallel_expert_scoring(false),
    );
    let router_parallel = FaaeRouter::with_config(
        EventBus::new(),
        FaaeConfig::default().with_balance_enabled(false),
    );
    for p in make_experts(N_EXPERTS) {
        rt.block_on(router_serial.register_expert(p));
    }
    for p in make_experts(N_EXPERTS) {
        rt.block_on(router_parallel.register_expert(p));
    }

    // 预热:rayon 池线程与内存缓存就绪后采样更接近稳态
    for _ in 0..4 {
        let _ = run_route(&rt, &router_parallel, &clv, &candidates);
        let _ = run_route(&rt, &router_serial, &clv, &candidates);
    }

    let mut group = c.benchmark_group("faae_scoring");
    group.sample_size(10);
    group.bench_function(BenchmarkId::from_parameter("serial_24k"), |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                total += run_route(&rt, &router_serial, &clv, &candidates);
            }
            total
        });
    });
    group.bench_function(BenchmarkId::from_parameter("parallel_24k"), |b| {
        b.iter_custom(|iters| {
            let mut total = Duration::ZERO;
            for _ in 0..iters {
                total += run_route(&rt, &router_parallel, &clv, &candidates);
            }
            total
        });
    });
    group.finish();

    // 端到端门禁采样（如实记录,不参与门禁判定——口径见模块 doc）
    let mut serial_samples = Vec::with_capacity(GATE_SAMPLES);
    for _ in 0..GATE_SAMPLES {
        serial_samples.push(run_route(&rt, &router_serial, &clv, &candidates));
    }
    serial_samples.sort_unstable();
    let mut parallel_samples = Vec::with_capacity(GATE_SAMPLES);
    for _ in 0..GATE_SAMPLES {
        parallel_samples.push(run_route(&rt, &router_parallel, &clv, &candidates));
    }
    parallel_samples.sort_unstable();

    let s_p50 = percentile(&serial_samples, 0.50);
    let s_p99 = percentile(&serial_samples, 0.99);
    let p_p50 = percentile(&parallel_samples, 0.50);
    let p_p99 = percentile(&parallel_samples, 0.99);
    let speedup = s_p50.as_secs_f64() / p_p50.as_secs_f64();
    eprintln!(
        "[faae_scoring] n_experts={N_EXPERTS} serial P50={:.2}ms P99={:.2}ms | parallel P50={:.2}ms P99={:.2}ms | speedup(P50)={:.2}× (端到端,含快照读锁等前置;注入收益见热点层)",
        s_p50.as_secs_f64() * 1e3,
        s_p99.as_secs_f64() * 1e3,
        p_p50.as_secs_f64() * 1e3,
        p_p99.as_secs_f64() * 1e3,
        speedup,
    );
}

criterion_group!(benches, bench_faae_scoring);
criterion_main!(benches);
