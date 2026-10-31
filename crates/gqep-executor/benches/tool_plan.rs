//! PlanRunner bounded-parallelism benchmark (Phase 2).
//!
//! Measures 16 independent 1ms tool calls with `max_parallel=1` vs `8`.
//! The absolute values are informational (bench-lite / CI authority); the
//! structural peak-bound tests live in `tests/plan_runner_bounded.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used)] // bench code idiom
use std::time::Duration;

use async_trait::async_trait;
use criterion::{criterion_group, criterion_main, Criterion};
use gqep_executor::{PlanGuards, PlanRunOptions, PlanRunner, ToolExecutor};
use nexus_contracts::tool_plan::{SideEffectDecl, ToolNode, ToolOp, ToolPlan};

struct FastExecutor;

#[async_trait]
impl ToolExecutor for FastExecutor {
    async fn execute(&self, tool_name: &str, _args_json: &str) -> Result<String, String> {
        tokio::time::sleep(Duration::from_millis(1)).await;
        Ok(format!(r#"{{"tool":"{tool_name}"}}"#))
    }
}

fn node(id: &str) -> ToolNode {
    ToolNode {
        id: id.to_string(),
        op: ToolOp::ToolCall,
        tool_name: Some("bench".to_string()),
        args_json: Some("{}".to_string()),
        field: None,
        predicate: None,
        side_effect: Some(SideEffectDecl::ReadOnly),
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
}

criterion_group!(benches, bench_plan);
criterion_main!(benches);
