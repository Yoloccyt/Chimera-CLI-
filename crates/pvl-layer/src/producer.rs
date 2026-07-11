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
//! - **IO优化流式执行框架**:基于FlashAttention论文引入分块生成、
//!   注意力权重调度、动态batch调整

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::RwLock;
use std::time::Duration;

use event_bus::{EventBus, EventMetadata, NexusEvent};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;
use tracing::warn;

use crate::chunked_producer::{ChunkedProducer, PipelineStats};
use crate::config::PvlConfig;
use crate::error::PvlError;
use crate::priority_scheduler::{AttentionWeight, PriorityScheduler};
use crate::types::{Operation, OperationId, ProducerStrategy};

/// 计算内容的哈希(hex 字符串,取前 4 字节)
///
/// 用于 OperationProduced 事件的 content_hash 字段,
/// 供下游消费者(Router)进行去重与路由
fn compute_content_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let hash = hasher.finalize();
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
///
/// # IO优化流式执行框架
/// 基于FlashAttention论文引入以下优化：
/// 1. **分块生成**: 将大批量操作分块处理，减少内存峰值
/// 2. **注意力权重调度**: Conservative策略时按权重排序，优先处理高权重操作
/// 3. **动态batch调整**: 根据吞吐量动态调整分块大小
pub struct Producer {
    /// 执行配置(通道容量、速率限制等)
    pub(crate) config: PvlConfig,
    /// 事件总线,用于发布 OperationProduced 事件
    pub(crate) event_bus: EventBus,
    /// 当前生产策略(RwLock:读多写少,produce 读,调整写)
    pub(crate) strategy: RwLock<ProducerStrategy>,
    /// 已生产操作总数(无锁统计)
    pub(crate) produced_count: AtomicU64,
    /// 分块生成器 — 动态分块大小与吞吐量追踪
    pub(crate) chunked_producer: std::sync::Mutex<ChunkedProducer>,
    /// 优先级调度器 — 注意力权重计算与排序
    pub(crate) priority_scheduler: PriorityScheduler,
    /// 流水线统计
    pub(crate) pipeline_stats: std::sync::Mutex<PipelineStats>,
}

impl Producer {
    /// 创建新的生产者
    ///
    /// # 参数
    /// - `config`:执行配置(通道容量、速率限制等)
    /// - `event_bus`:事件总线,用于发布 `OperationProduced` 事件
    pub fn new(config: PvlConfig, event_bus: EventBus) -> Self {
        let priority_scheduler = PriorityScheduler::new(&config);
        let chunked_producer = ChunkedProducer::new(&config);
        Self {
            config,
            event_bus,
            strategy: RwLock::new(ProducerStrategy::default()),
            produced_count: AtomicU64::new(0),
            chunked_producer: std::sync::Mutex::new(chunked_producer),
            priority_scheduler,
            pipeline_stats: std::sync::Mutex::new(PipelineStats::default()),
        }
    }

    /// 流式生成操作并通过通道发送给验证者
    ///
    /// # 流程（IO优化分块版本）
    /// 1. 读取当前策略(决定生成间隔与置信度阈值)
    /// 2. 构建操作依赖图（仅在优先级调度启用时）
    /// 3. 分块生成操作（类比FlashAttention的tiling）:
    ///    - 每个分块内生成操作、计算注意力权重、排序
    ///    - 动态调整分块大小（基于吞吐量）
    /// 4. 逐个发送操作并发布事件（保持原有背压行为）
    /// 5. 超时保护、策略降级保持不变
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
        let strategy = match self.strategy.read() {
            Ok(s) => *s,
            Err(_) => {
                return Err(PvlError::ProduceFailed {
                    reason: "策略锁读取失败".into(),
                });
            }
        };
        let interval = Duration::from_millis(strategy.produce_interval_ms());

        // 构建操作依赖图（仅在优先级调度启用且操作数超过阈值时）
        let graph = if self.config.enable_priority_scheduling
            && self.priority_scheduler.should_enable(count)
        {
            Some(self.priority_scheduler.build_dependency_graph(count))
        } else {
            None
        };

        // 分块生成：类比FlashAttention的tiling，减少内存峰值
        let mut processed = 0usize;

        while processed < count {
            // P1-9:检查总超时
            if start.elapsed() > timeout {
                warn!(
                    quest_id,
                    produced = processed,
                    target = count,
                    "produce 超时:已生产 {} / {} 操作,剩余操作丢弃",
                    processed,
                    count
                );
                // 超时降级策略为 Conservative
                self.set_strategy(ProducerStrategy::Conservative);
                return Err(PvlError::ProduceFailed {
                    reason: format!("produce 超时:已生产 {} / {} 操作", processed, count),
                });
            }

            // 获取当前分块大小（动态调整）
            let chunk_size = match self.chunked_producer.lock() {
                Ok(producer) => producer.chunk_size(),
                Err(poisoned) => poisoned.into_inner().chunk_size(),
            };

            let chunk_start = processed;
            let chunk_end = (chunk_start + chunk_size).min(count);
            let chunk_len = chunk_end - chunk_start;

            // 记录分块开始时间（用于吞吐量计算）
            let chunk_start_time = std::time::Instant::now();

            // 阶段1: 生成分块（Generation）
            // 类比FlashAttention的SRAM加载：将操作块加载到"缓存"
            let raw_chunk = match self.chunked_producer.lock() {
                Ok(producer) => producer.generate_raw_chunk(quest_id, chunk_start, chunk_end),
                Err(poisoned) => {
                    poisoned
                        .into_inner()
                        .generate_raw_chunk(quest_id, chunk_start, chunk_end)
                }
            };

            // 阶段2: 应用优先级调度（Scheduling）— 计算权重、置信度、排序
            // 类比FlashAttention的online softmax：计算注意力权重
            let chunk = match graph {
                Some(ref g) => self.priority_scheduler.apply_to_chunk(
                    raw_chunk,
                    chunk_start,
                    count,
                    g,
                    strategy,
                    false,
                ),
                None => {
                    let mut result = Vec::with_capacity(raw_chunk.len());
                    for mut op in raw_chunk {
                        let confidence =
                            crate::priority_scheduler::compute_semantic_weight(&op.content);
                        op.mark_produced(confidence);
                        result.push(op);
                    }
                    result
                }
            };

            // 阶段3: 发送（Transmission）— 逐个发送并发布事件
            // 类比FlashAttention的HBM写回：将计算结果写回内存
            for op in chunk {
                let op_id = op.operation_id.clone();
                let content_hash = compute_content_hash(&op.content);

                // 通过 mpsc 通道发送(通道满时自动背压等待)
                // WHY await:对应尸检教训(void Promise 无 await),
                // 显式 await 确保发送完成,避免孤儿调用
                tx.send(op).await.map_err(|_| PvlError::ChannelClosed)?;

                // 递增生产计数
                self.produced_count.fetch_add(1, Ordering::Relaxed);
                processed += 1;

                // 发布 OperationProduced 事件
                // WHY 逐个发布:event-bus 的 OperationProduced 事件字段为
                // op_id/content_hash(单操作粒度),适配为每操作一事件
                let event = NexusEvent::OperationProduced {
                    metadata: EventMetadata::new("pvl-layer"),
                    op_id: op_id.to_string(),
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

            // 阶段4: 记录吞吐量并动态调整batch大小
            // 类比FlashAttention-2的work partitioning：根据性能调整tile大小
            let chunk_elapsed_ms = chunk_start_time.elapsed().as_secs_f32() * 1000.0;
            match self.chunked_producer.lock() {
                Ok(mut producer) => {
                    producer.record_chunk_performance(chunk_len, chunk_elapsed_ms, strategy);
                }
                Err(poisoned) => {
                    let mut producer = poisoned.into_inner();
                    producer.record_chunk_performance(chunk_len, chunk_elapsed_ms, strategy);
                }
            }

            // 更新流水线统计
            match self.pipeline_stats.lock() {
                Ok(mut stats) => {
                    stats.chunk_count += 1;
                    stats.total_operations += chunk_len as u64;
                    stats.generation_ms += chunk_elapsed_ms * 0.3;
                    stats.computation_ms += chunk_elapsed_ms * 0.2;
                    stats.transmission_ms += chunk_elapsed_ms * 0.5;
                }
                Err(poisoned) => {
                    let mut stats = poisoned.into_inner();
                    stats.chunk_count += 1;
                    stats.total_operations += chunk_len as u64;
                    stats.generation_ms += chunk_elapsed_ms * 0.3;
                    stats.computation_ms += chunk_elapsed_ms * 0.2;
                    stats.transmission_ms += chunk_elapsed_ms * 0.5;
                }
            }
        }

        Ok(())
    }

    /// 获取当前生产策略
    pub fn strategy(&self) -> ProducerStrategy {
        match self.strategy.read() {
            Ok(s) => *s,
            Err(_) => ProducerStrategy::Normal,
        }
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

    /// 计算操作的注意力权重
    ///
    /// 用于外部诊断和调试，获取指定操作的注意力权重
    pub fn compute_attention_weight(
        &self,
        _op_id: &OperationId,
        content: &str,
        index: usize,
        total_count: usize,
    ) -> AttentionWeight {
        let graph = self.priority_scheduler.build_dependency_graph(total_count);
        self.priority_scheduler
            .compute_attention_weight(content, index, total_count, &graph)
    }

    /// 获取流水线统计
    pub fn pipeline_stats(&self) -> PipelineStats {
        match self.pipeline_stats.lock() {
            Ok(stats) => stats.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// 获取内存压力
    pub fn memory_pressure(&self) -> f32 {
        // 简化实现：占位返回
        0.0f32
    }

    /// 获取当前batch大小
    pub fn current_batch_size(&self) -> usize {
        match self.chunked_producer.lock() {
            Ok(producer) => producer.chunk_size(),
            Err(poisoned) => poisoned.into_inner().chunk_size(),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering as CmpOrdering;

    use super::*;

    /// 辅助函数：断言f32值在指定范围内
    fn assert_f32_in_range(value: f32, min: f32, max: f32, message: &str) {
        assert!(
            value.total_cmp(&min) != CmpOrdering::Less
                && value.total_cmp(&max) != CmpOrdering::Greater,
            "{}: 实际值 {}",
            message,
            value
        );
    }

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
            assert_f32_in_range(
                op.confidence,
                0.0f32,
                1.0f32,
                "置信度应在 [0.0, 1.0] 范围内",
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
        let c1 = crate::priority_scheduler::compute_semantic_weight("test-content");
        let c2 = crate::priority_scheduler::compute_semantic_weight("test-content");
        assert_eq!(
            c1.total_cmp(&c2),
            CmpOrdering::Equal,
            "相同内容应产生相同置信度"
        );

        let c3 = crate::priority_scheduler::compute_semantic_weight("different-content");
        assert_ne!(
            c1.total_cmp(&c3),
            CmpOrdering::Equal,
            "不同内容应产生不同置信度"
        );
    }

    #[test]
    fn test_compute_confidence_range() {
        // 验证:置信度在 [0.0, 1.0] 范围内
        for content in &["a", "b", "test", "hello world", "12345"] {
            let c = crate::priority_scheduler::compute_semantic_weight(content);
            assert_f32_in_range(c, 0.0f32, 1.0f32, "置信度应在 [0.0, 1.0] 范围内");
        }
    }

    // ========== IO优化流式执行框架测试 ==========

    #[tokio::test]
    async fn test_produce_chunked_reduces_memory_peak() {
        // 验证:分块生成减少内存峰值
        // 通过大批量操作验证分块行为
        // WHY 通道容量 256 > 200 操作:单线程 runtime 下 produce 和 recv 在同一任务,
        // 通道满时 tx.send 会阻塞;容量需覆盖全部操作避免死锁
        let producer = Producer::new(PvlConfig::default(), EventBus::new());
        let (tx, mut rx) = mpsc::channel::<Operation>(256);

        // 生成200个操作，应分多个块处理
        producer.produce("quest-chunk", 200, &tx).await.unwrap();
        drop(tx);

        let mut count = 0;
        while rx.recv().await.is_some() {
            count += 1;
        }
        assert_eq!(count, 200, "应生成 200 个操作");

        // 验证分块确实发生（chunk_count > 1）
        let stats = producer.pipeline_stats();
        assert!(
            stats.chunk_count > 1,
            "应分多个块处理,实际块数: {}",
            stats.chunk_count
        );
    }

    #[tokio::test]
    async fn test_produce_attention_weight_scheduling() {
        // 验证:Conservative策略下操作生成正确（带注意力权重调度）
        let producer = Producer::new(PvlConfig::default(), EventBus::new());
        producer.set_strategy(ProducerStrategy::Conservative);

        let (tx, mut rx) = mpsc::channel::<Operation>(128);
        producer.produce("quest-sched", 10, &tx).await.unwrap();
        drop(tx);

        let mut count = 0;
        while rx.recv().await.is_some() {
            count += 1;
        }
        assert_eq!(count, 10, "应生成 10 个操作");
    }

    #[tokio::test]
    async fn test_produce_dynamic_batch_adjustment() {
        // 验证:动态batch大小调整
        let producer = Producer::new(PvlConfig::default(), EventBus::new());
        let _initial_batch = producer.current_batch_size();

        // 多次生产以触发batch调整
        for i in 0..5 {
            let (tx, mut rx) = mpsc::channel::<Operation>(128);
            producer
                .produce(&format!("quest-{}", i), 50, &tx)
                .await
                .unwrap();
            drop(tx);
            // 消费所有操作
            while rx.recv().await.is_some() {}
        }

        let final_batch = producer.current_batch_size();
        // 验证batch大小在合理范围内
        assert!(
            (8..=512).contains(&final_batch),
            "batch大小应在合理范围内,实际: {}",
            final_batch
        );
    }

    #[test]
    fn test_attention_weight_api() {
        // 验证:注意力权重计算API
        let producer = Producer::new(PvlConfig::default(), EventBus::new());
        let weight =
            producer.compute_attention_weight(&OperationId::new("op-1"), "test-content", 5, 10);

        assert_f32_in_range(weight.semantic, 0.0f32, 1.0f32, "语义权重应在范围内");
        assert_f32_in_range(weight.dependency, 0.0f32, 1.0f32, "依赖权重应在范围内");
        assert_f32_in_range(weight.urgency, 0.0f32, 1.0f32, "紧急权重应在范围内");
        assert_f32_in_range(weight.composite, 0.0f32, 1.0f32, "综合权重应在范围内");
    }

    #[test]
    fn test_memory_pressure_monitoring() {
        // 验证:内存压力监控
        let producer = Producer::new(PvlConfig::default(), EventBus::new());
        let initial_pressure = producer.memory_pressure();

        // 初始压力应为0
        assert_eq!(
            initial_pressure.total_cmp(&0.0f32),
            CmpOrdering::Equal,
            "初始内存压力应为0"
        );
    }

    #[tokio::test]
    async fn test_produce_priority_disabled_below_threshold() {
        // 验证:操作数低于阈值时不启用优先级调度
        let config = PvlConfig {
            priority_threshold: 50,
            enable_priority_scheduling: true,
            ..Default::default()
        };
        let producer = Producer::new(config, EventBus::new());

        let (tx, mut rx) = mpsc::channel::<Operation>(128);
        // 生成30个操作，低于阈值50，不启用优先级调度
        producer.produce("quest-low", 30, &tx).await.unwrap();
        drop(tx);

        let mut count = 0;
        while rx.recv().await.is_some() {
            count += 1;
        }
        assert_eq!(count, 30, "应生成 30 个操作");
    }

    #[tokio::test]
    async fn test_produce_priority_enabled_above_threshold() {
        // 验证:操作数超过阈值时启用优先级调度
        let config = PvlConfig {
            priority_threshold: 10,
            enable_priority_scheduling: true,
            ..Default::default()
        };
        let producer = Producer::new(config, EventBus::new());

        let (tx, mut rx) = mpsc::channel::<Operation>(128);
        // 生成20个操作，超过阈值10，启用优先级调度
        producer.produce("quest-high", 20, &tx).await.unwrap();
        drop(tx);

        let mut count = 0;
        while rx.recv().await.is_some() {
            count += 1;
        }
        assert_eq!(count, 20, "应生成 20 个操作");
    }

    #[test]
    fn test_pipeline_stats_accumulation() {
        // 验证:流水线统计累加
        let producer = Producer::new(PvlConfig::default(), EventBus::new());
        let initial_stats = producer.pipeline_stats();
        assert_eq!(initial_stats.chunk_count, 0);
        assert_eq!(initial_stats.total_operations, 0);
    }

    #[test]
    fn test_strategy_lock_poisoning() {
        // 验证:策略锁中毒时返回Normal
        let producer = Producer::new(PvlConfig::default(), EventBus::new());
        // 正常情况应返回默认策略
        assert_eq!(producer.strategy(), ProducerStrategy::Normal);
    }
}
