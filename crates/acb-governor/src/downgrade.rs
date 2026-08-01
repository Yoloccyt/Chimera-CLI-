//! 预算超限降级策略 — 成本超限时的通道切换决策(ADR-068)
//!
//! 对应架构层:L8 Parliament(acb-governor)
//!
//! # 职责
//! 为"预算超限 → 切换到低成本通道"提供结构化决策支持。
//! 不含路由逻辑，只产出降级建议。
//!
//! # 降级链路
//! 预算充裕 → 廉价通道 → 降级思考模式 → 降级模型 → 阻止路由
//!
//! # 触发阈值
//! - trigger_threshold(默认 0.8):预算已用 80% 时触发降级建议
//! - block_threshold(默认 1.0):预算已用 100% 时阻止路由

/// 预算超限降级动作 — 成本超限时的通道切换决策(ADR-068)
///
/// 为"预算超限 → 切换到低成本通道"提供结构化决策支持。
/// 不含路由逻辑，只产出降级建议。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DowngradeAction {
    /// 不做降级(预算充裕)
    NoAction,
    /// 建议降级到廉价通道
    SwitchToCheaper,
    /// 建议降级思考模式(Deep → Standard → Fast)
    ReduceThinking,
    /// 建议降级模型(Pro → Flash)
    ReduceModel,
    /// 建议阻止路由(预算耗尽)
    Block,
}

impl DowngradeAction {
    /// 返回人类可读名称
    pub fn as_str(&self) -> &'static str {
        match self {
            DowngradeAction::NoAction => "no_action",
            DowngradeAction::SwitchToCheaper => "switch_to_cheaper",
            DowngradeAction::ReduceThinking => "reduce_thinking",
            DowngradeAction::ReduceModel => "reduce_model",
            DowngradeAction::Block => "block",
        }
    }
}

impl std::fmt::Display for DowngradeAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// 预算超限降级控制器
///
/// 根据当前预算状态和成本超限幅度，产出降级建议。
/// 实际降级执行由调用方(model-router/quest-engine)完成。
///
/// # 决策逻辑
/// 1. `total_spend < daily_budget × trigger_threshold` → NoAction
/// 2. `total_spend < daily_budget × block_threshold` → 按 cost 阶梯降级:
///    - cost 较小 → SwitchToCheaper
///    - cost 中等 → ReduceThinking
///    - cost 较大 → ReduceModel
/// 3. `total_spend >= daily_budget × block_threshold` → Block
///
/// # 默认阈值
/// - trigger_threshold: 0.8(预算用 80% 触发降级)
/// - block_threshold: 1.0(预算用尽阻止路由)
pub struct DowngradeController {
    /// 日预算上限(微元)
    daily_budget_micro: u64,
    /// 降级触发阈值(预算已用比例,默认 0.8)
    trigger_threshold: f32,
    /// 阻止阈值(预算已用比例,默认 1.0)
    block_threshold: f32,
}

impl DowngradeController {
    /// 创建降级控制器
    ///
    /// # 参数
    /// - `daily_budget_micro`: 日预算上限(微元)
    /// - `trigger_threshold`: 降级触发阈值(预算已用比例,默认 0.8)
    /// - `block_threshold`: 阻止阈值(预算已用比例,默认 1.0)
    ///
    /// # 校验
    /// 0.0 <= trigger_threshold < block_threshold <= 1.0
    /// 否则自动 clamp 到合法范围。
    pub fn new(daily_budget_micro: u64, trigger_threshold: f32, block_threshold: f32) -> Self {
        let trigger = trigger_threshold.clamp(0.0, 1.0);
        let block = block_threshold.clamp(trigger.max(0.0), 1.0);
        // 确保 block > trigger
        let block = if block <= trigger {
            (trigger + 0.1).min(1.0)
        } else {
            block
        };
        Self {
            daily_budget_micro,
            trigger_threshold: trigger,
            block_threshold: block,
        }
    }

    /// 评估当前支出状态，产出降级建议
    ///
    /// # 参数
    /// - `total_spend`: 当前累计总支出(微元)
    /// - `current_cost`: 当前请求的成本(微元)，用于细化降级阶梯
    ///
    /// # 返回
    /// 降级建议，不含路由逻辑，仅做决策参考。
    pub fn evaluate(&self, total_spend: u64, current_cost: u64) -> DowngradeAction {
        if self.daily_budget_micro == 0 {
            return DowngradeAction::NoAction;
        }

        let spend_ratio = total_spend as f32 / self.daily_budget_micro as f32;

        // 预算已用比例 >= 阻止阈值 → 阻止路由
        if spend_ratio >= self.block_threshold {
            return DowngradeAction::Block;
        }

        // 预算已用比例 < 触发阈值 → 不做降级
        if spend_ratio < self.trigger_threshold {
            return DowngradeAction::NoAction;
        }

        // 触发阈值 ≤ 预算已用比例 < 阻止阈值 → 按成本阶梯降级
        // 使用当前成本占日预算的比例来细化降级阶梯
        let cost_ratio = current_cost as f32 / self.daily_budget_micro.max(1) as f32;

        if cost_ratio < 0.05 {
            // 小成本请求 → 切到廉价通道
            DowngradeAction::SwitchToCheaper
        } else if cost_ratio < 0.15 {
            // 中等成本 → 降级思考模式
            DowngradeAction::ReduceThinking
        } else {
            // 大成本 → 降级模型
            DowngradeAction::ReduceModel
        }
    }

    /// 是否应阻止路由(预算耗尽)
    pub fn should_block(&self, total_spend: u64) -> bool {
        if self.daily_budget_micro == 0 {
            return false;
        }
        let spend_ratio = total_spend as f32 / self.daily_budget_micro as f32;
        spend_ratio >= self.block_threshold
    }

    /// 是否应触发降级(预算已用超过触发阈值)
    pub fn should_downgrade(&self, total_spend: u64) -> bool {
        if self.daily_budget_micro == 0 {
            return false;
        }
        let spend_ratio = total_spend as f32 / self.daily_budget_micro as f32;
        spend_ratio >= self.trigger_threshold
    }

    /// 返回日预算上限(微元)
    pub fn daily_budget_micro(&self) -> u64 {
        self.daily_budget_micro
    }

    /// 返回降级触发阈值
    pub fn trigger_threshold(&self) -> f32 {
        self.trigger_threshold
    }

    /// 返回阻止阈值
    pub fn block_threshold(&self) -> f32 {
        self.block_threshold
    }
}

impl Default for DowngradeController {
    /// 默认配置：日预算 1000000 微元，触发阈值 0.8，阻止阈值 1.0
    fn default() -> Self {
        Self::new(1_000_000, 0.8, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_controller() {
        let ctrl = DowngradeController::default();
        assert_eq!(ctrl.daily_budget_micro(), 1_000_000);
        assert!((ctrl.trigger_threshold() - 0.8).abs() < 1e-6);
        assert!((ctrl.block_threshold() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_no_action_when_below_trigger() {
        let ctrl = DowngradeController::new(1000, 0.8, 1.0);
        // 已用 500/1000 = 0.5 < 0.8 → NoAction
        assert_eq!(ctrl.evaluate(500, 100), DowngradeAction::NoAction);
    }

    #[test]
    fn test_block_when_at_block_threshold() {
        let ctrl = DowngradeController::new(1000, 0.8, 1.0);
        // 已用 1000/1000 = 1.0 >= 1.0 → Block
        assert_eq!(ctrl.evaluate(1000, 100), DowngradeAction::Block);
    }

    #[test]
    fn test_block_when_above_block_threshold() {
        let ctrl = DowngradeController::new(1000, 0.8, 1.0);
        // 已用 1200/1000 = 1.2 >= 1.0 → Block
        assert_eq!(ctrl.evaluate(1200, 100), DowngradeAction::Block);
    }

    #[test]
    fn test_switch_to_cheaper_for_small_cost() {
        let ctrl = DowngradeController::new(1000, 0.5, 1.0);
        // 已用 600/1000 = 0.6 >= 0.5, cost 30/1000 = 0.03 < 0.05 → SwitchToCheaper
        assert_eq!(ctrl.evaluate(600, 30), DowngradeAction::SwitchToCheaper);
    }

    #[test]
    fn test_reduce_thinking_for_medium_cost() {
        let ctrl = DowngradeController::new(1000, 0.5, 1.0);
        // 已用 600/1000 = 0.6 >= 0.5, cost 100/1000 = 0.1 ∈ [0.05, 0.15) → ReduceThinking
        assert_eq!(ctrl.evaluate(600, 100), DowngradeAction::ReduceThinking);
    }

    #[test]
    fn test_reduce_model_for_large_cost() {
        let ctrl = DowngradeController::new(1000, 0.5, 1.0);
        // 已用 600/1000 = 0.6 >= 0.5, cost 200/1000 = 0.2 >= 0.15 → ReduceModel
        assert_eq!(ctrl.evaluate(600, 200), DowngradeAction::ReduceModel);
    }

    #[test]
    fn test_should_block() {
        let ctrl = DowngradeController::new(1000, 0.8, 1.0);
        assert!(!ctrl.should_block(500));
        assert!(ctrl.should_block(1000));
        assert!(ctrl.should_block(1200));
    }

    #[test]
    fn test_should_downgrade() {
        let ctrl = DowngradeController::new(1000, 0.8, 1.0);
        assert!(!ctrl.should_downgrade(500));
        assert!(ctrl.should_downgrade(800));
        assert!(ctrl.should_downgrade(1000));
    }

    #[test]
    fn test_unlimited_budget_no_action() {
        // 日预算为 0 表示不限
        let ctrl = DowngradeController::new(0, 0.8, 1.0);
        assert_eq!(ctrl.evaluate(1_000_000, 100_000), DowngradeAction::NoAction);
        assert!(!ctrl.should_block(1_000_000));
        assert!(!ctrl.should_downgrade(1_000_000));
    }

    #[test]
    fn test_threshold_clamping() {
        // 触发阈值 > 阻止阈值时自动修正
        let ctrl = DowngradeController::new(1000, 0.9, 0.5);
        // trigger 被 clamp 到 0.5, block 被修正为 0.6
        assert!(ctrl.trigger_threshold() <= ctrl.block_threshold());
    }

    #[test]
    fn test_downgrade_action_display() {
        assert_eq!(DowngradeAction::NoAction.to_string(), "no_action");
        assert_eq!(
            DowngradeAction::SwitchToCheaper.to_string(),
            "switch_to_cheaper"
        );
        assert_eq!(
            DowngradeAction::ReduceThinking.to_string(),
            "reduce_thinking"
        );
        assert_eq!(DowngradeAction::ReduceModel.to_string(), "reduce_model");
        assert_eq!(DowngradeAction::Block.to_string(), "block");
    }
}
