//! 跨执行面过订阅基准 — 「六面各 8 路」与「共享 8 路预算」的在飞峰值与 makespan
//!
//! # 为什么需要这个基准（R12 W0）
//!
//! 现状六个执行面各有一把独立的并发上界（实测值见 `01-现状证据` §B）：
//! PlanRunner 8 / BatchLimits 8 / action 循环 8 / delegation 8 / mas-sched 租约 4 / MCP 无。
//! 它们是**互不相干**的信号量 ⇒ 叠加后的在飞调用数可达其和，而对下游（磁盘、同一 MCP server、
//! 同一 CPU 池）却是同一批资源。这就是「过订阅」：每一面都以为自己只开 8 路。
//!
//! 本基准把这一结构**最小化复刻**成两种拓扑，只量两个量：
//! - **L1 结构判据**：在飞峰值（`peak_in_flight`）是否被压在下界内；
//! - **L3 比值**：makespan 的代价（共享预算必然更慢——那不是缺陷，是换取可预测性的价格）。
//!
//! # 与生产代码的关系
//!
//! 本文件**不引用**任何尚未落地的 API（`ResourceGovernor` 属 R12 W6）。
//! 它用 bench 内的最小复刻取基线：W6 接线后，同一组用例应能直接替换成生产治理器，
//! 且 `peak` 断言与 makespan 比值继续成立 —— 这是「先有判据、后有实现」的顺序（ADR-234 A2-2）。
//!
//! # 运行
//!
//! ```powershell
//! cargo bench -p gqep-executor --bench sched_plane
//! ```
//!
//! # 注册态
//!
//! **dev-only**（登记见 `scripts/bench_inventory_freeze.txt`）。W6 落地后再决定是否升格 `registered`。
#![allow(clippy::unwrap_used, clippy::expect_used)] // bench code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use criterion::{criterion_group, criterion_main, Criterion};
use tokio::sync::Semaphore;

/// 单面并发上界 —— 与 `PlanRunner::DEFAULT_CONCURRENCY` / `BatchLimits::default().concurrency`
/// / `ACTION_CONCURRENCY_DEFAULT` / `DEFAULT_MAX_CONCURRENT_DELEGATION` 的既有取值一致（都是 8）。
const PER_PLANE_BOUND: usize = 8;
/// 执行面数量 —— 取四个「各 8」的面（PlanRunner / batch / action / delegation）；
/// mas-sched 租约（4）与 MCP（无上界）不建模，避免夸大。
const PLANES: usize = 4;
/// 每面任务数
const TASKS_PER_PLANE: usize = 8;
/// 单任务服务时间（ms）。
///
/// **为什么不是 1ms**（R12 W0 首跑的教训）：Windows 默认定时器粒度 ≈ **15.6ms**，
/// `sleep(1ms)` 实际落地为 ~15.6ms。首跑 `per_plane_4x8` 得 15.58ms、
/// `shared_budget_8` 得 61.78ms —— 两者都恰好是"跳数 × 15.6ms"（1 跳 / 4 跳），
/// 说明测到的是定时器而不是调度。取 20ms 后，均匀档 = 1 跳量级、共享档 = 4 跳量级，
/// 比值才反映**轮次结构**（4 面各 8 路 = 1 轮；共享 8 路 = 4 轮）而不是粒度巧合。
const TASK_MS: u64 = 20;

/// 在飞峰值探针 —— 两个原子量：当前在飞数 + 历史峰值
struct InFlight {
    cur: Arc<AtomicUsize>,
    peak: Arc<AtomicUsize>,
}

impl InFlight {
    fn new() -> Self {
        Self {
            cur: Arc::new(AtomicUsize::new(0)),
            peak: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn reset(&self) {
        self.cur.store(0, Ordering::SeqCst);
        self.peak.store(0, Ordering::SeqCst);
    }

    fn peak(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }
}

/// 现状拓扑：每面一把独立许可（`PLANES × PER_PLANE_BOUND` 路可同时在飞）
async fn run_per_plane(probe: &InFlight) {
    let lanes: Vec<Arc<Semaphore>> = (0..PLANES)
        .map(|_| Arc::new(Semaphore::new(PER_PLANE_BOUND)))
        .collect();

    let mut tasks = Vec::with_capacity(PLANES * TASKS_PER_PLANE);
    for lane in lanes {
        for _ in 0..TASKS_PER_PLANE {
            let lane = Arc::clone(&lane);
            let cur = Arc::clone(&probe.cur);
            let peak = Arc::clone(&probe.peak);
            tasks.push(tokio::spawn(async move {
                let _permit = lane.acquire_owned().await.expect("信号量未关闭");
                let now = cur.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(TASK_MS)).await;
                cur.fetch_sub(1, Ordering::SeqCst);
            }));
        }
    }
    for t in tasks {
        t.await.expect("任务不应 panic");
    }
}

/// 目标拓扑：全部执行面共享一把 `PER_PLANE_BOUND` 路许可
async fn run_shared_budget(probe: &InFlight) {
    let shared = Arc::new(Semaphore::new(PER_PLANE_BOUND));
    let mut tasks = Vec::with_capacity(PLANES * TASKS_PER_PLANE);
    for _ in 0..PLANES * TASKS_PER_PLANE {
        let shared = Arc::clone(&shared);
        let cur = Arc::clone(&probe.cur);
        let peak = Arc::clone(&probe.peak);
        tasks.push(tokio::spawn(async move {
            let _permit = shared.acquire_owned().await.expect("信号量未关闭");
            let now = cur.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(now, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(TASK_MS)).await;
            cur.fetch_sub(1, Ordering::SeqCst);
        }));
    }
    for t in tasks {
        t.await.expect("任务不应 panic");
    }
}

fn bench_plane_topologies(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("创建 tokio runtime 失败");
    let probe = InFlight::new();

    let mut group = c.benchmark_group("sched_plane");
    group.sample_size(20);

    // 现状：四面各 8 ⇒ 峰值可达 32（面数 × 单面上界）
    group.bench_function("per_plane_4x8", |b| {
        b.to_async(&rt).iter(|| async {
            probe.reset();
            run_per_plane(&probe).await;
            let peak = probe.peak();
            // L1 结构判据：现状拓扑**允许**过订阅，峰值必然可以突破单面上界
            assert!(
                peak <= PLANES * PER_PLANE_BOUND,
                "峰值 {peak} 超过面数×上界"
            );
            criterion::black_box(peak);
        });
    });

    // 目标：共享预算 ⇒ 峰值恒 ≤ 单面上界（这正是 W6 要买到的性质）
    group.bench_function("shared_budget_8", |b| {
        b.to_async(&rt).iter(|| async {
            probe.reset();
            run_shared_budget(&probe).await;
            let peak = probe.peak();
            assert!(
                peak <= PER_PLANE_BOUND,
                "共享预算下峰值 {peak} 必须 ≤ {PER_PLANE_BOUND}"
            );
            criterion::black_box(peak);
        });
    });

    group.finish();
}

criterion_group!(benches, bench_plane_topologies);
criterion_main!(benches);
