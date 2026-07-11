//! 优先级调度器 — 基于注意力权重的操作调度
//!
//! 参考FlashAttention的选择性关注机制，高权重操作优先处理。
//!
//! ## 核心概念映射
//! | FlashAttention 概念 | PVL 生产者映射 |
//! |---|---|
//! | Online Softmax | 注意力权重调度 (Attention Scheduling) |
//! | Attention Head | 多维权重计算 |

use sha2::{Digest, Sha256};

use crate::config::PvlConfig;
use crate::types::{Operation, OperationId, ProducerStrategy};

/// 操作注意力权重 — 用于优先级调度的多维评分
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AttentionWeight {
    /// 语义权重
    pub semantic: f32,
    /// 依赖权重
    pub dependency: f32,
    /// 紧急权重
    pub urgency: f32,
    /// 综合权重（softmax归一化）
    pub composite: f32,
}

impl AttentionWeight {
    /// 创建新的注意力权重
    ///
    /// 所有分量被限制在 [0.0, 1.0] 范围内，
    /// 综合权重通过softmax归一化计算。
    pub fn new(semantic: f32, dependency: f32, urgency: f32) -> Self {
        let composite = Self::softmax_norm(semantic, dependency, urgency);
        Self {
            semantic: semantic.clamp(0.0f32, 1.0f32),
            dependency: dependency.clamp(0.0f32, 1.0f32),
            urgency: urgency.clamp(0.0f32, 1.0f32),
            composite,
        }
    }

    /// 创建均衡权重（重计算模式使用）
    pub fn balanced() -> Self {
        Self::new(0.5f32, 0.5f32, 0.5f32)
    }

    /// softmax归一化
    ///
    /// 使用数值稳定的softmax计算，避免指数溢出。
    fn softmax_norm(a: f32, b: f32, c: f32) -> f32 {
        let max_val = a.max(b).max(c);
        let exp_a = ((a - max_val) * 5.0f32).exp();
        let exp_b = ((b - max_val) * 5.0f32).exp();
        let exp_c = ((c - max_val) * 5.0f32).exp();
        let sum = exp_a + exp_b + exp_c;
        (a * exp_a + b * exp_b + c * exp_c) / sum
    }
}

/// 操作依赖图
///
/// 使用轻量级DAG表示操作依赖关系。
/// 支持线性序列依赖图（滑动窗口）构建。
#[derive(Debug, Clone, Default)]
pub struct OperationDependencyGraph {
    /// 操作ID列表
    pub operations: Vec<OperationId>,
    /// 邻接表（表示依赖关系）
    pub adjacency: Vec<Vec<usize>>,
}

impl OperationDependencyGraph {
    /// 创建新的空依赖图
    pub fn new() -> Self {
        Self::default()
    }

    /// 构建线性序列依赖图
    ///
    /// 每个操作依赖于前 `window_size` 个操作。
    ///
    /// # 参数
    /// - `count`: 操作总数
    /// - `window_size`: 滑动窗口大小
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

    /// 计算指定操作的依赖权重
    ///
    /// 基于下游依赖数量计算权重。
    /// 被更多操作依赖的操作具有更高的权重。
    pub fn compute_dependency_weight(&self, index: usize) -> f32 {
        if self.operations.is_empty() {
            return 0.5f32;
        }
        let downstream_count = self
            .adjacency
            .iter()
            .filter(|deps| deps.contains(&index))
            .count();
        let total = self.operations.len().max(1);
        (downstream_count as f32 / total as f32).min(1.0f32)
    }

    /// 计算拓扑深度
    ///
    /// 从指定操作出发，计算可达的最大深度。
    ///
    /// WHY 使用 best_depth 而非 visited: longest path 计算需要允许节点
    /// 经由更长路径被重新访问,简单 visited 会阻断更优解
    pub fn compute_topological_depth(&self, index: usize) -> usize {
        let mut best_depth = vec![0usize; self.operations.len()];
        let mut stack = vec![(index, 0usize)];
        let mut max_depth = 0usize;
        while let Some((current, current_depth)) = stack.pop() {
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

/// 优先级调度器 — 基于注意力权重的操作调度
///
/// 使用轻量级DAG表示操作依赖，拓扑排序+注意力权重混合调度。
/// 仅在操作数超过阈值时启用，避免小批量overhead。
pub struct PriorityScheduler {
    /// 启用优先级调度的操作数阈值
    threshold: usize,
    /// 滑动窗口大小（依赖图构建用）
    window_size: usize,
}

impl PriorityScheduler {
    /// 创建新的优先级调度器
    ///
    /// # 参数
    /// - `config`: 执行配置
    pub fn new(config: &PvlConfig) -> Self {
        Self {
            threshold: config.priority_threshold,
            window_size: 4,
        }
    }

    /// 获取阈值
    pub fn threshold(&self) -> usize {
        self.threshold
    }

    /// 判断是否需要启用优先级调度
    ///
    /// 当操作数超过阈值时返回true。
    pub fn should_enable(&self, count: usize) -> bool {
        count > self.threshold
    }

    /// 构建依赖图
    ///
    /// # 参数
    /// - `count`: 操作总数
    pub fn build_dependency_graph(&self, count: usize) -> OperationDependencyGraph {
        OperationDependencyGraph::build_linear_sequence(count, self.window_size)
    }

    /// 计算注意力权重
    ///
    /// # 参数
    /// - `content`: 操作内容
    /// - `index`: 操作索引
    /// - `total_count`: 总操作数
    /// - `graph`: 依赖图
    pub fn compute_attention_weight(
        &self,
        content: &str,
        index: usize,
        total_count: usize,
        graph: &OperationDependencyGraph,
    ) -> AttentionWeight {
        let semantic = compute_semantic_weight(content);
        let dependency = if index < graph.operations.len() {
            graph.compute_dependency_weight(index)
        } else {
            let ratio = index as f32 / total_count.max(1) as f32;
            ratio.min(1.0f32)
        };
        let urgency = (-(index as f32) / total_count.max(1) as f32 * 3.0f32).exp();
        AttentionWeight::new(semantic, dependency, urgency)
    }

    /// 计算加权置信度
    ///
    /// # 参数
    /// - `content`: 操作内容
    /// - `weight`: 注意力权重
    pub fn compute_confidence(&self, content: &str, weight: AttentionWeight) -> f32 {
        let base = compute_semantic_weight(content);
        let adjusted = base * (0.5f32 + weight.composite * 0.5f32);
        adjusted.clamp(0.0f32, 1.0f32)
    }

    /// 对分块应用优先级调度
    ///
    /// 计算注意力权重、设置置信度、按策略排序。
    ///
    /// # 参数
    /// - `ops`: 原始操作列表
    /// - `start`: 分块起始索引（在总序列中的位置）
    /// - `total`: 总操作数
    /// - `graph`: 依赖图
    /// - `strategy`: 当前策略
    /// - `should_recompute`: 是否启用重计算模式（简化计算）
    ///
    /// # 返回
    /// 处理后的操作列表（已设置置信度，可能已排序）
    pub fn apply_to_chunk(
        &self,
        ops: Vec<Operation>,
        start: usize,
        total: usize,
        graph: &OperationDependencyGraph,
        strategy: ProducerStrategy,
        should_recompute: bool,
    ) -> Vec<Operation> {
        let mut weighted = Vec::with_capacity(ops.len());

        for (offset, mut op) in ops.into_iter().enumerate() {
            let index = start + offset;
            let weight = if should_recompute {
                AttentionWeight::balanced()
            } else {
                self.compute_attention_weight(&op.content, index, total, graph)
            };

            let confidence = if should_recompute {
                compute_semantic_weight(&op.content)
            } else {
                self.compute_confidence(&op.content, weight)
            };

            op.mark_produced(confidence);
            weighted.push((op, weight));
        }

        // Conservative策略时按权重排序
        if strategy == ProducerStrategy::Conservative && !should_recompute {
            weighted.sort_by(|a, b| b.1.composite.total_cmp(&a.1.composite));
        }

        weighted.into_iter().map(|(op, _)| op).collect()
    }
}

/// 计算内容的语义权重（基于内容哈希）
///
/// 占位实现：基于内容哈希生成确定性权重。
/// sha256 提供良好分布，取前4字节映射到 [0.0, 1.0]。
pub fn compute_semantic_weight(content: &str) -> f32 {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let hash = hasher.finalize();
    let raw = u32::from_be_bytes([hash[0], hash[1], hash[2], hash[3]]);
    raw as f32 / u32::MAX as f32
}

/// 计算加权置信度
pub fn compute_weighted_confidence(content: &str, weight: AttentionWeight) -> f32 {
    let base = compute_semantic_weight(content);
    let adjusted = base * (0.5f32 + weight.composite * 0.5f32);
    adjusted.clamp(0.0f32, 1.0f32)
}

#[cfg(test)]
mod tests {
    use std::cmp::Ordering as CmpOrdering;

    use super::*;

    /// 辅助函数：检查f32值是否在指定范围内
    fn in_range(value: f32, min: f32, max: f32) -> bool {
        value.total_cmp(&min) != CmpOrdering::Less && value.total_cmp(&max) != CmpOrdering::Greater
    }

    #[test]
    fn test_attention_weight_new() {
        let w = AttentionWeight::new(0.3f32, 0.6f32, 0.9f32);
        assert_eq!(w.semantic.total_cmp(&0.3f32), CmpOrdering::Equal);
        assert_eq!(w.dependency.total_cmp(&0.6f32), CmpOrdering::Equal);
        assert_eq!(w.urgency.total_cmp(&0.9f32), CmpOrdering::Equal);
        assert!(in_range(w.composite, 0.0f32, 1.0f32));
    }

    #[test]
    fn test_attention_weight_balanced() {
        let w = AttentionWeight::balanced();
        assert_eq!(w.semantic.total_cmp(&0.5f32), CmpOrdering::Equal);
        assert_eq!(w.dependency.total_cmp(&0.5f32), CmpOrdering::Equal);
        assert_eq!(w.urgency.total_cmp(&0.5f32), CmpOrdering::Equal);
    }

    #[test]
    fn test_attention_weight_softmax_norm() {
        // 均衡输入应输出接近均衡的值
        let w = AttentionWeight::new(0.5f32, 0.5f32, 0.5f32);
        assert!(
            w.composite.total_cmp(&0.45f32) != CmpOrdering::Less
                && w.composite.total_cmp(&0.55f32) != CmpOrdering::Greater
        );

        // 高语义权重应使综合权重偏向语义
        let w = AttentionWeight::new(1.0f32, 0.0f32, 0.0f32);
        assert!(w.composite.total_cmp(&0.5f32) == CmpOrdering::Greater);

        // 高紧急权重应使综合权重偏向紧急
        let w = AttentionWeight::new(0.0f32, 0.0f32, 1.0f32);
        assert!(w.composite.total_cmp(&0.5f32) == CmpOrdering::Greater);
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
        assert!(w0.total_cmp(&0.0f32) == CmpOrdering::Greater);
        let w9 = graph.compute_dependency_weight(9);
        assert_eq!(w9.total_cmp(&0.0f32), CmpOrdering::Equal);
    }

    #[test]
    fn test_dependency_graph_topological_depth() {
        let graph = OperationDependencyGraph::build_linear_sequence(5, 4);
        let depth = graph.compute_topological_depth(0);
        assert_eq!(depth, 4);
    }

    #[test]
    fn test_priority_scheduler_new() {
        let config = PvlConfig::default();
        let scheduler = PriorityScheduler::new(&config);
        assert_eq!(scheduler.threshold(), 32);
    }

    #[test]
    fn test_should_enable() {
        let config = PvlConfig::default();
        let scheduler = PriorityScheduler::new(&config);

        assert!(!scheduler.should_enable(32));
        assert!(scheduler.should_enable(33));
        assert!(scheduler.should_enable(100));
    }

    #[test]
    fn test_compute_attention_weight() {
        let config = PvlConfig::default();
        let scheduler = PriorityScheduler::new(&config);
        let graph = scheduler.build_dependency_graph(10);

        let weight = scheduler.compute_attention_weight("test-content", 5, 10, &graph);
        assert!(in_range(weight.semantic, 0.0f32, 1.0f32));
        assert!(in_range(weight.dependency, 0.0f32, 1.0f32));
        assert!(in_range(weight.urgency, 0.0f32, 1.0f32));
        assert!(in_range(weight.composite, 0.0f32, 1.0f32));
    }

    #[test]
    fn test_compute_confidence() {
        let config = PvlConfig::default();
        let scheduler = PriorityScheduler::new(&config);

        let c1 = scheduler.compute_confidence("test", AttentionWeight::new(0.5f32, 0.5f32, 0.5f32));
        let c2 = scheduler.compute_confidence("test", AttentionWeight::new(1.0f32, 1.0f32, 1.0f32));

        assert!(c2.total_cmp(&c1) != CmpOrdering::Less);
        assert!(in_range(c1, 0.0f32, 1.0f32));
        assert!(in_range(c2, 0.0f32, 1.0f32));
    }

    #[test]
    fn test_apply_to_chunk_conservative_sorting() {
        let config = PvlConfig::default();
        let scheduler = PriorityScheduler::new(&config);
        let graph = scheduler.build_dependency_graph(10);

        let mut ops = Vec::new();
        for i in 0..5 {
            let content = format!("operation-test-{}", i);
            let op = Operation::new(OperationId::new(format!("op-{}", i)), "test", content);
            ops.push(op);
        }

        let result =
            scheduler.apply_to_chunk(ops, 0, 10, &graph, ProducerStrategy::Conservative, false);
        assert_eq!(result.len(), 5);

        // 验证所有操作都被标记为已生产
        for op in &result {
            assert_eq!(op.status, crate::types::OperationStatus::Produced);
            assert!(in_range(op.confidence, 0.0f32, 1.0f32));
        }

        // 验证内容都在（只是顺序可能改变）
        let contents: Vec<_> = result.iter().map(|op| op.content.clone()).collect();
        for i in 0..5 {
            assert!(contents.contains(&format!("operation-test-{}", i)));
        }
    }

    #[test]
    fn test_apply_to_chunk_normal_no_sorting() {
        let config = PvlConfig::default();
        let scheduler = PriorityScheduler::new(&config);
        let graph = scheduler.build_dependency_graph(5);

        let mut ops = Vec::new();
        for i in 0..5 {
            let content = format!("operation-test-{}", i);
            let op = Operation::new(OperationId::new(format!("op-{}", i)), "test", content);
            ops.push(op);
        }

        let result =
            scheduler.apply_to_chunk(ops.clone(), 0, 5, &graph, ProducerStrategy::Normal, false);
        assert_eq!(result.len(), 5);

        // Normal策略下应保持原始顺序
        for (i, op) in result.iter().enumerate() {
            assert_eq!(op.content, format!("operation-test-{}", i));
        }
    }

    #[test]
    fn test_semantic_weight_deterministic() {
        let w1 = compute_semantic_weight("test-content");
        let w2 = compute_semantic_weight("test-content");
        assert_eq!(
            w1.total_cmp(&w2),
            CmpOrdering::Equal,
            "相同内容应产生相同语义权重"
        );

        let w3 = compute_semantic_weight("different-content");
        assert_ne!(
            w1.total_cmp(&w3),
            CmpOrdering::Equal,
            "不同内容应产生不同语义权重"
        );
    }

    #[test]
    fn test_semantic_weight_range() {
        for content in &["a", "b", "test", "hello world", "12345"] {
            let w = compute_semantic_weight(content);
            assert!(
                in_range(w, 0.0f32, 1.0f32),
                "语义权重应在 [0.0, 1.0] 范围内,内容: {},权重: {}",
                content,
                w
            );
        }
    }

    #[test]
    fn test_weighted_confidence() {
        let c1 = compute_weighted_confidence("test", AttentionWeight::new(0.5f32, 0.5f32, 0.5f32));
        let c2 = compute_weighted_confidence("test", AttentionWeight::new(1.0f32, 1.0f32, 1.0f32));
        assert!(c2.total_cmp(&c1) != CmpOrdering::Less);
        assert!(in_range(c1, 0.0f32, 1.0f32));
        assert!(in_range(c2, 0.0f32, 1.0f32));
    }

    #[test]
    fn test_recompute_mode() {
        let config = PvlConfig::default();
        let scheduler = PriorityScheduler::new(&config);
        let graph = scheduler.build_dependency_graph(5);

        let mut ops = Vec::new();
        for i in 0..5 {
            let content = format!("operation-test-{}", i);
            let op = Operation::new(OperationId::new(format!("op-{}", i)), "test", content);
            ops.push(op);
        }

        let result = scheduler.apply_to_chunk(ops, 0, 5, &graph, ProducerStrategy::Normal, true);
        assert_eq!(result.len(), 5);
        for op in &result {
            assert!(in_range(op.confidence, 0.0f32, 1.0f32));
        }
    }
}
