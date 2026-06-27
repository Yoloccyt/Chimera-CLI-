//! 语义块构建器 — 基于工具共现频率聚类生成 SemanticBlock
//!
//! 对应架构层:L6 Router
//!
//! # 核心算法
//! 基于共现频率的图连通分量聚类:
//! 1. 构建图:节点 = 工具,边 = 共现频率 > 阈值的工具对
//! 2. 使用 Union-Find(并查集)合并连通的工具
//! 3. 每个连通分量形成一个 SemanticBlock
//! 4. 块向量 = 块内工具向量的加权平均(权重 = usage_count)
//! 5. 块一致性 = 块内工具向量与块向量的平均余弦相似度
//!
//! # 设计决策(WHY)
//! - **Union-Find 而非 K-Means**:共现频率是二元关系(有/无共现),
//!   适合图聚类而非距离聚类。Union-Find O(N²) 复杂度,300 工具 < 1ms
//! - **加权平均而非简单平均**:高频工具(usage_count 大)代表块的核心能力,
//!   应在块向量中占主导,使一级路由更精准
//! - **块一致性作为质量指标**:coherence < 0.3 的块可能聚类错误,
//!   重平衡时可作为拆分信号(未来扩展)
//!
//! # 架构红线
//! - 单函数 ≤ 200 行,禁止 unwrap()/expect()
//! - 纯函数无副作用,可独立测试

use uuid::Uuid;

use crate::config::KvbsrConfig;
use crate::types::{CoOccurrenceMatrix, SemanticBlock, ToolId, ToolVector};

/// 块构建器 — 从工具向量与共现矩阵生成语义块
///
/// 基于共现频率 > 阈值的工具对构建连通图,使用 Union-Find 聚类。
/// 块向量 = 块内工具向量的加权平均(权重 = usage_count)。
///
/// # CLV 维度对齐(SubTask 14.8)
/// 块向量维度由 `KvbsrConfig::block_vector_dim`(默认 64)决定,是 CLV 512-dim 的
/// 降维投影。当前工具向量由外部以 64-dim 提供(与块向量维度一致),路由时 CLV
/// 通过截取前 64 维对齐(见 `router::clv_to_block_dim`)。Week 6 NMC 编码器实现后,
/// 将接入 PCA 降维统一处理 CLV→64-dim 的投影,届时工具向量也将由 NMC 统一编码。
///
/// # 示例
/// ```
/// use kvbsr_router::{BlockBuilder, ToolVector, CoOccurrenceMatrix, KvbsrConfig};
///
/// let tools = vec![
///     ToolVector::new("t1", vec![1.0; 64], 100),
///     ToolVector::new("t2", vec![1.0; 64], 100),
/// ];
/// let mut co = CoOccurrenceMatrix::new();
/// co.insert("t1", "t2", 150); // 共现 150 次 > 阈值 100
/// let builder = BlockBuilder::new(KvbsrConfig::default());
/// let blocks = builder.build_blocks(tools, &co);
/// assert_eq!(blocks.len(), 1); // t1, t2 归入同一块
/// ```
pub struct BlockBuilder {
    /// 配置(提供 co_occurrence_threshold 与 block_vector_dim)
    config: KvbsrConfig,
}

impl BlockBuilder {
    /// 创建块构建器,使用指定配置
    pub fn new(config: KvbsrConfig) -> Self {
        Self { config }
    }

    /// 获取配置引用
    pub fn config(&self) -> &KvbsrConfig {
        &self.config
    }

    /// 构建语义块 — 基于共现频率聚类
    ///
    /// 流程:
    /// 1. 构建 tool_id → index 映射(加速共现矩阵查找)
    /// 2. 初始化 Union-Find,每个工具自成一簇
    /// 3. 遍历共现矩阵,合并共现频率 > 阈值的工具对
    /// 4. 按根节点分组,每组构建一个 SemanticBlock
    /// 5. 块向量 = 加权平均,块一致性 = 平均余弦相似度
    ///
    /// # 参数
    /// - `tools`:工具向量列表(将按聚类结果分组)
    /// - `co_occurrence`:工具共现矩阵
    ///
    /// # 返回
    /// 语义块列表。共现频率 ≤ 阈值或无共现记录的工具各自独立成块。
    pub fn build_blocks(
        &self,
        tools: Vec<ToolVector>,
        co_occurrence: &CoOccurrenceMatrix,
    ) -> Vec<SemanticBlock> {
        if tools.is_empty() {
            return Vec::new();
        }

        let n = tools.len();

        // 1. 构建 tool_id → index 映射
        let mut id_to_index: std::collections::HashMap<&ToolId, usize> =
            std::collections::HashMap::with_capacity(n);
        for (i, t) in tools.iter().enumerate() {
            id_to_index.insert(&t.tool_id, i);
        }

        // 2. 初始化 Union-Find
        let mut uf = UnionFind::new(n);

        // 3. 遍历共现矩阵,合并共现频率 > 阈值的工具对
        // SubTask 13.11:CoOccurrenceMatrix 内部改用 u32 索引,通过 iter_pairs() 解析回 ToolId
        let threshold = self.config.co_occurrence_threshold;
        for (a, b, count) in co_occurrence.iter_pairs() {
            if count > threshold {
                // 两个工具都必须在当前工具列表中
                if let (Some(&ia), Some(&ib)) = (id_to_index.get(a), id_to_index.get(b)) {
                    uf.union(ia, ib);
                }
            }
        }

        // 4. 按根节点分组
        let mut groups: std::collections::HashMap<usize, Vec<ToolVector>> =
            std::collections::HashMap::new();
        for (i, tool) in tools.into_iter().enumerate() {
            let root = uf.find(i);
            groups.entry(root).or_default().push(tool);
        }

        // 5. 每组构建一个 SemanticBlock
        groups
            .into_values()
            .map(|group| self.build_single_block(group))
            .collect()
    }

    /// 从一组工具向量构建单个语义块
    ///
    /// 块向量 = 工具向量的加权平均(权重 = usage_count)
    /// 块一致性 = 块内工具向量与块向量的平均余弦相似度
    fn build_single_block(&self, tools: Vec<ToolVector>) -> SemanticBlock {
        debug_assert!(
            !tools.is_empty(),
            "build_single_block 不应接收空工具列表(调用方保证)"
        );

        let block_vector = self.weighted_average(&tools);
        let block_coherence = self.compute_coherence(&tools, &block_vector);
        let tool_ids: Vec<ToolId> = tools.iter().map(|t| t.tool_id.clone()).collect();

        SemanticBlock {
            block_id: Uuid::now_v7().to_string(),
            block_vector,
            tools: tool_ids,
            block_coherence,
        }
    }

    /// 计算工具向量的加权平均
    ///
    /// 权重 = usage_count。若所有 usage_count 为 0,使用均匀权重。
    /// WHY:高频工具代表块的核心能力,应在块向量中占主导
    fn weighted_average(&self, tools: &[ToolVector]) -> Vec<f32> {
        let dim = self.config.block_vector_dim;
        let mut result = vec![0.0_f32; dim];
        let mut total_weight: f32 = 0.0;

        for tool in tools {
            // 向量维度可能与配置维度不一致(外部输入),取较小值避免越界
            let len = tool.vector.len().min(dim);
            let weight = if tool.usage_count > 0 {
                tool.usage_count as f32
            } else {
                1.0 // 均匀权重兜底
            };
            for (i, &v) in tool.vector[..len].iter().enumerate() {
                result[i] += v * weight;
            }
            total_weight += weight;
        }

        // 归一化(除以总权重)
        if total_weight > 0.0 {
            for v in &mut result {
                *v /= total_weight;
            }
        }
        result
    }

    /// 计算块内一致性 — 块内工具向量与块向量的平均余弦相似度
    ///
    /// 返回 [0.0, 1.0]:
    /// - ≈ 1.0:块内工具高度相似,聚类紧凑
    /// - ≈ 0.0:块内工具差异大,聚类松散
    fn compute_coherence(&self, tools: &[ToolVector], block_vector: &[f32]) -> f32 {
        if tools.is_empty() || block_vector.is_empty() {
            return 0.0;
        }
        let dim = block_vector.len();
        let mut sum: f32 = 0.0;
        let mut count: usize = 0;
        for tool in tools {
            let len = tool.vector.len().min(dim);
            if len == 0 {
                continue;
            }
            // SubTask 21.4:使用 nexus_core 统一的 cosine_similarity_slices
            let sim =
                nexus_core::cosine_similarity_slices(&tool.vector[..len], &block_vector[..len]);
            sum += sim;
            count += 1;
        }
        if count == 0 {
            0.0
        } else {
            // 余弦相似度可能为负,钳制到 [0.0, 1.0]
            (sum / count as f32).clamp(0.0, 1.0)
        }
    }
}

/// Union-Find(并查集)— 带路径压缩与按秩合并
///
/// WHY:聚类需要高效的连通性判断与合并。Union-Find 均摊 O(α(N)) ≈ O(1),
/// 300 工具的全量合并 < 1ms。路径压缩与按秩合并确保最优复杂度。
struct UnionFind {
    /// parent[i] = i 的父节点(根节点的 parent 为自身)
    parent: Vec<usize>,
    /// rank[i] = 以 i 为根的树的高度(按秩合并用)
    rank: Vec<usize>,
}

impl UnionFind {
    /// 创建 n 个独立元素(每个自成一簇)
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
            rank: vec![0; n],
        }
    }

    /// 查找 i 的根节点(带路径压缩)
    fn find(&mut self, mut i: usize) -> usize {
        while self.parent[i] != i {
            // 路径压缩:将 i 的父节点直接指向祖父节点
            self.parent[i] = self.parent[self.parent[i]];
            i = self.parent[i];
        }
        i
    }

    /// 合并 i 和 j 所在的簇(按秩合并)
    fn union(&mut self, i: usize, j: usize) {
        let ri = self.find(i);
        let rj = self.find(j);
        if ri == rj {
            return; // 已在同一簇
        }
        // 按秩合并:小树挂到大树下,保持树平衡
        match self.rank[ri].cmp(&self.rank[rj]) {
            std::cmp::Ordering::Less => {
                self.parent[ri] = rj;
            }
            std::cmp::Ordering::Greater => {
                self.parent[rj] = ri;
            }
            std::cmp::Ordering::Equal => {
                self.parent[rj] = ri;
                self.rank[ri] += 1;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_union_find_basic() {
        let mut uf = UnionFind::new(5);
        assert_eq!(uf.find(0), 0);
        assert_eq!(uf.find(1), 1);
        uf.union(0, 1);
        assert_eq!(uf.find(0), uf.find(1));
        uf.union(2, 3);
        uf.union(1, 2);
        // 0-1-2-3 连通
        assert_eq!(uf.find(0), uf.find(3));
        // 4 独立
        assert_ne!(uf.find(0), uf.find(4));
    }

    #[test]
    fn test_cosine_similarity_identical() {
        // SubTask 21.4:使用 nexus_core 统一的 cosine_similarity_slices
        let v = vec![1.0, 2.0, 3.0];
        let sim = nexus_core::cosine_similarity_slices(&v, &v);
        assert!((sim - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        let sim = nexus_core::cosine_similarity_slices(&a, &b);
        assert!(sim.abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_zero_vector() {
        let a = vec![0.0, 0.0];
        let b = vec![1.0, 2.0];
        let sim = nexus_core::cosine_similarity_slices(&a, &b);
        assert_eq!(sim, 0.0);
    }

    #[test]
    fn test_build_blocks_empty_tools() {
        let builder = BlockBuilder::new(KvbsrConfig::default());
        let blocks = builder.build_blocks(Vec::new(), &CoOccurrenceMatrix::new());
        assert!(blocks.is_empty());
    }

    #[test]
    fn test_build_blocks_no_co_occurrence() {
        // 无共现记录,每个工具独立成块
        let tools = vec![
            ToolVector::new("t1", vec![1.0; 64], 100),
            ToolVector::new("t2", vec![1.0; 64], 100),
        ];
        let builder = BlockBuilder::new(KvbsrConfig::default());
        let blocks = builder.build_blocks(tools, &CoOccurrenceMatrix::new());
        assert_eq!(blocks.len(), 2);
    }

    #[test]
    fn test_build_blocks_below_threshold() {
        // 共现频率 = 阈值(100),不满足 > 阈值,不合并
        let tools = vec![
            ToolVector::new("t1", vec![1.0; 64], 100),
            ToolVector::new("t2", vec![1.0; 64], 100),
        ];
        let mut co = CoOccurrenceMatrix::new();
        co.insert("t1", "t2", 100); // = 阈值,不满足 > 100
        let builder = BlockBuilder::new(KvbsrConfig::default());
        let blocks = builder.build_blocks(tools, &co);
        assert_eq!(blocks.len(), 2); // 未合并
    }

    #[test]
    fn test_build_blocks_above_threshold() {
        // 共现频率 > 阈值,合并为同一块
        let tools = vec![
            ToolVector::new("t1", vec![1.0; 64], 100),
            ToolVector::new("t2", vec![1.0; 64], 100),
        ];
        let mut co = CoOccurrenceMatrix::new();
        co.insert("t1", "t2", 150); // > 100
        let builder = BlockBuilder::new(KvbsrConfig::default());
        let blocks = builder.build_blocks(tools, &co);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].tool_count(), 2);
        assert_eq!(blocks[0].dimension(), 64);
        // 相同向量加权平均后仍与原向量相同,一致性 = 1.0
        assert!((blocks[0].block_coherence - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_build_blocks_weighted_average() {
        // t1 usage=100, t2 usage=300,块向量应偏向 t2
        let tools = vec![
            ToolVector::new("t1", vec![1.0, 0.0], 100),
            ToolVector::new("t2", vec![0.0, 1.0], 300),
        ];
        let mut co = CoOccurrenceMatrix::new();
        co.insert("t1", "t2", 150);
        let builder = BlockBuilder::new(KvbsrConfig::default());
        let blocks = builder.build_blocks(tools, &co);
        assert_eq!(blocks.len(), 1);
        // 加权平均:(1*100 + 0*300)/400 = 0.25, (0*100 + 1*300)/400 = 0.75
        assert!((blocks[0].block_vector[0] - 0.25).abs() < 1e-5);
        assert!((blocks[0].block_vector[1] - 0.75).abs() < 1e-5);
    }

    #[test]
    fn test_build_blocks_transitive_merge() {
        // t1-t2 共现,t2-t3 共现,三者应合并为同一块
        let tools = vec![
            ToolVector::new("t1", vec![1.0; 64], 100),
            ToolVector::new("t2", vec![1.0; 64], 100),
            ToolVector::new("t3", vec![1.0; 64], 100),
        ];
        let mut co = CoOccurrenceMatrix::new();
        co.insert("t1", "t2", 150);
        co.insert("t2", "t3", 150);
        // t1-t3 无直接共现,但通过 t2 传递合并
        let builder = BlockBuilder::new(KvbsrConfig::default());
        let blocks = builder.build_blocks(tools, &co);
        assert_eq!(blocks.len(), 1);
        assert_eq!(blocks[0].tool_count(), 3);
    }

    #[test]
    fn test_build_blocks_block_id_is_uuidv7() {
        let tools = vec![ToolVector::new("t1", vec![1.0; 64], 100)];
        let builder = BlockBuilder::new(KvbsrConfig::default());
        let blocks = builder.build_blocks(tools, &CoOccurrenceMatrix::new());
        assert_eq!(blocks.len(), 1);
        // UUIDv7 字符串长度 36(含连字符)
        assert_eq!(blocks[0].block_id.len(), 36);
        // 可解析为 Uuid
        assert!(Uuid::parse_str(&blocks[0].block_id).is_ok());
    }
}
