//! GQEP 聚集查询执行协议属性测试 — 验证聚集结果不变量
//!
//! 对应 SubTask 29.6:为 gqep-executor 补充 proptest
//!
//! # 验证的不变量
//! 1. 聚集 succeeded + failed = total:任意操作数与成功/失败组合,
//!    `gather` 返回的 `succeeded + failed == total`
//! 2. 超时操作计入 total:超时的操作也计入总数,且计入 failed
//!
//! # 策略
//! - 生成 N 个 future(N ∈ [0, 20]),每个独立决定成功或失败
//! - 断言 succeeded + failed == total
//! - 生成超时 future(短超时 + 长运行),验证计入 total 与 failed

#![forbid(unsafe_code)]

use std::time::Duration;

use event_bus::EventBus;
use gqep_executor::{GatherResult, GqepConfig, GqepError, GqepExecutor, GqepFuture};
use proptest::prelude::*;

/// 将任意可显示错误转换为 TestCaseError
fn fail<E: std::fmt::Display>(e: E) -> TestCaseError {
    TestCaseError::fail(format!("{e}"))
}

/// 创建立即成功的 future
fn make_success_future(value: &str) -> GqepFuture<String> {
    let value = value.to_string();
    Box::pin(async move { Ok(value) })
}

/// 创建立即失败的 future
fn make_failure_future(reason: &str) -> GqepFuture<String> {
    let reason = reason.to_string();
    Box::pin(async move {
        Err(GqepError::OperationFailed {
            operation_id: String::new(),
            reason,
        })
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// 不变量 1:聚集 succeeded + failed = total
    ///
    /// 任意操作数与成功/失败组合,gather 返回的 succeeded + failed == total
    /// WHY:对应架构红线"所有异步操作必须有 GQEP 聚集/超时处理",
    /// 不允许有操作"消失"(孤儿调用)
    #[test]
    fn test_gather_succeeded_plus_failed_equals_total(
        n_success in 0u32..=15,
        n_failure in 0u32..=10,
    ) {
        let rt = tokio::runtime::Runtime::new().map_err(fail)?;
        let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());

        let total_expected = n_success + n_failure;
        let mut futures: Vec<GqepFuture<String>> = Vec::new();
        for i in 0..n_success {
            futures.push(make_success_future(&format!("ok-{i}")));
        }
        for i in 0..n_failure {
            futures.push(make_failure_future(&format!("fail-{i}")));
        }

        let result: GatherResult = rt.block_on(executor.gather(futures));

        // 核心不变量:succeeded + failed == total(无操作丢失)
        prop_assert_eq!(
            result.succeeded + result.failed,
            result.total,
            "succeeded({}) + failed({}) != total({})",
            result.succeeded,
            result.failed,
            result.total
        );
        prop_assert_eq!(result.total, total_expected);
        prop_assert_eq!(result.succeeded, n_success);
        prop_assert_eq!(result.failed, n_failure);
        // 错误列表长度 == failed
        prop_assert_eq!(
            result.errors.len() as u32,
            result.failed,
            "errors 长度应等于 failed"
        );
    }

    /// 不变量 1c:全成功时 is_all_success 为 true
    #[test]
    fn test_gather_all_success_flag(
        n in 1u32..=10,
    ) {
        let rt = tokio::runtime::Runtime::new().map_err(fail)?;
        let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());

        let futures: Vec<GqepFuture<String>> = (0..n)
            .map(|i| make_success_future(&format!("ok-{i}")))
            .collect();

        let result = rt.block_on(executor.gather(futures));

        prop_assert_eq!(result.total, n);
        prop_assert_eq!(result.succeeded, n);
        prop_assert_eq!(result.failed, 0);
        prop_assert!(result.is_all_success());
    }

    /// 不变量 2:超时操作计入 total 与 failed
    ///
    /// WHY:超时是失败的一种,必须计入总数与失败数,不能"消失"。
    /// 使用短超时配置 + 长运行 future 触发超时
    #[test]
    fn test_gather_timeout_counted_in_total(
        n_timeout in 1u32..=5,
    ) {
        let rt = tokio::runtime::Runtime::new().map_err(fail)?;
        // 配置短超时(10ms),使长运行 future(10s)必超时
        let config = GqepConfig {
            default_timeout_ms: 10,
            ..Default::default()
        };
        let executor = GqepExecutor::new(config, EventBus::new());

        let futures: Vec<GqepFuture<String>> = (0..n_timeout)
            .map(|_| {
                Box::pin(async {
                    tokio::time::sleep(Duration::from_secs(10)).await;
                    Ok("done".to_string())
                }) as GqepFuture<String>
            })
            .collect();

        let result = rt.block_on(executor.gather(futures));

        // 超时操作计入 total
        prop_assert_eq!(result.total, n_timeout, "超时操作应计入 total");
        // 超时操作计入 failed(不消失)
        prop_assert_eq!(
            result.failed, n_timeout,
            "超时操作应计入 failed"
        );
        prop_assert_eq!(result.succeeded, 0);
    }

    /// 不变量 3:延迟非负
    ///
    /// WHY:latency_ms = elapsed,必 >= 0
    #[test]
    fn test_gather_latency_non_negative(
        n in 0u32..=10,
    ) {
        let rt = tokio::runtime::Runtime::new().map_err(fail)?;
        let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());

        let futures: Vec<GqepFuture<String>> = (0..n)
            .map(|i| make_success_future(&format!("ok-{i}")))
            .collect();

        let result = rt.block_on(executor.gather(futures));

        prop_assert!(
            result.latency_ms >= 0.0,
            "延迟应非负,实际: {}",
            result.latency_ms
        );
    }
}

// WHY 空参数测试放在 proptest! 宏外:proptest! 宏要求至少 1 个 `parm in strategy` 参数,
// 零参数函数无法匹配宏模式,因此作为普通 #[test] 编写
#[test]
fn test_gather_empty_total_zero() {
    let rt = tokio::runtime::Runtime::new().expect("failed to create runtime");
    let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());

    let result = rt.block_on(executor.gather(vec![]));

    assert_eq!(result.total, 0);
    assert_eq!(result.succeeded, 0);
    assert_eq!(result.failed, 0);
    assert!(result.errors.is_empty());
    assert!(result.is_all_success(), "空聚集应视为全成功");
}
