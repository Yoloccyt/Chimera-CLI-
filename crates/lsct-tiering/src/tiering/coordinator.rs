//! LSCT 协调器 — 聚合任务负载画像、升降温器与 EventBus 集成
//!
//! 对应架构层:L3 Storage
//!
//! # 核心职责
//! - 维护 TierAssignment 映射(capability_id → 当前/目标层级)
//! - tick():扫描所有能力,基于任务负载画像生成升降温决策(逐级)
//! - apply_decision():执行单个决策,更新映射并发布 LsctTierSwitched 事件
//! - handle_quest_created():从 Quest 标题生成画像,触发 tick + 批量应用
//!
//! # 架构定位(WHY)
//! LSCT 是 CMT 之上的"任务感知策略层",不直接操作 CMT 存储:
//! - 计算策略 → 发布 LsctTierSwitched 事件 → CMT 订阅执行实际数据迁移
//! - 符合 §2.2 依赖铁律:同层 L3 互引 + 跨层走 EventBus
//!
//! # 并发安全
//! - DashMap 提供无锁并发读写 assignments
//! - `Mutex<LsctPromoter>` / `Mutex<LsctDemoter>` 保护升降温器的 HashSet
//! - 锁在 await 前释放,避免跨 await 持锁导致死锁(架构红线)

use std::sync::Mutex;

use cmt_tiering::Tier;
use dashmap::DashMap;
use event_bus::{EventBus, EventMetadata, NexusEvent};
use tracing::{debug, warn};

use crate::config::LsctConfig;
use crate::error::LsctError;
use crate::tiering::demoter::LsctDemoter;
use crate::tiering::profile::compute_target_tier;
use crate::tiering::promoter::LsctPromoter;
use crate::types::{
    next_colder, next_warmer, tier_rank, TaskLoadProfile, TierAssignment, TierSwitchDecision,
};

/// LSCT 协调器 — 任务感知能力分层的核心组件
///
/// 聚合任务负载画像、升降温器与 EventBus,提供完整的层级调度能力。
///
/// # 使用示例
/// ```no_run
/// use lsct_tiering::{LsctCoordinator, LsctConfig, TaskType, TaskLoadProfile};
/// use cmt_tiering::Tier;
///
/// # async fn example() {
/// let coordinator = LsctCoordinator::new(LsctConfig::default());
/// coordinator.register_capability("cap-1", Tier::Warm);
///
/// let profile = TaskLoadProfile::new(TaskType::Compile, 0.9, 1);
/// let decisions = coordinator.tick(&profile);
/// for decision in &decisions {
///     coordinator.apply_decision(decision).await.ok();
/// }
/// # }
/// ```
pub struct LsctCoordinator {
    /// 引擎配置(升降温阈值与扫描周期)
    config: LsctConfig,
    /// 能力层级分配映射:capability_id → TierAssignment
    assignments: DashMap<String, TierAssignment>,
    /// 升温器(Mutex 保护,提供内部可变性)
    promoter: Mutex<LsctPromoter>,
    /// 降温器(Mutex 保护,提供内部可变性)
    demoter: Mutex<LsctDemoter>,
    /// 事件总线(可选,测试场景可缺省;生产场景注入共享 EventBus)
    event_bus: Option<EventBus>,
}

impl LsctCoordinator {
    /// 创建协调器(无 EventBus,用于测试或无事件通知场景)
    pub fn new(config: LsctConfig) -> Self {
        Self {
            config,
            assignments: DashMap::new(),
            promoter: Mutex::new(LsctPromoter::new()),
            demoter: Mutex::new(LsctDemoter::new()),
            event_bus: None,
        }
    }

    /// 创建协调器并注入共享 EventBus(用于生产场景)
    ///
    /// WHY 注入而非内部创建:EventBus 是跨层共享资源,由上层统一创建
    /// 并注入,确保所有组件使用同一总线(§2.2 依赖铁律)。EventBus 基于
    /// Arc,Clone 廉价,按值持有安全。
    pub fn with_event_bus(config: LsctConfig, bus: EventBus) -> Self {
        Self {
            config,
            assignments: DashMap::new(),
            promoter: Mutex::new(LsctPromoter::new()),
            demoter: Mutex::new(LsctDemoter::new()),
            event_bus: Some(bus),
        }
    }

    /// 获取配置引用
    pub fn config(&self) -> &LsctConfig {
        &self.config
    }

    /// 注册能力,指定初始层级
    ///
    /// 若 capability_id 已存在,覆盖其层级分配。
    pub fn register_capability(&self, capability_id: &str, initial_tier: Tier) {
        self.assignments.insert(
            capability_id.to_string(),
            TierAssignment {
                capability_id: capability_id.to_string(),
                current_tier: initial_tier,
                target_tier: initial_tier,
                reason: "initial registration".into(),
            },
        );
        debug!(cap_id = capability_id, tier = ?initial_tier, "能力已注册");
    }

    /// 获取能力当前层级
    pub fn get_tier(&self, capability_id: &str) -> Option<Tier> {
        self.assignments
            .get(capability_id)
            .map(|entry| entry.current_tier)
    }

    /// 获取已注册能力数量
    pub fn len(&self) -> usize {
        self.assignments.len()
    }

    /// 是否没有注册任何能力
    pub fn is_empty(&self) -> bool {
        self.assignments.is_empty()
    }

    /// 周期性扫描:基于任务负载画像生成升降温决策
    ///
    /// # 决策逻辑(逐级迁移)
    /// 1. 重置 promoter/demoter 的级联防护集合(新 tick 周期)
    /// 2. 遍历所有能力,计算目标层级(compute_target_tier)
    /// 3. 比较 current 与 target:
    ///    - target 更热(rank 更小)→ Promote 到 next_warmer(current)
    ///    - target 更冷(rank 更大)→ Demote 到 next_colder(current)
    ///    - 相等 → Keep
    /// 4. 更新 assignment 的 target_tier(反映最新目标)
    ///
    /// WHY 逐级:每个 tick 只迁移一级,多 tick 周期逐步达到目标,
    /// 避免跨级跳跃造成存储层负载突变。
    ///
    /// # 返回
    /// 决策列表(每个能力一个决策),调用方决定是否 apply。
    pub fn tick(&self, profile: &TaskLoadProfile) -> Vec<TierSwitchDecision> {
        // 重置级联防护集合(新 tick 周期)
        self.promoter
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .reset();
        self.demoter
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .reset();

        // 快照当前状态(避免持有 DashMap 迭代器同时修改)
        let snapshot: Vec<(String, Tier)> = self
            .assignments
            .iter()
            .map(|entry| (entry.key().clone(), entry.current_tier))
            .collect();

        let target = compute_target_tier(profile);
        let target_rank = tier_rank(target);
        let mut decisions = Vec::with_capacity(snapshot.len());

        for (cap_id, current) in snapshot {
            let current_rank = tier_rank(current);
            let decision = if target_rank < current_rank {
                // 目标更热 → 升温(逐级)
                match next_warmer(current) {
                    Some(to) => TierSwitchDecision::Promote {
                        capability_id: cap_id.clone(),
                        from: current,
                        to,
                        reason: format!(
                            "{:?} task intensity {:.2} → promote to {:?}",
                            profile.task_type, profile.intensity, to
                        ),
                    },
                    None => TierSwitchDecision::Keep {
                        capability_id: cap_id.clone(),
                        tier: current,
                        reason: "already at hottest, cannot promote".into(),
                    },
                }
            } else if target_rank > current_rank {
                // 目标更冷 → 降温(逐级)
                match next_colder(current) {
                    Some(to) => TierSwitchDecision::Demote {
                        capability_id: cap_id.clone(),
                        from: current,
                        to,
                        reason: format!(
                            "{:?} task intensity {:.2} → demote to {:?}",
                            profile.task_type, profile.intensity, to
                        ),
                    },
                    None => TierSwitchDecision::Keep {
                        capability_id: cap_id.clone(),
                        tier: current,
                        reason: "already at coldest, cannot demote".into(),
                    },
                }
            } else {
                // 已在目标层级
                TierSwitchDecision::Keep {
                    capability_id: cap_id.clone(),
                    tier: current,
                    reason: "at target tier".into(),
                }
            };

            // 更新 assignment 的 target_tier
            if let Some(mut entry) = self.assignments.get_mut(&cap_id) {
                entry.target_tier = target;
            }

            decisions.push(decision);
        }

        debug!(
            task_type = ?profile.task_type,
            intensity = profile.intensity,
            decision_count = decisions.len(),
            "tick 完成,生成决策"
        );
        decisions
    }

    /// 应用单个决策,执行升降温并发布事件
    ///
    /// # 执行流程
    /// 1. Promote:锁 promoter → promote() 校验并执行 → 释放锁 → 更新 DashMap → 发布事件
    /// 2. Demote:锁 demoter → demote() 校验并执行 → 释放锁 → 更新 DashMap → 发布事件
    /// 3. Keep:无操作
    ///
    /// WHY 锁在 await 前释放:Mutex guard 不是 Send,不能跨 await 持有。
    /// 通过内联块确保锁在进入 async 上下文前 drop。
    ///
    /// # 事件发布失败处理
    /// 按 project_memory 教训:事件发布失败记录 warn 日志,不阻塞主流程。
    pub async fn apply_decision(&self, decision: &TierSwitchDecision) -> Result<(), LsctError> {
        match decision {
            TierSwitchDecision::Promote {
                capability_id,
                from,
                to,
                reason,
            } => {
                // 锁内执行 promote,锁在块结束时释放(await 前)
                {
                    let mut promoter = self.promoter.lock().unwrap_or_else(|p| p.into_inner());
                    promoter.promote(capability_id, *from, *to)?;
                }

                // 更新 DashMap 中的 current_tier
                if let Some(mut entry) = self.assignments.get_mut(capability_id) {
                    entry.current_tier = *to;
                    entry.reason = reason.clone();
                }

                self.publish_tier_switched(capability_id, *from, *to, reason)
                    .await;
            }
            TierSwitchDecision::Demote {
                capability_id,
                from,
                to,
                reason,
            } => {
                // 锁内执行 demote,锁在块结束时释放(await 前)
                {
                    let mut demoter = self.demoter.lock().unwrap_or_else(|p| p.into_inner());
                    demoter.demote(capability_id, *from, *to)?;
                }

                // 更新 DashMap 中的 current_tier
                if let Some(mut entry) = self.assignments.get_mut(capability_id) {
                    entry.current_tier = *to;
                    entry.reason = reason.clone();
                }

                self.publish_tier_switched(capability_id, *from, *to, reason)
                    .await;
            }
            TierSwitchDecision::Keep { .. } => {
                // 无需操作
            }
        }
        Ok(())
    }

    /// 处理 QuestCreated 事件:从标题生成画像,触发 tick 并批量应用
    ///
    /// # 流程
    /// 1. from_quest_title(title) → TaskLoadProfile
    /// 2. tick(profile) → `Vec<TierSwitchDecision>`
    /// 3. 逐个 apply_decision(非 Keep 决策)
    ///
    /// WHY 批量应用不中断:单个能力 apply 失败时记录 warn 并继续,
    /// 避免一个能力的失败阻塞整个 Quest 的层级调整。
    pub async fn handle_quest_created(
        &self,
        title: &str,
    ) -> Result<Vec<TierSwitchDecision>, LsctError> {
        let profile = TaskLoadProfile::from_quest_title(title);
        let decisions = self.tick(&profile);

        for decision in &decisions {
            if !matches!(decision, TierSwitchDecision::Keep { .. }) {
                if let Err(e) = self.apply_decision(decision).await {
                    warn!(
                        capability_id = %decision.capability_id(),
                        error = %e,
                        "apply_decision 失败,跳过此能力,继续处理其他能力"
                    );
                }
            }
        }

        debug!(
            quest_title = title,
            total_decisions = decisions.len(),
            "handle_quest_created 完成"
        );
        Ok(decisions)
    }

    /// 发布 LsctTierSwitched 事件(内部辅助方法)
    ///
    /// WHY 独立方法:Promote 和 Demote 的事件发布逻辑相同,提取避免重复。
    /// 事件发布失败仅记录 warn,不返回错误(不阻塞主流程)。
    async fn publish_tier_switched(&self, capability_id: &str, from: Tier, to: Tier, reason: &str) {
        if let Some(bus) = &self.event_bus {
            let event = NexusEvent::LsctTierSwitched {
                metadata: EventMetadata::new("lsct-tiering"),
                capability_id: capability_id.to_string(),
                from_tier: from.as_str().to_string(),
                to_tier: to.as_str().to_string(),
                reason: reason.to_string(),
            };
            if let Err(e) = bus.publish(event).await {
                warn!(
                    error = %e,
                    capability_id = capability_id,
                    "LsctTierSwitched 事件发布失败,继续执行(不阻塞主流程)"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::TaskType;

    #[test]
    fn test_register_and_get_tier() {
        let coordinator = LsctCoordinator::new(LsctConfig::default());
        assert!(coordinator.is_empty());

        coordinator.register_capability("cap-1", Tier::Warm);
        assert_eq!(coordinator.len(), 1);
        assert_eq!(coordinator.get_tier("cap-1"), Some(Tier::Warm));

        // 未注册的能力返回 None
        assert_eq!(coordinator.get_tier("cap-unknown"), None);
    }

    #[test]
    fn test_tick_promote_compile_high() {
        // Compile 高强度 → target Hot,当前 Warm → Promote Warm→Hot
        let coordinator = LsctCoordinator::new(LsctConfig::default());
        coordinator.register_capability("cap-1", Tier::Warm);

        let profile = TaskLoadProfile::new(TaskType::Compile, 0.9, 1);
        let decisions = coordinator.tick(&profile);

        assert_eq!(decisions.len(), 1);
        match &decisions[0] {
            TierSwitchDecision::Promote { from, to, .. } => {
                assert_eq!(*from, Tier::Warm);
                assert_eq!(*to, Tier::Hot);
            }
            other => panic!("期望 Promote,得到 {:?}", other),
        }
    }

    #[test]
    fn test_tick_demote_debug_low() {
        // Debug 低强度 → target Ice,当前 Warm → Demote Warm→Cold(逐级)
        let coordinator = LsctCoordinator::new(LsctConfig::default());
        coordinator.register_capability("cap-1", Tier::Warm);

        let profile = TaskLoadProfile::new(TaskType::Debug, 0.1, 1);
        let decisions = coordinator.tick(&profile);

        assert_eq!(decisions.len(), 1);
        match &decisions[0] {
            TierSwitchDecision::Demote { from, to, .. } => {
                assert_eq!(*from, Tier::Warm);
                assert_eq!(*to, Tier::Cold); // 逐级:Warm→Cold(不是直接到 Ice)
            }
            other => panic!("期望 Demote,得到 {:?}", other),
        }
    }

    #[test]
    fn test_tick_keep_at_target() {
        // Run 始终 → Hot,当前已在 Hot → Keep
        let coordinator = LsctCoordinator::new(LsctConfig::default());
        coordinator.register_capability("cap-1", Tier::Hot);

        let profile = TaskLoadProfile::new(TaskType::Run, 0.9, 1);
        let decisions = coordinator.tick(&profile);

        assert_eq!(decisions.len(), 1);
        assert!(matches!(decisions[0], TierSwitchDecision::Keep { .. }));
    }

    #[tokio::test]
    async fn test_apply_decision_promote_updates_assignment() {
        let coordinator = LsctCoordinator::new(LsctConfig::default());
        coordinator.register_capability("cap-1", Tier::Warm);

        let decision = TierSwitchDecision::Promote {
            capability_id: "cap-1".into(),
            from: Tier::Warm,
            to: Tier::Hot,
            reason: "test promote".into(),
        };

        coordinator.apply_decision(&decision).await.unwrap();

        // 验证 current_tier 已更新
        assert_eq!(coordinator.get_tier("cap-1"), Some(Tier::Hot));
    }

    #[tokio::test]
    async fn test_apply_decision_demote_updates_assignment() {
        let coordinator = LsctCoordinator::new(LsctConfig::default());
        coordinator.register_capability("cap-1", Tier::Hot);

        let decision = TierSwitchDecision::Demote {
            capability_id: "cap-1".into(),
            from: Tier::Hot,
            to: Tier::Warm,
            reason: "test demote".into(),
        };

        coordinator.apply_decision(&decision).await.unwrap();
        assert_eq!(coordinator.get_tier("cap-1"), Some(Tier::Warm));
    }

    #[tokio::test]
    async fn test_handle_quest_created_full_flow() {
        // 完整流程:Quest 标题 → 画像 → tick → apply
        let coordinator = LsctCoordinator::new(LsctConfig::default());
        coordinator.register_capability("cap-1", Tier::Cold);

        // "compile release" → Compile(0.9) → target Hot
        // cap-1 在 Cold → 逐级 Promote Cold→Warm
        let decisions = coordinator
            .handle_quest_created("compile production release")
            .await
            .unwrap();

        assert!(!decisions.is_empty());
        // 验证 cap-1 被升温到 Warm(逐级,不是直接到 Hot)
        assert_eq!(coordinator.get_tier("cap-1"), Some(Tier::Warm));
    }

    #[tokio::test]
    async fn test_handle_quest_created_debug_demotes() {
        let coordinator = LsctCoordinator::new(LsctConfig::default());
        coordinator.register_capability("cap-1", Tier::Hot);

        // "debug memory leak" → Debug(0.2) → target Ice
        // cap-1 在 Hot → 逐级 Demote Hot→Warm
        let decisions = coordinator
            .handle_quest_created("debug memory leak")
            .await
            .unwrap();

        assert!(!decisions.is_empty());
        assert_eq!(coordinator.get_tier("cap-1"), Some(Tier::Warm));
    }

    #[test]
    fn test_tick_multi_tick_progressive_promotion() {
        // 多 tick 逐步升温:Ice → Cold → Warm → Hot
        let coordinator = LsctCoordinator::new(LsctConfig::default());
        coordinator.register_capability("cap-1", Tier::Ice);

        let profile = TaskLoadProfile::new(TaskType::Run, 0.9, 1);

        // Tick 1: Ice → Cold
        let d1 = coordinator.tick(&profile);
        assert!(matches!(
            &d1[0],
            TierSwitchDecision::Promote { to: Tier::Cold, .. }
        ));

        // Tick 2: Cold → Warm(需要先 apply tick 1 的决策)
        // 这里只验证 tick 生成正确决策,apply 在 handle_quest_created 中测试
    }

    #[test]
    fn test_tick_multiple_capabilities() {
        let coordinator = LsctCoordinator::new(LsctConfig::default());
        coordinator.register_capability("cap-hot", Tier::Hot);
        coordinator.register_capability("cap-warm", Tier::Warm);
        coordinator.register_capability("cap-cold", Tier::Cold);

        // Compile 高强度 → target Hot
        let profile = TaskLoadProfile::new(TaskType::Compile, 0.9, 1);
        let decisions = coordinator.tick(&profile);

        assert_eq!(decisions.len(), 3);

        // cap-hot 已在 Hot → Keep
        // cap-warm 在 Warm → Promote Warm→Hot
        // cap-cold 在 Cold → Promote Cold→Warm
        let has_keep = decisions.iter().any(|d| {
            matches!(d, TierSwitchDecision::Keep { capability_id, .. } if capability_id == "cap-hot")
        });
        let has_promote_warm = decisions.iter().any(|d| {
            matches!(d, TierSwitchDecision::Promote { capability_id, to: Tier::Hot, .. } if capability_id == "cap-warm")
        });
        let has_promote_cold = decisions.iter().any(|d| {
            matches!(d, TierSwitchDecision::Promote { capability_id, to: Tier::Warm, .. } if capability_id == "cap-cold")
        });
        assert!(has_keep);
        assert!(has_promote_warm);
        assert!(has_promote_cold);
    }
}
