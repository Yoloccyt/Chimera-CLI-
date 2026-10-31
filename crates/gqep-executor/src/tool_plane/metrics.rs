//! Lock-free tool plane metrics.
//!
//! Deliberately atomics-only: the runtime hot path must not take a lock or
//! touch Prometheus label maps per invocation. Export/dashboarding can read
//! [`ToolMetrics::snapshot`] at any frequency without blocking calls.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use super::types::ToolStatus;

/// Cumulative runtime counters.
#[derive(Debug, Default)]
pub struct ToolMetrics {
    invocations: AtomicU64,
    ok: AtomicU64,
    failed: AtomicU64,
    timeouts: AtomicU64,
    cancelled: AtomicU64,
    in_flight: AtomicU64,
    peak_in_flight: AtomicU64,
    total_duration_us: AtomicU64,
}

/// Point-in-time metrics view.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ToolMetricsSnapshot {
    /// Total invocations started.
    pub invocations: u64,
    /// Successful invocations.
    pub ok: u64,
    /// Failed invocations (all non-timeout/non-cancel failures).
    pub failed: u64,
    /// Deadline-exceeded invocations.
    pub timeouts: u64,
    /// Cancelled invocations.
    pub cancelled: u64,
    /// Currently in-flight invocations.
    pub in_flight: u64,
    /// High-water mark of in-flight invocations.
    pub peak_in_flight: u64,
    /// Cumulative wall-clock duration (microseconds).
    pub total_duration_us: u64,
}

impl ToolMetrics {
    /// Mark one invocation as started. The returned guard decrements in-flight on drop.
    pub(crate) fn record_start(&self) -> InFlightGuard<'_> {
        self.invocations.fetch_add(1, Ordering::Relaxed);
        let current = self.in_flight.fetch_add(1, Ordering::Relaxed) + 1;
        self.peak_in_flight.fetch_max(current, Ordering::Relaxed);
        InFlightGuard { metrics: self }
    }

    /// Record a terminal status.
    pub(crate) fn record_status(&self, status: ToolStatus) {
        match status {
            ToolStatus::Ok => {
                self.ok.fetch_add(1, Ordering::Relaxed);
            }
            ToolStatus::Timeout => {
                self.timeouts.fetch_add(1, Ordering::Relaxed);
            }
            ToolStatus::Cancelled => {
                self.cancelled.fetch_add(1, Ordering::Relaxed);
            }
            _ => {
                self.failed.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Record the wall-clock duration of one invocation.
    pub(crate) fn record_duration(&self, duration: Duration) {
        self.total_duration_us.fetch_add(
            duration.as_micros().min(u128::from(u64::MAX)) as u64,
            Ordering::Relaxed,
        );
    }

    /// Read a consistent-enough snapshot (individual fields are relaxed).
    #[must_use]
    pub fn snapshot(&self) -> ToolMetricsSnapshot {
        ToolMetricsSnapshot {
            invocations: self.invocations.load(Ordering::Relaxed),
            ok: self.ok.load(Ordering::Relaxed),
            failed: self.failed.load(Ordering::Relaxed),
            timeouts: self.timeouts.load(Ordering::Relaxed),
            cancelled: self.cancelled.load(Ordering::Relaxed),
            in_flight: self.in_flight.load(Ordering::Relaxed),
            peak_in_flight: self.peak_in_flight.load(Ordering::Relaxed),
            total_duration_us: self.total_duration_us.load(Ordering::Relaxed),
        }
    }
}

/// RAII in-flight counter.
pub(crate) struct InFlightGuard<'a> {
    metrics: &'a ToolMetrics,
}

impl Drop for InFlightGuard<'_> {
    fn drop(&mut self) {
        self.metrics.in_flight.fetch_sub(1, Ordering::Relaxed);
    }
}
