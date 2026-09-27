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
//! Critical 安全/治理告警事件(is_critical_mpsc_event 清单,当前 14 类)在
//! `EventBus::publish`/`publish_blocking` 中自动额外
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

    /// 压力下降采样策略(ADR-191 D2 / 四维深审 E-2,2026-09-26)
    ///
    /// broadcast 占用率进入压力区(≥ trigger)后,非 Critical 事件按
    /// 「每 `every` 条广播 1 条」降采样,给实时订阅者腾出缓冲区;
    /// 占用回落到 resume 以下才解除。Critical 事件永远全量(红线:
    /// 降级不得伤害关键告警投递),mpsc 旁路不受影响。
    ///
    /// WHY 默认不启用:降采样面向统计/进度类 Normal 流量,消费方语义
    /// 因部署而异——机制先落地,启用由装配方显式选择(with_policy)。
    DownsampleUnderPressure {
        /// 通道容量(with_policy 构造时取用)
        broadcast_capacity: usize,
        /// 压力区内的发送间隔:每 `every` 条 Normal 事件广播 1 条(≥ 2)
        every: u64,
        /// 触发水位(千分比):queued*1000 ≥ capacity*trigger 时入压力区
        trigger_permille: u64,
        /// 解除水位(千分比):queued*1000 ≤ capacity*resume 时退出。
        /// 必须 < trigger 形成滞回死区,防临界振荡(抖动)反复切换
        resume_permille: u64,
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
            Self::DownsampleUnderPressure {
                broadcast_capacity, ..
            } => *broadcast_capacity,
        }
    }

    /// 获取 lag 阈值(若策略为 LagThreshold)
    pub fn max_lag(&self) -> Option<u64> {
        match self {
            Self::LagThreshold { max_lag } => Some(*max_lag),
            _ => None,
        }
    }

    /// 若策略为降采样型,返回其参数四元组 (every, trigger, resume)
    ///
    /// 非降采样策略返 None → 热路径零行为变更(默认路径仅一次枚举判断)
    pub fn downsample_params(&self) -> Option<(u64, u64, u64)> {
        match self {
            Self::DownsampleUnderPressure {
                every,
                trigger_permille,
                resume_permille,
                ..
            } => Some((*every, *trigger_permille, *resume_permille)),
            _ => None,
        }
    }
}

/// 压力滞回状态机(纯函数核)
///
/// 输入当前采样 (queued, capacity) 与上一状态 engaged,输出新状态:
/// - 未锁存 + 占用 ≥ trigger → 锁存(进入压力区)
/// - 已锁存 + 占用 ≤ resume → 解除(滞回死区 [resume, trigger) 内保持原态)
///
/// WHY 纯函数:状态机可直接用 proptest 验证"死区内永不翻转"不变量,
/// 总线侧仅需原子存取,不含判断逻辑(热路径与可测性分离)。
/// WHY 千分比整数比较:避免浮点与除法,`u64` 乘法在 queued/capacity
/// 量级(≤ 1e9)下无溢出风险。
#[must_use]
pub fn pressure_latch_next(
    engaged: bool,
    queued: usize,
    capacity: usize,
    trigger_permille: u64,
    resume_permille: u64,
) -> bool {
    if capacity == 0 {
        return engaged;
    }
    let scaled = queued as u64 * 1000;
    let cap = capacity as u64;
    if !engaged {
        scaled >= cap * trigger_permille
    } else {
        // 死区内( resume < scaled/cap < trigger )保持锁存,防抖动
        scaled > cap * resume_permille
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
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use proptest::prelude::*;

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

    // ==================== E-2 降采样策略与滞回状态机(2026-09-26) ====================

    #[test]
    fn latch_triggers_at_threshold_and_resumes_below_resume() {
        // capacity=1000, trigger=750‰, resume=500‰
        // 未锁存 + 占用 750(=75%) → 触发锁存
        assert!(pressure_latch_next(false, 750, 1000, 750, 500));
        // 已锁存 + 占用 500(=50% 临界) → 解除(≤ resume 语义)
        assert!(!pressure_latch_next(true, 499, 1000, 750, 500));
    }

    #[test]
    fn latch_holds_inside_hysteresis_deadzone_both_directions() {
        // 死区 (500, 750) 内:未锁存不因中等占用误触发,已锁存不误解除
        assert!(!pressure_latch_next(false, 600, 1000, 750, 500));
        assert!(pressure_latch_next(true, 600, 1000, 750, 500));
    }

    #[test]
    fn latch_zero_capacity_is_noop() {
        // 防除零/无意义比较:容量 0 时状态保持
        assert!(!pressure_latch_next(false, 0, 0, 750, 500));
        assert!(pressure_latch_next(true, 0, 0, 750, 500));
    }

    #[test]
    fn downsample_params_only_for_downsample_policy() {
        assert_eq!(
            BackpressurePolicy::default().downsample_params(),
            None,
            "默认策略必须无降采样参数(热路径早退)"
        );
        let p = BackpressurePolicy::DownsampleUnderPressure {
            broadcast_capacity: 512,
            every: 4,
            trigger_permille: 750,
            resume_permille: 500,
        };
        assert_eq!(p.downsample_params(), Some((4, 750, 500)));
        assert_eq!(p.broadcast_capacity(), 512);
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        /// 滞回死区不变量:占用严格落在 (resume, trigger) 开区间时,
        /// 无论上一态如何,输出必等于上一态(状态机在死区内永不翻转)
        #[test]
        fn prop_latch_never_flips_in_deadzone(
            capacity in 1usize..100_000,
            permille in 1u64..1_000,
        ) {
            let trigger = 750u64;
            let resume = 500u64;
            let cap = capacity as u64;
            // 取任意死区内占用:queued*1000 ∈ (cap*500, cap*750)
            let lo = (cap * resume) / 1000 + 1; // 开区间下界
            let hi = cap * trigger / 1000; // 闭区间内最大安全值(严格 < trigger)
            prop_assume!(lo <= hi);
            let queued = (lo + permille % (hi - lo + 1)) as usize;
            let scaled = queued as u64 * 1000;
            prop_assume!(scaled < cap * trigger && scaled > cap * resume);
            // 两方向均保持原态
            prop_assert!(!pressure_latch_next(false, queued, capacity, trigger, resume));
            prop_assert!(pressure_latch_next(true, queued, capacity, trigger, resume));
        }

        /// 单调性不变量:占用越高越应处于压力区(同上一态下的比较关系)
        #[test]
        fn prop_latch_monotone_in_queued(
            capacity in 1usize..50_000,
            q1 in 0usize..60_000,
            q2 in 0usize..60_000,
        ) {
            let (hi, lo) = (q1.max(q2), q1.min(q2));
            let e = true; // 从已锁存态评估解除条件(单调方向明确)
            let r_hi = pressure_latch_next(e, hi, capacity, 750, 500);
            let r_lo = pressure_latch_next(e, lo, capacity, 750, 500);
            // 已锁存时:高占用解除结果 ≥ 低占用解除结果(越高越保持锁存)
            prop_assert!((r_hi as u8) >= (r_lo as u8));
        }
    }
}
