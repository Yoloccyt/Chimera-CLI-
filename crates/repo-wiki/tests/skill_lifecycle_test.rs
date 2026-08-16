//! Skill 生命周期状态机集成测试 — MSCE 三态转移 + L0 契约消费（v3.4.0 §10.5）
//!
//! 覆盖: 顶层 API 可达性（re-export 验证）/ 三态转移 / Active 成功重置 failure_count /
//! Archived 终态 / proptest 状态机不变量

#![forbid(unsafe_code)]

use nexus_contracts::skill_lifecycle::SkillLifecycleState;
use proptest::prelude::*;
use repo_wiki::SkillLifecycleManager;

fn manager() -> SkillLifecycleManager {
    // 激活需 3 次成功，归档需 5 次失败
    SkillLifecycleManager::new(3_600_000, 3, 5)
}

// ----------------------------------------------------------
// 顶层 API 可达性（re-export 验证）
// ----------------------------------------------------------

#[test]
fn top_level_api_accessible() {
    let mgr = manager();
    // SkillLifecycleManager 可通过 crate 顶层访问
    assert_eq!(mgr.skill_count(), 0);
    assert!(mgr.get_active_skill_ids().is_empty());
}

// ----------------------------------------------------------
// 三态转移: Probationary → Active → Archived
// ----------------------------------------------------------

#[test]
fn full_lifecycle_probationary_to_active_to_archived() {
    let mut mgr = manager();
    mgr.register_skill("skill-1", 100);
    assert_eq!(
        mgr.get_state("skill-1"),
        Some(SkillLifecycleState::Probationary)
    );
    // 成功 3 次 → Active
    for i in 1..=3 {
        mgr.record_outcome("skill-1", true, 100 + i);
    }
    assert_eq!(mgr.get_state("skill-1"), Some(SkillLifecycleState::Active));
    // 连续失败 5 次 → Archived
    for i in 1..=5 {
        mgr.record_outcome("skill-1", false, 200 + i);
    }
    assert_eq!(
        mgr.get_state("skill-1"),
        Some(SkillLifecycleState::Archived)
    );
}

#[test]
fn probationary_directly_archived_on_failures() {
    let mut mgr = manager();
    mgr.register_skill("skill-1", 100);
    // 试用期内失败 5 次 → 直接归档（不经过 Active）
    for i in 1..=5 {
        mgr.record_outcome("skill-1", false, 100 + i);
    }
    assert_eq!(
        mgr.get_state("skill-1"),
        Some(SkillLifecycleState::Archived)
    );
    assert!(mgr.get_active_skill_ids().is_empty());
}

// ----------------------------------------------------------
// Active 成功重置 failure_count（规范 §10.5）
// ----------------------------------------------------------

#[test]
fn active_success_resets_failure_count_blocking_archive() {
    let mut mgr = manager();
    mgr.register_skill("skill-1", 100);
    for i in 1..=3 {
        mgr.record_outcome("skill-1", true, 100 + i);
    }
    // 失败 4 次（差一次归档），一次成功清零
    for i in 1..=4 {
        mgr.record_outcome("skill-1", false, 200 + i);
    }
    mgr.record_outcome("skill-1", true, 300);
    assert_eq!(mgr.get_contract("skill-1").unwrap().failure_count, 0);
    // 清零后再失败 4 次仍不归档
    for i in 1..=4 {
        mgr.record_outcome("skill-1", false, 400 + i);
    }
    assert_eq!(mgr.get_state("skill-1"), Some(SkillLifecycleState::Active));
}

// ----------------------------------------------------------
// Archived 终态（无复活路径）
// ----------------------------------------------------------

#[test]
fn archived_terminal_after_mixed_outcomes() {
    let mut mgr = manager();
    mgr.register_skill("skill-1", 100);
    for i in 1..=5 {
        mgr.record_outcome("skill-1", false, 100 + i);
    }
    assert_eq!(
        mgr.get_state("skill-1"),
        Some(SkillLifecycleState::Archived)
    );
    // 归档后任意结果序列不复活（终态，仅更新 last_used）
    for i in 1..=20 {
        mgr.record_outcome("skill-1", i % 2 == 0, 200 + i);
    }
    assert_eq!(
        mgr.get_state("skill-1"),
        Some(SkillLifecycleState::Archived)
    );
    assert_eq!(mgr.get_contract("skill-1").unwrap().last_used, 220);
}

#[test]
fn record_outcome_unknown_skill_is_noop() {
    let mut mgr = manager();
    // 未注册技能不 panic 且不产生注册
    mgr.record_outcome("ghost", true, 100);
    mgr.record_outcome("ghost", false, 100);
    assert_eq!(mgr.skill_count(), 0);
    assert_eq!(mgr.get_state("ghost"), None);
    assert!(mgr.get_contract("ghost").is_none());
}

// ----------------------------------------------------------
// 多技能注册表隔离
// ----------------------------------------------------------

#[test]
fn multiple_skills_transition_independently() {
    let mut mgr = manager();
    mgr.register_skill("s1", 100);
    mgr.register_skill("s2", 100);
    mgr.register_skill("s3", 100);
    // s1 激活 / s2 归档 / s3 保持试用
    for i in 1..=3 {
        mgr.record_outcome("s1", true, 100 + i);
    }
    for i in 1..=5 {
        mgr.record_outcome("s2", false, 100 + i);
    }
    assert_eq!(mgr.get_state("s1"), Some(SkillLifecycleState::Active));
    assert_eq!(mgr.get_state("s2"), Some(SkillLifecycleState::Archived));
    assert_eq!(mgr.get_state("s3"), Some(SkillLifecycleState::Probationary));
    assert_eq!(mgr.get_active_skill_ids(), vec!["s1".to_string()]);
}

// ----------------------------------------------------------
// proptest 状态机不变量
// ----------------------------------------------------------

proptest! {
    /// 任意结果序列下的状态机不变量:
    /// 1. Archived 终态不可逆（一旦归档永远归档）
    /// 2. Active 状态必有 success_count ≥ activation_threshold
    /// 3. 状态恒为三态之一（无非法状态）
    #[test]
    fn state_machine_invariants(outcomes in prop::collection::vec(any::<bool>(), 0..50)) {
        let mut mgr = manager();
        mgr.register_skill("skill-x", 0);
        let mut ever_archived = false;
        for (i, success) in outcomes.iter().enumerate() {
            mgr.record_outcome("skill-x", *success, (i + 1) as u64);
            let contract = mgr.get_contract("skill-x").unwrap();
            if contract.state == SkillLifecycleState::Archived {
                ever_archived = true;
            }
            if ever_archived {
                // 不变量 1: 终态不可逆
                prop_assert_eq!(contract.state, SkillLifecycleState::Archived);
            }
            if contract.state == SkillLifecycleState::Active {
                // 不变量 2: Active 必有足够成功计数
                prop_assert!(contract.success_count >= contract.activation_threshold);
                // 不变量补充: Active 未达归档阈值
                prop_assert!(contract.failure_count < contract.archive_threshold);
            }
        }
        // 不变量 3: 终态恒为合法三态
        let state = mgr.get_state("skill-x").unwrap();
        prop_assert!(matches!(
            state,
            SkillLifecycleState::Probationary
                | SkillLifecycleState::Active
                | SkillLifecycleState::Archived
        ));
    }
}
