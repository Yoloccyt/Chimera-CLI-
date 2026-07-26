//! HCW-Sparse v2.0 召回流水线共享类型
//!
//! 对应任务: P3-W9.1（粗召回）/ P3-W9.2（精排）/ P3-W10.1（重排填充）
//!
//! # 设计原则
//! - **Project 图抽象**: `ModuleGraph` 持有节点 + 边，不假设数据来源
//!   （可由 ISCM 锚点、csn-substitutor import 图或外部构建器注入）
//! - **共变更历史**: `CoChangeMatrix` 用稀疏 HashMap 存储（模块对 → 共变更次数），
//!   避免稠密矩阵的 O(N²) 内存（5000 模块 → 25M 条目 ≈ 600MB，不可接受）
//! - **CLV 复用**: 语义信号直接用 `nexus_core::CLV`，避免类型重复
//! - **Top-K 选择**: 输出 `Vec<ModuleScore>`，由调用方决定是否进一步筛选

use std::collections::HashMap;

use nexus_core::CLV;
use serde::{Deserialize, Serialize};

// ============================================================
// 基础类型
// ============================================================

/// 模块 ID — Project 图节点标识
///
/// 在 HCW 场景下对应 `ContextEntry::file_id` 或 ISCM `entity_id`，
/// 用 `String` 而非 newtype 是为了与既有 `file_id: String` 兼容（避免转换开销）。
pub type ModuleId = String;

// ============================================================
// 召回权重配置
// ============================================================

/// 召回三信号融合权重 — D1 修复的核心配置
///
/// 默认值 (0.4, 0.3, 0.3) 对应 v5.0 设计文档 §4.2「Project 图联合传播」：
/// - 依赖接近度 40%（Project 图传播）
/// - 语义相似度 30%（CLV 余弦相似度）
/// - 共变更历史 30%（Co-Change Matrix）
///
/// # 设计决策（WHY）
/// - **不派生 `Eq`**: 含 `f32` 字段，浮点类型不实现 `Eq`（仅 `PartialEq`）
/// - **验证 `is_valid()`**: 三权重之和需在 `[0.99, 1.01]` 容差内（浮点误差容忍），
///   避免任一信号被静默归零
/// - **P3-W10.3 衔接**: 后续 selector 权重外置（`SelectorPolicy`）会复用同样的
///   「静态默认 + 学习层覆盖」模式，本类型提供先例
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RecallWeights {
    /// 依赖接近度权重（默认 0.4）
    pub dependency: f32,
    /// 语义相似度权重（默认 0.3）
    pub semantic: f32,
    /// 共变更历史权重（默认 0.3）
    pub cochange: f32,
}

impl RecallWeights {
    /// 默认权重 (0.4, 0.3, 0.3) — 对齐 v5.0 设计文档 §4.2
    pub const DEFAULT: Self = Self {
        dependency: 0.4,
        semantic: 0.3,
        cochange: 0.3,
    };

    /// 创建自定义权重
    pub fn new(dependency: f32, semantic: f32, cochange: f32) -> Self {
        Self {
            dependency,
            semantic,
            cochange,
        }
    }

    /// 校验权重合法性 — 三权重之和需在 [0.99, 1.01] 容差内
    ///
    /// WHY: 0.4 + 0.3 + 0.3 = 1.0 是融合前提，浮点误差允许 ±0.01。
    /// 任一权重为 0 不会触发错误（允许信号被禁用），但三权重和偏离 1.0 太远
    /// 会导致综合分数失真。
    pub fn is_valid(&self) -> bool {
        let sum = self.dependency + self.semantic + self.cochange;
        (sum - 1.0).abs() < 0.01
    }
}

impl Default for RecallWeights {
    fn default() -> Self {
        Self::DEFAULT
    }
}

// ============================================================
// Project 图
// ============================================================

/// Project 图 — 模块依赖关系图（有向无权图）
///
/// 节点为 `ModuleId`（如 `"crates/hcw-window"`、`"src/recall/coarse.rs"`），
/// 边为有向依赖（`from -> to`，表示 from 依赖 to）。
///
/// # 数据来源（开放设计）
/// 本结构不假设数据来源，调用方可通过：
/// 1. `from_edges()` 从边列表构造
/// 2. `from_adjacency()` 从邻接表构造
/// 3. 后续可扩展 `from_iscm_anchors()` / `from_cargo_manifest()` 等构建器
///
/// # 性能特征
/// - 节点数 N ≤ 5000（HCW 场景典型规模）
/// - 边数 E ≤ 5N（稀疏图，模块依赖平均度 < 5）
/// - BFS 单源传播 O(N+E)，5000 节点 ≈ 1ms
///
/// # 线程安全
/// 结构体内部用 `HashMap`，非 `Send + Sync` 默认（需要时用 `Arc<RwLock<ModuleGraph>>` 包裹）。
/// HCW 粗召回为同步操作，调用方在 `spawn_blocking` 中执行（§4.4 反模式 2）。
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModuleGraph {
    /// 邻接表：module_id → 直接依赖列表（out-edges）
    adjacency: HashMap<ModuleId, Vec<ModuleId>>,
    /// 反向邻接表：module_id → 被依赖列表（in-edges，用于反向传播）
    reverse_adjacency: HashMap<ModuleId, Vec<ModuleId>>,
    /// 全部节点集合（含孤立节点）
    nodes: HashMap<ModuleId, ()>,
}

impl ModuleGraph {
    /// 从边列表构造 Project 图
    ///
    /// # 参数
    /// - `edges`: `Vec<(from, to)>`，每条边表示 `from` 依赖 `to`
    /// - `isolated_nodes`: 额外的孤立节点（无依赖关系的模块，仍参与语义/共变更打分）
    ///
    /// # 示例
    /// ```
    /// use hcw_window::recall::ModuleGraph;
    ///
    /// let graph = ModuleGraph::from_edges(
    ///     vec![("a".into(), "b".into()), ("b".into(), "c".into())],
    ///     vec![],
    /// );
    /// assert_eq!(graph.node_count(), 3);
    /// assert_eq!(graph.edge_count(), 2);
    /// ```
    pub fn from_edges(edges: Vec<(ModuleId, ModuleId)>, isolated_nodes: Vec<ModuleId>) -> Self {
        let mut graph = Self::default();
        for (from, to) in edges {
            graph
                .adjacency
                .entry(from.clone())
                .or_default()
                .push(to.clone());
            graph
                .reverse_adjacency
                .entry(to.clone())
                .or_default()
                .push(from.clone());
            graph.nodes.insert(from, ());
            graph.nodes.insert(to, ());
        }
        for node in isolated_nodes {
            graph.nodes.insert(node, ());
        }
        graph
    }

    /// 从邻接表构造（高级用例，避免重复解析边列表）
    pub fn from_adjacency(adjacency: HashMap<ModuleId, Vec<ModuleId>>) -> Self {
        let mut graph = Self::default();
        for (from, deps) in adjacency {
            graph.nodes.insert(from.clone(), ());
            for to in deps {
                graph
                    .adjacency
                    .entry(from.clone())
                    .or_default()
                    .push(to.clone());
                graph
                    .reverse_adjacency
                    .entry(to.clone())
                    .or_default()
                    .push(from.clone());
                graph.nodes.insert(to, ());
            }
        }
        graph
    }

    /// 返回节点数
    pub fn node_count(&self) -> usize {
        self.nodes.len()
    }

    /// 返回边数
    pub fn edge_count(&self) -> usize {
        self.adjacency.values().map(|v| v.len()).sum()
    }

    /// 返回指定模块的直接依赖（out-edges）— 用于前向传播
    ///
    /// 不存在时返回空切片（非错误，孤立节点合法）
    pub fn dependencies(&self, module: &str) -> &[ModuleId] {
        self.adjacency
            .get(module)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// 返回依赖指定模块的列表（in-edges）— 用于反向传播
    pub fn dependents(&self, module: &str) -> &[ModuleId] {
        self.reverse_adjacency
            .get(module)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }

    /// 返回全部节点 ID 迭代器
    pub fn nodes(&self) -> impl Iterator<Item = &ModuleId> {
        self.nodes.keys()
    }

    /// 判断节点是否存在
    pub fn contains(&self, module: &str) -> bool {
        self.nodes.contains_key(module)
    }
}

// ============================================================
// 共变更矩阵
// ============================================================

/// 共变更矩阵 — 记录模块对同时被访问的历史频率
///
/// # 数据结构
/// 用 `HashMap<(ModuleId, ModuleId), u32>` 稀疏存储，
/// 仅记录共变更次数 > 0 的模块对。5000 模块典型场景下稀疏度 < 5%，
/// 内存占用 < 5MB（远优于稠密矩阵 25M 条目 ≈ 600MB）。
///
/// # 对称性
/// 共变更关系对称（A 与 B 共变更 ↔ B 与 A 共变更），
/// 调用 `record()` 时会写入两个方向的键，查询时任一方向均可命中。
///
/// # 归一化
/// `cochange_score(module, seeds)` 返回 ∈ [0.0, 1.0]：
/// - 0.0 = 与所有种子模块均无共变更历史
/// - 1.0 = 与某一种子模块共变更次数达到全局最大值
///
/// 多种子时取最大值（最相关的种子决定分数，避免平均稀释）
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CoChangeMatrix {
    /// 共变更计数：有序模块对 → 次数
    ///
    /// WHY 用 `(String, String)` 而非 newtype：HashMap 的 key 需要Eq + Hash，
    /// `(String, String)` 已派生，避免引入额外类型
    counts: HashMap<(ModuleId, ModuleId), u32>,
    /// 全局最大共变更次数（用于归一化）
    max_count: u32,
}

impl CoChangeMatrix {
    /// 创建空矩阵
    pub fn new() -> Self {
        Self::default()
    }

    /// 从共变更记录构造（批量初始化）
    ///
    /// # 参数
    /// - `pairs`: `Vec<(module_a, module_b, count)>` — 模块对与共变更次数
    pub fn from_pairs(pairs: Vec<(ModuleId, ModuleId, u32)>) -> Self {
        let mut matrix = Self::new();
        for (a, b, count) in pairs {
            matrix.record_internal(a, b, count);
        }
        matrix
    }

    /// 内部记录共变更（双向写入）
    fn record_internal(&mut self, a: ModuleId, b: ModuleId, count: u32) {
        if count == 0 {
            return;
        }
        // 双向写入保证查询时任一方向命中
        self.counts.insert((a.clone(), b.clone()), count);
        self.counts.insert((b, a), count);
        if count > self.max_count {
            self.max_count = count;
        }
    }

    /// 记录一次共变更（递增计数，用于在线学习场景）
    ///
    /// # 参数
    /// - `a`, `b`: 共变更的两个模块（顺序无关）
    pub fn record(&mut self, a: ModuleId, b: ModuleId) {
        if a == b {
            return; // 自共变更无意义
        }
        let count = self
            .counts
            .get(&(a.clone(), b.clone()))
            .copied()
            .unwrap_or(0)
            + 1;
        self.record_internal(a, b, count);
    }

    /// 查询模块对的共变更次数
    pub fn count(&self, a: &str, b: &str) -> u32 {
        // 用 (String, String) 的引用查询需要构造 key，借用 `get` 接受 `&(String, String)`
        // 用 `(a.to_string(), b.to_string())` 会产生分配，但稀疏矩阵查询频率低（仅粗召回时）
        self.counts
            .get(&(a.to_string(), b.to_string()))
            .copied()
            .unwrap_or(0)
    }

    /// 计算模块与种子集合的共变更分数 ∈ [0.0, 1.0]
    ///
    /// 多种子时取最大值（最相关的种子决定分数）
    pub fn cochange_score(&self, module: &str, seeds: &[ModuleId]) -> f32 {
        if self.max_count == 0 || seeds.is_empty() {
            return 0.0;
        }
        let mut best = 0u32;
        for seed in seeds {
            let count = self.count(module, seed);
            if count > best {
                best = count;
            }
        }
        // 归一化到 [0.0, 1.0]，max_count 已保证 > 0
        // WHY `as f32`:Rust 未实现 `From<u32> for f32`(精度损失),
        // u32 → f32 在 [0, 2^24] 范围内精确,共变更次数远低于此上限
        best as f32 / self.max_count as f32
    }

    /// 返回矩阵中记录的模块对数（含双向，每个无序对计 2 个有序对）
    pub fn pair_count(&self) -> usize {
        self.counts.len()
    }
}

// ============================================================
// 粗召回输入/输出
// ============================================================

/// 粗召回输入 — 由调用方组装的多信号源
///
/// # 字段
/// - `seed_modules`: 种子模块 ID 列表（来自当前 Quest/Agent 上下文）
/// - `seed_clv`: 种子 CLV（语义信号源，通常是 Quest CLV 的加权平均）
/// - `module_clvs`: 全部候选模块的 CLV（用于计算语义相似度）
/// - `top_k`: 返回模块数（默认 100，spec.md 要求）
///
/// # 设计决策（WHY）
/// - `module_clvs` 用 `&HashMap` 而非 `Vec<CLV>`：粗召回需按模块 ID 查询 CLV，
///   HashMap O(1) 查询优于 Vec 线性扫描
/// - `seed_clv` 用 `&CLV` 而非 `&[CLV]`：多种子场景下调用方应预先融合为单个 CLV
///   （加权和或平均），避免实现层处理多种子融合逻辑
pub struct CoarseRecallInput<'a> {
    /// 种子模块 ID 列表（依赖接近度 + 共变更的种子源）
    pub seed_modules: &'a [ModuleId],
    /// 种子 CLV（语义相似度的查询向量）
    pub seed_clv: &'a CLV,
    /// 全部候选模块的 CLV 映射（module_id → CLV）
    pub module_clvs: &'a HashMap<ModuleId, CLV>,
    /// 返回模块数（spec.md 要求 100）
    pub top_k: usize,
}

/// 粗召回输出 — Top-K 模块列表 + 性能指标
///
/// # 字段
/// - `modules`: 按综合分数降序排列的 Top-K 模块
/// - `elapsed_us`: 召回耗时（微秒），用于性能基准断言 <10ms
///
/// # 设计决策（WHY）
/// - `elapsed_us` 在结构体中而非日志：调用方可能根据延迟动态调整策略
///   （如超 10ms 阈值时降级到只走依赖图传播，跳过语义计算）
/// - `modules` 排序而非未排序：调用方通常直接消费 Top-K，预排序避免重复排序
#[derive(Debug, Clone, PartialEq)]
pub struct CoarseRecallOutput {
    /// Top-K 模块列表（按综合分数降序）
    pub modules: Vec<ModuleScore>,
    /// 召回耗时（微秒）
    pub elapsed_us: u64,
}

/// 模块综合分数 — 粗召回输出条目
///
/// # 字段
/// - `module_id`: 模块 ID
/// - `score`: 综合分数 ∈ [0.0, 1.0]（三信号加权融合）
/// - `dep_score`: 依赖接近度分量 ∈ [0.0, 1.0]
/// - `semantic_score`: 语义相似度分量 ∈ [0.0, 1.0]
/// - `cochange_score`: 共变更分量 ∈ [0.0, 1.0]
///
/// # 不派生 `Eq`
/// 含 `f32` 字段，浮点类型不实现 `Eq`（仅 `PartialEq`）。
/// 若需排序后稳定顺序，按 `score` 降序 + `module_id` 字典序作为 tiebreaker。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModuleScore {
    /// 模块 ID
    pub module_id: ModuleId,
    /// 综合分数 ∈ [0.0, 1.0]
    pub score: f32,
    /// 依赖接近度分量 ∈ [0.0, 1.0]
    pub dep_score: f32,
    /// 语义相似度分量 ∈ [0.0, 1.0]
    pub semantic_score: f32,
    /// 共变更分量 ∈ [0.0, 1.0]
    pub cochange_score: f32,
}

impl ModuleScore {
    /// 创建新分数条目
    pub fn new(module_id: impl Into<ModuleId>, score: f32, dep: f32, sem: f32, co: f32) -> Self {
        Self {
            module_id: module_id.into(),
            score,
            dep_score: dep,
            semantic_score: sem,
            cochange_score: co,
        }
    }
}

// ============================================================
// 精排输入/输出（P3-W9.2）
// ============================================================

/// Block ID 类型别名 — 代码块级标识（如 `src/recall/coarse.rs:1-200`）
pub type BlockId = String;

/// Block 级分数 — 精排输出条目
///
/// # 字段
/// - `block_id`: Block 标识（与 VectorStore upsert 时的 id 一致）
/// - `score`: 精确 CLV 相似度 ∈ [0.0, 1.0]（用 `CLV::cosine_similarity` 精确计算，非 HNSW 近似）
/// - `hnsw_score`: HNSW 检索返回的相似度（用于诊断与对比）
/// - `source_module`: 来源模块 ID（粗召回的哪个模块检索到此 Block，空串表示直接检索）
///
/// # 不派生 `Eq`
/// 含 `f32` 字段，浮点类型不实现 `Eq`（仅 `PartialEq`）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BlockScore {
    /// Block 标识
    pub block_id: BlockId,
    /// 精确 CLV 相似度 ∈ [0.0, 1.0]
    pub score: f32,
    /// HNSW 检索相似度（诊断字段，用于对比精确 vs 近似）
    pub hnsw_score: f32,
    /// 来源模块 ID（粗召回的哪个模块检索到此 Block）
    pub source_module: ModuleId,
    /// Block 的 token 数（P3-W10.1 重排填充新增，用于密度贪心 score/token_count）
    /// 精排阶段填 0（未知），重排填充阶段由 `RerankFillInput.block_tokens` 注入
    pub token_count: usize,
}

impl BlockScore {
    /// 创建新 Block 分数条目
    ///
    /// # 参数
    /// - `token_count`: Block 的 token 数，精排阶段传 0（未知），
    ///   重排填充阶段从 `block_tokens` 映射注入实际值
    pub fn new(
        block_id: impl Into<BlockId>,
        score: f32,
        hnsw_score: f32,
        source_module: impl Into<ModuleId>,
        token_count: usize,
    ) -> Self {
        Self {
            block_id: block_id.into(),
            score,
            hnsw_score,
            source_module: source_module.into(),
            token_count,
        }
    }
}

/// 精排输出 — Top-K Block 列表 + 性能指标
///
/// # 字段
/// - `blocks`: 按精确 CLV 相似度降序排列的 Top-K Block
/// - `elapsed_us`: 精排耗时（微秒），用于性能基准断言 <50ms
/// - `candidate_count`: HNSW 检索的候选总数（over-fetch 数量，用于诊断召回率）
///
/// # 设计决策（WHY）
/// - `elapsed_us` 在结构体中而非日志：调用方可能根据延迟动态调整策略
///   （如超 50ms 阈值时减少 over-fetch 因子）
/// - `candidate_count` 记录 over-fetch 数量：诊断 HNSW 召回率时，
///   candidate_count / vector_store_size 反映 HNSW 覆盖度
#[derive(Debug, Clone, PartialEq)]
pub struct FineRecallOutput {
    /// Top-K Block 列表（按精确 CLV 相似度降序）
    pub blocks: Vec<BlockScore>,
    /// 精排耗时（微秒）
    pub elapsed_us: u64,
    /// HNSW 检索的候选总数（over-fetch 数量）
    pub candidate_count: usize,
}

/// 精排配置 — 控制 over-fetch 因子与精确重排策略
///
/// # 设计决策（WHY）
/// - `overfetch_factor`: HNSW 检索时获取 `top_k × factor` 个候选，
///   精确重排后截断到 `top_k`。默认 2.0 补偿 HNSW 近似误差
/// - `precise_rerank`: 是否用 `CLV::cosine_similarity` 精确重排。
///   设为 `false` 时直接用 HNSW score（性能优先，精度降低）
#[derive(Debug, Clone, PartialEq)]
pub struct FineRecallConfig {
    /// over-fetch 因子（默认 2.0，HNSW 检索 top_k × 2 个候选）
    pub overfetch_factor: f32,
    /// 是否启用精确 CLV 重排（默认 true）
    pub precise_rerank: bool,
}

impl Default for FineRecallConfig {
    fn default() -> Self {
        Self {
            overfetch_factor: 2.0,
            precise_rerank: true,
        }
    }
}

// ============================================================
// 错误类型
// ============================================================

/// 召回错误 — 覆盖粗召回/精排/重排填充全流水线
///
/// # 设计决策（WHY）
/// - 用 `thiserror` 派生（库层错误标准，§4.1）
/// - 错误粒度按"信号源"分类（依赖图/语义/共变更），便于定位问题
/// - `InvalidWeights` 在 `RecallWeights::is_valid() == false` 时返回
/// - `VectorStoreError`（P3-W9.2 新增）包装底层 VectorStore 错误（如 HnswStore WikiError）
#[derive(Debug, thiserror::Error, PartialEq)]
pub enum RecallError {
    /// Project 图未构建（粗召回前必须先注入 `ModuleGraph`）
    #[error("module graph not built, call register_graph() first")]
    GraphNotBuilt,

    /// 共变更矩阵未构建（粗召回前必须先注入 `CoChangeMatrix`）
    #[error("co-change matrix not built, call register_cochange() first")]
    CoChangeNotBuilt,

    /// 种子模块为空（粗召回至少需要一个种子）
    #[error("seed modules is empty, at least one seed required")]
    EmptySeeds,

    /// 权重非法（三权重之和偏离 1.0 超过容差）
    #[error("invalid weights: sum = {sum:.4}, expected 1.0 ± 0.01")]
    InvalidWeights {
        /// 实际权重之和
        sum: f32,
    },

    /// 候选模块 CLV 缺失（输入 `module_clvs` 未覆盖全部节点）
    #[error("module CLV missing for module: {module_id}")]
    MissingModuleClv {
        /// 缺失 CLV 的模块 ID
        module_id: ModuleId,
    },

    /// Top-K 超过候选总数（合法但需提醒）
    #[error("top_k {top_k} exceeds candidate count {candidate_count}, returning all")]
    TopKExceedsCandidates {
        /// 请求的 Top-K
        top_k: usize,
        /// 实际候选总数
        candidate_count: usize,
    },

    /// VectorStore 检索失败（P3-W9.2 精排新增）
    ///
    /// 包装底层 VectorStore 实现的错误（如 `HnswStore::WikiError`），
    /// 用 `String` 存储错误信息以解耦具体 Error 类型（关联类型 `S::Error` 无法直接持有）。
    #[error("vector store error: {0}")]
    VectorStoreError(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recall_weights_default() {
        let w = RecallWeights::default();
        assert_eq!(w.dependency, 0.4);
        assert_eq!(w.semantic, 0.3);
        assert_eq!(w.cochange, 0.3);
        assert!(w.is_valid());
    }

    #[test]
    fn test_recall_weights_invalid_sum() {
        // 三权重之和 = 0.9，偏离 1.0 超过 0.01 容差
        let w = RecallWeights::new(0.4, 0.3, 0.2);
        assert!(!w.is_valid());
    }

    #[test]
    fn test_recall_weights_zero_signal_allowed() {
        // 单信号为 0 合法（允许禁用信号）
        let w = RecallWeights::new(0.0, 0.5, 0.5);
        assert!(w.is_valid());
    }

    #[test]
    fn test_module_graph_from_edges() {
        let graph = ModuleGraph::from_edges(
            vec![
                ("a".into(), "b".into()),
                ("a".into(), "c".into()),
                ("b".into(), "c".into()),
            ],
            vec!["d".into()], // 孤立节点
        );

        assert_eq!(graph.node_count(), 4);
        assert_eq!(graph.edge_count(), 3);
        assert!(graph.contains("d"));

        // 依赖（out-edges）
        let a_deps = graph.dependencies("a");
        assert_eq!(a_deps.len(), 2);
        assert!(a_deps.contains(&"b".to_string()));
        assert!(a_deps.contains(&"c".to_string()));

        // 被依赖（in-edges）
        let c_dependents = graph.dependents("c");
        assert_eq!(c_dependents.len(), 2);
    }

    #[test]
    fn test_module_graph_missing_node_returns_empty() {
        let graph = ModuleGraph::from_edges(vec![], vec![]);
        // 不存在的节点依赖列表为空（非错误）
        assert!(graph.dependencies("nonexistent").is_empty());
        assert!(graph.dependents("nonexistent").is_empty());
        assert!(!graph.contains("nonexistent"));
    }

    #[test]
    fn test_cochange_matrix_record_and_query() {
        let mut matrix = CoChangeMatrix::new();
        matrix.record("a".into(), "b".into());
        matrix.record("a".into(), "b".into()); // 第二次共变更
        matrix.record("a".into(), "c".into());

        assert_eq!(matrix.count("a", "b"), 2);
        assert_eq!(matrix.count("a", "c"), 1);
        assert_eq!(matrix.count("b", "a"), 2); // 对称
        assert_eq!(matrix.count("a", "d"), 0); // 无记录
    }

    #[test]
    fn test_cochange_matrix_score_normalization() {
        let matrix = CoChangeMatrix::from_pairs(vec![
            ("a".into(), "b".into(), 5),
            ("a".into(), "c".into(), 10), // max
            ("a".into(), "d".into(), 0),  // 不记录
        ]);

        // max_count = 10，b 的分数 = 5/10 = 0.5
        let score_b = matrix.cochange_score("b", &["a".into()]);
        assert!((score_b - 0.5).abs() < 1e-5);

        // c 的分数 = 10/10 = 1.0
        let score_c = matrix.cochange_score("c", &["a".into()]);
        assert!((score_c - 1.0).abs() < 1e-5);

        // 多种子取最大值
        let score_multi = matrix.cochange_score("b", &["a".into(), "x".into()]);
        assert!((score_multi - 0.5).abs() < 1e-5);
    }

    #[test]
    fn test_cochange_matrix_empty_returns_zero() {
        let matrix = CoChangeMatrix::new();
        let score = matrix.cochange_score("a", &["b".into()]);
        assert_eq!(score, 0.0);
    }

    #[test]
    fn test_cochange_matrix_self_record_ignored() {
        let mut matrix = CoChangeMatrix::new();
        matrix.record("a".into(), "a".into()); // 自共变更忽略
        assert_eq!(matrix.pair_count(), 0);
    }

    #[test]
    fn test_module_score_new() {
        let score = ModuleScore::new("module-1", 0.85, 0.9, 0.8, 0.7);
        assert_eq!(score.module_id, "module-1");
        assert!((score.score - 0.85).abs() < 1e-5);
        assert!((score.dep_score - 0.9).abs() < 1e-5);
    }

    #[test]
    fn test_recall_error_display() {
        let err = RecallError::EmptySeeds;
        assert!(err.to_string().contains("seed modules is empty"));

        let err = RecallError::InvalidWeights { sum: 0.9 };
        assert!(err.to_string().contains("0.9000"));
    }
}
