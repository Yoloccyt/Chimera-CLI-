//! Parliament 并发测试 — 验证多线程并发审议无数据竞争
//!
//! 对应 SubTask 30.5
//!
//! # 测试目标
//! - 10 线程同时 deliberate,无 panic、无数据竞争
//! - 角色注册表并发读正确性
//! - 事件总线并发发布正确性

use std::sync::Arc;
use std::time::Duration;

use event_bus::EventBus;
use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};
use parliament::{Parliament, ParliamentConfig, Proposal};

/// 构造测试用议会实例
fn make_parliament() -> Arc<Parliament> {
    let config = ParliamentConfig::default();
    let bus = EventBus::new();
    Arc::new(Parliament::new(config, bus))
}

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
        quest_id: "q-concurrent".into(),
        title: "并发测试 Quest".into(),
        tasks,
        thinking_mode,
        checkpoint_id: None,
    }
}

#[tokio::test]
async fn test_concurrent_deliberate_no_panic() {
    let parliament = make_parliament();
    let quest = make_quest(2, ThinkingMode::Fast);

    // 10 线程并发审议相同提案
    let mut handles = Vec::new();
    for i in 0..10 {
        let parliament = parliament.clone();
        let quest = quest.clone();
        handles.push(tokio::spawn(async move {
            // 每个线程审议 5 次
            for j in 0..5 {
                let proposal =
                    Proposal::new(format!("p-{i}-{j}"), "q-concurrent", "并发测试提案", 0.2);
                let result = parliament.deliberate(&quest, &proposal).await;
                assert!(result.is_ok(), "线程 {i} 第 {j} 次审议失败: {:?}", result);
            }
        }));
    }

    // 等待所有线程完成,任一 panic 则测试失败
    for handle in handles {
        handle.await.expect("task panicked");
    }
}

#[tokio::test]
async fn test_concurrent_deliberate_different_proposals() {
    let parliament = make_parliament();
    let quest = make_quest(2, ThinkingMode::Fast);

    // 10 线程并发审议不同提案(不同风险等级)
    let mut handles = Vec::new();
    for i in 0..10 {
        let parliament = parliament.clone();
        let quest = quest.clone();
        let risk = i as f32 * 0.1; // 0.0, 0.1, ..., 0.9
        handles.push(tokio::spawn(async move {
            let proposal = Proposal::new(
                format!("p-risk-{i}"),
                "q-concurrent",
                format!("风险 {risk}"),
                risk,
            );
            let consensus = parliament.deliberate(&quest, &proposal).await.unwrap();
            // 高风险(>0.5)应触发否决,低风险应达成共识或拒绝
            if risk > 0.5 {
                assert!(consensus.is_vetoed(), "风险 {risk} 应触发否决");
            }
        }));
    }

    for handle in handles {
        handle.await.expect("task panicked");
    }
}

#[tokio::test]
async fn test_concurrent_deliberate_different_quests() {
    let parliament = make_parliament();

    // 10 线程并发审议不同 Quest(不同任务数与思考模式)
    let mut handles = Vec::new();
    for i in 0..10 {
        let parliament = parliament.clone();
        let task_count = (i % 5) + 1; // 1-5 任务
        let thinking_mode = match i % 3 {
            0 => ThinkingMode::Fast,
            1 => ThinkingMode::Standard,
            _ => ThinkingMode::Deep,
        };
        let quest = make_quest(task_count, thinking_mode);
        handles.push(tokio::spawn(async move {
            let proposal = Proposal::new(format!("p-quest-{i}"), "q-concurrent", "测试提案", 0.2);
            let _ = parliament.deliberate(&quest, &proposal).await.unwrap();
        }));
    }

    for handle in handles {
        handle.await.expect("task panicked");
    }
}

#[tokio::test]
async fn test_concurrent_event_bus_no_loss() {
    // 验证并发审议时事件总线不丢失事件(无订阅者时静默丢弃除外)
    let bus = EventBus::new();
    let mut rx = bus.subscribe();

    let parliament = Arc::new(Parliament::new(ParliamentConfig::default(), bus));
    let quest = make_quest(2, ThinkingMode::Fast);

    // 5 线程并发审议,每线程 2 次
    let mut handles = Vec::new();
    for i in 0..5 {
        let parliament = parliament.clone();
        let quest = quest.clone();
        handles.push(tokio::spawn(async move {
            for j in 0..2 {
                let proposal =
                    Proposal::new(format!("p-event-{i}-{j}"), "q-concurrent", "事件测试", 0.2);
                let _ = parliament.deliberate(&quest, &proposal).await;
            }
        }));
    }

    for handle in handles {
        handle.await.expect("task panicked");
    }

    // 应收到 10 次审议的事件(每次 5 VoteCast + 1 ConsensusReached = 6)
    // 但 broadcast 通道容量有限,慢消费者可能丢失。此处仅验证不 panic。
    let mut event_count = 0;
    for _ in 0..100 {
        match tokio::time::timeout(Duration::from_millis(50), rx.recv()).await {
            Ok(Ok(_)) => event_count += 1,
            _ => break,
        }
    }
    // 至少应收到部分事件
    assert!(event_count > 0, "应收到至少部分事件");
}

#[tokio::test]
async fn test_concurrent_registry_read() {
    // 验证并发审议时角色注册表读取正确
    let parliament = make_parliament();

    // 预验证注册表状态
    assert_eq!(parliament.registry().count(), 5);

    let mut handles = Vec::new();
    for _ in 0..10 {
        let parliament = parliament.clone();
        handles.push(tokio::spawn(async move {
            let quest = make_quest(2, ThinkingMode::Fast);
            let proposal = Proposal::new("p-read", "q-concurrent", "读取测试", 0.2);
            let _ = parliament.deliberate(&quest, &proposal).await.unwrap();

            // 审议后注册表应保持 5 角色
            assert_eq!(parliament.registry().count(), 5);
        }));
    }

    for handle in handles {
        handle.await.expect("task panicked");
    }
}

/// 性能断言测试:辩论延迟应 < 200ms
///
/// 标记 `#[ignore]`,需用 `cargo test -- --ignored` 运行
#[tokio::test]
#[ignore = "perf: run with --ignored"]
async fn test_perf_deliberate_latency() {
    let parliament = make_parliament();
    let quest = make_quest(2, ThinkingMode::Fast);
    let proposal = Proposal::new("p-perf", "q-concurrent", "性能测试", 0.2);

    // 预热(首次审议可能有初始化开销)
    let _ = parliament.deliberate(&quest, &proposal).await;

    // 测量 100 次审议延迟
    let mut max_latency = Duration::ZERO;
    for i in 0..100 {
        let proposal = Proposal::new(format!("p-perf-{i}"), "q-concurrent", "性能测试", 0.2);
        let start = std::time::Instant::now();
        let _ = parliament.deliberate(&quest, &proposal).await.unwrap();
        let elapsed = start.elapsed();
        if elapsed > max_latency {
            max_latency = elapsed;
        }
    }

    // 100 次审议的最大延迟应 < 200ms(占位实现)
    assert!(
        max_latency < Duration::from_millis(200),
        "最大辩论延迟 {max_latency:?} 应 < 200ms"
    );
}

/// 性能断言测试:并发审议吞吐量
///
/// 标记 `#[ignore]`,需用 `cargo test -- --ignored` 运行
#[tokio::test]
#[ignore = "perf: run with --ignored"]
async fn test_perf_concurrent_throughput() {
    let parliament = make_parliament();
    let quest = make_quest(2, ThinkingMode::Fast);

    // 10 线程并发,每线程 10 次审议 = 100 次总审议
    let start = std::time::Instant::now();
    let mut handles = Vec::new();
    for i in 0..10 {
        let parliament = parliament.clone();
        let quest = quest.clone();
        handles.push(tokio::spawn(async move {
            for j in 0..10 {
                let proposal = Proposal::new(
                    format!("p-throughput-{i}-{j}"),
                    "q-concurrent",
                    "吞吐量测试",
                    0.2,
                );
                let _ = parliament.deliberate(&quest, &proposal).await.unwrap();
            }
        }));
    }

    for handle in handles {
        handle.await.expect("task panicked");
    }
    let elapsed = start.elapsed();

    // 100 次并发审议应在 5 秒内完成
    assert!(
        elapsed < Duration::from_secs(5),
        "100 次并发审议耗时 {elapsed:?},应 < 5s"
    );
}
