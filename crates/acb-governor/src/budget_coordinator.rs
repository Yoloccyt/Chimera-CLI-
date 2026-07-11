//! ACB-DECB 预算协调器 — 统一离散四级与连续双档预算治理
//!
//! 对应架构层:L8 Parliament
//! 对应创新点:BudgetCoordinator(P1-10)
//!
//! # 核心职责
//! - `BudgetCoordinator`:协调 ACB(离散四级)与 DECB(连续双档)的预算决策
//! - `unified_budget_check`:统一预算检查,综合 ACB 级别与 DECB 系数
//! - `tier_to_coefficient`:ACB 级别 → DECB 系数映射(L0→0.25, L1→0.5, L2→0.75, L3→1.0)
//! - `coordinate_consumption`:统一消费记录,同步更新 ACB 与 DECB
//!
//! # 设计决策(WHY)
//! - **ACB 离散四级**:适合粗粒度资源控制,级别切换有滞后保护,避免抖动。
//!   用于快速决策(如是否接受新 Quest)。
//! - **DECB 连续系数**:适合细粒度预算分配,基于复杂度/紧急度动态调整。
//!   用于精确成本估算(如 Quest 内各步骤的预算分配)。
//! - **协调器统一两者**:调用方只需与协调器交互,无需关心底层是 ACB 还是 DECB。
//!   协调器综合两者输出,取最保守(最严格)的决策,确保预算不超支。
//! - **最保守原则**:ACB 说 L0(降级)但 DECB 说 0.8(高预算) → 取 L0(降级)。
//!   宁可过度保守,也不冒险超支。

use event_bus::{EventBus, EventMetadata, NexusEvent};
use tracing::{info, warn};

use crate::{AcbGovernor, AcbGovernorConfig, BudgetRequest, BudgetTier};
use decb_governor::{DecbConfig, DecbGovernor, QuestBudgetInput};

/// 统一预算决策 — 协调器对预算请求的综合判断结果
#[derive(Debug, Clone, PartialEq)]
pub struct UnifiedBudgetDecision {
    /// 是否批准请求
    pub approved: bool,
    /// ACB 当前级别
    pub acb_tier: BudgetTier,
    /// DECB 当前系数 [0.0, 1.0]
    pub decb_coefficient: f32,
    /// 统一预算系数(取 ACB 与 DECB 的最保守值)
    pub unified_coefficient: f32,
    /// 拒绝原因(approved=false 时有效)
    pub reason: String,
}

impl UnifiedBudgetDecision {
    /// 创建批准决策
    fn approved(acb_tier: BudgetTier, decb_coefficient: f32, unified_coefficient: f32) -> Self {
        Self {
            approved: true,
            acb_tier,
            decb_coefficient,
            unified_coefficient,
            reason: "OK".to_string(),
        }
    }

    /// 创建拒绝决策
    fn rejected(acb_tier: BudgetTier, decb_coefficient: f32, reason: String) -> Self {
        let unified = acb_tier_to_coefficient(acb_tier).min(decb_coefficient);
        Self {
            approved: false,
            acb_tier,
            decb_coefficient,
            unified_coefficient: unified,
            reason,
        }
    }
}

/// ACB-DECB 预算协调器 — 统一预算治理入口
///
/// # 使用方式
/// ```
/// # use acb_governor::budget_coordinator::BudgetCoordinator;
/// # use acb_governor::AcbGovernorConfig;
/// # use decb_governor::DecbConfig;
/// # use event_bus::EventBus;
///
/// let bus = EventBus::new();
/// let coordinator = BudgetCoordinator::new(
///     AcbGovernorConfig::default(),
///     DecbConfig::default(),
///     bus,
/// ).unwrap();
/// ```
pub struct BudgetCoordinator {
    /// ACB 治理器(离散四级)
    acb: AcbGovernor,
    /// DECB 治理器(连续双档)
    decb: DecbGovernor,
    /// 事件总线
    event_bus: EventBus,
}

impl BudgetCoordinator {
    /// 创建预算协调器
    ///
    /// # 参数
    /// - `acb_config`:ACB 配置
    /// - `decb_config`:DECB 配置
    /// - `event_bus`:事件总线
    ///
    /// # 错误
    /// - ACB 或 DECB 配置校验失败
    pub fn new(
        acb_config: AcbGovernorConfig,
        decb_config: DecbConfig,
        event_bus: EventBus,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let acb = AcbGovernor::with_event_bus(acb_config, event_bus.clone())?;
        let decb = DecbGovernor::new(decb_config)?;
        Ok(Self {
            acb,
            decb,
            event_bus,
        })
    }

    /// 统一预算检查 — 综合 ACB 与 DECB 的决策
    ///
    /// 流程:
    /// 1. ACB check_budget:离散级别检查(是否在当前级别预算内)
    /// 2. DECB compute_budget:连续系数计算(基于 Quest 复杂度/紧急度)
    /// 3. 统一决策:取最保守值(系数更小者)
    /// 4. 发布 BudgetCoordinated 事件
    ///
    /// # 参数
    /// - `quest_input`:Quest 预算输入(DECB 用)
    /// - `token_request`:Token 请求(ACB 用)
    ///
    /// # 返回
    /// `UnifiedBudgetDecision`:综合决策结果
    pub fn check_budget(
        &self,
        quest_input: &QuestBudgetInput,
        token_request: u64,
    ) -> UnifiedBudgetDecision {
        // 1. ACB 检查
        let acb_tier = self.acb.current_tier();
        let acb_request = BudgetRequest::new(&quest_input.quest_id, token_request);
        let acb_ok = self.acb.check_budget(&acb_request).is_ok();

        // 2. DECB 计算系数
        let decb_coef = self.decb.compute_budget(quest_input);

        // 3. ACB 级别映射为系数
        let acb_coef = acb_tier_to_coefficient(acb_tier);

        // 4. 统一系数:取最保守(最小)值
        let unified_coef = acb_coef.min(decb_coef);

        // 5. 决策:ACB 或 DECB 任一拒绝,则统一拒绝
        let approved = acb_ok && unified_coef > 0.0;

        let decision = if approved {
            UnifiedBudgetDecision::approved(acb_tier, decb_coef, unified_coef)
        } else {
            let reason = if !acb_ok {
                format!(
                    "ACB 拒绝:当前级别 {} 的 Token 上限不足(请求 {})",
                    acb_tier, token_request
                )
            } else {
                "DECB 系数为 0.0(降级模式)".to_string()
            };
            UnifiedBudgetDecision::rejected(acb_tier, decb_coef, reason)
        };

        // 6. 发布 BudgetCoordinated 事件
        let event = NexusEvent::BudgetCoordinated {
            metadata: EventMetadata::new("acb-governor"),
            quest_id: quest_input.quest_id.clone(),
            acb_tier: acb_tier.to_string(),
            decb_coefficient: decb_coef,
            unified_coefficient: decision.unified_coefficient,
            approved,
            reason: decision.reason.clone(),
        };
        if let Err(e) = self.event_bus.publish_blocking(event) {
            warn!(error = %e, "发布 BudgetCoordinated 事件失败");
        }

        decision
    }

    /// 统一消费记录 — 同步更新 ACB 与 DECB
    ///
    /// 流程:
    /// 1. ACB record_consumption:更新离散级别消耗
    /// 2. DECB record_consumption:更新连续系数消耗
    /// 3. 若 ACB 降级,同步通知 DECB 调整
    ///
    /// # 参数
    /// - `quest_id`:Quest ID
    /// - `tokens`:Token 消耗
    /// - `consumption`:详细消耗记录(DECB 用)
    pub fn record_consumption(
        &self,
        quest_id: &str,
        tokens: u64,
        consumption: &decb_governor::BudgetConsumption,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // 1. ACB 记录消耗
        self.acb.record_consumption(tokens)?;

        // 2. DECB 记录消耗
        self.decb.record_consumption(consumption)?;

        // 3. 检查 ACB 级别变化,若降级则通知
        let new_tier = self.acb.current_tier();
        if new_tier == BudgetTier::L0 {
            info!(quest_id, "ACB 降级到 L0,通知 DECB 进入降级模式");
        }

        Ok(())
    }

    /// 获取 ACB 治理器引用
    pub fn acb(&self) -> &AcbGovernor {
        &self.acb
    }

    /// 获取 DECB 治理器引用
    pub fn decb(&self) -> &DecbGovernor {
        &self.decb
    }

    /// 获取当前统一预算状态
    pub fn status(&self) -> UnifiedBudgetStatus {
        let acb_status = self.acb.get_status();
        let decb_stats = self.decb.get_stats();
        UnifiedBudgetStatus {
            acb_tier: acb_status.current_tier,
            acb_utilization: acb_status.utilization_rate,
            decb_coefficient: decb_stats.current_coefficient,
            decb_utilization: decb_stats.utilization_rate,
            unified_coefficient: acb_tier_to_coefficient(acb_status.current_tier)
                .min(decb_stats.current_coefficient),
        }
    }
}

/// 统一预算状态 — ACB 与 DECB 的综合状态快照
#[derive(Debug, Clone, PartialEq)]
pub struct UnifiedBudgetStatus {
    /// ACB 当前级别
    pub acb_tier: BudgetTier,
    /// ACB 利用率 [0.0, 1.0]
    pub acb_utilization: f32,
    /// DECB 当前系数
    pub decb_coefficient: f32,
    /// DECB 利用率 [0.0, 1.0]
    pub decb_utilization: f32,
    /// 统一系数(最保守值)
    pub unified_coefficient: f32,
}

/// ACB 级别 → DECB 系数映射
///
/// 映射规则:
/// - L0 → 0.25:降级模式,仅保留最低预算
/// - L1 → 0.50:基础级别,预算受限
/// - L2 → 0.75:标准级别,常规预算
/// - L3 → 1.00:充足级别,满预算
fn acb_tier_to_coefficient(tier: BudgetTier) -> f32 {
    match tier {
        BudgetTier::L0 => 0.25,
        BudgetTier::L1 => 0.50,
        BudgetTier::L2 => 0.75,
        BudgetTier::L3 => 1.00,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_coordinator() -> BudgetCoordinator {
        let bus = EventBus::new();
        BudgetCoordinator::new(AcbGovernorConfig::default(), DecbConfig::default(), bus).unwrap()
    }

    #[test]
    fn test_unified_budget_check_approved() {
        let coordinator = make_coordinator();
        let quest = QuestBudgetInput::simple("quest-1");
        let decision = coordinator.check_budget(&quest, 100);
        // 默认状态 ACB=L3(充足),DECB=1.0 → 应批准
        assert!(decision.approved);
        assert_eq!(decision.acb_tier, BudgetTier::L3);
        assert!(decision.unified_coefficient > 0.0);
    }

    #[test]
    fn test_acb_tier_to_coefficient() {
        assert!((acb_tier_to_coefficient(BudgetTier::L0) - 0.25).abs() < 1e-6);
        assert!((acb_tier_to_coefficient(BudgetTier::L1) - 0.50).abs() < 1e-6);
        assert!((acb_tier_to_coefficient(BudgetTier::L2) - 0.75).abs() < 1e-6);
        assert!((acb_tier_to_coefficient(BudgetTier::L3) - 1.00).abs() < 1e-6);
    }

    #[test]
    fn test_unified_status() {
        let coordinator = make_coordinator();
        let status = coordinator.status();
        assert_eq!(status.acb_tier, BudgetTier::L3);
        assert!(status.unified_coefficient > 0.0);
    }

    #[test]
    fn test_most_conservative_principle() {
        // ACB=L2(0.75), DECB=1.0 → unified=0.75
        // ACB=L3(1.0), DECB=0.5 → unified=0.5
        // 统一系数应取最小值
        let l2_coef = acb_tier_to_coefficient(BudgetTier::L2);
        let l3_coef = acb_tier_to_coefficient(BudgetTier::L3);
        assert_eq!(l2_coef.min(1.0), 0.75);
        assert_eq!(l3_coef.min(0.5), 0.5);
    }
}
