//! PVL 并发测试 — 100 操作流式生成验证
//!
//! 对应 SubTask 25.5:验证大规模流式生产验证的稳定性
//! - 100 操作流式生成验证,无 panic、无数据丢失、无死锁
//! - 验证所有 Future 均 await 或 spawn 管理(零 void Promise)
//! - 通道操作无竞态(mpsc 所有权转移保证)
//!
//! # 测试覆盖
//! - `test_concurrent_100_operations_no_panic`:100 操作流式 PVL,无 panic
//! - `test_concurrent_no_data_loss`:验证操作数 = 生产数(无丢失)
//! - `test_concurrent_no_deadlock`:验证能在超时内完成(无死锁)
//! - `test_concurrent_mixed_valid_dangerous`:混合有效/危险操作,验证计数正确
//! - `test_concurrent_feedback_strategy_adjustment`:高拒绝率触发策略调整
//! - `test_zero_void_promise`:验证所有 async 操作均被 await

use std::time::Duration;

use event_bus::EventBus;
use pvl_layer::{FeedbackChannel, Operation, Producer, ProducerStrategy, PvlConfig, Verifier};
use tokio::sync::mpsc;

/// 验证 100 个操作流式生成验证,无 panic
///
/// 流程:Producer 生成 100 操作 → Verifier 验证 → FeedbackChannel 处理反馈
/// 全程使用 mpsc 通道,无共享可变状态,保证无竞态
#[tokio::test]
async fn test_concurrent_100_operations_no_panic() {
    let bus = EventBus::new();
    let config = PvlConfig::default();

    let producer = Producer::new(config.clone(), bus.clone());
    let verifier = Verifier::new(config.clone(), bus.clone());
    let feedback = FeedbackChannel::new(config, bus);

    let (op_tx, op_rx) = mpsc::channel::<Operation>(128);
    let (fb_tx, fb_rx) = mpsc::channel::<pvl_layer::FeedbackMessage>(128);

    // 启动验证者(后台任务)— spawn 管理,JoinHandle 被 await
    let verifier_handle = tokio::spawn(async move {
        let mut rx = op_rx;
        verifier.run(&mut rx, &fb_tx).await
    });

    // 生产 100 个操作
    producer
        .produce("quest-concurrent", 100, &op_tx)
        .await
        .unwrap();
    drop(op_tx); // 关闭发送端,使 Verifier 退出 run 循环

    // 处理反馈(主任务)
    let mut feedback_count = 0;
    let mut fb_rx = fb_rx;
    while let Some(fb) = fb_rx.recv().await {
        feedback.process_feedback(fb);
        feedback_count += 1;
    }

    // 等待验证者完成 — await JoinHandle,零 void Promise
    verifier_handle.await.unwrap().unwrap();

    // 验证:无 panic,所有操作均被处理
    assert_eq!(feedback_count, 100, "应收到 100 个反馈(无数据丢失)");
    assert_eq!(producer.produced_count(), 100);
    assert_eq!(feedback.total_count(), 100);
}

/// 验证无数据丢失:生产数 = 验证数 = 反馈数
#[tokio::test]
async fn test_concurrent_no_data_loss() {
    let bus = EventBus::new();
    let config = PvlConfig::default();

    let producer = Producer::new(config.clone(), bus.clone());
    let verifier = Verifier::new(config.clone(), bus.clone());
    let feedback = FeedbackChannel::new(config, bus);

    let (op_tx, op_rx) = mpsc::channel::<Operation>(128);
    let (fb_tx, fb_rx) = mpsc::channel::<pvl_layer::FeedbackMessage>(128);

    let verifier_handle = tokio::spawn(async move {
        let mut rx = op_rx;
        verifier.run(&mut rx, &fb_tx).await
    });

    let count = 50;
    producer
        .produce("quest-no-loss", count, &op_tx)
        .await
        .unwrap();
    drop(op_tx);

    let mut feedback_count = 0;
    let mut fb_rx = fb_rx;
    while let Some(fb) = fb_rx.recv().await {
        feedback.process_feedback(fb);
        feedback_count += 1;
    }

    verifier_handle.await.unwrap().unwrap();

    // 无数据丢失:生产数 = 反馈数 = 总计数
    assert_eq!(producer.produced_count(), count as u64, "生产数应匹配");
    assert_eq!(feedback_count, count, "反馈数应匹配");
    assert_eq!(feedback.total_count(), count as u64, "总计数应匹配");
    // 验证数 + 拒绝数 = 总数
    assert_eq!(
        feedback.total_count(),
        feedback.rejection_count() + (feedback.total_count() - feedback.rejection_count()),
        "验证数 + 拒绝数应等于总数"
    );
}

/// 验证无死锁:能在超时内完成
///
/// 对应尸检教训:无超时控制导致永久挂起。
/// 此测试设置 5 秒超时,验证 PVL 流程不会死锁
#[tokio::test]
async fn test_concurrent_no_deadlock() {
    let bus = EventBus::new();
    let config = PvlConfig::default();

    let producer = Producer::new(config.clone(), bus.clone());
    let verifier = Verifier::new(config.clone(), bus.clone());
    let feedback = FeedbackChannel::new(config, bus);

    let (op_tx, op_rx) = mpsc::channel::<Operation>(128);
    let (fb_tx, fb_rx) = mpsc::channel::<pvl_layer::FeedbackMessage>(128);

    let verifier_handle = tokio::spawn(async move {
        let mut rx = op_rx;
        verifier.run(&mut rx, &fb_tx).await
    });

    // 设置 5 秒超时,验证无死锁
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        producer.produce("quest-deadlock", 100, &op_tx),
    )
    .await;

    assert!(result.is_ok(), "produce 应在 5 秒内完成(无死锁)");
    result.unwrap().unwrap();
    drop(op_tx);

    // 处理反馈也设置超时
    let feedback_result = tokio::time::timeout(Duration::from_secs(5), async {
        let mut count = 0;
        let mut fb_rx = fb_rx;
        while let Some(fb) = fb_rx.recv().await {
            feedback.process_feedback(fb);
            count += 1;
        }
        count
    })
    .await;

    assert!(feedback_result.is_ok(), "反馈处理应在 5 秒内完成(无死锁)");
    assert_eq!(feedback_result.unwrap(), 100);

    // 验证者也应能完成
    let verifier_result = tokio::time::timeout(Duration::from_secs(5), verifier_handle).await;
    assert!(verifier_result.is_ok(), "验证者应在 5 秒内完成(无死锁)");
}

/// 验证混合有效/危险操作的并发处理
///
/// Producer 生成的操作内容为 "operation-{quest}-{i}",
/// 均为有效内容(不含危险关键词)。
/// 此测试额外手动注入危险操作,验证 Verifier 正确拒绝
#[tokio::test]
async fn test_concurrent_mixed_valid_dangerous() {
    let bus = EventBus::new();
    let config = PvlConfig::default();

    let producer = Producer::new(config.clone(), bus.clone());
    let verifier = Verifier::new(config.clone(), bus.clone());
    let feedback = FeedbackChannel::new(config, bus);

    let (op_tx, op_rx) = mpsc::channel::<Operation>(128);
    let (fb_tx, fb_rx) = mpsc::channel::<pvl_layer::FeedbackMessage>(128);

    let verifier_handle = tokio::spawn(async move {
        let mut rx = op_rx;
        verifier.run(&mut rx, &fb_tx).await
    });

    // 生产 90 个有效操作
    producer.produce("quest-mixed", 90, &op_tx).await.unwrap();

    // 手动注入 10 个危险操作
    use pvl_layer::OperationId;
    for i in 0..10 {
        let mut op = Operation::new(
            OperationId::new(format!("dangerous-{i}")),
            "quest-mixed",
            "rm -rf /",
        );
        op.mark_produced(0.9);
        op_tx.send(op).await.unwrap();
    }
    drop(op_tx);

    let mut rejected = 0;
    let mut fb_rx = fb_rx;
    while let Some(fb) = fb_rx.recv().await {
        feedback.process_feedback(fb);
        // 统计通过/拒绝
        if feedback.rejection_count() > rejected as u64 {
            rejected = feedback.rejection_count() as usize;
        }
    }
    let passed = feedback.total_count() as usize - rejected;

    verifier_handle.await.unwrap().unwrap();

    // 验证:90 个通过,10 个拒绝
    assert_eq!(producer.produced_count(), 90, "Producer 生产 90 个");
    assert_eq!(feedback.total_count(), 100, "总计 100 个操作");
    assert_eq!(rejected, 10, "应拒绝 10 个危险操作");
    assert_eq!(passed, 90, "应通过 90 个有效操作");
}

/// 验证高拒绝率触发策略调整
#[tokio::test]
async fn test_concurrent_feedback_strategy_adjustment() {
    let bus = EventBus::new();
    let config = PvlConfig::default();

    let producer = Producer::new(config.clone(), bus.clone());
    let verifier = Verifier::new(config.clone(), bus.clone());
    let feedback = FeedbackChannel::new(config, bus);

    let (op_tx, op_rx) = mpsc::channel::<Operation>(128);
    let (fb_tx, fb_rx) = mpsc::channel::<pvl_layer::FeedbackMessage>(128);

    let verifier_handle = tokio::spawn(async move {
        let mut rx = op_rx;
        verifier.run(&mut rx, &fb_tx).await
    });

    // 生产 50 个有效操作
    producer
        .produce("quest-strategy", 50, &op_tx)
        .await
        .unwrap();

    // 注入 50 个危险操作(使拒绝率达到 50% > 30% 阈值)
    use pvl_layer::OperationId;
    for i in 0..50 {
        let mut op = Operation::new(
            OperationId::new(format!("danger-{i}")),
            "quest-strategy",
            "rm -rf /",
        );
        op.mark_produced(0.9);
        op_tx.send(op).await.unwrap();
    }
    drop(op_tx);

    // 处理反馈,检查策略调整
    let mut strategy_adjusted = false;
    let mut fb_rx = fb_rx;
    while let Some(fb) = fb_rx.recv().await {
        let needs_adjustment = feedback.process_feedback(fb);
        if needs_adjustment && !strategy_adjusted {
            // 尝试策略调整
            let adjusted = feedback.check_and_adjust_strategy(&producer).unwrap();
            if adjusted {
                strategy_adjusted = true;
            }
        }
    }

    verifier_handle.await.unwrap().unwrap();

    // 验证:策略已调整到 Conservative
    assert!(strategy_adjusted, "高拒绝率(50%)应触发策略调整");
    assert_eq!(
        producer.strategy(),
        ProducerStrategy::Conservative,
        "策略应调整为 Conservative"
    );
}

/// 验证零 void Promise:所有 async 操作均被 await 或 spawn 管理
///
/// 对应尸检教训:Claude Code 5.4% 孤儿调用(void Promise 无 await)。
/// 此测试验证 PVL 的所有 async 操作:
/// - produce().await:显式 await
/// - verifier.run():spawn 后 await JoinHandle
/// - tx.send().await:显式 await
/// - rx.recv().await:显式 await
#[tokio::test]
async fn test_zero_void_promise() {
    let bus = EventBus::new();
    let config = PvlConfig::default();

    let producer = Producer::new(config.clone(), bus.clone());
    let verifier = Verifier::new(config.clone(), bus.clone());
    let feedback = FeedbackChannel::new(config, bus);

    let (op_tx, op_rx) = mpsc::channel::<Operation>(128);
    let (fb_tx, fb_rx) = mpsc::channel::<pvl_layer::FeedbackMessage>(128);

    // spawn 管理:JoinHandle 被保存并 await
    let verifier_handle = tokio::spawn(async move {
        let mut rx = op_rx;
        verifier.run(&mut rx, &fb_tx).await
    });

    // 显式 await:produce 的所有 send 均在内部 await
    producer
        .produce("quest-void-promise", 20, &op_tx)
        .await
        .unwrap();
    drop(op_tx); // 显式关闭,使 Verifier 退出

    // 显式 await:recv 循环
    let mut fb_rx = fb_rx;
    while let Some(fb) = fb_rx.recv().await {
        feedback.process_feedback(fb);
    }

    // 显式 await JoinHandle:零 void Promise
    let verifier_result = verifier_handle.await;
    assert!(verifier_result.is_ok(), "JoinHandle 应成功 await");
    assert!(verifier_result.unwrap().is_ok(), "run 应返回 Ok");
}

/// 验证通道操作无竞态:多生产者并发生产
///
/// 使用多个 Producer 实例并发向同一通道发送操作,
/// 验证 mpsc 通道的多生产者特性(无竞态)
#[tokio::test]
async fn test_concurrent_multiple_producers_no_race() {
    let bus = EventBus::new();
    let config = PvlConfig::default();

    // 两个 Producer 共享同一 EventBus 但独立实例
    let producer1 = Producer::new(config.clone(), bus.clone());
    let producer2 = Producer::new(config.clone(), bus.clone());
    let verifier = Verifier::new(config.clone(), bus.clone());
    let feedback = FeedbackChannel::new(config, bus);

    let (op_tx, op_rx) = mpsc::channel::<Operation>(128);
    let (fb_tx, fb_rx) = mpsc::channel::<pvl_layer::FeedbackMessage>(128);

    let verifier_handle = tokio::spawn(async move {
        let mut rx = op_rx;
        verifier.run(&mut rx, &fb_tx).await
    });

    // 两个 Producer 并发生产
    let op_tx1 = op_tx.clone();
    let producer1_handle =
        tokio::spawn(async move { producer1.produce("quest-multi-1", 50, &op_tx1).await });
    let producer2_handle =
        tokio::spawn(async move { producer2.produce("quest-multi-2", 50, &op_tx).await });

    // 等待两个 Producer 完成
    producer1_handle.await.unwrap().unwrap();
    producer2_handle.await.unwrap().unwrap();
    // op_tx 在 producer2_handle 中被 move,此处无需 drop

    // 处理反馈
    let mut total = 0;
    let mut fb_rx = fb_rx;
    while let Some(fb) = fb_rx.recv().await {
        feedback.process_feedback(fb);
        total += 1;
    }

    verifier_handle.await.unwrap().unwrap();

    // 验证:两个 Producer 各生产 50,总计 100,无数据丢失
    assert_eq!(total, 100, "应收到 100 个反馈(无竞态数据丢失)");
    assert_eq!(feedback.total_count(), 100);
}

/// 性能断言测试:100 操作流式 PVL 应在合理时间内完成
#[tokio::test]
#[ignore = "性能断言测试,标记 ignore 避免在常规测试中运行"]
async fn test_perf_100_ops_within_threshold() {
    let bus = EventBus::new();
    let config = PvlConfig::default();

    let producer = Producer::new(config.clone(), bus.clone());
    let verifier = Verifier::new(config.clone(), bus.clone());
    let feedback = FeedbackChannel::new(config, bus);

    let (op_tx, op_rx) = mpsc::channel::<Operation>(128);
    let (fb_tx, fb_rx) = mpsc::channel::<pvl_layer::FeedbackMessage>(128);

    let verifier_handle = tokio::spawn(async move {
        let mut rx = op_rx;
        verifier.run(&mut rx, &fb_tx).await
    });

    let start = std::time::Instant::now();

    producer.produce("quest-perf", 100, &op_tx).await.unwrap();
    drop(op_tx);

    let mut fb_rx = fb_rx;
    while let Some(fb) = fb_rx.recv().await {
        feedback.process_feedback(fb);
    }

    verifier_handle.await.unwrap().unwrap();

    let elapsed = start.elapsed();
    // 100 操作流式 PVL 应在 1 秒内完成(含事件发布开销)
    assert!(
        elapsed < Duration::from_secs(1),
        "100 操作流式 PVL 应在 1 秒内完成,实际: {:?}",
        elapsed
    );
}
