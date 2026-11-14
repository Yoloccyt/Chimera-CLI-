//! Dependency-aware dispatch (`run_with`) — characterization + invariants + properties.
//!
//! Why these tests exist: `benches/tool_plan.rs`'s original two groups had
//! `edges: vec![]`, so they measured only the intra-layer window and could not
//! see the **layer barrier** at all. `tool_plan_edges/chain_plus_slow_sibling`
//! (110.35 ms before this change) shows the cost: a chain whose own predecessor
//! is finished still waits for an unrelated slow node that happens to share its
//! layer.
//!
//! The tests below assert *who has started* rather than elapsed time, because
//! this host's timer granularity is 1/64 s (measured in Phase 7), which makes
//! any wall-clock bound either flaky or vacuous.
//!
//! Two halves, deliberately:
//! - `started`/`finished` logs prove **liveness** (a node is dispatched as soon
//!   as its predecessors are committed — a barrier cannot show this);
//! - start order over every edge proves **safety** (`dispatch(n)` only after
//!   each predecessor was dispatched, and the child's input is complete —
//!   `inputs_of_precomputed`'s `debug_assert_eq!` fires in this profile if it
//!   is not, so the property tests below run with that check live).

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use gqep_executor::{
    PlanGuards, PlanRunError, PlanRunOptions, PlanRunner, PlanSummary, ToolExecutor,
};
use nexus_contracts::tool_plan::{PlanEdge, SideEffectDecl, ToolNode, ToolOp, ToolPlan};
use proptest::prelude::*;
use tokio::sync::Semaphore;
use tokio::task::yield_now;

/// Records start/finish events in the order they happened, and can hold one
/// node open so "has X started yet" is observable instead of inferred.
#[derive(Default)]
struct Trace {
    events: Mutex<Vec<(&'static str, String)>>,
    /// Gate for the deliberately-unfinished node; `None` = nothing gated.
    /// Held in a std `Mutex` and cloned out *before* any await (§4.4 #1).
    gate: Mutex<Option<Arc<Semaphore>>>,
}

impl Trace {
    fn record(&self, kind: &'static str, id: &str) {
        if let Ok(mut guard) = self.events.lock() {
            guard.push((kind, id.to_string()));
        }
    }

    fn seen(&self, kind: &str, id: &str) -> bool {
        self.order(kind).iter().any(|seen| seen == id)
    }

    /// Ids in the order they emitted `kind` — this is the dispatch order, since
    /// the executor marks the moment its tool call begins.
    fn order(&self, kind: &str) -> Vec<String> {
        self.events
            .lock()
            .map(|guard| {
                guard
                    .iter()
                    .filter(|(event, _)| *event == kind)
                    .map(|(_, id)| id.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn times(&self, kind: &str, id: &str) -> usize {
        self.events
            .lock()
            .map(|guard| {
                guard
                    .iter()
                    .filter(|(event, node)| *event == kind && node == id)
                    .count()
            })
            .unwrap_or(0)
    }

    fn position(&self, kind: &str, id: &str) -> Option<usize> {
        self.order(kind).iter().position(|seen| seen == id)
    }

    fn gate(&self) -> Option<Arc<Semaphore>> {
        self.gate.lock().ok().and_then(|guard| guard.clone())
    }

    fn set_gate(&self, permits: usize) {
        if let Ok(mut guard) = self.gate.lock() {
            *guard = Some(Arc::new(Semaphore::new(permits)));
        }
    }

    fn release(&self) {
        if let Some(sem) = self.gate() {
            sem.add_permits(8);
        }
    }
}

struct TracingExecutor {
    trace: Arc<Trace>,
    /// Node whose tool call waits for the gate before returning.
    gated: Option<String>,
}

#[async_trait]
impl ToolExecutor for TracingExecutor {
    async fn execute(&self, tool_name: &str, _args_json: &str) -> Result<String, String> {
        // tool_name carries the node id (see `node`), the only channel the trait
        // exposes for per-node behaviour.
        self.trace.record("start", tool_name);
        if self.gated.as_deref() == Some(tool_name) {
            if let Some(sem) = self.trace.gate() {
                // `sem` is an owned Arc: no lock is held across this await.
                if let Ok(permit) = sem.acquire_owned().await {
                    drop(permit);
                }
            }
        }
        // Two yields so that "started" also means "had a chance to run".
        yield_now().await;
        yield_now().await;
        self.trace.record("finish", tool_name);
        Ok(value_for(tool_name))
    }
}

/// The payload every node hands back — shared with the budget property so the
/// test computes the true total rather than trusting the runner's bookkeeping.
fn value_for(id: &str) -> String {
    format!(r#"{{"node":"{id}"}}"#)
}

fn node(id: &str) -> ToolNode {
    ToolNode {
        id: id.to_string(),
        op: ToolOp::ToolCall,
        tool_name: Some(id.to_string()),
        args_json: Some("{}".to_string()),
        field: None,
        predicate: None,
        side_effect: Some(SideEffectDecl::ReadOnly),
    }
}

fn plan(nodes: &[&str], edges: &[(&str, &str)]) -> ToolPlan {
    ToolPlan {
        id: "eager".to_string(),
        nodes: nodes.iter().map(|id| node(id)).collect(),
        edges: edges
            .iter()
            .map(|(from, to)| PlanEdge {
                from: (*from).to_string(),
                to: (*to).to_string(),
            })
            .collect(),
    }
}

fn runner(trace: Arc<Trace>, gated: Option<String>) -> PlanRunner {
    PlanRunner::new(
        Box::new(TracingExecutor { trace, gated }),
        PlanGuards::default(),
    )
}

/// Advance a spawned plan without wall-clock waits.
async fn pump(task: &mut tokio::task::JoinHandle<Result<(), PlanRunError>>, ticks: usize) {
    for _ in 0..ticks {
        yield_now().await;
        if task.is_finished() {
            return;
        }
    }
}

/// `a1 → a2`, plus an unrelated `slow` node sharing a1's layer that never
/// finishes until released.
///
/// A layer barrier cannot start a2 while `slow` holds layer 0 open.
/// Dependency-aware dispatch can: a2's only predecessor is already committed.
#[tokio::test]
async fn dependent_starts_without_waiting_for_unrelated_slow_sibling() {
    let trace = Arc::new(Trace::default());
    trace.set_gate(0);
    let plan = plan(&["a1", "slow", "a2"], &[("a1", "a2")]);
    let runner = runner(Arc::clone(&trace), Some("slow".to_string()));
    let observe = Arc::clone(&trace);

    let mut task = tokio::spawn(async move {
        runner
            .run_with(
                &plan,
                &PlanRunOptions {
                    max_parallel: 8,
                    ..Default::default()
                },
            )
            .await
            .map(|_| ())
    });
    pump(&mut task, 64).await;

    assert!(
        observe.seen("start", "a1"),
        "precondition: a1 must have run"
    );
    assert!(
        observe.seen("start", "slow"),
        "precondition: the slow sibling must be in flight"
    );
    assert!(
        observe.seen("start", "a2"),
        "a2's only predecessor is committed, yet a2 never started — that is a \
         layer barrier waiting on an unrelated node"
    );
    assert!(
        !observe.seen("finish", "slow"),
        "the gated sibling must still be holding its layer open"
    );

    // Open the gate so the plan finishes instead of leaking a pending task.
    trace.release();
    task.await
        .expect("plan task joins")
        .expect("plan completes once the gate opens");
}

/// Two chains in lockstep: the fast one must *finish* while the slow one is
/// still stuck on its first node. Mirrors `tool_plan_edges/two_chains_lockstep`
/// structurally — under a layer barrier `a2` cannot even be dispatched until
/// `b1` commits, so the whole `a` chain would still be open here.
#[tokio::test]
async fn fast_chain_finishes_while_slow_chain_is_stuck() {
    let trace = Arc::new(Trace::default());
    trace.set_gate(0);
    let shape = plan(
        &["b1", "a1", "a2", "a3", "b2"],
        &[("a1", "a2"), ("a2", "a3"), ("b1", "b2")],
    );
    // `b1` is declared first so a layer barrier seeds it into layer 0 ahead of
    // the whole `a` chain.
    let runner = runner(Arc::clone(&trace), Some("b1".to_string()));
    let observe = Arc::clone(&trace);

    let mut task = tokio::spawn(async move {
        runner
            .run_with(
                &shape,
                &PlanRunOptions {
                    max_parallel: 8,
                    ..Default::default()
                },
            )
            .await
            .map(|_| ())
    });
    pump(&mut task, 128).await;

    assert!(
        observe.seen("finish", "a3"),
        "a1/a2/a3 never ran to completion while b1 held layer 0 open — the          barrier costs the whole chain, not just one node"
    );
    assert!(
        !observe.seen("start", "b2"),
        "precondition: b1 must still be open, otherwise b2 proving nothing"
    );

    trace.release();
    task.await
        .expect("plan task joins")
        .expect("plan completes once the gate opens");
}

/// Safety half: a diamond sink depends on **both** branches, so it must not be
/// dispatched while either is open. Eager dispatch is not "dispatch early" —
/// a `remaining` that under-counts duplicate/multi edges would run `d` here.
#[tokio::test]
async fn diamond_sink_waits_for_both_branches() {
    let trace = Arc::new(Trace::default());
    trace.set_gate(0);
    let shape = plan(
        &["a", "slow_b", "c", "d"],
        &[("a", "slow_b"), ("a", "c"), ("slow_b", "d"), ("c", "d")],
    );
    let runner = runner(Arc::clone(&trace), Some("slow_b".to_string()));
    let observe = Arc::clone(&trace);

    let mut task = tokio::spawn(async move {
        runner
            .run_with(
                &shape,
                &PlanRunOptions {
                    max_parallel: 8,
                    ..Default::default()
                },
            )
            .await
            .map(|_| ())
    });
    pump(&mut task, 64).await;

    assert!(
        observe.seen("finish", "c"),
        "precondition: the fast branch must be committed"
    );
    assert!(
        !observe.seen("start", "d"),
        "d ran with only one of its two predecessors committed"
    );

    trace.release();
    task.await
        .expect("plan task joins")
        .expect("plan completes once the gate opens");
    assert!(observe.seen("finish", "d"), "d must run once both are in");
}

/// With `max_parallel == 1` eager dispatch degenerates to one sequential
/// linearization — that is where exact-content assertions belong, because under
/// concurrency the order of two simultaneously-ready nodes is deliberately
/// unspecified.
#[tokio::test]
async fn single_window_dispatch_is_deterministic() {
    let trace = Arc::new(Trace::default());
    let shape = plan(&["x", "y", "z"], &[("x", "z"), ("y", "z")]);
    let summary = runner(Arc::clone(&trace), None)
        .run_with(
            &shape,
            &PlanRunOptions {
                max_parallel: 1,
                ..Default::default()
            },
        )
        .await
        .expect("single-window plan completes");
    assert_eq!(summary.nodes_executed, 3);
    // The summary is the last node's value; with one window it is fully pinned.
    assert_eq!(summary.summary, value_for("z"));
    // Ready seeding is declaration order, not `HashMap` iteration order — the
    // barrier version seeded from a map, so same-layer order was unspecified.
    assert_eq!(trace.order("start"), ["x", "y", "z"]);
}

/// At one window a child must start strictly after its parent *finished*, not
/// merely after it started — that is the "committed before dispatch" half of the
/// linearization invariant, observable without peeking at `env`.
#[tokio::test]
async fn single_window_child_starts_after_parent_finishes() {
    let trace = Arc::new(Trace::default());
    let shape = plan(&["a", "b", "c"], &[("a", "b"), ("b", "c")]);
    runner(Arc::clone(&trace), None)
        .run_with(
            &shape,
            &PlanRunOptions {
                max_parallel: 1,
                ..Default::default()
            },
        )
        .await
        .expect("chain plan completes");

    let events = trace
        .events
        .lock()
        .expect("event log")
        .iter()
        .map(|(kind, id)| format!("{kind}:{id}"))
        .collect::<Vec<_>>();
    assert_eq!(
        events,
        ["start:a", "finish:a", "start:b", "finish:b", "start:c", "finish:c"]
    );
}

/// A node with two duplicate edges from the same predecessor must run exactly
/// once: `remaining` counts per edge occurrence, so the second decrement is what
/// crosses zero and a naive "is it zero now?" check would re-queue it.
#[tokio::test]
async fn duplicate_edges_do_not_dispatch_twice() {
    let trace = Arc::new(Trace::default());
    let shape = plan(&["p", "q"], &[("p", "q"), ("p", "q")]);
    let summary = runner(Arc::clone(&trace), None)
        .run_with(
            &shape,
            &PlanRunOptions {
                max_parallel: 4,
                ..Default::default()
            },
        )
        .await
        .expect("duplicate-edge plan completes");

    assert_eq!(summary.nodes_executed, 2, "each node runs once");
    assert_eq!(trace.times("start", "q"), 1, "q dispatched twice");
    assert_eq!(trace.times("finish", "q"), 1, "q executed twice");
}

/// Number of nodes in every generated plan — small enough that a full linear
/// search over the event log stays cheap in a property test.
const PROP_NODES: usize = 8;

/// Fixed node names, so generated index pairs map onto declaration order.
fn prop_names() -> Vec<String> {
    (0..PROP_NODES).map(|i| format!("n{i}")).collect()
}

/// Build a plan from index pairs. `from < to` is enforced by the generator, so
/// every shape is a DAG (`validate` would reject a cycle and the property would
/// assert nothing); duplicates and unreferenced nodes are kept on purpose.
fn prop_plan(edges: &[(usize, usize)]) -> ToolPlan {
    let names = prop_names();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let pairs: Vec<(&str, &str)> = edges
        .iter()
        .map(|&(from, to)| (names[from].as_str(), names[to].as_str()))
        .collect();
    plan(&refs, &pairs)
}

/// Run a plan to completion on a current-thread runtime, with the trace it
/// filled in. Deterministic because every executor step is `yield_now`, never a
/// sleep.
fn run_plan(
    shape: &ToolPlan,
    max_parallel: usize,
    budget_bytes: usize,
) -> (Arc<Trace>, Result<PlanSummary, PlanRunError>) {
    let trace = Arc::new(Trace::default());
    let runner = PlanRunner::new(
        Box::new(TracingExecutor {
            trace: Arc::clone(&trace),
            gated: None,
        }),
        PlanGuards {
            budget_bytes,
            ..PlanGuards::default()
        },
    );
    let summary = block_on(runner.run_with(
        shape,
        &PlanRunOptions {
            max_parallel,
            ..Default::default()
        },
    ));
    (trace, summary)
}

/// Current-thread runtime: yield-based scheduling is only deterministic when a
/// single thread owns the task queue.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current-thread runtime")
        .block_on(future)
}

/// Acyclic index pairs over `PROP_NODES` nodes, duplicates allowed.
fn edge_strategy() -> impl Strategy<Value = Vec<(usize, usize)>> {
    prop::collection::vec((0usize..PROP_NODES, 0usize..PROP_NODES), 0..20)
        .prop_map(|edges| edges.into_iter().filter(|(from, to)| from < to).collect())
}

/// **Halt**: once the budget is exhausted the run stops *committing*. The
/// payload of every node is 13 bytes (`value_for`), so any budget below that is
/// consumed by the first commit and the rest must never be counted.
#[test]
fn budget_exhaustion_halts_the_run() {
    let shape = prop_plan(&[(0, 1), (1, 2), (2, 3)]);
    let value_len = value_for("n0").len();
    let (trace, summary) = run_plan(&shape, 4, value_len - 1);
    let summary = summary.expect("a truncated plan still returns Ok");

    assert!(
        summary.truncated,
        "a budget below one payload must truncate"
    );
    assert_eq!(
        summary.nodes_executed, 1,
        "committed {} nodes after the budget was exhausted",
        summary.nodes_executed
    );
    // Nodes already in flight when the halt lands are dropped, not committed;
    // they may have started, which is why the assertion is on commits.
    assert!(
        !trace.seen("finish", "n3"),
        "the chain tail ran despite the halt"
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// **Linearization**: dispatch order respects every edge, every node runs
    /// exactly once, and a plan never stalls (all nodes are reached because an
    /// unreferenced node is indegree-0 and thus seeded ready).
    ///
    /// The stronger half — "all predecessors are *committed* when a node is
    /// dispatched" — is checked in this profile by `inputs_of_precomputed`'s
    /// `debug_assert_eq!`, which aborts this test if a parent value is missing.
    #[test]
    fn prop_dispatch_is_a_linearization(edges in edge_strategy()) {
        let shape = prop_plan(&edges);
        let (trace, summary) = run_plan(&shape, 4, 64 * 1024);
        let summary = summary.expect("an acyclic plan always completes");

        prop_assert_eq!(summary.nodes_executed, PROP_NODES);
        prop_assert_eq!(summary.tool_calls, PROP_NODES);
        let starts = trace.order("start");
        prop_assert_eq!(starts.len(), PROP_NODES, "dispatch count drifted");
        for id in prop_names() {
            prop_assert!(
                trace.times("start", &id) == 1,
                "{id} dispatched more than once"
            );
            prop_assert!(
                trace.times("finish", &id) == 1,
                "{id} executed more than once"
            );
        }
        for (from, to) in &edges {
            let names = prop_names();
            let (from_id, to_id) = (names[*from].clone(), names[*to].clone());
            let before = trace.position("start", &from_id);
            let after = trace.position("start", &to_id);
            prop_assert!(
                before < after,
                "{} dispatched before its predecessor {}: {:?}",
                to_id,
                from_id,
                starts
            );
        }
    }

    /// **Budget**: `truncated` must be set exactly when the true committed byte
    /// count reaches the budget — never spuriously, never missed. The ground
    /// truth is `value_for`'s length, not anything the runner reports.
    ///
    /// Which node trips the budget first depends on commit order and is
    /// deliberately unspecified under concurrency, so the property is about the
    /// flag, not about the split.
    #[test]
    fn prop_truncated_matches_true_byte_total(edges in edge_strategy(), budget in 1usize..128) {
        let shape = prop_plan(&edges);
        let names = prop_names();
        let total: usize = names.iter().map(|id| value_for(id).len()).sum();
        let (_, summary) = run_plan(&shape, 4, budget);
        let summary = summary.expect("an acyclic plan always completes");

        prop_assert!(
            summary.truncated == (total >= budget),
            "budget {}, true payload total {}, got truncated={}",
            budget,
            total,
            summary.truncated
        );
    }

    /// **No legacy drift**: at one window, eager dispatch degenerates to the
    /// only sequential linearization, so its summary must equal the layered
    /// legacy `run` byte for byte. Shapes are forced into "last node is the
    /// unique terminal" (every other node also edges into it) because the two
    /// entry points summarise differently by design — that difference is not
    /// what this property guards.
    #[test]
    fn prop_single_window_matches_legacy_run(edges in edge_strategy()) {
        let mut edges = edges;
        let last = PROP_NODES - 1;
        edges.extend((0..last).map(|from| (from, last)));
        let shape = prop_plan(&edges);
        let budget = 64 * 1024;

        let (_, eager) = run_plan(&shape, 1, budget);
        let eager = eager.expect("single-window plan completes");
        let legacy = block_on(
            runner(Arc::new(Trace::default()), None).run(&shape)
        )
        .expect("legacy plan completes");

        prop_assert_eq!(eager.summary.as_str(), legacy.summary.as_str());
        prop_assert_eq!(eager.nodes_executed, legacy.nodes_executed);
        prop_assert_eq!(eager.truncated, legacy.truncated);
    }
}
