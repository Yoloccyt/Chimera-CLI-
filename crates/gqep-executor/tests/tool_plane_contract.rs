//! Tool execution plane contract tests (Phase 1).
//!
//! These tests lock the L7 contracts before any production wiring:
//! registry uniqueness, policy outcomes, bounded scheduling, cancellation,
//! UTF-8 payload truncation, batch order/isolation, metrics, and the legacy
//! `ToolExecutor` compatibility surface.

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use gqep_executor::tool_plane::{
    BoundedToolScheduler, ToolDescriptor, ToolError, ToolInvocation, ToolOrigin, ToolPriority,
    ToolProvider, ToolRegistry, ToolRuntime, ToolRuntimeConfig, ToolScheduler, ToolStatus,
};
use gqep_executor::ToolExecutor;
use nexus_contracts::tool_plan::SideEffectDecl;
use seccore::execpolicy::{ExecPolicy, ExecPolicyRule, PolicyAction, RulePattern};
use tokio_util::sync::CancellationToken;

// ------------------------------------------------------------
// Test providers
// ------------------------------------------------------------

struct EchoProvider {
    tool: &'static str,
    provider: &'static str,
    sleep_ms: u64,
    calls: AtomicUsize,
    current: AtomicUsize,
    peak: AtomicUsize,
}

impl EchoProvider {
    fn new(tool: &'static str, provider: &'static str, sleep_ms: u64) -> Self {
        Self {
            tool,
            provider,
            sleep_ms,
            calls: AtomicUsize::new(0),
            current: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn peak(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ToolProvider for EchoProvider {
    fn provider_id(&self) -> &str {
        self.provider
    }

    fn descriptors(&self) -> Vec<ToolDescriptor> {
        vec![descriptor(self.tool, self.provider)]
    }

    async fn invoke(
        &self,
        inv: &ToolInvocation,
        _cancel: CancellationToken,
    ) -> Result<String, ToolError> {
        let current = self.current.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(current, Ordering::SeqCst);
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.sleep_ms > 0 {
            tokio::time::sleep(Duration::from_millis(self.sleep_ms)).await;
        }
        self.current.fetch_sub(1, Ordering::SeqCst);
        Ok(inv.args_json.clone())
    }
}

struct FailProvider;

#[async_trait]
impl ToolProvider for FailProvider {
    fn provider_id(&self) -> &str {
        "fail"
    }

    fn descriptors(&self) -> Vec<ToolDescriptor> {
        vec![descriptor("fail.fail", "fail")]
    }

    async fn invoke(
        &self,
        _inv: &ToolInvocation,
        _cancel: CancellationToken,
    ) -> Result<String, ToolError> {
        Err(ToolError::Provider("boom".into()))
    }
}

struct BigProvider;

#[async_trait]
impl ToolProvider for BigProvider {
    fn provider_id(&self) -> &str {
        "big"
    }

    fn descriptors(&self) -> Vec<ToolDescriptor> {
        vec![descriptor("big.big", "big")]
    }

    async fn invoke(
        &self,
        _inv: &ToolInvocation,
        _cancel: CancellationToken,
    ) -> Result<String, ToolError> {
        Ok("你好世界".to_string())
    }
}

fn descriptor(name: &str, provider: &str) -> ToolDescriptor {
    ToolDescriptor {
        name: name.to_string(),
        provider: provider.to_string(),
        description: "test tool".to_string(),
        side_effect: SideEffectDecl::ReadOnly,
        timeout_ms: 5_000,
        max_concurrency: 4,
        input_schema: None,
    }
}

fn inv(tool: &str) -> ToolInvocation {
    ToolInvocation {
        call_id: format!("call-{tool}"),
        tool: tool.to_string(),
        args_json: "{}".to_string(),
        timeout_ms: None,
        priority: ToolPriority::Normal,
        origin: ToolOrigin::Cli,
        quest_id: None,
        task_id: None,
        approved: false,
    }
}

fn runtime_with(
    providers: Vec<Arc<dyn ToolProvider>>,
    policy: Option<Arc<ExecPolicy>>,
    cfg: ToolRuntimeConfig,
) -> (Arc<ToolRuntime>, Arc<BoundedToolScheduler>) {
    let mut registry = ToolRegistry::new();
    for provider in providers {
        registry.register(provider).expect("register provider");
    }
    let scheduler = Arc::new(BoundedToolScheduler::new(
        cfg.interactive_inflight,
        cfg.background_inflight,
        cfg.max_waiters,
        cfg.max_provider_inflight,
    ));
    let runtime = Arc::new(ToolRuntime::new(
        Arc::new(registry),
        policy,
        scheduler.clone(),
        cfg,
    ));
    (runtime, scheduler)
}

// ------------------------------------------------------------
// Registry
// ------------------------------------------------------------

#[test]
fn registry_lists_descriptors() {
    let mut registry = ToolRegistry::new();
    registry
        .register(Arc::new(EchoProvider::new("echo.echo", "echo", 0)))
        .expect("register");
    let descriptors = registry.descriptors();
    assert_eq!(descriptors.len(), 1);
    assert_eq!(descriptors[0].name, "echo.echo");
    assert_eq!(descriptors[0].provider, "echo");
}

/// Resolution must hand back **the same descriptor allocation**, not a fresh
/// clone per call.
///
/// WHY "same heap buffer for `name`" instead of `Arc::ptr_eq(&a, &b)`: the point
/// is "no new heap work per call", and buffer identity is exactly that — it
/// compiles and means the same thing whether `resolve` returns
/// `ToolDescriptor` or `Arc<ToolDescriptor>`, so the lock survives the very API
/// change it is here to police (`Arc::ptr_eq` would not have compiled against
/// the clone-returning version at all, so there would have been no red reading
/// to record). `String::clone` always allocates, so a shared `name` buffer can
/// only mean a shared descriptor.
///
/// The provider assertion is the **control**: it already held before this
/// change (the `Arc<dyn ToolProvider>` was cloned, not the descriptor), so it
/// proves the fixture resolves at all and that the test is not vacuously red.
#[test]
fn resolve_returns_the_same_descriptor_allocation() {
    let mut registry = ToolRegistry::new();
    registry
        .register(Arc::new(EchoProvider::new("echo.echo", "echo", 0)))
        .expect("register");

    let (provider_a, desc_a) = registry.resolve("echo.echo").expect("resolve a");
    let (provider_b, desc_b) = registry.resolve("echo.echo").expect("resolve b");

    assert!(
        Arc::ptr_eq(&provider_a, &provider_b),
        "control: the provider must be the same allocation"
    );
    assert_eq!(
        desc_a.name.as_ptr(),
        desc_b.name.as_ptr(),
        "resolve re-allocated the descriptor's `name` (4 Strings per call); it \
         must hand back the registered one — this is the hot-path cost W1b-1 \
         removes"
    );
    assert_eq!(
        desc_a.description.as_ptr(),
        desc_b.description.as_ptr(),
        "descriptor `description` was re-allocated per resolve"
    );
}

#[test]
fn duplicate_registration_rejected() {
    let mut registry = ToolRegistry::new();
    let provider: Arc<dyn ToolProvider> = Arc::new(EchoProvider::new("echo.echo", "echo", 0));
    registry.register(provider.clone()).expect("first");
    let err = registry.register(provider).expect_err("duplicate");
    assert!(matches!(err, ToolError::InvalidArgs(_)));
}

// ------------------------------------------------------------
// Runtime outcomes
// ------------------------------------------------------------

#[tokio::test]
async fn runtime_invoke_unknown_tool_is_not_found() {
    let (runtime, _) = runtime_with(vec![], None, ToolRuntimeConfig::default());
    let outcome = runtime
        .invoke(inv("missing.tool"), &CancellationToken::new())
        .await;
    assert_eq!(outcome.status, ToolStatus::NotFound);
    assert!(outcome.payload.is_none());
    assert!(outcome.error.is_some());
}

#[tokio::test]
async fn policy_deny_is_outcome_not_panic() {
    let echo: Arc<dyn ToolProvider> = Arc::new(EchoProvider::new("echo.echo", "echo", 0));
    let (runtime, _) = runtime_with(
        vec![echo],
        Some(Arc::new(ExecPolicy::new())), // zero-trust: deny by default
        ToolRuntimeConfig::default(),
    );
    let outcome = runtime
        .invoke(inv("echo.echo"), &CancellationToken::new())
        .await;
    assert_eq!(outcome.status, ToolStatus::Denied);
    assert!(outcome.error.unwrap().contains("denied"));
}

#[tokio::test]
async fn ask_without_approval_is_ask_required_and_approved_executes() {
    let policy = Arc::new(ExecPolicy::new().add_rule(ExecPolicyRule {
        pattern: RulePattern::new("echo.echo", "*"),
        action: PolicyAction::Ask,
    }));
    let echo = Arc::new(EchoProvider::new("echo.echo", "echo", 0));
    let (runtime, _) = runtime_with(
        vec![echo.clone()],
        Some(policy),
        ToolRuntimeConfig::default(),
    );

    let denied = runtime
        .invoke(inv("echo.echo"), &CancellationToken::new())
        .await;
    assert_eq!(denied.status, ToolStatus::AskRequired);
    assert_eq!(echo.calls(), 0, "ask without approval must not execute");

    let mut approved = inv("echo.echo");
    approved.approved = true;
    let ok = runtime.invoke(approved, &CancellationToken::new()).await;
    assert_eq!(ok.status, ToolStatus::Ok);
    assert_eq!(echo.calls(), 1);
}

#[tokio::test]
async fn payload_truncated_on_utf8_boundary() {
    let (runtime, _) = runtime_with(
        vec![Arc::new(BigProvider)],
        None,
        ToolRuntimeConfig {
            max_payload_bytes: 5,
            ..Default::default()
        },
    );
    let outcome = runtime
        .invoke(inv("big.big"), &CancellationToken::new())
        .await;
    assert_eq!(outcome.status, ToolStatus::Ok);
    assert!(outcome.truncated, "payload over budget must be truncated");
    assert_eq!(outcome.payload.as_deref(), Some("你"));
}

// ------------------------------------------------------------
// Scheduler
// ------------------------------------------------------------

#[tokio::test]
async fn scheduler_bounds_inflight_per_lane() {
    let echo = Arc::new(EchoProvider::new("echo.echo", "echo", 25));
    let (runtime, _) = runtime_with(
        vec![echo.clone()],
        None,
        ToolRuntimeConfig {
            interactive_inflight: 2,
            background_inflight: 2,
            max_waiters: 32,
            max_provider_inflight: 8,
            ..Default::default()
        },
    );

    let mut handles = Vec::new();
    for i in 0..8 {
        let runtime = runtime.clone();
        let token = CancellationToken::new();
        handles.push(tokio::spawn(async move {
            let mut invocation = inv("echo.echo");
            invocation.call_id = format!("c-{i}");
            runtime.invoke(invocation, &token).await
        }));
    }
    for handle in handles {
        let outcome = handle.await.expect("join");
        assert_eq!(outcome.status, ToolStatus::Ok);
    }
    assert!(
        echo.peak() <= 2,
        "provider peak must respect background lane bound, peak={}",
        echo.peak()
    );
}

#[tokio::test]
async fn waiter_limit_returns_overloaded() {
    let scheduler = BoundedToolScheduler::new(1, 1, 0, 1);
    let err = scheduler
        .acquire(
            &inv("p.t"),
            &descriptor("p.t", "p"),
            &CancellationToken::new(),
        )
        .await
        .expect_err("waiter limit 0 must reject");
    assert!(matches!(err, ToolError::Overloaded { .. }));
}

#[tokio::test]
async fn cancel_before_permit_returns_cancelled() {
    let scheduler = BoundedToolScheduler::new(1, 1, 8, 1);
    let first = scheduler
        .acquire(
            &inv("p.t1"),
            &descriptor("p.t1", "p"),
            &CancellationToken::new(),
        )
        .await
        .expect("first permit");

    let token = CancellationToken::new();
    token.cancel();
    let err = scheduler
        .acquire(&inv("p.t2"), &descriptor("p.t2", "p"), &token)
        .await
        .expect_err("cancelled waiter");
    assert!(matches!(err, ToolError::Cancelled(_)));
    drop(first);
}

// ------------------------------------------------------------
// Batch
// ------------------------------------------------------------

#[tokio::test]
async fn batch_preserves_input_order() {
    let echo: Arc<dyn ToolProvider> = Arc::new(EchoProvider::new("echo.echo", "echo", 0));
    let (runtime, _) = runtime_with(vec![echo], None, ToolRuntimeConfig::default());
    let invocations = (0..8)
        .map(|i| {
            let mut invocation = inv("echo.echo");
            invocation.call_id = format!("c-{i}");
            invocation.args_json = i.to_string();
            invocation
        })
        .collect();
    let outcomes = runtime
        .invoke_batch(invocations, &CancellationToken::new())
        .await;
    assert_eq!(outcomes.len(), 8);
    for (i, outcome) in outcomes.iter().enumerate() {
        let expected = i.to_string();
        assert_eq!(outcome.payload.as_deref(), Some(expected.as_str()));
        assert_eq!(outcome.call_id, format!("c-{i}"));
    }
}

#[tokio::test]
async fn batch_bounds_inflight_and_keeps_failures_isolated() {
    let echo = Arc::new(EchoProvider::new("echo.echo", "echo", 20));
    let fail = Arc::new(FailProvider);
    let (runtime, _) = runtime_with(
        vec![echo.clone(), fail],
        None,
        ToolRuntimeConfig {
            background_inflight: 2,
            max_waiters: 32,
            ..Default::default()
        },
    );
    let invocations = (0..6)
        .map(|i| {
            let mut invocation = inv("echo.echo");
            invocation.call_id = format!("c-{i}");
            invocation.args_json = i.to_string();
            invocation
        })
        .chain(std::iter::once(inv("fail.fail")))
        .collect();
    let outcomes = runtime
        .invoke_batch(invocations, &CancellationToken::new())
        .await;
    assert_eq!(outcomes.len(), 7);
    assert_eq!(outcomes[6].status, ToolStatus::ProviderError);
    assert!(outcomes[..6]
        .iter()
        .all(|outcome| outcome.status == ToolStatus::Ok));
    assert!(
        echo.peak() <= 2,
        "batch must respect scheduler bound, peak={}",
        echo.peak()
    );
}

// ------------------------------------------------------------
// Metrics / compatibility
// ------------------------------------------------------------

#[tokio::test]
async fn metrics_counts_statuses() {
    let (runtime, _) = runtime_with(vec![], None, ToolRuntimeConfig::default());
    let _ = runtime
        .invoke(inv("missing.tool"), &CancellationToken::new())
        .await;
    let metrics = runtime.metrics();
    assert_eq!(metrics.invocations, 1);
    assert_eq!(metrics.failed, 1);
    assert_eq!(metrics.ok, 0);
    assert_eq!(metrics.in_flight, 0);
}

#[test]
fn runtime_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<ToolRuntime>();
}

#[tokio::test]
async fn legacy_tool_executor_error_is_plain_text() {
    let echo: Arc<dyn ToolProvider> = Arc::new(EchoProvider::new("echo.echo", "echo", 0));
    let (runtime, _) = runtime_with(
        vec![echo],
        Some(Arc::new(ExecPolicy::new())),
        ToolRuntimeConfig::default(),
    );
    let err = runtime
        .execute("echo.echo", "{}")
        .await
        .expect_err("policy deny must surface an error");
    assert!(err.contains("denied"), "unexpected error text: {err}");
    assert!(
        !err.trim_start().starts_with('{'),
        "legacy ToolExecutor error must stay plain text: {err}"
    );
}
