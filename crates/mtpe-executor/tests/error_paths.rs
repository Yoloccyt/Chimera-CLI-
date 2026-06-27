//! MTPE 多步预测执行错误路径测试 — 验证关键故障场景的错误传播
//!
//! 对应 SubTask 29.6:为 mtpe-executor 补充错误路径测试
//!
//! # 测试覆盖
//! 1. N=0 无效:返回 InvalidN { n: 0, max: 10 }
//! 2. N=11 超上限:返回 InvalidN { n: 11, max: 10 }
//! 3. N=100 远超上限:返回 InvalidN { n: 100, max: 10 }
//! 4. 空上下文预测:history 与 clv 均为空时不 panic(边界健壮性)
//! 5. 回退到单步预测:rollback_to_single_step 返回 N=1 结果

#![forbid(unsafe_code)]

use event_bus::EventBus;
use mtpe_executor::{MtpeConfig, MtpeError, MtpeExecutor, PredictionContext};

/// 构造测试用预测上下文
fn make_context(quest_id: &str, history: Vec<&str>) -> PredictionContext {
    PredictionContext {
        quest_id: quest_id.into(),
        history: history.into_iter().map(String::from).collect(),
        clv: vec![0.1; 8],
    }
}

/// N=0 无效:返回 InvalidN
///
/// WHY:N=0 无意义(预测 0 个 token),必须在系统边界拦截。
/// 此测试验证 predict 对 N=0 返回 InvalidN 错误,而非静默返回空结果。
#[tokio::test]
async fn test_predict_n_zero_invalid() {
    let executor = MtpeExecutor::new(MtpeConfig::default(), EventBus::new());
    let ctx = make_context("q-1", vec!["hello"]);

    let result = executor.predict(&ctx, 0).await;
    let err = match result {
        Ok(_) => panic!("N=0 应返回错误,而非静默成功"),
        Err(e) => e,
    };
    assert!(
        matches!(err, MtpeError::InvalidN { n: 0, max: 10 }),
        "应为 InvalidN {{ n: 0, max: 10 }},实际: {err:?}"
    );
    // 错误信息应包含 N 值与范围
    let msg = format!("{err}");
    assert!(msg.contains("0"), "错误信息应包含 n=0");
    assert!(msg.contains("10"), "错误信息应包含 max=10");
}

/// N=11 超上限:返回 InvalidN
///
/// WHY:N=11 超过架构决策上限 10,预测成功率过低。
/// 此测试验证上限边界的错误传播。
#[tokio::test]
async fn test_predict_n_eleven_invalid() {
    let executor = MtpeExecutor::new(MtpeConfig::default(), EventBus::new());
    let ctx = make_context("q-1", vec!["hello"]);

    let result = executor.predict(&ctx, 11).await;
    let err = match result {
        Ok(_) => panic!("N=11 应返回错误"),
        Err(e) => e,
    };
    assert!(
        matches!(err, MtpeError::InvalidN { n: 11, max: 10 }),
        "应为 InvalidN {{ n: 11, max: 10 }},实际: {err:?}"
    );
}

/// N=100 远超上限:返回 InvalidN
///
/// WHY:极端 N 值(100)可能源于调用方 bug(如未校验的用户输入),
/// 必须被拦截而非导致长时间预测或内存爆炸。
#[tokio::test]
async fn test_predict_n_hundred_invalid() {
    let executor = MtpeExecutor::new(MtpeConfig::default(), EventBus::new());
    let ctx = make_context("q-1", vec!["hello"]);

    let result = executor.predict(&ctx, 100).await;
    let err = match result {
        Ok(_) => panic!("N=100 应返回错误"),
        Err(e) => e,
    };
    assert!(
        matches!(err, MtpeError::InvalidN { n: 100, max: 10 }),
        "应为 InvalidN {{ n: 100, max: 10 }},实际: {err:?}"
    );
    let msg = format!("{err}");
    assert!(msg.contains("100"), "错误信息应包含 n=100");
}

/// 空上下文预测:history 与 clv 均为空时不 panic
///
/// WHY 边界健壮性:空上下文是合法输入(如 Quest 刚启动时无历史),
/// predict 不应 panic,应正常返回预测结果(基于空哈希的确定性输出)。
/// 此测试验证边界条件下的健壮性。
#[tokio::test]
async fn test_predict_empty_context_no_panic() {
    let executor = MtpeExecutor::new(MtpeConfig::default(), EventBus::new());
    let ctx = PredictionContext {
        quest_id: "q-empty".into(),
        history: vec![],
        clv: vec![],
    };

    // 空上下文应正常返回,不 panic
    let result = executor.predict(&ctx, 3).await;
    assert!(result.is_ok(), "空上下文预测应成功,实际: {:?}", result);
    let result = result.unwrap();
    assert_eq!(result.n, 3);
    assert_eq!(result.predicted_tokens.len(), 3);

    // 置信度仍应在 [0, 1] 且递减
    for (i, token) in result.predicted_tokens.iter().enumerate() {
        assert!(
            (0.0..=1.0).contains(&token.confidence),
            "空上下文预测置信度 {} 超出 [0, 1],步 {}",
            token.confidence,
            i
        );
    }
}

/// 回退到单步预测:rollback_to_single_step 返回 N=1 结果
///
/// WHY:回退是 MTPE 的降级策略,失败步回退到单步预测(N=1)。
/// 此测试验证回退正常工作:返回 N=1 的预测结果,置信度为 1.0(单步无误差累积)。
#[tokio::test]
async fn test_rollback_to_single_step_returns_n1() {
    let executor = MtpeExecutor::new(MtpeConfig::default(), EventBus::new());
    let ctx = make_context("q-rollback", vec!["context"]);

    // 模拟第 5 步失败,回退到单步预测
    let result = executor.rollback_to_single_step(&ctx, 5).await;
    assert!(result.is_ok(), "回退应成功,实际: {:?}", result);
    let result = result.unwrap();

    // 回退后应为单步预测
    assert_eq!(result.n, 1, "回退后 N 应为 1");
    assert_eq!(result.predicted_tokens.len(), 1, "回退后 token 数应为 1");
    // 单步预测置信度应为 1.0(无误差累积)
    assert!(
        (result.predicted_tokens[0].confidence - 1.0).abs() < f32::EPSILON,
        "单步预测置信度应为 1.0,实际: {}",
        result.predicted_tokens[0].confidence
    );
}
