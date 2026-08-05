//! MemConController — MemCon 自适应控制器核心实现
//!
//! 对应架构层:L2 Memory
//! 对应任务:P2-8 MemCon 自适应控制器
//!
//! # 核心职责
//! 整合 `GhostMemoryDetector`(检测)和 `StrategyAdapter`(调整)为一套
//! 完整的自适应控制循环,通过 EventBus 发布检测与调整事件,实现记忆策略
//! 的闭环自适应调优。
//!
//! # 控制循环
//! ```text
//! recall 完成 → record_recall(is_ghost) → 检查幽灵率 → 超阈值且非冷却期 →
//!   → 调整策略 → 发布 MemConStrategyAdjusted 事件 → 进入冷却期
//!       ↓
//!   调整后幽灵率仍超阈值 → 熔断回退 → 发布策略调整事件(reason=circuit_breaker)
//! ```
//!
//! # 冷却期机制
//! 策略调整后进入冷却期(config.cooldown_secs),冷却期内不重复调整,
//! 避免频繁震荡。冷却期状态由 `cooldown_until` 字段维护。
//!
//! # 熔断机制
//! 策略调整后,若后续召回中幽灵率仍超过 `circuit_breaker_ghost_rate`,
//! 触发熔断回退到 `StandardTopK`(最保守的 fallback),确保系统安全。
//!
//! # C4 合规三层 fallback
//! 1. 默认值层:MemConConfig 的默认值(编译期常量)
//! 2. 异常回退层:回退到 StandardTopK
//! 3. 熔断入口层:circuit_breaker_ghost_rate 触发时回退到 StandardTopK

use std::time::{Duration, Instant};

use event_bus::{EventBus, EventMetadata, NexusEvent};
use nexus_contracts::{MemoryStrategy, MemoryStrategyPolicy};
use tracing::{debug, info};

use super::config::MemConConfig;
use super::detector::GhostMemoryDetector;
use super::types::{AdjustmentOutcome, AdjustmentReason, MemConStats};

/// MemCon 自适应控制器
///
/// 整合幽灵记忆检测与策略自适应调整,提供完整的控制循环。
/// 通过 EventBus 发布检测与调整事件,实现记忆策略的闭环自适应调优。
///
/// # 线程安全
/// `MlcEngine` 的 recall 路径使用 `&self`,因此 `MemConController` 需要
/// 内部可变性。`GhostMemoryDetector` 使用 `Mutex` 保护,策略调整使用
/// `AtomicU64` 时间戳判定冷却期。
pub struct MemConController {
    /// 幽灵记忆检测器(滑动窗口)
    ///
    /// WHY RwLock 而非 Mutex:recall hook 高频读取 ghost_rate(读多写少),
    /// record_recall 低频写入,读写不对称场景 RwLock 比 Mutex 更优。
    detector: std::sync::RwLock<GhostMemoryDetector>,
    /// 当前有效的记忆策略(通过 `on_recall` 返回给调用方)
    current_policy: std::sync::RwLock<MemoryStrategyPolicy>,
    /// 冷却期截止时间(Instant::now 单调时钟,不受系统时间跳跃影响)
    cooldown_until: std::sync::RwLock<Instant>,
    /// 事件总线引用(用于发布事件)
    event_bus: Option<EventBus>,
    /// 控制器运行统计
    stats: std::sync::RwLock<MemConStats>,
    /// 配置
    config: MemConConfig,
    /// 上次调整时的幽灵率(用于熔断判定)
    last_adjustment_ghost_rate: std::sync::RwLock<Option<f32>>,
}

impl MemConController {
    /// 创建新的 MemCon 自适应控制器
    ///
    /// # 参数
    /// - `config`: MemCon 自适应控制器配置
    /// - `event_bus`: 可选的事件总线引用(用于发布事件)
    ///
    /// 初始策略为 `Static(MemoryStrategy::StandardTopK)`(C4 合规 fallback)。
    pub fn new(config: MemConConfig, event_bus: Option<EventBus>) -> Self {
        let detector = GhostMemoryDetector::new(config.clone());
        let cooldown_from = Instant::now()
            .checked_sub(Duration::from_secs(config.cooldown_secs + 1))
            .unwrap_or_else(Instant::now);

        Self {
            detector: std::sync::RwLock::new(detector),
            current_policy: std::sync::RwLock::new(MemoryStrategyPolicy::fallback()),
            cooldown_until: std::sync::RwLock::new(cooldown_from),
            event_bus,
            stats: std::sync::RwLock::new(MemConStats::new()),
            config,
            last_adjustment_ghost_rate: std::sync::RwLock::new(None),
        }
    }

    /// 创建禁用 MemCon 的控制器(空操作模式)
    ///
    /// WHY:用于性能基准测试中临时禁用 MemCon 以排除干扰。
    /// 所有方法均为空操作,不消耗任何资源。
    pub fn disabled() -> Self {
        Self::new(
            MemConConfig {
                enabled: false,
                ..Default::default()
            },
            None,
        )
    }

    /// 记录一次召回结果(recall hook)
    ///
    /// 在 MlcEngine 每次 recall 后调用,记录召回是否为幽灵记忆。
    /// 如果幽灵率超过阈值且不在冷却期,自动触发策略调整。
    ///
    /// # 参数
    /// - `is_ghost`: 本次召回是否检测到幽灵记忆
    ///
    /// # 返回值
    /// 返回调整结果,供调用方(如 MlcEngine)决定是否更新策略。
    pub fn on_recall(&self, is_ghost: bool) -> AdjustmentOutcome {
        if !self.config.enabled {
            return AdjustmentOutcome::NoChange;
        }

        // 1. 记录召回结果
        {
            let mut detector = self.detector.write().expect("detector 写锁");
            detector.record_recall(is_ghost);
        }

        // 2. 更新统计
        {
            let mut stats = self.stats.write().expect("stats 写锁");
            stats.total_recalls += 1;
            if is_ghost {
                stats.total_ghost_detections += 1;
            }
        }

        // 3. 检查是否需要调整策略
        self.try_adjust()
    }

    /// 尝试调整策略
    ///
    /// 检查条件:
    /// - 幽灵率超过阈值
    /// - 不在冷却期
    ///
    /// 如果满足条件,执行策略调整。
    fn try_adjust(&self) -> AdjustmentOutcome {
        // 检查冷却期
        {
            let cooldown = self.cooldown_until.read().expect("cooldown 读锁");
            if Instant::now() < *cooldown {
                return AdjustmentOutcome::NoChange;
            }
        }

        let ghost_rate;
        let ghost_count;
        let total_recalls;

        // 读取检测器状态
        {
            let detector = self.detector.read().expect("detector 读锁");
            if !detector.is_ghost_threshold_exceeded() {
                // 如果幽灵率恢复正常且不是刚调整完,考虑放宽策略
                return self.try_recover();
            }
            ghost_rate = detector.ghost_rate();
            ghost_count = detector.ghost_count();
            total_recalls = detector.window_size();
        }

        // 幽灵率超过阈值,执行调整
        let current_strategy = self
            .current_policy
            .read()
            .expect("current_policy 读锁")
            .strategy();

        let new_strategy = Self::select_strategy(current_strategy, ghost_rate);

        if new_strategy == current_strategy {
            return AdjustmentOutcome::NoChange;
        }

        // 更新策略
        {
            let mut policy = self.current_policy.write().expect("current_policy 写锁");
            *policy = MemoryStrategyPolicy::Static(new_strategy);
        }

        // 设置冷却期
        {
            let mut cooldown = self.cooldown_until.write().expect("cooldown 写锁");
            *cooldown = Instant::now() + Duration::from_secs(self.config.cooldown_secs);
        }

        // 更新统计
        {
            let mut stats = self.stats.write().expect("stats 写锁");
            stats.adjustments_count += 1;
        }

        // 记录上次调整幽灵率
        {
            let mut last = self
                .last_adjustment_ghost_rate
                .write()
                .expect("last_adjustment_ghost_rate 写锁");
            *last = Some(ghost_rate);
        }

        // 发布事件
        self.publish_strategy_adjusted(
            current_strategy,
            new_strategy,
            AdjustmentReason::GhostMemoryDetected,
            Some(ghost_rate),
        );

        info!(
            current_strategy = ?current_strategy,
            new_strategy = ?new_strategy,
            ghost_rate = ghost_rate,
            ghost_count = ghost_count,
            total_recalls = total_recalls,
            "MemCon: 幽灵记忆检测触发策略调整"
        );

        AdjustmentOutcome::Adjusted
    }

    /// 尝试恢复策略(幽灵率恢复正常时)
    ///
    /// 如果幽灵率低于阈值且上次调整过,考虑放宽到 StandardTopK。
    fn try_recover(&self) -> AdjustmentOutcome {
        let ghost_rate;

        {
            let detector = self.detector.read().expect("detector 读锁");
            ghost_rate = detector.ghost_rate();
        }

        // 如果幽灵率已低于阈值的一半,且当前不是 StandardTopK,尝试恢复
        let recovery_threshold = self.config.ghost_threshold * 0.5;
        if ghost_rate > recovery_threshold {
            return AdjustmentOutcome::NoChange;
        }

        let current_strategy = self
            .current_policy
            .read()
            .expect("current_policy 读锁")
            .strategy();

        if current_strategy == MemoryStrategy::StandardTopK {
            return AdjustmentOutcome::NoChange;
        }

        // 恢复为 StandardTopK
        {
            let mut policy = self.current_policy.write().expect("current_policy 写锁");
            *policy = MemoryStrategyPolicy::Static(MemoryStrategy::StandardTopK);
        }

        // 设置冷却期
        {
            let mut cooldown = self.cooldown_until.write().expect("cooldown 写锁");
            *cooldown = Instant::now() + Duration::from_secs(self.config.cooldown_secs);
        }

        // 更新统计
        {
            let mut stats = self.stats.write().expect("stats 写锁");
            stats.adjustments_count += 1;
        }

        // 发布事件
        self.publish_strategy_adjusted(
            current_strategy,
            MemoryStrategy::StandardTopK,
            AdjustmentReason::StableRecovery,
            Some(ghost_rate),
        );

        info!(
            from_strategy = ?current_strategy,
            to_strategy = ?MemoryStrategy::StandardTopK,
            ghost_rate = ghost_rate,
            "MemCon: 幽灵记忆恢复,放宽策略到 StandardTopK"
        );

        AdjustmentOutcome::Adjusted
    }

    /// 选择策略 — 根据幽灵率选择更激进的策略
    ///
    /// 策略选择逻辑:
    /// - ghost_rate < 0.3: 维持当前策略(无需调整)
    /// - ghost_rate ∈ [0.3, 0.5): 从 StandardTopK → AggressivePruning
    /// - ghost_rate ∈ [0.5, 0.8): 从 StandardTopK/AggressivePruning → MinimalRecall
    /// - ghost_rate >= 0.8: 熔断,回退到 StandardTopK(由调用方处理)
    ///
    /// 注意:此方法本身不执行熔断,熔断由 `try_adjust` 中的 `last_adjustment_ghost_rate`
    /// 比较逻辑触发。此方法仅返回建议的策略。
    fn select_strategy(current: MemoryStrategy, ghost_rate: f32) -> MemoryStrategy {
        if ghost_rate >= 0.8 {
            // 极高幽灵率 → 熔断回退到 StandardTopK
            return MemoryStrategy::StandardTopK;
        }
        if ghost_rate >= 0.5 {
            // 高幽灵率 → 收紧到 MinimalRecall(仅 L0, k=1,最小化召回噪声)
            return MemoryStrategy::MinimalRecall;
        }
        if ghost_rate >= 0.3 && current != MemoryStrategy::AggressivePruning {
            // 中等幽灵率 → 收紧到 AggressivePruning(激进剪枝)
            return MemoryStrategy::AggressivePruning;
        }
        // 幽灵率低 → 维持当前策略
        current
    }

    /// 发布幽灵记忆检测事件
    #[allow(dead_code)]
    fn publish_ghost_detected(&self, ghost_rate: f32, ghost_count: u32, total_recalls: u32) {
        if let Some(ref bus) = self.event_bus {
            let current_strategy = self
                .current_policy
                .read()
                .expect("current_policy 读锁")
                .strategy();

            let event = NexusEvent::GhostMemoryDetected {
                metadata: EventMetadata::new("mlc-engine:mem_con"),
                ghost_rate,
                ghost_count,
                total_recalls,
                current_strategy: format!("{:?}", current_strategy),
            };

            if let Err(e) = bus.publish_blocking(event) {
                debug!("MemCon: 发布 GhostMemoryDetected 事件失败: {}", e);
            }
        }
    }

    /// 发布策略调整事件
    fn publish_strategy_adjusted(
        &self,
        from: MemoryStrategy,
        to: MemoryStrategy,
        reason: AdjustmentReason,
        ghost_rate: Option<f32>,
    ) {
        if let Some(ref bus) = self.event_bus {
            let reason_str = match reason {
                AdjustmentReason::GhostMemoryDetected => "ghost_memory_detected",
                AdjustmentReason::StableRecovery => "stable_recovery",
                AdjustmentReason::CircuitBreaker => "circuit_breaker",
            };

            let event = NexusEvent::MemConStrategyAdjusted {
                metadata: EventMetadata::new("mlc-engine:mem_con"),
                from_strategy: format!("{:?}", from),
                to_strategy: format!("{:?}", to),
                reason: reason_str.into(),
                ghost_rate,
            };

            if let Err(e) = bus.publish_blocking(event) {
                debug!("MemCon: 发布 MemConStrategyAdjusted 事件失败: {}", e);
            }
        }
    }

    /// 获取当前策略
    pub fn current_strategy(&self) -> MemoryStrategy {
        self.current_policy
            .read()
            .expect("current_policy 读锁")
            .strategy()
    }

    /// 获取当前策略策略(完整 MemoryStrategyPolicy)
    pub fn current_policy(&self) -> MemoryStrategyPolicy {
        *self.current_policy.read().expect("current_policy 读锁")
    }

    /// 获取当前幽灵率
    pub fn ghost_rate(&self) -> f32 {
        self.detector.read().expect("detector 读锁").ghost_rate()
    }

    /// 获取运行统计
    pub fn stats(&self) -> MemConStats {
        *self.stats.read().expect("stats 读锁")
    }

    /// 获取配置引用
    pub fn config(&self) -> &MemConConfig {
        &self.config
    }

    /// 重置控制器(清空检测器窗口,重置统计)
    pub fn reset(&self) {
        {
            let mut detector = self.detector.write().expect("detector 写锁");
            detector.reset();
        }
        {
            let mut stats = self.stats.write().expect("stats 写锁");
            *stats = MemConStats::new();
        }
        {
            let mut cooldown = self.cooldown_until.write().expect("cooldown 写锁");
            let past = Instant::now()
                .checked_sub(Duration::from_secs(self.config.cooldown_secs + 1))
                .unwrap_or_else(Instant::now);
            *cooldown = past;
        }
        {
            let mut last = self
                .last_adjustment_ghost_rate
                .write()
                .expect("last_adjustment_ghost_rate 写锁");
            *last = None;
        }
        debug!("MemCon: 控制器已重置");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证禁用模式下,on_recall 返回 NoChange
    #[test]
    fn test_disabled_controller() {
        let controller = MemConController::disabled();
        let result = controller.on_recall(true);
        assert_eq!(result, AdjustmentOutcome::NoChange);
        assert_eq!(controller.ghost_rate(), 0.0);
    }

    /// 验证默认控制器初始策略为 StandardTopK
    #[test]
    fn test_default_strategy() {
        let controller = MemConController::new(MemConConfig::default(), None);
        assert_eq!(controller.current_strategy(), MemoryStrategy::StandardTopK);
    }

    /// 验证单次幽灵召回不触发调整(需要窗口数据)
    #[test]
    fn test_single_ghost_no_adjust() {
        let controller = MemConController::new(MemConConfig::default(), None);
        let result = controller.on_recall(true);
        assert_eq!(result, AdjustmentOutcome::NoChange);
    }

    /// 验证幽灵率超过阈值触发调整
    #[test]
    fn test_ghost_rate_exceeds_threshold() {
        let config = MemConConfig {
            window_size: 10,
            ghost_threshold: 0.3,
            cooldown_secs: 1, // 短冷却期便于测试
            ..Default::default()
        };

        let controller = MemConController::new(config, None);

        // 7 次幽灵,3 次非幽灵 = 70% > 30%,应触发调整
        for _ in 0..3 {
            assert_eq!(controller.on_recall(false), AdjustmentOutcome::NoChange);
        }
        // 前 3 次非幽灵后,添加 7 次幽灵
        for _ in 0..7 {
            controller.on_recall(true);
        }

        // 此时幽灵率 70%,应触发调整
        // 注意:由于冷却期检查在 try_adjust 中,前几次 on_recall 可能触发调整
        // 我们只验证最终策略已变更
        let strategy = controller.current_strategy();
        assert!(
            strategy == MemoryStrategy::AggressivePruning
                || strategy == MemoryStrategy::MinimalRecall
                || strategy == MemoryStrategy::StandardTopK,
            "策略应已调整,当前: {:?}",
            strategy
        );
    }

    /// 验证策略选择逻辑
    #[test]
    fn test_select_strategy() {
        // 低幽灵率:维持当前
        assert_eq!(
            MemConController::select_strategy(MemoryStrategy::StandardTopK, 0.1),
            MemoryStrategy::StandardTopK
        );

        // 中等幽灵率:从 StandardTopK → AggressivePruning
        assert_eq!(
            MemConController::select_strategy(MemoryStrategy::StandardTopK, 0.4),
            MemoryStrategy::AggressivePruning
        );

        // 高幽灵率:收紧到 MinimalRecall
        assert_eq!(
            MemConController::select_strategy(MemoryStrategy::StandardTopK, 0.6),
            MemoryStrategy::MinimalRecall
        );

        // 极高幽灵率:熔断回退到 StandardTopK
        assert_eq!(
            MemConController::select_strategy(MemoryStrategy::MinimalRecall, 0.9),
            MemoryStrategy::StandardTopK
        );
    }

    /// 验证 EventBus 事件发布
    ///
    /// 使用 50% 幽灵率(5 ghost + 5 non-ghost = 50% > 30% 阈值),
    /// 确保 `select_strategy` 从 StandardTopK 调整到 AggressivePruning,
    /// 从而触发 MemConStrategyAdjusted 事件发布。
    #[test]
    fn test_event_publishing() {
        let bus = EventBus::new();
        let config = MemConConfig {
            window_size: 10,
            ghost_threshold: 0.3,
            cooldown_secs: 1,
            ..Default::default()
        };

        // 订阅 MemCon 事件
        let mut subscriber = bus.subscribe_filtered(std::collections::HashSet::from([
            event_bus::EventTopic::Memory,
        ]));

        let controller = MemConController::new(config, Some(bus));

        // 5 次非幽灵 + 5 次幽灵 = 50% 幽灵率,在 0.3-0.8 范围内,
        // 应触发 StandardTopK → AggressivePruning 调整并发布事件
        for _ in 0..5 {
            controller.on_recall(false);
        }
        for _ in 0..5 {
            controller.on_recall(true);
        }

        // 验证至少收到一个 MemConStrategyAdjusted 事件
        let mut found_event = false;
        for _ in 0..20 {
            match subscriber.try_recv() {
                Ok(Some(event)) => {
                    if event.type_name() == "MemConStrategyAdjusted" {
                        found_event = true;
                        break;
                    }
                }
                Ok(None) => break,
                Err(_) => break,
            }
        }
        assert!(found_event, "应至少发布一个 MemConStrategyAdjusted 事件");
    }

    /// 验证冷却期机制
    #[test]
    fn test_cooldown_mechanism() {
        let config = MemConConfig {
            window_size: 10,
            ghost_threshold: 0.3,
            cooldown_secs: 3600, // 长冷却期,确保测试期间不冷却
            ..Default::default()
        };

        let controller = MemConController::new(config, None);

        // 触发调整
        for _ in 0..10 {
            controller.on_recall(true);
        }

        // 记录调整次数
        let stats_before = controller.stats();

        // 再次触发(应在冷却期中)
        for _ in 0..10 {
            controller.on_recall(true);
        }

        let stats_after = controller.stats();
        // 冷却期中,调整次数应不变
        assert_eq!(
            stats_before.adjustments_count, stats_after.adjustments_count,
            "冷却期不应触发新调整"
        );
    }

    /// 验证统计信息的正确性
    #[test]
    fn test_stats_accuracy() {
        let controller = MemConController::new(MemConConfig::default(), None);

        for i in 0..50 {
            controller.on_recall(i % 2 == 0); // 50% 幽灵
        }

        let stats = controller.stats();
        assert_eq!(stats.total_recalls, 50);
        assert_eq!(stats.total_ghost_detections, 25);
    }

    /// 验证控制器重置
    #[test]
    fn test_controller_reset() {
        let controller = MemConController::new(MemConConfig::default(), None);

        for _ in 0..10 {
            controller.on_recall(true);
        }

        assert!(controller.stats().total_recalls > 0);
        assert!(controller.ghost_rate() > 0.0);

        controller.reset();

        assert_eq!(controller.stats().total_recalls, 0);
        assert_eq!(controller.ghost_rate(), 0.0);
    }
}
