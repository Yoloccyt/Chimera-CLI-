//! `BatchAttributionSink` 加性契约验收（W3 / F-03 / ADR-248）
//!
//! # WHY 需要这个文件
//!
//! F-03 的缺口：`dispatch_batch` 的 `BatchAttribution` 快照只落
//! `tracing::debug!`（batch.rs 注释自陈「本 crate 零指标依赖，上层可按需
//! 推入」）——4.4×→8.35× 有界并行加速在运行期不可观测。ADR-248 C-A 落点：
//! 本 crate 保持零指标依赖，只加**依赖倒置**的 Sink 出口：
//! - `dispatch_batch_with_sink(..., Option<&dyn BatchAttributionSink>)`
//!   在归因快照生成后调用 `sink.record`（`total == 0` 的空批次提前返回，
//!   天然不发）；
//! - 旧 `dispatch_batch` 签名逐字不变（A5），语义 = `None` sink；
//! - `None` 与 `Some` 的结果**逐位等价**（派发路径零改动，仅观测出口）。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use gqep_executor::tool_runtime::InProcDispatcher;
use gqep_executor::tool_runtime::{
    dispatch_batch, dispatch_batch_with_sink, BatchAttribution, BatchAttributionSink, BatchLimits,
    DispatchRequest, ToolDispatcher,
};
use gqep_executor::ToolExecutor;

/// 回显执行器 — 派发路径与既有批量测试同构（零副作用）
struct EchoExecutor;

#[async_trait]
impl ToolExecutor for EchoExecutor {
    async fn execute(&self, _tool_name: &str, args_json: &str) -> Result<String, String> {
        Ok(args_json.to_string())
    }
}

fn dispatcher() -> Arc<dyn ToolDispatcher> {
    Arc::new(InProcDispatcher::new(Arc::new(EchoExecutor)))
}

fn requests(n: usize) -> Vec<DispatchRequest> {
    (0..n)
        .map(|i| DispatchRequest {
            tool_name: "bench.echo".to_string(),
            args_json: format!(r#"{{"i":{i}}}"#),
        })
        .collect()
}

/// 累加式 Sink — 捕获 `record` 调用次数与各字段累计
#[derive(Default)]
struct CountingSink {
    calls: AtomicU64,
    total: AtomicU64,
    succeeded: AtomicU64,
    failed: AtomicU64,
    abandoned: AtomicU64,
    not_started: AtomicU64,
}

impl BatchAttributionSink for CountingSink {
    fn record(&self, a: &BatchAttribution) {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.total.fetch_add(a.total as u64, Ordering::Relaxed);
        self.succeeded
            .fetch_add(a.succeeded as u64, Ordering::Relaxed);
        self.failed.fetch_add(a.failed as u64, Ordering::Relaxed);
        self.abandoned
            .fetch_add(a.abandoned as u64, Ordering::Relaxed);
        self.not_started
            .fetch_add(a.not_started as u64, Ordering::Relaxed);
    }
}

/// `Some(sink)`：归因逐字段透传（8 请求全成功 → total=8 / succeeded=8，一次 record）。
#[tokio::test]
async fn sink_records_attribution_fields() {
    let sink = CountingSink::default();
    let reqs = requests(8);
    let outcome =
        dispatch_batch_with_sink(&*dispatcher(), &reqs, BatchLimits::default(), Some(&sink)).await;
    assert_eq!(outcome.succeeded, 8);
    assert!(outcome.is_conserved());
    assert_eq!(sink.calls.load(Ordering::Relaxed), 1, "每批恰好一次 record");
    assert_eq!(sink.total.load(Ordering::Relaxed), 8);
    assert_eq!(sink.succeeded.load(Ordering::Relaxed), 8);
    assert_eq!(sink.failed.load(Ordering::Relaxed), 0);
    assert_eq!(sink.abandoned.load(Ordering::Relaxed), 0);
    assert_eq!(sink.not_started.load(Ordering::Relaxed), 0);
}

/// `None` 与旧 `dispatch_batch` 结果逐位等价（派发路径零改动的 L1 证据）。
#[tokio::test]
async fn none_sink_is_bitwise_equivalent_to_legacy_path() {
    let reqs = requests(16);
    let plain = dispatch_batch(&*dispatcher(), &reqs, BatchLimits::default()).await;
    let none = dispatch_batch_with_sink(&*dispatcher(), &reqs, BatchLimits::default(), None).await;
    assert_eq!(
        plain.results, none.results,
        "None sink 必须与旧路径逐位等价"
    );
    assert_eq!(plain.succeeded, none.succeeded);
    assert_eq!(plain.abandoned, none.abandoned);
    assert_eq!(plain.not_started, none.not_started);
}

/// 空批次（total == 0）：提前返回、不触发 record（防刷屏纪律的结构性满足）。
#[tokio::test]
async fn empty_batch_never_calls_sink() {
    let sink = CountingSink::default();
    let outcome =
        dispatch_batch_with_sink(&*dispatcher(), &[], BatchLimits::default(), Some(&sink)).await;
    assert_eq!(outcome.results.len(), 0);
    assert_eq!(
        sink.calls.load(Ordering::Relaxed),
        0,
        "空批次不得调用 record"
    );
}

/// 单操作超时计入 failed 而非 succeeded —— Sink 侧如实体现在归因字段。
#[tokio::test]
async fn timeout_failures_surface_in_sink() {
    struct SlowExecutor;
    #[async_trait]
    impl ToolExecutor for SlowExecutor {
        async fn execute(&self, _t: &str, _a: &str) -> Result<String, String> {
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
            Ok("late".to_string())
        }
    }
    let sink = CountingSink::default();
    let reqs = requests(2);
    let limits = BatchLimits {
        concurrency: 2,
        per_op_timeout_ms: 10,
        deadline_ms: 0,
    };
    let outcome = dispatch_batch_with_sink(
        &*dispatcher_of(Arc::new(SlowExecutor)),
        &reqs,
        limits,
        Some(&sink),
    )
    .await;
    assert_eq!(outcome.failed, 2, "超时应计入 failed");
    assert_eq!(sink.failed.load(Ordering::Relaxed), 2);
    assert_eq!(sink.succeeded.load(Ordering::Relaxed), 0);
}

fn dispatcher_of(executor: Arc<dyn ToolExecutor>) -> Arc<dyn ToolDispatcher> {
    Arc::new(InProcDispatcher::new(executor))
}
