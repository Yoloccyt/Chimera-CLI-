//! Event-Bus 订阅者 — 为 TUI 数据层缓冲 `NexusEvent`
//!
//! 对应 Task P1.1: 先同步调用 `EventBus::subscribe()`, 再 `tokio::spawn()`
//! 后台转发任务,遵循 §4.4 反模式 #3。
//!
//! # 设计要点
//! - 使用有界环形缓冲区(容量 1024),溢出时丢弃最旧事件,避免内存无限增长。
//! - `try_recv` 非阻塞消费,供 TUI 主循环在 tick 时批量取出。
//! - 后台任务遇到 broadcast `Lagged` 错误时记录 warn 并继续,不 panic。

use event_bus::{EventBus, EventBusError, NexusEvent};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tracing::warn;

/// 事件缓冲区容量
///
/// WHY 1024:与 `EventBus` 默认广播容量一致,可吸收一次完整广播窗口的
/// 突发事件;按每条 NexusEvent 约 500 字节估算,约 0.5MB。
const BUFFER_CAPACITY: usize = 1024;

/// TUI 事件订阅者
///
/// 在后台将 `EventBus` 的广播事件转发到本地有界缓冲区,供 `DataPipeline`
/// (Task P1.3) 非阻塞消费。
#[derive(Debug)]
pub struct EventSubscriber {
    /// 共享事件缓冲区
    ///
    /// WHY `std::sync::Mutex` + `VecDeque`: `try_recv` 是同步非阻塞方法,
    /// 不需要 async lock;锁内仅做 push/pop,持有时间极短,不跨 await。
    buffer: Arc<Mutex<VecDeque<NexusEvent>>>,
    /// 事件唤醒原语(感知延迟优化,§7.7 设计,2026-09-13)
    ///
    /// 转发任务每推入一条事件即 `notify_one()`;消费方(DataPipeline)以
    /// `tokio::select!{ sleep, notified }` 实现事件驱动的即时 tick——
    /// 此前 pipeline 只能等固定 tick 间隔,「事件 → 快照」最坏 ~250ms。
    ///
    /// WHY `Arc<Notify>` 而非直接持有:DataPipeline 在 guard 作用域**外**
    /// await(`std::sync::MutexGuard` 跨 await 是 §4.4 反模式 #1),需要
    /// 无锁借用的克隆句柄。
    notify: Arc<tokio::sync::Notify>,
    /// 后台转发任务句柄
    ///
    /// WHY `Option`: `shutdown(&mut self)` 需要取出手柄并 await,
    /// 同时保证重复调用为 no-op。
    handle: Option<JoinHandle<()>>,
    /// 关闭信号发送端
    ///
    /// WHY `watch` 而非 `oneshot`: 后台任务每次循环都需在 `rx.recv()` 与
    /// 关闭信号间做 `tokio::select!`; `watch::Receiver` 的 `changed()` 可重复
    /// 等待,语义上等同于“运行开关”。
    shutdown_tx: Option<watch::Sender<bool>>,
}

impl EventSubscriber {
    /// 创建订阅者并启动后台转发任务
    ///
    /// # 调用时机(§4.4 反模式 #3)
    /// `bus.subscribe()` 必须在 `tokio::spawn()` 之前同步调用,
    /// 否则可能错过早期发布的事件。
    pub fn new(bus: EventBus) -> Self {
        // 同步订阅,确保在 spawn 之前完成;否则 spawn 内可能错过
        // 调用方已经发布的事件。
        let mut rx = bus.subscribe();

        let buffer = Arc::new(Mutex::new(VecDeque::with_capacity(BUFFER_CAPACITY)));
        let buffer_clone = Arc::clone(&buffer);
        let notify = Arc::new(tokio::sync::Notify::new());
        let notify_for_task = Arc::clone(&notify);

        let (shutdown_tx, mut shutdown_rx) = watch::channel(false);

        let handle = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = shutdown_rx.changed() => break,
                    event = rx.recv() => {
                        match event {
                            Ok(event) => {
                                // P7: 记录特定事件接收日志(debug 级别,仅对新事件变体输出)
                                EventSubscriber::handle_event(&event);
                                let mut buf = buffer_clone.lock().unwrap_or_else(|poisoned| {
                                    tracing::warn!("TUI event subscriber buffer mutex was poisoned; recovering state");
                                    poisoned.into_inner()
                                });
                                if buf.len() >= BUFFER_CAPACITY {
                                    // 缓冲区满时丢弃最旧事件,保持最新 1024 条,
                                    // 避免 TUI 消费慢导致内存无限增长。
                                    buf.pop_front();
                                    warn!(
                                        capacity = BUFFER_CAPACITY,
                                        "TUI event buffer full, dropped oldest event"
                                    );
                                }
                                buf.push_back(event);
                                // 事件唤醒(感知延迟优化):通知消费方"有新事件",
                                // 使其 select! 的 notified 分支立即返回(无需等 tick)。
                                // Notify 许可语义:无等待者时存 1 个许可,下次
                                // notified().await 立即完成——连续事件不堆积唤醒。
                                notify_for_task.notify_one();
                            }
                            Err(EventBusError::ChannelClosed) => break,
                            Err(EventBusError::SlowConsumerDropped { lag, .. }) => {
                                // broadcast 层因消费慢已丢弃 lag 条事件;本订阅者继续
                                // 接收新事件,避免单次 lag 导致后续事件全部丢失。
                                warn!(
                                    lag,
                                    "TUI event subscriber lagged, events dropped by broadcast channel"
                                );
                            }
                            Err(_) => {
                                // 其他错误(如 RecvTimeout 不会从 recv() 返回)保守处理:
                                // 记录并继续,不中断循环。
                                warn!("TUI event subscriber encountered an unexpected error, continuing");
                            }
                        }
                    }
                }
            }
        });

        Self {
            buffer,
            notify,
            handle: Some(handle),
            shutdown_tx: Some(shutdown_tx),
        }
    }

    /// 事件唤醒句柄 — 消费方(DataPipeline)用于 `select!{ sleep, notified }`
    ///
    /// # 语义
    /// 每当转发任务推入事件,许可即被设置;等待中的 `notified()` 立即完成。
    /// 无等待者时许可驻留(下次等待立即返回)——「至少一条事件待消费」的
    /// 电平语义,不丢唤醒。
    pub fn notify_handle(&self) -> Arc<tokio::sync::Notify> {
        Arc::clone(&self.notify)
    }

    /// 处理单个 NexusEvent,记录特定事件变体的接收日志
    ///
    /// WHY 关联函数: 后台转发任务只持有 buffer 的 `Arc<Mutex<...>>`,
    /// 不持有 `EventSubscriber` 的 `&self`,因此使用关联函数避免生命周期约束。
    ///
    /// P7 新增: 为 `OmniSparseMasksComputed` 和 `ClvSnapshotReported` 事件
    /// 添加 debug 级别日志,便于调试 OSA/CLV 面板数据流。
    /// 其他事件变体走通配符分支,不记录日志(避免高频事件刷屏)。
    ///
    /// WHY 不在此调用 OsaSync/ClvSync: EventSubscriber 的职责仅是缓冲事件,
    /// 事件→状态的转换由 `DataPipeline`(data.rs)调用各同步器 `apply_event` 完成。
    /// 此处日志仅用于验证事件已进入订阅管道,不承担状态更新职责。
    fn handle_event(event: &NexusEvent) {
        match event {
            NexusEvent::OmniSparseMasksComputed {
                sparsity,
                context_mask,
                ..
            } => {
                tracing::debug!(
                    sparsity,
                    context_mask_count = context_mask.len(),
                    "OmniSparseMasksComputed event received"
                );
            }
            NexusEvent::ClvSnapshotReported { clv_summary, .. } => {
                tracing::debug!(
                    l2_norm = clv_summary.l2_norm,
                    block_means_count = clv_summary.block_means.len(),
                    "ClvSnapshotReported event received"
                );
            }
            _ => {}
        }
    }

    /// 非阻塞地从缓冲区取出一条事件
    ///
    /// 返回 `None` 表示当前缓冲区为空。
    pub fn try_recv(&mut self) -> Option<NexusEvent> {
        let mut buf = self.buffer.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("TUI event subscriber buffer mutex was poisoned; recovering state");
            poisoned.into_inner()
        });
        buf.pop_front()
    }

    /// 一次锁取走缓冲区里的全部事件（FIFO 序）；空缓冲区返回空 `Vec`
    ///
    /// WHY 与 [`try_recv`](Self::try_recv) 并存而不替换它：`try_recv` 是"一条一锁"
    /// 语义，逐条消费的调用方需要它；而 `DataPipeline` 每个 tick 都要清空缓冲区，
    /// 用 `while try_recv()` 循环等于 N 次 lock/unlock（N = 本 tick 到达数，突发时数十），
    /// 且每次放锁都给转发任务插队 push 的机会，"同一 tick 的批次边界"因此不确定。
    /// 本方法把批次压成一次临界区：`std::mem::take` 后原样交出，顺序与入队严格一致。
    ///
    /// WHY 返回 `Vec` 而非 `Option`：无事件不是错误，调用方不必为此多写一层分支。
    pub fn drain_all(&mut self) -> Vec<NexusEvent> {
        let mut buf = self.buffer.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("TUI event subscriber buffer mutex was poisoned; recovering state");
            poisoned.into_inner()
        });
        // take 后缓冲区回到空 deque；迭代顺序 = VecDeque 的前到后 = FIFO
        std::mem::take(&mut *buf).into_iter().collect()
    }

    /// 关闭订阅者,中止并等待后台任务结束
    ///
    /// 幂等:重复调用会立即返回,不会 panic。
    /// WHY 先 signal 再 abort: 让转发任务通过 `tokio::select!` 主动退出,
    /// 避免 drop `JoinHandle` 后任务成为 orphan(§4.4 反模式 #7)。
    pub async fn shutdown(&mut self) {
        if self.handle.is_none() {
            return;
        }

        // 发送关闭信号;若接收端已 drop(任务已自然退出),忽略错误。
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(true);
        }

        if let Some(handle) = self.handle.take() {
            handle.abort();
            // abort 后 await 可捕获 JoinError;忽略是因为任务已被显式取消。
            let _ = handle.await;
        }
    }
}

impl Drop for EventSubscriber {
    fn drop(&mut self) {
        // 若未显式 shutdown,至少 abort 转发任务,避免 orphan task。
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use event_bus::{ClvSummary, EventMetadata, NexusEvent};
    use tokio::time::{sleep, Duration};

    /// 构造 OmniSparseMasksComputed 事件
    fn osa_event(sparsity: f32, context_mask: Vec<&str>) -> NexusEvent {
        NexusEvent::OmniSparseMasksComputed {
            metadata: EventMetadata::new("osa-coordinator"),
            mask_hash: format!("mask-{sparsity}"),
            sparsity,
            context_mask: context_mask.into_iter().map(String::from).collect(),
        }
    }

    /// 构造 ClvSnapshotReported 事件
    fn clv_event(l2_norm: f32, block_count: usize) -> NexusEvent {
        NexusEvent::ClvSnapshotReported {
            metadata: EventMetadata::new("nmc-encoder"),
            modality: "Text".into(),
            content_hash: format!("hash-{l2_norm}"),
            clv_summary: ClvSummary {
                block_means: vec![0.1; block_count],
                l2_norm,
                top_dims: vec![(0, 0.8)],
            },
        }
    }

    /// 等待并取出订阅者缓冲区中的事件(带重试,避免时序依赖)
    ///
    /// WHY 重试: 后台转发任务与测试主循环并发执行,事件从 broadcast receiver
    /// 到 buffer 的转发存在微秒级延迟。固定 sleep 容易 flaky,轮询重试更稳健。
    async fn recv_with_retry(subscriber: &mut EventSubscriber) -> Option<NexusEvent> {
        for _ in 0..20 {
            if let Some(event) = subscriber.try_recv() {
                return Some(event);
            }
            sleep(Duration::from_millis(5)).await;
        }
        subscriber.try_recv()
    }

    /// 累计 drain 直到取满 `expect` 条（转发任务是异步 push,不能假设一次取全）
    ///
    /// WHY 累计而非单次断言：`drain_all` 的批次边界取决于转发进度,但 **FIFO 顺序
    /// 与总数是确定的** —— 把不确定的"何时到齐"挡在 helper 里,断言只谈确定性。
    async fn drain_until(sub: &mut EventSubscriber, expect: usize) -> Vec<NexusEvent> {
        let mut acc: Vec<NexusEvent> = Vec::new();
        for _ in 0..40 {
            acc.extend(sub.drain_all());
            if acc.len() >= expect {
                return acc;
            }
            sleep(Duration::from_millis(5)).await;
        }
        acc
    }

    /// `drain_all` 一次取走整批,且取后缓冲区为空
    #[tokio::test]
    async fn drain_all_takes_whole_batch_and_leaves_buffer_empty() {
        let bus = EventBus::new();
        let mut sub = EventSubscriber::new(bus.clone());
        for i in 0..5u32 {
            bus.publish_blocking(osa_event(0.1 * (i as f32), vec!["a.rs"]))
                .expect("publish");
        }

        let batch = drain_until(&mut sub, 5).await;
        assert_eq!(batch.len(), 5, "5 条发布必须一次或累计取满 5 条");
        let sparsities: Vec<f32> = batch
            .iter()
            .map(|e| match e {
                NexusEvent::OmniSparseMasksComputed { sparsity, .. } => *sparsity,
                other => panic!("unexpected event: {other:?}"),
            })
            .collect();
        assert_eq!(
            sparsities,
            vec![0.0, 0.1, 0.2, 0.3, 0.4],
            "drain_all 必须保持入队顺序"
        );
        assert!(sub.drain_all().is_empty(), "取走后缓冲区必须为空");

        sub.shutdown().await;
    }

    /// 批次语义等价：`drain_all` 与逐条 `try_recv` 必须给出同一 FIFO 序列
    ///
    /// WHY 这条是"无行为回归"的证据：pipeline 从 try_recv 循环切到 drain_all,
    /// 面板看到的事件顺序不得变。
    #[tokio::test]
    async fn drain_all_matches_try_recv_sequence() {
        let names = ["x.rs", "y.rs", "z.rs"];

        let bus_a = EventBus::new();
        let mut sub_a = EventSubscriber::new(bus_a.clone());
        for name in &names {
            bus_a
                .publish_blocking(osa_event(0.5, vec![*name]))
                .expect("publish");
        }
        let via_drain = drain_until(&mut sub_a, names.len()).await;

        let bus_b = EventBus::new();
        let mut sub_b = EventSubscriber::new(bus_b.clone());
        for name in &names {
            bus_b
                .publish_blocking(osa_event(0.5, vec![*name]))
                .expect("publish");
        }
        let mut via_recv: Vec<NexusEvent> = Vec::new();
        while via_recv.len() < names.len() {
            match recv_with_retry(&mut sub_b).await {
                Some(event) => via_recv.push(event),
                None => break,
            }
        }

        let mask_of = |events: &[NexusEvent]| -> Vec<Vec<String>> {
            events
                .iter()
                .map(|e| match e {
                    NexusEvent::OmniSparseMasksComputed { context_mask, .. } => {
                        context_mask.clone()
                    }
                    other => panic!("unexpected event: {other:?}"),
                })
                .collect()
        };
        assert_eq!(
            mask_of(&via_drain),
            mask_of(&via_recv),
            "两种取法必须给出同一序列"
        );

        sub_a.shutdown().await;
        sub_b.shutdown().await;
    }

    /// 空缓冲区：返回空 Vec 而非 panic / 假事件
    #[tokio::test]
    async fn drain_all_on_empty_buffer_returns_empty() {
        let bus = EventBus::new();
        let mut sub = EventSubscriber::new(bus.clone());
        assert!(sub.drain_all().is_empty());
        sub.shutdown().await;
    }

    /// Notify 许可语义(感知延迟优化,2026-09-13):事件入缓冲后,唤醒句柄的
    /// `notified()` **立即完成**(不等任何 tick 间隔)
    ///
    /// 验证链路: publish_blocking → 转发任务 push_back + notify_one →
    /// 消费方 `notify_handle().notified().await` 即时返回。许可语义:
    /// 无等待者时许可驻留(下次等待立即返回)——「至少一条事件待消费」
    /// 的电平语义,不丢唤醒。
    #[tokio::test]
    async fn notify_handle_fires_on_event_arrival() {
        let bus = EventBus::new();
        let mut subscriber = EventSubscriber::new(bus.clone());
        let notify = subscriber.notify_handle();

        // 先挂起等待(成为等待者),再发布事件 → notified() 应在远小于
        // 任何 tick 间隔的时间内完成
        let waiter = tokio::spawn(async move { notify.notified().await });
        // 让 waiter 先进入等待态(让出调度)
        tokio::time::sleep(Duration::from_millis(20)).await;

        bus.publish_blocking(osa_event(0.5, vec!["a.rs"]))
            .expect("publish should succeed");

        tokio::select! {
            _ = waiter => {} // 唤醒成功
            _ = sleep(Duration::from_millis(500)) => {
                panic!("事件发布后 500ms 内未收到唤醒——Notify 原语失效");
            }
        }
        subscriber.shutdown().await;
    }

    /// 验证 handle_event 对 OmniSparseMasksComputed 不 panic
    #[test]
    fn test_handle_event_omni_sparse_masks_computed() {
        let event = osa_event(0.45, vec!["file1.rs", "file2.rs"]);
        EventSubscriber::handle_event(&event);
    }

    /// 验证 handle_event 对 ClvSnapshotReported 不 panic
    #[test]
    fn test_handle_event_clv_snapshot_reported() {
        let event = clv_event(2.5, 8);
        EventSubscriber::handle_event(&event);
    }

    /// 验证 handle_event 对其他事件变体不 panic(走通配符分支)
    #[test]
    fn test_handle_event_other_variants() {
        let event = NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k1".into(),
        };
        EventSubscriber::handle_event(&event);
    }

    /// 端到端验证: OmniSparseMasksComputed 事件经 EventSubscriber 缓冲后可被取出
    ///
    /// 验证链路: EventBus.publish_blocking → broadcast → 后台任务 handle_event
    /// → buffer.push_back → try_recv 取出。
    #[tokio::test]
    async fn test_subscriber_buffers_omni_sparse_masks_computed() {
        let bus = EventBus::new();
        let mut subscriber = EventSubscriber::new(bus.clone());

        bus.publish_blocking(osa_event(0.45, vec!["file1.rs", "file2.rs"]))
            .expect("publish should succeed");

        let received = recv_with_retry(&mut subscriber).await;
        subscriber.shutdown().await;

        assert!(
            received.is_some(),
            "OmniSparseMasksComputed 事件应被 EventSubscriber 缓冲"
        );
        match received.unwrap() {
            NexusEvent::OmniSparseMasksComputed {
                sparsity,
                context_mask,
                ..
            } => {
                assert!((sparsity - 0.45).abs() < 1e-5);
                assert_eq!(context_mask.len(), 2);
                assert_eq!(context_mask[0], "file1.rs");
            }
            other => panic!("期望 OmniSparseMasksComputed, 实际收到 {:?}", other),
        }
    }

    /// 端到端验证: ClvSnapshotReported 事件经 EventSubscriber 缓冲后可被取出
    ///
    /// 验证链路: EventBus.publish_blocking → broadcast → 后台任务 handle_event
    /// → buffer.push_back → try_recv 取出。
    #[tokio::test]
    async fn test_subscriber_buffers_clv_snapshot_reported() {
        let bus = EventBus::new();
        let mut subscriber = EventSubscriber::new(bus.clone());

        bus.publish_blocking(clv_event(2.5, 8))
            .expect("publish should succeed");

        let received = recv_with_retry(&mut subscriber).await;
        subscriber.shutdown().await;

        assert!(
            received.is_some(),
            "ClvSnapshotReported 事件应被 EventSubscriber 缓冲"
        );
        match received.unwrap() {
            NexusEvent::ClvSnapshotReported { clv_summary, .. } => {
                assert_eq!(clv_summary.block_means.len(), 8);
                assert!((clv_summary.l2_norm - 2.5).abs() < 1e-5);
            }
            other => panic!("期望 ClvSnapshotReported, 实际收到 {:?}", other),
        }
    }
}
