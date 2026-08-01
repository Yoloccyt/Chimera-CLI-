//! 策略封顶守卫 — ratio 反馈驱动的审议深度降级封顶(推理悖论红线风控)
//!
//! 对应架构层:L8 Parliament
//! 对应分析:三重悖论"推理悖论红线"——当协调成本超过推理增益时,
//! 多 Agent 审议反而不如单 Agent,系统应自适应降低审议深度。
//!
//! # 与 LinUCB S5 接缝的关系(互补,不替代)
//! - S5 接缝(omega-learner)是**事前**"上下文 → 策略"选择
//! - 本守卫是**事后**"ratio 反馈 → 策略封顶"上界:
//!   `deliberate` 最终策略 = min(学习策略, 封顶),学习器输出不被改写
//! - 观测事件 `ParliamentStrategyCapChanged` 让 omega-learner 未来可将
//!   封顶状态纳入上下文特征,避免双控制器互相打架
//!
//! # 滞后带状态机(防抖振,参照 decb-governor 滞后机制先例)
//! - `ratio > threshold` 连续 `enter_consecutive` 次 → 封顶降一档
//!   (Full → Simplified → FastPath 上限,安全响应快速生效)
//! - `ratio < exit_ratio_factor × threshold` 连续 `exit_consecutive` 次
//!   且距上次变更 ≥ `min_dwell_ms` → 封顶升一档(恢复保守)
//! - 中间滞后带((exit_factor×threshold, threshold])→ 双计数器清零
//!
//! # 安全不变量(红队防线)
//! 封顶只影响审议深度上限;Skeptic 恶意意图检测在 `deliberate_with_policy`
//! 步骤 0 执行,先于封顶应用后的策略分派,任何封顶档位都不可绕过。

use std::sync::Mutex;
use std::time::Instant;

use event_bus::{EventBus, EventMetadata, NexusEvent};
use nexus_contracts::ActivationStrategy;
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

use crate::error::ParliamentError;

// ============================================================
// 配置类型
// ============================================================

/// 策略封顶守卫配置 — 滞后带阈值与驻留时间
///
/// 嵌入 [`crate::ParliamentConfig`](crate::config::ParliamentConfig) 的
/// `strategy_cap` 字段,`#[serde(default)]` 保证旧配置文件反序列化兼容。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StrategyCapConfig {
    /// 连续越阈次数达到此值触发封顶降档,默认 3
    ///
    /// WHY 3 次:单次 ratio 越阈可能是瞬时抖动(EWMA 已平滑一层),
    /// 连续 3 个报告周期越阈才认定协调成本持续超过推理增益。
    pub enter_consecutive: u32,

    /// 连续回落次数达到此值(且满足驻留时间)触发封顶升档,默认 5
    ///
    /// WHY 比 enter 更保守(5 > 3):降档是安全响应应快,
    /// 升档是恢复动作应慢,不对称阈值防止边界震荡。
    pub exit_consecutive: u32,

    /// 回落判定系数,默认 0.8(即 ratio < 0.8 × threshold 才计入回落)
    ///
    /// WHY 0.8:与越阈线(1.0 × threshold)之间形成 20% 滞后带,
    /// ratio 在带内徘徊时双计数器清零,避免降-升-降抖振。
    pub exit_ratio_factor: f64,

    /// 升档最小驻留时间(毫秒),默认 30000(30s)
    ///
    /// 距上次封顶变更不足此时长时不允许升档(降档不受限,安全优先)。
    pub min_dwell_ms: u64,
}

impl Default for StrategyCapConfig {
    fn default() -> Self {
        Self {
            enter_consecutive: 3,
            exit_consecutive: 5,
            exit_ratio_factor: 0.8,
            min_dwell_ms: 30_000,
        }
    }
}

impl StrategyCapConfig {
    /// 校验配置合法性
    ///
    /// WHY:连续计数为 0 会使状态机每个报告都触发变更(失去滞后意义),
    /// 回落系数越界会使滞后带反转或消失,均需提前拦截。
    pub fn validate(&self) -> Result<(), ParliamentError> {
        if self.enter_consecutive == 0 || self.exit_consecutive == 0 {
            return Err(ParliamentError::ConfigError {
                detail: "strategy_cap consecutive thresholds must be >= 1".into(),
            });
        }
        if !(0.0..=1.0).contains(&self.exit_ratio_factor) || self.exit_ratio_factor == 0.0 {
            return Err(ParliamentError::ConfigError {
                detail: format!(
                    "strategy_cap exit_ratio_factor must be in (0.0, 1.0], got {}",
                    self.exit_ratio_factor
                ),
            });
        }
        Ok(())
    }
}

// ============================================================
// 封顶变更通知
// ============================================================

/// 单次封顶变更 — `observe` 返回给订阅器用于发布观测事件
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapChange {
    /// 变更前封顶
    pub old_cap: ActivationStrategy,
    /// 变更后封顶
    pub new_cap: ActivationStrategy,
}

// ============================================================
// 守卫状态与实现
// ============================================================

/// 守卫内部状态(Mutex 保护,方法内不跨 `.await`)
#[derive(Debug)]
struct CapState {
    /// 当前策略封顶(审议深度上限),初始 Full = 不设限
    cap: ActivationStrategy,
    /// 连续越阈计数(ratio > threshold)
    consecutive_over: u32,
    /// 连续回落计数(ratio < exit_factor × threshold)
    consecutive_under: u32,
    /// 上次封顶变更时刻(升档驻留时间判定基准)
    last_transition: Instant,
}

/// 策略封顶守卫 — 消费 ratio 报告,维护审议策略上界
///
/// # 并发安全
/// 所有方法为同步方法,`Mutex` 锁在方法返回时释放,不跨 `.await` 点
/// (§4.4 反模式 #1);毒锁降级访问(与 CoordinationMetricsCollector 一致:
/// 状态仅含枚举/计数器,残留值只造成单次判定偏差,后续报告会纠正)。
pub struct StrategyCapGuard {
    /// 内部状态(Mutex 保护)
    state: Mutex<CapState>,
    /// 守卫配置(只读)
    config: StrategyCapConfig,
}

impl StrategyCapGuard {
    /// 创建守卫(初始封顶 Full = 不设限)
    pub fn new(config: StrategyCapConfig) -> Self {
        Self {
            state: Mutex::new(CapState {
                cap: ActivationStrategy::Full,
                consecutive_over: 0,
                consecutive_under: 0,
                last_transition: Instant::now(),
            }),
            config,
        }
    }

    /// 获取内部状态锁(毒锁降级恢复,WHY 见 struct 文档)
    fn lock_state(&self) -> std::sync::MutexGuard<'_, CapState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 当前策略封顶(审议深度上限)
    pub fn current_cap(&self) -> ActivationStrategy {
        self.lock_state().cap
    }

    /// 获取守卫配置引用
    pub fn config(&self) -> &StrategyCapConfig {
        &self.config
    }

    /// 将策略与封顶取 min,得到实际生效策略
    ///
    /// 序关系:FastPath(1) < Simplified(2) < Full(3)(repr(u8) 判别值),
    /// 封顶为上界:学习器/静态策略要求的深度超过封顶时被压到封顶档。
    pub fn apply(&self, strategy: ActivationStrategy) -> ActivationStrategy {
        min_strategy(strategy, self.current_cap())
    }

    /// 直接设置策略封顶(用于悖论风险仪表盘紧急降档/熔断/恢复)
    ///
    /// 此方法绕过滞后带状态机直接设置 cap,适用于悖论风险仪表盘
    /// 的紧急响应场景。会重置滞后带计数器与驻留时间。
    ///
    /// # 与 `observe` 的区别
    /// - `observe` 基于 ratio 反馈通过滞后带状态机渐进调整,适合常规风控
    /// - `set_max_strategy` 直接设置 cap,绕过滞后带,适合紧急响应
    /// - 两者互补:仪表盘紧急响应后,`observe` 继续处理常规 ratio 反馈
    pub fn set_max_strategy(&self, cap: ActivationStrategy) {
        let mut state = self.lock_state();
        state.cap = cap;
        state.consecutive_over = 0;
        state.consecutive_under = 0;
        state.last_transition = Instant::now();
    }

    /// 消费一次 ratio 报告,更新滞后带状态机
    ///
    /// # 返回
    /// `Some(CapChange)` 表示封顶发生升/降档(调用方据此发布观测事件),
    /// `None` 表示无变更(计数中/滞后带内/已达边界档)。
    ///
    /// # 状态机(WHY 三分支互斥)
    /// - 越阈:over+1、under 清零 —— 两方向证据互斥,越阈即否定回落趋势
    /// - 回落:under+1、over 清零 —— 同理
    /// - 滞后带内:双清零 —— 中间态不构成任何方向的连续证据
    pub fn observe(&self, ratio: f64, threshold: f64) -> Option<CapChange> {
        let mut state = self.lock_state();

        if ratio > threshold {
            // 越阈方向:累计降档证据
            state.consecutive_over += 1;
            state.consecutive_under = 0;

            if state.consecutive_over >= self.config.enter_consecutive {
                if let Some(lower) = lower_strategy(state.cap) {
                    let change = CapChange {
                        old_cap: state.cap,
                        new_cap: lower,
                    };
                    state.cap = lower;
                    state.consecutive_over = 0;
                    state.last_transition = Instant::now();
                    warn!(
                        old_cap = %change.old_cap,
                        new_cap = %change.new_cap,
                        ratio,
                        threshold,
                        "推理悖论风控:审议策略封顶降档(协调成本持续超过推理增益)"
                    );
                    return Some(change);
                }
                // 已在最低档(FastPath),计数封顶防溢出,等待回落
                state.consecutive_over = self.config.enter_consecutive;
            }
        } else if ratio < self.config.exit_ratio_factor * threshold {
            // 回落方向:累计升档证据
            state.consecutive_under += 1;
            state.consecutive_over = 0;

            let dwell_ok =
                state.last_transition.elapsed().as_millis() >= u128::from(self.config.min_dwell_ms);
            if state.consecutive_under >= self.config.exit_consecutive && dwell_ok {
                if let Some(higher) = higher_strategy(state.cap) {
                    let change = CapChange {
                        old_cap: state.cap,
                        new_cap: higher,
                    };
                    state.cap = higher;
                    state.consecutive_under = 0;
                    state.last_transition = Instant::now();
                    info!(
                        old_cap = %change.old_cap,
                        new_cap = %change.new_cap,
                        ratio,
                        threshold,
                        "推理悖论风控:审议策略封顶升档(协调成本已持续回落)"
                    );
                    return Some(change);
                }
                state.consecutive_under = self.config.exit_consecutive;
            }
        } else {
            // 滞后带内:双计数器清零(防抖振)
            state.consecutive_over = 0;
            state.consecutive_under = 0;
        }

        None
    }
}

impl Default for StrategyCapGuard {
    fn default() -> Self {
        Self::new(StrategyCapConfig::default())
    }
}

// ============================================================
// 策略序辅助函数
// ============================================================

/// 取两策略中审议深度更浅者(min)
///
/// 依赖 `ActivationStrategy` 的 repr(u8) 判别值序:
/// FastPath=1 < Simplified=2 < Full=3。
pub(crate) fn min_strategy(a: ActivationStrategy, b: ActivationStrategy) -> ActivationStrategy {
    if (a as u8) <= (b as u8) {
        a
    } else {
        b
    }
}

/// 封顶降一档(Full→Simplified→FastPath),已在最低档返回 None
fn lower_strategy(cap: ActivationStrategy) -> Option<ActivationStrategy> {
    match cap {
        ActivationStrategy::Full => Some(ActivationStrategy::Simplified),
        ActivationStrategy::Simplified => Some(ActivationStrategy::FastPath),
        ActivationStrategy::FastPath => None,
    }
}

/// 封顶升一档(FastPath→Simplified→Full),已在最高档返回 None
fn higher_strategy(cap: ActivationStrategy) -> Option<ActivationStrategy> {
    match cap {
        ActivationStrategy::FastPath => Some(ActivationStrategy::Simplified),
        ActivationStrategy::Simplified => Some(ActivationStrategy::Full),
        ActivationStrategy::Full => None,
    }
}

// ============================================================
// 订阅器
// ============================================================

/// 启动后台封顶订阅任务 — 消费 `CoordinationRatioReported` 驱动守卫
///
/// L8 订阅 L1 event-bus 合法(§2.2);封顶变更时发布
/// `ParliamentStrategyCapChanged` 观测事件供 TUI/监控展示。
///
/// WHY 先 subscribe 再 spawn:遵循 "subscribe-before-spawn" 规则,
/// 避免启动瞬间的事件丢失(§4.4 反模式 #3)。
pub fn spawn_strategy_cap_subscriber(
    guard: std::sync::Arc<StrategyCapGuard>,
    bus: EventBus,
) -> tokio::task::JoinHandle<()> {
    let mut rx = bus.subscribe();
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(NexusEvent::CoordinationRatioReported {
                    ratio, threshold, ..
                }) => {
                    // observe 为同步方法,锁在返回时释放,发布在锁外(§4.4 反模式 #1)
                    if let Some(change) = guard.observe(ratio, threshold) {
                        let event = NexusEvent::ParliamentStrategyCapChanged {
                            metadata: EventMetadata::new("parliament:StrategyCapGuard"),
                            old_cap: change.old_cap.short_name().to_string(),
                            new_cap: change.new_cap.short_name().to_string(),
                            ratio,
                            threshold,
                        };
                        if let Err(e) = bus.publish(event).await {
                            warn!(error = %e, "发布 ParliamentStrategyCapChanged 事件失败");
                        }
                    }
                }
                Ok(_) => {} // 其他事件忽略
                Err(e) => {
                    error!(error = %e, "策略封顶订阅者接收错误,退出");
                    break;
                }
            }
        }
    })
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试用配置:无驻留时间限制,便于验证状态机转换
    fn fast_config() -> StrategyCapConfig {
        StrategyCapConfig {
            enter_consecutive: 3,
            exit_consecutive: 5,
            exit_ratio_factor: 0.8,
            min_dwell_ms: 0,
        }
    }

    // === 配置测试 ===

    #[test]
    fn test_config_default_values() {
        let cfg = StrategyCapConfig::default();
        assert_eq!(cfg.enter_consecutive, 3);
        assert_eq!(cfg.exit_consecutive, 5);
        assert!((cfg.exit_ratio_factor - 0.8).abs() < 1e-9);
        assert_eq!(cfg.min_dwell_ms, 30_000);
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn test_config_validate_rejects_zero_counts_and_bad_factor() {
        let zero_enter = StrategyCapConfig {
            enter_consecutive: 0,
            ..Default::default()
        };
        assert!(zero_enter.validate().is_err(), "enter=0 应拒绝");

        let zero_factor = StrategyCapConfig {
            exit_ratio_factor: 0.0,
            ..Default::default()
        };
        assert!(
            zero_factor.validate().is_err(),
            "factor=0 应拒绝(滞后带消失)"
        );

        let big_factor = StrategyCapConfig {
            exit_ratio_factor: 1.5,
            ..Default::default()
        };
        assert!(
            big_factor.validate().is_err(),
            "factor>1 应拒绝(滞后带反转)"
        );
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let cfg = StrategyCapConfig {
            enter_consecutive: 2,
            exit_consecutive: 4,
            exit_ratio_factor: 0.7,
            min_dwell_ms: 1000,
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let restored: StrategyCapConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(cfg, restored);
    }

    // === 状态机测试:进入/驻留/退出/抖振抑制 ===

    #[test]
    fn test_cap_lowers_after_consecutive_over_threshold() {
        let guard = StrategyCapGuard::new(fast_config());
        assert_eq!(guard.current_cap(), ActivationStrategy::Full);

        // 前 2 次越阈:计数中,无变更
        assert!(guard.observe(1.5, 1.0).is_none());
        assert!(guard.observe(1.5, 1.0).is_none());
        // 第 3 次:降档 Full → Simplified
        let change = guard.observe(1.5, 1.0).expect("第 3 次越阈应降档");
        assert_eq!(change.old_cap, ActivationStrategy::Full);
        assert_eq!(change.new_cap, ActivationStrategy::Simplified);
        assert_eq!(guard.current_cap(), ActivationStrategy::Simplified);
    }

    #[test]
    fn test_cap_lowers_to_fastpath_and_saturates() {
        let guard = StrategyCapGuard::new(fast_config());
        // 6 次越阈:Full→Simplified(第3次)→FastPath(第6次)
        for _ in 0..6 {
            guard.observe(2.0, 1.0);
        }
        assert_eq!(guard.current_cap(), ActivationStrategy::FastPath);

        // 继续越阈:已在最低档,不再变更(饱和)
        for _ in 0..10 {
            assert!(guard.observe(2.0, 1.0).is_none(), "最低档应饱和不变");
        }
        assert_eq!(guard.current_cap(), ActivationStrategy::FastPath);
    }

    #[test]
    fn test_cap_raises_after_consecutive_under_exit_line() {
        let guard = StrategyCapGuard::new(fast_config());
        // 降到 Simplified
        for _ in 0..3 {
            guard.observe(1.5, 1.0);
        }
        assert_eq!(guard.current_cap(), ActivationStrategy::Simplified);

        // ratio < 0.8×1.0 连续 5 次 → 升档回 Full
        for i in 0..4 {
            assert!(
                guard.observe(0.5, 1.0).is_none(),
                "第 {} 次回落计数中",
                i + 1
            );
        }
        let change = guard.observe(0.5, 1.0).expect("第 5 次回落应升档");
        assert_eq!(change.old_cap, ActivationStrategy::Simplified);
        assert_eq!(change.new_cap, ActivationStrategy::Full);
        assert_eq!(guard.current_cap(), ActivationStrategy::Full);
    }

    #[test]
    fn test_hysteresis_band_resets_both_counters() {
        // 滞后带内((0.8, 1.0] × threshold)徘徊不构成任何方向证据
        let guard = StrategyCapGuard::new(fast_config());

        // 2 次越阈 + 1 次带内 + 2 次越阈:计数被清零,不足 3 连续 → 不降档
        guard.observe(1.5, 1.0);
        guard.observe(1.5, 1.0);
        guard.observe(0.9, 1.0); // 带内:0.8 < 0.9 <= 1.0,双清零
        guard.observe(1.5, 1.0);
        assert!(guard.observe(1.5, 1.0).is_none(), "计数被带内清零,2 次不足");
        assert_eq!(guard.current_cap(), ActivationStrategy::Full, "不应降档");

        // 第 3 次连续越阈才降档
        let change = guard.observe(1.5, 1.0);
        assert!(change.is_some(), "连续 3 次越阈应降档");
    }

    #[test]
    fn test_over_and_under_reset_each_other() {
        // 越阈与回落互相清零对方计数(方向证据互斥)
        let guard = StrategyCapGuard::new(fast_config());
        guard.observe(1.5, 1.0); // over=1
        guard.observe(0.5, 1.0); // under=1, over=0
        guard.observe(1.5, 1.0); // over=1, under=0
        guard.observe(1.5, 1.0); // over=2
        assert!(guard.observe(1.5, 1.0).is_some(), "重新连续 3 次才降档");
    }

    #[test]
    fn test_min_dwell_blocks_raise_but_not_lower() {
        // 大驻留时间:升档被阻止,降档不受限(安全优先)
        let cfg = StrategyCapConfig {
            min_dwell_ms: 3_600_000, // 1 小时,测试内不可能满足
            ..fast_config()
        };
        let guard = StrategyCapGuard::new(cfg);

        // 降档不受驻留限制
        for _ in 0..3 {
            guard.observe(1.5, 1.0);
        }
        assert_eq!(guard.current_cap(), ActivationStrategy::Simplified);

        // 回落 10 次:驻留时间未满,不升档
        for _ in 0..10 {
            assert!(guard.observe(0.1, 1.0).is_none(), "驻留期内不应升档");
        }
        assert_eq!(guard.current_cap(), ActivationStrategy::Simplified);

        // 继续越阈:仍可继续降档(FastPath)
        for _ in 0..3 {
            guard.observe(1.5, 1.0);
        }
        assert_eq!(guard.current_cap(), ActivationStrategy::FastPath);
    }

    // === apply(min 上界)测试 ===

    #[test]
    fn test_apply_caps_strategy_to_upper_bound() {
        let guard = StrategyCapGuard::new(fast_config());

        // 封顶 Full:任何策略原样通过
        assert_eq!(
            guard.apply(ActivationStrategy::Full),
            ActivationStrategy::Full
        );
        assert_eq!(
            guard.apply(ActivationStrategy::FastPath),
            ActivationStrategy::FastPath
        );

        // 降到 Simplified:Full 被压到 Simplified,FastPath 不受影响
        for _ in 0..3 {
            guard.observe(1.5, 1.0);
        }
        assert_eq!(
            guard.apply(ActivationStrategy::Full),
            ActivationStrategy::Simplified,
            "封顶应压低超出上界的策略"
        );
        assert_eq!(
            guard.apply(ActivationStrategy::FastPath),
            ActivationStrategy::FastPath,
            "低于封顶的策略不受影响(min 语义)"
        );
    }

    #[test]
    fn test_infinity_ratio_counts_as_over() {
        // 增益为零时 ratio = INFINITY,应计为越阈证据
        let guard = StrategyCapGuard::new(fast_config());
        for _ in 0..3 {
            guard.observe(f64::INFINITY, 1.0);
        }
        assert_eq!(guard.current_cap(), ActivationStrategy::Simplified);
    }

    // === 订阅器集成测试 ===

    #[tokio::test]
    async fn test_subscriber_lowers_cap_and_publishes_change_event() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let guard = std::sync::Arc::new(StrategyCapGuard::new(fast_config()));
        let handle = spawn_strategy_cap_subscriber(std::sync::Arc::clone(&guard), bus.clone());

        // 发布 3 个越阈 ratio 报告(模拟 quest-engine 度量收集器)
        for _ in 0..3 {
            bus.publish(NexusEvent::CoordinationRatioReported {
                metadata: EventMetadata::new("quest-engine"),
                coordination_cost_ms: 2000.0,
                inference_gain: 0.5,
                cost_index: 1.0,
                gain_index: 0.5,
                ratio: 2.0,
                is_paradox_risk: true,
                threshold: 1.0,
                sample_count: 1,
            })
            .await
            .expect("发布应成功");
        }

        // 轮询等待订阅者消费并降档(异步投递,最多 2s)
        let mut lowered = false;
        for _ in 0..40 {
            if guard.current_cap() == ActivationStrategy::Simplified {
                lowered = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert!(lowered, "订阅者应消费 ratio 报告并降档封顶");

        // 应收到 ParliamentStrategyCapChanged 观测事件
        let mut saw_change = false;
        for _ in 0..30 {
            match tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await {
                Ok(Ok(NexusEvent::ParliamentStrategyCapChanged {
                    old_cap, new_cap, ..
                })) => {
                    assert_eq!(old_cap, "full");
                    assert_eq!(new_cap, "simplified");
                    saw_change = true;
                    break;
                }
                Ok(_) => continue,
                Err(_) => break,
            }
        }
        handle.abort();
        assert!(saw_change, "封顶变更应发布观测事件");
    }
}
