//! DelegationExecutor 委托编排开销基准 — criterion 基准测试(M0-T0.2)
//!
//! # 口径说明
//! 注入固定人工延迟(1ms)的 TaskRunner 模拟真实子 Agent 执行耗时,
//! 测量 `execute_delegation` 的**调度 + 超时包装 + 事件发布 + 汇聚**
//! 编排开销随 fan-out(1/4/16 子任务)的扩展性。
//!
//! 作为 M1 埋点(批次 wall-clock 计时 + DelegationCompleted 发布)
//! 开销回归的对照基线(性能可证伪铁律)。

use chimera_mas::delegation::{DelegationExecutor, TaskRunner};
use chimera_mas::prelude::*;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use event_bus::EventBus;
use nexus_core::{Task, TaskStatus};
use std::sync::Arc;
use std::time::Duration;

/// 构造测试用 AgentTask(关联 quest,模拟生产归因路径)
fn make_task(task_id: &str) -> AgentTask {
    let inner = Task {
        task_id: task_id.into(),
        description: format!("bench task {task_id}"),
        status: TaskStatus::Pending,
        dependencies: vec![],
    };
    AgentTask::new(
        inner,
        TaskComplexity::Simple,
        100,
        Duration::from_secs(10),
        QualityLevel::Standard,
    )
    .with_quest("q-bench")
}

/// 固定 1ms 延迟 runner — 模拟真实子 Agent 执行耗时
///
/// WHY 注入人工延迟:默认 runner 立即返回,基准只会测到 spawn 开销;
/// 1ms 延迟使并行调度收益(wall-clock ≈ 单任务延迟而非求和)可观测。
fn delay_runner() -> TaskRunner {
    Arc::new(|task: AgentTask| {
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(1)).await;
            Ok(format!("done-{}", task.inner.task_id))
        })
    })
}

/// 零延迟 runner — 立即返回,暴露纯编排开销
///
/// WHY(M0-T0.2 第二轮):1ms delay runner 掩盖了编排开销(59µs 级编排
/// vs 1ms sleep)。零延迟使基准测出 spawn + clone runner/bus + 事件发布
/// 的纯编排成本,作为 M4 去重复/fan-out=1 免 spawn 优化的对照基线。
fn zero_delay_runner() -> TaskRunner {
    Arc::new(|task: AgentTask| Box::pin(async move { Ok(format!("done-{}", task.inner.task_id)) }))
}

/// 基准:委托 fan-out 扩展性(1/4/16/64 子任务,1ms 延迟)
fn bench_delegation_fanout(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut group = c.benchmark_group("delegation_fanout");

    for &fanout in &[1usize, 4, 16, 64] {
        group.bench_with_input(
            BenchmarkId::from_parameter(fanout),
            &fanout,
            |b, &fanout| {
                let executor = DelegationExecutor::with_runner(
                    EventBus::new(),
                    Duration::from_secs(10),
                    delay_runner(),
                );
                b.iter(|| {
                    let tasks: Vec<AgentTask> =
                        (0..fanout).map(|i| make_task(&format!("t-{i}"))).collect();
                    rt.block_on(executor.execute_delegation("bench-parent", tasks))
                        .unwrap()
                });
            },
        );
    }
    group.finish();
}

/// 基准:委托纯编排开销(零延迟 runner,1/4/16/64 扩展性)
///
/// 剔除子任务执行耗时后的 spawn/clone/事件发布编排开销,
/// 暴露 fan-out 增长时的 spawn 线性成本(M4 去重复与未来小 fan-out
/// 免 spawn 优化的回归对照)。
fn bench_delegation_fanout_zero_delay(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut group = c.benchmark_group("delegation_fanout_zero_delay");

    for &fanout in &[1usize, 4, 16, 64] {
        group.bench_with_input(
            BenchmarkId::from_parameter(fanout),
            &fanout,
            |b, &fanout| {
                let executor = DelegationExecutor::with_runner(
                    EventBus::new(),
                    Duration::from_secs(10),
                    zero_delay_runner(),
                );
                b.iter(|| {
                    let tasks: Vec<AgentTask> =
                        (0..fanout).map(|i| make_task(&format!("t-{i}"))).collect();
                    rt.block_on(executor.execute_delegation("bench-parent", tasks))
                        .unwrap()
                });
            },
        );
    }
    group.finish();
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(50)
        .warm_up_time(Duration::from_millis(500));
    targets = bench_delegation_fanout, bench_delegation_fanout_zero_delay
}

criterion_main!(benches);
