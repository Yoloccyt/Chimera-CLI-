//! MoE 稀疏门控集成测试 — 验证大规模模型下的 Top-K 激活与退化行为
//!
//! 对应架构层:L1 Core
//! 对应创新点:MoE(Mixture of Experts)稀疏门控 — Ω-Sparse 对齐
//!
//! # 测试目标
//! 1. **Top-K 激活**:50+ 模型时 `route_auto_with_gate` 仅激活 K 个候选
//!    (candidates 长度 = K-1 ≤ top_k-1)
//! 2. **阈值退化**:模型数 < 阈值时退化为全量评估(candidates = n-1)
//! 3. **稀疏不变量**(proptest):任意模型规模下,门控激活数始终 ≤ top_k
//!
//! # 语法约束(§4.4 规则)
//! proptest 1.11+ 用 block-named 语法:`fn name(arg in strategy) { body }`

#![forbid(unsafe_code)]
// WHY 用 `< top_k` 而非 `len() + 1 <= top_k`:CLI -D warnings 覆盖 #![allow],
// 改用 `< top_k` 表达 "selected(1) + candidates(K-1) = K < top_k+1" 语义

use model_router::{ModelInfo, ModelRegistry, MoeGate, RoutingRequest, RoutingStrategy};
use nexus_core::{MultimodalInput, UserIntent};
use proptest::prelude::*;

/// 批量生成 n 个差异化模型用于测试
///
/// WHY 差异化:若所有模型特征相同,评分相同,排序不确定(依赖 model_id 兜底),
/// 无法验证 Top-K 选取的正确性。通过 cost/latency/quality 随 index 递增/递减,
/// 确保每个模型评分不同,Top-K 可明确验证。
fn make_models(n: usize) -> Vec<ModelInfo> {
    (0..n)
        .map(|i| ModelInfo {
            model_id: format!("model-{i:03}"),
            provider: "test".into(),
            // 成本随 index 递增:0.0001 ~ 0.0001 + n*0.0001
            cost_per_1k_tokens: 0.0001 + i as f64 * 0.0001,
            // 延迟随 index 递增:50 ~ 50 + n*5
            avg_latency_ms: 50 + (i as u64) * 5,
            max_context: 8192,
            // 质量随 index 递减:0.99 ~ 0.99 - n*0.01(clamp 到 0.0)
            quality_score: (0.99 - i as f32 * 0.01).max(0.0),
        })
        .collect()
}

/// 从模型列表构造注册表
fn registry_from(models: &[ModelInfo]) -> ModelRegistry {
    let registry = ModelRegistry::new();
    for m in models {
        registry.register(m.clone()).expect("注册失败");
    }
    registry
}

/// 构造测试路由请求(Auto 策略)
fn make_request(tokens: u32) -> RoutingRequest {
    RoutingRequest {
        quest_id: "q-test".into(),
        intent: UserIntent {
            intent_id: "i-test".into(),
            raw_text: "test".into(),
            multimodal_inputs: vec![MultimodalInput::Text("test".into())],
            risk_level: 10,
        },
        estimated_tokens: tokens,
        strategy: RoutingStrategy::Auto,
    }
}

/// 门控模式(模型数 ≥ 阈值)应仅激活 Top-K 候选
///
/// 验证:`candidates.len() + 1 <= top_k`(selected 1 + candidates K-1 = K)
#[test]
fn test_moe_gate_activates_top_k_only() {
    // 50 模型 = 阈值,触发门控(50 >= 50)
    let models = make_models(50);
    let registry = registry_from(&models);
    let req = make_request(1000);
    let gate = MoeGate::default(); // threshold=50, top_k=5

    let decision =
        model_router::strategies::route_auto_with_gate(&registry, &req, &gate).expect("路由应成功");

    // selected(1) + candidates(K-1) = K ≤ top_k
    assert!(
        decision.candidates.len() < 5,
        "门控模式应激活 ≤ top_k=5 个,实际 candidates={} (selected 1 + candidates {} = {})",
        decision.candidates.len(),
        decision.candidates.len(),
        decision.candidates.len() + 1
    );
    assert_eq!(
        decision.candidates.len(),
        4,
        "top_k=5 时 candidates 应为 4(selected 1 + candidates 4 = 5)"
    );
}

/// 100 模型规模下门控仍仅激活 Top-K
#[test]
fn test_moe_gate_activates_top_k_only_100_models() {
    let models = make_models(100);
    let registry = registry_from(&models);
    let req = make_request(1000);
    let gate = MoeGate::default();

    let decision =
        model_router::strategies::route_auto_with_gate(&registry, &req, &gate).expect("路由应成功");

    assert!(
        decision.candidates.len() < 5,
        "100 模型门控应激活 ≤ 5,实际 {}",
        decision.candidates.len() + 1
    );
}

/// 200 模型规模下门控仍仅激活 Top-K
#[test]
fn test_moe_gate_activates_top_k_only_200_models() {
    let models = make_models(200);
    let registry = registry_from(&models);
    let req = make_request(1000);
    let gate = MoeGate::default();

    let decision =
        model_router::strategies::route_auto_with_gate(&registry, &req, &gate).expect("路由应成功");

    assert!(
        decision.candidates.len() < 5,
        "200 模型门控应激活 ≤ 5,实际 {}",
        decision.candidates.len() + 1
    );
}

/// 自定义 top_k=3 时应激活 ≤ 3 个
#[test]
fn test_moe_gate_custom_top_k() {
    let models = make_models(60);
    let registry = registry_from(&models);
    let req = make_request(1000);
    let gate = MoeGate::new(50, 3);

    let decision =
        model_router::strategies::route_auto_with_gate(&registry, &req, &gate).expect("路由应成功");

    assert!(
        decision.candidates.len() < 3,
        "top_k=3 应激活 ≤ 3,实际 {}",
        decision.candidates.len() + 1
    );
    assert_eq!(decision.candidates.len(), 2);
}

/// 模型数 < 阈值时退化为全量评估,candidates = n-1
#[test]
fn test_moe_gate_degrades_when_below_threshold() {
    // 49 模型 < 50 阈值,退化为全量
    let n = 49;
    let models = make_models(n);
    let registry = registry_from(&models);
    let req = make_request(1000);
    let gate = MoeGate::default(); // threshold=50

    let decision =
        model_router::strategies::route_auto_with_gate(&registry, &req, &gate).expect("路由应成功");

    assert_eq!(
        decision.candidates.len(),
        n - 1,
        "退化模式 candidates 应为 n-1={},实际 {}",
        n - 1,
        decision.candidates.len()
    );
}

/// 默认 3 模型配置退化为全量评估(向后兼容验证)
#[test]
fn test_moe_gate_degrades_default_config() {
    let registry = ModelRegistry::from_config(&model_router::RouterConfig::default());
    let req = make_request(1000);
    let gate = MoeGate::default();

    let decision =
        model_router::strategies::route_auto_with_gate(&registry, &req, &gate).expect("路由应成功");

    // 3 模型 < 50,退化,candidates = 2
    assert_eq!(decision.candidates.len(), 2);
}

/// 退化模式与门控模式选中的模型应一致(门控召回验证)
#[test]
fn test_moe_gate_recalls_best_model() {
    let models = make_models(55);
    let registry = registry_from(&models);
    let req = make_request(1000);

    // 全量评估(threshold 极大,强制退化)
    let full_gate = MoeGate::new(usize::MAX, 5);
    let full_decision = model_router::strategies::route_auto_with_gate(&registry, &req, &full_gate)
        .expect("全量评估应成功");

    // 门控评估(默认 threshold=50)
    let moe_gate = MoeGate::default();
    let moe_decision = model_router::strategies::route_auto_with_gate(&registry, &req, &moe_gate)
        .expect("门控评估应成功");

    // 门控应选中与全量评估相同的模型(召回保证)
    assert_eq!(
        full_decision.model_id, moe_decision.model_id,
        "门控应召回全量评估的最优模型: full={} moe={}",
        full_decision.model_id, moe_decision.model_id
    );
}

/// 退化模式下 route_auto(默认)与 route_auto_with_gate(退化)行为一致
#[test]
fn test_route_auto_backward_compatible_below_threshold() {
    let registry = ModelRegistry::from_config(&model_router::RouterConfig::default());
    let req = make_request(1000);

    // 原始 route_auto(内部用默认 MoeGate,3 模型退化)
    let decision_default =
        model_router::strategies::route_auto(&registry, &req).expect("route_auto 应成功");

    // 显式退化 gate
    let degrade_gate = MoeGate::new(usize::MAX, 5);
    let decision_degrade =
        model_router::strategies::route_auto_with_gate(&registry, &req, &degrade_gate)
            .expect("退化 gate 应成功");

    // 两者行为应完全一致(退化 = 全量)
    assert_eq!(decision_default.model_id, decision_degrade.model_id);
    assert_eq!(decision_default.candidates, decision_degrade.candidates);
    assert_eq!(
        decision_default.estimated_cost,
        decision_degrade.estimated_cost
    );
}

// 稀疏不变量:任意模型规模 n ∈ [50, 200] 时,门控激活数 ≤ top_k
//
// WHY proptest:覆盖边界与随机规模,验证门控稀疏性不变量始终成立。
// 256 cases 确保统计显著性。
proptest! {
    #[test]
    fn prop_moe_gate_sparsity_invariant(
        n in 50usize..=200,
        top_k in 1usize..=10,
    ) {
        let models = make_models(n);
        let registry = registry_from(&models);
        let req = make_request(1000);
        let gate = MoeGate::new(50, top_k);

        let decision = model_router::strategies::route_auto_with_gate(&registry, &req, &gate)
            .expect("路由应成功");

        // 不变量:selected(1) + candidates ≤ top_k
        // 当 n >= top_k 时,candidates = top_k - 1,总计 = top_k
        // 当 n < top_k(理论不会发生,因 n>=50 > top_k max=10)时,clamp
        let activated = decision.candidates.len() + 1;
        prop_assert!(
            activated <= top_k,
            "门控激活数 {} 应 ≤ top_k={}(n={})",
            activated, top_k, n
        );

        // 不变量:选中模型必须在原始模型列表中
        let model_ids: Vec<&str> = models.iter().map(|m| m.model_id.as_str()).collect();
        prop_assert!(
            model_ids.contains(&decision.model_id.as_str()),
            "选中模型 {} 必须在注册表中",
            decision.model_id
        );

        // 不变量:candidates 中的每个 id 都必须在注册表中
        for cid in &decision.candidates {
            prop_assert!(
                model_ids.contains(&cid.as_str()),
                "候选 {} 必须在注册表中",
                cid
            );
        }

        // 不变量:candidates 不应包含 selected model_id
        prop_assert!(
            !decision.candidates.contains(&decision.model_id),
            "candidates 不应包含选中的模型 {}",
            decision.model_id
        );

        // 不变量:candidates 无重复
        let unique: std::collections::HashSet<&String> = decision.candidates.iter().collect();
        prop_assert_eq!(
            unique.len(),
            decision.candidates.len(),
            "candidates 不应有重复"
        );
    }
}

// 退化不变量:任意模型规模 n < threshold 时,退化为全量评估
proptest! {
    #[test]
    fn prop_moe_gate_degrade_invariant(
        n in 1usize..=49,
        threshold in 50usize..=100,
    ) {
        let models = make_models(n);
        let registry = registry_from(&models);
        let req = make_request(1000);
        let gate = MoeGate::new(threshold, 5);

        let decision = model_router::strategies::route_auto_with_gate(&registry, &req, &gate)
            .expect("路由应成功");

        // 退化模式:candidates = n - 1(全量)
        prop_assert_eq!(
            decision.candidates.len(),
            n.saturating_sub(1),
            "退化模式(n={} < threshold={})candidates 应为 n-1",
            n, threshold
        );
    }
}
