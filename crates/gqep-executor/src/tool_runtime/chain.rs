//! 强制沙箱派发链 — policy → sandbox → route → transport（T2-3 / T2-4）
//!
//! # 顺序即安全边界
//!
//! 每个装饰器在放行时才调用 `next`，任何一层拒绝都**不会**触发后续层：
//! 策略拒绝时沙箱与传输都不得被调用；沙箱拒绝时传输不得被调用。
//! 这条顺序不变量由 `chain.rs` 单元测试与 `tests/tool_runtime_chain_proptest.rs`
//! 的计数断言共同锁定。
//!
//! # 只能经 builder 组装
//!
//! [`SandboxedDispatcher`] 字段私有且无公有构造，唯一入口是
//! [`DispatcherBuilder::build`]；缺任一环即返回 `DispatchError::Route`，
//! 使「忘了挂沙箱」无法以类型检查通过的方式静默上线。

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use seccore::execpolicy::{ExecPolicy, PolicyAction};

use super::dispatch::{
    BorrowedDispatchFuture, BorrowedToolDispatcher, DispatchError, DispatchFuture, DispatchRequest,
    ToolDispatcher,
};
use super::sandbox_gate::SandboxGate;
use nexus_contracts::tool_dispatch::DispatchRequestRef;

/// 策略层 — execpolicy 三态裁决（Deny/Ask 均 fail-closed）
pub(crate) struct PolicyLayer {
    policy: Arc<ExecPolicy>,
    next: Arc<dyn ToolDispatcher>,
}

impl PolicyLayer {
    pub(crate) fn new(policy: Arc<ExecPolicy>, next: Arc<dyn ToolDispatcher>) -> Self {
        Self { policy, next }
    }
}

impl ToolDispatcher for PolicyLayer {
    fn dispatch<'a>(&'a self, req: &'a DispatchRequest) -> DispatchFuture<'a> {
        let action = self
            .policy
            .evaluate(&req.tool_name, std::slice::from_ref(&req.args_json));
        match action {
            PolicyAction::Allow => self.next.dispatch(req),
            PolicyAction::Deny => {
                let msg = format!("denied: policy rejected tool '{}'", req.tool_name);
                Box::pin(async move { Err(DispatchError::Policy(msg)) })
            }
            PolicyAction::Ask => {
                let msg = format!(
                    "ask_required: policy requests approval for tool '{}'",
                    req.tool_name
                );
                Box::pin(async move { Err(DispatchError::Policy(msg)) })
            }
        }
    }
}

/// 沙箱层 — 同步安全校验，失败不进入路由
pub(crate) struct SandboxLayer {
    gate: Arc<dyn SandboxGate>,
    next: Arc<dyn ToolDispatcher>,
}

impl SandboxLayer {
    pub(crate) fn new(gate: Arc<dyn SandboxGate>, next: Arc<dyn ToolDispatcher>) -> Self {
        Self { gate, next }
    }
}

impl ToolDispatcher for SandboxLayer {
    fn dispatch<'a>(&'a self, req: &'a DispatchRequest) -> DispatchFuture<'a> {
        match self.gate.check(req) {
            Ok(()) => self.next.dispatch(req),
            Err(err) => Box::pin(async move { Err(err) }),
        }
    }
}

/// 路由层 — 精确工具名命中，否则 fallback，再否则 Route 错误
pub(crate) struct RouteLayer {
    routes: HashMap<String, Arc<dyn ToolDispatcher>>,
    fallback: Option<Arc<dyn ToolDispatcher>>,
}

impl RouteLayer {
    pub(crate) fn new(
        routes: HashMap<String, Arc<dyn ToolDispatcher>>,
        fallback: Option<Arc<dyn ToolDispatcher>>,
    ) -> Self {
        Self { routes, fallback }
    }
}

impl ToolDispatcher for RouteLayer {
    fn dispatch<'a>(&'a self, req: &'a DispatchRequest) -> DispatchFuture<'a> {
        if let Some(dispatcher) = self.routes.get(&req.tool_name) {
            return dispatcher.dispatch(req);
        }
        if let Some(fallback) = self.fallback.as_ref() {
            return fallback.dispatch(req);
        }
        let msg = format!("no route registered for tool '{}'", req.tool_name);
        Box::pin(async move { Err(DispatchError::Route(msg)) })
    }
}

/// 传输层 — 零成本包装真实 dispatcher（进程内/远程/回声均可）
pub(crate) struct TransportLayer {
    inner: Arc<dyn ToolDispatcher>,
}

impl TransportLayer {
    pub(crate) fn new(inner: Arc<dyn ToolDispatcher>) -> Self {
        Self { inner }
    }
}

impl ToolDispatcher for TransportLayer {
    fn dispatch<'a>(&'a self, req: &'a DispatchRequest) -> DispatchFuture<'a> {
        self.inner.dispatch(req)
    }
}

/// 派发链构建器 — 四环缺一不可
pub struct DispatcherBuilder {
    policy: Option<Arc<ExecPolicy>>,
    sandbox: Option<Arc<dyn SandboxGate>>,
    routes: Option<HashMap<String, Arc<dyn ToolDispatcher>>>,
    fallback: Option<Arc<dyn ToolDispatcher>>,
    transport: Option<Arc<dyn ToolDispatcher>>,
}

impl DispatcherBuilder {
    /// 空构建器
    #[must_use]
    pub fn new() -> Self {
        Self {
            policy: None,
            sandbox: None,
            routes: None,
            fallback: None,
            transport: None,
        }
    }

    /// 设置策略环
    #[must_use]
    pub fn with_policy(mut self, policy: Option<Arc<ExecPolicy>>) -> Self {
        self.policy = policy;
        self
    }

    /// 设置沙箱环
    #[must_use]
    pub fn with_sandbox(mut self, sandbox: Option<Arc<dyn SandboxGate>>) -> Self {
        self.sandbox = sandbox;
        self
    }

    /// 设置路由表
    #[must_use]
    pub fn with_route(mut self, routes: Option<HashMap<String, Arc<dyn ToolDispatcher>>>) -> Self {
        self.routes = routes;
        self
    }

    /// 设置路由 fallback（可选）
    #[must_use]
    pub fn with_fallback(mut self, fallback: Option<Arc<dyn ToolDispatcher>>) -> Self {
        self.fallback = fallback;
        self
    }

    /// 设置传输环
    #[must_use]
    pub fn with_transport(mut self, transport: Option<Arc<dyn ToolDispatcher>>) -> Self {
        self.transport = transport;
        self
    }

    /// 组装链；缺任一环返回 `DispatchError::Route`（消息含 `missing <环名>`）
    pub fn build(self) -> Result<SandboxedDispatcher, DispatchError> {
        let policy = self.policy.ok_or_else(|| incomplete("policy"))?;
        let sandbox = self.sandbox.ok_or_else(|| incomplete("sandbox"))?;
        let routes = self.routes.ok_or_else(|| incomplete("routes"))?;
        let transport = self.transport.ok_or_else(|| incomplete("transport"))?;

        let transport: Arc<dyn ToolDispatcher> = Arc::new(TransportLayer::new(transport));
        let route = Arc::new(RouteLayer::new(routes, self.fallback));
        let sandbox = Arc::new(SandboxLayer::new(sandbox, route));
        let head: Arc<dyn ToolDispatcher> = Arc::new(PolicyLayer::new(policy, sandbox));

        Ok(SandboxedDispatcher { head, transport })
    }
}

impl Default for DispatcherBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// 四环齐备的强制沙箱派发器（唯一构造入口为 [`DispatcherBuilder::build`]）
pub struct SandboxedDispatcher {
    head: Arc<dyn ToolDispatcher>,
    transport: Arc<dyn ToolDispatcher>,
}

impl fmt::Debug for SandboxedDispatcher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SandboxedDispatcher")
            .finish_non_exhaustive()
    }
}

impl SandboxedDispatcher {
    /// 链尾传输环（组装期已强制存在；供诊断与上层复用同一传输实例）
    #[must_use]
    pub fn transport(&self) -> &Arc<dyn ToolDispatcher> {
        &self.transport
    }
}

impl ToolDispatcher for SandboxedDispatcher {
    fn dispatch<'a>(&'a self, req: &'a DispatchRequest) -> DispatchFuture<'a> {
        self.head.dispatch(req)
    }
}

impl BorrowedToolDispatcher for SandboxedDispatcher {
    /// 借用式派发 — 与 [`ToolDispatcher::dispatch`] 走**同一套四环**
    ///
    /// WHY 走同一套层而非另建一条借用链：另建链意味着策略/沙箱/路由/传输
    /// 各有两个实现 —— 它们的行为可能分叉，借用路径就成了绕过安全校验的
    /// 旁路。这里只做**入参形态**的适配，链的语义完全共用。
    ///
    /// 代价：入站处有一次「借用 → 拥有」转换（2 次分配）。
    /// 收益：调用方（热路径上的批量编排）**不再为每次派发构造
    /// `DispatchRequest`** —— 那 2 次分配被消除，且调用方可直接复用
    /// 同一份 `args_json` 缓冲。
    fn dispatch_borrowed<'a>(
        &'a self,
        req: &'a DispatchRequestRef<'a>,
    ) -> BorrowedDispatchFuture<'a> {
        // 一次性升级为拥有型，之后走既有链（语义与 `dispatch` 逐位一致）。
        //
        // WHY owned 在 async 块**内部**创建：`head.dispatch(&owned)` 借用
        // `owned`，若在函数体创建再返回 future，future 会持有指向已 drop
        // 局部变量的借用。放进 async 块内创建 + 立即 await，生命周期自洽。
        let head = Arc::clone(&self.head);
        Box::pin(async move {
            let owned = req.to_owned_request();
            head.dispatch(&owned).await
        })
    }
}

/// 缺环错误 — 统一归因为 Route（组装期不是运行时策略/沙箱判定）
fn incomplete(layer: &str) -> DispatchError {
    DispatchError::Route(format!("incomplete dispatcher chain: missing {layer}"))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use seccore::execpolicy::{ExecPolicyRule, RulePattern};
    use seccore::PolicyAction;

    use crate::tool_runtime::sandbox_gate::AllowAllSandboxGate;
    use crate::tool_runtime::DispatchOutcome;

    /// 回显型 transport + 调用计数
    struct CountingTransport {
        calls: Arc<AtomicUsize>,
    }

    impl ToolDispatcher for CountingTransport {
        fn dispatch<'a>(&'a self, req: &'a DispatchRequest) -> DispatchFuture<'a> {
            Box::pin(async move {
                self.calls.fetch_add(1, Ordering::SeqCst);
                Ok(DispatchOutcome {
                    content_json: req.args_json.clone(),
                })
            })
        }
    }

    /// 计数型沙箱门
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

    fn request(tool_name: &str) -> DispatchRequest {
        DispatchRequest {
            tool_name: tool_name.to_string(),
            args_json: "{\"k\":\"v\"}".to_string(),
        }
    }

    fn allow_policy(tool: &str) -> Arc<ExecPolicy> {
        Arc::new(ExecPolicy::new().add_rule(ExecPolicyRule {
            pattern: RulePattern::new(tool, "*"),
            action: PolicyAction::Allow,
        }))
    }

    fn ask_policy(tool: &str) -> Arc<ExecPolicy> {
        Arc::new(ExecPolicy::new().add_rule(ExecPolicyRule {
            pattern: RulePattern::new(tool, "*"),
            action: PolicyAction::Ask,
        }))
    }

    fn counting_transport(calls: &Arc<AtomicUsize>) -> Arc<dyn ToolDispatcher> {
        Arc::new(CountingTransport {
            calls: Arc::clone(calls),
        })
    }

    fn gate(calls: &Arc<AtomicUsize>, deny: bool) -> Arc<dyn SandboxGate> {
        Arc::new(CountingGate {
            calls: Arc::clone(calls),
            deny,
        })
    }

    fn routes_hit(
        tool: &str,
        dispatcher: &Arc<dyn ToolDispatcher>,
    ) -> HashMap<String, Arc<dyn ToolDispatcher>> {
        let mut routes = HashMap::new();
        routes.insert(tool.to_string(), Arc::clone(dispatcher));
        routes
    }

    #[tokio::test]
    async fn policy_deny_stops_before_sandbox_and_transport() {
        let gate_calls = Arc::new(AtomicUsize::new(0));
        let transport_calls = Arc::new(AtomicUsize::new(0));
        let transport = counting_transport(&transport_calls);
        let dispatcher = DispatcherBuilder::new()
            .with_policy(Some(Arc::new(ExecPolicy::new())))
            .with_sandbox(Some(gate(&gate_calls, false)))
            .with_route(Some(routes_hit("echo", &transport)))
            .with_transport(Some(Arc::clone(&transport)))
            .build()
            .unwrap();

        let result = dispatcher.dispatch(&request("echo")).await;
        assert!(matches!(result, Err(DispatchError::Policy(_))));
        assert_eq!(
            gate_calls.load(Ordering::SeqCst),
            0,
            "policy deny 后不得进沙箱"
        );
        assert_eq!(
            transport_calls.load(Ordering::SeqCst),
            0,
            "policy deny 后不得进传输"
        );
    }

    #[tokio::test]
    async fn policy_ask_stops_before_sandbox_and_transport() {
        let gate_calls = Arc::new(AtomicUsize::new(0));
        let transport_calls = Arc::new(AtomicUsize::new(0));
        let transport = counting_transport(&transport_calls);
        let dispatcher = DispatcherBuilder::new()
            .with_policy(Some(ask_policy("echo")))
            .with_sandbox(Some(gate(&gate_calls, false)))
            .with_route(Some(routes_hit("echo", &transport)))
            .with_transport(Some(Arc::clone(&transport)))
            .build()
            .unwrap();

        let result = dispatcher.dispatch(&request("echo")).await;
        match result {
            Err(DispatchError::Policy(msg)) => {
                assert!(msg.contains("ask_required"), "ask 应显式标记: {msg}")
            }
            other => panic!("期望 Policy ask_required，实际: {other:?}"),
        }
        assert_eq!(gate_calls.load(Ordering::SeqCst), 0);
        assert_eq!(transport_calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn sandbox_deny_stops_before_transport() {
        let gate_calls = Arc::new(AtomicUsize::new(0));
        let transport_calls = Arc::new(AtomicUsize::new(0));
        let transport = counting_transport(&transport_calls);
        let dispatcher = DispatcherBuilder::new()
            .with_policy(Some(allow_policy("echo")))
            .with_sandbox(Some(gate(&gate_calls, true)))
            .with_route(Some(routes_hit("echo", &transport)))
            .with_transport(Some(Arc::clone(&transport)))
            .build()
            .unwrap();

        let result = dispatcher.dispatch(&request("echo")).await;
        assert!(matches!(result, Err(DispatchError::Sandbox(_))));
        assert_eq!(gate_calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            transport_calls.load(Ordering::SeqCst),
            0,
            "沙箱拒绝后不得进传输"
        );
    }

    #[tokio::test]
    async fn route_miss_stops_before_transport() {
        let gate_calls = Arc::new(AtomicUsize::new(0));
        let transport_calls = Arc::new(AtomicUsize::new(0));
        let transport = counting_transport(&transport_calls);
        let dispatcher = DispatcherBuilder::new()
            .with_policy(Some(allow_policy("echo")))
            .with_sandbox(Some(gate(&gate_calls, false)))
            .with_route(Some(HashMap::new()))
            .with_transport(Some(Arc::clone(&transport)))
            .build()
            .unwrap();

        let result = dispatcher.dispatch(&request("echo")).await;
        assert!(matches!(result, Err(DispatchError::Route(_))));
        assert_eq!(
            transport_calls.load(Ordering::SeqCst),
            0,
            "路由未命中不得进传输"
        );
    }

    #[tokio::test]
    async fn allow_path_returns_content() {
        let gate_calls = Arc::new(AtomicUsize::new(0));
        let transport_calls = Arc::new(AtomicUsize::new(0));
        let transport = counting_transport(&transport_calls);
        let dispatcher = DispatcherBuilder::new()
            .with_policy(Some(allow_policy("echo")))
            .with_sandbox(Some(gate(&gate_calls, false)))
            .with_route(Some(routes_hit("echo", &transport)))
            .with_transport(Some(Arc::clone(&transport)))
            .build()
            .unwrap();

        let outcome = dispatcher.dispatch(&request("echo")).await.unwrap();
        assert_eq!(outcome.content_json, "{\"k\":\"v\"}");
        assert_eq!(gate_calls.load(Ordering::SeqCst), 1);
        assert_eq!(transport_calls.load(Ordering::SeqCst), 1);
    }

    /// 4 个缺环 build 失败，且错误消息点名缺失环
    #[test]
    fn build_fails_for_each_missing_layer() {
        let transport_calls = Arc::new(AtomicUsize::new(0));
        let transport = counting_transport(&transport_calls);
        let sandbox: Arc<dyn SandboxGate> = Arc::new(AllowAllSandboxGate::new());

        // 缺 policy
        let err = DispatcherBuilder::new()
            .with_sandbox(Some(Arc::clone(&sandbox)))
            .with_route(Some(HashMap::new()))
            .with_transport(Some(Arc::clone(&transport)))
            .build()
            .expect_err("缺 policy 应失败");
        assert!(err.to_string().contains("missing policy"), "实际: {err}");

        // 缺 sandbox
        let err = DispatcherBuilder::new()
            .with_policy(Some(Arc::new(ExecPolicy::new())))
            .with_route(Some(HashMap::new()))
            .with_transport(Some(Arc::clone(&transport)))
            .build()
            .expect_err("缺 sandbox 应失败");
        assert!(err.to_string().contains("missing sandbox"), "实际: {err}");

        // 缺 routes
        let err = DispatcherBuilder::new()
            .with_policy(Some(Arc::new(ExecPolicy::new())))
            .with_sandbox(Some(Arc::clone(&sandbox)))
            .with_transport(Some(Arc::clone(&transport)))
            .build()
            .expect_err("缺 routes 应失败");
        assert!(err.to_string().contains("missing routes"), "实际: {err}");

        // 缺 transport
        let err = DispatcherBuilder::new()
            .with_policy(Some(Arc::new(ExecPolicy::new())))
            .with_sandbox(Some(Arc::clone(&sandbox)))
            .with_route(Some(HashMap::new()))
            .build()
            .expect_err("缺 transport 应失败");
        assert!(err.to_string().contains("missing transport"), "实际: {err}");
    }

    /// 路由 fallback 生效
    #[tokio::test]
    async fn fallback_route_is_used_on_miss() {
        let transport_calls = Arc::new(AtomicUsize::new(0));
        let transport = counting_transport(&transport_calls);
        let dispatcher = DispatcherBuilder::new()
            .with_policy(Some(allow_policy("echo")))
            .with_sandbox(Some(gate(&Arc::new(AtomicUsize::new(0)), false)))
            .with_route(Some(HashMap::new()))
            .with_fallback(Some(Arc::clone(&transport)))
            .with_transport(Some(Arc::clone(&transport)))
            .build()
            .unwrap();

        let outcome = dispatcher.dispatch(&request("echo")).await.unwrap();
        assert_eq!(outcome.content_json, "{\"k\":\"v\"}");
        assert_eq!(transport_calls.load(Ordering::SeqCst), 1);
    }
}
