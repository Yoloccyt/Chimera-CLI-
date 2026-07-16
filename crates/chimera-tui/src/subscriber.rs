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
            handle: Some(handle),
            shutdown_tx: Some(shutdown_tx),
        }
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
