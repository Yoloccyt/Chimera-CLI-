//! 发布路径等价性契约(ADR-314 W6 / BC-01)
//!
//! # 契约(被本文件固化为可执行断言)
//!
//! `EventBus` 的三条**自动路由**发布路径 ——
//! [`publish`](../event_bus/struct.EventBus.html#method.publish)(async)/
//! `publish_blocking`(sync)/ `publish_batch`(批量)——
//! 对**同一事件序列**必须产生**逐条相同**的订阅者可见序列。
//!
//! # WHY 需要它(把「文档说了」变成「CI 断言」)
//!
//! 历史上 9 条近重复发布路径并存,投递语义(走 broadcast 还是 Critical mpsc、
//! 是否分片、是否降采样、是否计入入口计数)散布在多个方法里。W6 收敛为三族:
//!
//! | 族 | 方法 | 路由规则 |
//! |---|---|---|
//! | 自动路由 | `publish` / `publish_blocking` / `publish_batch` | 按 `is_critical_mpsc_event` 清单自动决定是否走 mpsc 旁路 |
//! | 显式 Critical | `publish_critical` / `publish_critical_blocking` | **无条件**走 mpsc 旁路(清单外的 Critical 事件用) |
//! | 废弃中 | `publish_batch_blocking` / `publish_claim` / `publish_declaim` | `#[deprecated]`,版本窗后物理删除 |
//!
//! 三条自动路由路径**必须**对订阅者等价:任一处改了而另两处没改,订阅者看到的
//! 序列就会分叉(问题排查时最耗时的一类「行为不一致」)。
//!
//! # 等价性的适用范围(诚实边界)
//!
//! 断言在**默认背压策略**(`BackpressurePolicy::LagThreshold`)下成立 ——
//! 这是全仓唯一的生产配置(`EventBus::with_policy` 的降采样策略仅在
//! event-bus 自身测试中使用,生产调用方为 0,2026-11-16 实测)。
//!
//! 降采样策略下 `publish_batch` **刻意**摊销采样开销(`receiver_count()` 与
//! `sender.len()` 各采一次而非逐条),故其 latch 演进与逐条采样不可能逐位相同;
//! 本文件不假装二者等价,而是另行锁定「批量路径**不得绕过**降采样闸门」
//! (见 `batch_path_must_not_bypass_downsample_gate`)——
//! 批量路径此前**完全跳过**该闸门,是对齐前修复的真实缺陷。
//!
//! # 负例设计(证明断言有牙)
//!
//! `negative_control_*` 两个用例**主动喂入分叉序列**并断言等价判据必须判为
//! 不等。若有人把等价判据实现成恒真(最恶劣的误实现:测试全绿但什么都没测),
//! 负控必红。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code

use std::time::Duration;

use event_bus::{BackpressurePolicy, EventBus, EventMetadata, NexusEvent};
use tokio::time::timeout;

/// 构造带唯一 quest_id 的进度事件(Normal 车道,不进 mpsc 旁路)
fn progress_event(i: usize) -> NexusEvent {
    NexusEvent::QuestProgressUpdated {
        metadata: EventMetadata::new("publish-equivalence"),
        quest_id: format!("q-{i}"),
        completed: i as u32,
        total: 100,
    }
}

/// 构造 Critical 事件(BudgetExceeded —— 在 `is_critical_mpsc_event` 清单内,
/// 故自动路由路径必须同时投递到 broadcast 与 mpsc 旁路)
fn critical_event(i: usize) -> NexusEvent {
    NexusEvent::BudgetExceeded {
        metadata: EventMetadata::new("publish-equivalence"),
        budget_type: format!("bt-{i}"),
        current: 9_999,
        limit: 5_000,
    }
}

/// 构造混合序列(Normal 与 Critical 交替,覆盖两条投递分支)
fn mixed_sequence(n: usize) -> Vec<NexusEvent> {
    (0..n)
        .map(|i| {
            if i % 2 == 0 {
                progress_event(i)
            } else {
                critical_event(i)
            }
        })
        .collect()
}

/// 从订阅者排干至多 `want` 条事件(超时即视为已排干)
async fn drain(rx: &mut event_bus::EventReceiver, want: usize) -> Vec<NexusEvent> {
    let mut out = Vec::with_capacity(want);
    while out.len() < want {
        match timeout(Duration::from_millis(200), rx.recv()).await {
            Ok(Ok(ev)) => out.push(ev),
            _ => break,
        }
    }
    out
}

/// 等价判据(可复用实现)
///
/// WHY 抽成纯函数:负控测试需要**主动**喂入分叉序列,验证判据有牙。
/// 若判据恒真(最恶劣的误实现),主断言会「全绿但什么都没测」,负控则必红。
fn sequences_equivalent(a: &[NexusEvent], b: &[NexusEvent]) -> bool {
    a.len() == b.len() && a.iter().zip(b.iter()).all(|(x, y)| x == y)
}

/// 主契约:三条自动路由路径产生**逐条相同**的订阅者可见序列
#[tokio::test]
async fn three_auto_routed_paths_produce_identical_sequences() {
    let n = 12;
    let events = mixed_sequence(n);

    // 路径 A:publish(async 逐条)
    let bus_async = EventBus::new();
    let mut rx_async = bus_async.subscribe();
    for ev in &events {
        bus_async.publish(ev.clone()).await.unwrap();
    }
    let seq_async = drain(&mut rx_async, n).await;

    // 路径 B:publish_blocking(sync 逐条)
    let bus_sync = EventBus::new();
    let mut rx_sync = bus_sync.subscribe();
    for ev in &events {
        bus_sync.publish_blocking(ev.clone()).unwrap();
    }
    let seq_sync = drain(&mut rx_sync, n).await;

    // 路径 C:publish_batch(批量)
    let bus_batch = EventBus::new();
    let mut rx_batch = bus_batch.subscribe();
    bus_batch.publish_batch(events.clone()).await.unwrap();
    let seq_batch = drain(&mut rx_batch, n).await;

    assert_eq!(seq_async.len(), n, "publish 路径必须送达全部 {n} 条");
    assert_eq!(seq_sync.len(), n, "publish_blocking 路径必须送达全部 {n} 条");
    assert_eq!(seq_batch.len(), n, "publish_batch 路径必须送达全部 {n} 条");

    assert!(
        sequences_equivalent(&seq_async, &seq_sync),
        "publish 与 publish_blocking 的订阅者可见序列必须相同"
    );
    assert!(
        sequences_equivalent(&seq_async, &seq_batch),
        "publish 与 publish_batch 的订阅者可见序列必须相同"
    );
    assert!(
        sequences_equivalent(&seq_sync, &seq_batch),
        "publish_blocking 与 publish_batch 的订阅者可见序列必须相同"
    );
}

/// 负控 1:分叉序列(仅一条不同)必须被判为不等
#[tokio::test]
async fn negative_control_divergent_sequence_is_detected() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let events = mixed_sequence(4);
    for ev in &events {
        bus.publish(ev.clone()).await.unwrap();
    }
    let got = drain(&mut rx, events.len()).await;
    assert_eq!(got.len(), events.len(), "前置:订阅者应收齐全部事件");

    // 刻意构造分叉序列:只把第 1 条换成别的 quest_id
    let mut diverged = got.clone();
    diverged[1] = progress_event(999);

    assert!(
        !sequences_equivalent(&got, &diverged),
        "负控失败:等价判据把「仅一条不同」的分叉序列判成相等 ⇒ 等价性断言形同虚设"
    );

    // 长度不同也必须判为不等
    assert!(
        !sequences_equivalent(&got, &got[..1]),
        "负控失败:长度不同的序列被判成相等"
    );
}

/// 负控 2:顺序被改写(模拟「批量路径走了不同投递路径」的可观测后果)必须判为不等
#[tokio::test]
async fn negative_control_reordered_sequence_is_detected() {
    let events = mixed_sequence(4);
    let mut reordered = events.clone();
    reordered.reverse();

    assert!(
        !sequences_equivalent(&events, &reordered),
        "负控失败:顺序被改写的序列被判成相等 ⇒ 判据无法识别投递路径分叉"
    );
}

/// 批量路径的**入口计数**必须与单发路径一致
///
/// WHY:ADR-191 §16.5 把 `published_total` 定义为「发布入口计数」,是 L1 吞吐量
/// 指标的唯一输入;`publish_batch` 同样是发布入口,漏计会让吞吐量**低报**
/// (唯一生产批量发布方是 parliament 的辩论批量投递)。
#[tokio::test]
async fn batch_path_increments_entry_counters_like_single_path() {
    let n = 8;
    let events = mixed_sequence(n); // 奇数位为 Critical ⇒ critical_total 应为 n/2

    let bus_batch = EventBus::new();
    bus_batch.publish_batch(events.clone()).await.unwrap();

    let bus_single = EventBus::new();
    for ev in &events {
        bus_single.publish(ev.clone()).await.unwrap();
    }

    assert_eq!(
        bus_batch.published_total(),
        bus_single.published_total(),
        "批量路径的 published_total 必须与逐条发布一致(入口计数语义)"
    );
    assert_eq!(bus_batch.published_total(), n as u64);
    assert_eq!(
        bus_batch.shadow_stats().critical_total,
        bus_single.shadow_stats().critical_total,
        "批量路径的 critical_total 必须与逐条发布一致(Critical 车道前哨计数)"
    );
    assert_eq!(bus_batch.shadow_stats().critical_total, (n / 2) as u64);
}

/// 批量路径**不得绕过**降采样闸门
///
/// WHY:对齐前 `publish_batch` 完全跳过 `ds_admit` ⇒ 压力区内批量发布会
/// 绕过「每 every 条放行 1 条」策略,且降级量不被计入 `downsampled_total`
/// (运维看不见降级量)。本例锁死「批量路径与单发路径同样受闸门约束」。
///
/// 注:本例**不断言**批量与逐条路径的放行条数逐位相同 —— 批量路径刻意摊销
/// `sender.len()` 采样(每批一次而非逐条),压力区边界的放行位置本就会不同;
/// 这里锁定的是**闸门必须生效**(降级量 > 0),而非精确的放行位置。
#[tokio::test]
async fn batch_path_must_not_bypass_downsample_gate() {
    let policy = BackpressurePolicy::DownsampleUnderPressure {
        broadcast_capacity: 64,
        every: 8,
        trigger_permille: 500, // 50% 即触发(无消费者时快速入压力区)
        resume_permille: 100,  // 10% 解除
    };

    let bus = EventBus::with_policy(policy);
    let _rx = bus.subscribe(); // 故意不消费:占用持续走高 → 进入压力区
    let events: Vec<NexusEvent> = (0..160).map(progress_event).collect();
    bus.publish_batch(events).await.unwrap();

    assert!(
        bus.pressure_engaged(),
        "无消费持续批量发布必须进入压力区"
    );
    assert!(
        bus.downsampled_total() > 0,
        "批量路径必须经降采样闸门(压力区内应产生降级量);\
         downsampled_total == 0 说明批量路径绕过了闸门"
    );
    // 降级不等于丢弃:published_total 仍须等于发布总数(入口计数在闸门之前)
    assert_eq!(
        bus.published_total(),
        160,
        "降采样只影响广播放行,不影响入口计数"
    );
}
