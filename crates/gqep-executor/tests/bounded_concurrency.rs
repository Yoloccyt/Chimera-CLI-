//! ADR-202 并发上界生效断测（I4 / I5 / I6 / I7 / I8）
//!
//! 纪律：每条不变量都以**断言**兑现，不接受注释或文档承诺。
//! 其中 I5（四段守恒 + 未启动显式归因）与 I6（在飞峰值 ≤ bound）为**红线级**。
//!
//! 对应架构层：L7 Execution（gqep-executor）
//! 上游决议：`docs/architecture/ADR-202-gqep-concurrency-bound.md` §4 / §11

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use event_bus::EventBus;
use gqep_executor::{GatherResult, GqepConfig, GqepError, GqepExecutor, GqepFuture};

/// 四段守恒断言（ADR-202 I5，红线级）
///
/// WHY 独立助手而非就地 assert：本不变量须在 gather / gather_collected /
/// gather_atomic **三个入口**分别兑现（Annex C-5），三处各写一遍迟早会漂。
fn assert_four_segment_conserved(r: &GatherResult, ctx: &str) {
    let accounted = r.succeeded + r.failed + r.abandoned + r.not_started;
    assert_eq!(
        accounted, r.total,
        "{ctx}: 四段守恒必须成立 —— succeeded={} failed={} abandoned={} not_started={} 合计={accounted} != total={}",
        r.succeeded,
        r.failed,
        r.abandoned,
        r.not_started,
        r.total,
    );
}

/// I6（红线级）：在飞峰值 ≤ bound，断的是**峰值**不是均值。
///
/// WHY 这是「限流是否真的生效」的唯一硬判据：只看总耗时或最终计数，
/// 一次性全量入流与分批入流可以给出完全相同的结果，从而让「公开但不生效」
/// 的配置字段（本 ADR 立项根因）继续蒙混过关。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_bounded_concurrency_peak_le_bound() {
    const BOUND: usize = 5;
    const N: usize = 40;

    let in_flight = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));

    let futures: Vec<GqepFuture<String>> = (0..N)
        .map(|i| {
            let inflight = Arc::clone(&in_flight);
            let pk = Arc::clone(&peak);
            Box::pin(async move {
                // 进入即计数并锁存峰值；离开时归还额度
                let cur = inflight.fetch_add(1, Ordering::SeqCst) + 1;
                pk.fetch_max(cur, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(5)).await;
                inflight.fetch_sub(1, Ordering::SeqCst);
                Ok(format!("r-{i}"))
            }) as GqepFuture<String>
        })
        .collect();

    let executor = GqepExecutor::new(
        GqepConfig {
            max_concurrency: BOUND,
            ..GqepConfig::default()
        },
        EventBus::new(),
    );
    let result = executor.gather(futures).await;

    let observed_peak = peak.load(Ordering::SeqCst);
    assert!(
        observed_peak <= BOUND,
        "I6 违反：在飞峰值 {observed_peak} 超过上界 {BOUND}（N={N}）——说明并发上界只是配置字段而未约束执行",
    );
    // I7 的伴生判据：延后不是拒绝，全部 40 个最终都应成功完成
    assert_eq!(
        result.succeeded, N as u32,
        "I7：上界只延后不拒绝，全部应完成"
    );
    assert!(result.errors.is_empty(), "上界不得凭空制造错误");
    assert_four_segment_conserved(&result, "I6 用例");
    assert_eq!(result.not_started, 0, "deadline 充裕时不应有未启动操作");
}

/// 阳性对照：峰值探针必须能测到「不限流」时的真实并发，否则上面的 `≤ bound`
/// 断言是个恒假守卫（永绿的空转）。
///
/// WHY 单独一条：「断言通过」与「断言有牙齿」是两回事。若探针计数写错
/// （例如忘了 fetch_add），`test_bounded_concurrency_peak_le_bound` 会永远绿；
/// 本对照用 bound ≥ N（等价不限流）反证峰值确实能超过 BOUND。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_peak_probe_is_not_vacuous() {
    const BOUND: usize = 5;
    const N: usize = 40;

    let in_flight = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));

    let futures: Vec<GqepFuture<String>> = (0..N)
        .map(|i| {
            let inflight = Arc::clone(&in_flight);
            let pk = Arc::clone(&peak);
            Box::pin(async move {
                let cur = inflight.fetch_add(1, Ordering::SeqCst) + 1;
                pk.fetch_max(cur, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(5)).await;
                inflight.fetch_sub(1, Ordering::SeqCst);
                Ok(format!("r-{i}"))
            }) as GqepFuture<String>
        })
        .collect();

    // bound >= N ⇒ 单波全量入流（等价不限流），与 I6 用例唯一差别就是这个上界
    let executor = GqepExecutor::new(
        GqepConfig {
            max_concurrency: N,
            ..GqepConfig::default()
        },
        EventBus::new(),
    );
    let result = executor.gather(futures).await;
    let observed = peak.load(Ordering::SeqCst);

    assert_eq!(result.succeeded, N as u32, "前置条件：全部应执行");
    assert!(
        observed > BOUND,
        "阳性对照失败：bound>=N 时峰值仍不超过 {BOUND}，说明峰值探针未真正计数，\n\
         则 test_bounded_concurrency_peak_le_bound 的 `≤ bound` 断言是恒假守卫（实测峰值 {observed}）",
    );
}

/// I5（红线级）：bound 内 + 全局超时同时发生时，四段守恒且未启动操作被显式归因。
///
/// 场景构造：bound=2、N=20、每操作 200ms、deadline=150ms
/// ⇒ 首波 2 个都被放弃（abandoned=2，且经 OrphanGuard 可观测），
/// 其余 18 个从未入流 ⇒ 只能靠 `not_started` 记账（它们没有 OrphanGuard）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_conservancy_four_segment_under_bound_and_global_timeout() {
    const BOUND: usize = 2;
    const N: usize = 20;

    let futures: Vec<GqepFuture<String>> = (0..N)
        .map(|i| {
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(200)).await;
                Ok(format!("r-{i}"))
            }) as GqepFuture<String>
        })
        .collect();

    let executor = GqepExecutor::new(
        GqepConfig {
            max_concurrency: BOUND,
            gather_deadline_ms: 150,
            default_timeout_ms: 5000,
            ..GqepConfig::default()
        },
        EventBus::new(),
    );
    let result = executor.gather(futures).await;

    assert_four_segment_conserved(&result, "gather 超时 + 限流");
    assert_eq!(
        result.succeeded + result.failed,
        0,
        "首波未完成任务即超时，不应有已完成项"
    );
    assert_eq!(
        result.abandoned, BOUND as u32,
        "已入流的 {BOUND} 个应记为 abandoned（它们被 poll 过，有 OrphanGuard）"
    );
    assert_eq!(
        result.not_started,
        (N - BOUND) as u32,
        "I5 核心：留在 backlog 的 {} 个从未 poll、无孤儿上报，必须由 not_started 显式归因而非静默消失",
        N - BOUND
    );
}

/// I5（红线级）：未启动操作必须同时有**字段**与**事件**两个出口。
///
/// WHY 事件出口单独一条：`not_started` 只落在返回值上时，监控面看不到
/// 「有多少操作根本没跑」，容量压力与静默丢工作量就无法归因（R5 的可丢性风险）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_not_started_futures_are_attributed() {
    use event_bus::NexusEvent;

    const N: usize = 12;

    // 订阅必须在发布前建立（event-bus 反模式：先 subscribe 再交给 executor）
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let futures: Vec<GqepFuture<String>> = (0..N)
        .map(|i| {
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(120)).await;
                Ok(format!("r-{i}"))
            }) as GqepFuture<String>
        })
        .collect();

    let executor = GqepExecutor::new(
        GqepConfig {
            max_concurrency: 3,
            gather_deadline_ms: 50,
            ..GqepConfig::default()
        },
        bus,
    );
    let result = executor.gather(futures).await;

    assert_four_segment_conserved(&result, "事件归因用例");
    assert!(result.not_started > 0, "本用例的前提是存在未启动操作");
    let expected_not_started = result.not_started;

    // 沿仓内既有消费范式：逐个 recv，最多探 6 次防止无限等待
    let mut saw = false;
    for _ in 0..6 {
        match tokio::time::timeout(Duration::from_millis(150), rx.recv()).await {
            Ok(Ok(NexusEvent::GatherTimedOut {
                total,
                abandoned,
                not_started,
                ..
            })) => {
                let _ = abandoned;
                assert_eq!(total, N as u32, "事件 total 应等于入参操作数");
                assert_eq!(
                    not_started, expected_not_started,
                    "I5 事件出口：GatherTimedOut 携带的 not_started 必须与返回值一致"
                );
                assert!(
                    not_started > 0,
                    "I5 事件出口：未启动操作必须进事件，否则监控面无法归因「根本没跑」的量"
                );
                saw = true;
                break;
            }
            Ok(Ok(_)) => continue,
            Ok(Err(_)) | Err(_) => break,
        }
    }
    assert!(
        saw,
        "I5 事件出口：全局超时必须发布带 not_started 的 GatherTimedOut 事件"
    );
}

/// I4：所有批次共享**同一个绝对 deadline**，不得逐批重置。
///
/// 判据设计：bound=2、N=4、每操作 100ms、deadline=150ms。
/// - 正确实现（单一绝对时刻）：首波 100ms 完成，二波要 200ms > 150ms ⇒ 二波被放弃。
/// - 错误实现（每批各自 timeout(150ms)）：两波都能跑完 ⇒ 全部成功，
///   总墙钟变成 `deadline × 批数`，即本 ADR §7 R1 要防的放大。
/// 故断言「不是全部成功」即可把该缺陷钉红。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_deadline_is_single_absolute_across_waves() {
    const BOUND: usize = 2;
    const N: usize = 4;

    let futures: Vec<GqepFuture<String>> = (0..N)
        .map(|i| {
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(100)).await;
                Ok(format!("r-{i}"))
            }) as GqepFuture<String>
        })
        .collect();

    let executor = GqepExecutor::new(
        GqepConfig {
            max_concurrency: BOUND,
            gather_deadline_ms: 150,
            ..GqepConfig::default()
        },
        EventBus::new(),
    );

    let start = std::time::Instant::now();
    let result = executor.gather(futures).await;
    let elapsed = start.elapsed();

    assert_four_segment_conserved(&result, "I4 两波用例");
    assert_eq!(
        result.succeeded,
        N as u32 - BOUND as u32,
        "I4：首波 2 个完成、次波 2 个应被同一个绝对 deadline 放弃；若全部成功则说明 deadline 被逐批重置"
    );
    // 次波不是 not_started：它在首波完成的**瞬间**就被补入流（已 poll），
    // 之后才被绝对 deadline 放弃 ⇒ 归 abandoned（并应有 OrphanGuard 上报）。
    assert_eq!(
        result.abandoned, BOUND as u32,
        "次波已入流后放弃 ⇒ abandoned"
    );
    assert_eq!(result.not_started, 0, "已全部入流 ⇒ 无未启动项");
    assert!(
        elapsed < Duration::from_millis(400),
        "I4：总墙钟不得接近 deadline×批数(300ms)以上，实际 {elapsed:?}"
    );
}

/// I8：`bound >= total` 时与今日行为逐位等价（向后兼容判据）。
///
/// 默认 bound=100 覆盖既有真实消费者（doctor N=11）与既有测试规模，
/// 本用例断言限流对它们是完全 no-op：全成功、无孤儿、无未启动。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_backward_compat_bound_ge_total() {
    const N: usize = 30;
    let futures: Vec<GqepFuture<String>> = (0..N)
        .map(|i| {
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(1)).await;
                Ok(format!("r-{i}"))
            }) as GqepFuture<String>
        })
        .collect();

    let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());
    let result = executor.gather(futures).await;

    assert_eq!(result.succeeded, N as u32, "bound>=N 应全成功");
    assert_eq!(result.failed, 0);
    assert_eq!(result.abandoned, 0, "bound>=N 单波即完成，不应有放弃");
    assert_eq!(result.not_started, 0, "bound>=N 无 backlog");
    assert_eq!(executor.orphan_count(), 0, "不应产生孤儿");
    assert_four_segment_conserved(&result, "I8 兼容用例");
}

/// I1 运行期侧：`bound == 1` 等价串行且**无死锁**。
///
/// WHY 单独一条：钳制逻辑（0→1）在 config.rs 已测，但「1 能否把 gather 跑完」
/// 属执行语义——若分批循环的补波条件写错，bound=1 最容易死锁。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_bound_one_no_deadlock() {
    const N: usize = 6;
    let futures: Vec<GqepFuture<String>> = (0..N)
        .map(|i| {
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(2)).await;
                Ok(format!("r-{i}"))
            }) as GqepFuture<String>
        })
        .collect();

    let executor = GqepExecutor::new(
        GqepConfig {
            max_concurrency: 1,
            gather_deadline_ms: 5000,
            ..GqepConfig::default()
        },
        EventBus::new(),
    );

    let result = tokio::time::timeout(Duration::from_millis(2000), executor.gather(futures))
        .await
        .expect("bound=1 应在 2s 内完成（串行 6 个 2ms 操作），超时即死锁");

    assert_eq!(result.succeeded, N as u32, "串行模式全部应完成");
    assert_four_segment_conserved(&result, "I1 bound=1 用例");
}

/// I7：上界不得引入静默拒绝路径——被延后的操作最终必须执行。
///
/// 与 `mas-sched` 的显式 `ClaimOutcome::Denied` 区分：那里拒绝是一等公民且可归因；
/// 这里限流只改变**何时**执行，不改变**是否**执行（deadline 充裕时）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_bound_defers_rather_than_rejects() {
    const BOUND: usize = 4;
    const N: usize = 24;

    let futures: Vec<GqepFuture<String>> = (0..N)
        .map(|i| {
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(3)).await;
                Ok(format!("r-{i}"))
            }) as GqepFuture<String>
        })
        .collect();

    let executor = GqepExecutor::new(
        GqepConfig {
            max_concurrency: BOUND,
            gather_deadline_ms: 5000,
            ..GqepConfig::default()
        },
        EventBus::new(),
    );
    let result = executor.gather(futures).await;

    assert_eq!(result.succeeded, N as u32, "全部 {} 个都必须被执行", N);
    assert_eq!(result.not_started, 0, "deadline 充裕时不应有未启动操作");
    assert!(result.errors.is_empty(), "不得凭空制造错误（无拒绝变体）");
    assert_four_segment_conserved(&result, "I7 延后非拒绝用例");
}

/// gather_collected 入口同样必须四段守恒（Annex C-5 三入口之一）。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn test_gather_collected_conserved_under_bound() {
    const BOUND: usize = 3;
    const N: usize = 9;

    let futures: Vec<GqepFuture<u32>> = (0..N)
        .map(|i| {
            Box::pin(async move {
                tokio::time::sleep(Duration::from_millis(2)).await;
                Ok(i as u32)
            }) as GqepFuture<u32>
        })
        .collect();

    let executor = GqepExecutor::new(
        GqepConfig {
            max_concurrency: BOUND,
            ..GqepConfig::default()
        },
        EventBus::new(),
    );
    let collected = executor.gather_collected(futures).await;

    assert_four_segment_conserved(&collected.stats, "gather_collected");
    assert_eq!(collected.stats.succeeded, N as u32);
    assert_eq!(collected.values.len(), N, "values 应覆盖全部成功操作");
}

/// I5（红线级）+ Annex C-5：`gather_atomic` 的 break 尾部是**既存**静默消失面。
///
/// 场景取自仓内既有测试语义：10 个操作、第 5 个失败 ⇒ break ⇒ 尾部 5 个
/// future 被直接 drop，从未 poll、从未 OrphanGuard ⇒ 必须记为 not_started。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_gather_atomic_skipped_tail_is_not_started() {
    use std::sync::atomic::AtomicBool;

    const N: usize = 10;
    const FAIL_AT: usize = 4;

    let rolled_back = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&rolled_back);
    // RollbackFn = Box<dyn Fn() -> Pin<Box<dyn Future<Output=()> + Send>> + Send + Sync>，
    // 故必须返回 future 的闭包（沿 error_paths.rs 既有惯用法）而非直接包 async 块。
    let rollback: gqep_executor::RollbackFn = Box::new(move || {
        let flag = Arc::clone(&flag);
        Box::pin(async move {
            flag.store(true, Ordering::SeqCst);
        })
    });

    let futures: Vec<GqepFuture<String>> = (0..N)
        .map(|i| {
            if i == FAIL_AT {
                Box::pin(async move {
                    Err(GqepError::OperationFailed {
                        operation_id: format!("op-{i}"),
                        reason: "intentional".to_string(),
                    })
                }) as GqepFuture<String>
            } else {
                Box::pin(async move { Ok(format!("r-{i}")) }) as GqepFuture<String>
            }
        })
        .collect();

    let executor = GqepExecutor::new(GqepConfig::default(), EventBus::new());
    let result = executor.gather_atomic(futures, rollback).await;

    assert_eq!(
        result.succeeded, FAIL_AT as u32,
        "失败前 {} 个应成功",
        FAIL_AT
    );
    assert_eq!(result.failed, 1, "恰一个失败");
    assert!(
        rolled_back.load(Ordering::SeqCst),
        "任一失败应触发回滚（既有语义不得回退）"
    );
    assert_eq!(
        result.not_started,
        (N - FAIL_AT - 1) as u32,
        "I5：break 后被丢弃的尾部操作从未 poll、无孤儿上报，必须显式记为 not_started（静默消失即违 UNLEARNABLE #3）"
    );
    assert_eq!(
        result.abandoned, 0,
        "本入口顺序执行且无全局 deadline，不应有 abandoned"
    );
    assert_four_segment_conserved(&result, "gather_atomic");
}
