//! 批量工具派发「串行 vs 有界并行」基准 — criterion
//!
//! 三组对照（同进程同轮采样，只做相对比较）：
//! - `serial_*`：逐条 `dispatch().await`（优化前的唯一路径）
//! - `parallel_*`：`ToolRuntime::execute_batch`（有界并发 + 输入序回填）
//! - `overhead_*`：零延迟下的编排开销（boxed future + 波次调度），用于确认
//!   并行入口在"无 I/O"场景不会比串行更慢（否则该点应回退）
//!
//! 运行：
//! ```powershell
//! cargo bench -p gqep-executor --bench tool_runtime_batch
//! ```
//!
//! 分位数（p50/p99）不在这里测：criterion 只产 mean/median/slope，
//! 尾部延迟见 `tests/tool_runtime_batch_latency.rs`（`#[ignore]`，显式运行）。
#![allow(clippy::unwrap_used, clippy::expect_used)] // bench code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use gqep_executor::tool_runtime::{BatchLimits, DispatchRequest, ToolRuntime};
use gqep_executor::ToolExecutor;
use seccore::execpolicy::{ExecPolicy, ExecPolicyRule, PolicyAction, RulePattern};

/// 注入固定延迟的执行器 — 模拟真实工具 I/O（零副作用）
struct DelayExecutor {
    delay_ms: u64,
}

#[async_trait]
impl ToolExecutor for DelayExecutor {
    async fn execute(&self, _tool_name: &str, args_json: &str) -> Result<String, String> {
        if self.delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
        }
        Ok(args_json.to_string())
    }
}

fn allow_policy() -> Arc<ExecPolicy> {
    Arc::new(ExecPolicy::new().add_rule(ExecPolicyRule {
        pattern: RulePattern::new("bench.echo", "*"),
        action: PolicyAction::Allow,
    }))
}

fn runtime(delay_ms: u64) -> ToolRuntime {
    ToolRuntime::new(Arc::new(DelayExecutor { delay_ms }), allow_policy())
        .expect("合成执行者的四环装配应成功")
}

fn requests(n: usize) -> Vec<DispatchRequest> {
    (0..n)
        .map(|i| DispatchRequest {
            tool_name: "bench.echo".to_string(),
            args_json: format!("{{\"i\":{i}}}"),
        })
        .collect()
}

fn bench_serial_vs_parallel(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("创建 tokio runtime 失败");

    let mut group = c.benchmark_group("tool_batch");
    group.sample_size(20);

    // 1ms 延迟：模拟真实 I/O，比较墙钟吞吐
    for (n, bound) in [(16usize, 4usize), (64, 8)] {
        let runtime = runtime(1);
        let reqs = requests(n);
        let limits = BatchLimits {
            concurrency: bound,
            per_op_timeout_ms: 0,
            deadline_ms: 0,
        };

        group.bench_with_input(BenchmarkId::new("serial_ms1", n), &n, |b, _| {
            b.to_async(&rt).iter(|| async {
                for req in &reqs {
                    runtime.dispatch(req).await.unwrap();
                }
            });
        });

        group.bench_with_input(
            BenchmarkId::new(format!("parallel_ms1_bound{bound}"), n),
            &n,
            |b, _| {
                b.to_async(&rt).iter(|| async {
                    let outcome = runtime.execute_batch(&reqs, limits).await;
                    assert_eq!(outcome.succeeded, reqs.len());
                });
            },
        );
    }

    // 零延迟：只测编排开销（若并行入口反而更慢，说明该点无可证收益，应回退）
    let runtime0 = runtime(0);
    let reqs64 = requests(64);
    let limits0 = BatchLimits {
        concurrency: 8,
        per_op_timeout_ms: 0,
        deadline_ms: 0,
    };

    group.bench_with_input(BenchmarkId::new("serial_ms0", 64), &64, |b, _| {
        b.to_async(&rt).iter(|| async {
            for req in &reqs64 {
                runtime0.dispatch(req).await.unwrap();
            }
        });
    });

    group.bench_with_input(BenchmarkId::new("parallel_ms0_bound8", 64), &64, |b, _| {
        b.to_async(&rt).iter(|| async {
            let outcome = runtime0.execute_batch(&reqs64, limits0).await;
            assert_eq!(outcome.succeeded, reqs64.len());
        });
    });

    group.finish();
}

criterion_group!(benches, bench_serial_vs_parallel);
criterion_main!(benches);
