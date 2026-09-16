//! 审议预算门(ADR-160 孤岛偿还 M10:parliament → decb-governor 生产边)
//!
//! 对应架构层:L8 Parliament(同层互引,§2.2 依赖铁律)
//!
//! # 模块职责(WHY 本模块存在)
//! parliament 的审议(辩论/投票/共识)是 L8 层最大的认知预算消费者;
//! DECB(L8 同层)提供双档预算治理(HighTier / LowTier / Degraded)。
//! 本门把 DECB 裁决接入审议前置:Quest 进入审议前计算预算系数,
//! 系数与档位供上层编排器决定辩论轮次/超时预算;审议结束后回写实际
//! 消耗,驱动 DECB 溢出检测与档位切换。
//!
//! # 装配姿势
//! 构造参数注入(ADR-161 决策 3:禁 feature 门控)。**未装配时审议路径
//! 行为与此前完全一致**(零行为变化):本模块不参与 `Parliament::deliberate`
//! 既有调用链,由上层编排器显式持有并调用。
//!
//! # 依赖方向
//! parliament → decb-governor:L8 → L8 同层互引,合法(§2.2)。
//! 对外接口影响:零(纯 additive 模块)。

use decb_governor::{
    BudgetCoefficient, BudgetConsumption, BudgetTier, DecbConfig, DecbError, DecbGovernor,
    QuestBudgetInput,
};

/// 审议预算门 — DECB 治理器的 parliament 侧装配封装
///
/// 持有 `DecbGovernor` 实例,把预算裁决暴露为审议生命周期三个挂点:
/// 审议前评估(`assess`)、档位查询(`current_tier`)、审议后回写
/// (`record_debate_consumption`)。
///
/// WHY 不 derive Debug:`DecbGovernor` 未实现 Debug(内部含锁/原子状态),
/// 本结构是薄装配层,无调试输出需求。
pub struct DebateBudgetGate {
    governor: DecbGovernor,
}

impl DebateBudgetGate {
    /// 以默认配置创建预算门
    pub fn new(config: DecbConfig) -> Result<Self, DecbError> {
        Ok(Self {
            governor: DecbGovernor::new(config)?,
        })
    }

    /// 以既有治理器实例创建预算门(生产推荐:共享 EventBus 的治理器
    /// 经 `DecbGovernor::with_event_bus` 构造后由此接入,档位切换/溢出
    /// 事件走同一总线)
    pub fn with_governor(governor: DecbGovernor) -> Self {
        Self { governor }
    }

    /// 审议前裁决:Quest 的预算系数([0,1],越高预算越充裕)
    ///
    /// 真实调用 DECB `compute_budget`(复杂度/紧急度/剩余预算连续可调系数)。
    pub fn assess(&self, quest: &QuestBudgetInput) -> BudgetCoefficient {
        BudgetCoefficient::new(self.governor.compute_budget(quest))
    }

    /// 当前预算档(HighTier / LowTier / Degraded)
    ///
    /// 与 `assess` 返回系数的关系:`current_tier ==
    /// determine_tier(assess(quest).value())` 对同一治理器状态成立。
    pub fn current_tier(&self) -> BudgetTier {
        self.governor.current_tier()
    }

    /// 审议后回写:记录本轮审议实际消耗(令牌/工具调用/上下文加载)
    ///
    /// 真实调用 DECB `record_consumption`,驱动溢出检测与自动降级链路
    /// (`BudgetExceeded` [Critical] 事件由治理器侧发布)。
    pub fn record_debate_consumption(
        &self,
        consumption: &BudgetConsumption,
    ) -> Result<(), DecbError> {
        self.governor.record_consumption(consumption)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_gate_assesses_coefficient_in_range() {
        let gate = DebateBudgetGate::new(DecbConfig::default()).unwrap();
        let coefficient = gate.assess(&QuestBudgetInput::simple("quest-1"));
        let value = coefficient.value();
        assert!(
            (0.0..=1.0).contains(&value),
            "预算系数必须落在 [0,1],得到 {value}"
        );
    }

    #[test]
    fn assessed_coefficient_maps_to_determined_tier() {
        // WHY 用 determine_tier 而非 current_tier:后者是档位状态机
        // (带滞后切换),不由单次 assess 派生;前者才是系数的档位判定点
        let gate = DebateBudgetGate::new(DecbConfig::default()).unwrap();
        let quest = QuestBudgetInput::simple("quest-1");
        let coefficient = gate.assess(&quest);
        let tier = gate.governor.determine_tier(coefficient.value());
        assert!(
            matches!(
                tier,
                BudgetTier::HighTier | BudgetTier::LowTier | BudgetTier::Degraded
            ),
            "determine_tier 必须落在三档之一,得到 {tier:?}"
        );
    }

    #[test]
    fn record_debate_consumption_accumulates() {
        let gate = DebateBudgetGate::new(DecbConfig::default()).unwrap();
        gate.record_debate_consumption(&BudgetConsumption::new(100, 2, 1))
            .unwrap();
        let stats = gate.governor.get_stats();
        assert!(
            stats.total_consumption > 0.0,
            "回写审议消耗后累计消耗必须为正,得到 {}",
            stats.total_consumption
        );
    }

    #[test]
    fn with_governor_shares_event_bus_ready_instance() {
        // WHY:with_governor 是生产推荐路径(共享 EventBus 治理器接入),
        // 此处验证包装不改变治理器行为(透传语义)
        let governor = DecbGovernor::new(DecbConfig::default()).unwrap();
        let gate = DebateBudgetGate::with_governor(governor);
        let coefficient = gate.assess(&QuestBudgetInput::simple("quest-2"));
        assert!((0.0..=1.0).contains(&coefficient.value()));
    }
}
