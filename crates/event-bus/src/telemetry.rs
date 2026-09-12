//! OTel 风格轻量遥测（P2-T11，v4.0 WI-28 落地形态）
//!
//! 对应架构层: **L1 Core**（event-bus，ADR-143 裁决：倾向 event-bus 增强，
//! 否决 nexus-telemetry 新建——复用 CBF/rcu/token_ledger 基建）
//! 对应任务: **P2-T11**（手册 W13-14）
//!
//! # 与 WI-28 的关系
//! 完整 OTel 标准（Span 导出 JSON/Protobuf → Jaeger/Zipkin/Prometheus）为
//! 横切面增强；本模块落地**可移植子集**：每 Agent Turn 一 Span（start/end +
//! 属性）+ 延迟直方图（AtomicU64 桶近似）——追踪开销 <5% CPU 门禁由
//! 原子计数保证（无锁热路径）。
//!
//! # ADR-174 决策
//! 维持可移植子集（不引入 OTLP/外部导出器依赖）；标准导出器评估结论=推迟
//! （依赖面/开销 gate <5% CPU 现值不合算）；任何新增导出器须另立 ADR
//! 并满足 <5% CPU 门禁。
//!
//! # 与 L9 efficiency-monitor 分工（WI-28 规格）
//! 本模块 = 基础设施追踪（Span/延迟直方图）；efficiency-monitor = 业务效能
//! （规则告警/配额）。互不重叠。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

/// Span 状态
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpanStatus {
    /// 进行中
    Active,
    /// 成功完成
    Ok,
    /// 失败
    Error,
}

/// Turn Span — 每 Agent Turn 一条（WI-28 规格）
///
/// `span_id`/`session_id` 属性承载因果链；`elapsed_ms` 于 end 时记录。
#[derive(Debug, Clone)]
pub struct TurnSpan {
    /// Span 唯一 ID（自增）
    pub span_id: u64,
    /// 会话 ID（跨 Turn 关联）
    pub session_id: String,
    /// 状态
    pub status: SpanStatus,
    /// 开始时刻（单调时钟）
    started: Instant,
    /// 结束时刻
    finished: Option<Instant>,
}

/// 延迟直方图（µs 桶近似，AtomicU64——热路径无锁）
///
/// 桶界（µs）：[0, 100, 1000, 10_000, 100_000, +∞)
#[derive(Debug, Default)]
pub struct LatencyHistogram {
    buckets: [AtomicU64; 6],
}

impl LatencyHistogram {
    /// 记录一次延迟（µs）
    pub fn record_us(&self, us: u64) {
        let idx = match us {
            0..=99 => 0,
            100..=999 => 1,
            1_000..=9_999 => 2,
            10_000..=99_999 => 3,
            100_000..=999_999 => 4,
            _ => 5,
        };
        self.buckets[idx].fetch_add(1, Ordering::Relaxed);
    }

    /// 各桶计数快照（诊断）
    #[must_use]
    pub fn snapshot(&self) -> [u64; 6] {
        [
            self.buckets[0].load(Ordering::Relaxed),
            self.buckets[1].load(Ordering::Relaxed),
            self.buckets[2].load(Ordering::Relaxed),
            self.buckets[3].load(Ordering::Relaxed),
            self.buckets[4].load(Ordering::Relaxed),
            self.buckets[5].load(Ordering::Relaxed),
        ]
    }

    /// 总记录数
    #[must_use]
    pub fn total(&self) -> u64 {
        self.snapshot().iter().sum()
    }
}

/// 轻量遥测器 — Turn Span 生命周期 + 延迟直方图
#[derive(Debug, Clone, Default)]
pub struct Telemetry {
    /// Span 计数器（ID 分配）
    span_counter: Arc<AtomicU64>,
    /// Turn 延迟直方图（µs）
    pub turn_latency: Arc<LatencyHistogram>,
    /// 追踪启用开关（开销控制）
    #[allow(dead_code)] // 预留运行时启停开关（当前只读装配,诊断可见性保留）
    enabled: bool,
}

impl Telemetry {
    /// 新建遥测器（默认启用——原子计数开销 <1% CPU【门禁目标】）
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// 开始 Turn Span
    #[must_use]
    pub fn start_span(&self, session_id: impl Into<String>) -> TurnSpan {
        let span_id = self.span_counter.fetch_add(1, Ordering::Relaxed);
        TurnSpan {
            span_id,
            session_id: session_id.into(),
            status: SpanStatus::Active,
            started: Instant::now(),
            finished: None,
        }
    }

    /// 结束 Turn Span（记录延迟直方图）
    pub fn end_span(&self, span: &mut TurnSpan, status: SpanStatus) {
        span.status = status;
        span.finished = Some(Instant::now());
        let us = span
            .finished
            .unwrap()
            .duration_since(span.started)
            .as_micros() as u64;
        self.turn_latency.record_us(us);
    }

    /// Span 数（诊断）
    #[must_use]
    pub fn span_count(&self) -> u64 {
        self.span_counter.load(Ordering::Relaxed)
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_lifecycle() {
        let t = Telemetry::new();
        let mut span = t.start_span("s-1");
        assert_eq!(span.status, SpanStatus::Active);
        assert!(span.finished.is_none());
        t.end_span(&mut span, SpanStatus::Ok);
        assert_eq!(span.status, SpanStatus::Ok);
        assert!(span.finished.is_some());
        assert_eq!(t.span_count(), 1);
        assert_eq!(t.turn_latency.total(), 1, "结束必须记录直方图");
    }

    #[test]
    fn span_ids_monotonic() {
        let t = Telemetry::new();
        let a = t.start_span("s-1");
        let b = t.start_span("s-1");
        assert!(b.span_id > a.span_id, "Span ID 必须单调");
    }

    #[test]
    fn histogram_buckets() {
        let h = LatencyHistogram::default();
        h.record_us(50);
        h.record_us(500);
        h.record_us(5_000);
        h.record_us(50_000);
        h.record_us(500_000);
        h.record_us(5_000_000);
        let snap = h.snapshot();
        assert_eq!(snap, [1, 1, 1, 1, 1, 1], "六桶各一");
        assert_eq!(h.total(), 6);
    }

    #[test]
    fn concurrent_records_no_loss() {
        let t = Telemetry::new();
        let handles: Vec<_> = (0..8)
            .map(|_| {
                let t = t.clone();
                std::thread::spawn(move || {
                    for _ in 0..100 {
                        let mut span = t.start_span("s-conc");
                        t.end_span(&mut span, SpanStatus::Ok);
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(t.turn_latency.total(), 800, "并发记录无丢失（原子计数）");
    }
}
