//! `run --execute` 批量派发「串行 vs 有界并行」同轮对照基准（ADR-247 / W2）
//!
//! WHY 此 bench：F-13 闭环前，`ToolRuntime::execute_batch` 的 4.4× 加速只在
//! gqep-executor 的 bench 成立，**生产装配路径**（`with_optional_delegate`，
//! ADR-244 D1）无任何测量。本 bench 在 chimera-cli 侧经生产装配入口构造
//! ToolRuntime（注入固定延迟执行器模拟真实工具 I/O，零副作用），同轮同机对照：
//! - `runx_serial_1ms/64`：逐条 `ToolRuntime::dispatch`（优化前唯一生产路径）
//! - `runx_parallel_1ms/64`：`ToolRuntime::execute_batch`（有界并发，
//!   默认 `BatchLimits`——ADR-247 的首个生产调用方路径）
//! - `runx_{serial,parallel}_ms0/64`：零延迟编排开销对照
//!   （ADR-239 D6：零延迟下并行略慢属预期，防止误判为回归）
//!
//! 与 `gqep-executor/tool_runtime_batch` 的分工：那边测批量内核原语（裸
//! `dispatch_batch`），本边测「经生产装配入口（策略/沙箱/路由环齐备）的端到端
//! 批量」——两层信号互补，`runx_` 前缀守 I2 关键词互不为子串纪律。
//!
//! 运行：`cargo bench -p chimera-cli --bench run_execute_batch`
#![allow(clippy::unwrap_used, clippy::expect_used)] // bench code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use criterion::{criterion_group, criterion_main, Criterion};
use gqep_executor::tool_runtime::{BatchLimits, DispatchRequest, ToolRuntime};
use gqep_executor::ToolExecutor;
use seccore::execpolicy::PolicyAction;
use tokio::runtime::Runtime;

/// 注入固定延迟的执行器 — 模拟真实工具 I/O（零副作用；与 gqep bench 同构）
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

/// 经**生产装配入口**构造 ToolRuntime（ADR-244 D1 同款：空规则 + Allow 缺省），
/// 确保被测对象是「组合根装出来的那种四环链」而非裸批量内核。
fn production_runtime(delay_ms: u64) -> ToolRuntime {
    ToolRuntime::with_optional_delegate(
        Some(Arc::new(DelayExecutor { delay_ms })),
        Vec::new(),
        PolicyAction::Allow,
    )
    .expect("生产装配入口的四环装配应成功")
}

fn requests(n: usize) -> Vec<DispatchRequest> {
    (0..n)
        .map(|i| DispatchRequest {
            tool_name: "bench.echo".to_string(),
            args_json: format!("{{\"i\":{i}}}"),
        })
        .collect()
}

fn bench_run_execute_batch(c: &mut Criterion) {
    let rt = Runtime::new().expect("tokio multi-thread runtime build failed");
    for (tag, delay_ms) in [("zero", 0u64), ("1ms", 1u64)] {
        let runtime = production_runtime(delay_ms);
        let reqs = Arc::new(requests(64));

        // 串行腿：逐条 dispatch（ADR-247 前的唯一生产路径）
        let mut serial = c.benchmark_group(format!("runx_serial_{tag}"));
        serial.bench_function("64", |b| {
            let reqs = Arc::clone(&reqs);
            b.iter(|| {
                rt.block_on(async {
                    for r in reqs.iter() {
                        let _ = runtime.dispatch(r).await;
                    }
                });
            });
        });
        serial.finish();

        // 并行腿：execute_batch（F-13 首个生产调用方语义，默认 BatchLimits 8/30s/120s）
        let mut parallel = c.benchmark_group(format!("runx_parallel_{tag}"));
        parallel.bench_function("64", |b| {
            let reqs = Arc::clone(&reqs);
            b.iter(|| {
                rt.block_on(async {
                    let _ = runtime.execute_batch(&reqs, BatchLimits::default()).await;
                });
            });
        });
        parallel.finish();
    }
}

criterion_group!(benches, bench_run_execute_batch);
criterion_main!(benches);
