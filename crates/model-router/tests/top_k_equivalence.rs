//! TDD 测试 — route_auto Top-K 优化等价性验证(Task V-9)
//!
//! 验证 `select_nth_unstable_by(1)` + `sort_by([1..])` 实现的 `route_auto`
//! 与 `sort_by` 全排序参考实现的结果完全一致:
//! - 选中模型 (`model_id`) 必须相同
//! - 候选列表 (`candidates`) 必须有序且逐元素相同
//!
//! WHY: 此处是算法优化(行为不变),测试作为回归保护基线,
//! 确保部分排序不会破坏 `RoutingDecision.candidates` 的有序契约
//! (types.rs 文档:"按策略优先级降序排序")。

use model_router::{ModelInfo, ModelRegistry, RoutingRequest, RoutingStrategy};
use nexus_core::{MultimodalInput, UserIntent};
use std::cmp::Ordering;

// === 测试辅助构造 ===

fn make_intent() -> UserIntent {
    UserIntent {
        intent_id: "i-1".into(),
        raw_text: "test".into(),
        multimodal_inputs: vec![MultimodalInput::Text("test".into())],
        risk_level: 10,
    }
}

fn make_request(tokens: u32) -> RoutingRequest {
    RoutingRequest {
        quest_id: "q-1".into(),
        intent: make_intent(),
        estimated_tokens: tokens,
        strategy: RoutingStrategy::Auto,
    }
}

fn make_model(id: &str, cost: f64, latency: u64, quality: f32) -> ModelInfo {
    ModelInfo {
        model_id: id.into(),
        provider: "test".into(),
        cost_per_1k_tokens: cost,
        avg_latency_ms: latency,
        max_context: 8192,
        quality_score: quality,
    }
}

fn build_registry(models: &[ModelInfo]) -> ModelRegistry {
    let registry = ModelRegistry::new();
    for m in models {
        registry.register(m.clone()).unwrap();
    }
    registry
}

// === 参考实现:用 sort_by 全排序(原始算法,作为等价性基准) ===

fn reference_route_auto(models: &[ModelInfo]) -> (String, Vec<String>) {
    if models.is_empty() {
        return (String::new(), Vec::new());
    }
    let max_cost = models
        .iter()
        .map(|m| m.cost_per_1k_tokens)
        .fold(0.0_f64, f64::max);
    let max_latency = models
        .iter()
        .map(|m| m.avg_latency_ms as f64)
        .fold(0.0_f64, f64::max);

    let mut scored: Vec<(f64, &ModelInfo)> = models
        .iter()
        .map(|m| {
            let cost_score = if max_cost > 0.0 {
                1.0 - (m.cost_per_1k_tokens / max_cost)
            } else {
                1.0
            };
            let latency_score = if max_latency > 0.0 {
                1.0 - (m.avg_latency_ms as f64 / max_latency)
            } else {
                1.0
            };
            let score = 0.4 * cost_score + 0.4 * latency_score + 0.2 * m.quality_score as f64;
            (score, m)
        })
        .collect();

    // 全排序:评分降序,相同则 model_id 升序(保证确定性)
    scored.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(Ordering::Equal)
            .then_with(|| a.1.model_id.cmp(&b.1.model_id))
    });

    let selected = scored[0].1.model_id.clone();
    let candidates: Vec<String> = scored
        .iter()
        .skip(1)
        .map(|(_, m)| m.model_id.clone())
        .collect();
    (selected, candidates)
}

// === 多组测试数据:不同规模、分数分布、边界情况 ===

fn test_cases() -> Vec<Vec<ModelInfo>> {
    vec![
        // 3 模型(默认配置规模)
        vec![
            make_model("a", 0.001, 100, 0.8),
            make_model("b", 0.01, 200, 0.9),
            make_model("c", 0.005, 150, 0.7),
        ],
        // 5 模型(中等规模,分数分布均匀)
        vec![
            make_model("m1", 0.001, 100, 0.5),
            make_model("m2", 0.005, 300, 0.9),
            make_model("m3", 0.01, 50, 0.3),
            make_model("m4", 0.003, 200, 0.7),
            make_model("m5", 0.008, 150, 0.6),
        ],
        // 10 模型(较大规模,验证 select_nth 在 n 较大时的正确性)
        vec![
            make_model("t01", 0.001, 100, 0.5),
            make_model("t02", 0.002, 200, 0.6),
            make_model("t03", 0.003, 300, 0.7),
            make_model("t04", 0.004, 400, 0.8),
            make_model("t05", 0.005, 500, 0.9),
            make_model("t06", 0.006, 600, 0.4),
            make_model("t07", 0.007, 700, 0.3),
            make_model("t08", 0.008, 800, 0.2),
            make_model("t09", 0.009, 900, 0.1),
            make_model("t10", 0.010, 1000, 0.0),
        ],
        // 边界:1 模型(select_nth 跳过,candidates 为空)
        vec![make_model("only", 0.001, 100, 0.8)],
        // 边界:2 模型(select_nth(1) 分区,scored[1..] 单元素排序)
        vec![make_model("a", 0.001, 100, 0.8), make_model("b", 0.01, 200, 0.9)],
        // 边界:等分(同分不同 model_id,验证 tiebreaker 确定性)
        vec![
            make_model("z", 0.001, 100, 0.8),
            make_model("y", 0.001, 100, 0.8),
            make_model("x", 0.001, 100, 0.8),
        ],
        // 边界:零成本零延迟(除零保护,max_cost/max_latency = 0)
        vec![
            make_model("zero1", 0.0, 0, 0.5),
            make_model("zero2", 0.0, 0, 0.5),
        ],
        // 边界:成本相同延迟不同(验证 latency_score 区分)
        vec![
            make_model("fast", 0.005, 10, 0.5),
            make_model("medium", 0.005, 100, 0.5),
            make_model("slow", 0.005, 500, 0.5),
        ],
    ]
}

// === 核心等价性测试 ===

/// 验证 select_nth_unstable_by(1) + sort[1..] 优化实现
/// 与 sort_by 全排序参考实现的结果完全一致(选中模型 + 有序候选列表)。
#[test]
fn test_top_k_select_nth_unstable_equivalence() {
    for (idx, models) in test_cases().into_iter().enumerate() {
        let registry = build_registry(&models);
        let req = make_request(1000);
        let decision = model_router::strategies::route_auto(&registry, &req).unwrap();
        let (ref_selected, ref_candidates) = reference_route_auto(&models);

        // 选中模型必须一致
        assert_eq!(
            decision.model_id, ref_selected,
            "case {}: selected model mismatch (expected {}, got {})",
            idx,
            ref_selected,
            decision.model_id
        );

        // 候选列表必须有序且逐元素一致(验证候选有序契约未被破坏)
        assert_eq!(
            decision.candidates, ref_candidates,
            "case {}: candidates mismatch (expected {:?}, got {:?})",
            idx,
            ref_candidates,
            decision.candidates
        );
    }
}

/// 验证候选列表确实按评分降序排列(契约:scored descending)。
/// 这是对有序契约的独立验证,不依赖参考实现。
#[test]
fn test_candidates_are_ordered_descending() {
    // 5 模型,分数分布明显不同
    let models = vec![
        make_model("m1", 0.001, 100, 0.5),
        make_model("m2", 0.005, 300, 0.9),
        make_model("m3", 0.01, 50, 0.3),
        make_model("m4", 0.003, 200, 0.7),
        make_model("m5", 0.008, 150, 0.6),
    ];
    let registry = build_registry(&models);
    let req = make_request(1000);
    let decision = model_router::strategies::route_auto(&registry, &req).unwrap();

    // 重建评分验证候选顺序
    let max_cost = models
        .iter()
        .map(|m| m.cost_per_1k_tokens)
        .fold(0.0_f64, f64::max);
    let max_latency = models
        .iter()
        .map(|m| m.avg_latency_ms as f64)
        .fold(0.0_f64, f64::max);
    let score_of = |id: &str| -> f64 {
        let m = models.iter().find(|m| m.model_id == id).unwrap();
        let cost_score = if max_cost > 0.0 {
            1.0 - (m.cost_per_1k_tokens / max_cost)
        } else {
            1.0
        };
        let latency_score = if max_latency > 0.0 {
            1.0 - (m.avg_latency_ms as f64 / max_latency)
        } else {
            1.0
        };
        0.4 * cost_score + 0.4 * latency_score + 0.2 * m.quality_score as f64
    };

    // candidates 必须按评分降序(score[i] >= score[i+1])
    let cand_scores: Vec<f64> = decision
        .candidates
        .iter()
        .map(|id| score_of(id))
        .collect();
    for w in cand_scores.windows(2) {
        assert!(
            w[0] >= w[1] || (w[0] - w[1]).abs() < 1e-9,
            "candidates not in descending score order: {:?} -> scores {:?}",
            decision.candidates,
            cand_scores
        );
    }

    // selected 的评分必须 >= 第一个 candidate 的评分
    let selected_score = score_of(&decision.model_id);
    if !cand_scores.is_empty() {
        assert!(
            selected_score >= cand_scores[0] || (selected_score - cand_scores[0]).abs() < 1e-9,
            "selected score {} < first candidate score {}",
            selected_score,
            cand_scores[0]
        );
    }
}

/// 验证单模型边界:select_nth 跳过,candidates 为空。
#[test]
fn test_single_model_select_nth_skipped() {
    let models = vec![make_model("only", 0.001, 100, 0.8)];
    let registry = build_registry(&models);
    let req = make_request(1000);
    let decision = model_router::strategies::route_auto(&registry, &req).unwrap();
    assert_eq!(decision.model_id, "only");
    assert!(decision.candidates.is_empty());
}
