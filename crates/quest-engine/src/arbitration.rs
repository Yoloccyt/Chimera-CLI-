//! ArbitrationLayer — ACB/DECB 预算档位保守取严仲裁层
//!
//! 对应架构层:L9 Quest (TTG 子模块)
//! 对应创新点:N7 TTG ACB/DECB 仲裁层
//!
//! # 核心职责
//! 订阅 EventBus 上的 `BudgetAdjusted` 事件(Parliament topic),
//! 区分 ACB(L0-L3 四级)与 DECB(HighTier/LowTier/Degraded 三档)来源,
//! 应用保守取严策略返回有效 DECB 档位供 TTG 决策。
//!
//! # 保守取严策略(WHY)
//! ACB 是细粒度四级预算,L0 表示预算接近耗尽;DECB 是粗粒度三档,
//! HighTier 表示资源充足。当两者矛盾时,取更保守(更低)的档位:
//! - ACB L0 → DECB Degraded(无论 DECB 报告什么,立即降级)
//! - ACB L1 → DECB LowTier(ACB 基础级别映射为 DECB 低档)
//! - ACB L2/L3 → 跟随 DECB(ACB 资源充足,信任 DECB 的粗粒度判断)
//!
//! 这是 defense-in-depth 原则在预算治理中的体现:细粒度监控器(ACB)
//! 发出降级信号时,即使粗粒度监控器(DECB)认为资源充足,也应立即
//! 采取保守措施防止预算溢出。
//!
//! # 事件解析
//! `BudgetAdjusted` 事件携带 `metadata.source` 字段标识发布者:
//! - `"acb-governor"` → ACB 事件,`new_tier` 为 "L0_degraded"/"L1_basic"/"L2_standard"/"L3_abundant"
//! - `"decb-governor"` → DECB 事件,`new_tier` 为 "high_tier"/"low_tier"/"degraded"
//!
//! ArbitrationLayer 不依赖 `acb-governor` crate,通过字符串解析避免
//! L9→L8 的额外编译依赖(虽然依赖方向允许,但保持最小依赖原则)。
//!
//! # 线程安全
//! - `acb_tier` / `decb_tier` 用 `Mutex` 保护,check-then-act 原子化
//! - `subscriber` 用 `Option<FilteredSubscriber>`,构造时同步订阅(§4.4 #3)
//! - `drain_events` / `arbitrated_tier` 为同步方法,不跨 await

use std::collections::HashSet;
use std::sync::Mutex;

use decb_governor::BudgetTier;
use event_bus::{EventBus, EventTopic, FilteredSubscriber, NexusEvent};
use tracing::{debug, warn};

// ============================================================
// AcbTier — ACB 四级预算枚举(内部解析用,不依赖 acb-governor crate)
// ============================================================

/// ACB 预算级别 — 解析自 ACB BudgetAdjusted 事件的 new_tier 字符串
///
/// WHY 独立枚举而非依赖 acb_governor::BudgetTier:quest-engine 已依赖
/// decb_governor,再依赖 acb_governor 会增加编译时间与耦合度。
/// ACB 级别仅需 4 个变体的字符串解析,独立枚举更轻量。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AcbTier {
    /// L0 降级模式 — 预算接近耗尽
    L0,
    /// L1 基础级别 — 资源受限
    L1,
    /// L2 标准级别 — 常规 Quest
    L2,
    /// L3 充足级别 — 资源充沛
    L3,
}

impl AcbTier {
    /// 从 ACB BudgetTier 的 Display 字符串解析
    ///
    /// ACB BudgetTier::as_str() 返回值:
    /// - "L0_degraded" → L0
    /// - "L1_basic" → L1
    /// - "L2_standard" → L2
    /// - "L3_abundant" → L3
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "L0_degraded" => Some(Self::L0),
            "L1_basic" => Some(Self::L1),
            "L2_standard" => Some(Self::L2),
            "L3_abundant" => Some(Self::L3),
            _ => None,
        }
    }
}

// ============================================================
// ArbitrationLayer — 仲裁层主结构
// ============================================================

/// ACB/DECB 预算档位仲裁层
///
/// 订阅 `BudgetAdjusted` 事件,综合 ACB 与 DECB 两个治理器的信号,
/// 返回保守取严后的有效 DECB 档位。
///
/// # 使用场景
/// - TTG 在 `select_mode` 前调用 `arbitrated_tier()` 获取有效档位
/// - 当 ACB 发出 L0/L1 降级信号时,即使 DECB 报告 HighTier 也应降级
/// - 无事件时返回 None,调用方使用 fallback 档位
///
/// # 构造
/// - [`new`](Self::new):通过 EventBus 订阅 Parliament topic
/// - [`disabled`](Self::disabled):不订阅事件,所有方法返回 None
///
/// # 架构红线
/// - `subscribe_filtered` 在构造时同步调用(§4.4 #3 broadcast 订阅时序)
/// - `drain_events` / `arbitrated_tier` 为同步方法,不跨 await
/// - 锁内仅做状态更新,不持有锁跨 await
pub struct ArbitrationLayer {
    /// 最新 ACB 级别(解析自 "acb-governor" source 的 BudgetAdjusted 事件)
    acb_tier: Mutex<Option<AcbTier>>,
    /// 最新 DECB 档位(解析自 "decb-governor" source 的 BudgetAdjusted 事件)
    decb_tier: Mutex<Option<BudgetTier>>,
    /// FilteredSubscriber 订阅 Parliament topic(包含 BudgetAdjusted)
    ///
    /// WHY Mutex<Option<...>>:FilteredSubscriber::try_recv 需要 &mut self,
    /// 但 drain_events/arbitrated_tier 为 &self 方法(供 TtgGovernor 调用)。
    /// Mutex 提供内部可变性,与 N9 PrerequisiteChecker 一致。
    subscriber: Mutex<Option<FilteredSubscriber>>,
    /// 是否启用仲裁
    enabled: bool,
}

impl ArbitrationLayer {
    /// 创建仲裁层并订阅 EventBus 上的 Parliament 事件
    ///
    /// WHY 同步订阅:FilteredSubscriber 必须在构造时同步调用
    /// `subscribe_filtered`,确保 `tokio::spawn` 前订阅者已注册,
    /// 避免事件静默丢失(§4.4 #3 broadcast 订阅时序)。
    pub fn new(event_bus: &EventBus) -> Self {
        let mut topics = HashSet::new();
        topics.insert(EventTopic::Parliament);
        let subscriber = event_bus.subscribe_filtered(topics);
        Self {
            acb_tier: Mutex::new(None),
            decb_tier: Mutex::new(None),
            subscriber: Mutex::new(Some(subscriber)),
            enabled: true,
        }
    }

    /// 创建禁用的仲裁层(不订阅任何事件)
    ///
    /// WHY 保留 disabled 模式:测试场景或不需要仲裁时,
    /// `arbitrated_tier()` 始终返回 None,让调用方使用 fallback。
    pub fn disabled() -> Self {
        Self {
            acb_tier: Mutex::new(None),
            decb_tier: Mutex::new(None),
            subscriber: Mutex::new(None),
            enabled: false,
        }
    }

    /// 是否启用仲裁
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// 非阻塞排空待处理事件,更新内部 ACB/DECB 状态
    ///
    /// 调用 FilteredSubscriber::try_recv() 循环排空缓冲区,
    /// 解析 BudgetAdjusted 事件的 source 与 new_tier 字段,
    /// 更新对应的 acb_tier / decb_tier。
    ///
    /// WHY 非阻塞:仲裁层不阻塞事件循环,try_recv 返回 None 时立即返回。
    /// 不匹配 Parliament topic 的事件被 FilteredSubscriber 自动消费丢弃。
    pub fn drain_events(&self) {
        // 锁住 subscriber 以获取 &mut FilteredSubscriber
        let mut subscriber_guard = match self.subscriber.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        let subscriber = match subscriber_guard.as_mut() {
            Some(s) => s,
            None => return,
        };

        // 用局部变量收集解析结果,锁外解析锁内更新(避免持锁跨复杂逻辑)
        let mut new_acb: Option<AcbTier> = None;
        let mut new_decb: Option<BudgetTier> = None;

        loop {
            match subscriber.try_recv() {
                Ok(Some(event)) => {
                    if let NexusEvent::BudgetAdjusted {
                        metadata, new_tier, ..
                    } = &event
                    {
                        if metadata.source == "acb-governor" {
                            if let Some(tier) = AcbTier::from_str(new_tier) {
                                new_acb = Some(tier);
                                debug!(acb_tier = ?tier, "ACB 预算级别更新");
                            } else {
                                warn!(
                                    source = %metadata.source,
                                    new_tier = %new_tier,
                                    "无法解析 ACB 级别字符串,跳过"
                                );
                            }
                        } else if metadata.source == "decb-governor" {
                            if let Some(tier) = parse_decb_tier(new_tier) {
                                new_decb = Some(tier);
                                debug!(decb_tier = ?tier, "DECB 预算档位更新");
                            } else {
                                warn!(
                                    source = %metadata.source,
                                    new_tier = %new_tier,
                                    "无法解析 DECB 档位字符串,跳过"
                                );
                            }
                        }
                        // 其他 source 的 BudgetAdjusted 事件被忽略(未来扩展点)
                    }
                }
                Ok(None) => break, // 缓冲区已排空
                Err(e) => {
                    warn!(error = %e, "FilteredSubscriber try_recv 错误,停止排空");
                    break;
                }
            }
        }
        // subscriber_guard 在此 drop,释放 Mutex 锁

        // 锁内更新状态(短临界区,不跨 await)
        if new_acb.is_some() {
            if let Ok(mut guard) = self.acb_tier.lock() {
                *guard = new_acb;
            }
        }
        if new_decb.is_some() {
            if let Ok(mut guard) = self.decb_tier.lock() {
                *guard = new_decb;
            }
        }
    }

    /// 返回仲裁后的有效 DECB 档位(保守取严策略)
    ///
    /// 策略:
    /// - ACB L0 → `Degraded`(无论 DECB 报告什么)
    /// - ACB L1 → `LowTier`
    /// - ACB L2/L3 或无 ACB 事件 → 跟随 DECB 最新档位
    /// - 无任何事件 → `None`(调用方使用 fallback)
    ///
    /// WHY 先 drain 再仲裁:确保仲裁结果反映最新事件状态。
    /// drain_events 是非阻塞的,不会阻塞事件循环。
    pub fn arbitrated_tier(&self) -> Option<BudgetTier> {
        self.drain_events();

        let acb = self
            .acb_tier
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let decb = self
            .decb_tier
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        match *acb {
            // ACB L0:最保守,立即降级
            Some(AcbTier::L0) => Some(BudgetTier::Degraded),
            // ACB L1:基础级别,限制为 LowTier
            Some(AcbTier::L1) => Some(BudgetTier::LowTier),
            // ACB L2/L3 或无 ACB 事件:信任 DECB 判断
            Some(AcbTier::L2) | Some(AcbTier::L3) | None => *decb,
        }
    }
}

// ============================================================
// 辅助函数 — DECB BudgetTier 字符串解析
// ============================================================

/// 从 DECB BudgetAdjusted 事件的 new_tier 字符串解析 DECB BudgetTier
///
/// DECB BudgetTier::as_str() 返回值:
/// - "high_tier" → HighTier
/// - "low_tier" → LowTier
/// - "degraded" → Degraded
fn parse_decb_tier(s: &str) -> Option<BudgetTier> {
    match s {
        "high_tier" => Some(BudgetTier::HighTier),
        "low_tier" => Some(BudgetTier::LowTier),
        "degraded" => Some(BudgetTier::Degraded),
        _ => None,
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_acb_tier_from_str() {
        assert_eq!(AcbTier::from_str("L0_degraded"), Some(AcbTier::L0));
        assert_eq!(AcbTier::from_str("L1_basic"), Some(AcbTier::L1));
        assert_eq!(AcbTier::from_str("L2_standard"), Some(AcbTier::L2));
        assert_eq!(AcbTier::from_str("L3_abundant"), Some(AcbTier::L3));
        assert_eq!(AcbTier::from_str("unknown"), None);
    }

    #[test]
    fn test_parse_decb_tier() {
        assert_eq!(parse_decb_tier("high_tier"), Some(BudgetTier::HighTier));
        assert_eq!(parse_decb_tier("low_tier"), Some(BudgetTier::LowTier));
        assert_eq!(parse_decb_tier("degraded"), Some(BudgetTier::Degraded));
        assert_eq!(parse_decb_tier("unknown"), None);
    }

    #[test]
    fn test_disabled_layer_returns_none() {
        let layer = ArbitrationLayer::disabled();
        assert!(!layer.is_enabled());
        assert_eq!(layer.arbitrated_tier(), None);
    }
}
