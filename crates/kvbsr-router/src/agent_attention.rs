//! Agent Attention 路由引擎 — 基于Agent Token的两阶段交叉注意力机制
//!
//! 对应架构层: L6 Router
//! 对应创新点: Agent Token 注意力路由优化
//!
//! # 核心概念
//! 参考STAGformer (arXiv:2607.06614) 和 Agent Attention (arXiv:2312.08874) 论文，
//! 引入可学习的Agent Token作为语义聚合代理，替代静态余弦相似度。
//!
//! ## 两阶段注意力机制
//! 1. **Query → Agent Token 交叉注意力** (聚合阶段):
//!    - 查询向量与少量Agent Token计算交叉注意力
//!    - 聚合全局语义信息，得到Agent上下文
//! 2. **Agent Token → Tool 交叉注意力** (广播阶段):
//!    - 用Agent上下文调制查询向量
//!    - 与工具向量计算交叉注意力，得到动态路由分数
//!
//! ## 多头注意力
//! 将64维向量分为4个头(每头16维)，每个头独立捕捉不同的语义关系，
//! 最终取多头平均作为路由分数。
//!
//! ## 可学习性
//! - Agent Token可通过k-means++从工具向量初始化
//! - 支持运行时Hebbian-like在线更新
//! - 投影权重使用Xavier初始化
//!
//! # 模块结构
//! 本模块是 `agent_token` 核心模块的高层API包装，
//! 提供与现有路由器的直接集成接口。

pub use crate::agent_token::{
    AgentAttentionConfig, AgentToken, AgentTokenBank, AgentTokenError,
    AttentionEngine as AgentTokenAttentionEngine, AttentionProjector,
};

/// Agent Token 注意力路由引擎
///
/// 核心计算流程:
/// 1. 将Query和候选向量通过多头投影矩阵变换
/// 2. Query与Agent Token计算交叉注意力，聚合语义上下文
/// 3. 用Agent上下文调制Query(残差连接)
/// 4. 调制后的Query与候选向量计算交叉注意力
/// 5. 多头结果平均，得到每个候选的标量路由分数
///
/// 本结构是对 `AgentTokenAttentionEngine` 的包装，保持向后兼容的API。
#[derive(Clone, Debug)]
pub struct AgentAttentionEngine {
    /// 内部核心注意力引擎
    inner: AgentTokenAttentionEngine,
    /// 配置
    config: AgentAttentionConfig,
    /// 维度
    dim: usize,
}

impl AgentAttentionEngine {
    /// 创建新的Agent Attention引擎
    ///
    /// 所有权重使用Xavier初始化，Agent Token使用随机初始化。
    /// 调用方应在初始化后调用 `init_agent_tokens_from_tools` 进行数据驱动初始化。
    ///
    /// # 错误
    /// 当 `dim % num_heads != 0` 时返回错误(通过panic保持向后兼容，
    /// 因原API使用assert!，但Router中已在配置校验阶段捕获此错误)
    pub fn new(dim: usize, config: AgentAttentionConfig) -> Self {
        match AgentTokenAttentionEngine::new(dim, config.clone()) {
            Ok(inner) => Self { inner, config, dim },
            Err(e) => {
                // 配置校验应在Router创建时完成，此处panic为向后兼容
                panic!("AgentAttentionEngine初始化失败: {}", e);
            }
        }
    }

    /// 从工具向量通过k-means++初始化Agent Token
    ///
    /// k-means++确保初始中心点分布良好，避免局部最优。
    /// 初始化后，Agent Token代表工具空间的语义聚类中心。
    ///
    /// # 参数
    /// - `tool_vectors`: 工具向量切片列表，每个长度 = dim
    pub fn init_agent_tokens_from_tools(&mut self, tool_vectors: &[&[f32]]) {
        if let Err(e) = self.inner.init_agent_tokens_from_tools(tool_vectors) {
            tracing::warn!("Agent Token初始化失败: {}", e);
        }
    }

    /// 计算路由分数
    ///
    /// 给定查询向量和候选向量列表，返回每个候选的标量路由分数。
    /// 分数越高表示候选与查询的相关性越强。
    ///
    /// # 参数
    /// - `query`: 查询向量，长度 = dim
    /// - `candidates`: 候选向量列表，每个长度 = dim
    ///
    /// # 返回
    /// 每个候选的标量分数向量，长度 = candidates.len()
    ///
    /// # 计算复杂度
    /// O(num_candidates * dim * num_heads)，64维/4头/300工具约7.7万次乘加，<1ms
    pub fn compute_scores(&self, query: &[f32], candidates: &[&[f32]]) -> Vec<f32> {
        match self.inner.compute_scores(query, candidates) {
            Ok(scores) => scores,
            Err(e) => {
                tracing::warn!("Agent Attention计算失败: {}", e);
                // 回退到零分数
                vec![0.0f32; candidates.len()]
            }
        }
    }

    /// 在线更新Agent Token(Hebbian-like规则)
    ///
    /// 根据查询向量和选中的工具向量，微调与查询最相关的Agent Token。
    /// 规则: 将最相关Agent Token朝向选中工具的平均向量移动。
    ///
    /// # 参数
    /// - `query`: 查询向量
    /// - `selected_indices`: 选中的候选索引
    /// - `candidates`: 所有候选向量
    /// - `learning_rate`: 学习率，建议0.001~0.01
    pub fn online_update(
        &mut self,
        query: &[f32],
        selected_indices: &[usize],
        candidates: &[&[f32]],
        learning_rate: f32,
    ) {
        if let Err(e) = self
            .inner
            .online_update(query, selected_indices, candidates, learning_rate)
        {
            tracing::warn!("Agent Attention在线更新失败: {}", e);
        }
    }

    /// 获取维度
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// 获取头数
    pub fn num_heads(&self) -> usize {
        self.inner.num_heads()
    }

    /// 获取配置
    pub fn config(&self) -> &AgentAttentionConfig {
        &self.config
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_engine_new() {
        let config = AgentAttentionConfig::default();
        let engine = AgentAttentionEngine::new(64, config);
        assert_eq!(engine.dim(), 64);
        assert_eq!(engine.num_heads(), 4);
    }

    #[test]
    fn test_engine_init_from_tools() {
        let config = AgentAttentionConfig {
            num_agent_tokens: 4,
            ..Default::default()
        };
        let mut engine = AgentAttentionEngine::new(64, config);

        let tools: Vec<Vec<f32>> = (0..20)
            .map(|i| {
                let mut v = vec![0.0f32; 64];
                v[i % 4] = 1.0;
                v
            })
            .collect();
        let tool_refs: Vec<&[f32]> = tools.iter().map(|v| v.as_slice()).collect();

        engine.init_agent_tokens_from_tools(&tool_refs);
    }

    #[test]
    fn test_engine_compute_scores() {
        let config = AgentAttentionConfig::default();
        let engine = AgentAttentionEngine::new(64, config);

        let query = vec![1.0f32; 64];
        let candidates: Vec<Vec<f32>> = (0..10)
            .map(|i| {
                let mut v = vec![0.0f32; 64];
                v[i % 4] = 1.0;
                v
            })
            .collect();
        let cand_refs: Vec<&[f32]> = candidates.iter().map(|v| v.as_slice()).collect();

        let scores = engine.compute_scores(&query, &cand_refs);
        assert_eq!(scores.len(), 10);
        assert!(scores.iter().all(|&s| s >= 0.0));
    }

    #[test]
    fn test_engine_compute_scores_empty() {
        let config = AgentAttentionConfig::default();
        let engine = AgentAttentionEngine::new(64, config);
        let query = vec![1.0f32; 64];
        let scores = engine.compute_scores(&query, &[]);
        assert!(scores.is_empty());
    }

    #[test]
    fn test_engine_online_update() {
        let config = AgentAttentionConfig {
            num_agent_tokens: 4,
            ..Default::default()
        };
        let mut engine = AgentAttentionEngine::new(64, config);

        let query = vec![1.0f32; 64];
        let candidates: Vec<Vec<f32>> = (0..5).map(|i| vec![i as f32 * 0.1; 64]).collect();
        let cand_refs: Vec<&[f32]> = candidates.iter().map(|v| v.as_slice()).collect();

        engine.online_update(&query, &[0, 1], &cand_refs, 0.01);
        // 更新后不应panic
    }

    #[test]
    fn test_engine_consistency() {
        let config = AgentAttentionConfig {
            num_heads: 2,
            num_agent_tokens: 4,
            ..Default::default()
        };
        let engine = AgentAttentionEngine::new(16, config);

        let query = vec![1.0f32; 16];
        let candidates: Vec<Vec<f32>> = (0..4)
            .map(|i| {
                let mut v = vec![0.0f32; 16];
                v[i] = 1.0;
                v
            })
            .collect();
        let cand_refs: Vec<&[f32]> = candidates.iter().map(|v| v.as_slice()).collect();

        let scores1 = engine.compute_scores(&query, &cand_refs);
        let scores2 = engine.compute_scores(&query, &cand_refs);
        assert_eq!(scores1.len(), scores2.len());
        for (a, b) in scores1.iter().zip(scores2.iter()) {
            assert!((a - b).abs() < 1e-5);
        }
    }
}
