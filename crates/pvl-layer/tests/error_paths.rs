//! PVL 生产验证闭环错误路径测试 — 验证关键故障场景的错误传播
//!
//! 对应 SubTask 29.6:为 pvl-layer 补充错误路径测试
//!
//! # 测试覆盖
//! 1. 通道关闭(Producer 端):接收端 drop 后 produce 返回 ChannelClosed
//! 2. 通道关闭(Verifier 端):反馈接收端 drop 后 run 返回 ChannelClosed
//! 3. 验证失败:危险内容被 verify 拒绝(安全检查)
//! 4. 策略调整:高拒绝率触发 check_and_adjust_strategy 返回 Ok(true)
//! 5. 空操作:produce(0) 正常返回 Ok,无副作用

#![forbid(unsafe_code)]

use event_bus::EventBus;
use pvl_layer::{
    FeedbackChannel, FeedbackMessage, Operation, OperationId, Producer, ProducerStrategy,
    PvlConfig, PvlError, VerificationResult, Verifier,
};
use tokio::sync::mpsc;

/// 通道关闭(Producer 端):接收端 drop 后 produce 返回 ChannelClosed
///
/// WHY:对应 Claude Code 尸检教训(5.4% 孤儿调用),通道关闭未被处理导致操作丢失。
/// PVL 通过显式 ChannelClosed 错误使调用者能感知通道状态并恢复。
/// 此测试验证:drop(rx) 后,produce 的 tx.send().await 返回错误,映射为 ChannelClosed。
#[tokio::test]
async fn test_produce_channel_closed_when_receiver_dropped() {
    let producer = Producer::new(PvlConfig::default(), EventBus::new());
    let (tx, rx) = mpsc::channel::<Operation>(128);

    // drop 接收端,模拟消费者崩溃
    drop(rx);

    // produce 应返回 ChannelClosed(发送失败)
    let result = producer.produce("quest-closed", 5, &tx).await;
    let err = match result {
        Ok(_) => panic!("接收端已 drop 时应返回错误,而非静默成功"),
        Err(e) => e,
    };
    assert!(
        matches!(err, PvlError::ChannelClosed),
        "应为 ChannelClosed,实际: {err:?}"
    );
    // produced_count 应为 0(第一个操作发送就失败,未递增)
    assert_eq!(
        producer.produced_count(),
        0,
        "发送失败时 produced_count 应为 0"
    );
}

/// 通道关闭(Verifier 端):反馈接收端 drop 后 run 返回 ChannelClosed
///
/// WHY:Verifier.run 通过 feedback_tx.send().await 发送反馈,
/// 若 FeedbackChannel 崩溃(接收端 drop),run 应返回 ChannelClosed 而非静默丢弃。
/// 此测试验证反馈通道关闭时的错误传播。
#[tokio::test]
async fn test_verifier_run_channel_closed_when_feedback_receiver_dropped() {
    let verifier = Verifier::new(PvlConfig::default(), EventBus::new());
    let (op_tx, mut op_rx) = mpsc::channel::<Operation>(128);
    let (feedback_tx, feedback_rx) = mpsc::channel::<FeedbackMessage>(128);

    // 发送一个合法操作
    let mut op = Operation::new(OperationId::new("op-1"), "quest-1", "valid content");
    op.mark_produced(0.8);
    op_tx.send(op).await.unwrap();
    drop(op_tx);

    // drop 反馈接收端,模拟 FeedbackChannel 崩溃
    drop(feedback_rx);

    // run 应返回 ChannelClosed(反馈发送失败)
    let result = verifier.run(&mut op_rx, &feedback_tx).await;
    let err = match result {
        Ok(_) => panic!("反馈接收端已 drop 时应返回错误"),
        Err(e) => e,
    };
    assert!(
        matches!(err, PvlError::ChannelClosed),
        "应为 ChannelClosed,实际: {err:?}"
    );
}

/// 验证失败:危险内容被 verify 拒绝(安全检查)
///
/// WHY:Verifier 的安全检查是 PVL 的核心防线,危险命令(如 rm -rf)必须被拒绝。
/// 此测试验证 verify 对危险内容返回 rejected 结果,且原因包含"安全检查失败"。
/// 注意:这是验证结果的"拒绝"(正常验证流程),非 PvlError::VerificationFailed
/// (后者表示验证过程本身出错,如规则加载失败)。
#[tokio::test]
async fn test_verify_rejects_dangerous_content() {
    let verifier = Verifier::new(PvlConfig::default(), EventBus::new());

    // 测试多种危险关键词
    let dangerous_contents = [
        "rm -rf /",
        "sudo rm -rf /home",
        "chmod 777 /etc/passwd",
        "mkfs.ext4 /dev/sda",
        "dd if=/dev/zero of=/dev/sda",
        ":(){:|:&};:",
        "fork bomb",
    ];

    for content in &dangerous_contents {
        let mut op = Operation::new(OperationId::new("op-danger"), "quest-1", *content);
        op.mark_produced(0.9);
        let result = verifier.verify(&op);
        assert!(!result.passed, "危险内容 '{}' 应被拒绝", content);
        assert!(
            result.reason.contains("安全检查失败"),
            "拒绝原因应包含'安全检查失败',内容: '{}',实际: {}",
            content,
            result.reason
        );
    }

    // 验证大小写不敏感(安全检查应 to_lowercase 后匹配)
    let mut op_upper = Operation::new(OperationId::new("op-upper"), "quest-1", "RM -RF /");
    op_upper.mark_produced(0.9);
    let result_upper = verifier.verify(&op_upper);
    assert!(!result_upper.passed, "大写危险命令也应被拒绝");
}

/// 策略调整:高拒绝率触发 check_and_adjust_strategy 返回 Ok(true)
///
/// WHY:反馈闭环是 PVL 自适应生产控制的核心,拒绝率超阈值时必须触发策略降级。
/// 此测试验证:拒绝率 > 阈值时,策略从 Normal 调整到 Conservative。
#[tokio::test]
async fn test_strategy_adjustment_triggered_by_high_rejection_rate() {
    let producer = Producer::new(PvlConfig::default(), EventBus::new());
    let channel = FeedbackChannel::new(PvlConfig::default(), EventBus::new());

    // 初始策略为 Normal
    assert_eq!(producer.strategy(), ProducerStrategy::Normal);

    // 制造高拒绝率:5 拒绝 + 5 通过 = 50% > 30% 阈值
    for i in 0..5 {
        let id = OperationId::new(format!("op-rej-{i}"));
        let fb = FeedbackMessage::new(id.clone(), VerificationResult::rejected(id, "测试拒绝"));
        channel.process_feedback(fb);
    }
    for i in 0..5 {
        let id = OperationId::new(format!("op-pass-{i}"));
        let fb = FeedbackMessage::new(id.clone(), VerificationResult::passed(id));
        channel.process_feedback(fb);
    }

    // 拒绝率应为 50% > 30% 阈值
    assert!(
        channel.rejection_rate() > 0.3,
        "拒绝率 {} 应 > 0.3 阈值",
        channel.rejection_rate()
    );

    // 执行策略调整
    let adjusted = channel
        .check_and_adjust_strategy(&producer)
        .expect("策略调整不应失败");
    assert!(adjusted, "高拒绝率应触发策略调整");
    assert_eq!(
        producer.strategy(),
        ProducerStrategy::Conservative,
        "应调整到 Conservative"
    );
}

/// 空操作:produce(0) 正常返回 Ok,无副作用
///
/// WHY:空操作是合法输入(如 Producer 启动时的预热调用),
/// 不应 panic 或产生副作用。此测试验证边界条件的健壮性。
#[tokio::test]
async fn test_produce_zero_count_returns_ok_no_side_effects() {
    let producer = Producer::new(PvlConfig::default(), EventBus::new());
    let (tx, mut rx) = mpsc::channel::<Operation>(128);

    // produce 0 个操作
    let result = producer.produce("quest-empty", 0, &tx).await;
    assert!(result.is_ok(), "produce(0) 应返回 Ok");
    drop(tx);

    // 不应收到任何操作
    assert!(rx.recv().await.is_none(), "count=0 时不应生成任何操作");
    assert_eq!(producer.produced_count(), 0, "produced_count 应为 0");
}
