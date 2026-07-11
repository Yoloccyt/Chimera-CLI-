//! 在线学习框架属性测试 — 验证 ParameterRegistry 注册/注销不变量
//!
//! 对应 §3.3.2 新 crate 准入 checklist:proptest 属性测试覆盖
//!
//! # 验证的不变量
//! 1. register 成功后 get_value 返回注册时的值
//! 2. unregister 后 get_value 返回 Err(ParameterNotFound)
//! 3. 重复 register 同名参数返回 Err,且原值保持不变(幂等性)
//! 4. count 随 register/unregister 单调变化
//! 5. update_with_feedback 按 GradientDescent 公式更新(new = old + lr * reward)
//! 6. JSON roundtrip 保留参数标量值
//!
//! # 同步性说明
//! ParameterRegistry 所有方法均为同步(DashMap 内部锁无 await),
//! 因此 proptest 无需 tokio runtime 包裹。

#![forbid(unsafe_code)]

use online_learning::{FeedbackSignal, LearnableParameter, ParameterRegistry, ParameterValue};
use proptest::prelude::*;

/// 将任意可显示错误转换为 TestCaseError(避免 unwrap,用 ? 传播)
fn fail<E: std::fmt::Display>(e: E) -> TestCaseError {
    TestCaseError::fail(format!("{e}"))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// 不变量 1:register 成功后 get_value 返回注册时的标量值
    #[test]
    fn register_then_get_returns_value(
        id_seed in 0u32..100_000,
        value in -1000.0f32..=1000.0f32,
    ) {
        let registry = ParameterRegistry::new();
        let id = format!("param-{id_seed}");
        let param = LearnableParameter::new(
            id.clone(),
            format!("name-{id_seed}"),
            "test-crate",
            ParameterValue::scalar(value),
        );
        registry.register(param).map_err(fail)?;

        let retrieved = registry.get_value(&id).map_err(fail)?;
        prop_assert_eq!(retrieved.as_scalar(), Some(value));
    }

    /// 不变量 2:unregister 后 get_value 返回 Err
    #[test]
    fn unregister_makes_get_fail(
        id_seed in 0u32..100_000,
        value in -1000.0f32..=1000.0f32,
    ) {
        let registry = ParameterRegistry::new();
        let id = format!("param-{id_seed}");
        let param = LearnableParameter::new(
            id.clone(),
            format!("name-{id_seed}"),
            "test-crate",
            ParameterValue::scalar(value),
        );
        registry.register(param).map_err(fail)?;
        registry.unregister(&id).map_err(fail)?;

        let result = registry.get_value(&id);
        prop_assert!(result.is_err(), "get_value after unregister must fail");
    }

    /// 不变量 3:重复 register 同名参数返回 Err,且原值保持不变
    ///
    /// "幂等性"含义:多次 register 同名参数行为一致(第二次起返回错误),
    /// 且首次注册的值不受后续 register 影响。
    #[test]
    fn duplicate_register_returns_error_and_preserves_original(
        id_seed in 0u32..100_000,
        v1 in -1000.0f32..=1000.0f32,
        v2 in -1000.0f32..=1000.0f32,
    ) {
        let registry = ParameterRegistry::new();
        let id = format!("param-{id_seed}");

        let p1 = LearnableParameter::new(
            id.clone(),
            "name",
            "test-crate",
            ParameterValue::scalar(v1),
        );
        registry.register(p1).map_err(fail)?;

        // 第二次 register 同名参数,应失败
        let p2 = LearnableParameter::new(
            id.clone(),
            "name",
            "test-crate",
            ParameterValue::scalar(v2),
        );
        let dup_result = registry.register(p2);
        prop_assert!(dup_result.is_err(), "duplicate register must fail");

        // 原值应保持 v1 不变
        let retrieved = registry.get_value(&id).map_err(fail)?;
        prop_assert_eq!(retrieved.as_scalar(), Some(v1));
    }

    /// 不变量 4:count 随 register 单调递增,随 unregister 单调递减
    #[test]
    fn count_tracks_register_and_unregister(
        n in 1u32..100,
    ) {
        let registry = ParameterRegistry::new();
        for i in 0..n {
            let id = format!("p-{i}");
            let param = LearnableParameter::new(
                id,
                "name",
                "test-crate",
                ParameterValue::scalar(0.5),
            );
            registry.register(param).map_err(fail)?;
            prop_assert_eq!(registry.count(), (i + 1) as usize);
        }
        for i in 0..n {
            let id = format!("p-{i}");
            registry.unregister(&id).map_err(fail)?;
            prop_assert_eq!(registry.count(), (n - i - 1) as usize);
        }
    }

    /// 不变量 5:update_with_feedback 按 GradientDescent 公式更新
    ///
    /// 公式:new = old + learning_rate * reward
    /// Success 的 reward = 1.0,验证值增加 learning_rate
    #[test]
    fn update_with_feedback_applies_gradient_descent(
        initial in -100.0f32..=100.0f32,
        lr in 0.001f32..=1.0f32,
    ) {
        let registry = ParameterRegistry::new();
        let param = LearnableParameter::new(
            "p1",
            "name",
            "test-crate",
            ParameterValue::scalar(initial),
        )
        .with_learning_rate(lr);
        registry.register(param).map_err(fail)?;

        let new_val = registry
            .update_with_feedback("p1", FeedbackSignal::Success)
            .map_err(fail)?;
        let expected = initial + lr * 1.0;
        let actual = new_val.as_scalar().unwrap();
        prop_assert!(
            (actual - expected).abs() < 1e-4,
            "expected {} got {}", expected, actual
        );
    }

    /// 不变量 6:JSON roundtrip 保留参数标量值
    ///
    /// WHY 用 epsilon 比较:serde_json 内部用 f64 中转,
    /// f32 → f64 → f32 理论无损,但用 epsilon 比较更鲁棒。
    #[test]
    fn json_roundtrip_preserves_scalar_value(
        value in -1000.0f32..=1000.0f32,
    ) {
        let registry = ParameterRegistry::new();
        let param = LearnableParameter::new(
            "p1",
            "name",
            "test-crate",
            ParameterValue::scalar(value),
        );
        registry.register(param).map_err(fail)?;

        let json = registry.to_json().map_err(fail)?;
        let registry2 = ParameterRegistry::new();
        registry2.from_json(&json).map_err(fail)?;

        let retrieved = registry2.get_value("p1").map_err(fail)?;
        let retrieved_scalar = retrieved.as_scalar().unwrap();
        prop_assert!(
            (retrieved_scalar - value).abs() < 1e-5,
            "expected {} got {}", value, retrieved_scalar
        );
    }
}
