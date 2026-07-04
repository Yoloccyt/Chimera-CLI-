//! SSRA 融合引擎属性测试 — 验证融合不变量
//!
//! 对应 SubTask 1.6:proptest 属性测试
//!
//! # 验证的不变量
//! 1. 融合结果 confidence ∈ [0.0, 1.0]
//! 2. Top-K 数量 ≤ request.top_k(且 ≤ 源模板数)
//! 3. 融合延迟 ≤ deadline_ms(允许边界)
//!
//! # 策略
//! - 生成合法的 template_count ∈ [1, 50]
//! - 生成合法的 top_k ∈ [1, 20]
//! - 使用 tokio::runtime::Runtime 在 proptest 中执行 async 代码
//!   (WHY:proptest! 宏不兼容 #[tokio::test],需手动创建 runtime)

#![forbid(unsafe_code)]

use proptest::prelude::*;
use ssra_fusion::{FusionRequest, FusionStrategy, SlimeFusionEngine, SlimeTemplate, SsraConfig};

/// 将任意可显示错误转换为 TestCaseError
fn fail<E: std::fmt::Display>(e: E) -> TestCaseError {
    TestCaseError::fail(format!("{e}"))
}

/// 构建带 N 个模板的引擎(权重随机但 ∈ [0, 1])
fn make_engine(template_count: usize) -> SlimeFusionEngine {
    let config = SsraConfig::default();
    let engine = SlimeFusionEngine::new(config);
    for i in 0..template_count {
        let id = format!("cap-{i}");
        // 权重均匀分布在 [0.1, 1.0],避免全零导致除零
        let weight = 0.1 + (i as f32 * 0.03).rem_euclid(0.9);
        let strategy = match i % 3 {
            0 => FusionStrategy::WeightedAverage,
            1 => FusionStrategy::TopK,
            _ => FusionStrategy::MeanField,
        };
        let template = SlimeTemplate::new(id, vec!["x".into()], strategy).with_weight(weight);
        let _ = engine.registry().register(template);
    }
    engine
}

/// 构建融合请求,源适配器为 cap-0..cap-(template_count-1)
fn make_request(template_count: usize, top_k: usize) -> FusionRequest {
    let sources: Vec<String> = (0..template_count).map(|i| format!("cap-{i}")).collect();
    FusionRequest::new("q-prop", sources, "target", 20, top_k)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// 不变量 1:融合结果 confidence ∈ [0.0, 1.0]
    ///
    /// 无论模板数量、Top-K 值如何变化,置信度始终在单位区间内。
    #[test]
    fn test_confidence_in_unit_range(
        template_count in 1usize..50,
        top_k in 1usize..20,
    ) {
        let rt = tokio::runtime::Runtime::new().map_err(fail)?;
        rt.block_on(async {
            let engine = make_engine(template_count);
            let request = make_request(template_count, top_k);
            let result = engine.fuse(request).await.map_err(fail)?;

            prop_assert!(
                result.confidence >= 0.0 && result.confidence <= 1.0,
                "confidence 应在 [0, 1], got {}",
                result.confidence
            );

            Ok::<(), TestCaseError>(())
        })?;
    }

    /// 不变量 2:Top-K 数量 ≤ request.top_k(且 ≤ 源模板数)
    ///
    /// selected_count 不能超过请求的 top_k,也不能超过实际源模板数量。
    #[test]
    fn test_selected_count_le_top_k(
        template_count in 1usize..50,
        top_k in 1usize..20,
    ) {
        let rt = tokio::runtime::Runtime::new().map_err(fail)?;
        rt.block_on(async {
            let engine = make_engine(template_count);
            let request = make_request(template_count, top_k);
            let result = engine.fuse(request).await.map_err(fail)?;

            let expected_max = top_k.min(template_count);
            prop_assert!(
                result.selected_count <= expected_max,
                "selected_count({}) 应 ≤ min(top_k={}, template_count={})",
                result.selected_count, top_k, template_count
            );
            prop_assert!(result.selected_count >= 1, "至少选中 1 个模板");

            Ok::<(), TestCaseError>(())
        })?;
    }

    /// 不变量 3:融合延迟 ≤ deadline_ms(允许边界)
    ///
    /// 纯内存融合操作应在 deadline(20ms)内完成。
    /// 允许 latency_ms == deadline_ms(边界情况)。
    #[test]
    fn test_latency_within_deadline(
        template_count in 1usize..100,
    ) {
        let rt = tokio::runtime::Runtime::new().map_err(fail)?;
        rt.block_on(async {
            let engine = make_engine(template_count);
            // deadline_ms = 20,与默认配置一致
            let request = make_request(template_count, 8);
            let result = engine.fuse(request).await.map_err(fail)?;

            prop_assert!(
                result.latency_ms <= 20,
                "延迟({}ms)应 ≤ deadline(20ms),模板数={}",
                result.latency_ms, template_count
            );

            Ok::<(), TestCaseError>(())
        })?;
    }

    /// 不变量 4:fused_template_id 始终非空(UUIDv7 字符串)
    #[test]
    fn test_fused_template_id_nonempty(
        template_count in 1usize..30,
        top_k in 1usize..10,
    ) {
        let rt = tokio::runtime::Runtime::new().map_err(fail)?;
        rt.block_on(async {
            let engine = make_engine(template_count);
            let request = make_request(template_count, top_k);
            let result = engine.fuse(request).await.map_err(fail)?;

            prop_assert!(
                !result.fused_template_id.is_empty(),
                "fused_template_id 不应为空"
            );

            Ok::<(), TestCaseError>(())
        })?;
    }

    /// 不变量 5:相同输入的 selected_count 确定性(不依赖随机)
    ///
    /// 同样的模板集合与 top_k,selected_count 应一致。
    #[test]
    fn test_selected_count_deterministic(
        template_count in 1usize..30,
        top_k in 1usize..10,
    ) {
        let rt = tokio::runtime::Runtime::new().map_err(fail)?;
        rt.block_on(async {
            let engine1 = make_engine(template_count);
            let engine2 = make_engine(template_count);
            let req1 = make_request(template_count, top_k);
            let req2 = make_request(template_count, top_k);

            let result1 = engine1.fuse(req1).await.map_err(fail)?;
            let result2 = engine2.fuse(req2).await.map_err(fail)?;

            prop_assert_eq!(
                result1.selected_count, result2.selected_count,
                "相同输入应产生相同 selected_count"
            );

            Ok::<(), TestCaseError>(())
        })?;
    }
}
