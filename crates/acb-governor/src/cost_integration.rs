//! 路由成本模型集成 — 路由决策前置预检与后置回算(ADR-068)
//!
//! 对应架构层:L8 Parliament(acb-governor)
//!
//! # 职责
//! 1. 路由前预检：查询 acb-governor 当前预算状态，判定路由是否允许
//! 2. 路由后回算：接收实际成本，更新 EWMA 和累计支出
//! 3. 降级触发：超预算时返回降级信号，调用方据此切换通道
//!
//! # 微元整数口径
//! 所有成本单位为微元(µ¥/µ$,1e-6)，与 AffinityCostModel 一致。
//!
//! # 依赖方向(§2.2 铁律)
//! acb-governor(L8) 消费 event-bus(L1) 的事件，不依赖上层 crate。

use std::sync::Arc;

use event_bus::{EventBus, EventMetadata, NexusEvent};
use tracing::warn;

use crate::cost_model::{AffinityCostModel, CostVerdict};

/// 路由成本模型集成 — 路由决策前置预检与后置回算(ADR-068)
///
/// 统一管理路由成本预检、实际成本回算和 BudgetExceeded 事件发布，
/// 使 mca-gateway 等调用方无需直接操作 AffinityCostModel 和 EventBus。
///
/// # 使用流程
/// 1. 路由决策前:调用 `pre_check(route_key, estimate_micro)` 获取 CostVerdict
/// 2. 路由决策后:调用 `record_estimate(route_key, actual_micro)` 记录预估成本
/// 3. 会话完成后:调用 `record_actual(route_key, actual_micro)` 记录实际成本
/// 4. 超预算时:调用 `handle_actual_with_downgrade` 自动触发降级信号
pub struct CostModelIntegration {
    /// 成本模型（通道成本聚合 + 预算治理）
    cost_model: Arc<AffinityCostModel>,
    /// 事件总线（发布 BudgetExceeded 事件）
    event_bus: EventBus,
}

impl CostModelIntegration {
    /// 创建成本模型集成实例
    pub fn new(cost_model: Arc<AffinityCostModel>, event_bus: EventBus) -> Self {
        Self {
            cost_model,
            event_bus,
        }
    }

    /// 路由前预检：查询成本模型当前预算状态，判定路由是否允许
    ///
    /// 内部调用 `cost_model.record_estimate` 更新预估成本 EWMA，
    /// 再调用 `cost_model.route_verdict` 获取预算裁决。
    ///
    /// # 返回
    /// - `CostVerdict::Allow`: 预估成本在预算阈值内，允许路由
    /// - `CostVerdict::Veto`: 预估成本推预算过阈，应回落廉价档或换通道
    pub fn pre_check(&self, route_key: &str, estimate_micro: u64) -> CostVerdict {
        // 记录预估成本（用于 EWMA 统计）
        self.cost_model.record_estimate(route_key, estimate_micro);
        // 返回预算裁决
        self.cost_model.route_verdict(estimate_micro)
    }

    /// 记录预估成本（仅更新 EWMA，不触发预算检查）
    ///
    /// 在路由决策后、会话开始前调用，用于更新通道成本 EWMA。
    pub fn record_estimate(&self, route_key: &str, cost_micro: u64) {
        self.cost_model.record_estimate(route_key, cost_micro);
    }

    /// 记录实际成本并返回是否超预算
    ///
    /// 在会话完成后调用，回写 EWMA + 累计总支出。
    /// 返回 `true` 表示日成本已超过预算 × 120%，调用方应据此触发降级。
    pub fn record_actual(&self, route_key: &str, cost_micro: u64) -> bool {
        self.cost_model.record_actual(route_key, cost_micro)
    }

    /// 记录实际成本后自动降级判定（含 BudgetExceeded 事件发布）
    ///
    /// 结合 `record_actual` 与 BudgetExceeded 事件发布，
    /// 通过 `exceeded` 传出参数告知调用方是否超预算。
    ///
    /// # 参数
    /// - `route_key`: 路由通道标识
    /// - `cost_micro`: 实际成本(微元)
    /// - `exceeded`: 传出参数，接收是否超预算
    ///
    /// # 返回
    /// - `CostVerdict::Allow`: 预算充裕，后续路由可继续
    /// - `CostVerdict::Veto`: 预算超限，应阻止后续路由
    pub fn handle_actual_with_downgrade(
        &self,
        route_key: &str,
        cost_micro: u64,
        exceeded: &mut bool,
    ) -> CostVerdict {
        *exceeded = self.record_actual(route_key, cost_micro);
        if *exceeded {
            // 发布 BudgetExceeded 事件（Critical 级，必须确保送达）
            let event = NexusEvent::BudgetExceeded {
                metadata: EventMetadata::new("acb-governor/cost_integration"),
                budget_type: "cost_micro".to_string(),
                current: self.cost_model.total_spend_micro(),
                limit: self.cost_model.daily_budget_micro(),
            };
            if let Err(e) = self.event_bus.publish_blocking(event) {
                warn!(error = %e, route_key, cost_micro, "发布 BudgetExceeded 事件失败");
            }
            CostVerdict::Veto
        } else {
            CostVerdict::Allow
        }
    }

    /// 返回当前累计总支出（微元）
    pub fn total_spend_micro(&self) -> u64 {
        self.cost_model.total_spend_micro()
    }

    /// 返回通道实际成本 EWMA（微元；未记录返回 None）
    pub fn channel_actual_ewma(&self, route_key: &str) -> Option<f64> {
        self.cost_model.channel_actual_ewma(route_key)
    }

    /// 返回内部成本模型引用（供调试/监控使用）
    pub fn cost_model(&self) -> &Arc<AffinityCostModel> {
        &self.cost_model
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cost_model::AffinityCostModel;

    fn make_integration() -> CostModelIntegration {
        let cost_model = Arc::new(AffinityCostModel::new(10_000));
        let event_bus = EventBus::new();
        CostModelIntegration::new(cost_model, event_bus)
    }

    #[test]
    fn test_pre_check_allows_within_budget() {
        let integration = make_integration();
        // 日预算 10000 微元，阈值 120% = 12000
        // 预估 1000 微元，投影 1000 < 12000 → Allow
        assert_eq!(
            integration.pre_check("deep_seek/deepseek-v4-flash", 1000),
            CostVerdict::Allow
        );
    }

    #[test]
    fn test_pre_check_vetoes_when_over_budget() {
        let integration = make_integration();
        // 先花 11000，再预估 2000 → 投影 13000 > 12000 → Veto
        integration.record_actual("x/y", 11000);
        assert_eq!(integration.pre_check("x/y", 2000), CostVerdict::Veto);
    }

    #[test]
    fn test_record_actual_updates_spend() {
        let integration = make_integration();
        assert!(!integration.record_actual("x/y", 5000));
        assert_eq!(integration.total_spend_micro(), 5000);
    }

    #[test]
    fn test_record_actual_returns_over_budget() {
        let integration = make_integration();
        // 日预算 10000，阈值 120% = 12000
        // 花 11000 未超阈值
        assert!(!integration.record_actual("x/y", 11000));
        // 再花 2000 → 累计 13000 > 12000 → 超预算
        assert!(integration.record_actual("x/y", 2000));
    }

    #[test]
    fn test_handle_actual_with_downgrade_triggers_exceeded() {
        let integration = make_integration();
        let mut exceeded = false;
        // 日预算 10000，12000 超阈值
        let verdict = integration.handle_actual_with_downgrade("x/y", 13000, &mut exceeded);
        assert!(exceeded);
        assert_eq!(verdict, CostVerdict::Veto);
    }

    #[test]
    fn test_handle_actual_with_downgrade_within_budget() {
        let integration = make_integration();
        let mut exceeded = true;
        // 5000 在阈值内
        let verdict = integration.handle_actual_with_downgrade("x/y", 5000, &mut exceeded);
        assert!(!exceeded);
        assert_eq!(verdict, CostVerdict::Allow);
    }

    #[test]
    fn test_channel_ewma_after_record() {
        let integration = make_integration();
        assert!(integration.channel_actual_ewma("x/y").is_none());
        integration.record_actual("x/y", 1000);
        assert!(integration.channel_actual_ewma("x/y").is_some());
    }
}
