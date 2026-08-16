//! Skill 生命周期管理器 — MSCE 状态机（设计文档 §10.5）
//!
//! 对应架构层: **L5 Knowledge**（repo-wiki 子模块）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §10.5
//! 对应论文: MSCE（Skill 生命周期: Probationary → Active → Archived）
//! 对应 ADR: ADR-049 决策 1（skill-lifecycle 落点 repo-wiki，内嵌模块）
//!
//! # 核心职责
//!
//! 管理多技能的生命周期状态机，消费 L0 [`SkillLifecycleContract`] 契约：
//! - **Probationary → Active**: 成功计数 ≥ activation_threshold
//! - **Active → Archived**: 失败计数 ≥ archive_threshold
//! - **Probationary → Archived**: 失败计数 ≥ archive_threshold
//! - **Archived**: 终态（无复活路径，重新生成即新 skill_id）
//! - **Active 成功重置 failure_count**（规范 §10.5，连续失败后一次成功清零）
//!
//! # 设计约束（铁律）
//!
//! - **消费 L0 契约**: 复用 `SkillLifecycleContract.record_success/record_failure`
//!   纯函数（Phase 0 落地），本管理器仅维护多技能注册表与状态转移编排
//! - **职责边界**: 与 `skill_graph`（复用率图，Ω₆-Reuse）互补——本模块管生命周期
//!   状态机，skill_graph 管复用率推荐
//! - **铁律4**: 状态转移判定为纯函数（L0 next_state）

use std::collections::HashMap;

use nexus_contracts::skill_lifecycle::{SkillLifecycleContract, SkillLifecycleState};

/// Skill 生命周期管理器 — 多技能状态机注册表
pub struct SkillLifecycleManager {
    /// 技能契约注册表（skill_id → 契约）
    skills: HashMap<String, SkillLifecycleContract>,
    /// 试用期时长（毫秒，可观测性用）
    probation_period_ms: u64,
    /// 激活所需成功次数（新技能默认阈值）
    activation_threshold: u32,
    /// 归档所需失败次数（新技能默认阈值）
    archive_threshold: u32,
}

impl SkillLifecycleManager {
    /// 创建生命周期管理器
    ///
    /// - `probation_period_ms`: 试用期时长（毫秒）
    /// - `activation_threshold`: 激活所需成功次数
    /// - `archive_threshold`: 归档所需失败次数
    pub fn new(
        probation_period_ms: u64,
        activation_threshold: u32,
        archive_threshold: u32,
    ) -> Self {
        Self {
            skills: HashMap::new(),
            probation_period_ms,
            activation_threshold,
            archive_threshold,
        }
    }

    /// 注册新技能（进入试用期）
    pub fn register_skill(&mut self, skill_id: &str, now_ms: u64) {
        let mut contract = SkillLifecycleContract::new_probationary(skill_id, now_ms);
        contract.activation_threshold = self.activation_threshold;
        contract.archive_threshold = self.archive_threshold;
        self.skills.insert(skill_id.to_string(), contract);
    }

    /// 记录一次执行结果（状态转移编排）
    ///
    /// - `Probationary`: 委托 L0 record_success/record_failure（激活/归档）
    /// - `Active`: 成功 → 重置 failure_count；失败 → record_failure（可能归档）
    /// - `Archived`: 终态，仅更新 last_used
    pub fn record_outcome(&mut self, skill_id: &str, success: bool, now_ms: u64) {
        let Some(contract) = self.skills.get(skill_id) else {
            return;
        };
        let next = match contract.state {
            SkillLifecycleState::Probationary => {
                if success {
                    contract.record_success(now_ms)
                } else {
                    contract.record_failure(now_ms)
                }
            }
            SkillLifecycleState::Active => {
                if success {
                    // 规范 §10.5: Active 成功重置 failure_count
                    let mut next = contract.record_success(now_ms);
                    next.failure_count = 0;
                    next
                } else {
                    contract.record_failure(now_ms)
                }
            }
            SkillLifecycleState::Archived => {
                // 终态: 仅更新 last_used
                let mut next = contract.clone();
                next.last_used = now_ms;
                next
            }
        };
        self.skills.insert(skill_id.to_string(), next);
    }

    /// 获取技能契约只读引用
    pub fn get_contract(&self, skill_id: &str) -> Option<&SkillLifecycleContract> {
        self.skills.get(skill_id)
    }

    /// 获取技能当前状态
    pub fn get_state(&self, skill_id: &str) -> Option<SkillLifecycleState> {
        self.skills.get(skill_id).map(|c| c.state)
    }

    /// 获取全部 Active 状态技能 ID（可检索使用的技能）
    pub fn get_active_skill_ids(&self) -> Vec<String> {
        self.skills
            .iter()
            .filter(|(_, c)| c.state == SkillLifecycleState::Active)
            .map(|(id, _)| id.clone())
            .collect()
    }

    /// 技能总数（可观测性）
    pub fn skill_count(&self) -> usize {
        self.skills.len()
    }

    /// 试用期时长只读访问（可观测性）
    pub fn probation_period_ms(&self) -> u64 {
        self.probation_period_ms
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn manager() -> SkillLifecycleManager {
        SkillLifecycleManager::new(3_600_000, 3, 5)
    }

    #[test]
    fn register_skill_enters_probationary() {
        let mut mgr = manager();
        mgr.register_skill("skill-1", 100);
        assert_eq!(mgr.skill_count(), 1);
        assert_eq!(
            mgr.get_state("skill-1"),
            Some(SkillLifecycleState::Probationary)
        );
    }

    #[test]
    fn probationary_activates_after_threshold() {
        let mut mgr = manager();
        mgr.register_skill("skill-1", 100);
        // 成功 3 次（默认阈值）→ 激活
        for i in 1..=3 {
            mgr.record_outcome("skill-1", true, 100 + i);
        }
        assert_eq!(mgr.get_state("skill-1"), Some(SkillLifecycleState::Active));
        assert!(mgr.get_active_skill_ids().contains(&"skill-1".to_string()));
    }

    #[test]
    fn probationary_archives_after_failures() {
        let mut mgr = manager();
        mgr.register_skill("skill-1", 100);
        // 失败 5 次（默认阈值）→ 归档
        for i in 1..=5 {
            mgr.record_outcome("skill-1", false, 100 + i);
        }
        assert_eq!(
            mgr.get_state("skill-1"),
            Some(SkillLifecycleState::Archived)
        );
        assert!(!mgr.get_active_skill_ids().contains(&"skill-1".to_string()));
    }

    #[test]
    fn active_success_resets_failure_count() {
        let mut mgr = manager();
        mgr.register_skill("skill-1", 100);
        // 先激活
        for i in 1..=3 {
            mgr.record_outcome("skill-1", true, 100 + i);
        }
        assert_eq!(mgr.get_state("skill-1"), Some(SkillLifecycleState::Active));
        // 失败 4 次（未达阈值 5）
        for i in 1..=4 {
            mgr.record_outcome("skill-1", false, 200 + i);
        }
        assert_eq!(mgr.get_state("skill-1"), Some(SkillLifecycleState::Active));
        assert_eq!(mgr.get_contract("skill-1").unwrap().failure_count, 4);
        // 一次成功 → 重置 failure_count
        mgr.record_outcome("skill-1", true, 300);
        assert_eq!(mgr.get_contract("skill-1").unwrap().failure_count, 0);
        assert_eq!(mgr.get_state("skill-1"), Some(SkillLifecycleState::Active));
    }

    #[test]
    fn active_archives_after_consecutive_failures() {
        let mut mgr = manager();
        mgr.register_skill("skill-1", 100);
        for i in 1..=3 {
            mgr.record_outcome("skill-1", true, 100 + i);
        }
        assert_eq!(mgr.get_state("skill-1"), Some(SkillLifecycleState::Active));
        // 连续失败 5 次 → 归档
        for i in 1..=5 {
            mgr.record_outcome("skill-1", false, 200 + i);
        }
        assert_eq!(
            mgr.get_state("skill-1"),
            Some(SkillLifecycleState::Archived)
        );
    }

    #[test]
    fn archived_is_terminal() {
        let mut mgr = manager();
        mgr.register_skill("skill-1", 100);
        // 直接归档（失败 5 次）
        for i in 1..=5 {
            mgr.record_outcome("skill-1", false, 100 + i);
        }
        assert_eq!(
            mgr.get_state("skill-1"),
            Some(SkillLifecycleState::Archived)
        );
        // 归档后成功不复活（终态）
        for i in 1..=10 {
            mgr.record_outcome("skill-1", true, 200 + i);
        }
        assert_eq!(
            mgr.get_state("skill-1"),
            Some(SkillLifecycleState::Archived)
        );
    }

    #[test]
    fn record_outcome_unknown_skill_noop() {
        let mut mgr = manager();
        // 未注册技能 → 无操作（不 panic）
        mgr.record_outcome("unknown", true, 100);
        assert_eq!(mgr.skill_count(), 0);
    }

    #[test]
    fn get_active_skill_ids_multiple() {
        let mut mgr = manager();
        mgr.register_skill("s1", 100);
        mgr.register_skill("s2", 100);
        mgr.register_skill("s3", 100);
        // 只激活 s1 和 s2
        for _ in 0..3 {
            mgr.record_outcome("s1", true, 200);
            mgr.record_outcome("s2", true, 200);
        }
        let active = mgr.get_active_skill_ids();
        assert_eq!(active.len(), 2);
        assert!(active.contains(&"s1".to_string()));
        assert!(active.contains(&"s2".to_string()));
        assert!(!active.contains(&"s3".to_string()));
    }

    #[test]
    fn custom_thresholds_respected() {
        // 自定义阈值: 激活需 2 次成功
        let mut mgr = SkillLifecycleManager::new(3_600_000, 2, 3);
        mgr.register_skill("skill-1", 100);
        for i in 1..=2 {
            mgr.record_outcome("skill-1", true, 100 + i);
        }
        assert_eq!(mgr.get_state("skill-1"), Some(SkillLifecycleState::Active));
    }

    #[test]
    fn probation_period_observable() {
        let mgr = SkillLifecycleManager::new(7_200_000, 3, 5);
        assert_eq!(mgr.probation_period_ms(), 7_200_000);
    }
}
