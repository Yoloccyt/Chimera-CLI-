//! 统一调用预算包装 — 超时 + CancellationToken 注入(M2, M1-方向5)
//!
//! 对应架构层:L10 Interface(chimera-cli 本层防护,非传输层)
//!
//! # 动机(M1-方向5 痛点证据)
//! chimera-cli 自身的 LLM/长操作调用点(如 `QuestEngine::create_quest`)此前
//! 几乎无本层超时,依赖 mca-gateway 传输层(`transport.rs:220` 的 per-endpoint
//! `timeout_ms`)兜底。一旦绕过 gateway 直连(或 `mca-gateway` GATED 未编译),
//! 超时防护整体消失。本模块为 CLI 本层提供**纯增量**的兜底包装。
//!
//! # 设计决策(WHY)
//! - **默认时长对齐 mca-gateway per-endpoint 口径**:`DEFAULT_CALL_BUDGET`
//!   = 120s(`affinity.d/*.toml` 主流 `timeout_ms = 120000`,transport.rs:220
//!   直接应用该值)。纯增量防护:默认不比 gateway 口径更紧,对外 CLI 行为无感;
//!   历史"超 120s 永久挂起"路径现在转为"发事件 + 返回 Timeout 错误"。
//! - **超时语义沿用 gqep-executor `timeout.rs` 模式**:超时发布
//!   `NexusEvent::OperationTimedOut`(携带 `operation_id`/`timeout_ms`)经
//!   EventBus 广播,绝不静默吞错;失败仅 `warn` 日志(同 gqep 惯例)。
//!   ephemeral bus 无订阅者时 publish 照常成功,属正常(同现有模式)。
//! - **`Duration::ZERO` = 不超时**:沿用 gqep `timeout_ms == 0` 口径
//!   (显式放弃超时防护的场景),但仍响应取消。
//! - **取消传播**:future 内联轮询(不 `spawn`),超时/取消分支胜出时
//!   future 随作用域退出立即 Drop —— 不泄漏任务,取消经析构传进调用栈。
//! - **降级路径显式记档**:ephemeral bus 发布失败(如接收方全部关闭)仅
//!   `warn` 日志,不 panic、不回改返回路径 —— 超时错误的返回不依赖事件
//!   投递成功(fail-open 降级:事件丢失时退化为"日志可观测",此为有意设计)。
//!
//! # 与 gqep `with_timeout` 的差异
//! gqep 包装的是 L7 聚集执行(Box\<dyn Future\> + 计数探针);本包装面向
//! L10 调用点的泛型 future,额外注入 `CancellationToken`(gqep 无取消概念)。
//! 两者超时事件契约一致(`OperationTimedOut`),订阅方(efficiency-monitor)
//! 无需区分来源。

use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use event_bus::{EventBus, EventMetadata, NexusEvent};
use thiserror::Error;
use tokio_util::sync::CancellationToken;
use tracing::warn;

/// 默认调用预算 — 对齐 mca-gateway per-endpoint 超时口径
///
/// WHY 120s:`mca-gateway/affinity.d/` 实测 7 文件 12 个 endpoint
/// (5×`timeout_ms = 120000` / 5×60000 / 2×180000),120000 是各文件
/// 旗舰模型的首列值与最大常规值;`transport.rs:220` 直接
/// `Duration::from_millis(endpoint.timeout_ms)`。取该值使本层防护与
/// gateway 口径一致(不收紧现有行为);特定调用点可传自定义 budget。
pub const DEFAULT_CALL_BUDGET: Duration = Duration::from_millis(120_000);

/// 调用预算错误 — 包装器的两类失败(内部 future 的输出原样透传,不在此包装)
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CallBudgetError {
    /// 操作超过预算时长(GQEP 同语义);`OperationTimedOut` 事件已尝试发布
    #[error("OperationTimeout: {operation_id} 超过预算 {budget_ms}ms")]
    Timeout {
        /// 操作标识(与事件/日志关联)
        operation_id: String,
        /// 触发超时的预算时长(毫秒)
        budget_ms: u64,
    },
    /// 操作被 `CancellationToken` 取消(取消≠超时,不发布超时事件)
    #[error("OperationCancelled: {operation_id} 已被取消")]
    Cancelled {
        /// 操作标识
        operation_id: String,
    },
}

/// 统一调用预算包装 — 超时 + CancellationToken 注入
///
/// 三分支语义(与任务清单对齐):
/// 1. **正常返回**:`future` 在预算内完成 → `Ok(future 输出)`(原样透传)
/// 2. **超时触发**:预算耗尽 → 发布 `OperationTimedOut` 事件 + `Err(Timeout)`
/// 3. **取消传播**:token 触发 → `Err(Cancelled)`,future 立即 Drop
///
/// # 参数
/// - `future`:待包装的异步操作(按值传入,内部 pin;不 spawn)
/// - `budget`:预算时长;`Duration::ZERO` = 不超时(沿用 gqep 口径,仍响应取消)
/// - `token`:取消令牌;已取消则 future 一次都不会被轮询
/// - `operation_id`:操作标识(事件追踪/日志关联/错误携带)
/// - `bus`:事件总线(ephemeral;无订阅者时发布照常成功)
///
/// # 边界行为
/// - **零时长 budget**:`Duration::ZERO` → 永不超时(显式放弃超时防护);
///   预先取消的 token 仍然立即返回 `Cancelled`
/// - **预先取消的 token**:返回 `Cancelled`,future 不被轮询(无早退副作用)
/// - **内部 future 错误**:不在此包装 —— `Ok(Err(inner))` 原样透传
///   (同 gqep `with_timeout`:超时器只裁决"是否给足时间",不改写结果)
pub async fn call_with_budget<T, F>(
    future: F,
    budget: Duration,
    token: &CancellationToken,
    operation_id: &str,
    bus: &EventBus,
) -> Result<T, CallBudgetError>
where
    F: Future<Output = T>,
{
    let operation_id_owned = operation_id.to_string();

    // 预先取消:future 一次都不轮询(避免早退副作用,如重复发布/写状态)
    if token.is_cancelled() {
        return Err(CallBudgetError::Cancelled {
            operation_id: operation_id_owned,
        });
    }

    tokio::pin!(future);

    // 零预算 = 不超时(gqep timeout_ms==0 口径):睡眠分支换为永不就绪
    let sleep: Pin<Box<dyn Future<Output = ()> + Send>> = if budget.is_zero() {
        Box::pin(std::future::pending())
    } else {
        Box::pin(tokio::time::sleep(budget))
    };

    tokio::select! {
        result = &mut future => Ok(result),
        _ = token.cancelled() => Err(CallBudgetError::Cancelled {
            operation_id: operation_id_owned,
        }),
        () = sleep => {
            let budget_ms = budget.as_millis() as u64;
            // 超时事件发布(gqep timeout.rs 同模式):失败仅 warn,不 panic
            let event = NexusEvent::OperationTimedOut {
                metadata: EventMetadata::new("chimera-cli"),
                operation_id: operation_id_owned.clone(),
                timeout_ms: budget_ms,
            };
            if let Err(e) = bus.publish(event).await {
                warn!(error = %e, operation_id = %operation_id_owned,
                      "发布 OperationTimedOut 事件失败(降级:仅日志可观测)");
            }
            Err(CallBudgetError::Timeout {
                operation_id: operation_id_owned,
                budget_ms,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    /// 带 Drop 探针的 future:记录"是否被轮询"与"是否已 Drop"
    /// (恒 Pending,由包装的裁决分支终结;无输出值)
    struct ProbeFuture {
        polled: Arc<AtomicBool>,
        dropped: Arc<AtomicBool>,
    }

    impl Future for ProbeFuture {
        type Output = u32;
        fn poll(self: Pin<&mut Self>, _cx: &mut std::task::Context<'_>) -> std::task::Poll<u32> {
            self.polled.store(true, Ordering::SeqCst);
            // 未完成:恒 Pending,由包装的裁决分支(超时/取消)终结
            std::task::Poll::Pending
        }
    }

    impl Drop for ProbeFuture {
        fn drop(&mut self) {
            self.dropped.store(true, Ordering::SeqCst);
        }
    }

    fn probe() -> (ProbeFuture, Arc<AtomicBool>, Arc<AtomicBool>) {
        let polled = Arc::new(AtomicBool::new(false));
        let dropped = Arc::new(AtomicBool::new(false));
        (
            ProbeFuture {
                polled: Arc::clone(&polled),
                dropped: Arc::clone(&dropped),
            },
            polled,
            dropped,
        )
    }

    // 计数窗口内经 rx 到达的 OperationTimedOut 事件
    //
    // WHY 参数是预先订阅的 rx 而非 bus:broadcast 不缓存历史(§4.4 反模式 #3
    // 的测试侧体现),必须在被测调用**之前**订阅,否则事件已漏收。
    async fn count_timeout_events(
        rx: &mut event_bus::bus::EventReceiver,
        window: Duration,
    ) -> usize {
        let deadline = tokio::time::Instant::now() + window;
        let mut count = 0usize;
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            match rx.recv_timeout(remaining).await {
                Ok(NexusEvent::OperationTimedOut { .. }) => count += 1,
                Ok(_) => {}      // 其他事件忽略
                Err(_) => break, // Closed/Lagged/窗口耗尽
            }
        }
        count
    }

    // ---------- 三分支单测 ----------

    #[tokio::test]
    async fn normal_completion_returns_value() {
        let bus = EventBus::new();
        let token = CancellationToken::new();
        let result = call_with_budget(
            async { 42u32 },
            DEFAULT_CALL_BUDGET,
            &token,
            "op-normal",
            &bus,
        )
        .await;
        assert_eq!(result.expect("正常完成应 Ok"), 42);
    }

    #[tokio::test]
    async fn timeout_triggers_error_and_event() {
        let bus = EventBus::new();
        let token = CancellationToken::new();
        let mut rx = bus.subscribe(); // subscribe-before-call(broadcast 不缓存)
        let (fut, polled, dropped) = probe();
        let result =
            call_with_budget(fut, Duration::from_millis(30), &token, "op-timeout", &bus).await;
        assert!(
            matches!(result, Err(CallBudgetError::Timeout { ref operation_id, budget_ms }) if operation_id == "op-timeout" && budget_ms == 30),
            "应返回 Timeout,实际: {result:?}"
        );
        assert!(polled.load(Ordering::SeqCst), "超时前 future 应已被轮询");
        assert!(
            dropped.load(Ordering::SeqCst),
            "超时后 future 应立即 Drop(不泄漏)"
        );
        assert_eq!(
            count_timeout_events(&mut rx, Duration::from_millis(80)).await,
            1,
            "超时事件恰好一次"
        );
    }

    #[tokio::test]
    async fn cancellation_propagates_and_drops_future() {
        let bus = EventBus::new();
        let token = CancellationToken::new();
        let mut rx = bus.subscribe();
        let (fut, _, dropped) = probe();
        token.cancel();
        let result = call_with_budget(fut, DEFAULT_CALL_BUDGET, &token, "op-cancel", &bus).await;
        assert!(
            matches!(result, Err(CallBudgetError::Cancelled { ref operation_id }) if operation_id == "op-cancel"),
            "应返回 Cancelled,实际: {result:?}"
        );
        assert!(
            dropped.load(Ordering::SeqCst),
            "预先取消也应 Drop future(构造后未轮询即销毁)"
        );
        assert_eq!(
            count_timeout_events(&mut rx, Duration::from_millis(50)).await,
            0,
            "取消路径不发布超时事件"
        );
    }

    // ---------- 边界 ----------

    #[tokio::test]
    async fn zero_budget_means_no_timeout() {
        let bus = EventBus::new();
        let token = CancellationToken::new();
        // 零预算 + 会完成的 future → Ok(gqep 口径)
        let result =
            call_with_budget(async { "done" }, Duration::ZERO, &token, "op-zero", &bus).await;
        assert_eq!(result.expect("零预算不超时"), "done");
        // 零预算 + 预先取消 → 仍响应取消(取消独立于超时)
        let token2 = CancellationToken::new();
        token2.cancel();
        let r2 = call_with_budget(
            async { "x" },
            Duration::ZERO,
            &token2,
            "op-zero-cancel",
            &bus,
        )
        .await;
        assert!(matches!(r2, Err(CallBudgetError::Cancelled { .. })));
    }

    #[tokio::test]
    async fn pre_cancelled_token_never_polls_future() {
        let bus = EventBus::new();
        let token = CancellationToken::new();
        token.cancel();
        let (fut, polled, _) = probe();
        let result = call_with_budget(fut, DEFAULT_CALL_BUDGET, &token, "op-precancel", &bus).await;
        assert!(matches!(result, Err(CallBudgetError::Cancelled { .. })));
        assert!(
            !polled.load(Ordering::SeqCst),
            "预先取消的 token 不得轮询 future(无早退副作用)"
        );
    }

    #[tokio::test]
    async fn inner_error_passthrough_untouched() {
        let bus = EventBus::new();
        let token = CancellationToken::new();
        let mut rx = bus.subscribe();
        // 内部 future 自身返回 Result:包装层不得吞/改(绝不静默吞错)
        let result: Result<Result<u32, String>, CallBudgetError> = call_with_budget(
            async { Err("inner-failure".to_string()) },
            DEFAULT_CALL_BUDGET,
            &token,
            "op-inner-err",
            &bus,
        )
        .await;
        let inner = result.expect("包装层应透传内部错误(Ok 包裹)");
        assert_eq!(inner.unwrap_err(), "inner-failure");
        assert_eq!(
            count_timeout_events(&mut rx, Duration::from_millis(50)).await,
            0
        );
    }

    #[tokio::test]
    async fn cancellation_mid_flight_is_delivered() {
        let bus = EventBus::new();
        let token = CancellationToken::new();
        let mut rx = bus.subscribe();
        let (fut, _, dropped) = probe();
        let token_clone = token.clone();
        let cancels = Arc::new(AtomicUsize::new(0));
        let cancels_clone = Arc::clone(&cancels);
        let driver = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(20)).await;
            cancels_clone.fetch_add(1, Ordering::SeqCst);
            token_clone.cancel();
        });
        let result =
            call_with_budget(fut, Duration::from_secs(60), &token, "op-mid-cancel", &bus).await;
        driver.await.expect("driver 完成");
        assert!(
            matches!(result, Err(CallBudgetError::Cancelled { .. })),
            "飞行中取消应传播"
        );
        assert!(dropped.load(Ordering::SeqCst), "飞行中取消应 Drop future");
        assert_eq!(
            count_timeout_events(&mut rx, Duration::from_millis(50)).await,
            0,
            "取消路径零超时事件"
        );
    }

    // ---------- proptest 属性测试 ----------

    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(64))]

        /// 不早退:完成时间 < 预算 ⇒ 必然 Ok 且值等价(任意随机时长组合)
        ///
        /// WHY 断言全部在 block_on 之外:proptest 宏要求主体类型 `()` 且
        /// `prop_assert*` 的 `return Err` 不得落入嵌套闭包(见 sugar.rs `let (): () = $body`)
        #[test]
        fn prop_completes_within_budget_never_exits_early(
            budget_ms in 60u64..200,
            done_ms in 0u64..30,
        ) {
            let value = done_ms * 7 + 1; // 随机标记值,验证透传等价(u64: Copy)
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all().build().unwrap();
            let (result, events) = rt.block_on(async move {
                let bus = EventBus::new();
                let token = CancellationToken::new();
                let mut rx = bus.subscribe(); // subscribe-before-call
                let result = call_with_budget(
                    async move {
                        tokio::time::sleep(Duration::from_millis(done_ms)).await;
                        value
                    },
                    Duration::from_millis(budget_ms),
                    &token,
                    "prop-early",
                    &bus,
                ).await;
                let events = count_timeout_events(&mut rx, Duration::from_millis(20)).await;
                (result, events)
            });
            prop_assert_eq!(result.unwrap(), value);
            prop_assert_eq!(events, 0, "正常路径零超时事件");
        }

        /// 超时确定性:完成时间 > 预算 ⇒ 必然 Timeout,事件恰好一次
        #[test]
        fn prop_exceeds_budget_always_times_out(
            budget_ms in 20u64..60,
            done_ms in 100u64..160,
        ) {
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all().build().unwrap();
            let (result, events) = rt.block_on(async move {
                let bus = EventBus::new();
                let token = CancellationToken::new();
                let mut rx = bus.subscribe(); // subscribe-before-call
                let result = call_with_budget(
                    async move {
                        tokio::time::sleep(Duration::from_millis(done_ms)).await;
                        done_ms
                    },
                    Duration::from_millis(budget_ms),
                    &token,
                    "prop-timeout",
                    &bus,
                ).await;
                let events = count_timeout_events(&mut rx, Duration::from_millis(100)).await;
                (result, events)
            });
            let is_timeout = matches!(result, Err(CallBudgetError::Timeout { .. }));
            prop_assert!(is_timeout, "应返回 Timeout,实际: {result:?}");
            prop_assert_eq!(events, 1, "超时事件恰好一次");
        }

        /// 不泄漏:超时后 future 同步于返回前 Drop(任意预算下成立)
        #[test]
        fn prop_timeout_drops_future_exactly_once(
            budget_ms in 20u64..60,
        ) {
            struct DropMark(Arc<AtomicBool>);
            impl Future for DropMark {
                type Output = ();
                fn poll(self: Pin<&mut Self>, _cx: &mut std::task::Context<'_>)
                    -> std::task::Poll<()> { std::task::Poll::Pending }
            }
            impl Drop for DropMark {
                fn drop(&mut self) { self.0.store(true, Ordering::SeqCst); }
            }

            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all().build().unwrap();
            let (result, dropped) = rt.block_on(async move {
                let bus = EventBus::new();
                let token = CancellationToken::new();
                let dropped = Arc::new(AtomicBool::new(false));
                let result = call_with_budget(
                    DropMark(Arc::clone(&dropped)),
                    Duration::from_millis(budget_ms),
                    &token,
                    "prop-drop",
                    &bus,
                ).await;
                (result, dropped.load(Ordering::SeqCst))
            });
            let is_timeout = matches!(result, Err(CallBudgetError::Timeout { .. }));
            prop_assert!(is_timeout, "应返回 Timeout,实际: {result:?}");
            prop_assert!(dropped, "返回时 future 必须已 Drop(无泄漏)");
        }

        /// 预先取消的 token:任意预算(含零预算)下 future 都不得被轮询
        #[test]
        fn prop_pre_cancelled_never_polls(
            budget_ms in 0u64..200,
        ) {
            struct PollMark(Arc<AtomicBool>);
            impl Future for PollMark {
                type Output = ();
                fn poll(self: Pin<&mut Self>, _cx: &mut std::task::Context<'_>)
                    -> std::task::Poll<()> {
                    self.0.store(true, Ordering::SeqCst);
                    std::task::Poll::Pending
                }
            }
            impl Drop for PollMark {
                fn drop(&mut self) {}
            }

            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all().build().unwrap();
            let (result, polled, events) = rt.block_on(async move {
                let bus = EventBus::new();
                let token = CancellationToken::new();
                token.cancel();
                let mut rx = bus.subscribe();
                let polled = Arc::new(AtomicBool::new(false));
                let result = call_with_budget(
                    PollMark(Arc::clone(&polled)),
                    Duration::from_millis(budget_ms),
                    &token,
                    "prop-precancel",
                    &bus,
                ).await;
                let events = count_timeout_events(&mut rx, Duration::from_millis(20)).await;
                (result, polled.load(Ordering::SeqCst), events)
            });
            let is_cancelled = matches!(result, Err(CallBudgetError::Cancelled { .. }));
            prop_assert!(is_cancelled, "应返回 Cancelled,实际: {result:?}");
            prop_assert!(!polled, "预先取消不得轮询 future");
            prop_assert_eq!(events, 0);
        }
    }
}
