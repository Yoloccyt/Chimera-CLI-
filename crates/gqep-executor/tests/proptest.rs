//! GQEP 聚集查询执行协议属性测试 — 验证聚集结果不变量
//!
//! 对应 SubTask 29.6:为 gqep-executor 补充 proptest
//!
//! # 验证的不变量
//! 1. 聚集四段守恒：`succeeded + failed + abandoned + not_started == total`（ADR-202 I5）
//! 2. 超时操作计入 total：超时的操作也计入总数,且计入 failed
//!
//! # 关于第 1 条的历史修正（ADR-202 R3 / I9 / I11）
//! 旧版本文件断言的是两段守恒 `succeeded + failed == total`，它只在
//! 「无全局超时、无并发上界 backlog」的条件下成立——属**条件成立断言**。
//! 并发上界引入 `not_started` 后，两段式会把「根本没执行」误归为丢失或漏计，
//! 故本文件改为四段守恒，并把生成域扩到「含慢 future + 收紧 deadline」。
//! 纪律：改的是**不变量表达式的完整性**，不是放宽阈值。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code
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

/// 创建带时延的成功 future（用于把全局 deadline 逼进采样空间）
///
/// WHY 新增而非只测快 future：两段守恒的漏改止在「有操作未完成」时才暴露，
/// 全快 future + 宽松 deadline 的生成永远碰不到那个分支（ADR-202 R3）。
fn make_slow_future(value: &str, delay: Duration) -> GqepFuture<String> {
    let value = value.to_string();
    Box::pin(async move {
        tokio::time::sleep(delay).await;
        Ok(value)
    })
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

    /// 不变量 1：聚集四段守恒 succeeded + failed + abandoned + not_started = total
    ///
    /// 任意操作数与成功/失败组合，守恒必须成立。
    /// WHY：对应架构红线"所有异步操作必须有 GQEP 聚集/超时处理"，
    /// 不允许有操作"消失"（孤儿调用）——并发上界下「消失」有两条路径：
    /// 已入流被放弃（abandoned，有 OrphanGuard）与从未入流（not_started，无孤儿事件），
    /// 两段式只能遮住前者。
    #[test]
    fn test_gather_four_segment_conservation(
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

        // 核心不变量：四段守恒（任何一段漏计即说明有操作静默消失）
        prop_assert_eq!(
            result.succeeded + result.failed + result.abandoned + result.not_started,
            result.total,
            "四段不守恒：succeeded({}) failed({}) abandoned({}) not_started({}) != total({})",
            result.succeeded,
            result.failed,
            result.abandoned,
            result.not_started,
            result.total,
        );
        prop_assert_eq!(result.total, total_expected);
        prop_assert_eq!(result.succeeded, n_success);
        prop_assert_eq!(result.failed, n_failure);
        // 全快 future + 默认宽松 deadline 的强条件：无放弃、无未启动，
        // 且错误列表长度 == failed——这三条就是旧断言的**完整保留**，
        // 不因改成四段而丢掉原有的强语义（I9：不得为过而放宽）。
        prop_assert_eq!(result.abandoned, 0, "本生成域不应有放弃项");
        prop_assert_eq!(result.not_started, 0, "bound=100 ≥ N≤25 应单波全部入流");
        prop_assert_eq!(
            result.errors.len() as u32,
            result.failed,
            "errors 长度应等于 failed"
        );
    }

    /// 不变量 1b（ADR-202 I5/I9/I11）：限流 + 全局超时同时发生时守恒仍成立
    ///
    /// WHY 新增一条而不是改上面的：上面那条故意用「全快 + 宽松 deadline」，
    /// 守的是向后兼容强条件；本条用「慢 future + 紧 deadline + 小 bound」，
    /// 守的是新分支。两者合起来才能既不让旧语义静默退化，也不让新分支无人管。
    #[test]
    fn test_gather_four_segment_conservation_under_bound_and_deadline(
        n_fast in 0u32..=6,
        n_slow in 1u32..=12,
        bound in 1usize..=8,
        deadline_ms in 20u64..=120,
    ) {
        let rt = tokio::runtime::Runtime::new().map_err(fail)?;
        let executor = GqepExecutor::new(
            GqepConfig {
                max_concurrency: bound,
                gather_deadline_ms: deadline_ms,
                // 单操作超时放宽，确保只考察**全局** deadline 分支
                default_timeout_ms: 5_000,
                ..GqepConfig::default()
            },
            EventBus::new(),
        );

        let mut futures: Vec<GqepFuture<String>> = Vec::new();
        for i in 0..n_fast {
            futures.push(make_success_future(&format!("fast-{i}")));
        }
        for i in 0..n_slow {
            // 比最宽 deadline 还慢，必定落入 abandoned / not_started 两段之一
            futures.push(make_slow_future(
                &format!("slow-{i}"),
                Duration::from_millis(200),
            ));
        }
        let total_expected = n_fast + n_slow;

        let result: GatherResult = rt.block_on(executor.gather(futures));

        prop_assert_eq!(result.total, total_expected);
        // 主断言：四段守恒在最恶劣的组合下仍成立
        prop_assert_eq!(
            result.succeeded + result.failed + result.abandoned + result.not_started,
            result.total,
            "限流+超时下四段不守恒：succeeded({}) failed({}) abandoned({}) not_started({}) != total({})",
            result.succeeded,
            result.failed,
            result.abandoned,
            result.not_started,
            result.total,
        );
        // 慢 future 存在 ⇒ 必有未完成项（否则说明超时根本没生效，测试失去意义）
        prop_assert!(
            result.abandoned + result.not_started > 0,
            "慢 future + 紧 deadline 下应至少有未完成项，实际全完成→本用例退化为空转"
        );
        // errors 长度只能为 failed 或 failed+1（后者含一条 GlobalTimedOut）
        // ——这正是 types.rs 已自陈、但旧断言看不到的例外（R3）。
        prop_assert!(
            result.errors.len() as u32 == result.failed
                || result.errors.len() as u32 == result.failed + 1,
            "errors 长度应为 failed 或 failed+1，实际 len={} failed={}",
            result.errors.len(),
            result.failed,
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
