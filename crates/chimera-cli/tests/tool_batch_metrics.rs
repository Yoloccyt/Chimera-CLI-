//! 组合根批量归因计数器接线验收（W3 / F-03 / ADR-248 C-A）
//!
//! # WHY 需要这个文件
//!
//! ADR-248 的「会被发布而非能构造」纪律：Sink 链路（`TelemetryBatchSink`
//! 注册进 `BusLogger`、注入 `ToolRuntime`）必须在**组合根生产装配**中闭合——
//! `chimera exec/run` 走 `execute_batch` 时，`render_metrics()` 必须出现
//! 递增后的 `chimera_tool_batch_*_total`。若有人拆掉组合根注入
//! （`with_batch_sink`）或改掉 `TelemetryBatchSink::register`，本测试立即红。
//!
//! # 计数命名契约
//!
//! 注册名不带 `_total`（prometheus-client 渲染自动追加）：注册
//! `chimera_tool_batch_succeeded` → 输出行 `chimera_tool_batch_succeeded_total <n>`。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use chimera_cli::composition::build;
use chimera_cli::ChimeraConfig;
use gqep_executor::tool_runtime::{BatchLimits, DispatchRequest};

/// 组合根装配 → 真实批量执行 → 指标可见（生产者→消费者闭合）。
#[tokio::test]
async fn composition_root_wires_tool_batch_counters() {
    let ctx = build(&ChimeraConfig::default()).expect("组合根装配应成功");
    let reqs: Vec<DispatchRequest> = (0..3)
        .map(|i| DispatchRequest {
            tool_name: "text.echo".to_string(),
            args_json: format!(r#"{{"text":"m{i}"}}"#),
        })
        .collect();
    let outcome = ctx
        .tool_runtime
        .execute_batch(&reqs, BatchLimits::default())
        .await;
    assert_eq!(outcome.succeeded, 3, "内置只读工具应全部成功");

    let logger = ctx
        .bus
        .logger()
        .expect("组合根必须装配 BusLogger（composition.rs set_logger）");
    let metrics = logger.render_metrics();
    assert!(
        metrics.contains("chimera_tool_batch_succeeded_total 3"),
        "成功计数器应递增到 3，实际:\n{metrics}"
    );
    assert!(
        metrics.contains("chimera_tool_batch_total 3"),
        "总数计数器应递增到 3，实际:\n{metrics}"
    );
    assert!(
        metrics.contains("chimera_tool_batch_failed_total 0"),
        "失败计数器应存在且为 0，实际:\n{metrics}"
    );
}

/// 第二批累加（Counter 语义：跨批次累计而非覆盖）。
#[tokio::test]
async fn counters_accumulate_across_batches() {
    let ctx = build(&ChimeraConfig::default()).expect("组合根装配应成功");
    let mk = |i: usize| DispatchRequest {
        tool_name: "text.echo".to_string(),
        args_json: format!(r#"{{"text":"b{i}"}}"#),
    };
    let first: Vec<DispatchRequest> = (0..2).map(mk).collect();
    let second: Vec<DispatchRequest> = (0..3).map(mk).collect();
    ctx.tool_runtime
        .execute_batch(&first, BatchLimits::default())
        .await;
    ctx.tool_runtime
        .execute_batch(&second, BatchLimits::default())
        .await;
    let metrics = ctx.bus.logger().expect("BusLogger 已装配").render_metrics();
    assert!(
        metrics.contains("chimera_tool_batch_total 5"),
        "两批应累计 2+3=5，实际:\n{metrics}"
    );
}
