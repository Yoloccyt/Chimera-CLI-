//! 分块生成器 — 基于FlashAttention论文的IO优化流式生成
//!
//! 将大批量操作分块处理，每块生成后立即可发送，避免全量加载到内存。
//!
//! ## 核心概念映射
//! | FlashAttention 概念 | PVL 生产者映射 |
//! |---|---|
//! | Tiling (SRAM块) | 分块生成策略 (Chunked Generation) |
//! | HBM ↔ SRAM | 主内存 ↔ CPU缓存 |

use std::cmp::Ordering as CmpOrdering;
use std::time::Instant;

use crate::config::PvlConfig;
use crate::types::{Operation, OperationId, ProducerStrategy};

/// 流水线统计
#[derive(Debug, Clone, Default)]
pub struct PipelineStats {
    /// 生成阶段耗时（毫秒）
    pub generation_ms: f32,
    /// 计算阶段耗时（毫秒）
    pub computation_ms: f32,
    /// 传输阶段耗时（毫秒）
    pub transmission_ms: f32,
    /// 总操作数
    pub total_operations: u64,
    /// 分块计数
    pub chunk_count: u64,
}

/// 分块生成器 — 将大批量操作分块处理
///
/// 参考FlashAttention的分块策略：将大任务拆分为适合内存的块，
/// 每块生成后立即可发送，避免全量加载到内存。
///
/// # 设计决策
/// - **分块大小默认128**：与mpsc通道容量对齐，避免通道积压
/// - **动态调整**：基于吞吐量历史记录动态调整分块大小
/// - **冷却时间**：防止频繁调整导致振荡
pub struct ChunkedProducer {
    /// 当前分块大小
    chunk_size: usize,
    /// 最小分块大小
    min_chunk_size: usize,
    /// 最大分块大小
    max_chunk_size: usize,
    /// 吞吐量历史记录（操作数/毫秒）
    throughput_history: Vec<f32>,
    /// 历史窗口大小
    window_size: usize,
    /// 调整冷却时间（毫秒）
    cooldown_ms: u64,
    /// 上次实际调整时刻,None 表示从未调整过(不触发冷却)
    last_adjustment: Option<Instant>,
    /// 调整因子
    adjustment_factor: f32,
}

impl ChunkedProducer {
    /// 创建新的分块生成器
    ///
    /// # 参数
    /// - `config`: 执行配置
    pub fn new(config: &PvlConfig) -> Self {
        Self {
            chunk_size: config.chunk_size,
            min_chunk_size: config.min_chunk_size,
            max_chunk_size: config.max_chunk_size,
            throughput_history: Vec::with_capacity(10),
            window_size: 10,
            cooldown_ms: 50,
            last_adjustment: None,
            adjustment_factor: 1.25,
        }
    }

    /// 获取当前分块大小
    pub fn chunk_size(&self) -> usize {
        self.chunk_size
    }

    /// 计算下一个分块范围
    ///
    /// # 参数
    /// - `processed`: 已处理的操作数
    /// - `total`: 总操作数
    ///
    /// # 返回
    /// (start, end) 分块范围，包含start，不包含end
    pub fn next_chunk_range(&self, processed: usize, total: usize) -> (usize, usize) {
        let start = processed;
        let end = (start + self.chunk_size).min(total);
        (start, end)
    }

    /// 记录分块吞吐量并动态调整分块大小
    ///
    /// # 参数
    /// - `chunk_size`: 本次分块大小
    /// - `elapsed_ms`: 耗时（毫秒）
    /// - `strategy`: 当前生产策略
    pub fn record_chunk_performance(
        &mut self,
        chunk_size: usize,
        elapsed_ms: f32,
        strategy: ProducerStrategy,
    ) {
        if elapsed_ms.total_cmp(&0.0f32) != CmpOrdering::Greater || chunk_size == 0 {
            return;
        }

        let throughput = chunk_size as f32 / elapsed_ms;
        self.throughput_history.push(throughput);
        if self.throughput_history.len() > self.window_size {
            self.throughput_history.remove(0);
        }

        // 冷却检查:仅在实际调整过之后才限制频率,首次调整不受冷却限制
        if let Some(last) = self.last_adjustment {
            if last.elapsed().as_millis() < self.cooldown_ms as u128 {
                return;
            }
        }
        if self.throughput_history.len() < 2 {
            return;
        }

        let recent = self.throughput_history.last().copied().unwrap_or(0.0f32);
        let prev_idx = self.throughput_history.len().saturating_sub(2);
        let previous = self
            .throughput_history
            .get(prev_idx)
            .copied()
            .unwrap_or(recent);

        let mut new_size = self.chunk_size;

        // 吞吐量上升 > 15%
        if previous.total_cmp(&0.0f32) != CmpOrdering::Equal
            && recent.total_cmp(&(previous * 1.15f32)) == CmpOrdering::Greater
        {
            new_size = (new_size as f32 * self.adjustment_factor) as usize;
        } else if previous.total_cmp(&0.0f32) != CmpOrdering::Equal
            && recent.total_cmp(&(previous * 0.85f32)) == CmpOrdering::Less
        {
            // 吞吐量下降 < 85%
            new_size = (new_size as f32 / self.adjustment_factor) as usize;
        }

        // 根据策略约束大小
        match strategy {
            ProducerStrategy::Conservative => {
                new_size = new_size
                    .min(self.max_chunk_size / 2)
                    .max(self.min_chunk_size);
            }
            ProducerStrategy::Normal => {
                new_size = new_size.clamp(self.min_chunk_size, self.max_chunk_size);
            }
            ProducerStrategy::Aggressive => {
                new_size = new_size.clamp(self.min_chunk_size * 2, self.max_chunk_size);
            }
        }

        if new_size != self.chunk_size {
            self.chunk_size = new_size;
            self.last_adjustment = Some(Instant::now());
        }
    }

    /// 生成分块操作
    ///
    /// # 参数
    /// - `quest_id`: 所属 Quest ID
    /// - `start`: 分块起始索引
    /// - `end`: 分块结束索引（不包含）
    ///
    /// # 返回
    /// 生成的原始操作列表（未设置置信度）
    pub fn generate_raw_chunk(&self, quest_id: &str, start: usize, end: usize) -> Vec<Operation> {
        let chunk_len = end - start;
        let mut result = Vec::with_capacity(chunk_len);
        for i in start..end {
            let content = format!("operation-{quest_id}-{i}");
            let op_id = OperationId::new(format!("op-{quest_id}-{i}"));
            let op = Operation::new(op_id, quest_id, content);
            result.push(op);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunked_producer_new() {
        let config = PvlConfig::default();
        let producer = ChunkedProducer::new(&config);
        assert_eq!(producer.chunk_size(), 128);
        assert_eq!(producer.min_chunk_size, 16);
        assert_eq!(producer.max_chunk_size, 512);
    }

    #[test]
    fn test_next_chunk_range() {
        let config = PvlConfig::default();
        let producer = ChunkedProducer::new(&config);

        // 第一块
        let (start, end) = producer.next_chunk_range(0, 200);
        assert_eq!(start, 0);
        assert_eq!(end, 128);

        // 中间块
        let (start, end) = producer.next_chunk_range(128, 200);
        assert_eq!(start, 128);
        assert_eq!(end, 200);

        // 已处理完毕
        let (start, end) = producer.next_chunk_range(200, 200);
        assert_eq!(start, 200);
        assert_eq!(end, 200);
    }

    #[test]
    fn test_generate_raw_chunk() {
        let config = PvlConfig::default();
        let producer = ChunkedProducer::new(&config);
        let chunk = producer.generate_raw_chunk("quest", 0, 5);

        assert_eq!(chunk.len(), 5);
        for (i, op) in chunk.iter().enumerate() {
            assert_eq!(op.content, format!("operation-quest-{}", i));
            assert_eq!(op.status, crate::types::OperationStatus::Pending);
            assert_eq!(op.confidence.total_cmp(&0.0f32), CmpOrdering::Equal);
        }
    }

    #[test]
    fn test_record_chunk_performance_adjust_up() {
        let config = PvlConfig::default();
        let mut producer = ChunkedProducer::new(&config);
        let initial = producer.chunk_size();

        // 模拟吞吐量上升
        producer.record_chunk_performance(16, 10.0, ProducerStrategy::Normal);
        producer.record_chunk_performance(20, 10.0, ProducerStrategy::Normal);

        assert!(producer.chunk_size() > initial, "吞吐量上升应增大分块大小");
    }

    #[test]
    fn test_record_chunk_performance_adjust_down() {
        let config = PvlConfig::default();
        let mut producer = ChunkedProducer::new(&config);
        let initial = producer.chunk_size();

        // 模拟吞吐量下降
        producer.record_chunk_performance(16, 10.0, ProducerStrategy::Normal);
        producer.record_chunk_performance(16, 20.0, ProducerStrategy::Normal);

        assert!(producer.chunk_size() < initial, "吞吐量下降应减小分块大小");
    }

    #[test]
    fn test_strategy_constraints_conservative() {
        let config = PvlConfig::default();
        let mut producer = ChunkedProducer::new(&config);

        // 模拟吞吐量上升
        producer.record_chunk_performance(16, 10.0, ProducerStrategy::Normal);
        producer.record_chunk_performance(32, 10.0, ProducerStrategy::Normal);

        let _normal_size = producer.chunk_size();

        // 切换到Conservative策略
        producer.record_chunk_performance(32, 10.0, ProducerStrategy::Conservative);

        assert!(
            producer.chunk_size() <= config.max_chunk_size / 2,
            "Conservative策略下分块大小应受限"
        );
    }

    #[test]
    fn test_strategy_constraints_aggressive() {
        let config = PvlConfig::default();
        let mut producer = ChunkedProducer::new(&config);

        // 模拟吞吐量上升
        producer.record_chunk_performance(16, 10.0, ProducerStrategy::Normal);
        producer.record_chunk_performance(32, 10.0, ProducerStrategy::Normal);

        let _normal_size = producer.chunk_size();

        // 切换到Aggressive策略
        producer.record_chunk_performance(32, 10.0, ProducerStrategy::Aggressive);

        assert!(
            producer.chunk_size() >= config.min_chunk_size * 2,
            "Aggressive策略下分块大小应至少为最小值的2倍"
        );
    }

    #[test]
    fn test_pipeline_stats_default() {
        let stats = PipelineStats::default();
        assert_eq!(stats.chunk_count, 0);
        assert_eq!(stats.total_operations, 0);
        assert_eq!(stats.generation_ms.total_cmp(&0.0f32), CmpOrdering::Equal);
        assert_eq!(stats.computation_ms.total_cmp(&0.0f32), CmpOrdering::Equal);
        assert_eq!(stats.transmission_ms.total_cmp(&0.0f32), CmpOrdering::Equal);
    }

    #[test]
    fn test_small_chunk_handling() {
        let config = PvlConfig {
            chunk_size: 8,
            ..Default::default()
        };
        let producer = ChunkedProducer::new(&config);

        let (start, end) = producer.next_chunk_range(0, 5);
        assert_eq!(start, 0);
        assert_eq!(end, 5);

        let chunk = producer.generate_raw_chunk("quest-small", 0, 5);
        assert_eq!(chunk.len(), 5);
    }
}
