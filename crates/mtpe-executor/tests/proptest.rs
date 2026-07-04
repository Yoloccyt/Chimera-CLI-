//! MTPE 多步预测执行属性测试 — 验证预测 Token 数与回退不变量
//!
//! 对应 SubTask 29.6:为 mtpe-executor 补充 proptest
//!
//! # 验证的不变量
//! 1. 预测 Token 数 = N:predict(ctx, N) 返回的 predicted_tokens.len() == N
//! 2. 回退后 N = 1:rollback_to_single_step 返回的 n == 1
//! 3. 置信度 ∈ [0.0, 1.0] 且随步数递减
//! 4. 相同上下文产生相同预测(确定性)
//! 5. 延迟非负
//!
//! # 策略
//! - 生成合法的 N ∈ [1, 10]
//! - 生成合法的 failed_step ∈ [0, 9]
//! - 使用 tokio::runtime::Runtime 在 proptest 中执行 async 代码

#![forbid(unsafe_code)]

use event_bus::EventBus;
use mtpe_executor::{MtpeConfig, MtpeExecutor, PredictionContext};
use proptest::prelude::*;

/// 将任意可显示错误转换为 TestCaseError(避免 unwrap,用 ? 传播)
fn fail<E: std::fmt::Display>(e: E) -> TestCaseError {
    TestCaseError::fail(format!("{e}"))
}

/// 构造测试用预测上下文
fn make_context(quest_id: &str, history: Vec<&str>) -> PredictionContext {
    PredictionContext {
        quest_id: quest_id.into(),
        history: history.into_iter().map(String::from).collect(),
        clv: vec![0.1; 8],
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// 不变量 1:预测 Token 数 = N
    ///
    /// 任意合法 N ∈ [1, 10],predict 返回的 predicted_tokens.len() == N
    /// WHY Token 数一致:MTPE 的核心契约是一次推理产出 N 个 token,
    /// 数量不一致会导致下游消费者(PVL)验证逻辑混乱
    #[test]
    fn test_predict_token_count_equals_n(
        n in 1usize..=10,
    ) {
        let rt = tokio::runtime::Runtime::new().map_err(fail)?;
        rt.block_on(async {
            let executor = MtpeExecutor::new(MtpeConfig::default(), EventBus::new());
            let ctx = make_context("quest-prop", vec!["hello"]);

            let result = executor.predict(&ctx, n).await.map_err(fail)?;

            prop_assert_eq!(
                result.predicted_tokens.len(),
                n,
                "预测 token 数应等于 N"
            );
            prop_assert_eq!(result.n, n, "result.n 应等于输入 N");
            Ok::<(), TestCaseError>(())
        })?;
    }

    /// 不变量 2:回退后 N = 1
    ///
    /// 任意 failed_step ∈ [0, 9],rollback_to_single_step 返回的 n == 1
    /// WHY 回退到单步:回退是降级策略,目标 N 固定为 1,
    /// 确保失败后能用最可靠的单步预测恢复
    #[test]
    fn test_rollback_always_returns_single_step(
        failed_step in 0usize..=9,
    ) {
        let rt = tokio::runtime::Runtime::new().map_err(fail)?;
        rt.block_on(async {
            let executor = MtpeExecutor::new(MtpeConfig::default(), EventBus::new());
            let ctx = make_context("quest-rollback", vec!["context"]);

            let result = executor.rollback_to_single_step(&ctx, failed_step).await.map_err(fail)?;

            prop_assert_eq!(result.n, 1, "回退后 N 应为 1");
            prop_assert_eq!(result.predicted_tokens.len(), 1, "回退后 token 数应为 1");
            Ok::<(), TestCaseError>(())
        })?;
    }

    /// 不变量 3:置信度 ∈ [0.0, 1.0] 且随步数递减
    ///
    /// WHY 置信度递减:多步预测存在误差累积,后续 token 置信度自然降低,
    /// 此模型与真实 LLM 预测的行为特征一致。
    /// 公式:confidence[i] = 1.0 - i * 0.05,clamp 到 [0, 1]
    #[test]
    fn test_confidence_in_unit_interval_and_decreasing(
        n in 1usize..=10,
    ) {
        let rt = tokio::runtime::Runtime::new().map_err(fail)?;
        rt.block_on(async {
            let executor = MtpeExecutor::new(MtpeConfig::default(), EventBus::new());
            let ctx = make_context("quest-conf", vec!["hello"]);

            let result = executor.predict(&ctx, n).await.map_err(fail)?;

            for (i, token) in result.predicted_tokens.iter().enumerate() {
                // 置信度必须在 [0, 1] 区间
                prop_assert!(
                    token.confidence.is_finite(),
                    "置信度必须为有限值,步 {} 实际: {}",
                    i,
                    token.confidence
                );
                prop_assert!(
                    (0.0..=1.0).contains(&token.confidence),
                    "置信度 {} 超出 [0, 1] 区间,步 {}",
                    token.confidence,
                    i
                );

                // 验证递减性(非首步)
                if i > 0 {
                    let prev = result.predicted_tokens[i - 1].confidence;
                    prop_assert!(
                        token.confidence <= prev,
                        "置信度应递减,步 {} 的 {} > 步 {} 的 {}",
                        i, token.confidence, i - 1, prev
                    );
                }
            }
            Ok::<(), TestCaseError>(())
        })?;
    }

    /// 不变量 4:相同上下文产生相同预测(确定性)
    ///
    /// WHY 确定性:伪预测基于上下文哈希,相同上下文必须产生相同输出,
    /// 确保测试可复现且预测结果可缓存
    #[test]
    fn test_predict_deterministic_same_context(
        n in 1usize..=10,
    ) {
        let rt = tokio::runtime::Runtime::new().map_err(fail)?;
        rt.block_on(async {
            let executor = MtpeExecutor::new(MtpeConfig::default(), EventBus::new());
            let ctx = make_context("quest-det", vec!["deterministic"]);

            let r1 = executor.predict(&ctx, n).await.map_err(fail)?;
            let r2 = executor.predict(&ctx, n).await.map_err(fail)?;

            prop_assert_eq!(
                r1.predicted_tokens,
                r2.predicted_tokens,
                "相同上下文应产生相同预测"
            );
            Ok::<(), TestCaseError>(())
        })?;
    }

    /// 不变量 5:延迟非负
    ///
    /// WHY 延迟非负:延迟用于性能监控与加速比验证,
    /// 负延迟会导致加速比计算错误
    #[test]
    fn test_latency_non_negative(
        n in 1usize..=10,
    ) {
        let rt = tokio::runtime::Runtime::new().map_err(fail)?;
        rt.block_on(async {
            let executor = MtpeExecutor::new(MtpeConfig::default(), EventBus::new());
            let ctx = make_context("quest-latency", vec!["hello"]);

            let result = executor.predict(&ctx, n).await.map_err(fail)?;

            prop_assert!(
                result.latency_ms >= 0.0,
                "延迟应非负,实际: {}",
                result.latency_ms
            );
            prop_assert!(
                result.latency_ms.is_finite(),
                "延迟必须为有限值"
            );
            Ok::<(), TestCaseError>(())
        })?;
    }

    /// 不变量 6:不同上下文产生不同预测(可区分性)
    ///
    /// WHY 可区分性:不同上下文应产生不同预测,否则预测无意义。
    /// 使用不同 quest_id 确保哈希不同
    #[test]
    fn test_predict_different_context_different_output(
        n in 1usize..=10,
        quest_id_a in any::<String>(),
        quest_id_b in any::<String>(),
    ) {
        // 跳过相同 quest_id 的情况
        if quest_id_a == quest_id_b {
            return Ok(());
        }

        let rt = tokio::runtime::Runtime::new().map_err(fail)?;
        rt.block_on(async {
            let executor = MtpeExecutor::new(MtpeConfig::default(), EventBus::new());
            let ctx_a = make_context(&quest_id_a, vec!["hello"]);
            let ctx_b = make_context(&quest_id_b, vec!["hello"]);

            let r_a = executor.predict(&ctx_a, n).await.map_err(fail)?;
            let r_b = executor.predict(&ctx_b, n).await.map_err(fail)?;

            prop_assert_ne!(
                r_a.predicted_tokens,
                r_b.predicted_tokens,
                "不同上下文(quest_id={:?} vs {:?})应产生不同预测",
                quest_id_a,
                quest_id_b
            );
            Ok::<(), TestCaseError>(())
        })?;
    }

    /// 不变量 7:is_valid_n 与 predict 的一致性
    ///
    /// WHY 一致性:is_valid_n 是 predict 的前置校验,
    /// 两者对 N 的合法性判断必须一致,否则调用方无法可靠预判
    #[test]
    fn test_is_valid_n_consistent_with_predict(
        n in 0usize..=15,
    ) {
        let config = MtpeConfig::default();
        let is_valid = config.is_valid_n(n);

        let rt = tokio::runtime::Runtime::new().map_err(fail)?;
        rt.block_on(async {
            let executor = MtpeExecutor::new(config, EventBus::new());
            let ctx = make_context("quest-valid", vec!["hello"]);

            let result = executor.predict(&ctx, n).await;

            if is_valid {
                prop_assert!(
                    result.is_ok(),
                    "is_valid_n({})=true 但 predict 返回错误: {:?}",
                    n,
                    result
                );
            } else {
                prop_assert!(
                    result.is_err(),
                    "is_valid_n({})=false 但 predict 返回成功",
                    n
                );
            }
            Ok::<(), TestCaseError>(())
        })?;
    }
}
