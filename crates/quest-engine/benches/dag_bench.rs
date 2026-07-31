//! DAG 校验/拓扑排序基准 — L9 优化 2.1(TDD:先证伪 O(V²·D) 再优化)
//!
//! 三种拓扑形态 × 三档规模(100/1000/5000 节点):
//! - **线性链**:a→b→c→…(最深依赖链,Kahn 每轮只解锁 1 个节点)
//! - **菱形层**:每层 10 个节点,层间全连接(边数 = V/10 × 10 × 10)
//! - **宽扇出**:根节点被其余全部节点依赖(最宽单层)
//!
//! 基线用途:优化前记录 O(V²·D) 基线,优化后接入
//! `.github/workflows/bench_check.yml` 阈值断言(1000 节点 validate < 1ms)。

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use nexus_core::{Task, TaskStatus};
use quest_engine::dag::{topological_order, validate_dag};

fn make_task(id: String, deps: Vec<String>) -> Task {
    Task {
        task_id: id,
        description: String::new(),
        status: TaskStatus::Pending,
        dependencies: deps,
    }
}

/// 线性链:t0 ← t1 ← t2 ← …
fn linear_chain(n: usize) -> Vec<Task> {
    (0..n)
        .map(|i| {
            let deps = if i == 0 {
                vec![]
            } else {
                vec![format!("t{}", i - 1)]
            };
            make_task(format!("t{i}"), deps)
        })
        .collect()
}

/// 菱形层:每层 10 节点,依赖上一层全部 10 节点
fn diamond_layers(n: usize) -> Vec<Task> {
    const WIDTH: usize = 10;
    (0..n)
        .map(|i| {
            let layer = i / WIDTH;
            let deps = if layer == 0 {
                vec![]
            } else {
                let prev_start = (layer - 1) * WIDTH;
                (prev_start..prev_start + WIDTH)
                    .filter(|&j| j < n)
                    .map(|j| format!("t{j}"))
                    .collect()
            };
            make_task(format!("t{i}"), deps)
        })
        .collect()
}

/// 宽扇出:t0 为根,其余全部依赖 t0
fn wide_fanout(n: usize) -> Vec<Task> {
    (0..n)
        .map(|i| {
            let deps = if i == 0 {
                vec![]
            } else {
                vec!["t0".to_string()]
            };
            make_task(format!("t{i}"), deps)
        })
        .collect()
}

fn bench_validate_dag(c: &mut Criterion) {
    let mut group = c.benchmark_group("dag/validate");
    for &n in &[100usize, 1000, 5000] {
        for (shape, tasks) in [
            ("linear", linear_chain(n)),
            ("diamond", diamond_layers(n)),
            ("fanout", wide_fanout(n)),
        ] {
            group.bench_with_input(BenchmarkId::new(shape, n), &tasks, |b, tasks| {
                b.iter(|| validate_dag(black_box(tasks)))
            });
        }
    }
    group.finish();
}

fn bench_topological_order(c: &mut Criterion) {
    let mut group = c.benchmark_group("dag/topo_order");
    // 拓扑排序只测 1000 节点档(validate 已覆盖复杂度形态,此处守护端到端排序路径)
    for (shape, tasks) in [
        ("linear", linear_chain(1000)),
        ("diamond", diamond_layers(1000)),
        ("fanout", wide_fanout(1000)),
    ] {
        group.bench_with_input(BenchmarkId::new(shape, 1000), &tasks, |b, tasks| {
            b.iter(|| topological_order(black_box(tasks)))
        });
    }
    group.finish();
}

criterion_group!(benches, bench_validate_dag, bench_topological_order);
criterion_main!(benches);
