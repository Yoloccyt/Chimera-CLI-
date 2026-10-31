//! Tool runtime — policy → scheduler → provider → timeout/cancel → outcome.
//!
//! The runtime never panics and never lets a provider failure escape as an
//! exception: every invocation materializes into a [`ToolOutcome`]. Batch calls
//! preserve input order and isolate per-call failures.

use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::stream::FuturesUnordered;
use futures::StreamExt;
use seccore::execpolicy::{ExecPolicy, PolicyAction};
use tokio_util::sync::CancellationToken;

use nexus_contracts::tool_plan::ToolPlan;

use super::metrics::{ToolMetrics, ToolMetricsSnapshot};
use super::registry::ToolRegistry;
use super::scheduler::{SchedSnapshot, ToolScheduler};
use super::types::{ToolError, ToolInvocation, ToolOutcome, ToolPriority, ToolStatus};
use crate::toolplan_runner::{
    PlanGuards, PlanRunError, PlanRunOptions, PlanRunner, PlanSummary, ToolExecutor,
};

/// Runtime limits and defaults. All bounds are explicit; no unbounded spawn.
#[derive(Debug, Clone)]
pub struct ToolRuntimeConfig {
    /// Interactive-lane concurrency (clamped to >=1 by the scheduler).
    pub interactive_inflight: usize,
    /// Background-lane concurrency (clamped to >=1 by the scheduler).
    pub background_inflight: usize,
    /// Maximum callers waiting for permits before `Overloaded`.
    pub max_waiters: usize,
    /// Per-provider concurrency (clamped to >=1 by the scheduler).
    pub max_provider_inflight: usize,
    /// Default timeout when neither invocation nor descriptor specifies one.
    pub default_timeout_ms: u64,
    /// Payload budget; exceeded payloads are truncated on a UTF-8 boundary.
    pub max_payload_bytes: usize,
    /// Maximum invocations accepted by one `invoke_batch` call.
    pub max_batch: usize,
}

impl Default for ToolRuntimeConfig {
    fn default() -> Self {
        Self {
            interactive_inflight: 2,
            background_inflight: 6,
            max_waiters: 256,
            max_provider_inflight: 8,
            default_timeout_ms: 30_000,
            max_payload_bytes: 64 * 1024,
            max_batch: 1024,
        }
    }
}

/// Unified tool execution runtime.
pub struct ToolRuntime {
    registry: Arc<ToolRegistry>,
    policy: Option<Arc<ExecPolicy>>,
    scheduler: Arc<dyn ToolScheduler>,
    cfg: ToolRuntimeConfig,
    metrics: Arc<ToolMetrics>,
}

impl ToolRuntime {
    /// Assemble a runtime from a populated registry, optional policy and scheduler.
    #[must_use]
    pub fn new(
        registry: Arc<ToolRegistry>,
        policy: Option<Arc<ExecPolicy>>,
        scheduler: Arc<dyn ToolScheduler>,
        cfg: ToolRuntimeConfig,
    ) -> Self {
        Self {
            registry,
            policy,
            scheduler,
            cfg,
            metrics: Arc::new(ToolMetrics::default()),
        }
    }

    /// Registry accessor (listing/introspection).
    #[must_use]
    pub fn registry(&self) -> &Arc<ToolRegistry> {
        &self.registry
    }

    /// Cumulative metrics snapshot.
    #[must_use]
    pub fn metrics(&self) -> ToolMetricsSnapshot {
        self.metrics.snapshot()
    }

    /// Scheduler snapshot (queue depth / peak in-flight).
    #[must_use]
    pub fn scheduler_snapshot(&self) -> SchedSnapshot {
        self.scheduler.snapshot()
    }

    /// Execute one tool call.
    pub async fn invoke(&self, inv: ToolInvocation, cancel: &CancellationToken) -> ToolOutcome {
        let started = Instant::now();
        let _in_flight = self.metrics.record_start();

        let Some((provider, descriptor)) = self.registry.resolve(&inv.tool) else {
            return self.finish_failed(
                &inv,
                "registry",
                ToolStatus::NotFound,
                format!("tool not found: {}", inv.tool),
                started,
            );
        };

        if let Some(policy) = &self.policy {
            let action = policy.evaluate(&inv.tool, std::slice::from_ref(&inv.args_json));
            match action {
                PolicyAction::Deny => {
                    return self.finish_failed(
                        &inv,
                        &descriptor.provider,
                        ToolStatus::Denied,
                        format!("denied: execpolicy (tool={})", inv.tool),
                        started,
                    );
                }
                PolicyAction::Ask if !inv.approved => {
                    return self.finish_failed(
                        &inv,
                        &descriptor.provider,
                        ToolStatus::AskRequired,
                        format!(
                            "ask_required: execpolicy needs approval (tool={})",
                            inv.tool
                        ),
                        started,
                    );
                }
                _ => {}
            }
        }

        let timeout_ms = effective_timeout(
            inv.timeout_ms,
            descriptor.timeout_ms,
            self.cfg.default_timeout_ms,
        );
        let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);

        let _permit = tokio::select! {
            permit = self.scheduler.acquire(&inv, &descriptor, cancel) => match permit {
                Ok(permit) => permit,
                Err(err) => return self.finish_error(&inv, &descriptor.provider, err, started),
            },
            _ = cancel.cancelled() => {
                return self.finish_failed(
                    &inv,
                    &descriptor.provider,
                    ToolStatus::Cancelled,
                    format!("cancelled: {}", inv.call_id),
                    started,
                );
            }
            _ = tokio::time::sleep_until(deadline) => {
                return self.finish_failed(
                    &inv,
                    &descriptor.provider,
                    ToolStatus::Timeout,
                    format!("timeout: {} after {}ms", inv.call_id, timeout_ms),
                    started,
                );
            }
        };

        let result = tokio::select! {
            result = tokio::time::timeout_at(deadline, provider.invoke(&inv, cancel.clone())) => result,
            _ = cancel.cancelled() => {
                return self.finish_failed(
                    &inv,
                    &descriptor.provider,
                    ToolStatus::Cancelled,
                    format!("cancelled: {}", inv.call_id),
                    started,
                );
            }
        };

        match result {
            Ok(Ok(payload)) => {
                let (payload, truncated) = truncate_payload(payload, self.cfg.max_payload_bytes);
                self.finish_ok(&inv, &descriptor.provider, payload, truncated, started)
            }
            Ok(Err(err)) => self.finish_error(&inv, &descriptor.provider, err, started),
            Err(_) => self.finish_failed(
                &inv,
                &descriptor.provider,
                ToolStatus::Timeout,
                format!("timeout: {} after {}ms", inv.call_id, timeout_ms),
                started,
            ),
        }
    }

    /// Execute a batch concurrently. Results preserve input order; one failure
    /// does not cancel the rest. Invocations beyond `max_batch` are returned as
    /// `Overloaded` outcomes (never silently dropped).
    pub async fn invoke_batch(
        &self,
        invocations: Vec<ToolInvocation>,
        cancel: &CancellationToken,
    ) -> Vec<ToolOutcome> {
        let mut invocations = invocations;
        let overflow = if invocations.len() > self.cfg.max_batch {
            invocations.split_off(self.cfg.max_batch)
        } else {
            Vec::new()
        };

        let mut futures = FuturesUnordered::new();
        for (index, invocation) in invocations.into_iter().enumerate() {
            let cancel = cancel.clone();
            futures.push(async move { (index, self.invoke(invocation, &cancel).await) });
        }

        let mut collected: Vec<(usize, ToolOutcome)> = Vec::with_capacity(futures.len());
        while let Some(item) = futures.next().await {
            collected.push(item);
        }
        collected.sort_by_key(|(index, _)| *index);

        let mut outcomes: Vec<ToolOutcome> =
            collected.into_iter().map(|(_, outcome)| outcome).collect();
        for invocation in overflow {
            outcomes.push(ToolOutcome::failed(
                &invocation,
                "runtime",
                ToolStatus::Overloaded,
                format!("batch limit {} exceeded", self.cfg.max_batch),
                0,
            ));
        }
        outcomes
    }

    /// Execute a `ToolPlan` DAG through the same runtime (policy/timeout/cancel).
    ///
    /// The plan runner keeps its legacy `run` untouched; this method uses the
    /// bounded `run_with` path and a private adapter that re-enters
    /// [`ToolRuntime::invoke`], never `run_plan` (no recursion).
    pub async fn run_plan(
        self: &Arc<Self>,
        plan: &ToolPlan,
        opts: PlanRunOptions,
    ) -> Result<PlanSummary, PlanRunError> {
        let max_parallel = if opts.max_parallel == 0 {
            8
        } else {
            opts.max_parallel
        };
        let timeout_ms = if opts.timeout_ms == 0 {
            self.cfg.default_timeout_ms
        } else {
            opts.timeout_ms
        };
        let runner = PlanRunner::new(
            Box::new(RuntimeExecutor {
                runtime: Arc::clone(self),
            }),
            PlanGuards {
                max_parallel,
                timeout_ms,
                ..Default::default()
            },
        );
        runner.run_with(plan, &opts).await
    }

    fn finish_ok(
        &self,
        inv: &ToolInvocation,
        provider: &str,
        payload: String,
        truncated: bool,
        started: Instant,
    ) -> ToolOutcome {
        let duration = started.elapsed();
        self.metrics.record_status(ToolStatus::Ok);
        self.metrics.record_duration(duration);
        ToolOutcome::ok(
            inv,
            provider,
            payload,
            truncated,
            duration.as_millis().min(u128::from(u64::MAX)) as u64,
        )
    }

    fn finish_failed(
        &self,
        inv: &ToolInvocation,
        provider: &str,
        status: ToolStatus,
        error: String,
        started: Instant,
    ) -> ToolOutcome {
        let duration = started.elapsed();
        self.metrics.record_status(status);
        self.metrics.record_duration(duration);
        ToolOutcome::failed(
            inv,
            provider,
            status,
            error,
            duration.as_millis().min(u128::from(u64::MAX)) as u64,
        )
    }

    fn finish_error(
        &self,
        inv: &ToolInvocation,
        provider: &str,
        err: ToolError,
        started: Instant,
    ) -> ToolOutcome {
        let status = match &err {
            ToolError::NotFound(_) => ToolStatus::NotFound,
            ToolError::InvalidArgs(_) => ToolStatus::InvalidArgs,
            ToolError::Denied(_) => ToolStatus::Denied,
            ToolError::AskRequired(_) => ToolStatus::AskRequired,
            ToolError::Timeout { .. } => ToolStatus::Timeout,
            ToolError::Cancelled(_) => ToolStatus::Cancelled,
            ToolError::Overloaded { .. } => ToolStatus::Overloaded,
            ToolError::ProviderUnavailable(_) | ToolError::Provider(_) => ToolStatus::ProviderError,
        };
        self.finish_failed(inv, provider, status, err.to_string(), started)
    }
}

/// Adapter for `PlanRunner`; delegates each node to `ToolRuntime::invoke`.
struct RuntimeExecutor {
    runtime: Arc<ToolRuntime>,
}

#[async_trait::async_trait]
impl ToolExecutor for RuntimeExecutor {
    async fn execute(&self, tool_name: &str, args_json: &str) -> Result<String, String> {
        ToolExecutor::execute(&*self.runtime, tool_name, args_json).await
    }
}

/// Legacy `ToolExecutor` adapter: keeps the old trait usable without changing
/// any existing caller. Errors stay plain text (PlanRunner wraps them as JSON).
#[async_trait::async_trait]
impl ToolExecutor for ToolRuntime {
    async fn execute(&self, tool_name: &str, args_json: &str) -> Result<String, String> {
        let invocation = ToolInvocation {
            call_id: format!("compat-{tool_name}"),
            tool: tool_name.to_string(),
            args_json: args_json.to_string(),
            timeout_ms: None,
            priority: ToolPriority::Normal,
            origin: super::types::ToolOrigin::Cli,
            quest_id: None,
            task_id: None,
            approved: false,
        };
        let outcome = self.invoke(invocation, &CancellationToken::new()).await;
        match outcome.status {
            ToolStatus::Ok => Ok(outcome.payload.unwrap_or_default()),
            _ => Err(outcome
                .error
                .unwrap_or_else(|| format!("{:?}", outcome.status))),
        }
    }
}

fn effective_timeout(requested: Option<u64>, descriptor: u64, default: u64) -> u64 {
    let requested = requested.unwrap_or(descriptor);
    let requested = if requested == 0 { default } else { requested };
    if descriptor == 0 {
        requested
    } else {
        requested.min(descriptor)
    }
}

fn truncate_payload(mut payload: String, max_bytes: usize) -> (String, bool) {
    if payload.len() <= max_bytes {
        return (payload, false);
    }
    let mut end = max_bytes;
    while end > 0 && !payload.is_char_boundary(end) {
        end -= 1;
    }
    payload.truncate(end);
    (payload, true)
}
