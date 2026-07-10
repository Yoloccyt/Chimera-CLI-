//! 路由策略实现 — 三种策略对应不同任务场景
//!
//! 对应架构:L1 Core,被 ModelRouter 调用
//!
//! # 三策略设计
//! - `Lite`:成本优先,选择 `cost_per_1k_tokens` 最低的模型
//! - `Efficient`:延迟优先,选择 `avg_latency_ms` 最低的模型
//! - `Auto`:综合评分,加权计算成本/延迟/质量,选最高分
//!
//! # 成本预估公式
//! `estimated_cost = (estimated_tokens / 1000) * cost_per_1k_tokens * 100`
//! 单位为美分(1 美元 = 100 美分),与 `BudgetExceeded` 事件保持一致。

use crate::error::RouterError;
use crate::moe::{HistoryStore, MoeGate};
use crate::registry::ModelRegistry;
use crate::types::{ModelInfo, RoutingDecision, RoutingRequest};

/// 计算预估成本(美分)
///
/// 公式:`(tokens / 1000) * cost_per_1k_tokens * 100`
/// WHY:cost_per_1k_tokens 单位为美元/千 token,乘以 100 转为美分,
/// 与 `BudgetExceeded` 事件的 `current`/`limit` 字段单位一致。
///
/// pub(crate):供 `router.rs` 在 CACR 降级路径中复用,保证成本计算逻辑单一来源。
pub(crate) fn estimate_cost(tokens: u32, cost_per_1k: f64) -> u64 {
    let cost_usd = (tokens as f64 / 1000.0) * cost_per_1k;
    (cost_usd * 100.0).round() as u64
}

/// Lite 策略:选择 `cost_per_1k_tokens` 最低的模型
pub fn route_lite(
    registry: &ModelRegistry,
    req: &RoutingRequest,
) -> Result<RoutingDecision, RouterError> {
    let mut models = registry.list_by_cost();
    if models.is_empty() {
        return Err(RouterError::NoModelsRegistered);
    }

    let selected = models.remove(0);
    let estimated_cost = estimate_cost(req.estimated_tokens, selected.cost_per_1k_tokens);
    let candidates: Vec<String> = models.iter().map(|m| m.model_id.clone()).collect();

    Ok(RoutingDecision {
        model_id: selected.model_id.clone(),
        route_reason: format!(
            "Lite: selected {} (cost ${:.6}/1k, lowest among {} candidates)",
            selected.model_id,
            selected.cost_per_1k_tokens,
            candidates.len() + 1
        ),
        estimated_cost,
        candidates,
    })
}

/// Efficient 策略:选择 `avg_latency_ms` 最低的模型
pub fn route_efficient(
    registry: &ModelRegistry,
    req: &RoutingRequest,
) -> Result<RoutingDecision, RouterError> {
    let mut models = registry.list_by_latency();
    if models.is_empty() {
        return Err(RouterError::NoModelsRegistered);
    }

    let selected = models.remove(0);
    let estimated_cost = estimate_cost(req.estimated_tokens, selected.cost_per_1k_tokens);
    let candidates: Vec<String> = models.iter().map(|m| m.model_id.clone()).collect();

    Ok(RoutingDecision {
        model_id: selected.model_id.clone(),
        route_reason: format!(
            "Efficient: selected {} (latency {}ms, lowest among {} candidates)",
            selected.model_id,
            selected.avg_latency_ms,
            candidates.len() + 1
        ),
        estimated_cost,
        candidates,
    })
}

/// Auto 策略:加权评分选择综合最优(默认 MoE 门控)
///
/// 评分公式:
/// `score = 0.4 * (1 - cost_normalized) + 0.4 * (1 - latency_normalized) + 0.2 * quality_score`
///
/// WHY 权重分配:成本与延迟同等重要(各 0.4),质量作为补充(0.2)。
/// 归一化使用 `value / max_value`,确保所有维度在 [0, 1] 范围内可比。
/// 当 max_value 为 0(所有模型该维度相同)时,该项直接给满分 1.0,
/// 避免除零并表达"该维度无差异,不影响选择"的语义。
///
/// # MoE 稀疏门控
/// 50+ 模型时先用 `MoeGate` 轻量级评分粗筛 Top-5,再仅对 Top-5 做完整
/// 归一化评分;模型数 < 50 时退化为全量评估(行为与未启用 MoE 一致)。
/// 详见 [`crate::moe`] 模块文档。
///
/// WHY 使用默认 `MoeGate::default()` + `history=None`:保持 `route_auto` 签名
/// 不变(向后兼容 v1.2.0)。需要五维评分(历史维度)时使用
/// [`route_auto_with_gate`] 并传入 `Some(&dyn HistoryStore)`。
pub fn route_auto(
    registry: &ModelRegistry,
    req: &RoutingRequest,
) -> Result<RoutingDecision, RouterError> {
    route_auto_with_gate(registry, req, &MoeGate::default(), None)
}

/// Auto 策略(可配置 MoE 门控 + 历史存储)— 供 bench / 可配置场景使用
///
/// WHY 独立 pub fn:`route_auto` 签名不变(向后兼容),但 bench 需要对比
/// "全量评估"vs"MoE 门控"vs"五维评分",且未来 `ModelRouter` 可能持有
/// `MoeGate` + `HistoryStore`。提取完整逻辑到此函数,`route_auto` 仅作
/// 默认门控 + 无历史的薄封装。
///
/// # 行为
/// - `gate.gate()` 返回全部模型(退化):完整评估对全部模型做归一化,
///   行为与原 `route_auto` 完全一致
/// - `gate.gate()` 返回 Top-K(门控):完整评估仅对 Top-K 做归一化,
///   `candidates` 长度 = K-1(而非 n-1)
/// - `history=Some`:门控评分启用五维(历史充足模型),不足模型降级三维
/// - `history=None`:全部模型三维降级(v1.2.0 行为)
pub fn route_auto_with_gate(
    registry: &ModelRegistry,
    req: &RoutingRequest,
    gate: &MoeGate,
    history: Option<&dyn HistoryStore>,
) -> Result<RoutingDecision, RouterError> {
    let models = registry.list();
    if models.is_empty() {
        return Err(RouterError::NoModelsRegistered);
    }

    // MoE 稀疏门控:粗筛 Top-K(大规模)或退化全量(小规模)
    // gated: 退化模式返回全部引用(原顺序),门控模式返回 Top-K 引用(评分降序)
    let gated: Vec<&ModelInfo> = gate.gate(&models, history);

    // 完整评估:仅对门控候选做归一化评分
    // 退化模式下 gated = 全部模型,max 与历史全量评估一致
    let max_cost = gated
        .iter()
        .map(|m| m.cost_per_1k_tokens)
        .fold(0.0_f64, f64::max);
    let max_latency = gated
        .iter()
        .map(|m| m.avg_latency_ms as f64)
        .fold(0.0_f64, f64::max);

    // 计算每个候选模型的综合评分
    let mut scored: Vec<(f64, &ModelInfo)> = gated
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
            (score, *m)
        })
        .collect();

    // 评分降序比较器:评分高者优先,相同则 model_id 升序(保证确定性)
    // WHY 嵌套 fn:item 天生 Copy,可同时传递给 select_nth_unstable_by 和 sort_by,
    // 避免闭包 move 后无法复用的问题。
    fn cmp_score_desc(a: &(f64, &ModelInfo), b: &(f64, &ModelInfo)) -> std::cmp::Ordering {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| a.1.model_id.cmp(&b.1.model_id))
    }

    // O(n) 部分排序选出最佳模型(index 0),替代全排序 O(n log n)
    // WHY select_nth_unstable_by:符合 §4.1 Engineering Convention,
    // Top-1 选择用 O(n) 而非 O(n log n) 全排序。partition 后 [0] 为最佳,
    // [1..] 无序,需单独排序以满足候选列表有序契约。
    if scored.len() > 1 {
        scored.select_nth_unstable_by(1, cmp_score_desc);
    }

    let selected = scored[0].1;
    let estimated_cost = estimate_cost(req.estimated_tokens, selected.cost_per_1k_tokens);

    // 候选列表需按策略优先级降序(RoutingDecision.candidates 文档契约:
    // types.rs "按策略优先级降序排序"),对 [1..] 排序保证有序。
    // 复杂度 O((k-1) log (k-1))(门控模式)或 O((n-1) log (n-1))(退化模式)。
    scored[1..].sort_by(cmp_score_desc);
    let candidate_ids: Vec<String> = scored[1..]
        .iter()
        .map(|(_, m)| m.model_id.clone())
        .collect();

    Ok(RoutingDecision {
        model_id: selected.model_id.clone(),
        route_reason: format!(
            "Auto: selected {} (score {:.4}, cost_score {:.3}, latency_score {:.3}, quality {:.2})",
            selected.model_id,
            scored[0].0,
            if max_cost > 0.0 {
                1.0 - (selected.cost_per_1k_tokens / max_cost)
            } else {
                1.0
            },
            if max_latency > 0.0 {
                1.0 - (selected.avg_latency_ms as f64 / max_latency)
            } else {
                1.0
            },
            selected.quality_score
        ),
        estimated_cost,
        candidates: candidate_ids,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RouterConfig;
    use crate::types::RoutingStrategy;
    use nexus_core::{MultimodalInput, UserIntent};

    fn make_intent() -> UserIntent {
        UserIntent {
            intent_id: "i-1".into(),
            raw_text: "test".into(),
            multimodal_inputs: vec![MultimodalInput::Text("test".into())],
            risk_level: 10,
        }
    }

    fn make_request(strategy: RoutingStrategy, tokens: u32) -> RoutingRequest {
        RoutingRequest {
            quest_id: "q-1".into(),
            intent: make_intent(),
            estimated_tokens: tokens,
            strategy,
        }
    }

    fn make_registry() -> ModelRegistry {
        ModelRegistry::from_config(&RouterConfig::default())
    }

    #[test]
    fn test_estimate_cost() {
        // 1000 tokens * $0.001/1k * 100 = 0.1 美分
        assert_eq!(estimate_cost(1000, 0.001), 0);
        // 10000 tokens * $0.001/1k * 100 = 1 美分
        assert_eq!(estimate_cost(10000, 0.001), 1);
        // 1000 tokens * $0.015/1k * 100 = 1.5 美分 -> round to 2
        assert_eq!(estimate_cost(1000, 0.015), 2);
    }

    #[test]
    fn test_route_lite_selects_cheapest() {
        let registry = make_registry();
        let req = make_request(RoutingStrategy::Lite, 1000);
        let decision = route_lite(&registry, &req).unwrap();
        assert_eq!(decision.model_id, "lite-model");
        assert!(decision.route_reason.contains("Lite"));
        assert_eq!(decision.candidates.len(), 2);
    }

    #[test]
    fn test_route_efficient_selects_lowest_latency() {
        let registry = make_registry();
        let req = make_request(RoutingStrategy::Efficient, 1000);
        let decision = route_efficient(&registry, &req).unwrap();
        // lite-model 延迟 100ms 最低
        assert_eq!(decision.model_id, "lite-model");
        assert!(decision.route_reason.contains("Efficient"));
    }

    #[test]
    fn test_route_auto_returns_valid_decision() {
        let registry = make_registry();
        let req = make_request(RoutingStrategy::Auto, 1000);
        let decision = route_auto(&registry, &req).unwrap();
        // Auto 策略应选择 lite-model 或 efficient-model(综合评分最高)
        assert!(
            decision.model_id == "lite-model" || decision.model_id == "efficient-model",
            "Auto should pick lite or efficient, got {}",
            decision.model_id
        );
        assert!(decision.route_reason.contains("Auto"));
        assert_eq!(decision.candidates.len(), 2);
    }

    #[test]
    fn test_route_empty_registry() {
        let registry = ModelRegistry::new();
        let req = make_request(RoutingStrategy::Lite, 1000);
        let result = route_lite(&registry, &req);
        assert!(matches!(result, Err(RouterError::NoModelsRegistered)));
    }

    #[test]
    fn test_route_efficient_empty_registry() {
        let registry = ModelRegistry::new();
        let req = make_request(RoutingStrategy::Efficient, 1000);
        let result = route_efficient(&registry, &req);
        assert!(matches!(result, Err(RouterError::NoModelsRegistered)));
    }

    #[test]
    fn test_route_auto_empty_registry() {
        let registry = ModelRegistry::new();
        let req = make_request(RoutingStrategy::Auto, 1000);
        let result = route_auto(&registry, &req);
        assert!(matches!(result, Err(RouterError::NoModelsRegistered)));
    }

    #[test]
    fn test_route_auto_single_model() {
        let registry = ModelRegistry::new();
        registry
            .register(crate::types::ModelInfo {
                model_id: "only-model".into(),
                provider: "test".into(),
                cost_per_1k_tokens: 0.001,
                avg_latency_ms: 100,
                max_context: 8192,
                quality_score: 0.8,
            })
            .unwrap();
        let req = make_request(RoutingStrategy::Auto, 1000);
        let decision = route_auto(&registry, &req).unwrap();
        assert_eq!(decision.model_id, "only-model");
        assert!(decision.candidates.is_empty());
    }
}
