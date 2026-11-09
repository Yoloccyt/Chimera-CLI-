//! gatherer 惰性装载验收（ADR-274 / W8）
//!
//! # WHY 这个文件
//!
//! 旧实现先把**全部** N 个源 future 包成 entangle 载体（`Box::pin`），再分批入流。
//! 包装本身零副作用（QEEP 注册在首次 poll），所以「先全部包装」只是白付 N 次
//! 装箱 —— 峰值装箱数是 N，而不是并发上界 bound。
//!
//! 惰性装载后：backlog 存**源** future，`admit_future` 只在入波时调用。
//!
//! # 判据选择（诚实说明）
//!
//! 「装箱峰值」在行为上不可观测：包装创建的 async 体在未被 poll 前没有任何副作用。
//! 全局分配器计数也无法归因（同一测试进程里 tokio / 事件总线都在分配）。
//! 故本文件用 **L1 源码结构锁** + **L3 可观测代理**（未入波源 future 的 poll 计数为 0）
//! 两条判据，并按 ADR-234 的分层如实标注，不声称做过分配器级别的测量。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom
#![forbid(unsafe_code)]

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use event_bus::EventBus;
use gqep_executor::{GqepConfig, GqepExecutor, GqepFuture};

fn gatherer_source() -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("src")
        .join("gatherer.rs");
    std::fs::read_to_string(&path).expect("应能读取 gatherer.rs")
}

/// L1 结构锁：装载只有一条路径，且发生在入波处。
///
/// 三条断言各防一种退化：
/// 1. 恢复「先全量包装」的 eager 块 → 第一条红（峰值回到 N）；
/// 2. 首波或补波任一处忘记走 `admit_future` → 第二条红（直接 push 源 future
///    会类型不匹配，但被包一层后仍可能出现「补波绕过惰性」）；
/// 3. 结果容器退回不定容 → 第三条红（扩容搬移 + 峰值翻倍）。
#[test]
fn lazy_admission_is_structural() {
    let src = gatherer_source();

    assert!(
        !src.contains("let wrapped: Vec<GqepFuture<T>>"),
        "不得恢复「先全量包装再入流」的 eager 装载块"
    );
    assert_eq!(
        src.matches("admit_future(qeep.clone(), raw)").count(),
        2,
        "首波与补波必须走同一条惰性装载路径（admit_future）"
    );
    assert!(
        src.contains("fn admit_future<T: Send + 'static>"),
        "装载函数应独立存在（单一路径）"
    );
    assert!(
        src.contains("Vec::with_capacity(capacity)"),
        "values/errors 应按已知 total 定容"
    );
}

/// L3 可观测代理：未入波的源 future **一次都不会被 poll**。
///
/// 这条与既有 `bounded_concurrency` 的 `not_started` 断言互补：后者证明
/// 「记账归因存在」，本条证明「这些操作确实从未启动」——惰性装载不得偷偷
/// 提前 poll（那会在全局超时后被误记 abandoned，且破坏「未启动可安全重试」
/// 的语义）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unadmitted_sources_are_never_polled() {
    const BOUND: usize = 2;
    const N: usize = 8;

    let polls: Arc<Vec<AtomicUsize>> = Arc::new((0..N).map(|_| AtomicUsize::new(0)).collect());
    let futures: Vec<GqepFuture<String>> = (0..N)
        .map(|i| {
            let polls = Arc::clone(&polls);
            Box::pin(async move {
                polls[i].fetch_add(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(200)).await;
                Ok(format!("r-{i}"))
            }) as GqepFuture<String>
        })
        .collect();

    let executor = GqepExecutor::new(
        GqepConfig {
            max_concurrency: BOUND,
            gather_deadline_ms: 100,
            default_timeout_ms: 5_000,
            ..GqepConfig::default()
        },
        EventBus::new(),
    );
    let result = executor.gather(futures).await;

    let polled = polls
        .iter()
        .filter(|p| p.load(Ordering::SeqCst) > 0)
        .count();
    assert_eq!(polled, BOUND, "只有入波的操作会被 poll（惰性装载的下界）");
    assert_eq!(result.abandoned, BOUND as u32, "首波被全局超时放弃");
    assert_eq!(
        result.not_started,
        (N - BOUND) as u32,
        "其余操作从未入波 ⇒ 从未 poll"
    );
    assert_eq!(result.succeeded + result.failed, 0);
    let accounted = result.succeeded + result.failed + result.abandoned + result.not_started;
    assert_eq!(accounted, result.total, "四段守恒必须成立");
}
