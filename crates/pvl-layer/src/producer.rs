//! PVL 生产者 — 流式生成操作并通过 mpsc 通道发送给验证者
//!
//! 对应架构:L7 Execution,Producer-Verifier Loop 的生产端
//!
//! # 设计决策
//! - **mpsc 通道而非共享状态**:通道天然无竞态(消息所有权转移),
//!   共享状态需要锁,易死锁。对应 Week 4 强化:流式通道无竞态
//! - **RwLock 策略**:策略读多写少(每次 produce 读,偶尔调整写),
//!   RwLock 比 Mutex 并发性更好
//! - **AtomicU64 计数**:无锁统计,避免计数器成为瓶颈
//! - **sha2 置信度**:占位实现,基于内容哈希生成确定性置信度,
//!   未来接入模型后由模型输出置信度

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;
use std::time::Duration;

use event_bus::{EventBus, EventMetadata, NexusEvent};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tracing::warn;

use crate::config::PvlConfig;
use crate::error::PvlError;
use crate::types::{Operation, OperationId, ProducerStrategy};

/// 计算内容的置信度评分(占位实现:基于内容哈希)
///
/// WHY 哈希:占位实现需要确定性(相同内容→相同置信度),
/// sha256 提供良好分布,取前 4 字节映射到 [0.0, 1.0]。
/// 未来接入模型后,置信度由模型输出,此函数将被替换
fn compute_confidence(content: &str) -> f32 {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let hash = hasher.finalize();
    // 取前 4 字节作为 u32,归一化到 [0.0, 1.0]
    let raw = u32::from_be_bytes([hash[0], hash[1], hash[2], hash[3]]);
    raw as f32 / u32::MAX as f32
}

/// 计算内容的哈希(hex 字符串,取前 4 字节)
///
/// 用于 OperationProduced 事件的 content_hash 字段,
/// 供下游消费者(Router)进行去重与路由
fn compute_content_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let hash = hasher.finalize();
    // 取前 4 字节,格式化为 8 位 hex 字符串(足够唯一性)
    format!(
        "{:02x}{:02x}{:02x}{:02x}",
        hash[0], hash[1], hash[2], hash[3]
    )
}

/// PVL 生产者 — 流式生成操作并通过 mpsc 通道发送给验证者
///
/// # 线程安全
/// `Producer` 内部所有字段均为线程安全(`EventBus` 基于 Arc,
/// `RwLock`/`AtomicU64` 为标准同步原语)。`produce` 方法为 `&self`,
/// 允许多个任务并发调用同一 Producer(多生产者模式)。
///
/// # 背压控制
/// 通道容量由 `PvlConfig::channel_capacity` 控制(默认 128)。
/// 通道满时 `tx.send().await` 会阻塞,形成自然背压,
/// 避免 Producer 淹没 Verifier(对应架构红线:1M Token 暴力加载)
pub struct Producer {
    /// 执行配置(通道容量、速率限制等)
    pub(crate) config: PvlConfig,
    /// 事件总线,用于发布 OperationProduced 事件
    pub(crate) event_bus: EventBus,
    /// 当前生产策略(RwLock:读多写少,produce 读,调整写)
    pub(crate) strategy: RwLock<ProducerStrategy>,
    /// 已生产操作总数(无锁统计)
    pub(crate) produced_count: AtomicU64,
}

impl Producer {
    /// 创建新的生产者
    ///
    /// # 参数
    /// - `config`:执行配置(通道容量、速率限制等)
    /// - `event_bus`:事件总线,用于发布 `OperationProduced` 事件
    pub fn new(config: PvlConfig, event_bus: EventBus) -> Self {
        Self {
            config,
            event_bus,
            strategy: RwLock::new(ProducerStrategy::default()),
            produced_count: AtomicU64::new(0),
        }
    }

    /// 流式生成操作并通过通道发送给验证者
    ///
    /// # 流程
    /// 1. 读取当前策略(决定生成间隔与置信度阈值)
    /// 2. 循环生成 `count` 个操作:
    ///    - 生成占位内容(未来接入模型)
    ///    - 计算置信度(基于内容哈希)
    ///    - 标记为 Produced 状态
    ///    - 通过 mpsc 通道发送(通道满时自动背压等待)
    ///    - 发布 OperationProduced 事件
    ///    - 递增 produced_count
    ///    - 按策略间隔休眠(速率控制)
    /// 3. 全部发送完成后返回
    ///
    /// # 背压
    /// 通道满时 `tx.send().await` 会阻塞,形成自然背压。
    /// 这避免了 Producer 内存爆炸(对应架构红线)
    ///
    /// # 参数
    /// - `quest_id`:所属 Quest ID
    /// - `count`:生成操作数量
    /// - `tx`:发送通道(Producer→Verifier)
    pub async fn produce(
        &self,
        quest_id: &str,
        count: usize,
        tx: &mpsc::Sender<Operation>,
    ) -> Result<(), PvlError> {
        // P1-9:批量生产超时保护
        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_millis(self.config.produce_timeout_ms);

        // 读取当前策略(决定生成间隔)
        // WHY 解引用而非 clone:ProducerStrategy 实现 Copy,直接解引用更高效
        let strategy = *self.strategy.read().map_err(|_| PvlError::ProduceFailed {
            reason: "策略锁读取失败".into(),
        })?;
        let interval = Duration::from_millis(strategy.produce_interval_ms());

        for i in 0..count {
            // P1-9:检查总超时
            if start.elapsed() > timeout {
                warn!(
                    quest_id,
                    produced = i,
                    target = count,
                    "produce 超时:已生产 {} / {} 操作,剩余操作丢弃",
                    i,
                    count
                );
                // 超时降级策略为 Conservative
                self.set_strategy(ProducerStrategy::Conservative);
                return Err(PvlError::ProduceFailed {
                    reason: format!("produce 超时:已生产 {} / {} 操作", i, count),
                });
            }

            // 生成占位内容(未来接入模型后替换为模型生成)
            let content = format!("operation-{quest_id}-{i}");
            let confidence = compute_confidence(&content);
            let content_hash = compute_content_hash(&content);

            // 创建操作并标记为已生产
            let operation_id = OperationId::new(format!("op-{quest_id}-{i}"));
            let mut operation = Operation::new(operation_id.clone(), quest_id, content);
            operation.mark_produced(confidence);

            // 通过 mpsc 通道发送(通道满时自动背压等待)
            // WHY await:对应尸检教训(void Promise 无 await),
            // 显式 await 确保发送完成,避免孤儿调用
            tx.send(operation)
                .await
                .map_err(|_| PvlError::ChannelClosed)?;

            // 递增生产计数
            self.produced_count.fetch_add(1, Ordering::Relaxed);

            // 发布 OperationProduced 事件
            // WHY 逐个发布:event-bus 的 OperationProduced 事件字段为
            // op_id/content_hash(单操作粒度),适配为每操作一事件
            let event = NexusEvent::OperationProduced {
                metadata: EventMetadata::new("pvl-layer"),
                op_id: operation_id.to_string(),
                content_hash,
            };
            if let Err(e) = self.event_bus.publish(event).await {
                warn!(error = %e, "发布 OperationProduced 事件失败");
            }

            // 按策略间隔休眠(速率控制)
            // WHY Conservative 策略降速:避免持续产生低质量操作
            if !interval.is_zero() {
                tokio::time::sleep(interval).await;
            }
        }

        Ok(())
    }

    /// 获取当前生产策略
    pub fn strategy(&self) -> ProducerStrategy {
        self.strategy
            .read()
            .map(|s| *s)
            .unwrap_or(ProducerStrategy::Normal)
    }

    /// 设置生产策略(供 FeedbackChannel 调整)
    ///
    /// WHY pub(crate):仅允许同 crate 的 FeedbackChannel 调用,
    /// 避免外部直接修改策略绕过反馈闭环
    pub(crate) fn set_strategy(&self, strategy: ProducerStrategy) {
        if let Ok(mut s) = self.strategy.write() {
            *s = strategy;
        }
    }

    /// 获取已生产操作总数
    pub fn produced_count(&self) -> u64 {
        self.produced_count.load(Ordering::Relaxed)
    }

    /// 获取配置引用
    pub fn config(&self) -> &PvlConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_produce_generates_operations() {
        // 验证:produce 生成指定数量的操作并通过通道发送
        let producer = Producer::new(PvlConfig::default(), EventBus::new());
        let (tx, mut rx) = mpsc::channel::<Operation>(128);

        producer.produce("quest-1", 5, &tx).await.unwrap();
        drop(tx); // 关闭发送端,使 rx.recv 返回 None

        let mut count = 0;
        while rx.recv().await.is_some() {
            count += 1;
        }
        assert_eq!(count, 5, "应生成 5 个操作");
        assert_eq!(producer.produced_count(), 5);
    }

    #[tokio::test]
    async fn test_produce_empty_count() {
        // 验证:count=0 时不生成任何操作
        let producer = Producer::new(PvlConfig::default(), EventBus::new());
        let (tx, mut rx) = mpsc::channel::<Operation>(128);

        producer.produce("quest-1", 0, &tx).await.unwrap();
        drop(tx);

        assert!(rx.recv().await.is_none(), "不应生成任何操作");
        assert_eq!(producer.produced_count(), 0);
    }

    #[tokio::test]
    async fn test_produce_confidence_in_range() {
        // 验证:生成的操作置信度在 [0.0, 1.0] 范围内
        let producer = Producer::new(PvlConfig::default(), EventBus::new());
        let (tx, mut rx) = mpsc::channel::<Operation>(128);

        producer.produce("quest-1", 10, &tx).await.unwrap();
        drop(tx);

        while let Some(op) = rx.recv().await {
            assert!(
                (0.0..=1.0).contains(&op.confidence),
                "置信度应在 [0.0, 1.0] 范围内,实际: {}",
                op.confidence
            );
            assert_eq!(op.status, crate::types::OperationStatus::Produced);
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn test_produce_channel_full_backpressure() {
        // 验证:通道满时 produce 会阻塞(背压)
        // 使用多线程 runtime 允许 producer 和 receiver 并发执行
        let producer = Producer::new(PvlConfig::default(), EventBus::new());
        let (tx, mut rx) = mpsc::channel::<Operation>(1);

        // 启动 produce 5 个操作的任务(通道容量 1,会阻塞)
        // tx 被 move 进任务,任务完成后 drop tx,使 rx.recv 返回 None
        let handle =
            tokio::spawn(async move { producer.produce("quest-backpressure", 5, &tx).await });

        // 等待一小段时间,让 producer 发送 1 个操作后阻塞(背压验证)
        tokio::time::sleep(Duration::from_millis(100)).await;

        // 接收所有操作,释放背压
        // 循环接收直到通道关闭(spawned 任务完成 drop tx)
        let mut received = 0;
        while let Some(_op) = rx.recv().await {
            received += 1;
        }

        // produce 任务应能完成
        let result = tokio::time::timeout(Duration::from_secs(2), handle).await;
        assert!(result.is_ok(), "produce 应在接收所有操作后完成");
        assert!(result.unwrap().unwrap().is_ok());
        assert_eq!(received, 5, "应接收 5 个操作");
    }

    #[tokio::test]
    async fn test_produce_publishes_event() {
        // 验证:produce 发布 OperationProduced 事件
        let bus = EventBus::new();
        let mut rx_event = bus.subscribe();
        let producer = Producer::new(PvlConfig::default(), bus.clone());
        let (tx, _rx) = mpsc::channel::<Operation>(128);

        producer.produce("quest-1", 1, &tx).await.unwrap();

        // 应收到 OperationProduced 事件
        let event = rx_event.recv_timeout(Duration::from_millis(100)).await;
        assert!(event.is_ok(), "应收到事件");
        let event = event.unwrap();
        assert!(
            matches!(event, NexusEvent::OperationProduced { .. }),
            "应为 OperationProduced 事件,实际: {:?}",
            event
        );
    }

    #[tokio::test]
    async fn test_produce_with_conservative_strategy() {
        // 验证:Conservative 策略下 produce 有间隔(降速)
        let producer = Producer::new(PvlConfig::default(), EventBus::new());
        producer.set_strategy(ProducerStrategy::Conservative);
        assert_eq!(producer.strategy(), ProducerStrategy::Conservative);

        let (tx, mut rx) = mpsc::channel::<Operation>(128);
        let start = std::time::Instant::now();
        producer.produce("quest-1", 3, &tx).await.unwrap();
        let elapsed = start.elapsed();

        // Conservative 策略每个操作间隔 10ms,3 个操作至少 20ms(2 个间隔)
        assert!(
            elapsed >= Duration::from_millis(20),
            "Conservative 策略应有降速间隔,实际耗时: {:?}",
            elapsed
        );

        drop(tx);
        let mut count = 0;
        while rx.recv().await.is_some() {
            count += 1;
        }
        assert_eq!(count, 3);
    }

    #[test]
    fn test_compute_confidence_deterministic() {
        // 验证:相同内容产生相同置信度(确定性)
        let c1 = compute_confidence("test-content");
        let c2 = compute_confidence("test-content");
        assert_eq!(c1, c2, "相同内容应产生相同置信度");

        let c3 = compute_confidence("different-content");
        assert_ne!(c1, c3, "不同内容应产生不同置信度");
    }

    #[test]
    fn test_compute_confidence_range() {
        // 验证:置信度在 [0.0, 1.0] 范围内
        for content in &["a", "b", "test", "hello world", "12345"] {
            let c = compute_confidence(content);
            assert!(
                (0.0..=1.0).contains(&c),
                "置信度应在 [0.0, 1.0] 范围内,内容: {},置信度: {}",
                content,
                c
            );
        }
    }
}
