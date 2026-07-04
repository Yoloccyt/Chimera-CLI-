//! PVL 反馈通道 — 实时反馈处理与策略调整
//!
//! 对应架构:L7 Execution,Producer-Verifier Loop 的反馈端
//!
//! # 设计决策
//! - **AtomicU64 计数**:无锁统计拒绝/总数,避免锁竞争
//! - **滞后策略调整**:升档阈值 = rejection_rate_threshold,
//!   降档阈值 = rejection_rate_threshold / 2,避免策略振荡
//! - **publish_blocking**:check_and_adjust_strategy 为同步方法
//!   (无需 await IO),使用同步发布避免 async 开销
//! - **pub(crate) set_strategy**:Producer 暴露 set_strategy 供
//!   FeedbackChannel 调用,避免外部绕过反馈闭环直接修改策略

use std::sync::atomic::{AtomicU64, Ordering};

use event_bus::{EventBus, EventMetadata, NexusEvent};
use tracing::warn;

use crate::config::PvlConfig;
use crate::error::PvlError;
use crate::producer::Producer;
use crate::types::{FeedbackMessage, ProducerStrategy};

/// PVL 反馈通道 — 处理验证反馈并触发策略调整
///
/// # 线程安全
/// `FeedbackChannel` 内部所有字段均为线程安全。`process_feedback` 为
/// 同步方法,`check_and_adjust_strategy` 为同步方法(使用 publish_blocking)。
///
/// # 反馈闭环
/// 1. Verifier 验证操作后发送 FeedbackMessage
/// 2. FeedbackChannel.process_feedback 更新计数,返回是否需要调整
/// 3. 若需要调整,调用 check_and_adjust_strategy(&producer) 执行调整
/// 4. 策略调整发布 ProducerStrategyAdjusted 事件
pub struct FeedbackChannel {
    /// 执行配置(含拒绝率阈值)
    pub(crate) config: PvlConfig,
    /// 事件总线,用于发布 ProducerStrategyAdjusted 事件
    pub(crate) event_bus: EventBus,
    /// 拒绝操作数(无锁统计)
    pub(crate) rejection_count: AtomicU64,
    /// 总操作数(无锁统计)
    pub(crate) total_count: AtomicU64,
}

impl FeedbackChannel {
    /// 创建新的反馈通道
    ///
    /// # 参数
    /// - `config`:执行配置(含拒绝率阈值)
    /// - `event_bus`:事件总线,用于发布 `ProducerStrategyAdjusted` 事件
    pub fn new(config: PvlConfig, event_bus: EventBus) -> Self {
        Self {
            config,
            event_bus,
            rejection_count: AtomicU64::new(0),
            total_count: AtomicU64::new(0),
        }
    }

    /// 处理反馈消息,更新计数,返回是否需要策略调整
    ///
    /// # 流程
    /// 1. 递增 total_count
    /// 2. 若反馈为拒绝,递增 rejection_count
    /// 3. 计算当前拒绝率
    /// 4. 拒绝率 > 阈值时返回 true(需要调整)
    ///
    /// # 参数
    /// - `feedback`:验证者发送的反馈消息
    ///
    /// # 返回
    /// 是否需要策略调整(拒绝率超过阈值)
    pub fn process_feedback(&self, feedback: FeedbackMessage) -> bool {
        self.total_count.fetch_add(1, Ordering::Relaxed);

        if !feedback.result.passed {
            self.rejection_count.fetch_add(1, Ordering::Relaxed);
        }

        // 计算拒绝率,判断是否需要调整
        let rate = self.rejection_rate();
        rate > self.config.rejection_rate_threshold
    }

    /// 检查拒绝率并调整 Producer 策略
    ///
    /// # 策略调整逻辑(滞后避免振荡)
    /// - 拒绝率 > 阈值 且 当前非 Conservative → 切换到 Conservative
    /// - 拒绝率 < 阈值/2 且 当前为 Conservative → 切换到 Normal(恢复)
    /// - 其他情况 → 不调整
    ///
    /// WHY 滞后:升档阈值(阈值)与降档阈值(阈值/2)不同,
    /// 避免拒绝率在阈值附近波动导致策略频繁切换
    ///
    /// # 参数
    /// - `producer`:待调整的生产者
    ///
    /// # 返回
    /// 是否执行了策略调整
    pub fn check_and_adjust_strategy(&self, producer: &Producer) -> Result<bool, PvlError> {
        let rejection_rate = self.rejection_rate();
        let current_strategy = producer.strategy();

        // 决定新策略(滞后逻辑避免振荡)
        let new_strategy = if rejection_rate > self.config.rejection_rate_threshold
            && current_strategy != ProducerStrategy::Conservative
        {
            // 拒绝率超阈值,降级到 Conservative
            ProducerStrategy::Conservative
        } else if rejection_rate < self.config.rejection_rate_threshold / 2.0
            && current_strategy == ProducerStrategy::Conservative
        {
            // 拒绝率降至阈值一半以下,恢复到 Normal
            ProducerStrategy::Normal
        } else {
            // 无需调整
            return Ok(false);
        };

        // 执行策略调整
        producer.set_strategy(new_strategy);

        // 发布 ProducerStrategyAdjusted 事件
        // WHY publish_blocking:此方法为同步方法,使用同步发布避免 async 开销
        let adjustment_reason = if new_strategy == ProducerStrategy::Conservative {
            format!(
                "拒绝率 {:.2} 超过阈值 {:.2},降级到 Conservative",
                rejection_rate, self.config.rejection_rate_threshold
            )
        } else {
            format!(
                "拒绝率 {:.2} 降至阈值一半以下,恢复到 Normal",
                rejection_rate
            )
        };

        let event = NexusEvent::ProducerStrategyAdjusted {
            metadata: EventMetadata::new("pvl-layer"),
            adjustment_reason,
            new_strategy: new_strategy.name().to_string(),
        };
        if let Err(e) = self.event_bus.publish_blocking(event) {
            warn!(error = %e, "发布 ProducerStrategyAdjusted 事件失败");
            return Err(PvlError::StrategyAdjustmentFailed {
                reason: format!("事件发布失败: {e}"),
            });
        }

        Ok(true)
    }

    /// 计算当前拒绝率 [0.0, 1.0]
    ///
    /// 总数为 0 时返回 0.0(避免除零)
    pub fn rejection_rate(&self) -> f32 {
        let total = self.total_count.load(Ordering::Relaxed);
        if total == 0 {
            return 0.0;
        }
        let rejected = self.rejection_count.load(Ordering::Relaxed);
        rejected as f32 / total as f32
    }

    /// 获取拒绝操作数
    pub fn rejection_count(&self) -> u64 {
        self.rejection_count.load(Ordering::Relaxed)
    }

    /// 获取总操作数
    pub fn total_count(&self) -> u64 {
        self.total_count.load(Ordering::Relaxed)
    }

    /// 获取配置引用
    pub fn config(&self) -> &PvlConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{OperationId, VerificationResult};

    /// 创建通过验证的反馈
    fn make_passed_feedback() -> FeedbackMessage {
        let id = OperationId::new("op-passed");
        FeedbackMessage::new(
            id,
            VerificationResult::passed(OperationId::new("op-passed")),
        )
    }

    /// 创建拒绝验证的反馈
    fn make_rejected_feedback() -> FeedbackMessage {
        let id = OperationId::new("op-rejected");
        FeedbackMessage::new(
            id,
            VerificationResult::rejected(OperationId::new("op-rejected"), "测试拒绝"),
        )
    }

    #[test]
    fn test_process_feedback_updates_counts() {
        // 验证:process_feedback 正确更新计数
        let channel = FeedbackChannel::new(PvlConfig::default(), EventBus::new());

        channel.process_feedback(make_passed_feedback());
        channel.process_feedback(make_rejected_feedback());
        channel.process_feedback(make_passed_feedback());

        assert_eq!(channel.total_count(), 3);
        assert_eq!(channel.rejection_count(), 1);
    }

    #[test]
    fn test_process_feedback_returns_true_when_rate_exceeds_threshold() {
        // 验证:拒绝率超过阈值时返回 true
        let channel = FeedbackChannel::new(PvlConfig::default(), EventBus::new());

        // 发送 10 个反馈:4 个拒绝(40% > 30% 阈值)
        for _ in 0..6 {
            channel.process_feedback(make_passed_feedback());
        }
        // 6 个通过,拒绝率 0%,应返回 false
        assert!(!channel.process_feedback(make_passed_feedback()));

        // 累积拒绝,直到拒绝率超过 30%
        let mut need_adjust = false;
        for _ in 0..4 {
            need_adjust = channel.process_feedback(make_rejected_feedback());
        }
        // 7 通过 + 4 拒绝 = 11 总数,拒绝率 4/11 ≈ 36% > 30%
        assert!(need_adjust, "拒绝率超过 30% 应返回 true");
    }

    #[test]
    fn test_rejection_rate_zero_when_no_feedback() {
        // 验证:无反馈时拒绝率为 0
        let channel = FeedbackChannel::new(PvlConfig::default(), EventBus::new());
        assert_eq!(channel.rejection_rate(), 0.0);
        assert_eq!(channel.total_count(), 0);
        assert_eq!(channel.rejection_count(), 0);
    }

    #[test]
    fn test_check_and_adjust_strategy_to_conservative() {
        // 验证:拒绝率超阈值时调整到 Conservative
        let producer = Producer::new(PvlConfig::default(), EventBus::new());
        let channel = FeedbackChannel::new(PvlConfig::default(), EventBus::new());

        assert_eq!(producer.strategy(), ProducerStrategy::Normal);

        // 制造高拒绝率:3 拒绝 + 3 通过 = 50% > 30%
        for _ in 0..3 {
            channel.process_feedback(make_rejected_feedback());
        }
        for _ in 0..3 {
            channel.process_feedback(make_passed_feedback());
        }

        let adjusted = channel.check_and_adjust_strategy(&producer).unwrap();
        assert!(adjusted, "应执行策略调整");
        assert_eq!(
            producer.strategy(),
            ProducerStrategy::Conservative,
            "应调整到 Conservative"
        );
    }

    #[test]
    fn test_check_and_adjust_strategy_recovery_to_normal() {
        // 验证:拒绝率降至阈值一半以下时恢复到 Normal
        let producer = Producer::new(PvlConfig::default(), EventBus::new());
        let channel = FeedbackChannel::new(PvlConfig::default(), EventBus::new());

        // 先调整到 Conservative
        for _ in 0..4 {
            channel.process_feedback(make_rejected_feedback());
        }
        channel.check_and_adjust_strategy(&producer).unwrap();
        assert_eq!(producer.strategy(), ProducerStrategy::Conservative);

        // 增加大量通过反馈,降低拒绝率到 15% 以下(阈值 30% 的一半)
        // 当前:4 拒绝 + 0 通过 = 100%,需要增加通过使拒绝率 < 15%
        // 4 / (4 + x) < 0.15 → x > 22.67,取 23
        for _ in 0..23 {
            channel.process_feedback(make_passed_feedback());
        }
        // 4/27 ≈ 14.8% < 15%

        let adjusted = channel.check_and_adjust_strategy(&producer).unwrap();
        assert!(adjusted, "应执行策略恢复");
        assert_eq!(
            producer.strategy(),
            ProducerStrategy::Normal,
            "应恢复到 Normal"
        );
    }

    #[test]
    fn test_check_and_adjust_strategy_no_adjustment_needed() {
        // 验证:拒绝率在正常范围时不调整
        let producer = Producer::new(PvlConfig::default(), EventBus::new());
        let channel = FeedbackChannel::new(PvlConfig::default(), EventBus::new());

        // 1 拒绝 + 9 通过 = 10% < 30%,无需调整
        channel.process_feedback(make_rejected_feedback());
        for _ in 0..9 {
            channel.process_feedback(make_passed_feedback());
        }

        let adjusted = channel.check_and_adjust_strategy(&producer).unwrap();
        assert!(!adjusted, "拒绝率正常时不应调整");
        assert_eq!(producer.strategy(), ProducerStrategy::Normal);
    }

    #[test]
    fn test_check_and_adjust_strategy_hysteresis_no_oscillation() {
        // 验证:滞后逻辑避免策略振荡
        let producer = Producer::new(PvlConfig::default(), EventBus::new());
        let channel = FeedbackChannel::new(PvlConfig::default(), EventBus::new());

        // 调整到 Conservative(拒绝率 50%)
        for _ in 0..3 {
            channel.process_feedback(make_rejected_feedback());
        }
        for _ in 0..3 {
            channel.process_feedback(make_passed_feedback());
        }
        channel.check_and_adjust_strategy(&producer).unwrap();
        assert_eq!(producer.strategy(), ProducerStrategy::Conservative);

        // 拒绝率降到 25%(在阈值 30% 以下,但在阈值一半 15% 以上)
        // 当前 3/6=50%,增加 6 个通过 → 3/12=25%
        for _ in 0..6 {
            channel.process_feedback(make_passed_feedback());
        }
        // 25% < 30% 但 > 15%,不应恢复(滞后)
        let adjusted = channel.check_and_adjust_strategy(&producer).unwrap();
        assert!(!adjusted, "拒绝率在滞后区间内不应调整");
        assert_eq!(
            producer.strategy(),
            ProducerStrategy::Conservative,
            "滞后区间内应保持 Conservative"
        );
    }

    #[tokio::test]
    async fn test_strategy_adjustment_publishes_event() {
        // 验证:策略调整发布 ProducerStrategyAdjusted 事件
        let bus = EventBus::new();
        let mut event_rx = bus.subscribe();
        let producer = Producer::new(PvlConfig::default(), bus.clone());
        let channel = FeedbackChannel::new(PvlConfig::default(), bus.clone());

        // 制造高拒绝率
        for _ in 0..4 {
            channel.process_feedback(make_rejected_feedback());
        }

        channel.check_and_adjust_strategy(&producer).unwrap();

        // 应收到 ProducerStrategyAdjusted 事件
        let event = event_rx
            .recv_timeout(std::time::Duration::from_millis(100))
            .await;
        assert!(event.is_ok(), "应收到事件");
        let event = event.unwrap();
        assert!(
            matches!(event, NexusEvent::ProducerStrategyAdjusted { .. }),
            "应为 ProducerStrategyAdjusted 事件,实际: {:?}",
            event
        );
    }
}
