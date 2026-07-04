//! 背压策略 — 慢消费者隔离与关键事件保护
//!
//! 设计依据:架构红线"5.4% 孤儿调用"与"void Promise 无 await"教训,
//! 所有异步操作必须有超时/聚集处理,慢消费者不能拖垮整个系统。
//!
//! # 策略说明
//! - `LagThreshold`:订阅者 lag 超过阈值时发出 SlowConsumerDropped 告警
//! - `DropOldest`:broadcast 通道默认行为,旧事件被新事件覆盖
//! - `CriticalMpsc`:关键事件(CheckpointSaved 等)建议走 mpsc 点对点通道
//!
//! # 实现说明
//! 已实现双通道:broadcast + mpsc 旁路,2026-06-29。
//! 4 类 Critical 安全告警事件(SkepticVeto/RedTeamAudit/AsaIntervention/
//! BudgetExceeded)在 `EventBus::publish`/`publish_blocking` 中自动额外
//! 投递到 mpsc 旁路通道(见 `bus.rs::is_critical_mpsc_event`)。订阅者通过
//! `EventBus::subscribe_critical_events()` 获取 mpsc Receiver,确保在 broadcast
//! Lagged 场景下仍能接收 Critical 事件。

use crate::types::{EventMetadata, EventSeverity, NexusEvent};

/// 背压策略
#[derive(Debug, Clone)]
pub enum BackpressurePolicy {
    /// Lag 阈值策略:订阅者 lag 超过阈值时触发告警
    ///
    /// WHY:broadcast 通道在消费者慢时会丢弃旧消息并返回 Lagged 错误,
    /// 此策略将该错误转换为 SlowConsumerDropped 事件,便于运维感知
    LagThreshold {
        /// 允许的最大滞后事件数
        max_lag: u64,
    },

    /// 丢弃最旧策略:broadcast 通道的默认行为
    ///
    /// 通道满时新事件覆盖最旧事件,适用于可重算的普通事件
    DropOldest,

    /// 关键事件走 mpsc 点对点策略
    ///
    /// WHY:CheckpointSaved 等关键事件丢失会导致 Quest 无法恢复,
    /// 建议为这类事件建立独立 mpsc 通道确保投递。
    /// 已实现双通道:broadcast + mpsc 旁路,2026-06-29(见 `bus.rs`
    /// `subscribe_critical_events` / `is_critical_mpsc_event`)。
    CriticalMpsc {
        /// 普通事件仍走 broadcast
        broadcast_capacity: usize,
    },
}

impl Default for BackpressurePolicy {
    fn default() -> Self {
        // 默认策略:lag 阈值 256,平衡告警灵敏度与误报率
        Self::LagThreshold { max_lag: 256 }
    }
}

impl BackpressurePolicy {
    /// 获取默认广播容量
    pub fn broadcast_capacity(&self) -> usize {
        match self {
            Self::LagThreshold { .. } | Self::DropOldest => 1024,
            Self::CriticalMpsc { broadcast_capacity } => *broadcast_capacity,
        }
    }

    /// 获取 lag 阈值(若策略为 LagThreshold)
    pub fn max_lag(&self) -> Option<u64> {
        match self {
            Self::LagThreshold { max_lag } => Some(*max_lag),
            _ => None,
        }
    }
}

/// 慢消费者检测器 — 跟踪订阅者 lag 并在超阈值时生成告警事件
///
/// 使用方式:每次接收事件后调用 `record_lag`,若返回 Some 则发布告警
#[derive(Debug)]
pub struct SlowConsumerDetector {
    /// 订阅者标识,用于告警定位
    subscriber_id: String,
    /// lag 阈值
    threshold: u64,
    /// 累计丢弃计数
    dropped_total: u64,
}

impl SlowConsumerDetector {
    /// 创建检测器
    pub fn new(subscriber_id: impl Into<String>, threshold: u64) -> Self {
        Self {
            subscriber_id: subscriber_id.into(),
            threshold,
            dropped_total: 0,
        }
    }

    /// 记录一次 lag,若超过阈值返回告警事件
    ///
    /// 返回值:Some(SlowConsumerDropped) 表示需要发布告警
    pub fn record_lag(&mut self, lag: u64) -> Option<NexusEvent> {
        if lag > self.threshold {
            self.dropped_total = self.dropped_total.saturating_add(lag);
            let event = NexusEvent::SlowConsumerDropped {
                metadata: EventMetadata::new("event-bus"),
                subscriber_id: self.subscriber_id.clone(),
                lag,
                dropped_count: self.dropped_total,
            };
            Some(event)
        } else {
            None
        }
    }

    /// 获取累计丢弃总数
    pub fn dropped_total(&self) -> u64 {
        self.dropped_total
    }
}

/// 判断事件是否应走关键通道
///
/// 关键事件:CheckpointSaved、ConsensusReached、SlowConsumerDropped
/// 这些事件丢失会导致系统状态不一致或告警遗漏
pub fn is_critical_event(event: &NexusEvent) -> bool {
    event.severity() == EventSeverity::Critical
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detector_below_threshold() {
        let mut det = SlowConsumerDetector::new("sub-1", 100);
        assert!(det.record_lag(50).is_none());
        assert_eq!(det.dropped_total(), 0);
    }

    #[test]
    fn test_detector_above_threshold() {
        let mut det = SlowConsumerDetector::new("sub-1", 100);
        let event = det.record_lag(150).expect("应触发告警");
        match event {
            NexusEvent::SlowConsumerDropped {
                subscriber_id, lag, ..
            } => {
                assert_eq!(subscriber_id, "sub-1");
                assert_eq!(lag, 150);
            }
            _ => panic!("应为 SlowConsumerDropped 事件"),
        }
        assert_eq!(det.dropped_total(), 150);
    }

    #[test]
    fn test_critical_event_detection() {
        let critical = NexusEvent::CheckpointSaved {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: "q1".into(),
            checkpoint_id: "c1".into(),
            memory_snapshot_hash: "h".into(),
        };
        assert!(is_critical_event(&critical));

        let normal = NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k".into(),
        };
        assert!(!is_critical_event(&normal));
    }

    #[test]
    fn test_default_policy() {
        let p = BackpressurePolicy::default();
        assert_eq!(p.broadcast_capacity(), 1024);
        assert_eq!(p.max_lag(), Some(256));
    }
}
