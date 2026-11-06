//! 批量工具派发 — 有界并发 + 输入序结果契约（性能增量 2026-11-06）
//!
//! # 语义契约（硬门槛）
//! `results[i]` 恒对应 `requests[i]`：**结果顺序与输入顺序逐位一致**，
//! 与逐条串行调用同一批请求的返回值等价（当 `deadline_ms = 0` 且不触发
//! 单操作超时时，逐条结果**完全相同**；由 `tests/tool_runtime_batch_proptest.rs`
//! 的等价性 property 锁定）。
//!
//! # 为什么能并行
//! 每个请求独立经四环链（policy → sandbox → route → transport），请求之间
//! **无数据依赖、无共享可变状态**；结果按下标预分配槽位，与完成顺序解耦。
//!
//! # 为什么有界
//! 无上限并发会耗尽运行时资源（项目红线：禁止无上限 spawn）。本实现
//! 采用与 `gatherer.rs` 同构的「预填充 + 完成一个补一个」FIFO 波次：
//! 在飞峰值恒 ≤ `BatchLimits::concurrency`（钳制到 `[1, MAX_GATHER_FANOUT]`）。
//! `0` 等价串行而非「无限制」——与 `GqepConfig::effective_bound` 同口径。
//!
//! # 为什么不用 `buffer_unordered` / 不 spawn
//! - `buffer_unordered` 的未拉取源项**从未被 poll**，全局 deadline 触发时
//!   无任何归因（ADR-202 否决理由）；
//! - 本模块**不调用 `tokio::spawn`**：future 全部在 `FuturesUnordered` 内被
//!   原地 poll，不存在「任务被抛弃但仍运行」的孤儿场景，故无需 QEEP 包装。
//!
//! # 双层超时（与 GQEP 聚集同口径）
//! - 单操作超时：`BatchLimits::per_op_timeout_ms`（`0` = 禁用）
//! - 全局 deadline：`BatchLimits::deadline_ms`（`0` = 禁用），用
//!   `tokio::time::timeout_at` 一个**绝对时刻**包裹整批，不按波次重置。

use std::collections::VecDeque;
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use futures::stream::{FuturesUnordered, StreamExt};

use crate::config::MAX_GATHER_FANOUT;

use super::dispatch::{DispatchError, DispatchOutcome, DispatchRequest, ToolDispatcher};

/// 全局 deadline 放弃/未启动的槽位错误前缀（供上层按前缀识别可重试性）
pub const BATCH_DEADLINE_PREFIX: &str = "batch deadline exceeded";

/// 批量派发限额
///
/// WHY 显式结构体而非散参数：三个阈值语义不同（并发上界 / 单操作超时 /
/// 整批 deadline），散成三个 `u64` 极易按位置传错。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchLimits {
    /// 在飞并发上界（钳制到 `[1, MAX_GATHER_FANOUT]`；`0` 等价串行）
    pub concurrency: usize,
    /// 单操作超时（毫秒；`0` = 禁用单操作超时）
    pub per_op_timeout_ms: u64,
    /// 整批全局 deadline（毫秒；`0` = 禁用全局 deadline）
    pub deadline_ms: u64,
}

impl Default for BatchLimits {
    /// 默认值：8 路并发 / 单操作 30s / 整批 120s
    ///
    /// WHY 8：本机与主流 CI runner 的物理核数下界；既足以覆盖 I/O 等待，
    /// 又不会在单进程内把共享策略计数锁打成争用热点。调用方可显式覆盖。
    fn default() -> Self {
        Self {
            concurrency: 8,
            per_op_timeout_ms: 30_000,
            deadline_ms: 120_000,
        }
    }
}

/// 批量派发结果 — `results` 与输入等长同序
#[derive(Debug, Clone, PartialEq)]
pub struct BatchOutcome {
    /// 逐请求结果（**输入序**；`results[i]` ↔ `requests[i]`）
    pub results: Vec<Result<DispatchOutcome, DispatchError>>,
    /// 成功数（`results` 中 `Ok` 的个数）
    pub succeeded: usize,
    /// 执行失败数（含单操作超时；不含全局 deadline 放弃/未启动）
    pub failed: usize,
    /// 已入流被 poll、但被全局 deadline 放弃的请求数
    pub abandoned: usize,
    /// 从未入流、从未被 poll 的请求数（并发上界下的 backlog 尾部）
    pub not_started: usize,
}

impl BatchOutcome {
    /// 空批次结果
    #[must_use]
    pub fn empty() -> Self {
        Self {
            results: Vec::new(),
            succeeded: 0,
            failed: 0,
            abandoned: 0,
            not_started: 0,
        }
    }

    /// 四段守恒：`succeeded + failed + abandoned + not_started == results.len()`
    ///
    /// WHY 提供方法而非让调用方各自算：守恒式写两处即双源，漂移会假绿。
    #[must_use]
    pub fn is_conserved(&self) -> bool {
        self.succeeded + self.failed + self.abandoned + self.not_started == self.results.len()
    }

    /// 批量归因快照 — 聚合计数，供在线观测
    ///
    /// # WHY 只有聚合计数（无高基数标签）
    ///
    /// 工具名、参数、错误文案都是**高基数**的 —— 把它们放进指标标签会让
    /// 时序库的基数爆炸（ADR-241 D5 的教训同样适用于指标面）。
    /// 本快照只含**有限取值域**的计数与比率，可安全作为指标标签值。
    ///
    /// # WHY 快照而非直接在批量内部累加全局计数器
    ///
    /// 全局静态计数器无法区分「哪一次批量」——而排查并行化问题恰恰需要
    /// 逐批次的视角。快照由调用方按需取用并自行聚合（上层可按需推入
    /// 指标系统），保持本 crate **零指标依赖**（不引入 prometheus/telemetry）。
    #[must_use]
    pub fn attribution(&self) -> BatchAttribution {
        let total = self.results.len();
        BatchAttribution {
            total,
            succeeded: self.succeeded,
            failed: self.failed,
            abandoned: self.abandoned,
            not_started: self.not_started,
            success_ratio_bp: if total == 0 {
                0
            } else {
                ((self.succeeded as u128 * 10_000) / total as u128) as u32
            },
        }
    }
}

/// 批量归因快照 — 有限取值域的聚合计数
///
/// # 数值纪律
///
/// 比率用**千分比 `u32`**（项目红线：比率不进浮点相等比较）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchAttribution {
    /// 请求总数
    pub total: usize,
    /// 成功数
    pub succeeded: usize,
    /// 失败数（含单操作超时）
    pub failed: usize,
    /// 已入流被全局 deadline 放弃的请求数
    pub abandoned: usize,
    /// 从未入流的请求数
    pub not_started: usize,
    /// 成功率（千分比；`total == 0` 时为 0）
    pub success_ratio_bp: u32,
}

/// 单条批量任务 future — 显式 boxed 以统一 `FuturesUnordered` 元素类型
type BatchItemFuture<'a> =
    Pin<Box<dyn Future<Output = (usize, Result<DispatchOutcome, DispatchError>)> + Send + 'a>>;

/// 有界并发派发一批请求，结果按输入序返回
///
/// # 参数
/// - `dispatcher`：链头派发器（四环链的 `SandboxedDispatcher` 或任意 `dyn ToolDispatcher`）
/// - `requests`：请求列表；结果与之一一对应
/// - `limits`：并发上界与双层超时
///
/// # 返回
/// [`BatchOutcome`]；`deadline` 放弃的槽位以
/// `DispatchError::Transport("batch deadline exceeded: abandoned|not started")`
/// **显式填充**（不允许静默缺失）。
///
/// # 取消安全
/// 本函数被外部 drop 时，`FuturesUnordered` 内未完成的 dispatch future 一并
/// drop；由于未 spawn，不存在仍在后台运行的孤儿任务。
pub async fn dispatch_batch<'a>(
    dispatcher: &'a dyn ToolDispatcher,
    requests: &'a [DispatchRequest],
    limits: BatchLimits,
) -> BatchOutcome {
    let total = requests.len();
    if total == 0 {
        return BatchOutcome::empty();
    }

    // 上界钳制：0 → 1（串行），超大 → 静态帽；与 GqepConfig::effective_bound 同口径
    let bound = limits.concurrency.clamp(1, MAX_GATHER_FANOUT);
    let per_op_ms = limits.per_op_timeout_ms;

    // 结果槽：按下标预分配，完成顺序与回填顺序解耦
    let mut slots: Vec<Option<Result<DispatchOutcome, DispatchError>>> =
        (0..total).map(|_| None).collect();

    // 惰性 future 队列：入队不产生在飞额度（首次 poll 才真正开始派发）
    let mut backlog: VecDeque<BatchItemFuture<'a>> = requests
        .iter()
        .enumerate()
        .map(|(idx, req)| -> BatchItemFuture<'a> {
            Box::pin(async move {
                let outcome = if per_op_ms == 0 {
                    dispatcher.dispatch(req).await
                } else {
                    match tokio::time::timeout(
                        Duration::from_millis(per_op_ms),
                        dispatcher.dispatch(req),
                    )
                    .await
                    {
                        Ok(outcome) => outcome,
                        Err(_) => Err(DispatchError::Transport(format!(
                            "operation timeout after {per_op_ms} ms"
                        ))),
                    }
                };
                (idx, outcome)
            })
        })
        .collect();

    // 首波填充（≤ bound）
    let mut stream: FuturesUnordered<BatchItemFuture<'a>> = FuturesUnordered::new();
    while stream.len() < bound {
        let Some(next) = backlog.pop_front() else {
            break;
        };
        stream.push(next);
    }

    // 采集 + 补波（完成一个补一个，FIFO；禁止按结果价值重排）
    let collect = async {
        while let Some((idx, outcome)) = stream.next().await {
            slots[idx] = Some(outcome);
            if let Some(next) = backlog.pop_front() {
                stream.push(next);
            }
        }
    };

    if limits.deadline_ms == 0 {
        collect.await;
    } else {
        // 先绑定 outcome，使 Timeout 临时量在本语句结束处 drop，释放对 slots 的借用
        let deadline_at = tokio::time::Instant::now() + Duration::from_millis(limits.deadline_ms);
        let _ = tokio::time::timeout_at(deadline_at, collect).await;
    }

    // not_started = backlog 残留：它们从未入流、从未被 poll
    let not_started = backlog.len();
    let mut succeeded = 0usize;
    let mut failed = 0usize;
    let mut results = Vec::with_capacity(total);
    for (idx, slot) in slots.into_iter().enumerate() {
        match slot {
            Some(outcome) => {
                if outcome.is_ok() {
                    succeeded += 1;
                } else {
                    failed += 1;
                }
                results.push(outcome);
            }
            None => {
                // FIFO 波次保证：backlog 残留恰是输入序的尾段，其余未完成者均为已入流被放弃
                let reason = if idx >= total - not_started {
                    "not started"
                } else {
                    "abandoned"
                };
                results.push(Err(DispatchError::Transport(format!(
                    "{BATCH_DEADLINE_PREFIX}: {reason}"
                ))));
            }
        }
    }
    let abandoned = total - not_started - succeeded - failed;

    let outcome = BatchOutcome {
        results,
        succeeded,
        failed,
        abandoned,
        not_started,
    };

    // 在线归因（ADR-241 P-13）：批量派发的逐批次聚合计数。
    //
    // WHY 用 tracing 而非指标库：本 crate 保持**零指标依赖**（不引入
    // prometheus/telemetry），只发布**有限取值域**的结构化字段 ——
    // 上层（CLI/TUI）可按需把字段推入自己的指标系统。
    //
    // WHY 不打工具名/错误文案：那是高基数，会让时序库基数爆炸。
    // 逐条细节由 `BatchOutcome::results` 按需取用。
    let attribution = outcome.attribution();
    tracing::debug!(
        total = attribution.total,
        succeeded = attribution.succeeded,
        failed = attribution.failed,
        abandoned = attribution.abandoned,
        not_started = attribution.not_started,
        success_ratio_bp = attribution.success_ratio_bp,
        "tool batch dispatch completed"
    );

    outcome
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use crate::tool_runtime::dispatch::DispatchFuture;

    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    /// 可控延迟的派发器 + 在飞峰值统计
    struct DelayDispatcher {
        delay_ms: u64,
        in_flight: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
        calls: Arc<AtomicUsize>,
        fail_tool: Option<&'static str>,
    }

    impl ToolDispatcher for DelayDispatcher {
        fn dispatch<'a>(&'a self, req: &'a DispatchRequest) -> DispatchFuture<'a> {
            let delay_ms = self.delay_ms;
            let in_flight = Arc::clone(&self.in_flight);
            let peak = Arc::clone(&self.peak);
            let calls = Arc::clone(&self.calls);
            let fail = self.fail_tool == Some(req.tool_name.as_str());
            Box::pin(async move {
                let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                calls.fetch_add(1, Ordering::SeqCst);
                if delay_ms > 0 {
                    tokio::time::sleep(Duration::from_millis(delay_ms)).await;
                }
                in_flight.fetch_sub(1, Ordering::SeqCst);
                if fail {
                    return Err(DispatchError::Transport("injected failure".into()));
                }
                Ok(DispatchOutcome {
                    content_json: req.args_json.clone(),
                })
            })
        }
    }

    fn request(i: usize) -> DispatchRequest {
        DispatchRequest {
            tool_name: format!("tool-{i}"),
            args_json: format!("{{\"i\":{i}}}"),
        }
    }

    fn requests(n: usize) -> Vec<DispatchRequest> {
        (0..n).map(request).collect()
    }

    /// 逐条串行调用作为对照基准（语义等价的金标准）
    async fn serial(
        dispatcher: &dyn ToolDispatcher,
        reqs: &[DispatchRequest],
    ) -> Vec<Result<DispatchOutcome, DispatchError>> {
        let mut out = Vec::with_capacity(reqs.len());
        for req in reqs {
            out.push(dispatcher.dispatch(req).await);
        }
        out
    }

    fn delay_dispatcher(
        delay_ms: u64,
        in_flight: &Arc<AtomicUsize>,
        peak: &Arc<AtomicUsize>,
        calls: &Arc<AtomicUsize>,
    ) -> DelayDispatcher {
        DelayDispatcher {
            delay_ms,
            in_flight: Arc::clone(in_flight),
            peak: Arc::clone(peak),
            calls: Arc::clone(calls),
            fail_tool: None,
        }
    }

    /// 顺序契约：并行结果与串行结果逐位一致（含乱序完成场景）
    #[tokio::test]
    async fn batch_results_match_serial_elementwise() {
        let in_flight = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let calls = Arc::new(AtomicUsize::new(0));
        let dispatcher = delay_dispatcher(1, &in_flight, &peak, &calls);
        let reqs = requests(12);

        let expected = serial(&dispatcher, &reqs).await;
        let calls_after_serial = calls.load(Ordering::SeqCst);
        let limits = BatchLimits {
            concurrency: 4,
            per_op_timeout_ms: 0,
            deadline_ms: 0,
        };
        let outcome = dispatch_batch(&dispatcher, &reqs, limits).await;

        assert_eq!(outcome.results.len(), reqs.len());
        for (i, (got, want)) in outcome.results.iter().zip(expected.iter()).enumerate() {
            assert_eq!(got, want, "第 {i} 条结果必须与串行逐位一致");
        }
        assert!(outcome.is_conserved(), "四段守恒必须成立: {outcome:?}");
        assert_eq!(outcome.succeeded, reqs.len());
        // 只看批量阶段的增量（串行对照阶段已经消耗了 N 次调用）
        let batch_calls = calls.load(Ordering::SeqCst) - calls_after_serial;
        assert_eq!(batch_calls, reqs.len(), "批量阶段每条请求恰好执行一次");
    }

    /// 并发上界：在飞峰值不得超过 bound
    #[tokio::test]
    async fn batch_never_exceeds_concurrency_bound() {
        let in_flight = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let calls = Arc::new(AtomicUsize::new(0));
        let dispatcher = delay_dispatcher(2, &in_flight, &peak, &calls);
        let reqs = requests(16);

        let limits = BatchLimits {
            concurrency: 4,
            per_op_timeout_ms: 0,
            deadline_ms: 0,
        };
        let outcome = dispatch_batch(&dispatcher, &reqs, limits).await;

        assert_eq!(outcome.succeeded, 16);
        assert!(
            peak.load(Ordering::SeqCst) <= 4,
            "在飞峰值 {} 超过上界 4",
            peak.load(Ordering::SeqCst)
        );
        assert!(
            peak.load(Ordering::SeqCst) >= 2,
            "应当发生并行（峰值 ≥ 2），实际 {}",
            peak.load(Ordering::SeqCst)
        );
    }

    /// 单操作超时：只影响对应下标，其余仍成功
    #[tokio::test]
    async fn batch_per_op_timeout_isolated_to_index() {
        let in_flight = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let calls = Arc::new(AtomicUsize::new(0));
        let dispatcher = DelayDispatcher {
            delay_ms: 80,
            in_flight: Arc::clone(&in_flight),
            peak: Arc::clone(&peak),
            calls: Arc::clone(&calls),
            fail_tool: None,
        };
        // 3 条请求、并发 1（串行语义），单操作超时 10ms → 全部超时
        let reqs = requests(3);
        let limits = BatchLimits {
            concurrency: 1,
            per_op_timeout_ms: 10,
            deadline_ms: 0,
        };
        let outcome = dispatch_batch(&dispatcher, &reqs, limits).await;

        assert_eq!(outcome.results.len(), 3);
        for (i, res) in outcome.results.iter().enumerate() {
            let err = res.as_ref().expect_err("超时应为 Err");
            assert!(
                matches!(err, DispatchError::Transport(msg) if msg.contains("operation timeout")),
                "第 {i} 条应为单操作超时，实际: {res:?}"
            );
        }
        assert_eq!(outcome.failed, 3);
        assert_eq!(outcome.abandoned, 0);
        assert_eq!(outcome.not_started, 0);
        assert!(outcome.is_conserved());
    }

    /// 全局 deadline：所有槽位都有结果（显式填充），四段守恒
    #[tokio::test]
    async fn batch_global_deadline_fills_every_slot() {
        let in_flight = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let calls = Arc::new(AtomicUsize::new(0));
        let dispatcher = delay_dispatcher(60, &in_flight, &peak, &calls);
        let reqs = requests(8);
        let limits = BatchLimits {
            concurrency: 4,
            per_op_timeout_ms: 0,
            deadline_ms: 25,
        };
        let outcome = dispatch_batch(&dispatcher, &reqs, limits).await;

        assert_eq!(outcome.results.len(), 8, "不得因 deadline 丢槽位");
        assert!(outcome.is_conserved(), "四段守恒必须成立: {outcome:?}");
        assert!(outcome.abandoned > 0, "应有已入流被放弃的请求");
        assert!(outcome.not_started > 0, "应有从未入流的请求");
        let filled = outcome
            .results
            .iter()
            .filter(|r| matches!(r, Err(DispatchError::Transport(m)) if m.starts_with(BATCH_DEADLINE_PREFIX)))
            .count();
        assert_eq!(
            filled,
            outcome.abandoned + outcome.not_started,
            "未完成槽位必须显式填充 deadline 错误"
        );
    }

    /// 空批次：不 panic、不执行
    #[tokio::test]
    async fn batch_empty_returns_empty() {
        let in_flight = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let calls = Arc::new(AtomicUsize::new(0));
        let dispatcher = delay_dispatcher(0, &in_flight, &peak, &calls);
        let outcome = dispatch_batch(&dispatcher, &[], BatchLimits::default()).await;
        assert_eq!(outcome, BatchOutcome::empty());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    /// 并发 0 钳制为 1（串行），不得解释为「无限制」
    #[tokio::test]
    async fn batch_zero_concurrency_clamps_to_serial() {
        let in_flight = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let calls = Arc::new(AtomicUsize::new(0));
        let dispatcher = delay_dispatcher(1, &in_flight, &peak, &calls);
        let reqs = requests(4);
        let limits = BatchLimits {
            concurrency: 0,
            per_op_timeout_ms: 0,
            deadline_ms: 0,
        };
        let outcome = dispatch_batch(&dispatcher, &reqs, limits).await;
        assert_eq!(outcome.succeeded, 4);
        assert_eq!(
            peak.load(Ordering::SeqCst),
            1,
            "concurrency=0 必须等价串行（在飞峰值 1）"
        );
    }

    /// 单条失败不污染其他下标（结果归因逐位正确）
    #[tokio::test]
    async fn batch_single_failure_indexed_correctly() {
        let in_flight = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let calls = Arc::new(AtomicUsize::new(0));
        let dispatcher = DelayDispatcher {
            delay_ms: 1,
            in_flight: Arc::clone(&in_flight),
            peak: Arc::clone(&peak),
            calls: Arc::clone(&calls),
            fail_tool: Some("tool-2"),
        };
        let reqs = requests(5);
        let outcome = dispatch_batch(&dispatcher, &reqs, BatchLimits::default()).await;

        assert!(outcome.results[2].is_err(), "tool-2 应失败");
        for i in [0usize, 1, 3, 4] {
            assert!(outcome.results[i].is_ok(), "tool-{i} 应成功");
        }
        assert_eq!(outcome.failed, 1);
        assert_eq!(outcome.succeeded, 4);
        assert!(outcome.is_conserved());
    }
}
