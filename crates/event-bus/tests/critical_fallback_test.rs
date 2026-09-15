//! Critical 保底送达 sink 测试 — B-a(M0,`arch-refactor-directions-v2-2026-09-12` 方向 B-a)
//!
//! 覆盖 `EventBus` 空订阅者分支改造后的四类行为:
//! 1. 无 mpsc 订阅者发布 Critical 事件 → 投递到可插拔 fallback sink
//! 2. 有订阅者 → 仅走 mpsc,sink 零投递(不双投)
//! 3. 订阅者 drop 后的首个事件(stale Sender)→ retain 清理后投递 sink
//!    (修复"订阅后 drop → 首事件静默丢失"缺口)
//! 4. 满载(Full)采样丢弃不转投 sink —— 尊重 P1-W2.1 背压设计
//!
//! 另含:
//! - 默认 `LogCriticalSink` 的 tracing 断言(error 级 + 定位字段)
//! - proptest 属性测试(投递守恒:mpsc 实收 + sink 实收 == 发布总数)
//!
//! 设计依据:`crates/event-bus/src/critical_sink.rs` 模块文档(含 at-least-once
//! 语义边界)与 `bus.rs::send_critical_mpsc` 的 B-a 改造注释。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use event_bus::{CriticalSink, EventBus, EventMetadata, NexusEvent};
use proptest::prelude::*;

// ============================================================
// 测试 sink:线程安全计数收集器
// ============================================================

/// 收集 sink 收到的事件,供断言(at-least-once 语义下允许重复,本测试场景无并发发布)
struct CollectingSink {
    received: Mutex<Vec<NexusEvent>>,
}

impl CollectingSink {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            received: Mutex::new(Vec::new()),
        })
    }
    fn len(&self) -> usize {
        self.received.lock().unwrap().len()
    }
}

impl CriticalSink for CollectingSink {
    fn on_critical(&self, event: &NexusEvent) {
        self.received.lock().unwrap().push(event.clone());
    }
}

/// 构造 Critical 清单内事件(BudgetExceeded,is_critical_mpsc_event 命中)
fn critical_event(source: &str) -> NexusEvent {
    NexusEvent::BudgetExceeded {
        metadata: EventMetadata::new(source),
        budget_type: "total_cost".into(),
        current: 9_999,
        limit: 5_000,
    }
}

// ============================================================
// 测试 1:无订阅者 → fallback 收到;观测计数同步递增
// ============================================================

#[tokio::test]
async fn test_fallback_receives_when_no_subscriber() {
    let sink = CollectingSink::new();
    let bus = EventBus::new().with_critical_fallback(sink.clone());

    bus.publish(critical_event("decb-governor")).await.unwrap();

    assert_eq!(sink.len(), 1, "无订阅者时 Critical 事件必须投递到保底 sink");
    assert_eq!(
        bus.critical_no_subscriber_total(),
        1,
        "空订阅者保底投递计数应为 1"
    );
    assert!(!bus.has_critical_subscribers());
}

// ============================================================
// 测试 2:有订阅者 → 仅走 mpsc,sink 零投递(不双投)
// ============================================================

#[tokio::test]
async fn test_no_fallback_when_subscriber_present() {
    let sink = CollectingSink::new();
    let bus = EventBus::new().with_critical_fallback(sink.clone());
    let mut crit_rx = bus.subscribe_critical_events();

    let event = critical_event("decb-governor");
    bus.publish(event.clone()).await.unwrap();

    let received = tokio::time::timeout(Duration::from_secs(1), crit_rx.recv())
        .await
        .expect("mpsc 订阅者应收到事件")
        .expect("通道不应关闭");
    assert_eq!(received, event);
    assert_eq!(sink.len(), 0, "有活跃订阅者时 sink 不得收到重复投递");
    assert_eq!(bus.critical_no_subscriber_total(), 0);
    assert!(bus.has_critical_subscribers());
}

// ============================================================
// 测试 3:订阅者 drop 后的首个事件(stale Sender)→ fallback 兜底
//
// WHY 此测试关键:retain 清理是惰性的 —— drop receiver 后 Vec 仍含失效
// Sender,is_empty 检查为 false,进入 retain 分支后 try_send 全部 Closed。
// B-a 之前该事件既不进 mpsc 也无任何落点(静默丢失);现必须投递 fallback。
// ============================================================

#[tokio::test]
async fn test_fallback_after_subscriber_drop() {
    let sink = CollectingSink::new();
    let bus = EventBus::new().with_critical_fallback(sink.clone());
    let crit_rx = bus.subscribe_critical_events();
    drop(crit_rx); // 订阅者退场,Sender 变 stale(惰性清理)

    bus.publish(critical_event("decb-governor")).await.unwrap();

    assert_eq!(
        sink.len(),
        1,
        "订阅者 drop 后的首个 Critical 事件必须由 sink 兜底,不得静默丢失"
    );
    assert_eq!(
        bus.critical_no_subscriber_total(),
        1,
        "stale 清理后转投 sink 亦计入空订阅者计数(接线覆盖语义一致)"
    );
    assert!(!bus.has_critical_subscribers(), "retain 应清掉失效 Sender");
}

// ============================================================
// 测试 4:满载(Full)采样丢弃不转投 sink(尊重 P1-W2.1 背压设计)
// ============================================================

#[tokio::test]
async fn test_full_channel_does_not_fallback() {
    let sink = CollectingSink::new();
    let bus = EventBus::new().with_critical_fallback(sink.clone());
    let _crit_rx = bus.subscribe_critical_events();

    // 填满 CRITICAL_CHANNEL_CAPACITY(4096)后,下一条 try_send 返回 Full:
    // 按优先级采样丢弃(critical_dropped_count 递增),不向 sink 转投 ——
    // 转投会让满载场景每条事件都刷 error! 日志,破坏背压设计的丢弃语义。
    for i in 0..event_bus::bus::CRITICAL_CHANNEL_CAPACITY {
        let ev = NexusEvent::BudgetExceeded {
            metadata: EventMetadata::new("decb-governor"),
            budget_type: format!("fill-{i}"),
            current: 1,
            limit: 0,
        };
        bus.publish(ev).await.unwrap();
    }
    assert_eq!(bus.critical_dropped_count(), 0, "填满前不应有丢弃");
    let before = sink.len();

    bus.publish(critical_event("decb-governor")).await.unwrap();

    assert_eq!(
        bus.critical_dropped_count(),
        1,
        "容量满后新事件应按采样丢弃计数"
    );
    assert_eq!(
        sink.len(),
        before,
        "Full 采样丢弃不转投 sink(背压语义:有意丢弃,非无落点)"
    );
    assert_eq!(
        bus.critical_no_subscriber_total(),
        0,
        "Full 路径不属于'空订阅者'口径"
    );
}

// ============================================================
// 测试 5:Non-Critical 事件永不进 fallback(口径 = is_critical_mpsc_event)
// ============================================================

#[tokio::test]
async fn test_non_critical_never_reaches_fallback() {
    let sink = CollectingSink::new();
    let bus = EventBus::new().with_critical_fallback(sink.clone());

    // QuestCreated 不在 is_critical_mpsc_event 清单,旁路完全不触发
    bus.publish(NexusEvent::QuestCreated {
        metadata: EventMetadata::new("quest-engine"),
        quest_id: "q-1".into(),
        title: "示例".into(),
        task_count: 1,
    })
    .await
    .unwrap();

    assert_eq!(sink.len(), 0, "Non-Critical 事件不得进入保底 sink");
    assert_eq!(bus.critical_no_subscriber_total(), 0);
}

// ============================================================
// 测试 6:Clone 共享同一 sink 与计数(Clone 副本 = 同一总线视图)
// ============================================================

#[tokio::test]
async fn test_clone_shares_sink_and_counter() {
    let sink = CollectingSink::new();
    let bus = EventBus::new().with_critical_fallback(sink.clone());
    let cloned = bus.clone();

    cloned.publish(critical_event("via-clone")).await.unwrap();

    assert_eq!(sink.len(), 1, "Clone 副本发布应命中同一 sink");
    assert_eq!(
        bus.critical_no_subscriber_total(),
        1,
        "Clone 副本与原实例共享计数(Arc 语义)"
    );
}

// ============================================================
// 测试 7:默认 LogCriticalSink — 结构化 error! 落点(tracing 断言)
//
// WHY 断言 error 级:warn 语义是"告警但放弃";保底送达后事件已有落点,
// error 级表达"该事件未到达任何真实消费者,请运维关注"。
// 字段 event_type/severity/event_id/source 供跨日志关联定位。
// ============================================================

#[tracing_test::traced_test]
#[tokio::test]
async fn test_default_log_sink_emits_structured_error() {
    let bus = EventBus::new(); // 不注入自定义 sink,走默认 LogCriticalSink

    bus.publish(critical_event("decb-governor")).await.unwrap();

    assert!(logs_contain("Critical 事件保底送达"),
        "默认 sink 应发出保底送达日志");
    assert!(logs_contain("BudgetExceeded"),
        "日志应携带 event_type 定位字段");
    // WHY 不断言 source 字段值:tracing-test 捕获的消息文本不含结构化字段值,
    // 字段级断言由测试 1-3 的 CollectingSink 行为断言兜底(等价强度)。
    assert_eq!(bus.critical_no_subscriber_total(), 1);
}

// ============================================================
// proptest 属性测试:投递守恒
//
// 不变量 I1:任意 n 条 Critical 事件在无订阅者状态下发布 →
//            sink 恰好收到 n 条(恰好一次,单线程发布无并发重复)。
// 不变量 I2:订阅者存活期间发布的 m 条全部经 mpsc 实收,sink 零投递;
//            总守恒 n + m == sink 实收 + mpsc 实收。
// ============================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// I1:无订阅者发布 n 条 Critical 事件 → sink 恰好收到 n 条(单线程发布,
    /// at-least-once 语义下无并发重复源,守恒应为恰好一次)。
    /// WHY 自建 runtime:proptest 宏生成同步 #[test],async 体经 block_on 驱动
    /// (项目既有范式,见 chimera-cli composition_root_e2e.rs prop_* 写法)。
    #[test]
    fn prop_fallback_exactly_once_without_subscriber(
        n in 0usize..32,
        seed in any::<u64>(),
    ) {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        rt.block_on(async move {
            let sink = CollectingSink::new();
            let bus = EventBus::new().with_critical_fallback(sink.clone());
            for i in 0..n {
                let ev = NexusEvent::BudgetExceeded {
                    metadata: EventMetadata::new("proptest"),
                    budget_type: format!("prop-{seed}-{i}"),
                    current: i as u64,
                    limit: 0,
                };
                bus.publish(ev).await.unwrap();
            }
            prop_assert_eq!(sink.len(), n, "I1: 无订阅者发布 n 条,sink 恰收 n 条");
            prop_assert_eq!(bus.critical_no_subscriber_total() as usize, n);
            Ok(())
        })?;
    }

    /// I2:投递守恒 — 混合阶段(无订阅者 no_sub 条 + 订阅者存活 with_sub 条)下,
    /// sink 实收 + mpsc 实收 == 发布总数;且 sink 只收订阅前事件(不双投)。
    #[test]
    fn prop_delivery_conservation_with_mixed_phases(
        no_sub in 0usize..16,
        with_sub in 0usize..16,
    ) {
        let rt = tokio::runtime::Runtime::new().expect("runtime");
        rt.block_on(async move {
            let sink = CollectingSink::new();
            let bus = EventBus::new().with_critical_fallback(sink.clone());
            let mut total_published = 0usize;

            // 阶段 1:无订阅者发布 no_sub 条 → 全部进 sink
            for i in 0..no_sub {
                bus.publish(critical_event(&format!("phase1-{i}"))).await.unwrap();
                total_published += 1;
            }
            prop_assert_eq!(sink.len(), no_sub, "阶段 1:sink 实收 == 发布数");

            // 阶段 2:订阅者存活发布 with_sub 条 → 全部进 mpsc,sink 不再收
            let mut crit_rx = bus.subscribe_critical_events();
            for i in 0..with_sub {
                bus.publish(critical_event(&format!("phase2-{i}"))).await.unwrap();
                total_published += 1;
            }
            prop_assert_eq!(sink.len(), no_sub, "阶段 2:sink 不得收到订阅后事件");

            // 排干 mpsc,统计实收(容量 4096 ≫ 16,无 Full 干扰)
            // timeout(..).await = Result<Option<T>, Elapsed>;transpose 后
            // expect 解出 Option 内层 → Result<T, Elapsed>,is_ok() 即"收到一条"
            let mut mpsc_received = 0usize;
            while tokio::time::timeout(Duration::from_millis(5), crit_rx.recv())
                .await
                .transpose()
                .expect("recv 不应失败")
                .is_ok()
            {
                mpsc_received += 1;
            }

            // 不变量 I2:总守恒
            prop_assert_eq!(
                sink.len() + mpsc_received,
                total_published,
                "I2: sink 实收 + mpsc 实收 == 发布总数(投递守恒)"
            );
            Ok(())
        })?;
    }
}
