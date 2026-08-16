//! Skill 生命周期契约集成测试 — 状态机属性与序列化（v3.4.0 §5.5）
//!
//! 覆盖: 三态状态机全转移路径 / 不可变记录链 / proptest 状态机属性 /
//! 与六维控制面 D2 Skills 渐进加载的配置协同

#![forbid(unsafe_code)]

use nexus_contracts::{HarnessConfigContract, SkillLifecycleContract, SkillLifecycleState};
use proptest::prelude::*;

// ----------------------------------------------------------
// 状态机全转移路径
// ----------------------------------------------------------

#[test]
fn full_transition_path_success_dominant() {
    // 路径: Probationary → Active（3 成功）→ 保持 Active（继续成功）
    let mut c = SkillLifecycleContract::new_probationary("skill-a", 100);
    for i in 1..=3 {
        c = c.record_success(100 + i);
    }
    assert_eq!(c.state, SkillLifecycleState::Active);
    assert!(c.is_retrievable());
    let before = c.clone();
    c = c.record_success(200);
    assert_eq!(c.state, SkillLifecycleState::Active);
    assert_eq!(c.success_count, before.success_count + 1);
}

#[test]
fn full_transition_path_failure_dominant() {
    // 路径: Probationary → Archived（5 失败，未经 Active）
    let mut c = SkillLifecycleContract::new_probationary("skill-b", 100);
    for i in 1..=5 {
        c = c.record_failure(100 + i);
    }
    assert_eq!(c.state, SkillLifecycleState::Archived);
    assert!(!c.is_retrievable());
    // 终态不可逆（铁律: 归档无复活路径）——计数仍累加，但状态保持 Archived
    let archived = c.clone();
    c = c.record_success(200);
    assert_eq!(c.state, SkillLifecycleState::Archived);
    assert_eq!(c.success_count, archived.success_count + 1);
}

#[test]
fn mixed_history_respects_both_thresholds() {
    // 混合历史: 2 成功 + 4 失败 → 失败达阈先归档（5），成功未达（3）
    let mut c = SkillLifecycleContract::new_probationary("skill-c", 100);
    c = c.record_success(101);
    c = c.record_failure(102);
    c = c.record_failure(103);
    c = c.record_success(104);
    c = c.record_failure(105);
    assert_eq!(c.state, SkillLifecycleState::Probationary); // 2 成功 < 3, 3 失败 < 5
    c = c.record_failure(106);
    assert_eq!(c.state, SkillLifecycleState::Probationary); // 4 失败 < 5，仍试用期
    c = c.record_failure(107);
    assert_eq!(c.state, SkillLifecycleState::Archived); // 5 失败 → 归档
}

// ----------------------------------------------------------
// 不可变记录链
// ----------------------------------------------------------

#[test]
fn record_chain_is_immutable() {
    let original = SkillLifecycleContract::new_probationary("skill-d", 100);
    let v1 = original.record_success(101);
    let v2 = v1.record_success(102);
    // 原始与中间版本均保持原状（版本化记录链）
    assert_eq!(original.success_count, 0);
    assert_eq!(v1.success_count, 1);
    assert_eq!(v2.success_count, 2);
    assert_eq!(original.state, SkillLifecycleState::Probationary);
}

// ----------------------------------------------------------
// 与六维控制面 D2 的配置协同
// ----------------------------------------------------------

#[test]
fn skill_config_coordination_with_six_dimensions() {
    // Skill 生命周期契约 ↔ HarnessConfigContract D2 渐进加载协同
    let config = HarnessConfigContract::default_contract();
    assert!(config.d2_tool.progressive_skill_loading);
    assert_eq!(config.d2_tool.max_full_skill_load, 4);
    // 激活后的技能可被检索使用（与渐进加载配置一致）
    let mut skill = SkillLifecycleContract::new_probationary("skill-e", 100);
    for i in 1..=3 {
        skill = skill.record_success(100 + i);
    }
    assert!(skill.is_retrievable());
    assert!(config.d2_tool.progressive_skill_loading);
}

// ----------------------------------------------------------
// proptest 状态机属性
// ----------------------------------------------------------

proptest! {
    /// 状态机单调性: 任意成功/失败序列，success_count 与 failure_count 单调不减
    #[test]
    fn state_machine_counts_monotonic(
        successes in 0..10u32,
        failures in 0..10u32,
        start_success in 0..5u32,
        start_failure in 0..5u32,
    ) {
        let mut c = SkillLifecycleContract::new_probationary("skill-prop", 0);
        c.success_count = start_success;
        c.failure_count = start_failure;
        let initial_s = c.success_count;
        let initial_f = c.failure_count;
        for i in 0..successes {
            c = c.record_success(u64::from(i) + 1);
        }
        for i in 0..failures {
            c = c.record_failure(u64::from(i) + 1);
        }
        prop_assert!(c.success_count >= initial_s);
        prop_assert!(c.failure_count >= initial_f);
        // 状态合法性: 归档后不得回到 Active/Probationary
        if c.state == SkillLifecycleState::Archived {
            // 再记录成功也不得复活
            let after = c.record_success(999);
            prop_assert_eq!(after.state, SkillLifecycleState::Archived);
        }
    }

    /// 序列化属性: 任意状态机历史可无损往返（JSON + MsgPack）
    #[test]
    fn lifecycle_serde_roundtrip(
        successes in 0..6u32,
        failures in 0..6u32,
    ) {
        let mut c = SkillLifecycleContract::new_probationary("skill-ser", 0);
        for i in 0..successes {
            c = c.record_success(u64::from(i) + 1);
        }
        for i in 0..failures {
            c = c.record_failure(u64::from(i) + 1);
        }
        let json = serde_json::to_string(&c).expect("JSON 序列化失败");
        let back: SkillLifecycleContract = serde_json::from_str(&json).expect("JSON 反序列化失败");
        prop_assert_eq!(&back, &c);
        let bytes = rmp_serde::to_vec(&c).expect("MsgPack 序列化失败");
        let back2: SkillLifecycleContract = rmp_serde::from_slice(&bytes).expect("MsgPack 反序列化失败");
        prop_assert_eq!(&back2, &c);
    }
}

// ----------------------------------------------------------
// 顶层 API 可达性
// ----------------------------------------------------------

#[test]
fn top_level_api_accessible() {
    use nexus_contracts::prelude::*;
    let c = SkillLifecycleContract::new_probationary("skill-top", 0);
    assert_eq!(c.state, SkillLifecycleState::Probationary);
    // 线格式冻结: 三态 JSON 形态
    assert_eq!(
        serde_json::to_string(&SkillLifecycleState::Active).expect("JSON 序列化失败"),
        "\"active\""
    );
    assert_eq!(
        serde_json::to_string(&SkillLifecycleState::Archived).expect("JSON 序列化失败"),
        "\"archived\""
    );
}
