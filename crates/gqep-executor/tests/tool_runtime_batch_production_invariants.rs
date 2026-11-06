//! 批量派发的生产接线级不变量（ADR-240 W2-T2）
//!
//! # 与既有 proptest 的分工
//!
//! `tool_runtime_batch_proptest.rs` 覆盖：输入序逐位等价（invariant 1）、
//! 四段守恒与不丢槽位（invariant 2）、显式填充（invariant 6）。
//!
//! 本文件覆盖**尚缺**的三条，且是**生产接线级**（用真实 `ToolRuntime` +
//! 可计数执行器，非测试替身）：
//!
//! | 不变量 | 内容 | 本文件 |
//! |---|---|---|
//! | 3 | 每条请求**独立经完整四环** —— 批量不减免策略/沙箱评估 | ✅ |
//! | 4 | 在飞峰值恒 `≤ limits.concurrency` | ✅ |
//! | 5 | `concurrency == 0` 等价**串行**（峰值 1），不得解释为「无限制」 | ✅ |
//!
//! # WHY invariant 3 最关键
//!
//! 并行化最容易引入的安全退化是「为了吞吐，把安全前置挪到并发之后」——
//! 例如只对第一条请求做策略评估，或让沙箱检查在批量外做一次。
//! 本文件用**计数断言**证明：被策略拒绝的请求，其 delegate **一次都没被调用**，
//! 且拒绝归因是 `Policy`（先于沙箱与传输短路）。
//!
//! 与 `tool_runtime_chain_proptest.rs` 的分工：后者测单条派发的链序不变量；
//! 本文件测**批量场景下**链序不变量不被并发破坏。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use gqep_executor::tool_runtime::{BatchLimits, DispatchError, DispatchRequest, ToolRuntime};
use gqep_executor::ToolExecutor;
use seccore::execpolicy::{ExecPolicy, ExecPolicyRule, PolicyAction, RulePattern};

/// 可计数 + 可控在飞峰值的执行器
struct CountingExecutor {
    calls: AtomicUsize,
    in_flight: AtomicUsize,
    peak: AtomicUsize,
    delay_ms: u64,
}

impl CountingExecutor {
    fn new(delay_ms: u64) -> Self {
        Self {
            calls: AtomicUsize::new(0),
            in_flight: AtomicUsize::new(0),
            peak: AtomicUsize::new(0),
            delay_ms,
        }
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn peak(&self) -> usize {
        self.peak.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl ToolExecutor for CountingExecutor {
    async fn execute(&self, _tool_name: &str, args_json: &str) -> Result<String, String> {
        let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
        self.peak.fetch_max(now, Ordering::SeqCst);
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.delay_ms > 0 {
            tokio::time::sleep(Duration::from_millis(self.delay_ms)).await;
        }
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        Ok(args_json.to_string())
    }
}

/// 仅允许 `echo` 的策略 —— 其余工具走零信任 Deny（覆盖拒绝路径）
fn echo_only_policy() -> Arc<ExecPolicy> {
    Arc::new(ExecPolicy::new().add_rule(ExecPolicyRule {
        pattern: RulePattern::new("echo", "*"),
        action: PolicyAction::Allow,
    }))
}

fn runtime_with(exec: Arc<CountingExecutor>, policy: Arc<ExecPolicy>) -> ToolRuntime {
    ToolRuntime::new(exec, policy).expect("四环齐备的装配应成功")
}

fn request(tool_name: &str, i: usize) -> DispatchRequest {
    DispatchRequest {
        tool_name: tool_name.to_string(),
        args_json: format!("{{\"i\":{i}}}"),
    }
}

/// 不变量 3：每条请求独立经完整四环 —— 被拒项 0 次触达 delegate。
///
/// WHY 这是并行化的安全底线：批量若只做一次策略评估或把评估放到并发之后，
/// 被拒的工具就会真的执行。此断言用**计数**证明拒绝路径零副作用。
#[tokio::test]
async fn batch_each_request_passes_full_four_ring_chain() {
    let exec = Arc::new(CountingExecutor::new(0));
    let runtime = runtime_with(Arc::clone(&exec), echo_only_policy());

    let requests: Vec<DispatchRequest> = (0..8)
        .map(|i| {
            if i % 2 == 0 {
                request("echo", i)
            } else {
                request("blocked", i)
            }
        })
        .collect();

    let outcome = runtime
        .execute_batch(
            &requests,
            BatchLimits {
                concurrency: 4,
                per_op_timeout_ms: 0,
                deadline_ms: 0,
            },
        )
        .await;

    assert_eq!(outcome.results.len(), requests.len());
    assert!(outcome.is_conserved(), "四段守恒必须成立");

    // 拒绝项：归因必须是 Policy（链头短路），且不得触达 delegate
    for (i, res) in outcome.results.iter().enumerate() {
        if i % 2 == 1 {
            assert!(
                matches!(res, Err(DispatchError::Policy(_))),
                "第 {i} 条应被策略层拒绝（归因 Policy，先于沙箱与传输短路），实际: {res:?}"
            );
        } else {
            assert!(res.is_ok(), "第 {i} 条应成功，实际: {res:?}");
        }
    }

    // 关键计数：只有允许项触达 delegate（4 个 echo，4 个 blocked）
    assert_eq!(
        exec.calls(),
        4,
        "被策略拒绝的请求 0 次触达 delegate（安全前置不因批量而减少）"
    );
}

/// 不变量 4：在飞峰值恒 ≤ 并发上界。
#[tokio::test]
async fn batch_never_exceeds_concurrency_bound_in_production_runtime() {
    for bound in [1usize, 2, 4, 8] {
        let exec = Arc::new(CountingExecutor::new(5));
        let runtime = runtime_with(Arc::clone(&exec), echo_only_policy());

        let requests: Vec<DispatchRequest> = (0..32).map(|i| request("echo", i)).collect();
        let outcome = runtime
            .execute_batch(
                &requests,
                BatchLimits {
                    concurrency: bound,
                    per_op_timeout_ms: 0,
                    deadline_ms: 0,
                },
            )
            .await;

        assert_eq!(outcome.succeeded, requests.len());
        assert!(
            exec.peak() <= bound,
            "在飞峰值 {} 超过上界 {bound}",
            exec.peak()
        );
    }
}

/// 不变量 5：`concurrency == 0` 等价串行（峰值 1），不得解释为「无限制」。
#[tokio::test]
async fn batch_zero_concurrency_is_serial_not_unbounded() {
    let exec = Arc::new(CountingExecutor::new(5));
    let runtime = runtime_with(Arc::clone(&exec), echo_only_policy());

    let requests: Vec<DispatchRequest> = (0..16).map(|i| request("echo", i)).collect();
    let outcome = runtime
        .execute_batch(
            &requests,
            BatchLimits {
                concurrency: 0,
                per_op_timeout_ms: 0,
                deadline_ms: 0,
            },
        )
        .await;

    assert_eq!(outcome.succeeded, requests.len());
    assert_eq!(
        exec.peak(),
        1,
        "concurrency=0 必须等价串行（在飞峰值 1），不得解释为无限制"
    );
}

/// 批量与逐条串行的**归因**逐位一致（含错误变体与文案）。
///
/// WHY 与既有 proptest 并存：既有 proptest 用回显执行器（无拒绝路径的文案差异），
/// 本测试额外覆盖**拒绝路径**下批量与串行的错误归因一致 —— 这是
/// 「上层按下标消费结果」的正确性前提。
#[tokio::test]
async fn batch_rejection_attribution_matches_serial() {
    let exec = Arc::new(CountingExecutor::new(0));
    let runtime = runtime_with(Arc::clone(&exec), echo_only_policy());

    let requests: Vec<DispatchRequest> = (0..10)
        .map(|i| {
            if i % 3 == 0 {
                request("echo", i)
            } else {
                request("blocked", i)
            }
        })
        .collect();

    // 串行金标准
    let mut expected = Vec::with_capacity(requests.len());
    for req in &requests {
        expected.push(runtime.dispatch(req).await);
    }
    let calls_after_serial = exec.calls();

    // 批量
    let outcome = runtime
        .execute_batch(
            &requests,
            BatchLimits {
                concurrency: 4,
                per_op_timeout_ms: 0,
                deadline_ms: 0,
            },
        )
        .await;

    assert_eq!(outcome.results.len(), expected.len());
    for (i, (got, want)) in outcome.results.iter().zip(expected.iter()).enumerate() {
        assert_eq!(
            got, want,
            "第 {i} 条的归因（变体 + 文案）必须与串行逐位一致"
        );
    }

    // 批量阶段的增量调用数 = 仅允许项
    let echo_count = requests.iter().filter(|r| r.tool_name == "echo").count();
    assert_eq!(
        exec.calls() - calls_after_serial,
        echo_count,
        "批量阶段仅允许项触达 delegate"
    );
    assert!(outcome.is_conserved(), "四段守恒必须成立");
}

/// 单操作超时只影响对应下标，其余仍成功（隔离性）。
#[tokio::test]
async fn batch_per_op_timeout_isolated_to_its_index() {
    let exec = Arc::new(CountingExecutor::new(80));
    let runtime = runtime_with(Arc::clone(&exec), echo_only_policy());

    let requests: Vec<DispatchRequest> = (0..4).map(|i| request("echo", i)).collect();
    let outcome = runtime
        .execute_batch(
            &requests,
            BatchLimits {
                concurrency: 4,
                per_op_timeout_ms: 10,
                deadline_ms: 0,
            },
        )
        .await;

    assert_eq!(outcome.results.len(), 4);
    for (i, res) in outcome.results.iter().enumerate() {
        assert!(
            matches!(res, Err(DispatchError::Transport(msg)) if msg.contains("operation timeout")),
            "第 {i} 条应为单操作超时，实际: {res:?}"
        );
    }
    assert_eq!(outcome.failed, 4);
    assert!(outcome.is_conserved(), "四段守恒必须成立");
}

/// 全局 deadline 放弃的槽位必须**显式填充**（不静默缺失）。
#[tokio::test]
async fn batch_global_deadline_fills_every_slot() {
    let exec = Arc::new(CountingExecutor::new(60));
    let runtime = runtime_with(Arc::clone(&exec), echo_only_policy());

    let requests: Vec<DispatchRequest> = (0..8).map(|i| request("echo", i)).collect();
    let outcome = runtime
        .execute_batch(
            &requests,
            BatchLimits {
                concurrency: 4,
                per_op_timeout_ms: 0,
                deadline_ms: 25,
            },
        )
        .await;

    assert_eq!(outcome.results.len(), 8, "不得因 deadline 丢槽位");
    assert!(outcome.is_conserved(), "四段守恒必须成立");
    let filled = outcome
        .results
        .iter()
        .filter(|r| {
            matches!(r, Err(DispatchError::Transport(m)) if m.starts_with("batch deadline exceeded"))
        })
        .count();
    assert_eq!(
        filled,
        outcome.abandoned + outcome.not_started,
        "未完成槽位必须显式填充 deadline 错误"
    );
}

/// 归因快照的计数必须与 `BatchOutcome` 逐字段一致（ADR-241 P-13）。
#[tokio::test]
async fn batch_attribution_matches_outcome_counts() {
    let exec = Arc::new(CountingExecutor::new(0));
    let runtime = runtime_with(Arc::clone(&exec), echo_only_policy());

    // 4 允许 + 6 拒绝
    let requests: Vec<DispatchRequest> = (0..10)
        .map(|i| {
            if i < 4 {
                request("echo", i)
            } else {
                request("blocked", i)
            }
        })
        .collect();

    let outcome = runtime
        .execute_batch(
            &requests,
            BatchLimits {
                concurrency: 4,
                per_op_timeout_ms: 0,
                deadline_ms: 0,
            },
        )
        .await;

    let attribution = outcome.attribution();
    assert_eq!(attribution.total, outcome.results.len());
    assert_eq!(attribution.succeeded, outcome.succeeded);
    assert_eq!(attribution.failed, outcome.failed);
    assert_eq!(attribution.abandoned, outcome.abandoned);
    assert_eq!(attribution.not_started, outcome.not_started);
    // 成功率千分比：4/10 = 4000 bp
    assert_eq!(attribution.success_ratio_bp, 4_000);
}

/// 空批次的归因不得除零（比率必须为 0 而非 panic）。
#[tokio::test]
async fn batch_attribution_on_empty_batch_is_zero() {
    let exec = Arc::new(CountingExecutor::new(0));
    let runtime = runtime_with(Arc::clone(&exec), echo_only_policy());

    let outcome = runtime
        .execute_batch(
            &[],
            BatchLimits {
                concurrency: 4,
                per_op_timeout_ms: 0,
                deadline_ms: 0,
            },
        )
        .await;

    let attribution = outcome.attribution();
    assert_eq!(attribution.total, 0);
    assert_eq!(attribution.success_ratio_bp, 0);
    assert_eq!(
        attribution.succeeded
            + attribution.failed
            + attribution.abandoned
            + attribution.not_started,
        0
    );
}

/// 归因快照**只含有限取值域的聚合字段** —— 不得含高基数（工具名/错误文案）。
///
/// WHY 这条重要：高基数标签会让时序库基数爆炸。工具名与错误文案都是
/// 无界的；本快照必须只含计数与比率。
#[test]
fn batch_attribution_has_no_high_cardinality_fields() {
    // 用类型系统层面保证：快照字段只允许 usize / u32
    let attribution = gqep_executor::tool_runtime::BatchAttribution {
        total: 1,
        succeeded: 1,
        failed: 0,
        abandoned: 0,
        not_started: 0,
        success_ratio_bp: 10_000,
    };
    // 逐字段断言类型（编译期 + 运行期双重）
    let _: usize = attribution.total;
    let _: usize = attribution.succeeded;
    let _: usize = attribution.failed;
    let _: usize = attribution.abandoned;
    let _: usize = attribution.not_started;
    let _: u32 = attribution.success_ratio_bp;
    // 快照是 Copy（小值，可按值传递）
    let copied = attribution;
    assert_eq!(copied, attribution);
}
