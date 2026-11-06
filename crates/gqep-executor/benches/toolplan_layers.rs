//! PlanRunner 只读层「有界并发」基准 — criterion
//!
//! 场景:8 个只读 ToolCall 同层（无依赖）,mock executor 每次 `sleep(2ms)`;
//! 对比 `concurrency = 1`（等价串行）与 `concurrency = 8`（有界并行）。
//!
//! 理论墙钟:串行 ≈ 8 × 2ms = 16ms;有界并行 ≈ 2ms + 调度开销。
//!
//! 运行:
//! ```powershell
//! cargo bench -p gqep-executor --bench toolplan_layers
//! ```
#![allow(clippy::unwrap_used, clippy::expect_used)] // bench code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::time::Duration;

use async_trait::async_trait;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use gqep_executor::toolplan_runner::{PlanGuards, PlanRunner, ToolExecutor};
use nexus_contracts::tool_plan::{SideEffectDecl, ToolNode, ToolOp, ToolPlan};

/// 固定延迟执行器 — 模拟真实只读工具 I/O（零副作用）
struct SleepExecutor {
    delay_ms: u64,
}

#[async_trait]
impl ToolExecutor for SleepExecutor {
    async fn execute(&self, _tool_name: &str, args_json: &str) -> Result<String, String> {
        tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
        Ok(args_json.to_string())
    }
}

/// n 个只读 ToolCall 同层（无依赖 → 同一层）
fn readonly_layer_plan(n: usize) -> ToolPlan {
    let nodes: Vec<ToolNode> = (0..n)
        .map(|i| ToolNode {
            id: format!("t{i}"),
            op: ToolOp::ToolCall,
            tool_name: Some(format!("bench.readonly.{i}")),
            args_json: Some(format!(r#"{{"i":{i}}}"#)),
            field: None,
            predicate: None,
            side_effect: Some(SideEffectDecl::ReadOnly),
        })
        .collect();
    ToolPlan {
        id: format!("bench-readonly-layer-{n}"),
        nodes,
        edges: Vec::new(),
    }
}

fn bench_toolplan_layers(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("创建 tokio runtime 失败");
    let plan = readonly_layer_plan(8);

    let mut group = c.benchmark_group("toolplan_layers");
    group.sample_size(20);

    for concurrency in [1usize, 8] {
        let runner = PlanRunner::new(
            Box::new(SleepExecutor { delay_ms: 2 }),
            PlanGuards::default(),
        )
        .with_concurrency(concurrency);
        group.bench_with_input(
            BenchmarkId::new("readonly8_ms2", concurrency),
            &concurrency,
            |b, _| {
                b.to_async(&rt).iter(|| async {
                    let summary = runner.run(&plan).await.expect("基准计划必须执行成功");
                    assert_eq!(summary.tool_calls, 8);
                    assert!(!summary.truncated);
                });
            },
        );
    }

    group.finish();
}

criterion_group!(benches, bench_toolplan_layers);
criterion_main!(benches);
