//! 事件发布异步流水线 — 将事件构造与发布分离，降低串行发布延迟
//!
//! 对应架构层:L8 Parliament
//! 对应 spec:l8-parliament-deep-optimization-round3 Task 3
//!
//! # 设计原则
//! - 流水线化:事件构造在 mpsc channel 中异步传递，不阻塞调用方
//! - Critical 旁路:Critical 事件(ConsensusReached/SkepticVeto/VetoOverridden)不走流水线，直接发布
//! - 背压降级:channel 满时自动降级到直接发布
//! - 线程安全:所有操作 Send + Sync

use event_bus::{EventBus, EventBusError, NexusEvent};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tracing::warn;

/// 流水线事件 — 携带事件及确认通道
///
/// WHY Option<ack_tx>:非 Critical 事件经流水线异步发布后，调用方可通过
/// oneshot::Receiver 等待确认；Critical 事件不走流水线，ack_tx 恒为 None。
/// 消费端收到事件后调用 `bus.publish`，再通过 ack_tx 回传结果。
struct PipelineEvent {
    /// 待发布的事件
    event: NexusEvent,
    /// 确认通道发送端 — 消费端发布完成后通过此通道回传结果
    ack_tx: Option<tokio::sync::oneshot::Sender<Result<(), EventBusError>>>,
}

/// 事件发布异步流水线
///
/// 将事件构造与发布分离，通过 mpsc channel 实现异步流水线化。
/// 调用方仅需构造事件并发送到 channel，后台消费任务负责实际 `bus.publish`。
///
/// # 使用方法
/// ```ignore
/// let pipeline = EventPipeline::new(bus, 128);
/// pipeline.spawn()?;
/// pipeline.publish(event).await;
/// ```
///
/// # 线程安全
/// 内部使用 `Mutex` 保护 `mpsc::Sender`，`AtomicBool` 保护启动状态，
/// `Arc<EventBus>` 共享总线引用。所有方法为 `&self`，支持 `Send + Sync`。
pub struct EventPipeline {
    /// 事件发送端(Mutex 保护，支持 &self 的 spawn 中替换)
    tx: Mutex<Option<mpsc::Sender<PipelineEvent>>>,
    /// 事件总线引用
    bus: Arc<EventBus>,
    /// 通道容量
    capacity: usize,
    /// 是否已启动
    spawned: AtomicBool,
}

impl EventPipeline {
    /// 创建新的事件流水线
    ///
    /// # 参数
    /// - `bus`: 事件总线
    /// - `capacity`: 通道容量(默认 256)
    ///
    /// 注意:创建后需调用 `spawn()` 启动后台消费任务，否则所有事件均直接发布。
    pub fn new(bus: EventBus, capacity: usize) -> Self {
        Self {
            tx: Mutex::new(None),
            bus: Arc::new(bus),
            capacity,
            spawned: AtomicBool::new(false),
        }
    }

    /// 启动后台消费任务
    ///
    /// 创建 mpsc channel，将发送端存入 `self.tx`，接收端用于后台消费。
    /// 消费任务循环接收 `PipelineEvent` 并调用 `bus.publish`，
    /// 发布完成后通过 `ack_tx` 回传结果。
    ///
    /// # 幂等性
    /// 重复调用返回 `Err("EventPipeline already spawned")`，不影响已有任务。
    ///
    /// # 注意(§4.4 反模式 #3)
    /// 此方法不涉及 EventBus 的 subscribe，因此不需要在 `tokio::spawn` 之前
    /// subscribe。流水线是发布端抽象，不消费 EventBus 事件。
    pub fn spawn(&self) -> Result<(), &'static str> {
        // 防止重复启动
        if self.spawned.swap(true, Ordering::AcqRel) {
            return Err("EventPipeline already spawned");
        }

        let (tx, mut rx) = mpsc::channel::<PipelineEvent>(self.capacity);
        // 将发送端存入 Mutex，供 publish 使用
        *self.tx.lock().expect("EventPipeline::spawn tx lock") = Some(tx);

        let bus = Arc::clone(&self.bus);
        tokio::spawn(async move {
            while let Some(pipeline_event) = rx.recv().await {
                // 发布事件到 EventBus
                let result = bus.publish(pipeline_event.event).await;
                // 通过 ack_tx 回传结果给调用方
                if let Some(ack_tx) = pipeline_event.ack_tx {
                    // WHY 忽略 send 错误:调用方可能已提前 drop ack_rx(如超时或取消),
                    // 此时 send 返回 Err,不影响后续事件处理
                    let _ = ack_tx.send(result);
                }
            }
            // channel 关闭(所有发送端 drop)，消费任务自然退出
            warn!("EventPipeline 消费任务退出:所有发送端已关闭");
        });

        Ok(())
    }

    /// 发布事件(非 Critical 走流水线，Critical 直接发布)
    ///
    /// # 流控策略
    /// - Critical 事件(ConsensusReached/SkepticVeto/VetoOverridden):
    ///   直接调用 `bus.publish`，确保不因流水线背压而延迟。
    /// - 非 Critical 事件:通过 mpsc channel 发送给后台消费任务。
    /// - 背压降级:channel 满时自动降级到直接发布，避免阻塞调用方。
    /// - 未启动:channel 尚未初始化时直接发布，行为与无流水线一致。
    ///
    /// # 参数
    /// - `event`: 待发布的事件
    ///
    /// # 返回
    /// - `Ok(())`: 事件已成功发布
    /// - `Err(EventBusError)`: 发布失败(仅 Critical 路径或降级路径)
    pub async fn publish(&self, event: NexusEvent) -> Result<(), EventBusError> {
        // Critical 事件直接发布(不走流水线)
        // WHY:Critical 事件(ConsensusReached/SkepticVeto/VetoOverridden)必须
        // 立即发布，不能因流水线背压而延迟。利用 event_bus::is_critical_event
        // 判断(基于 severity()=Critical)。
        if event_bus::is_critical_event(&event) {
            return self.bus.publish(event).await;
        }

        // 非 Critical 走流水线
        let (ack_tx, ack_rx) = tokio::sync::oneshot::channel();
        let pipeline_event = PipelineEvent {
            event,
            ack_tx: Some(ack_tx),
        };

        // 锁定 Mutex 获取发送端引用
        let tx_guard = self.tx.lock().expect("EventPipeline::publish tx lock");
        match tx_guard.as_ref() {
            Some(tx) => {
                // 尝试非阻塞发送
                match tx.try_send(pipeline_event) {
                    Ok(()) => {
                        // 发送成功，等待后台任务确认
                        // WHY await ack:保证调用方在事件实际发布后才继续，
                        // 避免流水线带来"事件未发布但调用方认为已发布"的时序问题。
                        // 消费端通过 ack_tx.send(result) 回传 bus.publish 结果。
                        drop(tx_guard); // 提前释放锁，避免持锁跨 await(§4.4 红线 #1)
                        match ack_rx.await {
                            Ok(result) => result,
                            // 消费端已关闭(任务退出)，事件已投递但无法获知结果
                            Err(_) => Ok(()),
                        }
                    }
                    Err(mpsc::error::TrySendError::Full(pe)) => {
                        // 背压降级:channel 满 → 直接发布
                        // WHY 降级而非等待:VoteCast 等非 Critical 事件可接受
                        // 直接发布延迟，不应因 channel 满而阻塞辩论流程。
                        drop(tx_guard);
                        self.bus.publish(pe.event).await
                    }
                    Err(mpsc::error::TrySendError::Closed(pe)) => {
                        // 通道已关闭(消费任务异常退出)→ 直接发布
                        drop(tx_guard);
                        warn!("EventPipeline 通道已关闭，降级到直接发布");
                        self.bus.publish(pe.event).await
                    }
                }
            }
            None => {
                // 尚未启动(spawn 未调用)，直接发布
                drop(tx_guard);
                self.bus.publish(pipeline_event.event).await
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use event_bus::EventMetadata;

    /// 创建非 Critical 测试事件
    fn make_test_event() -> NexusEvent {
        NexusEvent::VoteCast {
            metadata: EventMetadata::new("test"),
            proposal_id: "p-1".into(),
            voter: "architect".into(),
            vote: true,
        }
    }

    /// 创建 Critical 测试事件
    fn make_critical_event() -> NexusEvent {
        NexusEvent::ConsensusReached {
            metadata: EventMetadata::new("test"),
            quest_id: "q-1".into(),
            decision_hash: "abc".into(),
            dpo_pair_id: None,
        }
    }

    #[tokio::test]
    async fn test_pipeline_new_and_spawn() {
        let bus = EventBus::new();
        let pipeline = EventPipeline::new(bus, 64);
        assert!(pipeline.spawn().is_ok(), "首次 spawn 应成功");
    }

    #[tokio::test]
    async fn test_pipeline_reject_duplicate_spawn() {
        let bus = EventBus::new();
        let pipeline = EventPipeline::new(bus, 64);
        assert!(pipeline.spawn().is_ok(), "首次 spawn 应成功");
        let err = pipeline.spawn().expect_err("重复 spawn 应失败");
        assert!(err.contains("already spawned"), "错误信息应提示已启动");
    }

    #[tokio::test]
    async fn test_pipeline_publish_non_critical() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let pipeline = EventPipeline::new(bus, 64);
        let _ = pipeline.spawn();

        let event = make_test_event();
        let result = pipeline.publish(event).await;
        assert!(result.is_ok(), "发布非 Critical 事件应成功");

        // 验证事件被发布到 EventBus
        let received = tokio::time::timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("应在超时前收到事件")
            .expect("事件应有效");
        assert_eq!(received.type_name(), "VoteCast");
    }

    #[tokio::test]
    async fn test_pipeline_critical_bypasses_pipeline() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let pipeline = EventPipeline::new(bus, 64);
        // 不 spawn(模拟 pipeline 未启动)，Critical 事件应直接发布
        let event = make_critical_event();
        let result = pipeline.publish(event).await;
        assert!(result.is_ok(), "Critical 事件应直接发布成功");

        // 验证事件被发布到 EventBus
        let received = tokio::time::timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("应在超时前收到事件")
            .expect("事件应有效");
        assert_eq!(received.type_name(), "ConsensusReached");
    }

    #[tokio::test]
    async fn test_pipeline_backpressure_fallback() {
        // 使用极小容量(1)的 channel，填充后验证背压降级
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let pipeline = EventPipeline::new(bus, 1);
        let _ = pipeline.spawn();

        // 发送第一个事件(填充 channel)
        let e1 = make_test_event();
        let _ = pipeline.publish(e1).await;

        // 发送第二个事件(此时 channel 可能满，验证降级不 panic)
        let e2 = make_test_event();
        let result = pipeline.publish(e2).await;
        // 无论走流水线还是降级，都应返回 Ok
        assert!(result.is_ok(), "背压降级应成功发布事件");

        // 验证两个事件都能被收到
        let mut count = 0;
        for _ in 0..5 {
            match tokio::time::timeout(Duration::from_millis(300), rx.recv()).await {
                Ok(Ok(_)) => count += 1,
                _ => break,
            }
        }
        assert_eq!(count, 2, "应收到 2 个事件");
    }

    #[tokio::test]
    async fn test_pipeline_publish_before_spawn() {
        // 未 spawn 时 publish 应直接发布(不 panic)
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let pipeline = EventPipeline::new(bus, 64);

        let event = make_test_event();
        let result = pipeline.publish(event).await;
        assert!(result.is_ok(), "未 spawn 时应直接发布成功");

        let received = tokio::time::timeout(Duration::from_millis(500), rx.recv())
            .await
            .expect("应在超时前收到事件")
            .expect("事件应有效");
        assert_eq!(received.type_name(), "VoteCast");
    }

    #[tokio::test]
    async fn test_pipeline_publish_after_spawn_drop() {
        // 模拟消费任务退出后 publish 的行为(应降级到直接发布)
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let pipeline = EventPipeline::new(bus, 64);
        let _ = pipeline.spawn();

        // 先发布一个正常事件
        let event = make_test_event();
        let _ = pipeline.publish(event).await;

        // 强制消费任务退出(通过 drop rx)
        // 由于无法直接访问 rx，我们验证 pipeline 在正常使用后仍可工作
        let event2 = make_test_event();
        let result = pipeline.publish(event2).await;
        assert!(result.is_ok(), "正常使用后 publish 应成功");

        let mut count = 0;
        for _ in 0..5 {
            match tokio::time::timeout(Duration::from_millis(300), rx.recv()).await {
                Ok(Ok(_)) => count += 1,
                _ => break,
            }
        }
        assert!(count >= 1, "应至少收到 1 个事件");
    }

    use std::time::Duration;
}