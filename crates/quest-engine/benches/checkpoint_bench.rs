//! Checkpoint save/list 性能基准 — L9 优化 2.5(数据关门决策依据)
//!
//! 度量:
//! - `save`:Quest 含 10/100/1000 任务的 MessagePack 序列化 + SHA-256 + 落盘
//! - `list_checkpoints`:列取某 Quest 的检查点 ID 序列
//!
//! 关门原则(§计划 2.5):若 save(100 任务)< 5ms,则 IncrementalCheckpointSystem
//! (v4 报告 §9.1 二期,涉 ADR-004 格式变更)**明确不做**,以数据关门。
//! 元数据边车优化仅在 list 路径证实为瓶颈时才落地。

use std::hint::black_box;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};
use quest_engine::checkpoint::CheckpointManager;
use tempfile::tempdir;

fn make_quest(id: &str, task_count: usize) -> Quest {
    let tasks = (0..task_count)
        .map(|i| Task {
            task_id: format!("task-{i}"),
            description: format!("任务 {i} 的描述文本,模拟真实任务体量"),
            status: TaskStatus::Pending,
            dependencies: if i == 0 {
                vec![]
            } else {
                vec![format!("task-{}", i - 1)]
            },
        })
        .collect();
    Quest {
        quest_id: id.into(),
        title: format!("基准 Quest {id}"),
        tasks,
        thinking_mode: ThinkingMode::Standard,
        checkpoint_id: None,
        priority: 128,
    }
}

fn bench_checkpoint_save(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut group = c.benchmark_group("checkpoint_save");
    for &n in &[10usize, 100, 1000] {
        let quest = make_quest("q-bench", n);
        group.bench_with_input(BenchmarkId::from_parameter(n), &quest, |b, quest| {
            b.iter_batched(
                // 每次全新临时目录,隔离 prune_old 对连续 save 的干扰
                || tempdir().unwrap(),
                |tmp| {
                    let cm = CheckpointManager::new(tmp.path().to_path_buf());
                    rt.block_on(cm.save(black_box(quest))).unwrap();
                },
                criterion::BatchSize::SmallInput,
            );
        });
    }
    group.finish();
}

fn bench_checkpoint_list(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    // 预建一个含多个检查点的 Quest 目录(max_keep=5,故实际保留 5 个)
    let tmp = tempdir().unwrap();
    let cm = CheckpointManager::new(tmp.path().to_path_buf());
    let quest = make_quest("q-list", 100);
    for _ in 0..5 {
        rt.block_on(cm.save(&quest)).unwrap();
    }
    c.bench_function("checkpoint_list/100tasks_5cp", |b| {
        b.iter(|| cm.list_checkpoints(black_box("q-list")).unwrap());
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default().sample_size(20);
    targets = bench_checkpoint_save, bench_checkpoint_list
}
criterion_main!(benches);
