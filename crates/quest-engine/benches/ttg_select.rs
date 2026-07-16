//! TTG 模式选择性能基准 — 验证 select_mode 延迟 < 1ms
//!
//! 对应 SubTask 35.6
//!
//! # 运行方式
//! ```powershell
//! cargo bench -p quest-engine --bench ttg_select --jobs 1
//! ```

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use decb_governor::BudgetTier;
use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};
use quest_engine::{TtgConfig, TtgGovernor};

/// 构造测试用 Quest
fn make_quest(task_count: usize) -> Quest {
    let tasks: Vec<Task> = (0..task_count)
        .map(|idx| Task {
            task_id: format!("task-{idx}"),
            description: format!("do task {idx}"),
            status: TaskStatus::Pending,
            dependencies: if idx == 0 {
                vec![]
            } else {
                vec![format!("task-{}", idx - 1)]
            },
        })
        .collect();
    Quest {
        quest_id: "q-bench".into(),
        title: "benchmark quest".into(),
        tasks,
        thinking_mode: ThinkingMode::Standard,
        checkpoint_id: None,
        priority: 128,
    }
}

fn bench_select_mode_simple(c: &mut Criterion) {
    let governor = TtgGovernor::new(TtgConfig::default());
    let quest = make_quest(1);

    c.bench_function("select_mode/simple_1_task", |b| {
        b.iter(|| {
            let (mode, _) = governor.select_mode(black_box(&quest), black_box(BudgetTier::LowTier));
            black_box(mode);
        })
    });
}

fn bench_select_mode_medium(c: &mut Criterion) {
    let governor = TtgGovernor::new(TtgConfig::default());
    let quest = make_quest(5);

    c.bench_function("select_mode/medium_5_tasks", |b| {
        b.iter(|| {
            let (mode, _) = governor.select_mode(black_box(&quest), black_box(BudgetTier::LowTier));
            black_box(mode);
        })
    });
}

fn bench_select_mode_complex(c: &mut Criterion) {
    let governor = TtgGovernor::new(TtgConfig::default());
    let quest = make_quest(20);

    c.bench_function("select_mode/complex_20_tasks", |b| {
        b.iter(|| {
            let (mode, _) =
                governor.select_mode(black_box(&quest), black_box(BudgetTier::HighTier));
            black_box(mode);
        })
    });
}

fn bench_evaluate_complexity(c: &mut Criterion) {
    let governor = TtgGovernor::new(TtgConfig::default());
    let quest = make_quest(20);

    c.bench_function("evaluate_complexity/20_tasks", |b| {
        b.iter(|| {
            let score = governor.evaluate_complexity(black_box(&quest));
            black_box(score);
        })
    });
}

criterion_group!(
    benches,
    bench_select_mode_simple,
    bench_select_mode_medium,
    bench_select_mode_complex,
    bench_evaluate_complexity,
);
criterion_main!(benches);
