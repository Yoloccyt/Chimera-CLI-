//! 独立调用并行率判据（本 Spec D0-4 / C4）
//!
//! 判据不是"耗时多少"，而是**在飞峰值**：N 个无依赖只读节点在
//! `max_parallel >= N` 下必须同时处于执行中（峰值 = N），即无依赖调用的
//! 重叠率为 1.0；`max_parallel < N` 时峰值必须 = 上限（满额并行，不超卖）。
//!
//! 使用 20ms 服务时间而非 2ms：Windows 定时器粒度约 15.6ms（R12 实测），
//! 2ms 的 sleep 会全部落进同一跳，测不出重叠。

#![allow(clippy::unwrap_used, clippy::expect_used)]
#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use gqep_executor::{PlanGuards, PlanRunOptions, PlanRunner, ToolExecutor};
use nexus_contracts::tool_plan::{SideEffectDecl, ToolNode, ToolOp, ToolPlan};

struct LoadProbe {
    in_flight: AtomicUsize,
    peak: AtomicUsize,
}

impl LoadProbe {
    fn new() -> Self {
        Self {
            in_flight: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
        }
    }

    fn peak(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ToolExecutor for LoadProbe {
    async fn execute(&self, _tool_name: &str, _args_json: &str) -> Result<String, String> {
        let current = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(current, Ordering::SeqCst);
        tokio::time::sleep(Duration::from_millis(20)).await;
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        Ok("{}".to_string())
    }
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

fn independent_plan(n: usize) -> ToolPlan {
    ToolPlan {
        id: "parallel-rate".to_string(),
        nodes: (0..n).map(|i| node(&format!("n{i}"))).collect(),
        edges: Vec::new(),
    }
}

async fn run_with_cap(n: usize, cap: usize) -> usize {
    let probe = Arc::new(LoadProbe::new());
    let runner = PlanRunner::new(
        Box::new(ProbeHandle(Arc::clone(&probe))),
        PlanGuards::default(),
    );
    runner
        .run_with(
            &independent_plan(n),
            &PlanRunOptions {
                max_parallel: cap,
                ..Default::default()
            },
        )
        .await
        .expect("独立节点计划应成功");
    probe.peak()
}

/// `Arc<LoadProbe>` 不能直接实现 ToolExecutor（孤儿规则下的便利转发）
struct ProbeHandle(Arc<LoadProbe>);

#[async_trait]
impl ToolExecutor for ProbeHandle {
    async fn execute(&self, tool_name: &str, args_json: &str) -> Result<String, String> {
        self.0.execute(tool_name, args_json).await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn independent_calls_reach_full_parallelism() {
    assert_eq!(
        run_with_cap(8, 8).await,
        8,
        "8 个独立只读节点在 cap=8 下必须同时在飞（重叠率 1.0）"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cap_limits_in_flight_without_oversubscription() {
    assert_eq!(
        run_with_cap(8, 4).await,
        4,
        "cap=4 时在飞峰值必须恰为 4：满额并行且不超卖"
    );
}
