//! 语义块重平衡器 — 分析共现频率并重建语义块
//!
//! 对应架构层:L6 Router
//!
//! # 核心职责
//! - `analyze_co_occurrence`:从使用日志统计工具共现频率,生成新的 CoOccurrenceMatrix
//! - `rebuild_blocks`:基于新共现矩阵重建语义块,委托给 BlockBuilder
//!
//! # 重平衡流程(由 KVBlockSemanticRouter::auto_rebalance 编排)
//! 1. 从使用日志分析共现频率 → 新 CoOccurrenceMatrix
//! 2. 基于新矩阵重建语义块 → `Vec<SemanticBlock>`
//! 3. 原子切换块列表(`Arc<RwLock<Vec<SemanticBlock>>>` 保护)
//! 4. 发布 BlocksRebalanced 事件
//!
//! # 设计决策(WHY)
//! - **Rebalancer 无状态**:仅持有 config,不持有块列表或共现矩阵。
//!   状态由 KVBlockSemanticRouter 管理,Rebalancer 作为纯计算组件,
//!   便于测试与复用
//! - **重建而非增量更新**:共现频率变化可能改变整个聚类结构,
//!   增量更新难以保证一致性。全量重建虽计算成本略高,但保证正确性。
//!   300 工具重建 < 5ms,不影响路由性能(异步执行)
//!
//! # 架构红线
//! - 单函数 ≤ 200 行,禁止 unwrap()/expect()
//! - 纯函数无副作用,可独立测试

use crate::blocks::BlockBuilder;
use crate::config::KvbsrConfig;
use crate::types::{CoOccurrenceMatrix, SemanticBlock, ToolId, ToolVector};

/// 语义块重平衡器 — 分析共现频率并重建语义块
///
/// 无状态组件,仅持有配置。状态管理由 `KVBlockSemanticRouter` 负责。
/// 可跨 async 任务共享(Send + Sync),所有方法满足 Send 约束。
///
/// # 示例
/// ```
/// use kvbsr_router::{Rebalancer, ToolVector, CoOccurrenceMatrix, KvbsrConfig, ToolId};
///
/// let rebalancer = Rebalancer::new(KvbsrConfig::default());
/// let usage_log = vec![(ToolId::new("t1"), ToolId::new("t2")); 150];
/// let co = rebalancer.analyze_co_occurrence(&usage_log);
/// assert_eq!(co.get("t1", "t2"), 150);
/// ```
#[derive(Clone)]
pub struct Rebalancer {
    /// 配置(提供 co_occurrence_threshold 与 block_vector_dim)
    config: KvbsrConfig,
}

impl Rebalancer {
    /// 创建重平衡器,使用指定配置
    pub fn new(config: KvbsrConfig) -> Self {
        Self { config }
    }

    /// 获取配置引用
    pub fn config(&self) -> &KvbsrConfig {
        &self.config
    }

    /// 从使用日志分析共现频率,生成共现矩阵
    ///
    /// 使用日志为工具对列表,每对表示一次共现(如同一任务中先后使用)。
    /// 内部调用 `CoOccurrenceMatrix::from_pairs` 统计每对工具的共现次数。
    ///
    /// # 参数
    /// - `usage_log`:使用日志,每条记录为 (ToolId, ToolId) 工具对
    ///
    /// # 返回
    /// 共现矩阵,Key 为 (小 ToolId, 大 ToolId),Value 为共现次数
    pub fn analyze_co_occurrence(&self, usage_log: &[(ToolId, ToolId)]) -> CoOccurrenceMatrix {
        CoOccurrenceMatrix::from_pairs(usage_log)
    }

    /// 基于共现矩阵重建语义块
    ///
    /// 委托给 `BlockBuilder::build_blocks`,使用当前配置的共现阈值聚类。
    /// 块向量 = 工具向量的加权平均,块一致性 = 平均余弦相似度。
    ///
    /// # 参数
    /// - `tools`:工具向量列表(将按聚类结果分组)
    /// - `co_occurrence`:共现矩阵
    ///
    /// # 返回
    /// 重建后的语义块列表。共现频率 ≤ 阈值的工具各自独立成块。
    pub fn rebuild_blocks(
        &self,
        tools: Vec<ToolVector>,
        co_occurrence: &CoOccurrenceMatrix,
    ) -> Vec<SemanticBlock> {
        BlockBuilder::new(self.config.clone()).build_blocks(tools, co_occurrence)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_analyze_co_occurrence() {
        let rebalancer = Rebalancer::new(KvbsrConfig::default());
        let log = vec![
            (ToolId::new("t1"), ToolId::new("t2")),
            (ToolId::new("t1"), ToolId::new("t2")),
            (ToolId::new("t2"), ToolId::new("t3")),
        ];
        let co = rebalancer.analyze_co_occurrence(&log);
        assert_eq!(co.get("t1", "t2"), 2);
        assert_eq!(co.get("t2", "t3"), 1);
        assert_eq!(co.get("t1", "t3"), 0);
    }

    #[test]
    fn test_rebuild_blocks_merges_by_co_occurrence() {
        let rebalancer = Rebalancer::new(KvbsrConfig::default());
        let tools = vec![
            ToolVector::new("t1", vec![1.0; 64], 100),
            ToolVector::new("t2", vec![1.0; 64], 100),
            ToolVector::new("t3", vec![1.0; 64], 100),
        ];
        let mut co = CoOccurrenceMatrix::new();
        co.insert("t1", "t2", 150);
        co.insert("t2", "t3", 150);
        let blocks = rebalancer.rebuild_blocks(tools, &co);
        // t1-t2-t3 通过传递性合并为同一块
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].tool_count(), 3);
    }

    #[test]
    fn test_rebuild_blocks_no_co_occurrence() {
        let rebalancer = Rebalancer::new(KvbsrConfig::default());
        let tools = vec![
            ToolVector::new("t1", vec![1.0; 64], 100),
            ToolVector::new("t2", vec![1.0; 64], 100),
        ];
        let co = CoOccurrenceMatrix::new();
        let blocks = rebalancer.rebuild_blocks(tools, &co);
        // 无共现,每个工具独立成块
        assert_eq!(blocks.len(), 2);
    }

    #[test]
    fn test_rebalancer_clone() {
        let rebalancer = Rebalancer::new(KvbsrConfig::default());
        let cloned = rebalancer.clone();
        assert_eq!(rebalancer.config(), cloned.config());
    }
}
