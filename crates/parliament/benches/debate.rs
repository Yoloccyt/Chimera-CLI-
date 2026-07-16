//! Parliament 辩论性能基准 — criterion 基准测试
//!
//! 对应 SubTask 30.5
//!
//! # 基准配置
//! - warmup: 10 次迭代
//! - measurement: 100 次采样
//! - 测量 P50/P99 延迟

use criterion::{criterion_group, criterion_main, Criterion};
use event_bus::EventBus;
use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};
use parliament::{Parliament, ParliamentConfig, Proposal};
use std::time::Duration;

/// 构造测试用 Quest
fn make_quest(task_count: usize, thinking_mode: ThinkingMode) -> Quest {
    let tasks: Vec<Task> = (0..task_count)
        .map(|i| Task {
            task_id: format!("t-{i}"),
            description: format!("任务 {i}"),
            status: TaskStatus::Pending,
            dependencies: vec![],
        })
        .collect();
    Quest {
        quest_id: "q-bench".into(),
        title: "基准测试 Quest".into(),
        tasks,
        thinking_mode,
        checkpoint_id: None,
        priority: 128,
    }
}

/// 基准:低风险少任务辩论(全赞成场景)
fn bench_deliberate_low_risk(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let bus = EventBus::new();
    let parliament = Parliament::new(ParliamentConfig::default(), bus);
    let quest = make_quest(2, ThinkingMode::Fast);

    c.bench_function("deliberate_low_risk", |b| {
        let mut idx = 0u64;
        b.iter(|| {
            let proposal = Proposal::new(format!("p-low-{idx}"), "q-bench", "低风险提案", 0.2);
            idx += 1;
            rt.block_on(parliament.deliberate(&quest, &proposal))
                .unwrap()
        });
    });
}

/// 基准:高风险辩论(Skeptic 否决场景)
fn bench_deliberate_high_risk(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let bus = EventBus::new();
    let parliament = Parliament::new(ParliamentConfig::default(), bus);
    let quest = make_quest(2, ThinkingMode::Fast);

    c.bench_function("deliberate_high_risk", |b| {
        let mut idx = 0u64;
        b.iter(|| {
            let proposal = Proposal::new(format!("p-high-{idx}"), "q-bench", "高风险提案", 0.8);
            idx += 1;
            rt.block_on(parliament.deliberate(&quest, &proposal))
                .unwrap()
        });
    });
}

/// 基准:复杂任务辩论(部分赞成场景)
fn bench_deliberate_complex(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let bus = EventBus::new();
    let parliament = Parliament::new(ParliamentConfig::default(), bus);
    let quest = make_quest(7, ThinkingMode::Deep);

    c.bench_function("deliberate_complex", |b| {
        let mut idx = 0u64;
        b.iter(|| {
            let proposal =
                Proposal::new(format!("p-complex-{idx}"), "q-bench", "复杂任务提案", 0.2);
            idx += 1;
            rt.block_on(parliament.deliberate(&quest, &proposal))
                .unwrap()
        });
    });
}

/// 基准:并发辩论(10 线程同时审议)
fn bench_deliberate_concurrent(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let bus = EventBus::new();
    let parliament = std::sync::Arc::new(Parliament::new(ParliamentConfig::default(), bus));
    let quest = make_quest(2, ThinkingMode::Fast);

    c.bench_function("deliberate_concurrent_10", |b| {
        b.iter(|| {
            rt.block_on(async {
                let mut handles = Vec::new();
                for i in 0..10 {
                    let parliament = parliament.clone();
                    let quest = quest.clone();
                    handles.push(tokio::spawn(async move {
                        let proposal =
                            Proposal::new(format!("p-conc-{i}"), "q-bench", "并发提案", 0.2);
                        parliament.deliberate(&quest, &proposal).await.unwrap()
                    }));
                }
                for handle in handles {
                    let _ = handle.await;
                }
            });
        });
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(100)
        .warm_up_time(Duration::from_millis(500));
    targets = bench_deliberate_low_risk, bench_deliberate_high_risk, bench_deliberate_complex, bench_deliberate_concurrent
}

criterion_main!(benches);
