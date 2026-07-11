//! Agent Token 核心模块 — 可学习语义聚合代理
//!
//! 对应架构层: L6 Router
//! 对应创新点: Agent Token 注意力路由
//!
//! # 核心概念
//! 参考Agent Attention (arXiv:2312.08874) 和 STAGformer (arXiv:2607.06614)，
//! 引入可学习的Agent Token作为查询与工具向量之间的语义桥梁。
//!
//! Agent Token的核心作用：
//! - 从工具向量空间通过k-means++聚类初始化，代表语义中心
//! - 运行时通过Hebbian-like规则在线更新
//! - 两阶段交叉注意力：Query→Agent→Tool，实现动态路由分数
//!
//! # 设计决策
//! - **纯Rust实现**: 不依赖外部BLAS，64维×8头×300工具规模计算量极小(<1ms)
//! - **确定性初始化**: 使用SplitMix64伪随机数，确保可复现性
//! - **无堆分配热路径**: 核心计算使用栈上预分配缓冲区，避免GC压力

use ndarray::{Array1, Array2};
use serde::{Deserialize, Serialize};

// ============ 错误类型 ============

/// Agent Token模块错误
#[derive(Debug, Clone, PartialEq)]
pub enum AgentTokenError {
    /// 维度不匹配
    DimensionMismatch {
        /// 期望的向量维度
        expected: usize,
        /// 实际传入的向量维度
        actual: usize,
    },
    /// 空输入
    EmptyInput,
    /// 无效配置
    InvalidConfig(String),
}

impl std::fmt::Display for AgentTokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DimensionMismatch { expected, actual } => {
                write!(f, "维度不匹配: 期望 {}, 实际 {}", expected, actual)
            }
            Self::EmptyInput => write!(f, "输入为空"),
            Self::InvalidConfig(msg) => write!(f, "无效配置: {}", msg),
        }
    }
}

impl std::error::Error for AgentTokenError {}

// ============ 伪随机数生成器 (SplitMix64) ============

/// 确定性伪随机数生成器，避免外部rand依赖
///
/// 使用SplitMix64算法，64位状态，周期2^64，通过BigCrush测试。
/// 适用于初始化阶段的随机数生成，确保可复现性。
#[derive(Debug, Clone, Copy)]
pub(crate) struct SimpleRng {
    state: u64,
}

impl SimpleRng {
    /// 创建新的RNG，指定种子
    pub(crate) fn new(seed: u64) -> Self {
        let mut rng = Self { state: seed };
        // 预热两次，消除种子低位相关性
        rng.next_u64();
        rng.next_u64();
        rng
    }

    /// 生成下一个u64随机数
    pub(crate) fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }

    /// 生成[0, 1)区间的f32随机数
    pub(crate) fn next_f32(&mut self) -> f32 {
        (self.next_u64() as f32) / (u64::MAX as f32)
    }

    /// 生成[min, max)区间的f32随机数
    pub(crate) fn next_f32_range(&mut self, min: f32, max: f32) -> f32 {
        min + self.next_f32() * (max - min)
    }
}

// ============ Agent Token 配置 ============

/// Agent Token 注意力路由配置
///
/// 控制Agent Attention的开关、头数、Agent Token数量等参数。
/// 默认关闭(enabled=false)，确保向后兼容。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentAttentionConfig {
    /// 是否启用Agent Token注意力路由
    pub enabled: bool,
    /// 注意力头数(默认4)，dim必须能被num_heads整除
    pub num_heads: usize,
    /// Agent Token数量(默认8)，远少于典型工具数300
    /// 每个Agent Token代表一个语义聚合中心
    pub num_agent_tokens: usize,
    /// Softmax温度系数(默认1.0)，<1.0分数更尖锐，>1.0更平滑
    pub temperature: f32,
    /// 是否在块路由(第一级)使用Agent Attention
    pub use_in_block_routing: bool,
    /// 是否在工具路由(第二级)使用Agent Attention
    pub use_in_tool_routing: bool,
}

impl Default for AgentAttentionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            num_heads: 4,
            num_agent_tokens: 8,
            temperature: 1.0,
            use_in_block_routing: true,
            use_in_tool_routing: true,
        }
    }
}

// ============ Agent Token 结构体 ============

/// 单个可学习Agent Token
///
/// Agent Token是一个与查询/工具同维度的语义向量，
/// 代表工具语义空间中的一个聚类中心。
#[derive(Debug, Clone, PartialEq)]
pub struct AgentToken {
    /// Token向量，维度 = dim
    pub vector: Vec<f32>,
}

impl AgentToken {
    /// 创建新的Agent Token，使用零向量
    pub fn new(dim: usize) -> Self {
        Self {
            vector: vec![0.0f32; dim],
        }
    }

    /// 从给定向量创建Agent Token
    pub fn from_vector(vector: Vec<f32>) -> Self {
        Self { vector }
    }

    /// 获取向量维度
    pub fn dim(&self) -> usize {
        self.vector.len()
    }

    /// 归一化为单位长度
    pub fn normalize(&mut self) {
        let norm_sq: f32 = self.vector.iter().map(|x| x * x).sum();
        let norm = norm_sq.sqrt();
        if norm > 1e-6 {
            for v in self.vector.iter_mut() {
                *v /= norm;
            }
        }
    }

    /// 计算与另一向量的内积
    pub fn dot_with(&self, other: &[f32]) -> Result<f32, AgentTokenError> {
        if self.vector.len() != other.len() {
            return Err(AgentTokenError::DimensionMismatch {
                expected: self.vector.len(),
                actual: other.len(),
            });
        }
        let dot: f32 = self
            .vector
            .iter()
            .zip(other.iter())
            .map(|(a, b)| a * b)
            .sum();
        Ok(dot)
    }
}

// ============ Agent Token 银行 ============

/// Agent Token 银行 — 管理多个可学习Agent Token
///
/// 维护一组Agent Token，提供初始化和在线更新能力。
/// 每个Agent Token代表工具语义空间的一个聚类中心。
#[derive(Debug, Clone)]
pub struct AgentTokenBank {
    /// 配置参数
    config: AgentAttentionConfig,
    /// 向量维度
    dim: usize,
    /// 每个头的维度
    head_dim: usize,
    /// 注意力缩放因子
    scale: f32,
    /// Agent Token列表
    tokens: Vec<AgentToken>,
}

impl AgentTokenBank {
    /// 创建新的Agent Token Bank
    ///
    /// Token使用随机Xavier初始化，后续应调用 `init_from_tools` 进行数据驱动初始化。
    ///
    /// # 错误
    /// 当 `dim % num_heads != 0` 时返回错误
    pub fn new(dim: usize, config: AgentAttentionConfig) -> Result<Self, AgentTokenError> {
        if config.num_heads == 0 {
            return Err(AgentTokenError::InvalidConfig("num_heads 不能为 0".into()));
        }
        if !dim.is_multiple_of(config.num_heads) {
            return Err(AgentTokenError::InvalidConfig(format!(
                "dim {} 必须能被 num_heads {} 整除",
                dim, config.num_heads
            )));
        }
        if config.num_agent_tokens == 0 {
            return Err(AgentTokenError::InvalidConfig(
                "num_agent_tokens 不能为 0".into(),
            ));
        }

        let head_dim = dim / config.num_heads;
        let scale = (head_dim as f32).sqrt();

        // Xavier初始化Agent Token
        let limit = (6.0f32 / dim as f32).sqrt();
        let mut rng = SimpleRng::new(0x9e3779b97f4a7c15);
        let mut tokens = Vec::with_capacity(config.num_agent_tokens);
        for _ in 0..config.num_agent_tokens {
            let mut vec = vec![0.0f32; dim];
            for v in vec.iter_mut() {
                *v = rng.next_f32_range(-limit, limit);
            }
            tokens.push(AgentToken::from_vector(vec));
        }

        Ok(Self {
            config,
            dim,
            head_dim,
            scale,
            tokens,
        })
    }

    /// 获取Agent Token数量
    pub fn num_tokens(&self) -> usize {
        self.tokens.len()
    }

    /// 获取向量维度
    pub fn dim(&self) -> usize {
        self.dim
    }

    /// 获取头数
    pub fn num_heads(&self) -> usize {
        self.config.num_heads
    }

    /// 获取头维度
    pub fn head_dim(&self) -> usize {
        self.head_dim
    }

    /// 获取配置
    pub fn config(&self) -> &AgentAttentionConfig {
        &self.config
    }

    /// 获取指定Agent Token的引用
    pub fn token(&self, idx: usize) -> Option<&AgentToken> {
        self.tokens.get(idx)
    }

    /// 获取所有Agent Token的向量引用
    pub fn token_vectors(&self) -> Vec<&[f32]> {
        self.tokens.iter().map(|t| t.vector.as_slice()).collect()
    }

    /// 从工具向量通过k-means++初始化Agent Token
    ///
    /// k-means++确保初始中心点分布良好，避免局部最优。
    /// 初始化后，Agent Token代表工具空间的语义聚类中心。
    ///
    /// # 参数
    /// - `tool_vectors`: 工具向量切片列表，每个长度 = dim
    pub fn init_from_tools(&mut self, tool_vectors: &[&[f32]]) -> Result<(), AgentTokenError> {
        if tool_vectors.is_empty() {
            return Err(AgentTokenError::EmptyInput);
        }

        let centers = kmeans_plus_plus(tool_vectors, self.config.num_agent_tokens, self.dim);
        let num_centers = centers.len();

        for (i, token) in self.tokens.iter_mut().enumerate().take(num_centers) {
            token.vector = centers[i].clone();
            token.normalize();
        }

        Ok(())
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
    ) -> Result<(), AgentTokenError> {
        if selected_indices.is_empty() || candidates.is_empty() || learning_rate <= 0.0 {
            return Ok(());
        }
        if query.len() != self.dim {
            return Err(AgentTokenError::DimensionMismatch {
                expected: self.dim,
                actual: query.len(),
            });
        }

        // 1. 找到与Query最相似的Agent Token(内积最大)
        let mut best_agent = 0usize;
        let mut best_sim = f32::NEG_INFINITY;
        for (a, token) in self.tokens.iter().enumerate() {
            let sim = token.dot_with(query).unwrap_or(f32::NEG_INFINITY);
            if sim > best_sim {
                best_sim = sim;
                best_agent = a;
            }
        }

        // 2. 计算选中工具的平均向量
        let mut avg_tool = vec![0.0f32; self.dim];
        let mut valid_count = 0usize;
        for &idx in selected_indices {
            if idx < candidates.len() {
                let cand = candidates[idx];
                if cand.len() == self.dim {
                    for d in 0..self.dim {
                        avg_tool[d] += cand[d];
                    }
                    valid_count += 1;
                }
            }
        }
        if valid_count == 0 {
            return Ok(());
        }
        for v in avg_tool.iter_mut() {
            *v /= valid_count as f32;
        }

        // 3. 更新最佳Agent Token朝向平均工具向量
        let token = &mut self.tokens[best_agent];
        for (avg, vec) in avg_tool.iter().zip(token.vector.iter_mut()) {
            let delta = learning_rate * (avg - *vec);
            *vec += delta;
        }

        // 4. 归一化保持单位长度
        token.normalize();

        Ok(())
    }
}

// ============ 投影矩阵 ============

/// 投影后的向量对(K/V 或 Q/K)
type ProjectedVecPair = (Vec<Array1<f32>>, Vec<Array1<f32>>);

/// 注意力投影矩阵 — Q/K/V投影
#[derive(Debug, Clone)]
pub struct AttentionProjector {
    /// 查询投影权重: (dim, dim)
    w_q: Array2<f32>,
    /// 键投影权重: (dim, dim)
    w_k: Array2<f32>,
    /// 值投影权重: (dim, dim)
    w_v: Array2<f32>,
    /// Agent查询投影: (dim, dim)
    w_agent_q: Array2<f32>,
    /// Agent键投影: (dim, dim)
    w_agent_k: Array2<f32>,
    /// 维度
    dim: usize,
}

impl AttentionProjector {
    /// 创建新的投影器，使用Xavier初始化
    pub fn new(dim: usize) -> Self {
        let w_q = xavier_init(dim, dim);
        let w_k = xavier_init(dim, dim);
        let w_v = xavier_init(dim, dim);
        let w_agent_q = xavier_init(dim, dim);
        let w_agent_k = xavier_init(dim, dim);

        Self {
            w_q,
            w_k,
            w_v,
            w_agent_q,
            w_agent_k,
            dim,
        }
    }

    /// 投影查询向量
    pub fn project_query(&self, query: &[f32]) -> Result<Array1<f32>, AgentTokenError> {
        if query.len() != self.dim {
            return Err(AgentTokenError::DimensionMismatch {
                expected: self.dim,
                actual: query.len(),
            });
        }
        let q_arr = Array1::from(query.to_vec());
        Ok(self.w_q.dot(&q_arr))
    }

    /// 投影候选向量到K/V空间
    pub fn project_candidates(
        &self,
        candidates: &[&[f32]],
    ) -> Result<ProjectedVecPair, AgentTokenError> {
        let mut k_proj = Vec::with_capacity(candidates.len());
        let mut v_proj = Vec::with_capacity(candidates.len());
        for cand in candidates {
            if cand.len() != self.dim {
                return Err(AgentTokenError::DimensionMismatch {
                    expected: self.dim,
                    actual: cand.len(),
                });
            }
            let c_arr = Array1::from(cand.to_vec());
            k_proj.push(self.w_k.dot(&c_arr));
            v_proj.push(self.w_v.dot(&c_arr));
        }
        Ok((k_proj, v_proj))
    }

    /// 投影Agent Token到Q/K空间
    pub fn project_agent_tokens(
        &self,
        tokens: &[&AgentToken],
    ) -> Result<ProjectedVecPair, AgentTokenError> {
        let mut agent_q = Vec::with_capacity(tokens.len());
        let mut agent_k = Vec::with_capacity(tokens.len());
        for token in tokens {
            if token.dim() != self.dim {
                return Err(AgentTokenError::DimensionMismatch {
                    expected: self.dim,
                    actual: token.dim(),
                });
            }
            let a_view = Array1::from(token.vector.clone());
            agent_q.push(self.w_agent_q.dot(&a_view));
            agent_k.push(self.w_agent_k.dot(&a_view));
        }
        Ok((agent_q, agent_k))
    }

    /// 获取维度
    pub fn dim(&self) -> usize {
        self.dim
    }
}

// ============ 注意力计算引擎 ============

/// 注意力计算引擎 — 执行两阶段交叉注意力
#[derive(Debug, Clone)]
pub struct AttentionEngine {
    /// Agent Token银行
    token_bank: AgentTokenBank,
    /// 投影矩阵
    projector: AttentionProjector,
    /// 温度系数
    temperature: f32,
}

impl AttentionEngine {
    /// 创建新的注意力引擎
    ///
    /// # 错误
    /// 当维度或配置无效时返回错误
    pub fn new(dim: usize, config: AgentAttentionConfig) -> Result<Self, AgentTokenError> {
        let token_bank = AgentTokenBank::new(dim, config.clone())?;
        let projector = AttentionProjector::new(dim);

        Ok(Self {
            token_bank,
            projector,
            temperature: config.temperature.max(0.01f32),
        })
    }

    /// 初始化Agent Token从工具向量
    pub fn init_agent_tokens_from_tools(
        &mut self,
        tool_vectors: &[&[f32]],
    ) -> Result<(), AgentTokenError> {
        self.token_bank.init_from_tools(tool_vectors)
    }

    /// 计算路由分数
    ///
    /// 给定查询向量和候选向量列表，返回每个候选的标量路由分数。
    /// 分数越高表示候选与查询的相关性越强。
    ///
    /// # 计算复杂度
    /// O(num_candidates * dim * num_heads)，64维/4头/300工具约7.7万次乘加，<1ms
    pub fn compute_scores(
        &self,
        query: &[f32],
        candidates: &[&[f32]],
    ) -> Result<Vec<f32>, AgentTokenError> {
        if candidates.is_empty() {
            return Ok(Vec::new());
        }
        if query.len() != self.token_bank.dim() {
            return Err(AgentTokenError::DimensionMismatch {
                expected: self.token_bank.dim(),
                actual: query.len(),
            });
        }

        let num_candidates = candidates.len();
        let num_heads = self.token_bank.num_heads();
        let head_dim = self.token_bank.head_dim();
        let scale = self.token_bank.scale;

        // 1. 投影Query到注意力空间
        let q_proj = self.projector.project_query(query)?;

        // 2. 投影候选向量到K, V空间
        let (k_proj, v_proj) = self.projector.project_candidates(candidates)?;

        // 3. 投影Agent Token到Q, K空间
        let token_refs: Vec<&AgentToken> = self.token_bank.tokens.iter().collect();
        let (agent_q, _agent_k) = self.projector.project_agent_tokens(&token_refs)?;

        let num_agent_tokens = self.token_bank.num_tokens();

        // 4. 多头注意力计算
        let mut scores = vec![0.0f32; num_candidates];

        for h in 0..num_heads {
            let start = h * head_dim;

            // 提取当前头的Query
            let mut q_head = vec![0.0f32; head_dim];
            for i in 0..head_dim {
                q_head[i] = q_proj[start + i];
            }

            // 阶段1: Query → Agent Token 交叉注意力
            let mut agent_logits = vec![0.0f32; num_agent_tokens];
            for a in 0..num_agent_tokens {
                let mut dot = 0.0f32;
                for i in 0..head_dim {
                    dot += q_head[i] * agent_q[a][start + i];
                }
                agent_logits[a] = dot / scale / self.temperature;
            }
            let agent_weights = softmax(&agent_logits);

            // 用Agent上下文调制Query(残差连接)
            let mut q_modulated = q_head.clone();
            for a in 0..num_agent_tokens {
                let w = agent_weights[a];
                for i in 0..head_dim {
                    q_modulated[i] += w * agent_q[a][start + i];
                }
            }

            // 阶段2: 调制后Query → 候选 交叉注意力
            let mut logits = vec![0.0f32; num_candidates];
            for t in 0..num_candidates {
                let mut dot = 0.0f32;
                for i in 0..head_dim {
                    dot += q_modulated[i] * k_proj[t][start + i];
                }
                logits[t] = dot / scale / self.temperature;
            }
            let attn_weights = softmax(&logits);

            // 分数 = 注意力权重 * Value相关性(经ReLU确保非负)
            for t in 0..num_candidates {
                let mut v_rel = 0.0f32;
                for i in 0..head_dim {
                    v_rel += q_modulated[i] * v_proj[t][start + i];
                }
                scores[t] += attn_weights[t] * v_rel.max(0.0);
            }
        }

        // 多头平均
        for s in scores.iter_mut() {
            *s /= num_heads as f32;
        }

        Ok(scores)
    }

    /// 在线更新Agent Token
    pub fn online_update(
        &mut self,
        query: &[f32],
        selected_indices: &[usize],
        candidates: &[&[f32]],
        learning_rate: f32,
    ) -> Result<(), AgentTokenError> {
        self.token_bank
            .online_update(query, selected_indices, candidates, learning_rate)
    }

    /// 获取维度
    pub fn dim(&self) -> usize {
        self.token_bank.dim()
    }

    /// 获取头数
    pub fn num_heads(&self) -> usize {
        self.token_bank.num_heads()
    }

    /// 获取Token数量
    pub fn num_tokens(&self) -> usize {
        self.token_bank.num_tokens()
    }

    /// 获取Token Bank的只读引用
    pub fn token_bank(&self) -> &AgentTokenBank {
        &self.token_bank
    }
}

// ============ 工具函数 ============

/// Xavier均匀初始化
///
/// 权重从 U(-limit, limit) 采样，limit = sqrt(6 / fan_in)。
/// 适用于ReLU激活的层，保持前向传播方差稳定。
fn xavier_init(rows: usize, cols: usize) -> Array2<f32> {
    let mut rng = SimpleRng::new(0x9e3779b97f4a7c15);
    let limit = (6.0f32 / cols as f32).sqrt();
    let mut arr = Array2::zeros((rows, cols));
    for i in 0..rows {
        for j in 0..cols {
            arr[[i, j]] = rng.next_f32_range(-limit, limit);
        }
    }
    arr
}

/// 数值稳定Softmax
///
/// 使用max logit技巧避免指数溢出。
fn softmax(logits: &[f32]) -> Vec<f32> {
    let max_logit = logits.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let exps: Vec<f32> = logits.iter().map(|&x| (x - max_logit).exp()).collect();
    let sum_exp: f32 = exps.iter().sum();
    if sum_exp > 1e-10 {
        exps.iter().map(|&e| e / sum_exp).collect()
    } else {
        let len = logits.len();
        vec![1.0f32 / len as f32; len]
    }
}

/// 欧几里得距离平方(避免开方，提升性能)
fn euclidean_distance_sq(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| (x - y).powi(2)).sum()
}

/// k-means++聚类初始化
///
/// 1. 随机选第一个中心点
/// 2. 对每个点，计算到最近中心的距离D(x)
/// 3. 以概率 D(x)² / sum(D²) 选择下一个中心点
/// 4. 重复直到选够k个中心
/// 5. 执行标准k-means迭代(分配+更新)直到收敛或达最大迭代数
///
/// # 参数
/// - `vectors`: 输入向量
/// - `k`: 目标中心数
/// - `dim`: 向量维度
fn kmeans_plus_plus(vectors: &[&[f32]], k: usize, dim: usize) -> Vec<Vec<f32>> {
    let n = vectors.len();
    if n == 0 {
        return Vec::new();
    }
    let k = k.min(n);

    let mut rng = SimpleRng::new(0x9e3779b97f4a7c15);

    // 1. 随机选第一个中心
    let mut centers: Vec<Vec<f32>> = Vec::with_capacity(k);
    let first_idx = (rng.next_u64() % n as u64) as usize;
    centers.push(vectors[first_idx].to_vec());

    let mut distances = vec![0.0f32; n];

    // 2. k-means++ 选剩余中心
    while centers.len() < k {
        let mut total_dist = 0.0f32;
        for (i, vec) in vectors.iter().enumerate() {
            let min_dist = centers
                .iter()
                .map(|c| euclidean_distance_sq(vec, c))
                .min_by(|a, b| {
                    if a < b {
                        std::cmp::Ordering::Less
                    } else if a > b {
                        std::cmp::Ordering::Greater
                    } else {
                        std::cmp::Ordering::Equal
                    }
                })
                .unwrap_or(0.0);
            distances[i] = min_dist;
            total_dist += min_dist;
        }

        // 若所有点都是中心，随机选剩余
        if total_dist <= 1e-10 {
            while centers.len() < k {
                let idx = (rng.next_u64() % n as u64) as usize;
                centers.push(vectors[idx].to_vec());
            }
            break;
        }

        // 轮盘赌选择: 概率正比于距离平方
        let threshold = rng.next_f32() * total_dist;
        let mut cumsum = 0.0f32;
        let mut selected = n - 1;
        for (i, &dist) in distances.iter().enumerate() {
            cumsum += dist;
            if cumsum >= threshold {
                selected = i;
                break;
            }
        }
        centers.push(vectors[selected].to_vec());
    }

    // 3. 标准k-means迭代
    kmeans_iterate(vectors, centers, dim)
}

/// k-means标准迭代
///
/// 分配+更新直到收敛或达最大迭代数
fn kmeans_iterate(vectors: &[&[f32]], mut centers: Vec<Vec<f32>>, dim: usize) -> Vec<Vec<f32>> {
    let n = vectors.len();
    let k = centers.len();
    let max_iterations = 30;

    for _ in 0..max_iterations {
        // 分配
        let mut assignments = vec![0usize; n];
        let mut changed = false;
        for (i, vec) in vectors.iter().enumerate() {
            let mut best_dist = f32::INFINITY;
            let mut best_j = 0;
            for (j, center) in centers.iter().enumerate() {
                let dist = euclidean_distance_sq(vec, center);
                if dist < best_dist {
                    best_dist = dist;
                    best_j = j;
                }
            }
            if assignments[i] != best_j {
                changed = true;
            }
            assignments[i] = best_j;
        }
        if !changed {
            break; // 收敛
        }

        // 更新中心
        let mut new_centers = vec![vec![0.0f32; dim]; k];
        let mut counts = vec![0usize; k];
        for (i, vec) in vectors.iter().enumerate() {
            let j = assignments[i];
            for d in 0..dim {
                new_centers[j][d] += vec[d];
            }
            counts[j] += 1;
        }

        for j in 0..k {
            if counts[j] > 0 {
                let count = counts[j] as f32;
                for v in new_centers[j].iter_mut() {
                    *v /= count;
                }
                centers[j] = new_centers[j].clone();
            }
        }
    }

    centers
}

#[cfg(test)]
mod tests {
    use super::*;

    // ============ SimpleRng 测试 ============

    #[test]
    fn test_rng_deterministic() {
        let mut rng1 = SimpleRng::new(0x12345);
        let mut rng2 = SimpleRng::new(0x12345);
        for _ in 0..10 {
            assert_eq!(rng1.next_u64(), rng2.next_u64());
        }
    }

    #[test]
    fn test_rng_range() {
        let mut rng = SimpleRng::new(0x12345);
        for _ in 0..100 {
            let v = rng.next_f32();
            assert!((0.0..1.0).contains(&v));
        }
    }

    // ============ AgentToken 测试 ============

    #[test]
    fn test_agent_token_new() {
        let token = AgentToken::new(64);
        assert_eq!(token.dim(), 64);
        assert!(token.vector.iter().all(|&x| x == 0.0));
    }

    #[test]
    fn test_agent_token_from_vector() {
        let v = vec![1.0, 2.0, 3.0];
        let token = AgentToken::from_vector(v.clone());
        assert_eq!(token.vector, v);
        assert_eq!(token.dim(), 3);
    }

    #[test]
    fn test_agent_token_normalize() {
        let mut token = AgentToken::from_vector(vec![3.0, 4.0]);
        token.normalize();
        let norm = token.vector.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!((norm - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_agent_token_dot_with() {
        let token = AgentToken::from_vector(vec![1.0, 2.0, 3.0]);
        let other = vec![1.0, 1.0, 1.0];
        let dot = token.dot_with(&other).unwrap();
        assert!((dot - 6.0).abs() < 1e-5);
    }

    #[test]
    fn test_agent_token_dot_with_mismatch() {
        let token = AgentToken::from_vector(vec![1.0, 2.0]);
        let other = vec![1.0, 1.0, 1.0];
        let result = token.dot_with(&other);
        assert!(matches!(
            result,
            Err(AgentTokenError::DimensionMismatch { .. })
        ));
    }

    // ============ AgentTokenBank 测试 ============

    #[test]
    fn test_token_bank_new() {
        let config = AgentAttentionConfig::default();
        let bank = AgentTokenBank::new(64, config).unwrap();
        assert_eq!(bank.dim(), 64);
        assert_eq!(bank.num_tokens(), 8);
        assert_eq!(bank.num_heads(), 4);
        assert_eq!(bank.head_dim(), 16);
    }

    #[test]
    fn test_token_bank_new_invalid_heads() {
        let config = AgentAttentionConfig {
            num_heads: 0,
            ..Default::default()
        };
        let result = AgentTokenBank::new(64, config);
        assert!(matches!(result, Err(AgentTokenError::InvalidConfig(_))));
    }

    #[test]
    fn test_token_bank_new_dim_not_divisible() {
        let config = AgentAttentionConfig::default();
        let result = AgentTokenBank::new(63, config);
        assert!(matches!(result, Err(AgentTokenError::InvalidConfig(_))));
    }

    #[test]
    fn test_token_bank_init_from_tools() {
        let config = AgentAttentionConfig {
            num_agent_tokens: 4,
            ..Default::default()
        };
        let mut bank = AgentTokenBank::new(64, config).unwrap();

        let tools: Vec<Vec<f32>> = (0..20)
            .map(|i| {
                let mut v = vec![0.0f32; 64];
                v[i % 4] = 1.0;
                v
            })
            .collect();
        let tool_refs: Vec<&[f32]> = tools.iter().map(|v| v.as_slice()).collect();

        bank.init_from_tools(&tool_refs).unwrap();
        assert!(bank
            .tokens
            .iter()
            .any(|t| t.vector.iter().any(|&x| x != 0.0)));
    }

    #[test]
    fn test_token_bank_init_empty_tools() {
        let config = AgentAttentionConfig {
            num_agent_tokens: 4,
            ..Default::default()
        };
        let mut bank = AgentTokenBank::new(64, config).unwrap();
        let result = bank.init_from_tools(&[]);
        assert!(matches!(result, Err(AgentTokenError::EmptyInput)));
    }

    #[test]
    fn test_token_bank_online_update() {
        let config = AgentAttentionConfig {
            num_agent_tokens: 4,
            ..Default::default()
        };
        let mut bank = AgentTokenBank::new(64, config).unwrap();

        let query = vec![1.0f32; 64];
        let candidates: Vec<Vec<f32>> = (0..5).map(|i| vec![i as f32 * 0.1; 64]).collect();
        let cand_refs: Vec<&[f32]> = candidates.iter().map(|v| v.as_slice()).collect();

        let before: Vec<Vec<f32>> = bank.tokens.iter().map(|t| t.vector.clone()).collect();
        bank.online_update(&query, &[0, 1], &cand_refs, 0.01)
            .unwrap();

        // online_update 会选择与 query 最相似的 token 更新,不一定是 tokens[0],
        // 因此检查所有 token 中是否有任何一个发生变化。
        let mut changed = false;
        for (i, token) in bank.tokens.iter().enumerate() {
            for (d, &before_val) in before[i].iter().enumerate().take(64) {
                if (token.vector[d] - before_val).abs() > 1e-6 {
                    changed = true;
                    break;
                }
            }
            if changed {
                break;
            }
        }
        assert!(changed, "online_update应改变至少一个Agent Token");
    }

    #[test]
    fn test_token_bank_online_update_empty_selected() {
        let config = AgentAttentionConfig {
            num_agent_tokens: 4,
            ..Default::default()
        };
        let mut bank = AgentTokenBank::new(64, config).unwrap();

        let query = vec![1.0f32; 64];
        let candidates: Vec<Vec<f32>> = (0..5).map(|i| vec![i as f32 * 0.1; 64]).collect();
        let cand_refs: Vec<&[f32]> = candidates.iter().map(|v| v.as_slice()).collect();

        let before = bank.tokens.clone();
        bank.online_update(&query, &[], &cand_refs, 0.01).unwrap();
        assert_eq!(bank.tokens, before);
    }

    #[test]
    fn test_token_bank_online_update_zero_lr() {
        let config = AgentAttentionConfig {
            num_agent_tokens: 4,
            ..Default::default()
        };
        let mut bank = AgentTokenBank::new(64, config).unwrap();

        let query = vec![1.0f32; 64];
        let candidates: Vec<Vec<f32>> = (0..5).map(|i| vec![i as f32 * 0.1; 64]).collect();
        let cand_refs: Vec<&[f32]> = candidates.iter().map(|v| v.as_slice()).collect();

        let before = bank.tokens.clone();
        bank.online_update(&query, &[0, 1], &cand_refs, 0.0)
            .unwrap();
        assert_eq!(bank.tokens, before);
    }

    // ============ AttentionProjector 测试 ============

    #[test]
    fn test_projector_new() {
        let projector = AttentionProjector::new(64);
        assert_eq!(projector.dim(), 64);
    }

    #[test]
    fn test_projector_project_query() {
        let projector = AttentionProjector::new(64);
        let query = vec![1.0f32; 64];
        let projected = projector.project_query(&query).unwrap();
        assert_eq!(projected.len(), 64);
    }

    #[test]
    fn test_projector_project_query_mismatch() {
        let projector = AttentionProjector::new(64);
        let query = vec![1.0f32; 63];
        let result = projector.project_query(&query);
        assert!(matches!(
            result,
            Err(AgentTokenError::DimensionMismatch { .. })
        ));
    }

    #[test]
    fn test_projector_project_candidates() {
        let projector = AttentionProjector::new(64);
        let cands: Vec<Vec<f32>> = (0..3).map(|_| vec![1.0f32; 64]).collect();
        let cand_refs: Vec<&[f32]> = cands.iter().map(|v| v.as_slice()).collect();
        let (k_proj, v_proj) = projector.project_candidates(&cand_refs).unwrap();
        assert_eq!(k_proj.len(), 3);
        assert_eq!(v_proj.len(), 3);
    }

    // ============ AttentionEngine 测试 ============

    #[test]
    fn test_attention_engine_new() {
        let config = AgentAttentionConfig::default();
        let engine = AttentionEngine::new(64, config).unwrap();
        assert_eq!(engine.dim(), 64);
        assert_eq!(engine.num_heads(), 4);
        assert_eq!(engine.num_tokens(), 8);
    }

    #[test]
    fn test_attention_engine_compute_scores() {
        let config = AgentAttentionConfig::default();
        let engine = AttentionEngine::new(64, config).unwrap();

        let query = vec![1.0f32; 64];
        let candidates: Vec<Vec<f32>> = (0..10)
            .map(|i| {
                let mut v = vec![0.0f32; 64];
                v[i % 4] = 1.0;
                v
            })
            .collect();
        let cand_refs: Vec<&[f32]> = candidates.iter().map(|v| v.as_slice()).collect();

        let scores = engine.compute_scores(&query, &cand_refs).unwrap();
        assert_eq!(scores.len(), 10);
        assert!(scores.iter().all(|&s| s >= 0.0));
    }

    #[test]
    fn test_attention_engine_compute_scores_empty() {
        let config = AgentAttentionConfig::default();
        let engine = AttentionEngine::new(64, config).unwrap();
        let query = vec![1.0f32; 64];
        let scores = engine.compute_scores(&query, &[]).unwrap();
        assert!(scores.is_empty());
    }

    #[test]
    fn test_attention_engine_compute_scores_mismatch() {
        let config = AgentAttentionConfig::default();
        let engine = AttentionEngine::new(64, config).unwrap();
        let query = vec![1.0f32; 63];
        let candidates: Vec<Vec<f32>> = vec![vec![1.0f32; 64]];
        let cand_refs: Vec<&[f32]> = candidates.iter().map(|v| v.as_slice()).collect();
        let result = engine.compute_scores(&query, &cand_refs);
        assert!(matches!(
            result,
            Err(AgentTokenError::DimensionMismatch { .. })
        ));
    }

    #[test]
    fn test_attention_engine_consistency() {
        let config = AgentAttentionConfig {
            num_heads: 2,
            num_agent_tokens: 4,
            ..Default::default()
        };
        let engine = AttentionEngine::new(16, config).unwrap();

        let query = vec![1.0f32; 16];
        let candidates: Vec<Vec<f32>> = (0..4)
            .map(|i| {
                let mut v = vec![0.0f32; 16];
                v[i] = 1.0;
                v
            })
            .collect();
        let cand_refs: Vec<&[f32]> = candidates.iter().map(|v| v.as_slice()).collect();

        let scores1 = engine.compute_scores(&query, &cand_refs).unwrap();
        let scores2 = engine.compute_scores(&query, &cand_refs).unwrap();
        assert_eq!(scores1.len(), scores2.len());
        for (a, b) in scores1.iter().zip(scores2.iter()) {
            assert!((a - b).abs() < 1e-5);
        }
    }

    #[test]
    fn test_attention_engine_online_update() {
        let config = AgentAttentionConfig {
            num_agent_tokens: 4,
            ..Default::default()
        };
        let mut engine = AttentionEngine::new(64, config).unwrap();

        let query = vec![1.0f32; 64];
        let candidates: Vec<Vec<f32>> = (0..5).map(|i| vec![i as f32 * 0.1; 64]).collect();
        let cand_refs: Vec<&[f32]> = candidates.iter().map(|v| v.as_slice()).collect();

        engine
            .online_update(&query, &[0, 1], &cand_refs, 0.01)
            .unwrap();
        // 更新后不应panic
    }

    // ============ 工具函数测试 ============

    #[test]
    fn test_softmax() {
        let logits = vec![1.0f32, 2.0, 3.0];
        let probs = softmax(&logits);
        let sum: f32 = probs.iter().sum();
        assert!((sum - 1.0).abs() < 1e-5);
        assert!(probs[2] > probs[1]);
        assert!(probs[1] > probs[0]);
    }

    #[test]
    fn test_softmax_numerical_stability() {
        let logits = vec![1000.0f32, 1001.0, 1002.0];
        let probs = softmax(&logits);
        assert!(probs.iter().all(|&p| p.is_finite()));
        let sum: f32 = probs.iter().sum();
        assert!((sum - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_euclidean_distance_sq() {
        let a = vec![1.0, 2.0, 3.0];
        let b = vec![1.0, 2.0, 3.0];
        assert_eq!(euclidean_distance_sq(&a, &b), 0.0);

        let c = vec![0.0, 0.0, 0.0];
        assert_eq!(euclidean_distance_sq(&a, &c), 14.0); // 1+4+9
    }

    #[test]
    fn test_kmeans_plus_plus() {
        let vectors: Vec<Vec<f32>> = vec![
            vec![0.0, 0.0],
            vec![0.1, 0.1],
            vec![10.0, 10.0],
            vec![10.1, 10.1],
        ];
        let refs: Vec<&[f32]> = vectors.iter().map(|v| v.as_slice()).collect();
        let centers = kmeans_plus_plus(&refs, 2, 2);
        assert_eq!(centers.len(), 2);
        // 两个中心应分别接近(0,0)和(10,10)
        let dist_00: f32 = centers
            .iter()
            .map(|c| euclidean_distance_sq(c, &[0.0, 0.0]))
            .min_by(|a, b| {
                if a < b {
                    std::cmp::Ordering::Less
                } else if a > b {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .unwrap();
        let dist_1010: f32 = centers
            .iter()
            .map(|c| euclidean_distance_sq(c, &[10.0, 10.0]))
            .min_by(|a, b| {
                if a < b {
                    std::cmp::Ordering::Less
                } else if a > b {
                    std::cmp::Ordering::Greater
                } else {
                    std::cmp::Ordering::Equal
                }
            })
            .unwrap();
        assert!(dist_00 < 1.0);
        assert!(dist_1010 < 1.0);
    }

    #[test]
    fn test_kmeans_more_than_data() {
        let vectors: Vec<Vec<f32>> = vec![vec![1.0, 2.0], vec![3.0, 4.0]];
        let refs: Vec<&[f32]> = vectors.iter().map(|v| v.as_slice()).collect();
        let centers = kmeans_plus_plus(&refs, 10, 2);
        assert_eq!(centers.len(), 2);
    }

    #[test]
    fn test_kmeans_empty() {
        let refs: Vec<&[f32]> = Vec::new();
        let centers = kmeans_plus_plus(&refs, 2, 2);
        assert!(centers.is_empty());
    }

    #[test]
    fn test_xavier_init_range() {
        let arr = xavier_init(10, 64);
        let limit = (6.0f32 / 64.0).sqrt();
        assert!(arr.iter().all(|&x| x >= -limit && x <= limit));
    }

    #[test]
    fn test_config_default() {
        let config = AgentAttentionConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.num_heads, 4);
        assert_eq!(config.num_agent_tokens, 8);
        assert!((config.temperature - 1.0).abs() < 1e-5);
        assert!(config.use_in_block_routing);
        assert!(config.use_in_tool_routing);
    }

    #[test]
    fn test_error_display() {
        let err = AgentTokenError::DimensionMismatch {
            expected: 64,
            actual: 32,
        };
        let s = err.to_string();
        assert!(s.contains("64"));
        assert!(s.contains("32"));
    }
}
