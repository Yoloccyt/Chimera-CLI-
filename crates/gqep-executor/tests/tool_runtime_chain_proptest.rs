//! 工具运行时强制沙箱派发链属性测试（T2-4）
//!
//! # 验证的不变量（每条 64 cases）
//! 1. `denied_policy_never_reaches_transport`：策略拒绝时 transport 调用恒为 0
//! 2. `route_miss_never_reaches_transport`：路由未命中时 transport 调用恒为 0
//! 3. `sandbox_denied_never_reaches_transport`：沙箱拒绝时 transport 调用恒为 0
//!
//! WHY：派发链是 fail-closed 的前置证据；一旦任一前置层「先放过去再报错」，
//! 真实副作用已经发生，错误变体保留但安全边界已破。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use gqep_executor::tool_runtime::{
    DispatchError, DispatchOutcome, DispatchRequest, DispatcherBuilder, SandboxGate,
    SandboxedDispatcher, ToolDispatcher,
};
use proptest::prelude::*;
use seccore::execpolicy::{ExecPolicy, ExecPolicyRule, PolicyAction, RulePattern};

/// 回显型 transport + 调用计数（用于断言「从未到达」）
struct CountingTransport {
    calls: Arc<AtomicUsize>,
}

impl ToolDispatcher for CountingTransport {
    fn dispatch<'a>(
        &'a self,
        req: &'a DispatchRequest,
    ) -> gqep_executor::tool_runtime::DispatchFuture<'a> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Ok(DispatchOutcome {
                content_json: req.args_json.clone(),
            })
        })
    }
}

/// 计数型沙箱门（可配置是否拒绝）
struct CountingGate {
    calls: Arc<AtomicUsize>,
    deny: bool,
}

impl SandboxGate for CountingGate {
    fn check(&self, req: &DispatchRequest) -> Result<(), DispatchError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.deny {
            Err(DispatchError::Sandbox(format!("denied: {}", req.tool_name)))
        } else {
            Ok(())
        }
    }
}

/// 允许任意工具的 ExecPolicy（工具名与请求同名，arg 通配 `*`）
fn allow_policy(tool: &str) -> Arc<ExecPolicy> {
    Arc::new(ExecPolicy::new().add_rule(ExecPolicyRule {
        pattern: RulePattern::new(tool, "*"),
        action: PolicyAction::Allow,
    }))
}

/// 组装完整四层链
fn build_chain(
    policy: Arc<ExecPolicy>,
    gate: Arc<dyn SandboxGate>,
    routes: HashMap<String, Arc<dyn ToolDispatcher>>,
    transport: Arc<dyn ToolDispatcher>,
) -> SandboxedDispatcher {
    DispatcherBuilder::new()
        .with_policy(Some(policy))
        .with_sandbox(Some(gate))
        .with_route(Some(routes))
        .with_transport(Some(transport))
        .build()
        .expect("四层齐备的链应构建成功")
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// 不变量 1：策略拒绝时 transport 调用恒为 0
    #[test]
    fn denied_policy_never_reaches_transport(tool in any::<String>(), args_json in any::<String>()) {
        let transport_calls = Arc::new(AtomicUsize::new(0));
        let gate_calls = Arc::new(AtomicUsize::new(0));

        let transport: Arc<dyn ToolDispatcher> = Arc::new(CountingTransport {
            calls: Arc::clone(&transport_calls),
        });
        let gate: Arc<dyn SandboxGate> = Arc::new(CountingGate {
            calls: Arc::clone(&gate_calls),
            deny: false,
        });
        // 默认策略为零信任 Deny
        let policy = Arc::new(ExecPolicy::new());

        let dispatcher = build_chain(policy, gate, HashMap::new(), transport);
        let req = DispatchRequest { tool_name: tool, args_json };
        let rt = tokio::runtime::Runtime::new().map_err(|e| TestCaseError::fail(e.to_string()))?;

        let result = rt.block_on(dispatcher.dispatch(&req));

        prop_assert!(
            matches!(result, Err(DispatchError::Policy(_))),
            "策略拒绝应返回 Policy 错误，实际: {:?}",
            result
        );
        prop_assert_eq!(transport_calls.load(Ordering::SeqCst), 0, "transport 不得被调用");
        prop_assert_eq!(gate_calls.load(Ordering::SeqCst), 0, "sandbox 不得被调用");
    }

    /// 不变量 2：路由未命中时 transport 调用恒为 0
    #[test]
    fn route_miss_never_reaches_transport(tool in any::<String>(), args_json in any::<String>()) {
        let transport_calls = Arc::new(AtomicUsize::new(0));
        let gate_calls = Arc::new(AtomicUsize::new(0));

        let transport: Arc<dyn ToolDispatcher> = Arc::new(CountingTransport {
            calls: Arc::clone(&transport_calls),
        });
        let gate: Arc<dyn SandboxGate> = Arc::new(CountingGate {
            calls: Arc::clone(&gate_calls),
            deny: false,
        });
        let policy = allow_policy(&tool);

        // 路由表为空且无 fallback ⇒ 必然 miss
        let dispatcher = build_chain(policy, gate, HashMap::new(), transport);
        let req = DispatchRequest { tool_name: tool, args_json };
        let rt = tokio::runtime::Runtime::new().map_err(|e| TestCaseError::fail(e.to_string()))?;

        let result = rt.block_on(dispatcher.dispatch(&req));

        prop_assert!(
            matches!(result, Err(DispatchError::Route(_))),
            "路由未命中应返回 Route 错误，实际: {:?}",
            result
        );
        prop_assert_eq!(transport_calls.load(Ordering::SeqCst), 0, "transport 不得被调用");
        prop_assert!(gate_calls.load(Ordering::SeqCst) >= 1, "sandbox 层应已被评估");
    }

    /// 不变量 3：沙箱拒绝时 transport 调用恒为 0
    #[test]
    fn sandbox_denied_never_reaches_transport(tool in any::<String>(), args_json in any::<String>()) {
        let transport_calls = Arc::new(AtomicUsize::new(0));
        let gate_calls = Arc::new(AtomicUsize::new(0));

        let transport: Arc<dyn ToolDispatcher> = Arc::new(CountingTransport {
            calls: Arc::clone(&transport_calls),
        });
        let gate: Arc<dyn SandboxGate> = Arc::new(CountingGate {
            calls: Arc::clone(&gate_calls),
            deny: true,
        });
        let policy = allow_policy(&tool);

        let mut routes: HashMap<String, Arc<dyn ToolDispatcher>> = HashMap::new();
        // 路由命中的结果不改写结论：沙箱先于路由评估
        routes.insert(tool.clone(), Arc::clone(&transport));

        let dispatcher = build_chain(policy, gate, routes, transport);
        let req = DispatchRequest { tool_name: tool, args_json };
        let rt = tokio::runtime::Runtime::new().map_err(|e| TestCaseError::fail(e.to_string()))?;

        let result = rt.block_on(dispatcher.dispatch(&req));

        prop_assert!(
            matches!(result, Err(DispatchError::Sandbox(_))),
            "沙箱拒绝应返回 Sandbox 错误，实际: {:?}",
            result
        );
        prop_assert_eq!(transport_calls.load(Ordering::SeqCst), 0, "transport 不得被调用");
    }
}
