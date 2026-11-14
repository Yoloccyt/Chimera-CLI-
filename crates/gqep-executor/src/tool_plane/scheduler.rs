//! Bounded tool scheduler — two lanes + per-provider permits + waiter limit.
//!
//! The scheduler bounds actual provider executions, not merely queued futures:
//! `invoke_batch` may create all futures up front, but each future waits here
//! for a lane permit and a provider permit before touching the provider.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio_util::sync::CancellationToken;

use super::types::{ToolDescriptor, ToolError, ToolInvocation, ToolPriority};

/// Snapshot for diagnostics/tests.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SchedSnapshot {
    /// Current interactive-lane in-flight calls.
    pub interactive_in_flight: usize,
    /// Current background-lane in-flight calls.
    pub background_in_flight: usize,
    /// Callers currently waiting for a permit.
    pub waiters: usize,
    /// High-water mark of in-flight calls (both lanes).
    pub peak_in_flight: usize,
}

/// RAII permit: dropping it returns the lane and provider capacity.
#[derive(Debug)]
pub struct ToolPermit {
    _lane: OwnedSemaphorePermit,
    _provider: OwnedSemaphorePermit,
}

/// Scheduler abstraction (tests can substitute a deterministic implementation).
#[async_trait::async_trait]
pub trait ToolScheduler: Send + Sync {
    /// Acquire lane + provider permits, or fail with `Overloaded`/`Cancelled`.
    async fn acquire(
        &self,
        inv: &ToolInvocation,
        desc: &ToolDescriptor,
        cancel: &CancellationToken,
    ) -> Result<ToolPermit, ToolError>;

    /// Current scheduler state.
    fn snapshot(&self) -> SchedSnapshot;
}

/// Fair-ish bounded scheduler.
pub struct BoundedToolScheduler {
    interactive: Arc<Semaphore>,
    background: Arc<Semaphore>,
    interactive_total: usize,
    background_total: usize,
    per_provider: Mutex<HashMap<String, Arc<Semaphore>>>,
    waiters: AtomicUsize,
    peak_in_flight: AtomicUsize,
    max_waiters: usize,
    max_provider_inflight: usize,
}

impl BoundedToolScheduler {
    /// Create a scheduler. Lane and provider bounds are clamped to at least 1
    /// (a zero-capacity semaphore would deadlock forever); `max_waiters` may be
    /// zero to reject all waiters (useful for fail-fast tests).
    #[must_use]
    pub fn new(
        interactive_inflight: usize,
        background_inflight: usize,
        max_waiters: usize,
        max_provider_inflight: usize,
    ) -> Self {
        let interactive_total = interactive_inflight.max(1);
        let background_total = background_inflight.max(1);
        Self {
            interactive: Arc::new(Semaphore::new(interactive_total)),
            background: Arc::new(Semaphore::new(background_total)),
            interactive_total,
            background_total,
            per_provider: Mutex::new(HashMap::new()),
            waiters: AtomicUsize::new(0),
            peak_in_flight: AtomicUsize::new(0),
            max_waiters,
            max_provider_inflight: max_provider_inflight.max(1),
        }
    }

    fn lane(&self, priority: ToolPriority) -> Arc<Semaphore> {
        match priority {
            ToolPriority::Interactive => Arc::clone(&self.interactive),
            ToolPriority::Normal | ToolPriority::Background => Arc::clone(&self.background),
        }
    }

    fn provider_semaphore(&self, provider: &str) -> Arc<Semaphore> {
        let mut map = self
            .per_provider
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // WHY `get` before `entry`: `entry` needs an owned key, so the plain
        // `entry(...).or_insert_with(...)` form allocated a `String` and hashed
        // it twice on **every** acquire — including the common case where the
        // lane already exists. `get` answers the hit in one hash lookup with zero
        // allocation; the `to_string()` now only happens when a provider lane is
        // created for the first time.
        if let Some(existing) = map.get(provider) {
            return Arc::clone(existing);
        }
        Arc::clone(
            map.entry(provider.to_string())
                .or_insert_with(|| Arc::new(Semaphore::new(self.max_provider_inflight))),
        )
    }

    fn interactive_in_flight(&self) -> usize {
        self.interactive_total
            .saturating_sub(self.interactive.available_permits())
    }

    fn background_in_flight(&self) -> usize {
        self.background_total
            .saturating_sub(self.background.available_permits())
    }
}

#[async_trait::async_trait]
impl ToolScheduler for BoundedToolScheduler {
    async fn acquire(
        &self,
        inv: &ToolInvocation,
        desc: &ToolDescriptor,
        cancel: &CancellationToken,
    ) -> Result<ToolPermit, ToolError> {
        let current = self.waiters.fetch_add(1, Ordering::SeqCst) + 1;
        if current > self.max_waiters {
            self.waiters.fetch_sub(1, Ordering::SeqCst);
            return Err(ToolError::Overloaded { waiters: current });
        }
        let waiter = WaiterGuard {
            waiters: &self.waiters,
        };

        let lane = self.lane(inv.priority);
        let provider = self.provider_semaphore(&desc.provider);

        // WHY 取序是 provider 先、lane 后（`ADR-234` A2-1 / `ADR-233` 追加节九 D3）：
        // 车道是**全局**槽位，provider 槽位只约束它自己那个 provider。反过来取
        // （旧写法）时，慢 provider 的等待者会先占满全局车道再去等自己的 provider 许可，
        // 于是"别的 provider 明明有空位却一条也进不去"——实测完成序
        // `["p-slow","p-slow","p-fast",…]`（`tests/scheduler_permit_order.rs`）。
        // 两个否决项留此备查：① 换回 lane 先取＝现状；② `futures::join!` 两把许可
        // 也不解决问题——lane 一就绪就被取得并持有到 provider 就绪，钉槽位的行为一模一样。
        // 固定全局顺序（provider→lane）不存在环路，故无死锁；残留代价是"持 provider
        // 许可等车道"会挡住同 provider 的后来者，但那时车道本就是全局瓶颈，它们照样得等。
        let provider_permit = tokio::select! {
            permit = provider.acquire_owned() => permit.map_err(|_| {
                ToolError::Cancelled(inv.call_id.clone())
            })?,
            _ = cancel.cancelled() => {
                return Err(ToolError::Cancelled(inv.call_id.clone()));
            }
        };
        let lane_permit = tokio::select! {
            permit = lane.acquire_owned() => permit.map_err(|_| {
                ToolError::Cancelled(inv.call_id.clone())
            })?,
            _ = cancel.cancelled() => {
                return Err(ToolError::Cancelled(inv.call_id.clone()));
            }
        };
        drop(waiter);

        let in_flight = self.interactive_in_flight() + self.background_in_flight();
        self.peak_in_flight.fetch_max(in_flight, Ordering::SeqCst);

        Ok(ToolPermit {
            _lane: lane_permit,
            _provider: provider_permit,
        })
    }

    fn snapshot(&self) -> SchedSnapshot {
        SchedSnapshot {
            interactive_in_flight: self.interactive_in_flight(),
            background_in_flight: self.background_in_flight(),
            waiters: self.waiters.load(Ordering::SeqCst),
            peak_in_flight: self.peak_in_flight.load(Ordering::SeqCst),
        }
    }
}

/// Decrements the waiter counter on every exit path (including cancellation).
struct WaiterGuard<'a> {
    waiters: &'a AtomicUsize,
}

impl Drop for WaiterGuard<'_> {
    fn drop(&mut self) {
        self.waiters.fetch_sub(1, Ordering::SeqCst);
    }
}
