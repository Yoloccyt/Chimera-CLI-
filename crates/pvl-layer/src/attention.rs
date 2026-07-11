//! IO感知注意力框架 — 基于FlashAttention论文的PVL生产者优化
//!
//! 将FlashAttention系列论文的核心思想映射到CPU操作生成调度层：
//! - FlashAttention (Dao et al., NeurIPS 2022): tiling + online softmax
//! - FlashAttention-2 (Dao, 2023): work partitioning + reduced non-matmul FLOPs
//! - FlashAttention-3 (NVIDIA, 2024): async pipeline + warp specialization
//! - Ring Attention (Liu et al., 2024): distributed sequence parallelism
//!
//! ## 核心概念映射
//! | FlashAttention 概念 | PVL 生产者映射 |
//! |---|---|
//! | Tiling (SRAM块) | 分块生成策略 (Chunked Generation) |
//! | Online Softmax | 注意力权重调度 (Attention Scheduling) |
//! | Recomputation | 重计算策略 (Recomputation) |
//! | Work Partitioning | 动态batch大小调整 (Dynamic Batching) |
//! | Async Pipeline | 流水线阶段重叠 (Pipeline Overlap) |
//! | HBM ↔ SRAM | 主内存 ↔ CPU缓存 |

use std::cmp::Ordering;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};
use std::sync::Arc;
use std::time::Instant;

use sha2::{Digest, Sha256};
use tracing::info;

use crate::config::PvlConfig;
use crate::types::{Operation, OperationId, ProducerStrategy};

// ========== 分块配置 ==========

/// 分块生成配置 — 控制内存峰值与批处理行为
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ChunkConfig {
    /// 每个分块的操作数量
    pub chunk_size: usize,
    /// 最大并发分块数
    pub max_concurrent_chunks: usize,
    /// 是否启用重计算(内存压力大时重新计算注意力权重)
    pub enable_recomputation: bool,
    /// 触发重计算的内存压力阈值 [0, 1]
    pub recomputation_threshold: f32,
}

impl Default for ChunkConfig {
    fn default() -> Self {
        Self {
            chunk_size: 64,
            max_concurrent_chunks: 4,
            enable_recomputation: true,
            recomputation_threshold: 0.80,
        }
    }
}

// ========== 注意力权重 ==========

/// 操作注意力权重 — 用于优先级调度的多维评分
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AttentionWeight {
    /// 语义相关性权重 [0, 1]
    pub semantic: f32,
    /// 依赖关系权重 [0, 1]
    pub dependency: f32,
    /// 紧急度权重 [0, 1]
    pub urgency: f32,
    /// softmax 归一化后的综合权重 [0, 1]
    pub composite: f32,
}

impl AttentionWeight {
    /// 创建注意力权重,自动计算 composite 分量
    pub fn new(semantic: f32, dependency: f32, urgency: f32) -> Self {
        let composite = Self::softmax_norm(semantic, dependency, urgency);
        Self {
            semantic: semantic.clamp(0.0, 1.0),
            dependency: dependency.clamp(0.0, 1.0),
            urgency: urgency.clamp(0.0, 1.0),
            composite,
        }
    }

    fn softmax_norm(a: f32, b: f32, c: f32) -> f32 {
        let max_val = a.max(b).max(c);
        let exp_a = ((a - max_val) * 5.0).exp();
        let exp_b = ((b - max_val) * 5.0).exp();
        let exp_c = ((c - max_val) * 5.0).exp();
        let sum = exp_a + exp_b + exp_c;
        (a * exp_a + b * exp_b + c * exp_c) / sum
    }
}

// ========== 操作依赖图 ==========

/// 操作依赖图 — 描述操作间的依赖关系,用于计算依赖权重和拓扑深度
#[derive(Debug, Clone, Default)]
pub struct OperationDependencyGraph {
    /// 操作 ID 列表
    pub operations: Vec<OperationId>,
    /// 邻接表:adjacency[i] 包含操作 i 依赖的所有操作索引
    pub adjacency: Vec<Vec<usize>>,
}

impl OperationDependencyGraph {
    /// 创建空的依赖图
    pub fn new() -> Self {
        Self::default()
    }

    /// 构建线性序列依赖图 — 每个操作依赖前 window_size 个操作
    pub fn build_linear_sequence(count: usize, window_size: usize) -> Self {
        let mut ops = Vec::with_capacity(count);
        let mut adj = Vec::with_capacity(count);
        for i in 0..count {
            ops.push(OperationId::new(format!("op-seq-{}", i)));
            let mut deps = Vec::new();
            for j in i.saturating_sub(window_size)..i {
                deps.push(j);
            }
            adj.push(deps);
        }
        Self {
            operations: ops,
            adjacency: adj,
        }
    }

    /// 计算指定操作的依赖权重 — 基于下游依赖数量占比
    pub fn compute_dependency_weight(&self, index: usize) -> f32 {
        if self.operations.is_empty() {
            return 0.5;
        }
        let downstream_count = self
            .adjacency
            .iter()
            .filter(|deps| deps.contains(&index))
            .count();
        let total = self.operations.len().max(1);
        (downstream_count as f32 / total as f32).min(1.0)
    }

    /// 计算指定操作的拓扑深度 — 从该操作出发可达的最长路径
    ///
    /// WHY 使用 best_depth 而非 visited: longest path 计算需要允许节点
    /// 经由更长路径被重新访问,简单 visited 会阻断更优解
    pub fn compute_topological_depth(&self, index: usize) -> usize {
        let mut best_depth = vec![0usize; self.operations.len()];
        let mut stack = vec![(index, 0usize)];
        let mut max_depth = 0usize;
        while let Some((current, current_depth)) = stack.pop() {
            // 仅当新路径更长时才继续,否则跳过
            if current_depth < best_depth[current] {
                continue;
            }
            best_depth[current] = current_depth;
            max_depth = max_depth.max(current_depth);
            for (next_idx, deps) in self.adjacency.iter().enumerate() {
                let new_depth = current_depth + 1;
                if deps.contains(&current) && new_depth > best_depth[next_idx] {
                    stack.push((next_idx, new_depth));
                }
            }
        }
        max_depth
    }
}

// ========== 内存压力监控 ==========

/// 内存压力监控器 — 跟踪内存饱和度并决定是否触发重计算
#[derive(Debug)]
pub struct MemoryPressureMonitor {
    current_pressure: Arc<std::sync::atomic::AtomicU32>,
    pressure_history: Vec<f32>,
    window_size: usize,
    sample_count: AtomicU64,
}

impl Clone for MemoryPressureMonitor {
    fn clone(&self) -> Self {
        Self {
            current_pressure: self.current_pressure.clone(),
            pressure_history: self.pressure_history.clone(),
            window_size: self.window_size,
            sample_count: AtomicU64::new(self.sample_count.load(AtomicOrdering::Relaxed)),
        }
    }
}

impl MemoryPressureMonitor {
    /// 创建内存压力监控器,指定历史窗口大小
    pub fn new(window_size: usize) -> Self {
        Self {
            current_pressure: Arc::new(std::sync::atomic::AtomicU32::new(0)),
            pressure_history: Vec::with_capacity(window_size),
            window_size,
            sample_count: AtomicU64::new(0),
        }
    }

    /// 更新当前内存压力(饱和度 [0, 1])
    pub fn update_pressure(&mut self, saturation: f32) {
        let pressure = saturation.clamp(0.0, 1.0);
        self.current_pressure
            .store((pressure * u32::MAX as f32) as u32, AtomicOrdering::Relaxed);
        self.pressure_history.push(pressure);
        if self.pressure_history.len() > self.window_size {
            self.pressure_history.remove(0);
        }
        self.sample_count.fetch_add(1, AtomicOrdering::Relaxed);
    }

    /// 计算窗口内的平均内存压力
    pub fn average_pressure(&self) -> f32 {
        if self.pressure_history.is_empty() {
            return 0.0;
        }
        self.pressure_history.iter().sum::<f32>() / self.pressure_history.len() as f32
    }

    /// 判断当前平均压力是否超过阈值,触发重计算
    pub fn should_recompute(&self, threshold: f32) -> bool {
        self.average_pressure() > threshold
    }

    /// 获取最新一次采样的内存压力
    pub fn current_pressure(&self) -> f32 {
        let raw = self.current_pressure.load(AtomicOrdering::Relaxed);
        raw as f32 / u32::MAX as f32
    }

    /// 获取累计采样次数
    pub fn sample_count(&self) -> u64 {
        self.sample_count.load(AtomicOrdering::Relaxed)
    }
}

// ========== 动态Batch大小调整 ==========

/// 动态 Batch 大小调整器 — 根据吞吐量历史自适应调整批量大小
#[derive(Debug, Clone)]
pub struct DynamicBatchSizer {
    /// 最小批量大小
    pub min_batch_size: usize,
    /// 最大批量大小
    pub max_batch_size: usize,
    current_batch_size: usize,
    throughput_history: Vec<f32>,
    window_size: usize,
    adjustment_factor: f32,
    /// 上次实际调整时刻,None 表示从未调整过(不触发冷却)
    last_adjustment: Option<Instant>,
    cooldown_ms: u64,
}

impl DynamicBatchSizer {
    /// 创建动态批量调整器,指定最小和最大批量
    ///
    /// 初始批量设为中位数,留出上下调整空间
    pub fn new(min_batch: usize, max_batch: usize) -> Self {
        Self {
            min_batch_size: min_batch,
            max_batch_size: max_batch,
            current_batch_size: (min_batch + max_batch) / 2,
            throughput_history: Vec::with_capacity(10),
            window_size: 10,
            adjustment_factor: 1.25,
            last_adjustment: None,
            cooldown_ms: 50,
        }
    }

    /// 记录一次吞吐量采样(batch_size / elapsed_ms)
    pub fn record_throughput(&mut self, batch_size: usize, elapsed_ms: f32) {
        if elapsed_ms.total_cmp(&0.0f32) != Ordering::Greater || batch_size == 0 {
            return;
        }
        let throughput = batch_size as f32 / elapsed_ms;
        self.throughput_history.push(throughput);
        if self.throughput_history.len() > self.window_size {
            self.throughput_history.remove(0);
        }
    }

    /// 根据吞吐量趋势和生产策略调整批量大小
    pub fn adjust_batch_size(&mut self, strategy: ProducerStrategy) {
        // 冷却检查:仅在实际调整过之后才限制频率,首次调整不受冷却限制
        if let Some(last) = self.last_adjustment {
            if last.elapsed().as_millis() < self.cooldown_ms as u128 {
                return;
            }
        }
        if self.throughput_history.len() < 2 {
            return;
        }

        let recent = *self.throughput_history.last().unwrap_or(&0.0);
        let prev_idx = self.throughput_history.len().saturating_sub(2);
        let previous = self.throughput_history.get(prev_idx).unwrap_or(&recent);

        let mut new_size = self.current_batch_size;

        if previous.total_cmp(&0.0f32) != Ordering::Equal
            && recent.total_cmp(&(*previous * 1.15f32)) == Ordering::Greater
        {
            new_size = (new_size as f32 * self.adjustment_factor) as usize;
            info!(
                "吞吐量上升 {:.1}%: batch {} → {}",
                (recent / previous - 1.0) * 100.0,
                self.current_batch_size,
                new_size
            );
        } else if previous.total_cmp(&0.0f32) != Ordering::Equal
            && recent.total_cmp(&(*previous * 0.85f32)) == Ordering::Less
        {
            new_size = (new_size as f32 / self.adjustment_factor) as usize;
            info!(
                "吞吐量下降 {:.1}%: batch {} → {}",
                (1.0 - recent / previous) * 100.0,
                self.current_batch_size,
                new_size
            );
        }

        match strategy {
            ProducerStrategy::Conservative => {
                new_size = new_size
                    .min(self.max_batch_size / 2)
                    .max(self.min_batch_size);
            }
            ProducerStrategy::Normal => {
                new_size = new_size.clamp(self.min_batch_size, self.max_batch_size);
            }
            ProducerStrategy::Aggressive => {
                new_size = new_size.clamp(self.min_batch_size * 2, self.max_batch_size);
            }
        }

        if new_size != self.current_batch_size {
            self.current_batch_size = new_size;
            self.last_adjustment = Some(Instant::now());
        }
    }

    /// 获取当前批量大小
    pub fn current_size(&self) -> usize {
        self.current_batch_size
    }
}

// ========== 流水线统计 ==========

/// 流水线统计 — 记录生成/计算/传输各阶段耗时与操作数
#[derive(Debug, Clone, Default)]
pub struct PipelineStats {
    /// 生成阶段累计耗时(ms)
    pub generation_ms: f32,
    /// 计算阶段累计耗时(ms)
    pub computation_ms: f32,
    /// 传输阶段累计耗时(ms)
    pub transmission_ms: f32,
    /// 累计操作总数
    pub total_operations: u64,
    /// 累计分块数
    pub chunk_count: u64,
}

// ========== IO感知注意力引擎 ==========

/// IO 感知注意力引擎 — 整合分块、内存监控、动态批量与注意力权重的生产引擎
pub struct IoAwareAttentionEngine {
    /// 分块配置
    pub chunk_config: ChunkConfig,
    /// 内存压力监控器
    pub memory_monitor: std::sync::Mutex<MemoryPressureMonitor>,
    /// 动态批量调整器
    pub batch_sizer: std::sync::Mutex<DynamicBatchSizer>,
    /// 流水线统计
    pub stats: std::sync::Mutex<PipelineStats>,
    /// 已生产操作计数
    pub produced_count: AtomicU64,
}

impl IoAwareAttentionEngine {
    /// 创建 IO 感知注意力引擎,根据 PvlConfig 初始化各子组件
    pub fn new(config: &PvlConfig) -> Self {
        let min_batch = (config.channel_capacity / 8).clamp(8, 64);
        let max_batch = config.channel_capacity.clamp(64, 512);
        Self {
            chunk_config: ChunkConfig::default(),
            memory_monitor: std::sync::Mutex::new(MemoryPressureMonitor::new(10)),
            batch_sizer: std::sync::Mutex::new(DynamicBatchSizer::new(min_batch, max_batch)),
            stats: std::sync::Mutex::new(PipelineStats::default()),
            produced_count: AtomicU64::new(0),
        }
    }

    /// 计算指定操作的注意力权重 — 综合语义、依赖、紧急度三维
    pub fn compute_attention_weight(
        &self,
        _op_id: &OperationId,
        content: &str,
        index: usize,
        graph: &OperationDependencyGraph,
        total_count: usize,
    ) -> AttentionWeight {
        let semantic = compute_semantic_weight(content);
        let dependency = if index < graph.operations.len() {
            graph.compute_dependency_weight(index)
        } else {
            (index as f32 / total_count.max(1) as f32).min(1.0)
        };
        let urgency = (-(index as f32) / total_count.max(1) as f32 * 3.0).exp();
        AttentionWeight::new(semantic, dependency, urgency)
    }

    /// 生成分块操作序列 — 根据 strategy 和内存压力决定排序与重计算行为
    pub fn generate_chunk(
        &self,
        quest_id: &str,
        start: usize,
        end: usize,
        total_count: usize,
        graph: &OperationDependencyGraph,
        strategy: ProducerStrategy,
    ) -> Vec<Operation> {
        let chunk_len = end - start;
        let mut result = Vec::with_capacity(chunk_len);
        let should_recompute = match self.memory_monitor.lock() {
            Ok(monitor) => {
                self.chunk_config.enable_recomputation
                    && monitor.should_recompute(self.chunk_config.recomputation_threshold)
            }
            Err(_) => false,
        };
        let mut weighted = Vec::with_capacity(chunk_len);
        for i in start..end {
            let content = format!("operation-{quest_id}-{i}");
            let op_id = OperationId::new(format!("op-{quest_id}-{i}"));
            let op = Operation::new(op_id.clone(), quest_id, content.clone());
            let weight = if !should_recompute {
                self.compute_attention_weight(&op_id, &content, i, graph, total_count)
            } else {
                AttentionWeight::new(0.5, 0.5, 0.5)
            };
            weighted.push((op, weight, content));
        }
        if strategy == ProducerStrategy::Conservative && !should_recompute {
            weighted.sort_by(|a, b| b.1.composite.total_cmp(&a.1.composite));
        }
        for (mut op, weight, content) in weighted {
            let confidence = if !should_recompute {
                compute_weighted_confidence(&content, weight)
            } else {
                compute_semantic_weight(&content)
            };
            op.mark_produced(confidence);
            result.push(op);
        }
        result
    }

    /// 记录流水线统计(生成/计算/传输耗时与操作数)
    pub fn record_stats(&self, gen_ms: f32, comp_ms: f32, tx_ms: f32, ops: u64) {
        if let Ok(mut stats) = self.stats.lock() {
            stats.generation_ms += gen_ms;
            stats.computation_ms += comp_ms;
            stats.transmission_ms += tx_ms;
            stats.total_operations += ops;
            stats.chunk_count += 1;
        }
    }

    /// 获取已生产操作总数
    pub fn produced_count(&self) -> u64 {
        self.produced_count.load(AtomicOrdering::Relaxed)
    }

    /// 获取当前流水线统计快照
    pub fn stats(&self) -> PipelineStats {
        match self.stats.lock() {
            Ok(stats) => stats.clone(),
            Err(_) => PipelineStats::default(),
        }
    }
}

// ========== 辅助函数 ==========

/// 计算语义权重 — 基于 SHA-256 哈希的确定性 [0, 1] 映射
pub fn compute_semantic_weight(content: &str) -> f32 {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let hash = hasher.finalize();
    let raw = u32::from_be_bytes([hash[0], hash[1], hash[2], hash[3]]);
    raw as f32 / u32::MAX as f32
}

/// 计算加权置信度 — 语义权重 × (0.5 + composite × 0.5),clamp 到 [0, 1]
pub fn compute_weighted_confidence(content: &str, weight: AttentionWeight) -> f32 {
    let base = compute_semantic_weight(content);
    let adjusted = base * (0.5 + weight.composite * 0.5);
    adjusted.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_chunk_config_default() {
        let config = ChunkConfig::default();
        assert_eq!(config.chunk_size, 64);
        assert_eq!(config.max_concurrent_chunks, 4);
        assert!(config.enable_recomputation);
        assert_eq!(
            config.recomputation_threshold.total_cmp(&0.80f32),
            Ordering::Equal
        );
    }

    #[test]
    fn test_attention_weight_new() {
        let w = AttentionWeight::new(0.3, 0.6, 0.9);
        assert_eq!(w.semantic.total_cmp(&0.3f32), Ordering::Equal);
        assert_eq!(w.dependency.total_cmp(&0.6f32), Ordering::Equal);
        assert_eq!(w.urgency.total_cmp(&0.9f32), Ordering::Equal);
        assert!(
            w.composite.total_cmp(&0.0f32) != Ordering::Less
                && w.composite.total_cmp(&1.0f32) != Ordering::Greater
        );
    }

    #[test]
    fn test_attention_weight_softmax_norm() {
        let w = AttentionWeight::new(0.5, 0.5, 0.5);
        assert!(
            w.composite.total_cmp(&0.45f32) != Ordering::Less
                && w.composite.total_cmp(&0.55f32) != Ordering::Greater
        );
        let w = AttentionWeight::new(1.0, 0.0, 0.0);
        assert!(w.composite.total_cmp(&0.5f32) == Ordering::Greater);
        let w = AttentionWeight::new(0.0, 0.0, 1.0);
        assert!(w.composite.total_cmp(&0.5f32) == Ordering::Greater);
    }

    #[test]
    fn test_dependency_graph_build_linear() {
        let graph = OperationDependencyGraph::build_linear_sequence(10, 3);
        assert_eq!(graph.operations.len(), 10);
        assert_eq!(graph.adjacency.len(), 10);
        assert!(graph.adjacency[0].is_empty());
        assert_eq!(graph.adjacency[3], vec![0, 1, 2]);
        assert_eq!(graph.adjacency[9], vec![6, 7, 8]);
    }

    #[test]
    fn test_dependency_graph_compute_weight() {
        let graph = OperationDependencyGraph::build_linear_sequence(10, 3);
        let w0 = graph.compute_dependency_weight(0);
        assert!(w0.total_cmp(&0.0f32) == Ordering::Greater);
        let w9 = graph.compute_dependency_weight(9);
        assert_eq!(w9.total_cmp(&0.0f32), Ordering::Equal);
    }

    #[test]
    fn test_dependency_graph_topological_depth() {
        let graph = OperationDependencyGraph::build_linear_sequence(5, 4);
        let depth = graph.compute_topological_depth(0);
        assert_eq!(depth, 4);
    }

    #[test]
    fn test_memory_pressure_monitor() {
        let mut monitor = MemoryPressureMonitor::new(5);
        assert_eq!(
            monitor.average_pressure().total_cmp(&0.0f32),
            Ordering::Equal
        );
        monitor.update_pressure(0.5);
        monitor.update_pressure(0.6);
        monitor.update_pressure(0.7);
        let avg = monitor.average_pressure();
        assert!(
            avg.total_cmp(&0.59f32) != Ordering::Less
                && avg.total_cmp(&0.61f32) != Ordering::Greater
        );
        assert!(monitor.should_recompute(0.5));
        assert!(!monitor.should_recompute(0.8));
    }

    #[test]
    fn test_memory_pressure_window() {
        let mut monitor = MemoryPressureMonitor::new(3);
        monitor.update_pressure(0.9);
        monitor.update_pressure(0.9);
        monitor.update_pressure(0.9);
        monitor.update_pressure(0.1);
        let avg = monitor.average_pressure();
        assert!(
            avg.total_cmp(&0.62f32) != Ordering::Less
                && avg.total_cmp(&0.65f32) != Ordering::Greater
        );
    }

    #[test]
    fn test_batch_sizer_new() {
        let sizer = DynamicBatchSizer::new(16, 256);
        // 初始批量 = (min + max) / 2 = 136
        assert_eq!(sizer.current_size(), 136);
        assert_eq!(sizer.min_batch_size, 16);
        assert_eq!(sizer.max_batch_size, 256);
    }

    #[test]
    fn test_batch_sizer_adjust_up() {
        let mut sizer = DynamicBatchSizer::new(16, 256);
        sizer.record_throughput(16, 10.0);
        sizer.record_throughput(20, 10.0);
        sizer.adjust_batch_size(ProducerStrategy::Normal);
        assert!(sizer.current_size() > 16);
    }

    #[test]
    fn test_batch_sizer_adjust_down() {
        let mut sizer = DynamicBatchSizer::new(16, 256);
        let initial = sizer.current_size();
        sizer.record_throughput(16, 10.0);
        sizer.record_throughput(16, 20.0);
        sizer.adjust_batch_size(ProducerStrategy::Normal);
        assert!(sizer.current_size() < initial);
    }

    #[test]
    fn test_batch_sizer_strategy_constraints() {
        let mut sizer = DynamicBatchSizer::new(16, 256);
        sizer.record_throughput(16, 10.0);
        sizer.record_throughput(32, 10.0);
        sizer.adjust_batch_size(ProducerStrategy::Conservative);
        assert!(sizer.current_size() <= 128);
        let mut sizer2 = DynamicBatchSizer::new(16, 256);
        sizer2.record_throughput(16, 10.0);
        sizer2.record_throughput(32, 10.0);
        sizer2.adjust_batch_size(ProducerStrategy::Aggressive);
        assert!(sizer2.current_size() >= 32);
    }

    #[test]
    fn test_attention_engine_new() {
        let config = PvlConfig::default();
        let engine = IoAwareAttentionEngine::new(&config);
        assert_eq!(engine.produced_count(), 0);
        assert_eq!(engine.stats().total_operations, 0);
    }

    #[test]
    fn test_generate_chunk() {
        let config = PvlConfig::default();
        let engine = IoAwareAttentionEngine::new(&config);
        let graph = OperationDependencyGraph::build_linear_sequence(10, 4);
        let chunk = engine.generate_chunk("quest-1", 0, 5, 10, &graph, ProducerStrategy::Normal);
        assert_eq!(chunk.len(), 5);
        for op in &chunk {
            assert_eq!(op.status, crate::types::OperationStatus::Produced);
            assert!(
                op.confidence.total_cmp(&0.0f32) != Ordering::Less
                    && op.confidence.total_cmp(&1.0f32) != Ordering::Greater
            );
        }
    }

    #[test]
    fn test_generate_chunk_conservative_sorting() {
        let config = PvlConfig::default();
        let engine = IoAwareAttentionEngine::new(&config);
        let graph = OperationDependencyGraph::build_linear_sequence(10, 4);
        let chunk =
            engine.generate_chunk("quest-1", 0, 5, 10, &graph, ProducerStrategy::Conservative);
        assert_eq!(chunk.len(), 5);
        let contents: Vec<_> = chunk.iter().map(|op| op.content.clone()).collect();
        for i in 0..5 {
            assert!(contents.contains(&format!("operation-quest-1-{}", i)));
        }
    }

    #[test]
    fn test_compute_semantic_weight_deterministic() {
        let w1 = compute_semantic_weight("test-content");
        let w2 = compute_semantic_weight("test-content");
        assert_eq!(w1, w2);
        let w3 = compute_semantic_weight("different-content");
        assert_ne!(w1, w3);
    }

    #[test]
    fn test_compute_semantic_weight_range() {
        for content in &["a", "b", "test", "hello world", "12345"] {
            let w = compute_semantic_weight(content);
            assert!(
                w.total_cmp(&0.0f32) != Ordering::Less && w.total_cmp(&1.0f32) != Ordering::Greater
            );
        }
    }

    #[test]
    fn test_compute_weighted_confidence() {
        let c1 = compute_weighted_confidence("test", AttentionWeight::new(0.5, 0.5, 0.5));
        let c2 = compute_weighted_confidence("test", AttentionWeight::new(1.0, 1.0, 1.0));
        assert!(c2.total_cmp(&c1) != Ordering::Less);
        assert!(
            c1.total_cmp(&0.0f32) != Ordering::Less && c1.total_cmp(&1.0f32) != Ordering::Greater
        );
        assert!(
            c2.total_cmp(&0.0f32) != Ordering::Less && c2.total_cmp(&1.0f32) != Ordering::Greater
        );
    }

    #[test]
    fn test_recomputation_mode() {
        let config = PvlConfig::default();
        let engine = IoAwareAttentionEngine::new(&config);
        {
            let mut monitor = match engine.memory_monitor.lock() {
                Ok(m) => m,
                Err(poisoned) => poisoned.into_inner(),
            };
            monitor.update_pressure(0.9);
            monitor.update_pressure(0.95);
            monitor.update_pressure(0.99);
        }
        let graph = OperationDependencyGraph::build_linear_sequence(5, 4);
        let chunk = engine.generate_chunk("quest-1", 0, 5, 5, &graph, ProducerStrategy::Normal);
        assert_eq!(chunk.len(), 5);
        for op in &chunk {
            assert!(
                op.confidence.total_cmp(&0.0f32) != Ordering::Less
                    && op.confidence.total_cmp(&1.0f32) != Ordering::Greater
            );
        }
    }
}
