//! `BusLogger::register_counter` 域中立加性 API 验收（W3 / ADR-248 C-A 路线）
//!
//! # WHY 需要这个文件
//!
//! 深审 spike（dwfrun-66cff073）裁决：`BusLogger` 的 Registry/Counter 字段
//! 全部私有、`new()` 只注册 3 个固定指标——批量归因（F-03）此前无指标落点。
//! C-A 主案给 event-bus 加 15-25 行域中立 `register_counter` API（不绑定
//! 工具语义、零事件变体新增、棘轮零消耗），由上层（chimera-cli）在装配期
//! 注册业务计数器。本文件锁三面契约：
//! 1. 注册名不带 `_total`（prometheus-client 渲染自动追加，重复追加即
//!    `_total_total`——logging.rs `new()` 的既有 WHY 纪律对新 API 同样生效）；
//! 2. 句柄可 clone、`inc_by` 后 `render_metrics()` 立即可见；
//! 3. 同一 logger 上注册的多个计数器互不干扰（各自独立累计）。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use event_bus::logging::BusLogger;

/// 注册 → 递增 → 渲染可见；注册名不带 `_total` 而输出自动带 `_total`。
#[test]
fn register_counter_renders_incremented_total() {
    let mut logger = BusLogger::new("gate-test");
    let counter = logger.register_counter("gate_probe", "ADR-248 negative-control probe");
    counter.inc_by(7);
    let metrics = logger.render_metrics();
    assert!(
        metrics.contains("gate_probe_total 7"),
        "渲染应含 `gate_probe_total 7`，实际:\n{metrics}"
    );
    // 固定 3 指标不受新 API 影响（既有观测面零削减）。
    // WHY 查 HELP/TYPE 而非样本行：nexus_event 是 Family——零发布时无标签集、
    // 无样本行；nexus_critical_event（无标签 Counter）才有恒在样本行。
    assert!(
        metrics.contains("# TYPE nexus_event counter"),
        "既有 nexus_event 注册必须在，实际:\n{metrics}"
    );
    assert!(
        metrics.contains("nexus_critical_event_total"),
        "既有 critical 计数器样本必须在，实际:\n{metrics}"
    );
}

/// 句柄 Clone 共享同一底层 Counter；多个计数器相互独立。
#[test]
fn cloned_handles_share_and_counters_stay_independent() {
    let mut logger = BusLogger::new("gate-test");
    let a = logger.register_counter("probe_a", "probe a");
    let b = logger.register_counter("probe_b", "probe b");
    let a2 = a.clone();
    a.inc();
    a2.inc_by(2);
    b.inc_by(5);
    let metrics = logger.render_metrics();
    assert!(
        metrics.contains("probe_a_total 3"),
        "clone 句柄共享计数，实际:\n{metrics}"
    );
    assert!(
        metrics.contains("probe_b_total 5"),
        "多计数器相互独立，实际:\n{metrics}"
    );
}
