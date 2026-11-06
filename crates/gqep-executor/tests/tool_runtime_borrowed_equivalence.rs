//! 借用化派发与拥有型派发的**逐位等价**属性测试（ADR-241 W3-T2）
//!
//! # 验证的不变量
//!
//! 1. `borrowed_dispatch_matches_owned_dispatch`：任意工具名/参数组合下，
//!    `dispatch_borrowed` 的结果与 `dispatch` **逐位相同**（含错误变体与文案）。
//! 2. `borrowed_sandbox_verdict_matches_owned`：沙箱门的 `check_ref` 与 `check`
//!    判决**逐位相同** —— 借用路径不得成为绕过安全校验的旁路。
//! 3. `dispatch_request_ref_has_no_hidden_allocation`：
//!    `size_of::<DispatchRequestRef>() == 2 * size_of::<&str>()` —— 无隐藏分配。
//!
//! # WHY 这是硬约束
//!
//! 借用化引入了两条路径（拥有型 / 借用型）走同一套四环。若两者判决分叉，
//! 借用路径就成了一条**绕过策略与沙箱的旁路** —— 这比不优化严重得多。
//! 因此等价性必须由 property test 锁定，而不是靠人工核对。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::sync::Arc;

use async_trait::async_trait;
use gqep_executor::tool_runtime::{
    BorrowedToolDispatcher, DispatchError, DispatchOutcome, DispatchRequest, DispatchRequestRef,
    SandboxGate, SeccoreSandboxGate, ToolDispatcher, ToolRuntime,
};
use gqep_executor::ToolExecutor;
use nexus_contracts::tool_dispatch::DispatchRequestRef as L0DispatchRequestRef;
use proptest::prelude::*;
use seccore::execpolicy::{ExecPolicy, ExecPolicyRule, PolicyAction, RulePattern};

/// 回显执行器
struct EchoExecutor;

#[async_trait]
impl ToolExecutor for EchoExecutor {
    async fn execute(&self, _tool_name: &str, args_json: &str) -> Result<String, String> {
        Ok(args_json.to_string())
    }
}

/// 允许 `echo`，其余走零信任 Deny
fn echo_only_policy() -> Arc<ExecPolicy> {
    Arc::new(ExecPolicy::new().add_rule(ExecPolicyRule {
        pattern: RulePattern::new("echo", "*"),
        action: PolicyAction::Allow,
    }))
}

fn runtime() -> ToolRuntime {
    ToolRuntime::new(Arc::new(EchoExecutor), echo_only_policy()).expect("四环齐备的装配应成功")
}

/// 不变量 1：借用派发与拥有派发逐位等价。
proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn borrowed_dispatch_matches_owned_dispatch(
        tool_name in "[a-zA-Z0-9._-]{1,32}",
        args_json in ".{0,120}",
    ) {
        let rt = tokio::runtime::Runtime::new().map_err(|e| TestCaseError::fail(e.to_string()))?;

        let runtime = runtime();
        let owned = DispatchRequest {
            tool_name: tool_name.clone(),
            args_json: args_json.clone(),
        };
        let borrowed = DispatchRequestRef {
            tool_name: &tool_name,
            args_json: &args_json,
        };

        let got = rt.block_on(async {
            let a = runtime.dispatch(&owned).await;
            let b = runtime.dispatch_borrowed(&borrowed).await;
            (a, b)
        });

        let (own_result, borrowed_result) = got;
        prop_assert_eq!(
            &own_result, &borrowed_result,
            "借用派发必须与拥有派发逐位一致（含错误变体与文案）"
        );
    }

    /// 不变量 2：沙箱门借用校验与拥有校验判决逐位一致。
    ///
    /// WHY 独立于不变量 1：沙箱判决的一致性是**安全属性**，
    /// 不能只靠「端到端结果相同」间接证明 —— 若两者都放行，
    /// 端到端结果也相同，但拦截能力已经退化。
    #[test]
    fn borrowed_sandbox_verdict_matches_owned(
        tool_name in "[a-zA-Z0-9._-]{1,32}",
        args_json in ".{0,120}",
    ) {
        let gate = SeccoreSandboxGate::default_secure();
        let owned = DispatchRequest {
            tool_name: tool_name.clone(),
            args_json: args_json.clone(),
        };
        let borrowed = DispatchRequestRef {
            tool_name: &tool_name,
            args_json: &args_json,
        };

        let own = gate.check(&owned);
        let bor = gate.check_ref(&borrowed);
        prop_assert_eq!(
            &own, &bor,
            "沙箱门的借用校验必须与拥有校验判决逐位一致"
        );
    }
}

/// 不变量 3：借用视图无隐藏分配。
#[test]
fn dispatch_request_ref_has_no_hidden_allocation() {
    assert_eq!(
        std::mem::size_of::<DispatchRequestRef<'static>>(),
        2 * std::mem::size_of::<&str>(),
        "借用视图必须恰好是两个引用的大小（无隐藏分配、无填充）"
    );
    // L0 契约处导出的同一类型必须等价（防止两处定义分叉）
    assert_eq!(
        std::mem::size_of::<L0DispatchRequestRef<'static>>(),
        2 * std::mem::size_of::<&str>(),
        "L0 契约处的借用视图必须与 gqep-executor 导出的是同一类型"
    );
}

/// 借用视图与拥有型之间的转换必须逐字段相等。
#[test]
fn dispatch_request_ref_conversion_is_field_exact() {
    let owned = DispatchRequest {
        tool_name: "repo.search".to_string(),
        args_json: r#"{"q":"x"}"#.to_string(),
    };
    let borrowed: DispatchRequestRef<'_> = (&owned).into();
    assert_eq!(borrowed.tool_name, owned.tool_name.as_str());
    assert_eq!(borrowed.args_json, owned.args_json.as_str());

    // 显式升级回拥有型：内容必须完全一致
    let round_trip = borrowed.to_owned_request();
    assert_eq!(round_trip, owned);
}

/// 借用派发的**安全前置顺序**不变：沙箱拒绝时不得触达传输环。
///
/// WHY：借用路径走同一套四环，但入参形态不同 —— 必须证明形态差异
/// 没有改变短路顺序（否则借用路径会先执行再校验）。
#[tokio::test]
async fn borrowed_dispatch_preserves_sandbox_short_circuit() {
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counting = Arc::new(CountingExecutor {
        calls: Arc::clone(&calls),
    });
    let runtime = ToolRuntime::new(counting, echo_only_policy()).expect("装配应成功");

    let tool_name = "echo";
    let args_json = "$(rm -rf /)";
    let borrowed = DispatchRequestRef {
        tool_name,
        args_json,
    };

    let result = runtime.dispatch_borrowed(&borrowed).await;
    assert!(
        matches!(result, Err(DispatchError::Sandbox(_))),
        "注入形态应被沙箱层拦截（归因 Sandbox），实际: {result:?}"
    );
    assert_eq!(
        calls.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "沙箱拒绝后不得触达 delegate（借用路径同样必须短路）"
    );
}

/// 计数执行器
struct CountingExecutor {
    calls: Arc<std::sync::atomic::AtomicUsize>,
}

#[async_trait]
impl ToolExecutor for CountingExecutor {
    async fn execute(&self, _tool_name: &str, args_json: &str) -> Result<String, String> {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(args_json.to_string())
    }
}
