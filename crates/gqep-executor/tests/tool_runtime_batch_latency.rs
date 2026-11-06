//! 批量派发延迟分位数实测（`#[ignore]` — 显式运行）
//!
//! # 为什么单独存在
//! criterion 输出 mean/median/slope/MAD，**不输出 p50/p99**。为避免用 mean
//! 冒充 p99，本测试逐调用记录真实完成时刻，排序后打印分位数与吞吐。
//!
//! # 运行
//! ```powershell
//! cargo test -p gqep-executor --test tool_runtime_batch_latency -- --ignored --nocapture
//! ```
//!
//! # 口径（两种延迟分开登记，不混算）
//! - **串行**单调用延迟 = 该次 `dispatch().await` 的墙钟。
//! - **并行**单调用延迟 = 该请求**完成时刻 − 整批开始时刻**（含排队等待），
//!   即调用方视角的端到端延迟。并发有界必然使尾调用等待更久，这是并行要
//!   如实暴露的代价（吞吐换单调用尾延迟），不拿"平均"掩盖。
//!
//! 预热 3 轮丢弃，正式 15 轮；N=64，注入延迟 1ms，并发上界 8。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use gqep_executor::tool_runtime::{BatchLimits, DispatchRequest, ToolRuntime};
use gqep_executor::ToolExecutor;
use seccore::execpolicy::{ExecPolicy, ExecPolicyRule, PolicyAction, RulePattern};

const N: usize = 64;
const BOUND: usize = 8;
const DELAY_MS: u64 = 1;
const WARMUP_ROUNDS: usize = 3;
const ROUNDS: usize = 15;

/// 记录「相对批开始时刻的完成时刻」的执行器
///
/// WHY 在 execute 内部记录：这是链的最后一环，其返回时刻即调用方可见的完成时刻。
struct RecordingExecutor {
    batch_start: Instant,
    completions_ms: Arc<Mutex<Vec<f64>>>,
}

#[async_trait]
impl ToolExecutor for RecordingExecutor {
    async fn execute(&self, _tool_name: &str, args_json: &str) -> Result<String, String> {
        tokio::time::sleep(Duration::from_millis(DELAY_MS)).await;
        let done_ms = self.batch_start.elapsed().as_secs_f64() * 1000.0;
        self.completions_ms
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(done_ms);
        Ok(args_json.to_string())
    }
}

fn allow_policy() -> Arc<ExecPolicy> {
    Arc::new(ExecPolicy::new().add_rule(ExecPolicyRule {
        pattern: RulePattern::new("bench.echo", "*"),
        action: PolicyAction::Allow,
    }))
}

fn runtime(batch_start: Instant, completions_ms: &Arc<Mutex<Vec<f64>>>) -> ToolRuntime {
    ToolRuntime::new(
        Arc::new(RecordingExecutor {
            batch_start,
            completions_ms: Arc::clone(completions_ms),
        }),
        allow_policy(),
    )
    .expect("装配应成功")
}

fn requests() -> Vec<DispatchRequest> {
    (0..N)
        .map(|i| DispatchRequest {
            tool_name: "bench.echo".to_string(),
            args_json: format!("{{\"i\":{i}}}"),
        })
        .collect()
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((sorted.len() - 1) as f64 * p).round() as usize;
    sorted[idx]
}

fn summarize(label: &str, mut latencies: Vec<f64>, rounds_ms: &[f64]) -> (f64, f64) {
    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let p50 = percentile(&latencies, 0.50);
    let p99 = percentile(&latencies, 0.99);
    let mean = latencies.iter().sum::<f64>() / latencies.len() as f64;
    let wall_mean = rounds_ms.iter().sum::<f64>() / rounds_ms.len() as f64;
    let throughput = N as f64 / (wall_mean / 1000.0);
    println!(
        "{label:<10} p50={p50:.3} ms  p99={p99:.3} ms  mean={mean:.3} ms  batch_wall={wall_mean:.2} ms  throughput={throughput:.0} calls/s"
    );
    (wall_mean, throughput)
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "perf: 显式运行（--ignored --nocapture）"]
async fn serial_vs_parallel_latency_percentiles() {
    let reqs = requests();

    // 预热（丢弃）
    let warm_start = Instant::now();
    let warm_records = Arc::new(Mutex::new(Vec::new()));
    let warm_runtime = runtime(warm_start, &warm_records);
    for _ in 0..WARMUP_ROUNDS {
        for req in &reqs {
            let _ = warm_runtime.dispatch(req).await;
        }
        let _ = warm_runtime
            .execute_batch(
                &reqs,
                BatchLimits {
                    concurrency: BOUND,
                    per_op_timeout_ms: 0,
                    deadline_ms: 0,
                },
            )
            .await;
    }

    // 串行：逐调用墙钟
    let mut serial_latencies = Vec::with_capacity(N * ROUNDS);
    let mut serial_wall = Vec::with_capacity(ROUNDS);
    let serial_start = Instant::now();
    let serial_records = Arc::new(Mutex::new(Vec::new()));
    let serial_runtime = runtime(serial_start, &serial_records);
    for _ in 0..ROUNDS {
        let round_start = Instant::now();
        for req in &reqs {
            let call_start = Instant::now();
            let _ = serial_runtime.dispatch(req).await;
            serial_latencies.push(call_start.elapsed().as_secs_f64() * 1000.0);
        }
        serial_wall.push(round_start.elapsed().as_secs_f64() * 1000.0);
    }

    // 并行：完成时刻 - 批开始时刻
    let mut parallel_latencies = Vec::with_capacity(N * ROUNDS);
    let mut parallel_wall = Vec::with_capacity(ROUNDS);
    for _ in 0..ROUNDS {
        let batch_start = Instant::now();
        let records = Arc::new(Mutex::new(Vec::new()));
        let runtime = runtime(batch_start, &records);
        let outcome = runtime
            .execute_batch(
                &reqs,
                BatchLimits {
                    concurrency: BOUND,
                    per_op_timeout_ms: 0,
                    deadline_ms: 0,
                },
            )
            .await;
        assert_eq!(outcome.succeeded, N, "全部请求应成功");
        parallel_wall.push(batch_start.elapsed().as_secs_f64() * 1000.0);
        let mut guard = records.lock().unwrap_or_else(|p| p.into_inner());
        assert_eq!(guard.len(), N, "每个请求都应被记录");
        parallel_latencies.append(&mut *guard);
    }

    let (serial_wall_mean, serial_tp) = summarize("serial", serial_latencies, &serial_wall);
    let (parallel_wall_mean, parallel_tp) =
        summarize("parallel", parallel_latencies, &parallel_wall);
    println!(
        "speedup = {:.2}x  (throughput {serial_tp:.0} → {parallel_tp:.0} calls/s; wall {serial_wall_mean:.2} → {parallel_wall_mean:.2} ms; N={N}, bound={BOUND}, delay={DELAY_MS}ms)",
        serial_wall_mean / parallel_wall_mean
    );
}
