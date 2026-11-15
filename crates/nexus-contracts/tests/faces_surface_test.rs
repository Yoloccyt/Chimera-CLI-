//! 域导向聚合面（faces）锁 —— L0-C
//!
//! 锁两件事：
//! 1. **面符号全部可达**：四个面（domain/execution/governance/learning）的每个
//!    再导出都必须解析（编译期即证）——任何"面里有、根面没有"的孤儿会在
//!    `use nexus_contracts::faces::<face>::<Sym>` 处直接编译失败。
//! 2. **面符号与根面同源**：re-export 不制造新类型，本锁以 `TypeId` 相等
//!    抽查高频类型，防止未来有人把面改成独立定义（那会是第二真值源）；
//!    trait 无 `TypeId`，改以 `&dyn` 路径解析证明可达。
#![allow(clippy::unwrap_used, clippy::expect_used)] // test-module unwrap is the Rust idiom; E-5 targets production code

use std::any::TypeId;

use nexus_contracts::faces::domain as f_domain;
use nexus_contracts::faces::execution as f_execution;
use nexus_contracts::faces::governance as f_governance;
use nexus_contracts::faces::learning as f_learning;

#[test]
fn domain_face_reexports_resolve_and_match_root() {
    assert_eq!(
        TypeId::of::<f_domain::UserIntent>(),
        TypeId::of::<nexus_contracts::UserIntent>()
    );
    assert_eq!(
        TypeId::of::<f_domain::Quest>(),
        TypeId::of::<nexus_contracts::Quest>()
    );
    assert_eq!(
        TypeId::of::<f_domain::Checkpoint>(),
        TypeId::of::<nexus_contracts::Checkpoint>()
    );
    assert_eq!(
        TypeId::of::<f_domain::EventMetadata>(),
        TypeId::of::<nexus_contracts::EventMetadata>()
    );
    assert_eq!(
        TypeId::of::<f_domain::GraphIdentity>(),
        TypeId::of::<nexus_contracts::GraphIdentity>()
    );
    assert_eq!(
        TypeId::of::<f_domain::TaskStatus>(),
        TypeId::of::<nexus_contracts::TaskStatus>()
    );
}

#[test]
fn execution_face_reexports_resolve_and_match_root() {
    assert_eq!(
        TypeId::of::<f_execution::ToolPlan>(),
        TypeId::of::<nexus_contracts::ToolPlan>()
    );
    // tool_dispatch 族无顶层值级导出（重构前实况即如此），根面对照走模块路径
    assert_eq!(
        TypeId::of::<f_execution::DispatchRequest>(),
        TypeId::of::<nexus_contracts::tool_dispatch::DispatchRequest>()
    );
    // trait 无 TypeId：`&dyn` 路径解析即可达证明
    let _: Option<&dyn f_execution::ToolDispatcher> = None;
    assert_eq!(
        TypeId::of::<f_execution::TodoClaim>(),
        TypeId::of::<nexus_contracts::TodoClaim>()
    );
    assert_eq!(
        TypeId::of::<f_execution::ToolId>(),
        TypeId::of::<nexus_contracts::ToolId>()
    );
    // guards 常量模块经面可达
    let _ = f_execution::guards::MAX_STEPS;
}

#[test]
fn governance_face_reexports_resolve_and_match_root() {
    let _: Option<&dyn f_governance::CommandValidator> = None;
    assert_eq!(
        TypeId::of::<f_governance::NexusError>(),
        TypeId::of::<nexus_contracts::NexusError>()
    );
    assert_eq!(
        TypeId::of::<f_governance::BudgetTier>(),
        TypeId::of::<nexus_contracts::BudgetTier>()
    );
    assert_eq!(
        TypeId::of::<f_governance::NamespaceQuota>(),
        TypeId::of::<nexus_contracts::NamespaceQuota>()
    );
    // 纯函数经面可达（函数无 TypeId，路径解析即证明）
    let _ = f_governance::agreement_rate_milli;
    let _ = f_governance::assert_archive_monotonicity;
}

#[test]
fn learning_face_reexports_resolve_and_match_root() {
    assert_eq!(
        TypeId::of::<f_learning::ExperienceCard>(),
        TypeId::of::<nexus_contracts::ExperienceCard>()
    );
    let _: Option<&dyn f_learning::RLHook> = None;
    assert_eq!(
        TypeId::of::<f_learning::CapabilityToken>(),
        TypeId::of::<nexus_contracts::CapabilityToken>()
    );
    // reward 族无顶层值级导出（重构前实况即如此），面的再导出走模块路径
    assert_eq!(
        TypeId::of::<f_learning::RewardSpec>(),
        TypeId::of::<nexus_contracts::reward::RewardSpec>()
    );
    assert_eq!(
        TypeId::of::<f_learning::SkillLifecycleState>(),
        TypeId::of::<nexus_contracts::SkillLifecycleState>()
    );
}
