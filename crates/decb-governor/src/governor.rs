//! DECB 治理器主逻辑 — 预算系数计算、档位切换、溢出检测与消耗统计
//!
//! 对应架构层:L8 Parliament
//! 对应创新点:DECB(Dual-tier Cognitive Budget)
//!
//! # 设计决策(WHY)
//! - `tier_state` 用 `Arc<Mutex<TierState>>`:档位判定与切换是 check-then-act 模式,
//!   必须原子化(§6 架构红线:竞态防护)。Arc 允许后台溢出监控任务共享状态。
//! - `total_consumption` 用 `Arc<Mutex<f64>>`:消耗累加是读-改-写,需原子性。
//!   Arc 允许后台监控任务读取当前消耗。
//! - `consumption_count` 用 `AtomicU64`:无锁计数,高频写入无争抢。
//! - `current_coefficient` 用 `Mutex<f32>`:仅 compute_budget 写,get_stats 读,
//!   读写均衡用 Mutex(§4.3 锁策略)。
//! - `OverflowDetector` 为 Clone:后台任务持有独立副本,避免与主线程争抢。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use event_bus::{EventBus, EventMetadata, NexusEvent};
use tracing::{error, info, warn};

use crate::config::DecbConfig;
use crate::error::DecbError;
use crate::overflow::OverflowDetector;
use crate::types::{BudgetConsumption, BudgetStats, BudgetTier, QuestBudgetInput};

/// 每 N 次消耗记录发布一次统计
const STATS_REPORT_INTERVAL: u64 = 100;

/// 1 小时秒数(紧急度计算用)
const ONE_HOUR_SECS: i64 = 3_600;
/// 1 天秒数(紧急度计算用)
const ONE_DAY_SECS: i64 = 86_400;

/// 档位状态(原子操作单元,包含当前档位与上次切换时间)
///
/// WHY 合并到一个结构体:switch_tier 需要同时读取当前档位、检查滞后时间、
/// 更新档位与切换时间,必须在同一个锁中完成,保证 check-then-act 原子性。
#[derive(Debug)]
struct TierState {
    /// 当前档位
    current_tier: BudgetTier,
    /// 上次档位切换时间(UTC),用于滞后机制
    last_switch_time: Option<DateTime<Utc>>,
}

/// DECB 治理器 — 双档认知预算治理核心
///
/// 维护预算系数、档位状态与消耗统计,提供:
/// - 连续可调预算系数计算(基于复杂度/紧急度/剩余预算)
/// - 高低档自动切换(带滞后机制,防止抖动)
/// - 预算溢出检测与降级(后台监控 + 自动降级链路)
/// - 预算消耗统计(累计消耗、利用率、剩余预算)
///
/// # 线程安全
/// - `tier_state` 用 `Arc<Mutex<TierState>>` 保护,check-then-act 原子化
/// - `total_consumption` 用 `Arc<Mutex<f64>>` 保护,读-改-写原子化
/// - `consumption_count` 用原子计数器,无锁统计
/// - `current_coefficient` 用 `Mutex<f32>` 保护,读写均衡
/// - `event_bus` 为 `EventBus`(内部 `Arc` 引用计数,Clone 廉价):后台溢出监控
///   任务持 Clone 副本发布事件,与主线程无争抢
pub struct DecbGovernor {
    /// DECB 配置(只读,构造后不变)
    config: DecbConfig,
    /// 档位状态(包含当前档位与上次切换时间,原子操作)
    tier_state: Arc<Mutex<TierState>>,
    /// 累计消耗(读写均衡,用 Mutex 保护)
    total_consumption: Arc<Mutex<f64>>,
    /// 消耗记录计数(用于每 100 次发布统计)
    consumption_count: AtomicU64,
    /// 当前预算系数(用于统计快照)
    current_coefficient: Mutex<f32>,
    /// 溢出检测器(Clone 到后台任务)
    overflow_detector: OverflowDetector,
    /// 事件总线(发布 BudgetAdjusted/BudgetExceeded/BudgetStatsReported 事件)
    ///
    /// WHY:Ω-Event 定律要求所有状态变更经 EventBus 广播,打破"仅 tracing 日志"
    /// 的断裂。`EventBus` 内部为 `Arc<broadcast::Sender>`,Clone 廉价,后台任务
    /// 可持副本独立发布,避免与主线程争抢锁。
    event_bus: EventBus,
}

impl DecbGovernor {
    /// 创建新的 DECB 治理器(内部创建私有 EventBus,仅用于测试)
    ///
    /// WHY 保留 `new()`:DecbGovernor 有 27 处测试调用点,保留 `new()` 零测试修改。
    /// 生产代码(Week 6 集成时)改用 [`with_event_bus`](Self::with_event_bus) 注入共享总线。
    ///
    /// # 错误
    /// - `ConfigError`:配置校验失败(阈值倒挂、预算为负等)
    pub fn new(config: DecbConfig) -> Result<Self, DecbError> {
        Self::with_event_bus(config, EventBus::new())
    }

    /// 创建带共享 EventBus 的 DECB 治理器(生产代码推荐)
    ///
    /// WHY:生产代码需注入共享总线,使 `BudgetAdjusted` 等事件能被 Parliament/Quest
    /// 订阅。测试代码用 [`new`](Self::new) 创建私有总线,`publish` 静默丢弃,不影响
    /// 测试逻辑(无订阅者时 broadcast::send 返回 Err,被 publish 静默吞掉)。
    ///
    /// # 错误
    /// - `ConfigError`:配置校验失败(阈值倒挂、预算为负等)
    pub fn with_event_bus(config: DecbConfig, bus: EventBus) -> Result<Self, DecbError> {
        config.validate()?;
        let overflow_detector = OverflowDetector::new(&config);
        Ok(Self {
            config,
            tier_state: Arc::new(Mutex::new(TierState {
                current_tier: BudgetTier::HighTier,
                last_switch_time: None,
            })),
            total_consumption: Arc::new(Mutex::new(0.0)),
            consumption_count: AtomicU64::new(0),
            current_coefficient: Mutex::new(1.0),
            overflow_detector,
            event_bus: bus,
        })
    }

    /// EventBus 访问器(供测试与上层共享总线)
    pub fn event_bus(&self) -> &EventBus {
        &self.event_bus
    }

    /// 计算连续可调预算系数
    ///
    /// 公式:`coefficient = base_budget × complexity_factor × urgency_factor × remaining_budget_ratio`
    ///
    /// 所有因子 clamp 到合法区间,最终系数 clamp 到 [0.0, 1.0]。
    /// 计算延迟 < 1ms(仅浮点运算 + 一次锁读取)。
    pub fn compute_budget(&self, quest_input: &QuestBudgetInput) -> f32 {
        let complexity_factor = self.compute_complexity_factor(quest_input);
        let urgency_factor = self.compute_urgency_factor(quest_input);
        let remaining_budget_ratio = self.compute_remaining_budget_ratio();

        let coefficient =
            self.config.base_budget * complexity_factor * urgency_factor * remaining_budget_ratio;

        let clamped = coefficient.clamp(0.0, 1.0);

        // 更新当前系数(用于统计快照)
        if let Ok(mut coef) = self.current_coefficient.lock() {
            *coef = clamped;
        }

        clamped
    }

    /// 计算复杂度因子 ∈ [complexity_factor_min, complexity_factor_max]
    ///
    /// 基于 task_count 和 dependency_depth:
    /// - task_count 越多,复杂度越高(20 个任务达到上限)
    /// - dependency_depth 越深,复杂度越高(深度 5 时 +50%)
    fn compute_complexity_factor(&self, quest_input: &QuestBudgetInput) -> f32 {
        let task_ratio = (quest_input.task_count as f32 / 20.0).clamp(0.0, 1.0);
        let mut factor = self.config.complexity_factor_min
            + task_ratio * (self.config.complexity_factor_max - self.config.complexity_factor_min);

        // 考虑依赖深度:深度 10 时 boost 1.5,但 clamp 到 [0, 0.5] 增量
        let depth_boost = 1.0 + (quest_input.dependency_depth as f32 / 10.0).clamp(0.0, 0.5);
        factor *= depth_boost;

        // 最终 clamp 到 [complexity_factor_min, complexity_factor_max]
        factor.clamp(
            self.config.complexity_factor_min,
            self.config.complexity_factor_max,
        )
    }

    /// 计算紧急度因子 ∈ [urgency_factor_min, urgency_factor_max]
    ///
    /// 基于 deadline:
    /// - 无 deadline:返回 1.0(标准,不加成也不降权)
    /// - 距 deadline <= 1 小时:urgency 最高(urgency_factor_max)
    /// - 距 deadline >= 1 天:urgency 最低(urgency_factor_min)
    /// - 1 小时 < 剩余 < 1 天:线性插值
    fn compute_urgency_factor(&self, quest_input: &QuestBudgetInput) -> f32 {
        match quest_input.deadline {
            None => {
                // WHY 1.0:无 deadline 的 Quest 不加成也不降权,保持中性
                1.0
            }
            Some(deadline) => {
                let now = Utc::now();
                let remaining = deadline.signed_duration_since(now);
                let remaining_secs = remaining.num_seconds();

                // urgency_ratio:1 小时以内=1.0,1 天以上=0.0,线性插值
                let urgency_ratio = if remaining_secs <= ONE_HOUR_SECS {
                    // 1 小时以内或已过期,urgency 最高
                    1.0
                } else if remaining_secs >= ONE_DAY_SECS {
                    // 1 天以上,urgency 最低
                    0.0
                } else {
                    // 线性插值:(ONE_DAY - remaining) / (ONE_DAY - ONE_HOUR)
                    let range = (ONE_DAY_SECS - ONE_HOUR_SECS) as f32;
                    ((ONE_DAY_SECS - remaining_secs) as f32 / range).clamp(0.0, 1.0)
                };

                self.config.urgency_factor_min
                    + urgency_ratio
                        * (self.config.urgency_factor_max - self.config.urgency_factor_min)
            }
        }
    }

    /// 计算剩余预算比例 ∈ [0.0, 1.0]
    ///
    /// `remaining_budget / total_budget_limit`,clamp 到 [0.0, 1.0]。
    fn compute_remaining_budget_ratio(&self) -> f32 {
        let total_consumption = self
            .total_consumption
            .lock()
            .map(|c| *c)
            .unwrap_or_else(|e| {
                error!("total_consumption lock poisoned: {e}");
                // 锁中毒时视为预算已耗尽,触发降级
                self.config.total_budget_limit
            });
        let remaining = (self.config.total_budget_limit - total_consumption).max(0.0);
        let ratio = if self.config.total_budget_limit > 0.0 {
            remaining / self.config.total_budget_limit
        } else {
            0.0
        };
        ratio.clamp(0.0, 1.0) as f32
    }

    /// 判定预算系数对应的档位
    ///
    /// - `coefficient >= high_tier_threshold` → HighTier
    /// - `low_tier_threshold <= coefficient < high_tier_threshold` → LowTier
    /// - `coefficient < low_tier_threshold` → Degraded
    pub fn determine_tier(&self, coefficient: f32) -> BudgetTier {
        if coefficient >= self.config.high_tier_threshold {
            BudgetTier::HighTier
        } else if coefficient >= self.config.low_tier_threshold {
            BudgetTier::LowTier
        } else {
            BudgetTier::Degraded
        }
    }

    /// 返回当前档位
    ///
    /// 锁中毒时安全降级到 Degraded(§6 架构红线:竞态防护)。
    pub fn current_tier(&self) -> BudgetTier {
        self.tier_state
            .lock()
            .map(|state| state.current_tier)
            .unwrap_or_else(|e| {
                error!("tier_state lock poisoned: {e}");
                BudgetTier::Degraded
            })
    }

    /// 切换档位(带滞后机制)
    ///
    /// # 滞后机制
    /// 档位变化后 `tier_switch_lag_ms` 内不再次切换,防止频繁切换(抖动)。
    /// 滞后期内的切换请求被静默忽略(返回 Ok,非错误)。
    ///
    /// # 错误
    /// - `ConfigError`:tier_state 锁中毒
    ///
    /// 已集成 event-bus:档位切换成功后发布 `BudgetAdjusted` 事件(Ω-Event 定律)
    pub fn switch_tier(&self, new_tier: BudgetTier) -> Result<(), DecbError> {
        let mut state = self.tier_state.lock().map_err(|e| DecbError::ConfigError {
            detail: format!("tier_state lock poisoned: {e}"),
        })?;

        // 如果档位相同,无需切换
        if state.current_tier == new_tier {
            return Ok(());
        }

        let now = Utc::now();

        // 检查滞后机制:上次切换后 tier_switch_lag_ms 内不再次切换
        if let Some(last_switch) = state.last_switch_time {
            let elapsed = now.signed_duration_since(last_switch);
            let lag = chrono::Duration::milliseconds(self.config.tier_switch_lag_ms as i64);
            if elapsed < lag {
                // 滞后期内,静默忽略(非错误,是预期行为)
                tracing::debug!(
                    old_tier = %state.current_tier,
                    new_tier = %new_tier,
                    "Tier switch skipped due to lag"
                );
                return Ok(());
            }
        }

        let old_tier = state.current_tier;
        state.current_tier = new_tier;
        state.last_switch_time = Some(now);
        // check-then-act 已完成,提前释放 tier_state 锁,避免持锁发布事件
        drop(state);

        info!(
            old_tier = %old_tier,
            new_tier = %new_tier,
            "Budget tier switched"
        );

        // 发布 BudgetAdjusted 事件(Ω-Event 定律)
        // WHY quest_id 留空:档位切换是全局预算治理事件,DECB 管理全局预算不绑定
        // 单个 Quest;switch_tier 调用方(含 record_consumption 内部调用与外部直接
        // 调用)均未携带 quest_id,留空待 Week 6 上层集成时由调用方注入。
        // WHY publish_blocking:switch_tier 为同步方法,且被同步 #[test] 调用;
        // publish_blocking 直接调用 broadcast::send,不依赖 tokio 运行时,同步测试
        // 不 panic。相比 tokio::spawn fire-and-forget,事件立即投递、不丢失。
        let coefficient = self.current_coefficient.lock().map(|c| *c).unwrap_or(1.0);
        let event = NexusEvent::BudgetAdjusted {
            metadata: EventMetadata::new("decb-governor"),
            quest_id: String::new(),
            old_tier: old_tier.to_string(),
            new_tier: new_tier.to_string(),
            coefficient,
            reason: "manual tier switch".to_string(),
        };
        if let Err(e) = self.event_bus.publish_blocking(event) {
            warn!(error = %e, "发布 BudgetAdjusted 事件失败");
        }

        Ok(())
    }

    /// 记录预算消耗并检查溢出
    ///
    /// # 流程
    /// 1. 计算消耗成本(优先用 total_cost,否则按单价计算)
    /// 2. 累加到 total_consumption
    /// 3. 每 100 次记录发布统计(`BudgetStatsReported` 事件)
    /// 4. 检查溢出,触发降级(发布 `BudgetExceeded` 事件 `[Critical]`)
    /// 5. Degraded 模式下仍超预算时返回 `DegradedModeRejected`
    ///
    /// # 错误
    /// - `ConfigError`:锁中毒
    /// - `DegradedModeRejected`:Degraded 模式下仍超预算
    ///
    /// 已集成 event-bus:发布 `BudgetStatsReported`(每 100 次)与
    /// `BudgetExceeded` `[Critical]`(溢出时)事件(Ω-Event 定律)
    pub fn record_consumption(&self, consumption: &BudgetConsumption) -> Result<(), DecbError> {
        // 步骤 1:计算消耗成本
        let cost = if consumption.total_cost > 0.0 {
            consumption.total_cost
        } else {
            consumption.token_count as f64 * self.config.cost_per_token
                + consumption.tool_call_count as f64 * self.config.cost_per_tool_call
        };

        // 步骤 2:累加到 total_consumption(原子操作)
        let current_total = {
            let mut total = self
                .total_consumption
                .lock()
                .map_err(|e| DecbError::ConfigError {
                    detail: format!("total_consumption lock poisoned: {e}"),
                })?;
            *total += cost;
            *total
        };

        // 步骤 3:更新计数器,每 100 次发布统计
        let count = self.consumption_count.fetch_add(1, Ordering::Relaxed) + 1;
        if count.is_multiple_of(STATS_REPORT_INTERVAL) {
            let stats = self.get_stats();
            info!(
                total_consumption = stats.total_consumption,
                remaining_budget = stats.remaining_budget,
                utilization_rate = stats.utilization_rate,
                "Budget stats reported"
            );
            // 发布 BudgetStatsReported 事件(Ω-Event 定律)
            // WHY publish_blocking:record_consumption 为同步方法,且被同步 #[test]
            // 调用;publish_blocking 不依赖 tokio 运行时,同步测试不 panic。
            let event = NexusEvent::BudgetStatsReported {
                metadata: EventMetadata::new("decb-governor"),
                total_consumption: stats.total_consumption,
                remaining_budget: stats.remaining_budget,
                utilization_rate: stats.utilization_rate,
            };
            if let Err(e) = self.event_bus.publish_blocking(event) {
                warn!(error = %e, "发布 BudgetStatsReported 事件失败");
            }
        }

        // 步骤 4:检查溢出
        if let Some(suggested_tier) = self.overflow_detector.check_overflow(current_total) {
            error!(
                budget_type = "total_cost",
                current = current_total,
                limit = self.config.total_budget_limit,
                "Budget exceeded, triggering degradation"
            );

            // 发布 BudgetExceeded 事件 [Critical](Ω-Event 定律)
            // WHY current/limit 转 u64:事件字段为 u64,预算(美分)截断到整数;
            // 预算量级远小于 u64::MAX,转换安全。BudgetExceeded 为 Critical 级,
            // publish_blocking 会在无订阅者时额外告警(见 bus.rs SubTask 17.2)。
            let event = NexusEvent::BudgetExceeded {
                metadata: EventMetadata::new("decb-governor"),
                budget_type: "total_cost".to_string(),
                current: current_total as u64,
                limit: self.config.total_budget_limit as u64,
            };
            if let Err(e) = self.event_bus.publish_blocking(event) {
                warn!(error = %e, "发布 BudgetExceeded 事件失败");
            }

            let current_tier = self.current_tier();

            // 步骤 5:Degraded 模式下仍超预算,拒绝
            if current_tier == BudgetTier::Degraded {
                return Err(DecbError::DegradedModeRejected {
                    quest_id: String::new(),
                    reason: format!(
                        "budget exhausted in Degraded mode: {current_total} / {}",
                        self.config.total_budget_limit
                    ),
                });
            }

            // 触发降级(switch_tier 内部会发布 BudgetAdjusted 事件)
            self.switch_tier(suggested_tier)?;
        }

        Ok(())
    }

    /// 返回当前预算统计快照
    pub fn get_stats(&self) -> BudgetStats {
        let total_consumption = self
            .total_consumption
            .lock()
            .map(|c| *c)
            .unwrap_or_else(|e| {
                error!("total_consumption lock poisoned: {e}");
                self.config.total_budget_limit
            });
        let remaining_budget = (self.config.total_budget_limit - total_consumption).max(0.0);
        let utilization_rate = if self.config.total_budget_limit > 0.0 {
            (total_consumption / self.config.total_budget_limit) as f32
        } else {
            1.0
        };
        let utilization_rate = utilization_rate.clamp(0.0, 1.0);
        let current_tier = self.current_tier();
        let current_coefficient = self
            .current_coefficient
            .lock()
            .map(|c| *c)
            .unwrap_or_else(|e| {
                error!("current_coefficient lock poisoned: {e}");
                0.0
            });

        BudgetStats {
            total_consumption,
            remaining_budget,
            utilization_rate,
            current_tier,
            current_coefficient,
        }
    }

    /// 重置预算(用于周期重置)
    ///
    /// 清零累计消耗、计数器,档位恢复到 HighTier。
    pub fn reset_budget(&self) {
        if let Ok(mut total) = self.total_consumption.lock() {
            *total = 0.0;
        }
        self.consumption_count.store(0, Ordering::Relaxed);
        if let Ok(mut coef) = self.current_coefficient.lock() {
            *coef = 1.0;
        }
        if let Ok(mut state) = self.tier_state.lock() {
            state.current_tier = BudgetTier::HighTier;
            state.last_switch_time = None;
        }
        info!("Budget reset to initial state");
    }

    /// 启动后台溢出监控任务
    ///
    /// 每 `overflow_check_interval_ms` 毫秒检查一次总消耗,触发降级。
    /// 返回 `JoinHandle`,调用方可用于取消监控。
    ///
    /// # 线程安全
    /// 后台任务持有 `tier_state`、`total_consumption` 的 Arc 副本与 `event_bus` 的
    /// Clone(Arc 引用计数),与主线程通过 Mutex 同步,无数据竞争。
    ///
    /// 已集成 event-bus:溢出触发的档位切换发布 `BudgetAdjusted` 事件(Ω-Event 定律)
    pub fn spawn_overflow_monitor(&self) -> tokio::task::JoinHandle<()> {
        let tier_state = self.tier_state.clone();
        let total_consumption = self.total_consumption.clone();
        let overflow_detector = self.overflow_detector.clone();
        let event_bus = self.event_bus.clone();
        let interval_ms = self.config.overflow_check_interval_ms;
        let lag_ms = self.config.tier_switch_lag_ms;

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(interval_ms));
            loop {
                interval.tick().await;

                // 读取当前消耗
                let current_total = match total_consumption.lock() {
                    Ok(total) => *total,
                    Err(e) => {
                        error!("total_consumption lock poisoned: {e}");
                        continue;
                    }
                };

                // 检查溢出
                if let Some(suggested_tier) = overflow_detector.check_overflow(current_total) {
                    // state(MutexGuard) 非 Send,不能跨 await 持有;用内层块限制其作用域,
                    // 锁在块结束自动释放,await 在块外执行(避免 future 非 Send 导致 spawn 失败)
                    let (old_tier, new_tier) = {
                        let mut state = match tier_state.lock() {
                            Ok(s) => s,
                            Err(e) => {
                                error!("tier_state lock poisoned: {e}");
                                continue;
                            }
                        };

                        // 如果档位相同,无需切换
                        if state.current_tier == suggested_tier {
                            continue;
                        }

                        let now = Utc::now();

                        // 检查滞后机制
                        let can_switch = state.last_switch_time.is_none_or(|last| {
                            let elapsed = now.signed_duration_since(last);
                            let lag = chrono::Duration::milliseconds(lag_ms as i64);
                            elapsed >= lag
                        });

                        if !can_switch {
                            continue;
                        }

                        let old = state.current_tier;
                        state.current_tier = suggested_tier;
                        state.last_switch_time = Some(now);
                        (old, suggested_tier)
                    }; // state 在此自动 drop,MutexGuard 释放,后续 await 安全

                    info!(
                        old_tier = %old_tier,
                        new_tier = %new_tier,
                        "Budget tier switched by overflow monitor"
                    );
                    // 发布 BudgetAdjusted 事件(Ω-Event 定律)
                    // WHY publish().await:此处已在 tokio::spawn 的 async 上下文中,
                    // 直接 await 投递,语义最清晰;锁已释放,无持锁跨挂起点。
                    // WHY quest_id 留空:后台监控不绑定特定 Quest(同 switch_tier)。
                    // WHY coefficient=0.0:后台任务无 current_coefficient 访问权
                    // (字段为 Mutex<f32> 非 Arc,无法 clone);溢出降级时新档位系数
                    // 将由下次 compute_budget 重算,此处 0.0 占位表示"待重算"。
                    let event = NexusEvent::BudgetAdjusted {
                        metadata: EventMetadata::new("decb-governor"),
                        quest_id: String::new(),
                        old_tier: old_tier.to_string(),
                        new_tier: new_tier.to_string(),
                        coefficient: 0.0,
                        reason: "overflow detected, auto degrade".to_string(),
                    };
                    if let Err(e) = event_bus.publish(event).await {
                        warn!(error = %e, "发布 BudgetAdjusted 事件失败(overflow monitor)");
                    }
                }
            }
        })
    }

    /// 返回配置引用(测试与监控用)
    pub fn config(&self) -> &DecbConfig {
        &self.config
    }

    /// 返回溢出检测器引用(测试用)
    pub fn overflow_detector(&self) -> &OverflowDetector {
        &self.overflow_detector
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration as ChronoDuration;

    fn make_governor() -> DecbGovernor {
        DecbGovernor::new(DecbConfig::default()).unwrap()
    }

    fn make_governor_with_lag(lag_ms: u64) -> DecbGovernor {
        DecbGovernor::new(DecbConfig {
            tier_switch_lag_ms: lag_ms,
            ..Default::default()
        })
        .unwrap()
    }

    // ============================================================
    // SubTask 34.2: 预算系数计算测试
    // ============================================================

    #[test]
    fn test_compute_budget_simple_quest_low_coefficient() {
        // WHY 简单任务:单任务、无依赖、无 deadline,复杂度因子最低
        let governor = make_governor();
        let quest = QuestBudgetInput::simple("quest-simple");
        let coef = governor.compute_budget(&quest);

        // 简单任务计算:
        // task_ratio = 1/20 = 0.05,complexity = 0.5 + 0.05 * 1.0 = 0.55
        // depth_boost = 1.0,urgency = 1.0,remaining = 1.0
        // coefficient = 0.8 * 0.55 * 1.0 * 1.0 = 0.44
        assert!(
            (coef - 0.44).abs() < 1e-3,
            "simple quest coefficient should be ~0.44, got {coef}"
        );
    }

    #[test]
    fn test_compute_budget_complex_quest_higher_coefficient() {
        let governor = make_governor();
        // 复杂任务:20 个任务,依赖深度 5
        let quest = QuestBudgetInput::new("quest-complex", 20, 5, None, 1000);
        let coef_complex = governor.compute_budget(&quest);

        // 简单任务作为对比
        let simple = QuestBudgetInput::simple("quest-simple");
        let coef_simple = governor.compute_budget(&simple);

        assert!(
            coef_complex > coef_simple,
            "complex quest ({coef_complex}) should have higher coefficient than simple ({coef_simple})"
        );
    }

    #[test]
    fn test_compute_budget_urgent_quest_boost() {
        let governor = make_governor();
        // 紧急任务:deadline 30 分钟后
        let urgent_deadline = Utc::now() + ChronoDuration::minutes(30);
        let urgent = QuestBudgetInput::new("quest-urgent", 5, 2, Some(urgent_deadline), 100);

        // 非紧急任务:deadline 2 天后
        let far_deadline = Utc::now() + ChronoDuration::days(2);
        let not_urgent = QuestBudgetInput::new("quest-not-urgent", 5, 2, Some(far_deadline), 100);

        let coef_urgent = governor.compute_budget(&urgent);
        let coef_not_urgent = governor.compute_budget(&not_urgent);

        assert!(
            coef_urgent > coef_not_urgent,
            "urgent quest ({coef_urgent}) should have higher coefficient than not-urgent ({coef_not_urgent})"
        );
    }

    #[test]
    fn test_compute_budget_no_deadline_default() {
        let governor = make_governor();
        let quest = QuestBudgetInput::new("quest-no-deadline", 5, 2, None, 100);
        let coef = governor.compute_budget(&quest);

        // 无 deadline:urgency = 1.0,不应加成也不应降权
        // coefficient = 0.8 * complexity * 1.0 * 1.0
        assert!(coef > 0.0, "coefficient should be positive");
        assert!(coef <= 1.0, "coefficient should be <= 1.0");
    }

    #[test]
    fn test_compute_budget_budget_exhausted_degradation() {
        let governor = make_governor();

        // 模拟预算耗尽:手动设置 total_consumption
        {
            let mut total = governor.total_consumption.lock().unwrap();
            *total = governor.config.total_budget_limit;
        }

        let quest = QuestBudgetInput::simple("quest-after-exhaust");
        let coef = governor.compute_budget(&quest);

        // 预算耗尽:remaining_budget_ratio = 0.0,coefficient = 0.0
        assert!(
            (coef - 0.0).abs() < 1e-6,
            "exhausted budget should yield 0 coefficient, got {coef}"
        );
    }

    #[test]
    fn test_compute_budget_clamped_to_unit_interval() {
        let governor = make_governor();
        // 极端复杂任务,确保 clamp 到 [0, 1]
        let quest = QuestBudgetInput::new("quest-extreme", 100, 100, None, 10000);
        let coef = governor.compute_budget(&quest);
        assert!(
            (0.0..=1.0).contains(&coef),
            "coefficient must be in [0, 1], got {coef}"
        );
    }

    #[test]
    fn test_compute_complexity_factor_bounds() {
        let governor = make_governor();
        // 最低复杂度:0 任务,0 深度
        let simple = QuestBudgetInput::new("q", 0, 0, None, 0);
        let factor_low = governor.compute_complexity_factor(&simple);
        assert!(factor_low >= governor.config.complexity_factor_min);

        // 最高复杂度:20+ 任务,10+ 深度
        let complex = QuestBudgetInput::new("q", 30, 20, None, 0);
        let factor_high = governor.compute_complexity_factor(&complex);
        assert!(factor_high <= governor.config.complexity_factor_max);
        assert!(factor_high > factor_low);
    }

    #[test]
    fn test_compute_urgency_factor_no_deadline() {
        let governor = make_governor();
        let quest = QuestBudgetInput::new("q", 1, 0, None, 0);
        let factor = governor.compute_urgency_factor(&quest);
        assert!((factor - 1.0).abs() < 1e-6, "no deadline should yield 1.0");
    }

    #[test]
    fn test_compute_urgency_factor_within_one_hour() {
        let governor = make_governor();
        let deadline = Utc::now() + ChronoDuration::minutes(30);
        let quest = QuestBudgetInput::new("q", 1, 0, Some(deadline), 0);
        let factor = governor.compute_urgency_factor(&quest);
        // 1 小时以内:urgency_ratio = 1.0,factor = urgency_factor_max = 1.2
        assert!(
            (factor - governor.config.urgency_factor_max).abs() < 1e-3,
            "within 1h should yield urgency_factor_max, got {factor}"
        );
    }

    #[test]
    fn test_compute_urgency_factor_beyond_one_day() {
        let governor = make_governor();
        let deadline = Utc::now() + ChronoDuration::days(2);
        let quest = QuestBudgetInput::new("q", 1, 0, Some(deadline), 0);
        let factor = governor.compute_urgency_factor(&quest);
        // 1 天以上:urgency_ratio = 0.0,factor = urgency_factor_min = 0.8
        assert!(
            (factor - governor.config.urgency_factor_min).abs() < 1e-3,
            "beyond 1 day should yield urgency_factor_min, got {factor}"
        );
    }

    // ============================================================
    // SubTask 34.3: 档位切换测试
    // ============================================================

    #[test]
    fn test_determine_tier_high() {
        let governor = make_governor();
        assert_eq!(governor.determine_tier(0.8), BudgetTier::HighTier);
        assert_eq!(
            governor.determine_tier(governor.config.high_tier_threshold),
            BudgetTier::HighTier
        );
    }

    #[test]
    fn test_determine_tier_low() {
        let governor = make_governor();
        assert_eq!(governor.determine_tier(0.4), BudgetTier::LowTier);
        assert_eq!(
            governor.determine_tier(governor.config.low_tier_threshold),
            BudgetTier::LowTier
        );
    }

    #[test]
    fn test_determine_tier_degraded() {
        let governor = make_governor();
        assert_eq!(governor.determine_tier(0.1), BudgetTier::Degraded);
        assert_eq!(governor.determine_tier(0.0), BudgetTier::Degraded);
    }

    #[test]
    fn test_switch_tier_high_to_low() {
        let governor = make_governor();
        assert_eq!(governor.current_tier(), BudgetTier::HighTier);

        governor.switch_tier(BudgetTier::LowTier).unwrap();
        assert_eq!(governor.current_tier(), BudgetTier::LowTier);
    }

    #[test]
    fn test_switch_tier_low_to_high() {
        let governor = make_governor();
        // 先切换到 LowTier
        governor.switch_tier(BudgetTier::LowTier).unwrap();
        assert_eq!(governor.current_tier(), BudgetTier::LowTier);

        // 使用 lag_ms = 0 的配置,允许立即切换
        let governor2 = make_governor_with_lag(0);
        // governor2 初始为 HighTier,先切换到 LowTier
        governor2.switch_tier(BudgetTier::LowTier).unwrap();
        // 立即切换回 HighTier(lag = 0 允许)
        governor2.switch_tier(BudgetTier::HighTier).unwrap();
        assert_eq!(governor2.current_tier(), BudgetTier::HighTier);
    }

    #[test]
    fn test_switch_tier_high_to_degraded() {
        let governor = make_governor_with_lag(0);
        governor.switch_tier(BudgetTier::Degraded).unwrap();
        assert_eq!(governor.current_tier(), BudgetTier::Degraded);
    }

    #[test]
    fn test_switch_tier_low_to_degraded() {
        let governor = make_governor_with_lag(0);
        governor.switch_tier(BudgetTier::LowTier).unwrap();
        governor.switch_tier(BudgetTier::Degraded).unwrap();
        assert_eq!(governor.current_tier(), BudgetTier::Degraded);
    }

    #[test]
    fn test_switch_tier_same_tier_noop() {
        let governor = make_governor();
        // 切换到相同档位应为 no-op
        governor.switch_tier(BudgetTier::HighTier).unwrap();
        assert_eq!(governor.current_tier(), BudgetTier::HighTier);
    }

    #[test]
    fn test_switch_tier_lag_mechanism() {
        // WHY 滞后机制:切换后立即再次切换应被阻止
        let governor = make_governor_with_lag(10_000); // 10 秒滞后
        assert_eq!(governor.current_tier(), BudgetTier::HighTier);

        // 第一次切换:HighTier → LowTier,成功
        governor.switch_tier(BudgetTier::LowTier).unwrap();
        assert_eq!(governor.current_tier(), BudgetTier::LowTier);

        // 立即再次切换:LowTier → HighTier,应被滞后机制阻止
        governor.switch_tier(BudgetTier::HighTier).unwrap();
        assert_eq!(
            governor.current_tier(),
            BudgetTier::LowTier,
            "tier should remain LowTier due to lag"
        );
    }

    #[test]
    fn test_switch_tier_lag_expired() {
        // lag_ms = 1,等待 10ms 后应允许切换
        let governor = make_governor_with_lag(1);
        governor.switch_tier(BudgetTier::LowTier).unwrap();
        assert_eq!(governor.current_tier(), BudgetTier::LowTier);

        // 等待滞后过期
        std::thread::sleep(Duration::from_millis(20));

        // 滞后过期后应允许切换
        governor.switch_tier(BudgetTier::HighTier).unwrap();
        assert_eq!(governor.current_tier(), BudgetTier::HighTier);
    }

    // ============================================================
    // SubTask 34.4: 溢出检测与降级测试
    // ============================================================

    #[test]
    fn test_record_consumption_triggers_degradation() {
        let governor = make_governor_with_lag(0);
        assert_eq!(governor.current_tier(), BudgetTier::HighTier);

        // 消耗超过 80% 预算,触发降级到 LowTier
        let consumption = BudgetConsumption {
            token_count: 0,
            tool_call_count: 0,
            context_load_count: 0,
            total_cost: governor.config.total_budget_limit * 0.85,
        };
        governor.record_consumption(&consumption).unwrap();

        assert_eq!(
            governor.current_tier(),
            BudgetTier::LowTier,
            "85% consumption should trigger degradation to LowTier"
        );
    }

    #[test]
    fn test_record_consumption_critical_degradation() {
        let governor = make_governor_with_lag(0);

        // 消耗超过 100% 预算,触发降级到 Degraded
        let consumption = BudgetConsumption {
            token_count: 0,
            tool_call_count: 0,
            context_load_count: 0,
            total_cost: governor.config.total_budget_limit * 1.1,
        };
        governor.record_consumption(&consumption).unwrap();

        assert_eq!(
            governor.current_tier(),
            BudgetTier::Degraded,
            "110% consumption should trigger degradation to Degraded"
        );
    }

    #[test]
    fn test_record_consumption_degraded_mode_rejected() {
        let governor = make_governor_with_lag(0);

        // 先消耗到 Degraded
        let critical = BudgetConsumption {
            total_cost: governor.config.total_budget_limit * 1.1,
            ..BudgetConsumption::zero()
        };
        governor.record_consumption(&critical).unwrap();
        assert_eq!(governor.current_tier(), BudgetTier::Degraded);

        // Degraded 模式下继续消耗,应返回 DegradedModeRejected
        let more = BudgetConsumption {
            total_cost: 100.0,
            ..BudgetConsumption::zero()
        };
        let result = governor.record_consumption(&more);
        assert!(
            matches!(result, Err(DecbError::DegradedModeRejected { .. })),
            "Degraded mode should reject further consumption"
        );
    }

    #[test]
    fn test_record_consumption_no_overflow() {
        let governor = make_governor();
        let initial_tier = governor.current_tier();

        // 少量消耗,不触发溢出
        let consumption = BudgetConsumption::new(1000, 5, 2);
        governor.record_consumption(&consumption).unwrap();

        assert_eq!(governor.current_tier(), initial_tier);
    }

    #[test]
    fn test_record_consumption_cost_calculation() {
        let governor = make_governor();
        let consumption = BudgetConsumption::new(1000, 5, 2);
        governor.record_consumption(&consumption).unwrap();

        let stats = governor.get_stats();
        let expected =
            1000.0 * governor.config.cost_per_token + 5.0 * governor.config.cost_per_tool_call;
        assert!(
            (stats.total_consumption - expected).abs() < 1e-6,
            "consumption should be {expected}, got {}",
            stats.total_consumption
        );
    }

    #[tokio::test]
    async fn test_spawn_overflow_monitor_runs() {
        // 配置短间隔用于测试
        let config = DecbConfig {
            overflow_check_interval_ms: 10,
            tier_switch_lag_ms: 0,
            total_budget_limit: 100.0,
            ..DecbConfig::default()
        };
        let governor = DecbGovernor::new(config).unwrap();

        // 手动设置高消耗
        {
            let mut total = governor.total_consumption.lock().unwrap();
            *total = 90.0; // 90% > 80%,触发 LowTier 降级
        }

        let handle = governor.spawn_overflow_monitor();
        tokio::time::sleep(Duration::from_millis(50)).await;

        // 后台任务应已触发降级
        assert_eq!(
            governor.current_tier(),
            BudgetTier::LowTier,
            "overflow monitor should degrade to LowTier"
        );

        handle.abort();
    }

    // ============================================================
    // SubTask 34.5: 消耗统计测试
    // ============================================================

    #[test]
    fn test_get_stats_initial() {
        let governor = make_governor();
        let stats = governor.get_stats();

        assert!((stats.total_consumption - 0.0).abs() < 1e-6);
        assert!((stats.remaining_budget - governor.config.total_budget_limit).abs() < 1e-6);
        assert!((stats.utilization_rate - 0.0).abs() < 1e-6);
        assert_eq!(stats.current_tier, BudgetTier::HighTier);
        assert!((stats.current_coefficient - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_get_stats_after_consumption() {
        let governor = make_governor();
        let consumption = BudgetConsumption {
            total_cost: 100_000.0,
            ..BudgetConsumption::zero()
        };
        governor.record_consumption(&consumption).unwrap();

        let stats = governor.get_stats();
        assert!((stats.total_consumption - 100_000.0).abs() < 1e-6);
        assert!(
            (stats.remaining_budget - (governor.config.total_budget_limit - 100_000.0)).abs()
                < 1e-6
        );
        let expected_rate = 100_000.0 / governor.config.total_budget_limit;
        assert!((stats.utilization_rate - expected_rate as f32).abs() < 1e-3);
    }

    #[test]
    fn test_reset_budget() {
        let governor = make_governor();

        // 记录一些消耗
        let consumption = BudgetConsumption {
            total_cost: 100_000.0,
            ..BudgetConsumption::zero()
        };
        governor.record_consumption(&consumption).unwrap();

        // 重置预算
        governor.reset_budget();

        let stats = governor.get_stats();
        assert!((stats.total_consumption - 0.0).abs() < 1e-6);
        assert!((stats.remaining_budget - governor.config.total_budget_limit).abs() < 1e-6);
        assert!((stats.utilization_rate - 0.0).abs() < 1e-6);
        assert_eq!(stats.current_tier, BudgetTier::HighTier);
    }

    #[test]
    fn test_multiple_consumption_accumulation() {
        let governor = make_governor();

        for i in 0..5 {
            let consumption = BudgetConsumption {
                total_cost: 10_000.0 * (i as f64 + 1.0),
                ..BudgetConsumption::zero()
            };
            governor.record_consumption(&consumption).unwrap();
        }

        let stats = governor.get_stats();
        // 累计:10000 + 20000 + 30000 + 40000 + 50000 = 150000
        assert!((stats.total_consumption - 150_000.0).abs() < 1e-6);
    }

    #[test]
    fn test_stats_report_at_interval() {
        // WHY 每 100 次记录发布统计:验证计数器正确递增
        let governor = make_governor();

        for _ in 0..150 {
            let consumption = BudgetConsumption::new(1, 0, 0);
            governor.record_consumption(&consumption).unwrap();
        }

        // 150 次记录后,计数器应为 150
        let count = governor.consumption_count.load(Ordering::Relaxed);
        assert_eq!(count, 150);
    }

    // ============================================================
    // 配置错误测试
    // ============================================================

    #[test]
    fn test_invalid_config_rejected() {
        let config = DecbConfig {
            base_budget: 1.5,
            ..Default::default()
        };
        assert!(DecbGovernor::new(config).is_err());
    }

    #[test]
    fn test_config_accessor() {
        let governor = make_governor();
        assert!((governor.config().base_budget - 0.8).abs() < 1e-6);
    }
}
