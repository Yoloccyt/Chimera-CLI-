//! Tool plane runtime configuration (env-driven, default off).

use std::env;

/// Three-state rollout switch: off / observe / apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ToolPlaneMode {
    /// Feature disabled (default): `tool call/plan` refuse with a hint.
    #[default]
    Off,
    /// Validate and preview without invoking providers or writing files.
    Observe,
    /// Execute for real.
    Apply,
}

/// Tool plane limits.
#[derive(Debug, Clone, Copy)]
pub struct ToolPlaneConfig {
    /// Rollout mode.
    pub mode: ToolPlaneMode,
    /// Global/provider concurrency bound.
    pub max_inflight: usize,
    /// Default `tool plan` layer concurrency.
    pub max_parallel_plan: usize,
    /// Default per-call timeout.
    pub default_timeout_ms: u64,
    /// Payload cap (also used for process stdout/stderr).
    pub max_payload_bytes: usize,
}

impl Default for ToolPlaneConfig {
    fn default() -> Self {
        Self {
            mode: ToolPlaneMode::Off,
            max_inflight: 8,
            max_parallel_plan: 8,
            default_timeout_ms: 30_000,
            max_payload_bytes: 64 * 1024,
        }
    }
}

impl ToolPlaneConfig {
    /// Read the rollout switch and limits from `CHIMERA_TOOL_*` env vars.
    #[must_use]
    pub fn from_env() -> Self {
        let mode = match env::var("CHIMERA_TOOL_PLANE").ok().as_deref() {
            Some("apply") => ToolPlaneMode::Apply,
            Some("observe") => ToolPlaneMode::Observe,
            _ => ToolPlaneMode::Off,
        };
        let defaults = Self::default();
        Self {
            mode,
            max_inflight: env_usize("CHIMERA_TOOL_MAX_INFLIGHT", defaults.max_inflight),
            max_parallel_plan: env_usize(
                "CHIMERA_TOOL_MAX_PARALLEL_PLAN",
                defaults.max_parallel_plan,
            ),
            default_timeout_ms: env_u64(
                "CHIMERA_TOOL_DEFAULT_TIMEOUT_MS",
                defaults.default_timeout_ms,
            ),
            max_payload_bytes: env_usize(
                "CHIMERA_TOOL_MAX_PAYLOAD_BYTES",
                defaults.max_payload_bytes,
            ),
        }
    }
}

fn env_usize(name: &str, default: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn env_u64(name: &str, default: u64) -> u64 {
    env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}
