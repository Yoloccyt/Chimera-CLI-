//! Property tests for the L7 tool execution plane (Phase 1 contracts).
//!
//! The example-based suite (`tool_plane_contract.rs`) locks specific fixtures;
//! these three properties lock the *rules* those fixtures were picked to
//! illustrate, over inputs the example suite cannot enumerate:
//!
//! 1. payload truncation is the **longest char-boundary prefix** that fits the
//!    budget (not "some valid UTF-8 cut", and not "a cut that silently drops a
//!    whole char when the budget lands mid-codepoint);
//! 2. `invoke_batch` returns results in **request order even when completions
//!    arrive in the opposite order** — the example suite drives every provider
//!    with `sleep_ms = 0`, so its completions are already in order and it cannot
//!    distinguish "order preserving" from "collect as completed";
//! 3. registration is all-or-nothing: a rejected batch leaves the registry
//!    byte-for-byte as it was.
//!
//! Deliberately **not** property-tested here: deadline/timeout behaviour. The
//! host clock granularity is 1/64 s (measured in Phase 7: `sleep(ZERO)` costs
//! ~15 ms), so any wall-clock property over `timeout_ms` is either flaky or so
//! loosely bounded that it asserts nothing. Deadlines are locked by the
//! deterministic cancellation tests in `tool_plane_contract.rs` instead.

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use gqep_executor::tool_plane::{
    BoundedToolScheduler, ToolDescriptor, ToolError, ToolInvocation, ToolOrigin, ToolPriority,
    ToolProvider, ToolRegistry, ToolRuntime, ToolRuntimeConfig, ToolScheduler, ToolStatus,
};
use nexus_contracts::tool_plan::SideEffectDecl;
use proptest::prelude::*;
use tokio_util::sync::CancellationToken;

/// Tasks per provider whose payload echoes `args_json` — enough to drive the
/// truncation property with arbitrary text.
struct EchoProvider;

#[async_trait]
impl ToolProvider for EchoProvider {
    fn provider_id(&self) -> &str {
        "echo"
    }

    fn descriptors(&self) -> Vec<ToolDescriptor> {
        vec![desc("echo.echo", "echo", 8)]
    }

    async fn invoke(
        &self,
        inv: &ToolInvocation,
        _c: CancellationToken,
    ) -> Result<String, ToolError> {
        Ok(inv.args_json.clone())
    }
}

/// Number of `yield_now` ticks task `i` performs, so **higher i becomes ready
/// earlier**: with `YIELDS - i` ticks, the last request finishes first. Using
/// yields (not sleeps) keeps the inversion deterministic on a current-thread
/// runtime and independent of the host clock.
const YIELDS: usize = 8;

struct InvertingProvider {
    completion: Arc<Mutex<Vec<String>>>,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl ToolProvider for InvertingProvider {
    fn provider_id(&self) -> &str {
        "inv"
    }

    fn descriptors(&self) -> Vec<ToolDescriptor> {
        vec![desc("inv.inv", "inv", YIELDS)]
    }

    async fn invoke(
        &self,
        inv: &ToolInvocation,
        _c: CancellationToken,
    ) -> Result<String, ToolError> {
        let index: usize = inv
            .args_json
            .parse()
            .map_err(|_| ToolError::InvalidArgs("args_json must be the task index".into()))?;
        for _ in 0..(YIELDS - index).min(YIELDS) {
            tokio::task::yield_now().await;
        }
        self.calls.fetch_add(1, Ordering::SeqCst);
        // Lock taken and released without crossing an await (repo red line).
        match self.completion.lock() {
            Ok(mut guard) => guard.push(inv.call_id.clone()),
            Err(_) => return Err(ToolError::Provider("completion log poisoned".into())),
        }
        Ok(inv.args_json.clone())
    }
}

/// Exposes exactly `names` under provider id `pid`.
struct NamedProvider {
    pid: &'static str,
    names: Vec<String>,
}

#[async_trait]
impl ToolProvider for NamedProvider {
    fn provider_id(&self) -> &str {
        self.pid
    }

    fn descriptors(&self) -> Vec<ToolDescriptor> {
        self.names
            .iter()
            .map(|name| desc(name, self.pid, 4))
            .collect()
    }

    async fn invoke(
        &self,
        _inv: &ToolInvocation,
        _c: CancellationToken,
    ) -> Result<String, ToolError> {
        Ok("{}".to_string())
    }
}

fn desc(name: &str, provider: &str, max_concurrency: usize) -> ToolDescriptor {
    ToolDescriptor {
        name: name.to_string(),
        provider: provider.to_string(),
        description: "property-test tool".to_string(),
        side_effect: SideEffectDecl::ReadOnly,
        timeout_ms: 5_000,
        max_concurrency,
        input_schema: None,
    }
}

fn invocation(tool: &str, call_id: &str, args_json: &str) -> ToolInvocation {
    ToolInvocation {
        call_id: call_id.to_string(),
        tool: tool.to_string(),
        args_json: args_json.to_string(),
        timeout_ms: None,
        priority: ToolPriority::Normal,
        origin: ToolOrigin::Cli,
        quest_id: None,
        task_id: None,
        approved: false,
    }
}

fn runtime_with(providers: Vec<Arc<dyn ToolProvider>>, cfg: ToolRuntimeConfig) -> Arc<ToolRuntime> {
    let mut registry = ToolRegistry::new();
    for provider in providers {
        registry.register(provider).expect("register provider");
    }
    let scheduler: Arc<dyn ToolScheduler> = Arc::new(BoundedToolScheduler::new(
        cfg.interactive_inflight,
        cfg.background_inflight,
        cfg.max_waiters,
        cfg.max_provider_inflight,
    ));
    Arc::new(ToolRuntime::new(Arc::new(registry), None, scheduler, cfg))
}

/// Current-thread runtime: the yield-scheduling in property 2 is only
/// deterministic when a single thread owns the task queue.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("current-thread runtime")
        .block_on(future)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// The truncated payload must be the longest prefix of the original whose
    /// byte length fits the budget. Anything weaker (any valid cut) would let a
    /// regression drop a whole character, or cut mid-codepoint and hand back
    /// bytes the caller cannot losslessly re-serialize.
    #[test]
    fn prop_truncation_is_longest_boundary_prefix(
        payload in prop::string::string_regex("[a-z你€\u{1F600}]{0,40}").unwrap(),
        cap in 0usize..48,
    ) {
        let cfg = ToolRuntimeConfig { max_payload_bytes: cap, ..Default::default() };
        let runtime = runtime_with(vec![Arc::new(EchoProvider)], cfg);
        let invocation = invocation("echo.echo", "c-1", &payload);

        let outcome = block_on(runtime.invoke(invocation, &CancellationToken::new()));
        prop_assert_eq!(outcome.status, ToolStatus::Ok);

        let got = outcome.payload.clone().unwrap_or_default();
        // Fitting inputs are handed back untouched; oversized ones are cut.
        prop_assert_eq!(outcome.truncated, payload.len() > cap);
        if !outcome.truncated {
            prop_assert_eq!(got.as_str(), payload.as_str());
        }
        // Never over budget, and always a prefix (so never a re-encoded string).
        prop_assert!(got.len() <= cap, "payload {} bytes exceeds budget {}", got.len(), cap);
        prop_assert!(payload.starts_with(got.as_str()), "payload is not a prefix of the input");
        // Maximality: the next character would not have fit.
        if outcome.truncated {
            if let Some(next) = payload[got.len()..].chars().next() {
                prop_assert!(
                    got.len() + next.len_utf8() > cap,
                    "cut left {} bytes unused but next char is {} bytes (budget {})",
                    cap - got.len(), next.len_utf8(), cap
                );
            }
        }
    }

    /// `invoke_batch` must key results to requests, never to completion order.
    #[test]
    fn prop_batch_order_survives_reversed_completions(n in 2usize..=YIELDS) {
        let completion: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let calls = Arc::new(AtomicUsize::new(0));
        let provider: Arc<dyn ToolProvider> = Arc::new(InvertingProvider {
            completion: Arc::clone(&completion),
            calls: Arc::clone(&calls),
        });
        let cfg = ToolRuntimeConfig {
            interactive_inflight: YIELDS,
            background_inflight: YIELDS,
            max_provider_inflight: YIELDS,
            max_waiters: YIELDS * 4,
            ..Default::default()
        };
        let runtime = runtime_with(vec![provider], cfg);
        let invocations = (0..n)
            .map(|i| invocation("inv.inv", &format!("c-{i}"), &i.to_string()))
            .collect();

        let outcomes = block_on(runtime.invoke_batch(invocations, &CancellationToken::new()));

        prop_assert_eq!(outcomes.len(), n);
        for (i, outcome) in outcomes.iter().enumerate() {
            let expected = i.to_string();
            prop_assert_eq!(&outcome.call_id, &format!("c-{i}"));
            prop_assert_eq!(outcome.payload.as_deref(), Some(expected.as_str()));
            prop_assert_eq!(outcome.status, ToolStatus::Ok);
        }

        // Teeth check: the fixture really inverted completion order, otherwise
        // this test would silently degrade into the example-based one.
        prop_assert_eq!(calls.load(Ordering::SeqCst), n, "not every call reached the provider");
        let seen = completion.lock().map(|g| g.clone()).unwrap_or_default();
        prop_assert_eq!(seen.len(), n);
        if n > 1 {
            prop_assert_ne!(seen.first().map(String::as_str), Some("c-0"),
                "fixture lost its teeth: completions arrived in request order");
        }
    }

    /// A rejected registration must leave the registry exactly as it was:
    /// no partial batch, no lost neighbours.
    #[test]
    fn prop_registration_is_all_or_nothing(names in prop::collection::vec(r"[a-z]{0,3}", 1..6)) {
        let mut registry = ToolRegistry::new();
        let seed: Arc<dyn ToolProvider> =
            Arc::new(NamedProvider { pid: "seed", names: vec!["seed.one".to_string()] });
        registry.register(seed).expect("seed registers");
        let before = registry.len();
        prop_assert_eq!(before, 1);

        let candidate: Arc<dyn ToolProvider> = Arc::new(NamedProvider { pid: "p", names: names.clone() });
        let result = registry.register(candidate);

        let unique_and_non_empty =
            names.iter().all(|n| !n.is_empty()) && names.iter().collect::<std::collections::HashSet<_>>().len() == names.len();
        if unique_and_non_empty {
            prop_assert!(
                result.is_ok(),
                "unexpected reject of {:?}: {:?}",
                names,
                result.err()
            );
            prop_assert_eq!(registry.len(), before + names.len());
        } else {
            prop_assert!(result.is_err(), "duplicate/empty names must be rejected: {:?}", names);
            prop_assert_eq!(registry.len(), before, "rejected batch must not insert anything");
            prop_assert!(registry.resolve("seed.one").is_some(), "seed entry lost");
            for name in &names {
                prop_assert!(registry.resolve(name).is_none(), "partial insert for {name}");
            }
        }
    }
}
