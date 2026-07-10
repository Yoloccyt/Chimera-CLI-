//! ACB 治理器主逻辑 — 预算检查、级别调整与消耗统计
//!
//! 对应架构层:L8 Parliament
//! 对应创新点:ACB(Adaptive Cognitive Budget)
//!
//! # 设计决策(WHY)
//! - `current_tier` 用 `Mutex<BudgetTier>`:级别判定与切换是 check-then-act 模式,
//!   必须原子化(§6 架构红线:竞态防护)
//! - `total_consumption` 用 `AtomicU64`:无锁计数,高频写入无争抢
//! - `event_bus` 为 `EventBus`(内部 `Arc` 引用计数,Clone 廉价):后台监控
//!   任务持 Clone 副本发布事件,与主线程无争抢

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use event_bus::{EventBus, EventMetadata, NexusEvent};
use tracing::{info, warn};

use crate::config::AcbGovernorConfig;
use crate::error::AcbError;
use crate::types::{BudgetRequest, BudgetStatus, BudgetTier, TierSwitchResult};

/// ACB 治理器 — 自适应认知预算治理核心
///
/// 维护当前预算级别与累计消耗,提供:
/// - 预算检查(请求是否在当前级别预算内)
/// - 级别自动调整(基于利用率阈值,带滞后带)
/// - 预算消耗统计(累计消耗、利用率、剩余预算)
///
/// # 线程安全
/// - `current_tier` 用 `Mutex<BudgetTier>` 保护,check-then-act 原子化
/// - `total_consumption` 用 `AtomicU64` 保护,无锁累加
/// - `event_bus` 为 `EventBus`(内部 `Arc` 引用计数,Clone 廉价)
pub struct AcbGovernor {
    /// ACB 配置(只读,构造后不变)
    config: AcbGovernorConfig,
    /// 当前预算级别(check-then-act 原子化)
    current_tier: Mutex<BudgetTier>,
    /// 累计消耗(无锁计数)
    total_consumption: AtomicU64,
    /// 事件总线(发布 BudgetAdjusted/BudgetExceeded 事件)
    ///
    /// WHY:Ω-Event 定律要求所有状态变更经 EventBus 广播,打破"仅 tracing 日志"
    /// 的断裂。`EventBus` 内部为 `Arc<broadcast::Sender>`,Clone 廉价。
    event_bus: EventBus,
    /// 上次级别切换时间(UTC),用于时间滞后机制
    ///
    /// WHY 复用 DECB TierState 模式而非抽象共享 trait:ACB 与 DECB 的 `BudgetTier`
    /// 枚举不同(ACB=L0-L3 离散四级,DECB=HighTier/LowTier/Degraded 连续三档),
    /// 强行抽象会引入泛型噪声与 phantom 类型,违背"避免过度工程化"原则。复制模式
    /// 更直观,且 lag 检查逻辑极简(约 10 行),重复成本低于抽象成本。
    last_switch_time: Mutex<Option<DateTime<Utc>>>,
}

impl AcbGovernor {
    /// 创建新的 ACB 治理器(内部创建私有 EventBus,仅用于测试)
    ///
    /// WHY 保留 `new()`:测试代码用 `new()` 创建私有总线,`publish` 静默丢弃,
    /// 不影响测试逻辑(无订阅者时 broadcast::send 返回 Err,被 publish 静默吞掉)。
    /// 生产代码改用 [`with_event_bus`](Self::with_event_bus) 注入共享总线。
    ///
    /// # 错误
    /// - `ConfigError`:配置校验失败(级别上限倒挂、阈值为负等)
    pub fn new(config: AcbGovernorConfig) -> Result<Self, AcbError> {
        Self::with_event_bus(config, EventBus::new())
    }

    /// 创建带共享 EventBus 的 ACB 治理器(生产代码推荐)
    ///
    /// WHY:生产代码需注入共享总线,使 `BudgetAdjusted` 等事件能被 Parliament/Quest
    /// 订阅。初始级别为 L3(充足),保证系统启动时资源充沛。
    ///
    /// # 错误
    /// - `ConfigError`:配置校验失败(级别上限倒挂、阈值为负等)
    pub fn with_event_bus(config: AcbGovernorConfig, bus: EventBus) -> Result<Self, AcbError> {
        config.validate()?;
        Ok(Self {
            config,
            current_tier: Mutex::new(BudgetTier::L3),
            total_consumption: AtomicU64::new(0),
            event_bus: bus,
            // WHY None:首次启动无历史切换时间,首次切换不受滞后约束
            last_switch_time: Mutex::new(None),
        })
    }

    /// EventBus 访问器(供测试与上层共享总线)
    pub fn event_bus(&self) -> &EventBus {
        &self.event_bus
    }

    /// 返回配置引用(测试与监控用)
    pub fn config(&self) -> &AcbGovernorConfig {
        &self.config
    }

    /// 返回当前预算级别
    ///
    /// 锁中毒时安全降级到 L0(§6 架构红线:竞态防护)。
    pub fn current_tier(&self) -> BudgetTier {
        self.current_tier.lock().map(|t| *t).unwrap_or_else(|e| {
            tracing::error!("current_tier lock poisoned: {e}");
            BudgetTier::L0
        })
    }

    /// 检查预算请求是否在当前级别预算内
    ///
    /// # 流程
    /// 1. 获取当前级别与对应 Token 上限
    /// 2. 比较请求消耗与上限
    /// 3. 超限时返回 `BudgetExceeded`(不自动降级,由调用方决策)
    ///
    /// # 错误
    /// - `BudgetExceeded`:请求消耗超过当前级别上限
    /// - `DegradedModeRejected`:当前为 L0 且请求超限(最低级别无法降级)
    pub fn check_budget(&self, request: &BudgetRequest) -> Result<(), AcbError> {
        let tier = self.current_tier();
        let limit = self.config.token_limit_for(tier);

        if request.requested_tokens > limit {
            // L0 已是最低级别,无法降级,直接拒绝
            if tier == BudgetTier::L0 {
                return Err(AcbError::DegradedModeRejected {
                    quest_id: request.quest_id.clone(),
                    reason: format!("L0 limit={}, requested={}", request.requested_tokens, limit),
                });
            }

            // 发布 BudgetExceeded 事件(Ω-Event 定律)
            // WHY publish_blocking:check_budget 为同步方法,且被同步 #[test] 调用;
            // publish_blocking 不依赖 tokio 运行时,同步测试不 panic。
            let event = NexusEvent::BudgetExceeded {
                metadata: EventMetadata::new("acb-governor"),
                budget_type: "token".to_string(),
                current: request.requested_tokens,
                limit,
            };
            if let Err(e) = self.event_bus.publish_blocking(event) {
                warn!(error = %e, "发布 BudgetExceeded 事件失败");
            }

            return Err(AcbError::BudgetExceeded {
                quest_id: request.quest_id.clone(),
                current: request.requested_tokens,
                limit,
            });
        }
        Ok(())
    }

    /// 记录预算消耗并触发自动级别调整
    ///
    /// # 流程
    /// 1. 累加消耗到 total_consumption(原子操作)
    /// 2. 计算当前利用率
    /// 3. 根据阈值自动降级或升级(带滞后带)
    /// 4. 级别变化时发布 `BudgetAdjusted` 事件(Ω-Event 定律)
    ///
    /// # 错误
    /// - `ConfigError`:锁中毒
    /// - `DegradedModeRejected`:L0 级别下消耗后仍超预算
    pub fn record_consumption(&self, consumed_tokens: u64) -> Result<(), AcbError> {
        // 步骤 1:原子累加消耗
        let current_total = self
            .total_consumption
            .fetch_add(consumed_tokens, Ordering::Relaxed)
            + consumed_tokens;

        // 步骤 2:检查总预算超限
        if current_total > self.config.total_budget_limit {
            let tier = self.current_tier();
            if tier == BudgetTier::L0 {
                return Err(AcbError::DegradedModeRejected {
                    quest_id: String::new(),
                    reason: format!(
                        "total budget exhausted in L0: {current_total} / {}",
                        self.config.total_budget_limit
                    ),
                });
            }
        }

        // 步骤 3:自动级别调整
        self.adjust_budget()?;
        Ok(())
    }

    /// 根据当前利用率自动调整预算级别
    ///
    /// # 调整规则(滞后带机制)
    /// - 利用率 > `degrade_threshold` → 降级一级(避免预算耗尽)
    /// - 利用率 < `upgrade_threshold` → 升级一级(释放更多资源)
    /// - 中间区间保持当前级别(稳定带,避免抖动)
    ///
    /// # 错误
    /// - `ConfigError`:锁中毒
    pub fn adjust_budget(&self) -> Result<TierSwitchResult, AcbError> {
        let utilization = self.utilization_rate();
        let old_tier = self.current_tier();

        // 根据利用率决定目标级别
        let target_tier = if utilization > self.config.degrade_threshold {
            // 高利用率,降级
            old_tier.degrade()
        } else if utilization < self.config.upgrade_threshold {
            // 低利用率,升级
            old_tier.upgrade()
        } else {
            // 稳定带,保持当前级别
            old_tier
        };

        // 级别相同,无需切换
        if target_tier == old_tier {
            return Ok(TierSwitchResult {
                from_tier: old_tier,
                to_tier: old_tier,
                switched: false,
            });
        }

        // WHY 时间滞后:切换后 tier_switch_lag_ms 内不再次切换,防止阈值附近抖动。
        // 复用 DECB switch_tier 模式(§6 架构红线:竞态/抖动防护)。
        // 锁内 check-then-act 原子化:检查 elapsed 与更新 last_switch_time 在同一锁内,
        // 避免并发 adjust_budget 双双通过滞后检查后双重切换。锁在 `}` 释放,不跨 await。
        {
            let mut last = self
                .last_switch_time
                .lock()
                .map_err(|e| AcbError::ConfigError {
                    detail: format!("last_switch_time lock poisoned: {e}"),
                })?;
            if let Some(last_switch) = *last {
                let elapsed = Utc::now().signed_duration_since(last_switch);
                let lag = chrono::Duration::milliseconds(self.config.tier_switch_lag_ms as i64);
                if elapsed < lag {
                    // 滞后期内,静默抑制切换(非错误,是预期防抖行为)
                    info!(
                        old_tier = %old_tier,
                        target_tier = %target_tier,
                        lag_ms = self.config.tier_switch_lag_ms,
                        "ACB tier switch suppressed by lag"
                    );
                    return Ok(TierSwitchResult {
                        from_tier: old_tier,
                        to_tier: old_tier,
                        switched: false,
                    });
                }
            }
            // 更新切换时间戳(同一锁内,check-then-act 原子化)
            *last = Some(Utc::now());
        }

        // 原子切换级别(check-then-act)
        {
            let mut tier_guard = self
                .current_tier
                .lock()
                .map_err(|e| AcbError::ConfigError {
                    detail: format!("current_tier lock poisoned: {e}"),
                })?;
            *tier_guard = target_tier;
        }

        info!(
            old_tier = %old_tier,
            new_tier = %target_tier,
            utilization,
            "ACB budget tier adjusted"
        );

        // 发布 BudgetAdjusted 事件(Ω-Event 定律)
        // WHY coefficient 用级别归一化值:L0=0.25, L1=0.5, L2=0.75, L3=1.0,
        // 表达"当前级别相对最高级别的资源比例"。
        // WHY publish_blocking:adjust_budget 为同步方法,且被同步 #[test] 调用。
        let coefficient = tier_to_coefficient(target_tier);
        let event = NexusEvent::BudgetAdjusted {
            metadata: EventMetadata::new("acb-governor"),
            quest_id: String::new(),
            old_tier: old_tier.to_string(),
            new_tier: target_tier.to_string(),
            coefficient,
            reason: format!(
                "utilization {:.2} {} threshold",
                utilization,
                if target_tier.as_level() < old_tier.as_level() {
                    "exceeds degrade"
                } else {
                    "below upgrade"
                }
            ),
        };
        if let Err(e) = self.event_bus.publish_blocking(event) {
            warn!(error = %e, "发布 BudgetAdjusted 事件失败");
        }

        Ok(TierSwitchResult {
            from_tier: old_tier,
            to_tier: target_tier,
            switched: true,
        })
    }

    /// 计算当前利用率 [0.0, 1.0]
    ///
    /// `total_consumption / total_budget_limit`,clamp 到 [0.0, 1.0]。
    pub fn utilization_rate(&self) -> f32 {
        let total = self.total_consumption.load(Ordering::Relaxed);
        if self.config.total_budget_limit == 0 {
            return 1.0;
        }
        let rate = total as f64 / self.config.total_budget_limit as f64;
        (rate.clamp(0.0, 1.0)) as f32
    }

    /// 返回当前预算状态快照
    pub fn get_status(&self) -> BudgetStatus {
        let total_consumption = self.total_consumption.load(Ordering::Relaxed);
        let remaining_budget = self
            .config
            .total_budget_limit
            .saturating_sub(total_consumption);
        let utilization_rate = self.utilization_rate();
        let current_tier = self.current_tier();

        BudgetStatus {
            current_tier,
            total_consumption,
            remaining_budget,
            utilization_rate,
        }
    }

    /// 重置预算(用于周期重置)
    ///
    /// 清零累计消耗,级别恢复到 L3(充足)。
    pub fn reset_budget(&self) {
        self.total_consumption.store(0, Ordering::Relaxed);
        if let Ok(mut tier) = self.current_tier.lock() {
            *tier = BudgetTier::L3;
        }
        // WHY 同步清零 last_switch_time:reset 语义为"恢复初始状态",
        // 初始状态无历史切换时间,reset 后首次切换不应受滞后约束
        if let Ok(mut last) = self.last_switch_time.lock() {
            *last = None;
        }
        info!("ACB budget reset to initial state (L3)");
    }
}

/// 将级别转换为归一化系数 [0.0, 1.0]
///
/// L0=0.25, L1=0.5, L2=0.75, L3=1.0
/// WHY:用于 BudgetAdjusted 事件的 coefficient 字段,表达"当前级别相对
/// 最高级别的资源比例",与 DECB 的连续系数语义对齐。
fn tier_to_coefficient(tier: BudgetTier) -> f32 {
    match tier {
        BudgetTier::L0 => 0.25,
        BudgetTier::L1 => 0.5,
        BudgetTier::L2 => 0.75,
        BudgetTier::L3 => 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_governor() -> AcbGovernor {
        AcbGovernor::new(AcbGovernorConfig::default()).unwrap()
    }

    // ============================================================
    // 预算检查测试
    // ============================================================

    #[test]
    fn test_check_budget_within_limit() {
        let governor = make_governor();
        // L3 默认,上限 100000,请求 1000 在范围内
        let request = BudgetRequest::new("quest-1", 1000);
        assert!(governor.check_budget(&request).is_ok());
    }

    #[test]
    fn test_check_budget_exceeds_limit() {
        let governor = make_governor();
        // L3 上限 100000,请求 200000 超限
        let request = BudgetRequest::new("quest-1", 200_000);
        let result = governor.check_budget(&request);
        assert!(
            matches!(result, Err(AcbError::BudgetExceeded { .. })),
            "should reject request exceeding L3 limit"
        );
    }

    #[test]
    fn test_check_budget_degraded_mode_rejected() {
        let governor = make_governor();
        // 手动降到 L0
        {
            let mut tier = governor.current_tier.lock().unwrap();
            *tier = BudgetTier::L0;
        }
        // L0 上限 1000,请求 2000 超限,应返回 DegradedModeRejected
        let request = BudgetRequest::new("quest-1", 2000);
        let result = governor.check_budget(&request);
        assert!(
            matches!(result, Err(AcbError::DegradedModeRejected { .. })),
            "L0 should reject with DegradedModeRejected"
        );
    }

    // ============================================================
    // 级别调整测试
    // ============================================================

    #[test]
    fn test_adjust_budget_high_utilization_degrades() {
        let governor = make_governor();
        // 消耗到 85% > degrade_threshold(0.8),应降级 L3 → L2
        let consume = (governor.config.total_budget_limit as f64 * 0.85) as u64;
        governor.total_consumption.store(consume, Ordering::Relaxed);

        let result = governor.adjust_budget().unwrap();
        assert!(result.switched);
        assert_eq!(result.from_tier, BudgetTier::L3);
        assert_eq!(result.to_tier, BudgetTier::L2);
        assert_eq!(governor.current_tier(), BudgetTier::L2);
    }

    #[test]
    fn test_adjust_budget_low_utilization_upgrades() {
        let governor = make_governor();
        // 先降到 L2
        {
            let mut tier = governor.current_tier.lock().unwrap();
            *tier = BudgetTier::L2;
        }
        // 消耗 20% < upgrade_threshold(0.3),应升级 L2 → L3
        let consume = (governor.config.total_budget_limit as f64 * 0.20) as u64;
        governor.total_consumption.store(consume, Ordering::Relaxed);

        let result = governor.adjust_budget().unwrap();
        assert!(result.switched);
        assert_eq!(result.from_tier, BudgetTier::L2);
        assert_eq!(result.to_tier, BudgetTier::L3);
    }

    #[test]
    fn test_adjust_budget_stable_band_no_switch() {
        let governor = make_governor();
        // 消耗 50%,在稳定带(0.3, 0.8)内,不应切换
        let consume = (governor.config.total_budget_limit as f64 * 0.50) as u64;
        governor.total_consumption.store(consume, Ordering::Relaxed);

        let result = governor.adjust_budget().unwrap();
        assert!(!result.switched);
        assert_eq!(governor.current_tier(), BudgetTier::L3);
    }

    // ============================================================
    // 消耗记录测试
    // ============================================================

    #[test]
    fn test_record_consumption_accumulates() {
        let governor = make_governor();
        governor.record_consumption(1000).unwrap();
        governor.record_consumption(2000).unwrap();
        assert_eq!(governor.total_consumption.load(Ordering::Relaxed), 3000);
    }

    #[test]
    fn test_record_consumption_triggers_degradation() {
        let governor = make_governor();
        // 一次性消耗 85%,触发降级
        let consume = (governor.config.total_budget_limit as f64 * 0.85) as u64;
        governor.record_consumption(consume).unwrap();
        assert_eq!(governor.current_tier(), BudgetTier::L2);
    }

    // ============================================================
    // 状态与重置测试
    // ============================================================

    #[test]
    fn test_get_status_initial() {
        let governor = make_governor();
        let status = governor.get_status();
        assert_eq!(status.current_tier, BudgetTier::L3);
        assert_eq!(status.total_consumption, 0);
        assert_eq!(status.remaining_budget, governor.config.total_budget_limit);
        assert!((status.utilization_rate - 0.0).abs() < 1e-6);
    }

    #[test]
    fn test_reset_budget() {
        let governor = make_governor();
        governor.record_consumption(100_000).unwrap();
        governor.reset_budget();
        let status = governor.get_status();
        assert_eq!(status.total_consumption, 0);
        assert_eq!(status.current_tier, BudgetTier::L3);
    }

    #[test]
    fn test_utilization_rate_calculation() {
        let governor = make_governor();
        governor.total_consumption.store(500_000, Ordering::Relaxed);
        let rate = governor.utilization_rate();
        assert!(
            (rate - 0.5).abs() < 1e-3,
            "50% consumption should yield 0.5 rate"
        );
    }

    // ============================================================
    // 配置错误测试
    // ============================================================

    #[test]
    fn test_invalid_config_rejected() {
        let config = AcbGovernorConfig {
            l1_token_limit: 500, // L1 < L0,违反递增
            ..Default::default()
        };
        assert!(AcbGovernor::new(config).is_err());
    }

    #[test]
    fn test_tier_to_coefficient() {
        assert!((tier_to_coefficient(BudgetTier::L0) - 0.25).abs() < 1e-6);
        assert!((tier_to_coefficient(BudgetTier::L1) - 0.5).abs() < 1e-6);
        assert!((tier_to_coefficient(BudgetTier::L2) - 0.75).abs() < 1e-6);
        assert!((tier_to_coefficient(BudgetTier::L3) - 1.0).abs() < 1e-6);
    }

    // ============================================================
    // 时间滞后机制测试(WHY 复用 DECB tier_switch_lag_ms 模式)
    // ============================================================

    #[test]
    fn test_adjust_budget_lag_default_1000ms() {
        // WHY 默认 1000ms:阈值滞后带的补充冷却,比 DECB 的 10s 短(ACB 恢复敏感度更高)
        let config = AcbGovernorConfig::default();
        assert_eq!(config.tier_switch_lag_ms, 1_000, "默认时间滞后应为 1000ms");
    }

    #[test]
    fn test_adjust_budget_lag_suppresses_rapid_switch() {
        // 场景:降级后立即出现"可升级"信号,但滞后期内不应切换
        let config = AcbGovernorConfig {
            tier_switch_lag_ms: 10_000,
            ..Default::default()
        };
        let governor = AcbGovernor::new(config).unwrap();

        // 步骤 1:消耗到 85% > degrade_threshold(0.8),触发降级 L3→L2
        let degrade_consume = (governor.config.total_budget_limit as f64 * 0.85) as u64;
        governor
            .total_consumption
            .store(degrade_consume, Ordering::Relaxed);
        let result = governor.adjust_budget().unwrap();
        assert!(result.switched, "首次降级应成功(无历史时间戳,不受滞后约束)");
        assert_eq!(result.from_tier, BudgetTier::L3);
        assert_eq!(result.to_tier, BudgetTier::L2);

        // 步骤 2:利用率降至 20% < upgrade_threshold(0.3),目标应为升级 L2→L3
        let upgrade_consume = (governor.config.total_budget_limit as f64 * 0.20) as u64;
        governor
            .total_consumption
            .store(upgrade_consume, Ordering::Relaxed);

        // 步骤 3:立即再次调用,距上次切换不足 10s,应被滞后机制抑制
        let result2 = governor.adjust_budget().unwrap();
        assert!(!result2.switched, "滞后期内不应切换(距上次切换 < 10s)");
        assert_eq!(
            result2.to_tier,
            BudgetTier::L2,
            "被抑制时 to_tier 应保持当前级别"
        );
        assert_eq!(governor.current_tier(), BudgetTier::L2);
    }

    #[test]
    fn test_adjust_budget_lag_expired_allows_switch() {
        use std::thread::sleep;
        use std::time::Duration;

        // 场景:滞后极短(1ms),sleep 20ms 后应允许切换
        let config = AcbGovernorConfig {
            tier_switch_lag_ms: 1,
            ..Default::default()
        };
        let governor = AcbGovernor::new(config).unwrap();

        // 步骤 1:降级 L3→L2
        let degrade_consume = (governor.config.total_budget_limit as f64 * 0.85) as u64;
        governor
            .total_consumption
            .store(degrade_consume, Ordering::Relaxed);
        let result = governor.adjust_budget().unwrap();
        assert!(result.switched);
        assert_eq!(result.to_tier, BudgetTier::L2);

        // 步骤 2:利用率降至 20%,目标升级
        let upgrade_consume = (governor.config.total_budget_limit as f64 * 0.20) as u64;
        governor
            .total_consumption
            .store(upgrade_consume, Ordering::Relaxed);

        // 步骤 3:等待滞后过期(20ms > 1ms lag)
        sleep(Duration::from_millis(20));

        // 步骤 4:滞后已过期,应成功升级 L2→L3
        let result2 = governor.adjust_budget().unwrap();
        assert!(result2.switched, "滞后过期后应允许切换");
        assert_eq!(result2.to_tier, BudgetTier::L3);
        assert_eq!(governor.current_tier(), BudgetTier::L3);
    }
}
