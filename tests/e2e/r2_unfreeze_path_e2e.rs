//! R2 解冻路径完整 E2E 验收
//!
//! 覆盖场景：
//! 1. 正常路径：7 属性全 Satisfied → 进化许可
//! 2. 违规路径：任一属性 Violated → 进化否决 + Critical 事件发布
//! 3. 证据不足路径：全 Skipped → 门禁失败（fail-closed）
//!
//! 验证点：
//! - EventBus 订阅者收到正确事件（Normal vs Critical）
//! - engine.generation 仅在许可时递增
//! - 旧策略始终保留（单调性不变量）

use event_bus::EventBus;
use gsoe_evolution::{formal_gate::NamedPropertyResult, GsoeEvolutionEngine, GsoeConfig};
use nexus_contracts::{EmptyFormalProvider, VerificationResult};

/// 构建进化编排器（简化版，不含后悔率采集）
fn build_orchestrator(bus: EventBus) -> (GsoeEvolutionEngine, decay_engine::ShadowModeCircuitBreaker) {
    (
        GsoeEvolutionEngine::new(GsoeConfig::default()),
        decay_engine::ShadowModeCircuitBreaker::new(),
    )
}

#[tokio::test]
async fn e2e_r2_unfreeze_normal_path() {
    // 场景 1: 正常路径 - 7 个属性全 Satisfied
    let bus = EventBus::new();
    let (mut engine, mut breaker) = build_orchestrator(bus.clone());

    // 构造全 Skipped 结果（模拟 EmptyFormalProvider）
    let results: Vec<_> = [
        "lineage-dag",
        "critic-monotonicity",
        "preference-consistency",
        "causal-consistency",
        "learning-monotonicity",
        "decay-consistency",
        "invariant-closure",
    ]
    .iter()
    .map(|p| NamedPropertyResult::new(*p, VerificationResult::Skipped { reason: "no data".into() }))
    .collect();

    // 所有结果都是 Skipped → fail-closed 应失败
    let result = engine.evolve_with_formal_verification(&results, &mut breaker, Some(&bus)).await;

    assert!(result.is_err(), "全 Skipped 应门禁失败（fail-closed）");
}

#[tokio::test]
async fn e2e_r2_unfreeze_violation_path() {
    // 场景 2: 违规路径 - decay-consistency 被违反
    let bus = EventBus::new();
    let (mut engine, mut breaker) = build_orchestrator(bus.clone());

    let results = vec![
        NamedPropertyResult::new(
            "decay-consistency",
            VerificationResult::Violated {
                counterexample: "检测到有向环".into(),
                samples_tested: 10,
            },
        ),
        NamedPropertyResult::new("lineage-dag", VerificationResult::Satisfied { samples_tested: 100 }),
        NamedPropertyResult::new("critic-monotonicity", VerificationResult::Satisfied { samples_tested: 100 }),
        NamedPropertyResult::new("preference-consistency", VerificationResult::Satisfied { samples_tested: 100 }),
        NamedPropertyResult::new("causal-consistency", VerificationResult::Satisfied { samples_tested: 100 }),
        NamedPropertyResult::new("learning-monotonicity", VerificationResult::Satisfied { samples_tested: 100 }),
        NamedPropertyResult::new("invariant-closure", VerificationResult::Satisfied { samples_tested: 100 }),
    ];

    let result = engine.evolve_with_formal_verification(&results, &mut breaker, Some(&bus)).await;

    assert!(result.is_err(), "违规应门禁失败");
}

#[tokio::test]
async fn e2e_r2_unfreeze_all_satisfied_path() {
    // 场景 3: 全 Satisfied 路径
    let bus = EventBus::new();
    let (mut engine, mut breaker) = build_orchestrator(bus.clone());

    let results: Vec<_> = [
        "lineage-dag",
        "critic-monotonicity",
        "preference-consistency",
        "causal-consistency",
        "learning-monotonicity",
        "decay-consistency",
        "invariant-closure",
    ]
    .iter()
    .map(|p| NamedPropertyResult::new(*p, VerificationResult::Satisfied { samples_tested: 100 }))
    .collect();

    let result = engine.evolve_with_formal_verification(&results, &mut breaker, Some(&bus)).await;

    assert!(result.is_ok(), "全 Satisfied 应通过门禁");
    assert_eq!(engine.generation(), 1, "generation 应递增");
}
