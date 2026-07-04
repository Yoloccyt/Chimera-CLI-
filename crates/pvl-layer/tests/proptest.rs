//! PVL 生产验证闭环属性测试 — 验证通道消息无丢失与反馈闭环不变量
//!
//! 对应 SubTask 29.6:为 pvl-layer 补充 proptest
//!
//! # 验证的不变量
//! 1. 通道消息无丢失:produce N → verify N,produced_count == N,
//!    verified_count + rejected_count == N,feedback total_count == N
//! 2. 反馈闭环有效:拒绝率 > 阈值时 process_feedback 返回 true
//! 3. 置信度 ∈ [0.0, 1.0]:produce 生成的操作置信度始终在单位区间
//! 4. 拒绝率 ∈ [0.0, 1.0]:rejection_rate 始终在单位区间
//!
//! # 策略
//! - 生成合法的 count ∈ [0, 50]
//! - 生成合法的 rejection_rate_threshold ∈ [0.1, 0.9]
//! - 使用 tokio::runtime::Runtime 在 proptest 中执行 async 代码
//!   (WHY:proptest! 宏不兼容 #[tokio::test],需手动创建 runtime)

#![forbid(unsafe_code)]

use event_bus::EventBus;
use proptest::prelude::*;
use pvl_layer::{
    FeedbackChannel, FeedbackMessage, Operation, OperationId, Producer, ProducerStrategy,
    PvlConfig, VerificationResult, Verifier,
};
use tokio::sync::mpsc;

/// 将任意可显示错误转换为 TestCaseError(避免 unwrap,用 ? 传播)
fn fail<E: std::fmt::Display>(e: E) -> TestCaseError {
    TestCaseError::fail(format!("{e}"))
}

/// 生成 [0, 50] 范围的操作数量策略
fn prop_count() -> impl Strategy<Value = usize> {
    0usize..=50
}

/// 生成 [0.1, 0.9] 范围的拒绝率阈值策略
///
/// WHY 范围限制:阈值过低(接近 0)会导致几乎每次反馈都触发调整,
/// 阈值过高(接近 1)会导致几乎永不触发,两者均使测试失去区分度
fn prop_threshold() -> impl Strategy<Value = f32> {
    any::<f32>().prop_map(|v| {
        // 映射到 [0.1, 0.9]:取绝对值 mod 0.8 后加 0.1
        if v.is_nan() || v.is_infinite() {
            0.3
        } else {
            (v.abs().rem_euclid(0.8)) + 0.1
        }
    })
}

/// 构造通过验证的反馈消息
fn make_passed_feedback(id: &str) -> FeedbackMessage {
    let op_id = OperationId::new(id);
    FeedbackMessage::new(op_id.clone(), VerificationResult::passed(op_id))
}

/// 构造拒绝验证的反馈消息
fn make_rejected_feedback(id: &str) -> FeedbackMessage {
    let op_id = OperationId::new(id);
    FeedbackMessage::new(
        op_id.clone(),
        VerificationResult::rejected(op_id, "测试拒绝"),
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// 不变量 1:通道消息无丢失 — produce N → verify N
    ///
    /// 验证:produced_count == N,verified_count + rejected_count == N,
    /// feedback total_count == N(三处计数一致,无消息丢失)
    ///
    /// WHY 通道无丢失:对应 Claude Code 尸检教训(5.4% 孤儿调用),
    /// PVL 通过 mpsc 通道所有权转移保证无竞态,此测试验证消息完整性
    #[test]
    fn test_channel_no_message_loss_produce_verify_count_consistent(
        count in prop_count(),
    ) {
        let rt = tokio::runtime::Runtime::new().map_err(fail)?;
        rt.block_on(async {
            let producer = Producer::new(PvlConfig::default(), EventBus::new());
            let verifier = Verifier::new(PvlConfig::default(), EventBus::new());
            let (op_tx, op_rx) = mpsc::channel::<Operation>(128);
            let (feedback_tx, mut feedback_rx) = mpsc::channel::<FeedbackMessage>(128);

            // 启动 verifier 后台任务
            // WHY 返回 verifier:spawn 消费 verifier 所有权,需返回以便后续检查计数
            let verifier_handle = {
                let mut op_rx = op_rx;
                tokio::spawn(async move {
                    let result = verifier.run(&mut op_rx, &feedback_tx).await;
                    // feedback_tx 在此 drop,关闭反馈发送端
                    (verifier, result)
                })
            };

            // 生产 count 个操作
            producer.produce("quest-prop", count, &op_tx).await.map_err(fail)?;
            drop(op_tx); // 关闭发送端,使 verifier.run 退出

            // 等待 verifier 完成,取回 verifier 实例
            let (verifier, run_result) = verifier_handle.await.map_err(fail)?;
            run_result.map_err(fail)?;

            // 收集所有反馈(feedback_tx 已在 spawn 内 drop,通道已关闭)
            let mut feedback_count = 0usize;
            while let Some(_fb) = feedback_rx.recv().await {
                feedback_count += 1;
            }

            // 核心不变量 1:produced_count == N
            prop_assert_eq!(
                producer.produced_count() as usize,
                count,
                "produced_count 应等于生成数"
            );

            // 核心不变量 2:verified_count + rejected_count == N
            let verified = verifier.verified_count() as usize;
            let rejected = verifier.rejected_count() as usize;
            prop_assert_eq!(
                verified + rejected,
                count,
                "verified({}) + rejected({}) 应等于生成数({})",
                verified, rejected, count
            );

            // 核心不变量 3:feedback total_count == N
            prop_assert_eq!(
                feedback_count, count,
                "反馈数({}) 应等于生成数({}),消息有丢失",
                feedback_count, count
            );

            Ok::<(), TestCaseError>(())
        })?;
    }

    /// 不变量 2:反馈闭环有效 — 拒绝率 > 阈值时 process_feedback 返回 true
    ///
    /// 构造拒绝率超过阈值的反馈序列,验证 process_feedback 在超过阈值时返回 true
    ///
    /// WHY 反馈闭环:对应 PVL 设计中的自适应生产控制,
    /// 拒绝率超阈值时必须触发策略调整,否则 Producer 持续产生低质量操作
    #[test]
    fn test_feedback_loop_triggers_when_rejection_rate_exceeds_threshold(
        threshold in prop_threshold(),
        n_rejected in 1u32..=10,
        n_passed in 0u32..=10,
    ) {
        let config = PvlConfig {
            rejection_rate_threshold: threshold,
            ..Default::default()
        };
        let channel = FeedbackChannel::new(config, EventBus::new());

        // 先发送 n_passed 个通过反馈
        for i in 0..n_passed {
            channel.process_feedback(make_passed_feedback(&format!("p-{i}")));
        }

        // 逐步发送拒绝反馈,检测何时触发调整
        // WHY actual_sent:触发调整后 break,实际发送数 < n_rejected,
        // 需追踪实际发送数用于后续不变量验证
        let mut triggered = false;
        let mut actual_rejected_sent = 0u32;
        for i in 0..n_rejected {
            let need_adjust = channel.process_feedback(make_rejected_feedback(&format!("r-{i}")));
            actual_rejected_sent += 1;
            if need_adjust {
                triggered = true;
                // 触发时拒绝率应 > 阈值
                let rate = channel.rejection_rate();
                prop_assert!(
                    rate > threshold,
                    "触发调整时拒绝率 {} 应 > 阈值 {}",
                    rate,
                    threshold
                );
                break;
            }
        }

        // 若未触发,验证最终拒绝率 <= 阈值
        if !triggered {
            let rate = channel.rejection_rate();
            prop_assert!(
                rate <= threshold,
                "未触发调整时拒绝率 {} 应 <= 阈值 {}",
                rate,
                threshold
            );
        }

        // 不变量:总计数应等于实际发送的反馈总数
        let total_sent = (n_passed + actual_rejected_sent) as u64;
        prop_assert_eq!(
            channel.total_count(),
            total_sent,
            "总计数应等于实际发送的反馈总数"
        );
    }

    /// 不变量 3:produce 生成的操作置信度 ∈ [0.0, 1.0]
    ///
    /// WHY 置信度区间:置信度用于 Verifier 风险门控,
    /// 超出 [0, 1] 会导致门控逻辑混乱(如 1.5 置信度绕过阈值检查)
    #[test]
    fn test_produce_confidence_always_in_unit_interval(
        count in 1usize..=30,
    ) {
        let rt = tokio::runtime::Runtime::new().map_err(fail)?;
        rt.block_on(async {
            let producer = Producer::new(PvlConfig::default(), EventBus::new());
            let (tx, mut rx) = mpsc::channel::<Operation>(128);

            producer.produce("quest-conf", count, &tx).await.map_err(fail)?;
            drop(tx);

            let mut collected = 0usize;
            while let Some(op) = rx.recv().await {
                prop_assert!(
                    op.confidence.is_finite(),
                    "置信度必须为有限值,实际: {}",
                    op.confidence
                );
                prop_assert!(
                    (0.0..=1.0).contains(&op.confidence),
                    "置信度 {} 超出 [0, 1] 区间",
                    op.confidence
                );
                collected += 1;
            }
            prop_assert_eq!(collected, count, "应收集到全部操作");
            Ok::<(), TestCaseError>(())
        })?;
    }

    /// 不变量 4:rejection_rate 始终 ∈ [0.0, 1.0]
    ///
    /// WHY 拒绝率区间:拒绝率用于策略调整决策,
    /// 超出 [0, 1] 会导致滞后逻辑误判(如 1.5 > 阈值恒为真)
    #[test]
    fn test_rejection_rate_always_in_unit_interval(
        n_rejected in 0u32..=20,
        n_passed in 0u32..=20,
    ) {
        let channel = FeedbackChannel::new(PvlConfig::default(), EventBus::new());

        // 发送混合反馈
        for i in 0..n_rejected {
            channel.process_feedback(make_rejected_feedback(&format!("r-{i}")));
        }
        for i in 0..n_passed {
            channel.process_feedback(make_passed_feedback(&format!("p-{i}")));
        }

        let rate = channel.rejection_rate();
        prop_assert!(
            rate.is_finite(),
            "拒绝率必须为有限值,实际: {}",
            rate
        );
        prop_assert!(
            (0.0..=1.0).contains(&rate),
            "拒绝率 {} 超出 [0, 1] 区间 (rejected={}, passed={})",
            rate, n_rejected, n_passed
        );

        // 拒绝数应 <= 总数
        prop_assert!(
            channel.rejection_count() <= channel.total_count(),
            "拒绝数 {} 应 <= 总数 {}",
            channel.rejection_count(),
            channel.total_count()
        );
    }

    /// 不变量 6:verify 对合法内容返回 passed,对危险内容返回 rejected
    ///
    /// WHY 验证一致性:相同内容始终产生相同验证结果(确定性),
    /// 危险关键词检测不区分大小写
    #[test]
    fn test_verify_consistency_valid_vs_dangerous(
        suffix in any::<u32>(),
    ) {
        let verifier = Verifier::new(PvlConfig::default(), EventBus::new());

        // 合法内容应通过
        let valid_content = format!("print('valid-{suffix}')");
        let mut op_valid = Operation::new(
            OperationId::new(format!("op-valid-{suffix}")),
            "quest-1",
            &valid_content,
        );
        op_valid.mark_produced(0.8);
        let result_valid = verifier.verify(&op_valid);
        prop_assert!(result_valid.passed, "合法内容应通过验证");

        // 危险内容应被拒绝
        let dangerous_content = format!("rm -rf /tmp/{suffix}");
        let mut op_danger = Operation::new(
            OperationId::new(format!("op-danger-{suffix}")),
            "quest-1",
            &dangerous_content,
        );
        op_danger.mark_produced(0.9);
        let result_danger = verifier.verify(&op_danger);
        prop_assert!(!result_danger.passed, "危险内容应被拒绝");
        prop_assert!(
            result_danger.reason.contains("安全检查失败"),
            "拒绝原因应包含安全检查失败,实际: {}",
            result_danger.reason
        );
    }

    /// 不变量 7:策略调整 — 高拒绝率触发升档到 Conservative
    ///
    /// WHY 策略调整:拒绝率超阈值时必须降级到 Conservative,
    /// 避免Producer 持续产生低质量操作
    #[test]
    fn test_strategy_adjustment_to_conservative(
        threshold in prop_threshold(),
    ) {
        let config = PvlConfig {
            rejection_rate_threshold: threshold,
            ..Default::default()
        };
        let producer = Producer::new(config.clone(), EventBus::new());
        let channel = FeedbackChannel::new(config, EventBus::new());

        // 初始策略应为 Normal
        prop_assert_eq!(producer.strategy(), ProducerStrategy::Normal);

        // 制造高拒绝率(全部拒绝),触发升档到 Conservative
        for i in 0..10 {
            channel.process_feedback(make_rejected_feedback(&format!("r-{i}")));
        }

        // 拒绝率应为 100% > threshold
        let rate = channel.rejection_rate();
        prop_assert!(
            rate > threshold,
            "拒绝率 {} 应 > 阈值 {}",
            rate, threshold
        );

        let adjusted = channel.check_and_adjust_strategy(&producer).map_err(fail)?;
        prop_assert!(adjusted, "高拒绝率应触发策略调整");
        prop_assert_eq!(
            producer.strategy(),
            ProducerStrategy::Conservative,
            "应调整到 Conservative"
        );
    }
}

// WHY 空参数测试放在 proptest! 宏外:proptest! 宏要求至少 1 个 `parm in strategy` 参数,
// 零参数函数无法匹配宏模式,因此作为普通 #[test] 编写
#[tokio::test]
async fn test_produce_zero_count_no_side_effects() {
    let producer = Producer::new(PvlConfig::default(), EventBus::new());
    let (tx, mut rx) = mpsc::channel::<Operation>(128);

    producer
        .produce("quest-empty", 0, &tx)
        .await
        .expect("produce with count=0 should not fail");
    drop(tx);

    // 不应收到任何操作
    assert!(rx.recv().await.is_none(), "count=0 时不应生成任何操作");
    assert_eq!(producer.produced_count(), 0, "produced_count 应为 0");
}
