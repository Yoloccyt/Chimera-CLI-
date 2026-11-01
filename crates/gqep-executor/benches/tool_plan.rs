//! PlanRunner bounded-parallelism benchmark (Phase 2; edge shapes added W1a).
//!
//! `serial_16` / `parallel_16` measure 16 independent 1 ms tool calls with
//! `max_parallel=1` vs `8` — that pair can only see the *intra-layer window*,
//! because the plan has no edges at all.
//!
//! `chain_plus_slow_sibling` / `two_chains_lockstep` are the shapes the first
//! two cannot see: with edges, a **layer barrier** makes unrelated downstream
//! work wait for the slowest node of the current layer. Those two groups are
//! what makes "dispatch as soon as a node's own predecessors finish"
//! measurable at all; without them a change to the scheduling policy could
//! neither be shown as a win nor as a regression.
//!
//! Delays are deliberately far larger than this host's 1/64 s (15.6 ms) timer
//! granularity: a 1 ms vs 8 ms contrast collapses to the same quantized wake-up
//! and would silently flatten the effect being measured. Absolute values remain
//! informational (bench-lite / CI authority); the structural peak-bound tests
//! live in `tests/plan_runner_bounded.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used)] // bench code idiom
use std::time::Duration;

use async_trait::async_trait;
use criterion::{criterion_group, criterion_main, Criterion};
use gqep_executor::{PlanGuards, PlanRunOptions, PlanRunner, ToolExecutor};
use nexus_contracts::tool_plan::{PlanEdge, SideEffectDecl, ToolNode, ToolOp, ToolPlan};

/// Fast node: one quantum of work.
const FAST_TOOL: &str = "fast";
/// Slow node: several quanta, so the ratio survives timer quantization.
const SLOW_TOOL: &str = "slow";
const SLOW_MS: u64 = 60;

struct FastExecutor;

#[async_trait]
impl ToolExecutor for FastExecutor {
    async fn execute(&self, tool_name: &str, _args_json: &str) -> Result<String, String> {
        tokio::time::sleep(Duration::from_millis(1)).await;
        Ok(format!(r#"{{"tool":"{tool_name}"}}"#))
    }
}

/// Same trait, but the cost depends on which tool the node names — that is the
/// only channel `ToolExecutor` exposes, hence the name-as-encoding.
struct TimedExecutor;

#[async_trait]
impl ToolExecutor for TimedExecutor {
    async fn execute(&self, tool_name: &str, _args_json: &str) -> Result<String, String> {
        let ms = if tool_name == SLOW_TOOL { SLOW_MS } else { 1 };
        tokio::time::sleep(Duration::from_millis(ms)).await;
        Ok(format!(r#"{{"tool":"{tool_name}"}}"#))
    }
}

fn node(id: &str) -> ToolNode {
    node_tool(id, "bench")
}

fn node_tool(id: &str, tool: &str) -> ToolNode {
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

fn edges(pairs: &[(&str, &str)]) -> Vec<PlanEdge> {
    pairs
        .iter()
        .map(|(from, to)| PlanEdge {
            from: (*from).to_string(),
            to: (*to).to_string(),
        })
        .collect()
}

/// a1→a2→a3→a4 with an unrelated slow node sharing layer 0.
///
/// Under a layer barrier the chain cannot leave layer 0 until the slow sibling
/// finishes; dependency-aware dispatch keeps the chain flowing underneath it.
fn chain_with_slow_sibling() -> ToolPlan {
    ToolPlan {
        id: "chain-sibling".to_string(),
        nodes: vec![
            node_tool("a1", FAST_TOOL),
            node_tool("slow", SLOW_TOOL),
            node_tool("a2", FAST_TOOL),
            node_tool("a3", FAST_TOOL),
            node_tool("a4", FAST_TOOL),
        ],
        edges: edges(&[("a1", "a2"), ("a2", "a3"), ("a3", "a4")]),
    }
}

/// Two independent 3-node chains, `a*` fast and `b*` slow, in lock-step layers.
///
/// A layer barrier forces the fast chain to pay the slow chain's cost at **every**
/// layer; eager dispatch lets the fast chain finish while `b*` is still running.
fn two_chains_lockstep() -> ToolPlan {
    ToolPlan {
        id: "two-chains".to_string(),
        nodes: vec![
            node_tool("a1", FAST_TOOL),
            node_tool("b1", SLOW_TOOL),
            node_tool("a2", FAST_TOOL),
            node_tool("b2", SLOW_TOOL),
            node_tool("a3", FAST_TOOL),
            node_tool("b3", SLOW_TOOL),
        ],
        edges: edges(&[("a1", "a2"), ("a2", "a3"), ("b1", "b2"), ("b2", "b3")]),
    }
}

fn bench_plan(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime");
    let plan = ToolPlan {
        id: "bench-plan".to_string(),
        nodes: (0..16).map(|i| node(&format!("n{i}"))).collect(),
        edges: vec![],
    };
    let mut group = c.benchmark_group("tool_plan");
    group.sample_size(10);

    for (label, max_parallel) in [("serial_16", 1usize), ("parallel_16", 8usize)] {
        group.bench_function(label, |b| {
            let runner = PlanRunner::new(Box::new(FastExecutor), PlanGuards::default());
            b.to_async(&rt).iter(|| async {
                runner
                    .run_with(
                        &plan,
                        &PlanRunOptions {
                            max_parallel,
                            timeout_ms: 30_000,
                            ..Default::default()
                        },
                    )
                    .await
                    .expect("plan run")
            });
        });
    }
    group.finish();

    // Edge-bearing shapes: one iteration is ~0.1-0.3 s on this host, so keep
    // the sample size minimal and the measurement window honest.
    let mut group = c.benchmark_group("tool_plan_edges");
    group.sample_size(10);
    for (label, plan) in [
        ("chain_plus_slow_sibling", chain_with_slow_sibling()),
        ("two_chains_lockstep", two_chains_lockstep()),
    ] {
        group.bench_function(label, |b| {
            b.to_async(&rt).iter(|| async {
                let runner = PlanRunner::new(Box::new(TimedExecutor), PlanGuards::default());
                runner
                    .run_with(
                        &plan,
                        &PlanRunOptions {
                            max_parallel: 8,
                            timeout_ms: 30_000,
                            ..Default::default()
                        },
                    )
                    .await
                    .expect("plan run")
            });
        });
    }
    group.finish();
}

criterion_group!(benches, bench_plan);
criterion_main!(benches);
