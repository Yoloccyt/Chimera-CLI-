//! SSRA 主导策略回归 proptest
//!
//! 回归目标:验证 `fuse_inner` 在 `select_top_k_desc` 后,通过显式 max_by
//! 挑选的始终是 Top-K 中权重最大的模板策略,而不是误用 `selected[0]`。

#![forbid(unsafe_code)]

use proptest::prelude::*;
use ssra_fusion::{FusionRequest, FusionStrategy, SlimeFusionEngine, SlimeTemplate, SsraConfig};

/// 将任意可显示错误转换为 TestCaseError
fn fail<E: std::fmt::Display>(e: E) -> TestCaseError {
    TestCaseError::fail(format!("{e}"))
}

/// 根据主导策略计算期望置信度(与 engine.rs 的 compute_confidence 对齐)
fn compute_expected_confidence(top: &[(f32, FusionStrategy)]) -> f32 {
    if top.is_empty() {
        return 0.0;
    }
    let k = top.len() as f32;
    // 先选出主导策略:Top-K 中权重最大者对应的策略
    let dominant = top
        .iter()
        .max_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal))
        .map(|(_, s)| *s)
        .unwrap_or(FusionStrategy::TopK);

    match dominant {
        FusionStrategy::WeightedAverage => {
            let sum_w: f32 = top.iter().map(|(w, _)| *w).sum();
            let sum_w2: f32 = top.iter().map(|(w, _)| w * w).sum();
            if sum_w > 0.0 {
                (sum_w2 / sum_w).clamp(0.0, 1.0)
            } else {
                0.0
            }
        }
        FusionStrategy::TopK => top
            .iter()
            .map(|(w, _)| *w)
            .fold(0.0_f32, f32::max)
            .clamp(0.0, 1.0),
        FusionStrategy::MeanField => {
            let sum: f32 = top.iter().map(|(w, _)| *w).sum();
            (sum / k).clamp(0.0, 1.0)
        }
    }
}

/// 将策略索引映射为 FusionStrategy
fn strategy_from_index(idx: usize) -> FusionStrategy {
    match idx {
        0 => FusionStrategy::WeightedAverage,
        1 => FusionStrategy::TopK,
        _ => FusionStrategy::MeanField,
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// 主导策略恒为最大权重策略
    ///
    /// 随机生成非空模板集合与 top_k,通过公共 `fuse` API 触发完整融合流程,
    /// 并独立计算期望置信度。若代码退化为使用 `selected[0]` 作为主导策略,
    /// 此属性测试将以高概率失败。
    #[test]
    fn prop_main_strategy_always_max(
        weights in prop::collection::vec(0.0f32..1.0, 1..30),
        strategy_indices in prop::collection::vec(0usize..3, 1..30),
        top_k in 1usize..30,
    ) {
        let rt = tokio::runtime::Runtime::new().map_err(fail)?;
        rt.block_on(async {
            let count = weights.len().min(strategy_indices.len());
            let weights = &weights[..count];
            let strategy_indices = &strategy_indices[..count];
            let top_k = top_k.min(count).max(1);

            let engine = SlimeFusionEngine::new(SsraConfig::default());
            let mut source_adapters = Vec::with_capacity(count);

            for i in 0..count {
                let strategy = strategy_from_index(strategy_indices[i]);
                let id = format!("cap-{i}");
                let template = SlimeTemplate::new(&id, vec!["x".into()], strategy)
                    .with_weight(weights[i]);
                engine.registry().register(template).map_err(fail)?;
                source_adapters.push(id);
            }

            let request = FusionRequest::new("q-prop", source_adapters, "target", 20, top_k);
            let result = engine.fuse(request).await.map_err(fail)?;

            // 独立计算:按权重降序取 top_k,再按同样语义取主导策略与置信度
            let mut indexed: Vec<(usize, f32)> =
                weights.iter().copied().enumerate().collect();
            indexed.sort_by(|a, b| {
                b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
            });
            let top: Vec<(f32, FusionStrategy)> = indexed[..top_k]
                .iter()
                .map(|(idx, w)| (*w, strategy_from_index(strategy_indices[*idx])))
                .collect();

            let expected = compute_expected_confidence(&top);

            prop_assert!(
                (result.confidence - expected).abs() < 1e-4,
                "主导策略应为最大权重策略: confidence={}, expected={}, top_k={}, count={}",
                result.confidence, expected, top_k, count
            );

            Ok::<(), TestCaseError>(())
        })?;
    }
}
