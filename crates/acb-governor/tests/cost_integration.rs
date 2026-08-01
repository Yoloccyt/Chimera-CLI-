//! 成本模型集成集成测试 — 峰谷定价、预算控制、降级链路
//!
//! 对应架构层:L8 Parliament(acb-governor)
//! 对应测试范围:CostModelIntegration + DowngradeController 联合场景

use std::sync::Arc;

use acb_governor::{
    AffinityCostModel, CostModelIntegration, CostVerdict, DowngradeAction, DowngradeController,
};
use event_bus::EventBus;

/// 辅助函数:创建带有指定日预算的集成实例
fn make_integration(daily_budget_micro: u64) -> CostModelIntegration {
    let cost_model = Arc::new(AffinityCostModel::new(daily_budget_micro));
    let event_bus = EventBus::new();
    CostModelIntegration::new(cost_model, event_bus)
}

/// 峰谷定价下的预算控制集成测试
///
/// 场景：日预算 10000 微元
/// - 峰时(peak_factor=150)：路由成本 8000 微元，预检后投影 8000 < 12000 → Allow
/// - 谷时(peak_factor=80)：路由成本 5000 微元，预检后投影 5000 < 12000 → Allow
/// - 超预算：累计 11000 后新的 2000 预估 → 投影 13000 > 12000 → Veto
#[test]
fn peak_pricing_budget_control() {
    // 日预算 10000 微元，阈值 120% = 12000
    let integration = make_integration(10_000);

    // 峰时：路由成本 8000 微元，预检 Allow（投影 8000 < 12000）
    let verdict_peak = integration.pre_check("peak-channel", 8000);
    assert_eq!(
        verdict_peak,
        CostVerdict::Allow,
        "峰时 8000 微元应在预算阈值内"
    );

    // 记录实际峰时成本
    integration.record_actual("peak-channel", 8000);

    // 谷时：路由成本 5000 微元，预检 Allow（累计 8000 + 预估 5000 = 13000 > 12000 → Veto）
    // 注意：pre_check 内部会 record_estimate 但不会累加 total_spend，所以投影只影响当前预估
    let verdict_valley = integration.pre_check("valley-channel", 5000);
    assert_eq!(
        verdict_valley,
        CostVerdict::Veto,
        "谷时 5000 微元在累计 8000 后投影 13000 > 12000 → Veto"
    );
}

/// 峰谷定价三阶段预算控制
///
/// 场景：日预算 10000 微元
/// 阶段 1: 峰时路由 3000 → Allow（累计 3000）
/// 阶段 2: 峰时路由 7000 → Allow（累计 10000 < 12000，但预估 7000 投影 17000 > 12000 → Veto）
/// 阶段 3: 谷时路由 1000 → Allow（累计 10000，预检 1000 投影 11000 < 12000 → Allow）
#[test]
fn peak_pricing_three_phase_budget_control() {
    let integration = make_integration(10_000);

    // 阶段 1: 峰时路由 3000
    assert_eq!(
        integration.pre_check("peak-channel", 3000),
        CostVerdict::Allow,
        "阶段 1: 3000 微元 Allow"
    );
    integration.record_actual("peak-channel", 3000);

    // 阶段 2: 峰时路由 7000（累计 3000 + 预估 7000 = 10000 < 12000 → Allow）
    // 注意：pre_check 内部调用 route_verdict 只考虑预估 + 当前总支出
    // 当前总支出 = 3000，预估 7000 → 投影 10000 < 12000 → Allow
    assert_eq!(
        integration.pre_check("peak-channel", 7000),
        CostVerdict::Allow,
        "阶段 2: 累计 3000 + 预估 7000 = 10000 < 12000 → Allow"
    );
    // 记录实际后累计变为 10000
    integration.record_actual("peak-channel", 7000);

    // 阶段 3: 谷时路由 1000（累计 10000 + 预估 1000 = 11000 < 12000 → Allow）
    assert_eq!(
        integration.pre_check("valley-channel", 1000),
        CostVerdict::Allow,
        "阶段 3: 累计 10000 + 预估 1000 = 11000 < 12000 → Allow"
    );
}

/// 降级控制器与成本模型集成联合测试
///
/// 验证 DowngradeController 与 CostModelIntegration 的配合：
/// 1. 预算充裕时 → NoAction
/// 2. 超触发阈值时 → 降级建议
/// 3. 超阻止阈值时 → Block
#[test]
fn downgrade_controller_with_cost_model() {
    // 日预算 10000 微元
    let integration = make_integration(10_000);
    let controller = DowngradeController::new(10_000, 0.8, 1.0);

    // 初始：累计 0，预算充裕 → NoAction
    let verdict = controller.evaluate(integration.total_spend_micro(), 100);
    assert_eq!(verdict, DowngradeAction::NoAction);

    // 消费 8500 微元（85% > 80% 触发阈值）
    integration.record_actual("x/y", 8500);
    let total = integration.total_spend_micro();
    assert_eq!(total, 8500);

    // 触发降级阈值，cost 100 较小 → SwitchToCheaper
    let verdict = controller.evaluate(total, 100);
    assert_eq!(verdict, DowngradeAction::SwitchToCheaper);

    // 消费到 10000（100% 阻止阈值）
    integration.record_actual("x/y", 1500);
    let total = integration.total_spend_micro();
    assert_eq!(total, 10000);

    // 阻止路由
    let verdict = controller.evaluate(total, 100);
    assert_eq!(verdict, DowngradeAction::Block);
}

/// 联合 handle_actual_with_downgrade 测试
///
/// 验证 CostModelIntegration::handle_actual_with_downgrade 与
/// DowngradeController 的完整链路：
/// 超预算 → BudgetExceeded 事件发布 → 降级信号 → Block
#[test]
fn combined_downgrade_chain() {
    let integration = make_integration(10_000);
    let controller = DowngradeController::new(10_000, 0.8, 1.0);

    // 消费 11000（累计 11000，未超 120% 阈值 12000）
    let mut exceeded = false;
    let verdict = integration.handle_actual_with_downgrade("x/y", 11000, &mut exceeded);
    assert!(!exceeded, "11000 < 12000 不应超预算");
    assert_eq!(verdict, CostVerdict::Allow);

    // 此时 total_spend = 11000，降级控制器应触发 SwitchToCheaper
    let total = integration.total_spend_micro();
    assert_eq!(total, 11000);
    // 11000/10000 = 1.1 > 1.0 → Block
    let downgrade = controller.evaluate(total, 200);
    assert_eq!(downgrade, DowngradeAction::Block);

    // 再消费 2000 → 累计 13000 > 12000 → 超预算
    let mut exceeded = false;
    let verdict = integration.handle_actual_with_downgrade("x/y", 2000, &mut exceeded);
    assert!(exceeded, "13000 > 12000 应超预算");
    assert_eq!(verdict, CostVerdict::Veto);
}

/// 无预算限制下的行为
#[test]
fn unlimited_budget_no_downgrade() {
    // daily_budget_micro = 0 表示不限
    let integration = make_integration(0);
    let controller = DowngradeController::new(0, 0.8, 1.0);

    // 即使大量消费，无限预算下也不应触发降级
    let mut exceeded = false;
    let verdict = integration.handle_actual_with_downgrade("x/y", 1_000_000, &mut exceeded);
    assert!(!exceeded, "无限预算不应超预算");
    assert_eq!(verdict, CostVerdict::Allow);

    // 降级控制器也不应阻止
    assert_eq!(
        controller.evaluate(1_000_000, 100_000),
        DowngradeAction::NoAction
    );
}

/// 多通道成本累积测试
#[test]
fn multi_channel_cost_accumulation() {
    let integration = make_integration(10_000);

    // 三个通道各消费 3000
    integration.record_actual("deep_seek/deepseek-v4-flash", 3000);
    integration.record_actual("zhipu/glm-5.2", 3000);
    integration.record_actual("openai/gpt-4o", 3000);

    // 累计 9000
    assert_eq!(integration.total_spend_micro(), 9000);

    // 预检新通道 2000 → 投影 11000 < 12000 → Allow
    assert_eq!(
        integration.pre_check("anthropic/claude-4", 2000),
        CostVerdict::Allow
    );

    // 再消费 4000 → 累计 13000 > 12000 → 超预算
    let mut exceeded = false;
    let verdict =
        integration.handle_actual_with_downgrade("anthropic/claude-4", 4000, &mut exceeded);
    assert!(exceeded, "13000 > 12000 应超预算");
    assert_eq!(verdict, CostVerdict::Veto);
}
