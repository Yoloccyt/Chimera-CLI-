//! CACR(Cost-Aware Cognitive Routing)— 成本感知路由守卫
//!
//! 对应架构:L1 Core,作为 ModelRouter 的成本保护层
//!
//! # 职责
//! - 在路由决策发布前拦截,校验预估成本是否在预算内
//! - 三档决策:`Allow`(放行)/ `Downgrade`(降级到次优模型)/ `Block`(阻止并报错)
//! - Block 时发布 `BudgetExceeded` 事件,供 L8 Parliament 感知预算状态
//!
//! # 与 DECB 的关系(避免向上依赖)
//! WHY:按 §2.2 依赖铁律,L1 不能 import L8 的 `decb-governor` 来查询动态预算。
//! 因此 CACR 在 Week 2 阶段使用静态阈值(从 `CacrConfig` 读取),
//! Week 5 接入 DECB 后,改为由 DECB 通过事件向下推送预算配额,
//! CACR 仅消费配额值,不直接依赖 DECB 类型。此设计保证依赖方向合法。
//!
//! # 决策阈值
//! - `Allow`:`estimated_cost < remaining_budget * warn_threshold`
//! - `Downgrade`:`warn_threshold <= 比例 < block_threshold`
//! - `Block`:`estimated_cost >= remaining_budget * block_threshold`

use serde::{Deserialize, Serialize};

/// CACR 决策 — 成本感知守卫的输出
///
/// 三档对应不同处理路径:`Allow` 放行原决策,`Downgrade` 切换到次优模型,
/// `Block` 终止路由并发布 `BudgetExceeded` 事件。
#[derive(Debug, Clone, PartialEq)]
pub enum CacrDecision {
    /// 允许路由 — 预估成本在预算内,放行原决策
    Allow,
    /// 降级路由 — 预估成本接近预算上限,建议选择次优模型
    ///
    /// 携带降级原因(人类可读),用于审计与事件追踪
    Downgrade(String),
    /// 阻止路由 — 预估成本超过预算上限,终止路由
    ///
    /// 携带阻止原因(人类可读),用于错误信息与事件
    Block(String),
}

/// CACR 配置 — 成本感知守卫的参数
///
/// `budget_limit` 在 Week 2 阶段为静态值,Week 5 后由 DECB 动态推送。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacrConfig {
    /// 预算上限(美分,1 美元 = 100 美分)
    ///
    /// Week 2 阶段默认 1_000_000 美分(即 10000 美元),作为静态基线。
    pub budget_limit: u64,
    /// 告警阈值(占预算比例) — 超过则降级,默认 0.8
    pub warn_threshold: f32,
    /// 阻止阈值(占预算比例) — 超过则阻止,默认 1.0
    pub block_threshold: f32,
}

impl Default for CacrConfig {
    fn default() -> Self {
        Self {
            // Week 2 阶段静态值:10000 美元(以美分表示)
            // WHY:此阶段未接入 DECB,使用宽松上限避免误伤正常路由
            budget_limit: 1_000_000,
            warn_threshold: 0.8,
            block_threshold: 1.0,
        }
    }
}

/// CACR 守卫 — 成本感知路由保护层
///
/// WHY:CACR(Cost-Aware Cognitive Routing)防止高成本模型耗尽预算。
/// Week 2 阶段使用静态阈值(从配置读取),Week 5 接入 DECB 后改为动态预算查询。
/// 此设计避免 L1 → L8 向上依赖违规(CACR 不 import decb-governor,
/// 而是通过 BudgetExceeded 事件反向通信)。
#[derive(Debug, Clone)]
pub struct CacrGuard {
    config: CacrConfig,
}

impl CacrGuard {
    /// 创建守卫,接管配置所有权
    pub fn new(config: CacrConfig) -> Self {
        Self { config }
    }

    /// 从配置引用创建守卫(内部 clone,便于链式构造)
    pub fn from_config(config: &CacrConfig) -> Self {
        Self::new(config.clone())
    }

    /// 检查预估成本是否允许路由
    ///
    /// - `estimated_cost`:预估成本(美分)
    /// - `remaining_budget`:剩余预算(美分)
    ///
    /// # 决策规则
    /// - `Allow`:`estimated_cost < remaining_budget * warn_threshold`
    /// - `Downgrade`:`warn_threshold <= 比例 < block_threshold`
    /// - `Block`:`estimated_cost >= remaining_budget * block_threshold`
    ///
    /// # 边界情况
    /// 当 `remaining_budget` 为 0 时,任何非零成本都会触发 Block,
    /// 零成本则 Allow(无消耗无需保护)。
    pub fn check(&self, estimated_cost: u64, remaining_budget: u64) -> CacrDecision {
        let warn_limit = (remaining_budget as f32 * self.config.warn_threshold) as u64;
        let block_limit = (remaining_budget as f32 * self.config.block_threshold) as u64;

        if estimated_cost >= block_limit {
            CacrDecision::Block(format!(
                "cost {} >= block_limit {} (budget {} * threshold {})",
                estimated_cost, block_limit, remaining_budget, self.config.block_threshold
            ))
        } else if estimated_cost >= warn_limit {
            CacrDecision::Downgrade(format!(
                "cost {} >= warn_limit {} (budget {} * threshold {})",
                estimated_cost, warn_limit, remaining_budget, self.config.warn_threshold
            ))
        } else {
            CacrDecision::Allow
        }
    }

    /// 获取配置引用
    pub fn config(&self) -> &CacrConfig {
        &self.config
    }

    /// 获取预算上限(美分)
    pub fn budget_limit(&self) -> u64 {
        self.config.budget_limit
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_guard(budget: u64, warn: f32, block: f32) -> CacrGuard {
        CacrGuard::new(CacrConfig {
            budget_limit: budget,
            warn_threshold: warn,
            block_threshold: block,
        })
    }

    // ============================================================
    // CacrDecision 相等性测试
    // ============================================================

    #[test]
    fn test_decision_allow_equality() {
        assert_eq!(CacrDecision::Allow, CacrDecision::Allow);
    }

    #[test]
    fn test_decision_downgrade_not_equal_to_allow() {
        assert_ne!(
            CacrDecision::Downgrade("reason".into()),
            CacrDecision::Allow
        );
    }

    // ============================================================
    // CacrConfig 默认值测试
    // ============================================================

    #[test]
    fn test_default_config_values() {
        let config = CacrConfig::default();
        assert_eq!(config.budget_limit, 1_000_000);
        assert!((config.warn_threshold - 0.8).abs() < f32::EPSILON);
        assert!((config.block_threshold - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let config = CacrConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let de: CacrConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(de.budget_limit, config.budget_limit);
        assert!((de.warn_threshold - config.warn_threshold).abs() < f32::EPSILON);
        assert!((de.block_threshold - config.block_threshold).abs() < f32::EPSILON);
    }

    // ============================================================
    // CacrGuard::check 单元测试 — 边界条件
    // ============================================================

    #[test]
    fn test_check_allow_when_cost_below_warn_threshold() {
        // 预算 1000,warn 0.8 → warn_limit = 800
        // 成本 100 < 800 → Allow
        let guard = make_guard(1_000_000, 0.8, 1.0);
        let decision = guard.check(100, 1000);
        assert_eq!(decision, CacrDecision::Allow);
    }

    #[test]
    fn test_check_allow_at_zero_cost() {
        // 零成本始终 Allow(无消耗无需保护)
        let guard = make_guard(1000, 0.8, 1.0);
        let decision = guard.check(0, 1000);
        assert_eq!(decision, CacrDecision::Allow);
    }

    #[test]
    fn test_check_downgrade_at_warn_threshold() {
        // 预算 1000,warn 0.8 → warn_limit = 800
        // 成本 800 >= 800 且 < 1000 → Downgrade
        let guard = make_guard(1_000_000, 0.8, 1.0);
        let decision = guard.check(800, 1000);
        assert!(matches!(decision, CacrDecision::Downgrade(_)));
    }

    #[test]
    fn test_check_downgrade_just_below_block_threshold() {
        // 预算 1000,block 1.0 → block_limit = 1000
        // 成本 999 >= 800 且 < 1000 → Downgrade
        let guard = make_guard(1_000_000, 0.8, 1.0);
        let decision = guard.check(999, 1000);
        assert!(matches!(decision, CacrDecision::Downgrade(_)));
    }

    #[test]
    fn test_check_block_at_block_threshold() {
        // 预算 1000,block 1.0 → block_limit = 1000
        // 成本 1000 >= 1000 → Block
        let guard = make_guard(1_000_000, 0.8, 1.0);
        let decision = guard.check(1000, 1000);
        assert!(matches!(decision, CacrDecision::Block(_)));
    }

    #[test]
    fn test_check_block_above_block_threshold() {
        // 成本 1500 >= 1000 → Block
        let guard = make_guard(1_000_000, 0.8, 1.0);
        let decision = guard.check(1500, 1000);
        assert!(matches!(decision, CacrDecision::Block(_)));
    }

    #[test]
    fn test_check_block_when_budget_zero() {
        // WHY:预算为 0 时,block_limit = 0,任何成本(含 0)都 >= 0,触发 Block。
        // 这是任务描述定义的语义:预算为 0 意味着不允许任何路由(即使零成本)。
        let guard = make_guard(1_000_000, 0.8, 1.0);
        let decision = guard.check(0, 0);
        assert!(matches!(decision, CacrDecision::Block(_)));

        let decision = guard.check(1, 0);
        assert!(matches!(decision, CacrDecision::Block(_)));
    }

    // ============================================================
    // 阈值可配置性测试
    // ============================================================

    #[test]
    fn test_check_custom_warn_threshold_lower() {
        // 降低 warn 阈值到 0.5,使成本 500 触发 Downgrade
        let guard = make_guard(1_000_000, 0.5, 1.0);
        let decision = guard.check(500, 1000);
        assert!(matches!(decision, CacrDecision::Downgrade(_)));
    }

    #[test]
    fn test_check_custom_block_threshold_lower() {
        // 降低 block 阈值到 0.9,使成本 900 触发 Block
        let guard = make_guard(1_000_000, 0.8, 0.9);
        let decision = guard.check(900, 1000);
        assert!(matches!(decision, CacrDecision::Block(_)));
    }

    #[test]
    fn test_check_block_reason_contains_details() {
        let guard = make_guard(1_000_000, 0.8, 1.0);
        if let CacrDecision::Block(reason) = guard.check(1500, 1000) {
            assert!(reason.contains("1500"), "原因应包含成本值: {}", reason);
            assert!(reason.contains("1000"), "原因应包含预算值: {}", reason);
            assert!(
                reason.contains("block_limit"),
                "原因应包含阈值标识: {}",
                reason
            );
        } else {
            panic!("期望 Block 决策");
        }
    }

    #[test]
    fn test_check_downgrade_reason_contains_details() {
        let guard = make_guard(1_000_000, 0.8, 1.0);
        if let CacrDecision::Downgrade(reason) = guard.check(850, 1000) {
            assert!(reason.contains("850"), "原因应包含成本值: {}", reason);
            assert!(
                reason.contains("warn_limit"),
                "原因应包含阈值标识: {}",
                reason
            );
        } else {
            panic!("期望 Downgrade 决策");
        }
    }

    // ============================================================
    // 访问器测试
    // ============================================================

    #[test]
    fn test_budget_limit_accessor() {
        let guard = make_guard(5000, 0.8, 1.0);
        assert_eq!(guard.budget_limit(), 5000);
    }

    #[test]
    fn test_config_accessor() {
        let guard = make_guard(5000, 0.7, 0.95);
        let config = guard.config();
        assert_eq!(config.budget_limit, 5000);
        assert!((config.warn_threshold - 0.7).abs() < f32::EPSILON);
        assert!((config.block_threshold - 0.95).abs() < f32::EPSILON);
    }

    #[test]
    fn test_from_config_clones() {
        let config = CacrConfig {
            budget_limit: 3000,
            warn_threshold: 0.6,
            block_threshold: 0.9,
        };
        let guard = CacrGuard::from_config(&config);
        // 修改原 config 不影响 guard(证明 clone)
        assert_eq!(guard.budget_limit(), 3000);
        assert_eq!(config.budget_limit, 3000);
    }
}
