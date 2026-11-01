//! `ToolRuntime::run_plan` must hand the **full** invocation to providers.
//!
//! Why this file exists: the plan path used to go through a compatibility
//! adapter that fabricated its own `ToolInvocation` — `approved: false`,
//! `call_id: "compat-{tool}"`, and a brand-new `CancellationToken` per node.
//! Every existing `run`/`run_with` test drove a `Mem`/`Peak` fixture through the
//! legacy `ToolExecutor` trait, so **no test ever ran `run_plan` against a real
//! provider** and the fabrication was structurally invisible. These tests pin the
//! four things the adapter was throwing away:
//!
//! 1. `approved` is derived from the node's declared side effect plus the
//!    explicit `approve_writes` option — fail-closed when neither says yes;
//! 2. a denied write node never reaches the provider;
//! 3. `call_id` is unique per node, so two nodes calling the same tool stay
//!    distinguishable in outcomes and metrics;
//! 4. the caller's cancellation token actually reaches the provider.
//!
//! Everything here is driven by `yield_now`, not wall-clock sleeps: this host's
//! clock granularity is 1/64 s, so a sleep-based fixture would be flaky or would
//! assert nothing at all.

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use gqep_executor::tool_plane::{
    BoundedToolScheduler, ToolDescriptor, ToolError, ToolInvocation, ToolProvider, ToolRegistry,
    ToolRuntime, ToolRuntimeConfig,
};
use gqep_executor::PlanRunOptions;
use nexus_contracts::tool_plan::{PlanEdge, SideEffectDecl, ToolNode, ToolOp, ToolPlan};
use tokio::task::yield_now;
use tokio_util::sync::CancellationToken;

/// Records what the provider actually received, so assertions are about the
/// invocation the runtime built — not about what a caller printed afterwards.
#[derive(Default)]
struct Seen {
    invocations: Mutex<Vec<ToolInvocation>>,
    /// The cancellation token the provider was actually handed — kept so a test
    /// can check its lineage against the caller's token after the run.
    token: Mutex<Option<CancellationToken>>,
    calls: AtomicUsize,
}

struct RecordingProvider {
    seen: Arc<Seen>,
    write: bool,
}

#[async_trait]
impl ToolProvider for RecordingProvider {
    fn provider_id(&self) -> &str {
        if self.write {
            "wrec"
        } else {
            "rrec"
        }
    }

    fn descriptors(&self) -> Vec<ToolDescriptor> {
        let provider = self.provider_id();
        vec![ToolDescriptor {
            name: format!("{provider}.tool"),
            provider: provider.to_string(),
            description: "recording test tool".to_string(),
            side_effect: if self.write {
                SideEffectDecl::Write
            } else {
                SideEffectDecl::ReadOnly
            },
            timeout_ms: 5_000,
            max_concurrency: 4,
            input_schema: None,
        }]
    }

    async fn invoke(
        &self,
        inv: &ToolInvocation,
        cancel: CancellationToken,
    ) -> Result<String, ToolError> {
        // Mirrors what the real write providers do (`process.rs` / `mcp.rs` both
        // start with `if !inv.approved { AskRequired }`). The runtime's own Ask
        // branch sits behind an `ExecPolicy`, and production assembles the
        // runtime with `policy: None` — so `inv.approved` is the only live
        // approval signal today. This provider must enforce it for the test to
        // be about that signal at all.
        if self.write && !inv.approved {
            return Err(ToolError::AskRequired(format!(
                "execpolicy needs approval (tool={})",
                inv.tool
            )));
        }
        self.seen.calls.fetch_add(1, Ordering::SeqCst);
        {
            let mut guard = self
                .seen
                .token
                .lock()
                .map_err(|_| ToolError::Provider("token log poisoned".into()))?;
            if guard.is_none() {
                *guard = Some(cancel.clone());
            }
        }
        self.seen
            .invocations
            .lock()
            .map(|mut guard| guard.push(inv.clone()))
            .map_err(|_| ToolError::Provider("seen log poisoned".into()))?;
        yield_now().await;
        Ok(format!(r#"{{"via":"{}"}}"#, inv.call_id))
    }
}

fn node(id: &str, tool: &str, write: bool) -> ToolNode {
    ToolNode {
        id: id.to_string(),
        op: ToolOp::ToolCall,
        tool_name: Some(tool.to_string()),
        args_json: Some("{}".to_string()),
        field: None,
        predicate: None,
        side_effect: Some(if write {
            SideEffectDecl::Write
        } else {
            SideEffectDecl::ReadOnly
        }),
    }
}

fn plan(id: &str, nodes: Vec<ToolNode>, edges: Vec<(&str, &str)>) -> ToolPlan {
    ToolPlan {
        id: id.to_string(),
        nodes,
        edges: edges
            .into_iter()
            .map(|(from, to)| PlanEdge {
                from: from.to_string(),
                to: to.to_string(),
            })
            .collect(),
    }
}

/// A real `ToolRuntime` with a real provider — that pairing is the point.
fn runtime_with(write: bool) -> (Arc<ToolRuntime>, Arc<Seen>) {
    let seen = Arc::new(Seen::default());
    let provider: Arc<dyn ToolProvider> = Arc::new(RecordingProvider {
        seen: Arc::clone(&seen),
        write,
    });
    let mut registry = ToolRegistry::new();
    registry.register(provider).expect("register provider");
    let scheduler = Arc::new(BoundedToolScheduler::new(8, 8, 64, 8));
    (
        Arc::new(ToolRuntime::new(
            Arc::new(registry),
            None,
            scheduler,
            ToolRuntimeConfig::default(),
        )),
        seen,
    )
}

fn options(approve_writes: bool) -> PlanRunOptions {
    PlanRunOptions {
        max_parallel: 4,
        // fail-fast so a denied node surfaces as a plan error instead of being
        // written into the environment as a {"error":...} value.
        fail_fast: true,
        approve_writes,
        ..PlanRunOptions::default()
    }
}

fn first(seen: &Arc<Seen>) -> ToolInvocation {
    let guard = seen.invocations.lock().expect("log");
    guard.first().cloned().expect("provider was never called")
}

/// A `Write` node is denied unless `approve_writes` says yes — and when it does,
/// the provider must actually see `approved: true` rather than a fabricated
/// `false`.
#[tokio::test]
async fn write_node_denied_without_approval_and_threads_it_with() {
    // (a) fail-closed default.
    let (runtime, seen) = runtime_with(true);
    let error = runtime
        .run_plan(
            &plan("p-deny", vec![node("n1", "wrec.tool", true)], vec![]),
            options(false),
        )
        .await
        .expect_err("a Write node without approve_writes must not succeed");
    let text = format!("{error:?}");
    assert!(
        text.contains("ask_required"),
        "expected an ask_required denial, got {text}"
    );
    assert_eq!(
        seen.calls.load(Ordering::SeqCst),
        0,
        "a node denied at the runtime gate must never reach the provider"
    );

    // (b) explicit approval reaches the provider instead of being dropped.
    let (runtime, seen) = runtime_with(true);
    let summary = runtime
        .run_plan(
            &plan("p-ok", vec![node("n1", "wrec.tool", true)], vec![]),
            options(true),
        )
        .await
        .expect("Write node with approve_writes must execute");
    assert_eq!(summary.nodes_executed, 1);
    assert_eq!(seen.calls.load(Ordering::SeqCst), 1);
    assert!(
        first(&seen).approved,
        "approve_writes must land on the invocation the provider sees"
    );
}

/// Read-only nodes run without any approval option.
///
/// Note the expected value of `approved`: the convention in this plane is that
/// `approved` means "there is no outstanding approval requirement for this
/// call" — the LLM loop likewise sets it to `true` for every call it dispatches.
/// A read-only node satisfies that trivially; it is not "auto-approving a write".
#[tokio::test]
async fn read_only_node_needs_no_approval() {
    let (runtime, seen) = runtime_with(false);
    let summary = runtime
        .run_plan(
            &plan("p-read", vec![node("n1", "rrec.tool", false)], vec![]),
            options(false),
        )
        .await
        .expect("read-only plan runs");
    assert_eq!(summary.nodes_executed, 1);
    assert!(
        first(&seen).approved,
        "read-only should carry no outstanding approval requirement"
    );
}

/// Two nodes calling the *same* tool must stay distinguishable. The old adapter
/// emitted `compat-{tool}` for both, so outcomes and metrics collided.
#[tokio::test]
async fn call_ids_are_unique_per_node_not_per_tool() {
    let (runtime, seen) = runtime_with(false);
    runtime
        .run_plan(
            &plan(
                "p-two",
                vec![node("a", "rrec.tool", false), node("b", "rrec.tool", false)],
                vec![("a", "b")],
            ),
            options(false),
        )
        .await
        .expect("two-node plan runs");
    let ids: Vec<String> = seen
        .invocations
        .lock()
        .expect("log")
        .iter()
        .map(|inv| inv.call_id.clone())
        .collect();
    assert_eq!(ids.len(), 2);
    assert_ne!(
        ids[0], ids[1],
        "call_id must carry the node, not only the tool: {ids:?}"
    );
    assert!(
        ids.iter().all(|id| id.starts_with("p-two-")),
        "call_id should be traceable to plan and node: {ids:?}"
    );
}

/// The caller's cancellation token must be the provider's token **or an
/// ancestor of it**. The old adapter invented a fresh root token per node, so
/// the caller could cancel all day and the provider would never notice.
///
/// Asserted by lineage rather than by racing a mid-flight cancel: cancel the
/// caller's token *after* the run and check what the provider was handed. A
/// descendant of it flips to cancelled; an independently invented root token
/// does not. This keeps the test deterministic on a host whose clock granularity
/// is 1/64 s.
#[tokio::test]
async fn provider_token_descends_from_the_callers_token() {
    let (runtime, seen) = runtime_with(false);
    let cancel = CancellationToken::new();
    let opts = PlanRunOptions {
        cancel: Some(cancel.clone()),
        ..options(false)
    };
    runtime
        .run_plan(
            &plan("p-lineage", vec![node("n1", "rrec.tool", false)], vec![]),
            opts,
        )
        .await
        .expect("plan runs");

    let handed = seen
        .token
        .lock()
        .expect("log")
        .clone()
        .expect("provider never recorded a token");
    assert!(
        !handed.is_cancelled() && !cancel.is_cancelled(),
        "precondition: nothing cancelled yet"
    );
    cancel.cancel();
    assert!(
        handed.is_cancelled(),
        "the provider's token is not linked to the caller's — plan-level cancel cannot reach it"
    );
}

/// An already-cancelled plan must not report success.
#[tokio::test]
async fn already_cancelled_plan_does_not_succeed() {
    let (runtime, _seen) = runtime_with(false);
    let cancel = CancellationToken::new();
    cancel.cancel();
    let opts = PlanRunOptions {
        cancel: Some(cancel),
        ..options(false)
    };
    let error = runtime
        .run_plan(
            &plan("p-cancel", vec![node("n1", "rrec.tool", false)], vec![]),
            opts,
        )
        .await
        .expect_err("an already-cancelled plan must not report success");
    assert!(
        format!("{error:?}").contains("Cancelled"),
        "expected a cancellation error, got {error:?}"
    );
}
