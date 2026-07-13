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
