//! PlanRunner bounded-parallelism, deadline, cancellation, budget and
//! fail-fast contracts (Phase 2).

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom
#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use gqep_executor::{PlanGuards, PlanRunError, PlanRunOptions, PlanRunner, ToolExecutor};
use nexus_contracts::tool_plan::{PlanEdge, SideEffectDecl, ToolNode, ToolOp, ToolPlan};
use tokio_util::sync::CancellationToken;

struct PeakExecutor {
    sleep_ms: u64,
    fail_on: Option<&'static str>,
    calls: AtomicUsize,
    current: AtomicUsize,
    peak: AtomicUsize,
}

impl PeakExecutor {
    fn new(sleep_ms: u64, fail_on: Option<&'static str>) -> Self {
        Self {
            sleep_ms,
            fail_on,
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
impl ToolExecutor for PeakExecutor {
    async fn execute(&self, tool_name: &str, _args_json: &str) -> Result<String, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail_on == Some(tool_name) {
            return Err("boom".to_string());
        }
        let current = self.current.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(current, Ordering::SeqCst);
        if self.sleep_ms > 0 {
            tokio::time::sleep(Duration::from_millis(self.sleep_ms)).await;
        }
        self.current.fetch_sub(1, Ordering::SeqCst);
        Ok(format!(r#"{{"tool":"{tool_name}"}}"#))
    }
}

/// `PlanRunner` owns `Box<dyn ToolExecutor>`; this adapter keeps a handle for assertions.
struct Shared<T>(Arc<T>);

#[async_trait]
impl<T> ToolExecutor for Shared<T>
where
    T: ToolExecutor + Send + Sync + 'static,
{
    async fn execute(&self, tool_name: &str, args_json: &str) -> Result<String, String> {
        self.0.execute(tool_name, args_json).await
    }
}

fn call_node(id: &str, tool: &str) -> ToolNode {
    ToolNode {
        id: id.to_string(),
        op: ToolOp::ToolCall,
        tool_name: Some(tool.to_string()),
        args_json: Some("{}".to_string()),
        field: None,
        predicate: None,
        side_effect: Some(SideEffectDecl::ReadOnly),
    }
}

fn plan_with_nodes(nodes: Vec<ToolNode>, edges: Vec<PlanEdge>) -> ToolPlan {
    ToolPlan {
        id: "test-plan".to_string(),
        nodes,
        edges,
    }
}

fn linear_plan() -> ToolPlan {
    plan_with_nodes(
        vec![call_node("a", "tool"), call_node("b", "tool")],
        vec![PlanEdge {
            from: "a".to_string(),
            to: "b".to_string(),
        }],
    )
}

fn runner(exec: Arc<PeakExecutor>, guards: PlanGuards) -> PlanRunner {
    PlanRunner::new(Box::new(Shared(exec)), guards)
}

#[tokio::test]
async fn layer_parallelism_never_exceeds_bound() {
    let exec = Arc::new(PeakExecutor::new(10, None));
    let runner = runner(
        Arc::clone(&exec),
        PlanGuards {
            max_parallel: 4,
            ..Default::default()
        },
    );
    let nodes = (0..16)
        .map(|i| call_node(&format!("n{i}"), "tool"))
        .collect();
    let plan = plan_with_nodes(nodes, vec![]);
    let summary = runner
        .run_with(
            &plan,
            &PlanRunOptions {
                max_parallel: 4,
                timeout_ms: 5_000,
                ..Default::default()
            },
        )
        .await
        .expect("run");
    assert_eq!(summary.nodes_executed, 16);
    assert!(exec.peak() <= 4, "peak={} must be <= 4", exec.peak());
}

#[tokio::test]
async fn deadline_cancels_inflight() {
    let exec = Arc::new(PeakExecutor::new(10_000, None));
    let runner = runner(exec, PlanGuards::default());
    let plan = plan_with_nodes(vec![call_node("slow", "tool")], vec![]);
    let result = runner
        .run_with(
            &plan,
            &PlanRunOptions {
                timeout_ms: 50,
                ..Default::default()
            },
        )
        .await;
    assert!(
        matches!(result, Err(PlanRunError::TimedOut { .. })),
        "expected TimedOut, got {result:?}"
    );
}

#[tokio::test]
async fn cancel_token_aborts_layer() {
    let exec = Arc::new(PeakExecutor::new(10_000, None));
    let runner = runner(exec, PlanGuards::default());
    let plan = plan_with_nodes(vec![call_node("slow", "tool")], vec![]);
    let token = CancellationToken::new();
    let cancel_token = token.clone();
    let canceller = tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(20)).await;
        cancel_token.cancel();
    });
    let result = runner
        .run_with(
            &plan,
            &PlanRunOptions {
                timeout_ms: 5_000,
                cancel: Some(token),
                ..Default::default()
            },
        )
        .await;
    canceller.await.expect("canceller");
    assert!(
        matches!(result, Err(PlanRunError::Cancelled { .. })),
        "expected Cancelled, got {result:?}"
    );
}

#[tokio::test]
async fn budget_tracks_incrementally() {
    let exec = Arc::new(PeakExecutor::new(0, None));
    let runner = runner(
        exec,
        PlanGuards {
            budget_bytes: 20,
            ..Default::default()
        },
    );
    let nodes = (0..4)
        .map(|i| call_node(&format!("n{i}"), "tool"))
        .collect();
    let plan = plan_with_nodes(nodes, vec![]);
    let summary = runner
        .run_with(
            &plan,
            &PlanRunOptions {
                max_parallel: 1,
                ..Default::default()
            },
        )
        .await
        .expect("run");
    assert!(summary.truncated, "budget overrun must set truncated");
    assert!(
        summary.nodes_executed < 4,
        "budget must stop scheduling after overrun, executed={}",
        summary.nodes_executed
    );
}

#[tokio::test]
async fn fail_fast_skips_dependents() {
    let exec = Arc::new(PeakExecutor::new(0, Some("fail")));
    let runner = runner(exec.clone(), PlanGuards::default());
    let plan = plan_with_nodes(
        vec![call_node("a", "fail"), call_node("b", "tool")],
        vec![PlanEdge {
            from: "a".to_string(),
            to: "b".to_string(),
        }],
    );
    let result = runner
        .run_with(
            &plan,
            &PlanRunOptions {
                fail_fast: true,
                ..Default::default()
            },
        )
        .await;
    match result {
        Err(PlanRunError::NodeFailed { node_id, .. }) => assert_eq!(node_id, "a"),
        other => panic!("expected NodeFailed(a), got {other:?}"),
    }
    assert_eq!(
        exec.calls(),
        1,
        "dependent must not execute after fail-fast"
    );
}

#[tokio::test]
async fn summary_joins_all_terminal_nodes() {
    let exec = Arc::new(PeakExecutor::new(0, None));
    let runner = runner(exec, PlanGuards::default());
    let plan = plan_with_nodes(vec![call_node("a", "tool"), call_node("b", "tool")], vec![]);
    let summary = runner
        .run_with(
            &plan,
            &PlanRunOptions {
                max_parallel: 2,
                ..Default::default()
            },
        )
        .await
        .expect("run");
    assert!(
        summary.summary.contains("\"terminals\""),
        "multi-terminal summary must be keyed, got {}",
        summary.summary
    );
    assert!(summary.summary.contains("\"a\""));
    assert!(summary.summary.contains("\"b\""));
}

#[tokio::test]
async fn legacy_run_summary_unchanged() {
    let exec = Arc::new(PeakExecutor::new(0, None));
    let runner = runner(exec, PlanGuards::default());
    let plan = linear_plan();
    let legacy = runner.run(&plan).await.expect("legacy run");
    let new = runner
        .run_with(
            &plan,
            &PlanRunOptions {
                max_parallel: 1,
                ..Default::default()
            },
        )
        .await
        .expect("run_with");
    assert_eq!(legacy.summary, new.summary);
    assert_eq!(legacy.tool_calls, new.tool_calls);
    assert_eq!(legacy.nodes_executed, new.nodes_executed);
    assert_eq!(legacy.truncated, new.truncated);
}

#[tokio::test]
async fn invalid_plan_maps_to_invalid_error() {
    let exec = Arc::new(PeakExecutor::new(0, None));
    let runner = runner(exec, PlanGuards::default());
    let plan = plan_with_nodes(
        vec![call_node("a", "tool")],
        vec![PlanEdge {
            from: "a".to_string(),
            to: "ghost".to_string(),
        }],
    );
    let result = runner.run_with(&plan, &PlanRunOptions::default()).await;
    assert!(matches!(result, Err(PlanRunError::Invalid(_))));
}
