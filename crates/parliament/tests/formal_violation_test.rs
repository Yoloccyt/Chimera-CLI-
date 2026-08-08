//! 行为契约强制层测试（Milestone B-3c，九层防御 L0 补齐）
//!
//! 对应方案（CHIMERA_V3_专项优化方案_v2.21基线.md §5.1 P2 / §7.2 九层防御 L0 补齐）：
//! 行为契约仅"审计消费"覆盖、无强制层 → 补齐 `enforce` 强制校验 +
//! FormalViolation 事件（违反路径可发布事件）+ Parliament 审议入口。

#![forbid(unsafe_code)]

use event_bus::{EventBus, EventMetadata, NexusEvent};
use nexus_contracts::behavior_contract::{BehaviorContract, ContractCheckOutcome, ContractContext};
use parliament::formal_violation::handle_formal_violation;

/// 构造含前置/后置/不变量断言的契约
fn make_contract() -> BehaviorContract {
    BehaviorContract::new("bc-test-1", "event_bus::EventBus", ContractContext::Runtime)
        .with_precondition("bus 已初始化")
        .with_postcondition("事件已发布")
        .with_invariant("无未消费订阅者")
}

/// 全部断言满足 → Satisfied
#[test]
fn enforce_satisfied_when_all_assertions_observed() {
    let contract = make_contract();
    let observed = vec![
        "bus 已初始化".to_string(),
        "事件已发布".to_string(),
        "无未消费订阅者".to_string(),
    ];
    let outcome = contract.enforce(&observed);
    assert_eq!(outcome, ContractCheckOutcome::Satisfied);
}

/// 缺失断言 → Violated 且列出缺失项
#[test]
fn enforce_violated_lists_missing_assertions() {
    let contract = make_contract();
    let observed = vec!["bus 已初始化".to_string()];
    let outcome = contract.enforce(&observed);
    match outcome {
        ContractCheckOutcome::Violated { missing } => {
            assert!(missing.len() >= 2, "应缺失后置与不变量: {missing:?}");
            assert!(missing.iter().any(|m| m.contains("事件已发布")));
        }
        other => panic!("应 Violated: {other:?}"),
    }
}

/// 空观测 → 全部断言缺失
#[test]
fn enforce_violated_on_empty_observation() {
    let contract = make_contract();
    let outcome = contract.enforce(&[]);
    assert!(matches!(outcome, ContractCheckOutcome::Violated { .. }));
}

/// 空契约（无断言）→ 恒 Satisfied
#[test]
fn empty_contract_is_satisfied() {
    let contract = BehaviorContract::new("bc-empty", "my::Type", ContractContext::Runtime);
    assert!(contract.is_empty());
    assert_eq!(contract.enforce(&[]), ContractCheckOutcome::Satisfied);
}

/// E2E 违反路径：contract 违反 → 发布 FormalViolation 事件
#[tokio::test]
async fn violation_publishes_formal_violation_event() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let contract = make_contract();
    let observed = vec!["bus 已初始化".to_string()];

    let outcome = contract.enforce(&observed);
    if let ContractCheckOutcome::Violated { missing } = outcome {
        bus.publish(NexusEvent::FormalViolation {
            metadata: EventMetadata::new("test"),
            contract_id: contract.contract_id.clone(),
            target_type: contract.target_type.clone(),
            violations: missing,
            context: ContractContext::Runtime,
        })
        .await
        .unwrap();
    }

    // 消费 FormalViolation 事件
    let event = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
        .await
        .expect("应收到 FormalViolation")
        .expect("recv 不应失败");
    match event {
        NexusEvent::FormalViolation { contract_id, .. } => {
            assert_eq!(contract_id, "bc-test-1");
        }
        other => panic!("应收到 FormalViolation: {other:?}"),
    }
}

/// Parliament 审议入口：FormalViolation → 返回否决建议
#[tokio::test]
async fn parliament_handles_formal_violation() {
    let contract = make_contract();
    let verdict = handle_formal_violation(&contract.contract_id, &contract.target_type);
    assert!(
        verdict.contains("reject") || verdict.contains("否决") || verdict.contains("deny"),
        "审议应给出否决建议: {verdict:?}"
    );
}
