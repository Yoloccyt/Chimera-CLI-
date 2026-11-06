//! 批量派发语义等价属性测试（性能增量 2026-11-06）
//!
//! # 验证的不变量（每条 64 cases）
//! 1. `batch_equals_serial_elementwise`：任意请求序列下，`execute_batch` 的
//!    **逐位结果**（含成功内容与错误变体/文案）与逐条串行派发完全一致。
//! 2. `batch_preserves_slot_count_and_conservation`：结果条数恒等于输入条数，
//!    且四段守恒 `succeeded + failed + abandoned + not_started == total` 恒成立。
//!
//! WHY：并行化是硬门槛——"结果集相同"不够，**顺序契约**与错误归因也必须逐位一致；
//! 否则调用方按下标消费时会静默错位。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::sync::Arc;

use async_trait::async_trait;
use gqep_executor::tool_runtime::{BatchLimits, DispatchOutcome, DispatchRequest, ToolRuntime};
use gqep_executor::ToolExecutor;
use proptest::prelude::*;
use seccore::execpolicy::{ExecPolicy, ExecPolicyRule, PolicyAction, RulePattern};

/// 回显执行器 — 内容原样回传（无延迟，只测编排正确性）
struct EchoExecutor;

#[async_trait]
impl ToolExecutor for EchoExecutor {
    async fn execute(&self, _tool_name: &str, args_json: &str) -> Result<String, String> {
        Ok(args_json.to_string())
    }
}

/// 仅允许 `echo` 的策略（其余工具走零信任 Deny，覆盖拒绝路径）
fn echo_only_policy() -> Arc<ExecPolicy> {
    Arc::new(ExecPolicy::new().add_rule(ExecPolicyRule {
        pattern: RulePattern::new("echo", "*"),
        action: PolicyAction::Allow,
    }))
}

fn runtime() -> ToolRuntime {
    ToolRuntime::new(Arc::new(EchoExecutor), echo_only_policy()).expect("仅进程内回显的装配应成功")
}

/// 逐条串行派发（对照金标准）
fn serial_dispatches(
    runtime: &ToolRuntime,
    requests: &[DispatchRequest],
) -> Vec<Result<DispatchOutcome, gqep_executor::tool_runtime::DispatchError>> {
    let rt = tokio::runtime::Runtime::new().expect("创建 tokio runtime 失败");
    rt.block_on(async {
        let mut out = Vec::with_capacity(requests.len());
        for req in requests {
            out.push(runtime.dispatch(req).await);
        }
        out
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// 不变量 1：任意请求序列下，批量结果与串行结果逐位一致
    #[test]
    fn batch_equals_serial_elementwise(
        plans in proptest::collection::vec(
            (any::<String>(), any::<String>()),
            0..6,
        ),
    ) {
        let requests: Vec<DispatchRequest> = plans
            .into_iter()
            .map(|(tool_name, args_json)| DispatchRequest { tool_name, args_json })
            .collect();

        let runtime = runtime();
        let expected = serial_dispatches(&runtime, &requests);

        let rt = tokio::runtime::Runtime::new().map_err(|e| TestCaseError::fail(e.to_string()))?;
        let outcome = rt.block_on(runtime.execute_batch(&requests, BatchLimits {
            concurrency: 3,
            per_op_timeout_ms: 0,
            deadline_ms: 0,
        }));

        prop_assert_eq!(
            outcome.results.len(),
            expected.len(),
            "批量结果条数必须与输入一致"
        );
        for (i, (got, want)) in outcome.results.iter().zip(expected.iter()).enumerate() {
            prop_assert_eq!(
                got, want,
                "第 {} 条结果必须与串行逐位一致（含错误变体与文案）",
                i
            );
        }
        prop_assert!(outcome.is_conserved(), "四段守恒必须成立");
    }

    /// 不变量 2：任意规模下不丢槽位且四段守恒
    #[test]
    fn batch_preserves_slot_count_and_conservation(n in 0usize..24) {
        let requests: Vec<DispatchRequest> = (0..n)
            .map(|i| DispatchRequest {
                tool_name: "echo".to_string(),
                args_json: format!("{{\"i\":{i}}}"),
            })
            .collect();

        let runtime = runtime();
        let rt = tokio::runtime::Runtime::new().map_err(|e| TestCaseError::fail(e.to_string()))?;
        let outcome = rt.block_on(runtime.execute_batch(&requests, BatchLimits {
            concurrency: 4,
            per_op_timeout_ms: 0,
            deadline_ms: 0,
        }));

        prop_assert_eq!(outcome.results.len(), n, "结果条数必须等于输入条数");
        prop_assert_eq!(outcome.succeeded, n, "全部允许的 echo 请求应成功");
        prop_assert_eq!(outcome.failed, 0, "无失败");
        prop_assert_eq!(outcome.abandoned, 0, "无全局 deadline 时不应有放弃");
        prop_assert_eq!(outcome.not_started, 0, "无全局 deadline 时不应有未启动");
        prop_assert!(outcome.is_conserved(), "四段守恒必须成立");
    }
}
