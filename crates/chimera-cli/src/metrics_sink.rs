//! 批量归因指标 Sink — W3 / F-03 / ADR-248 C-A 路线的 chimera-cli 侧实现
//!
//! # 职责
//!
//! 把 [`BatchAttribution`] 快照翻译成 `BusLogger` 上的 5 个域中立计数器
//! （经 `register_counter` 注册，ADR-248 C-A 主案）。`gqep-executor` 保持
//! 零指标依赖（依赖倒置：Sink trait 在 L7，实现在 L10 组合根）。
//!
//! # 计数命名契约
//!
//! 注册名**不带 `_total`**（prometheus-client 渲染自动追加）：
//! 注册 `chimera_tool_batch_succeeded` → 输出行
//! `chimera_tool_batch_succeeded_total <n>`。Counter 语义 = 跨批次累计。
//!
//! # 装配时序（组合根唯一装配点，composition.rs）
//!
//! 1. `BusLogger::new(...)` 构造（尚未收编进 bus）；
//! 2. [`TelemetryBatchSink::register`] 在该 logger 上注册 5 计数器
//!    （**必须先于 `bus.set_logger(logger)`**——注册写的是 logger 内部
//!    Registry，收编后经 `bus.logger()` 看到的是同一份）；
//! 3. `bus.set_logger(logger)`；
//! 4. `ToolRuntime::with_batch_sink(Arc::new(sink))`。

use std::sync::atomic::{AtomicU64, Ordering};

use event_bus::logging::{BusLogger, RegisteredCounter};
use gqep_executor::tool_runtime::{BatchAttribution, BatchAttributionSink};

/// 批量归因 → Prometheus 计数器（跨批次累计）
pub struct TelemetryBatchSink {
    /// 批次总请求数累计
    total: RegisteredCounter,
    /// 成功数累计
    succeeded: RegisteredCounter,
    /// 失败数累计（含单操作超时）
    failed: RegisteredCounter,
    /// deadline 放弃数累计
    abandoned: RegisteredCounter,
    /// 未入流数累计
    not_started: RegisteredCounter,
    /// record 调用次数（本地审计计数，不进 Registry——供诊断与测试）
    calls: AtomicU64,
}

impl TelemetryBatchSink {
    /// 在 `logger` 上注册 5 个计数器并构造 Sink（组合根装配期调用一次）
    ///
    /// WHY 收 `&mut BusLogger`：prometheus-client 0.22 的 `Registry::register`
    /// 收 `&mut self`——注册只能在构造期（`set_logger` 收编前）发生，类型
    /// 系统强化「注册先于收编」时序契约。
    ///
    /// WHY 关联函数命名 `register` 而非 `new`：组合根装配点门
    /// （check_composition_root.py）以 `Type::new`/`Type::with_*` 关联调用
    /// 识别基础设施装配——本类型是观测胶水而非基础设施，`register` 命名
    /// 使 G-4 基线零变更（深审 W3 规格裁决）。
    #[must_use]
    pub fn register(logger: &mut BusLogger) -> Self {
        Self {
            total: logger.register_counter(
                "chimera_tool_batch",
                "Tool batch dispatch: total requests across batches",
            ),
            succeeded: logger.register_counter(
                "chimera_tool_batch_succeeded",
                "Tool batch dispatch: succeeded requests",
            ),
            failed: logger.register_counter(
                "chimera_tool_batch_failed",
                "Tool batch dispatch: failed requests (incl. per-op timeout)",
            ),
            abandoned: logger.register_counter(
                "chimera_tool_batch_abandoned",
                "Tool batch dispatch: requests abandoned by batch deadline",
            ),
            not_started: logger.register_counter(
                "chimera_tool_batch_not_started",
                "Tool batch dispatch: requests never started (backlog tail)",
            ),
            calls: AtomicU64::new(0),
        }
    }

    /// record 调用次数（诊断/测试）
    #[must_use]
    pub fn record_calls(&self) -> u64 {
        self.calls.load(Ordering::Relaxed)
    }
}

impl BatchAttributionSink for TelemetryBatchSink {
    fn record(&self, attribution: &BatchAttribution) {
        // WHY 不走 sink 内 total==0 过滤：空批次在 dispatch_batch_with_sink
        // 提前返回、根本不会 record——这里只做忠实翻译，不重复判定。
        self.total.inc_by(attribution.total as u64);
        self.succeeded.inc_by(attribution.succeeded as u64);
        self.failed.inc_by(attribution.failed as u64);
        self.abandoned.inc_by(attribution.abandoned as u64);
        self.not_started.inc_by(attribution.not_started as u64);
        self.calls.fetch_add(1, Ordering::Relaxed);
    }
}
