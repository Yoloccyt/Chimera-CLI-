//! Parliament 辩论性能基准 — criterion 基准测试
//!
//! 对应 SubTask 30.5 + M0-T0.1(三档策略基线扩展)
//!
//! # 基准配置
//! - warmup: 10 次迭代
//! - measurement: 100 次采样
//! - 测量 P50/P99 延迟
//!
//! # 口径说明(M0-T0.1)
//! `generate_opinion` 当前为占位实现(仅 yield_now),本基准测量的是
//! **编排 wall-clock 开销**(事件发布 + 并发收集 + 投票),非真实模型延迟。
//! 三档策略基线(fastpath/simplified/full)为 M1 埋点开销回归对照与
//! M3 封顶降档收益证据提供对比基准(性能可证伪铁律)。

use criterion::{criterion_group, criterion_main, Criterion};
use event_bus::{EventBus, EventMetadata, NexusEvent};
use nexus_contracts::{ActivationStrategy, ParliamentPolicy};
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

// ============================================================
// M0-T0.1:三档策略基线扩展(fastpath/simplified + 否决短路)
//
// WHY 新增:既有基准只测 Full 路径(默认策略)。三档对比基线是:
// - M1 埋点(Instant + DebateCompleted 发布)开销回归的对照(<5% 红线)
// - M3 封顶降档(Full→Simplified→FastPath)真实延迟收益的证据
// ============================================================

/// 基准:FastPath 策略(跳过 Opinion 生成,仅 Skeptic 检测 + 直接共识)
fn bench_deliberate_fastpath(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let bus = EventBus::new();
    let parliament = Parliament::new(ParliamentConfig::default(), bus);
    let quest = make_quest(2, ThinkingMode::Fast);
    let policy = ParliamentPolicy::static_policy(ActivationStrategy::FastPath);

    c.bench_function("deliberate_fastpath", |b| {
        let mut idx = 0u64;
        b.iter(|| {
            let proposal = Proposal::new(format!("p-fp-{idx}"), "q-bench", "低风险提案", 0.2);
            idx += 1;
            rt.block_on(parliament.deliberate_with_policy(&quest, &proposal, &policy))
                .unwrap()
        });
    });
}

/// 基准:Simplified 策略(3 关键角色辩论)
fn bench_deliberate_simplified(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let bus = EventBus::new();
    let parliament = Parliament::new(ParliamentConfig::default(), bus);
    let quest = make_quest(2, ThinkingMode::Fast);
    let policy = ParliamentPolicy::static_policy(ActivationStrategy::Simplified);

    c.bench_function("deliberate_simplified", |b| {
        let mut idx = 0u64;
        b.iter(|| {
            let proposal = Proposal::new(format!("p-sim-{idx}"), "q-bench", "中风险提案", 0.2);
            idx += 1;
            rt.block_on(parliament.deliberate_with_policy(&quest, &proposal, &policy))
                .unwrap()
        });
    });
}

/// 基准:Skeptic 否决短路(恶意模式提案,验证 <10ms 声明)
fn bench_deliberate_veto_path(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let bus = EventBus::new();
    let parliament = Parliament::new(ParliamentConfig::default(), bus);
    let quest = make_quest(2, ThinkingMode::Fast);

    c.bench_function("deliberate_veto_path", |b| {
        let mut idx = 0u64;
        b.iter(|| {
            // 恶意模式触发 Skeptic 前置否决,跳过辩论直接短路
            let proposal = Proposal::new(format!("p-veto-{idx}"), "q-bench", "sudo rm -rf /", 0.9);
            idx += 1;
            rt.block_on(parliament.deliberate(&quest, &proposal))
                .unwrap()
        });
    });
}

// ============================================================
// M0-T0.1(第二轮):测量盲区修复 — 带活跃订阅者的真实发布成本
//
// WHY 新增:既有 7 个基准均在无订阅者下运行,broadcast send 立即返回 Err,
// publish 的 receiver_count/lag 检测/背压采样等固定开销被严重低估。保留一个
// 持续 drain 的订阅者使 send 走完整"有订阅者"路径,测出真实事件发布成本。
// 这些基准是 T1.4 publish_vote_events 采用 publish_batch 的证伪门禁基线。
// ============================================================

/// 基准:Full 路径带活跃订阅者(真实发布成本,T1.4 门禁基线)
fn bench_deliberate_full_with_subscriber(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let bus = EventBus::new();
    // 保留一个持续消费的订阅者:使 receiver_count>0,send 走完整 lag 检测路径。
    // 持续 drain 避免 Lagged 告警噪声污染测量(§4.4 反模式 3:先 subscribe 再 spawn)。
    let mut rx = bus.subscribe();
    rt.spawn(async move { while rx.recv().await.is_ok() {} });
    let parliament = Parliament::new(ParliamentConfig::default(), bus);
    let quest = make_quest(2, ThinkingMode::Fast);

    c.bench_function("deliberate_full_with_subscriber", |b| {
        let mut idx = 0u64;
        b.iter(|| {
            let proposal = Proposal::new(format!("p-fws-{idx}"), "q-bench", "低风险提案", 0.2);
            idx += 1;
            rt.block_on(parliament.deliberate(&quest, &proposal))
                .unwrap()
        });
    });
}

/// 基准:Simplified 路径带活跃订阅者(3 角色发布成本对照)
fn bench_deliberate_simplified_with_subscriber(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    rt.spawn(async move { while rx.recv().await.is_ok() {} });
    let parliament = Parliament::new(ParliamentConfig::default(), bus);
    let quest = make_quest(2, ThinkingMode::Fast);
    let policy = ParliamentPolicy::static_policy(ActivationStrategy::Simplified);

    c.bench_function("deliberate_simplified_with_subscriber", |b| {
        let mut idx = 0u64;
        b.iter(|| {
            let proposal = Proposal::new(format!("p-sws-{idx}"), "q-bench", "中风险提案", 0.2);
            idx += 1;
            rt.block_on(parliament.deliberate_with_policy(&quest, &proposal, &policy))
                .unwrap()
        });
    });
}

/// 基准:VoteCast 串行发布段隔离基线(N=5)
///
/// 直接模拟 `publish_vote_events` 的串行 for 循环发布,作为 T1.4
/// publish_batch 摊销收益的直接对照基线(相同订阅者环境)。
fn bench_publish_votecast_serial(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    rt.spawn(async move { while rx.recv().await.is_ok() {} });

    c.bench_function("publish_votecast_serial_5", |b| {
        b.iter(|| {
            rt.block_on(async {
                for i in 0..5u8 {
                    let event = NexusEvent::VoteCast {
                        metadata: EventMetadata::new("parliament"),
                        proposal_id: "p-bench".to_string(),
                        voter: format!("role-{i}"),
                        vote: true,
                    };
                    let _ = bus.publish(event).await;
                }
            });
        });
    });
}

/// 基准:Full 路径 50 任务(放大 O(R×T) clone 开销)
///
/// make_quest(50) 使 collect_opinions_filtered 的每角色 quest.clone()
/// 深拷贝 50 个 Task,5 角色共 250 次 Task 克隆——作为 T1.2 Arc 共享
/// 优化收益的对照基线(大任务数下 clone 开销应显著下降)。
fn bench_deliberate_full_50tasks(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let bus = EventBus::new();
    let parliament = Parliament::new(ParliamentConfig::default(), bus);
    let quest = make_quest(50, ThinkingMode::Deep);

    c.bench_function("deliberate_full_50tasks", |b| {
        let mut idx = 0u64;
        b.iter(|| {
            let proposal = Proposal::new(format!("p-50t-{idx}"), "q-bench", "大任务提案", 0.2);
            idx += 1;
            rt.block_on(parliament.deliberate(&quest, &proposal))
                .unwrap()
        });
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(100)
        .warm_up_time(Duration::from_millis(500));
    targets = bench_deliberate_low_risk, bench_deliberate_high_risk, bench_deliberate_complex,
        bench_deliberate_concurrent, bench_deliberate_fastpath, bench_deliberate_simplified,
        bench_deliberate_veto_path, bench_deliberate_full_with_subscriber,
        bench_deliberate_simplified_with_subscriber, bench_publish_votecast_serial,
        bench_deliberate_full_50tasks
}

criterion_main!(benches);
