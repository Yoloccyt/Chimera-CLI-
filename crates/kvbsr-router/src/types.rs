//! KVBSR 核心领域类型 — 两级语义块路由的统一数据模型
//!
//! 对应架构层:L6 Router
//! 对应创新点:KVBSR(KV-Block Semantic Router)— 两级块路由的键值缓存语义检索
//!
//! # 类型职责
//! - `ToolId`:工具唯一标识(String 别名,与 OSA/FaaE 共享)
//! - `ToolVector`:工具向量(64-dim f32)+ 使用次数,块构建与二级路由的基础
//! - `SemanticBlock`:语义块(block_id + block_vector + tools + coherence)
//! - `CoOccurrenceMatrix`:工具共现矩阵,驱动块构建与重平衡
//! - `RoutingRequest`/`RoutingResult`:路由请求与结果
//!
//! # 设计决策(WHY)
//! - **ToolVector.vector 维度 = block_vector_dim(默认 64)**:
//!   低于 CLV 的 512 维,降低存储与计算成本。路由时从 CLV 截取前 64 维
//!   作为查询向量(见 `router::clv_to_block_dim`),前 64 维承载足够区分度
//! - **block_id 使用 UUIDv7 字符串**:时间有序,便于因果追踪与去重
//! - **CoOccurrenceMatrix 基于 HashMap**:稀疏存储,300 工具全量共现约 9 万条,
//!   实际共现远少于全量,HashMap 节省内存
//! - **RoutingResult 携带 latency_ms**:路由延迟是核心性能指标,需在结果中回传

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

// 使用 nexus_core 共享的 id_newtype! 宏(SubTask 21.1)
// WHY:消除与 mlc-engine / osa-coordinator 的 newtype 实现重复(约 50 行手动实现),
// 统一 ID 类型行为(Deref / AsRef / Borrow / From / Display / serde(transparent))
nexus_core::id_newtype!(ToolId, "工具唯一标识 — 与 OSA/FaaE 共享同一命名空间");

/// 工具向量 — 工具的语义表示,块构建与二级路由的基础单元
///
/// `vector` 维度由 `KvbsrConfig::block_vector_dim` 决定(默认 64)。
/// `usage_count` 作为块向量加权平均的权重,使用次数高的工具对块向量贡献更大。
///
/// # 设计决策
/// - 维度 64 而非 CLV 的 512:降低存储(300 工具 × 64 维 × 4 字节 ≈ 75KB)
///   与计算成本(两级路由总计算量 < 200 次余弦相似度)
/// - `usage_count` 作为权重:高频工具代表块的核心能力,应在块向量中占主导
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolVector {
    /// 工具唯一标识
    pub tool_id: ToolId,
    /// 工具语义向量(维度 = block_vector_dim,默认 64)
    pub vector: Vec<f32>,
    /// 工具使用次数(作为块向量加权平均的权重)
    pub usage_count: u32,
}

impl ToolVector {
    /// 创建新的工具向量
    ///
    /// # 参数
    /// - `tool_id`:工具唯一标识(接受 `ToolId`/`String`/`&str`,通过 `Into<ToolId>` 转换)
    /// - `vector`:语义向量(维度应与 `block_vector_dim` 一致)
    /// - `usage_count`:使用次数(权重)
    pub fn new(tool_id: impl Into<ToolId>, vector: Vec<f32>, usage_count: u32) -> Self {
        Self {
            tool_id: tool_id.into(),
            vector,
            usage_count,
        }
    }

    /// 获取向量维度
    pub fn dimension(&self) -> usize {
        self.vector.len()
    }
}

/// 语义块 — 共现频率高的工具聚类,一级路由的检索单元
///
/// 每个 SemanticBlock 含:
/// - `block_id`:UUIDv7 字符串,时间有序便于追踪
/// - `block_vector`:块内工具向量的加权平均(权重 = usage_count)
/// - `tools`:块内工具 ID 列表
/// - `block_coherence`:块内一致性 [0.0, 1.0],块内工具向量与块向量的平均余弦相似度
///
/// # 一致性含义
/// - `coherence ≈ 1.0`:块内工具高度相似,聚类紧凑
/// - `coherence ≈ 0.0`:块内工具差异大,聚类松散(可能需要重平衡)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SemanticBlock {
    /// 块唯一标识(UUIDv7 字符串,时间有序)
    pub block_id: String,
    /// 块向量(维度 = block_vector_dim,工具向量的加权平均)
    pub block_vector: Vec<f32>,
    /// 块内工具 ID 列表
    pub tools: Vec<ToolId>,
    /// 块内一致性 [0.0, 1.0],块内工具向量与块向量的平均余弦相似度
    pub block_coherence: f32,
}

impl SemanticBlock {
    /// 获取块内工具数量
    pub fn tool_count(&self) -> usize {
        self.tools.len()
    }

    /// 获取块向量维度
    pub fn dimension(&self) -> usize {
        self.block_vector.len()
    }

    /// 判断块内是否包含指定工具
    pub fn contains(&self, tool_id: &str) -> bool {
        self.tools.iter().any(|t| t.as_str() == tool_id)
    }
}

/// 工具 ID 注册表 — 将 String 工具 ID 映射为紧凑的 u32 索引(SubTask 13.11)
///
/// WHY:CoOccurrenceMatrix 的键原为 `(String, String)`,每个键占用约 80 字节
/// (两个 String 各 24 字节栈 + 内容堆分配)。300 工具全量共现约 44850 条,
/// 总内存约 7.2MB。改用 `(u32, u32)` 键后,每键 8 字节,总内存降至 1.8MB(4× 压缩)。
///
/// # 设计决策
/// - `map: HashMap<ToolId, u32>`:正向映射,ID → 索引,O(1) 查找
/// - `reverse: Vec<ToolId>`:反向映射,索引 → ID,O(1) 随机访问(Vec 比 HashMap 快)
/// - `register` 方法:若 ID 已存在返回已有索引,否则分配新索引(单调递增,不复用)
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ToolIdRegistry {
    /// 正向映射:ToolId → u32 索引
    map: HashMap<ToolId, u32>,
    /// 反向映射:u32 索引 → ToolId(Vec 索引访问 O(1))
    reverse: Vec<ToolId>,
}

impl ToolIdRegistry {
    /// 创建空注册表
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册工具 ID,返回对应的 u32 索引
    ///
    /// 若 ID 已存在,返回已有索引;否则分配新索引并返回。
    /// 索引单调递增(0, 1, 2, ...),不复用已删除的索引。
    pub fn register(&mut self, id: &str) -> u32 {
        if let Some(&idx) = self.map.get(id) {
            return idx;
        }
        let idx = self.reverse.len() as u32;
        let tool_id = ToolId::from(id);
        self.reverse.push(tool_id.clone());
        self.map.insert(tool_id, idx);
        idx
    }

    /// 查询工具 ID 的索引(不注册)
    ///
    /// 返回 None 表示 ID 未注册
    pub fn get(&self, id: &str) -> Option<u32> {
        self.map.get(id).copied()
    }

    /// 根据索引解析回工具 ID
    ///
    /// 返回 None 表示索引越界
    pub fn resolve(&self, idx: u32) -> Option<&ToolId> {
        self.reverse.get(idx as usize)
    }

    /// 获取已注册的工具数量
    pub fn len(&self) -> usize {
        self.reverse.len()
    }

    /// 判断注册表是否为空
    pub fn is_empty(&self) -> bool {
        self.reverse.is_empty()
    }
}

/// 工具共现矩阵 — 记录工具两两共现频率,驱动块构建与重平衡
///
/// 内部使用 `HashMap<(u32, u32), u32>` 稀疏存储,u32 索引由 `ToolIdRegistry` 分配。
/// Key 的两个索引按 (小, 大) 排列,确保 (A,B) 与 (B,A) 去重为同一键。
///
/// # 内存优化(SubTask 13.11)
/// 原设计 `HashMap<(String, String), u32>`,300 工具全量共现约 44850 条,内存约 7.2MB。
/// 改用 `HashMap<(u32, u32), u32>` + `ToolIdRegistry`,内存降至 1.8MB(4× 压缩)。
///
/// # 使用场景
/// - `BlockBuilder::build_blocks`:共现频率 > 阈值的工具归入同一块
/// - `Rebalancer::analyze_co_occurrence`:从使用日志统计共现频率
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct CoOccurrenceMatrix {
    /// 共现计数表,Key 为 (小索引, 大索引),Value 为共现次数
    counts: HashMap<(u32, u32), u32>,
    /// 工具 ID 注册表(String ↔ u32 映射)
    registry: ToolIdRegistry,
}

impl CoOccurrenceMatrix {
    /// 创建空共现矩阵
    pub fn new() -> Self {
        Self::default()
    }

    /// 从共现对列表构建共现矩阵
    ///
    /// # 参数
    /// - `pairs`:共现工具对列表(每对表示一次共现)
    pub fn from_pairs(pairs: &[(ToolId, ToolId)]) -> Self {
        let mut matrix = Self::new();
        for (a, b) in pairs {
            matrix.increment(a, b);
        }
        matrix
    }

    /// 增量递增一对工具的共现次数(SubTask 13.11)
    ///
    /// 内部用 registry 将 String 转为 u32 索引,键为 (小, 大)。
    /// 若工具 ID 未注册,自动注册。
    pub fn increment(&mut self, a: &str, b: &str) {
        let ia = self.registry.register(a);
        let ib = self.registry.register(b);
        let key = if ia <= ib { (ia, ib) } else { (ib, ia) };
        *self.counts.entry(key).or_insert(0) += 1;
    }

    /// 查询两个工具的共现次数
    ///
    /// 返回 0 表示无共现记录或工具未注册
    pub fn get(&self, a: &str, b: &str) -> u32 {
        // 用 get 而非 register,避免查询时修改状态
        match (self.registry.get(a), self.registry.get(b)) {
            (Some(ia), Some(ib)) => {
                let key = if ia <= ib { (ia, ib) } else { (ib, ia) };
                self.counts.get(&key).copied().unwrap_or(0)
            }
            _ => 0,
        }
    }

    /// 获取共现矩阵中的条目数
    pub fn len(&self) -> usize {
        self.counts.len()
    }

    /// 判断共现矩阵是否为空
    pub fn is_empty(&self) -> bool {
        self.counts.is_empty()
    }

    /// 插入或更新一对工具的共现次数
    ///
    /// WHY:测试与重平衡场景需手动构造共现矩阵,提供此方法简化操作
    pub fn insert(&mut self, a: impl Into<ToolId>, b: impl Into<ToolId>, count: u32) {
        let a = a.into();
        let b = b.into();
        let ia = self.registry.register(&a);
        let ib = self.registry.register(&b);
        let key = if ia <= ib { (ia, ib) } else { (ib, ia) };
        self.counts.insert(key, count);
    }

    /// 迭代共现对 — 返回 (ToolId_a, ToolId_b, count)
    ///
    /// WHY:内部用 u32 索引存储,外部需要 String 工具 ID。
    /// 提供此方法避免外部直接访问 registry 和 counts,保持封装。
    pub fn iter_pairs(&self) -> impl Iterator<Item = (&ToolId, &ToolId, u32)> {
        self.counts.iter().filter_map(move |((ia, ib), count)| {
            let a = self.registry.resolve(*ia)?;
            let b = self.registry.resolve(*ib)?;
            Some((a, b, *count))
        })
    }

    /// 获取工具 ID 注册表(只读访问)
    pub fn registry(&self) -> &ToolIdRegistry {
        &self.registry
    }

    /// 获取共现计数表的只读引用(内部用 u32 索引)
    ///
    /// WHY:内存占用测试需要读取 capacity 估算内存
    pub fn counts(&self) -> &HashMap<(u32, u32), u32> {
        &self.counts
    }
}

/// 路由请求 — 封装路由输入与可选参数
///
/// `route(&CLV)` 方法使用默认配置,`route_with_request` 方法支持自定义 Top-K。
/// 保留此类型为未来扩展(如风险等级、时间压力等路由因子)预留接口。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RoutingRequest {
    /// 查询 CLV(512 维,内部截取前 block_vector_dim 维)
    pub clv: nexus_core::CLV,
    /// 一级路由选取的块数(默认取 KvbsrConfig::top_blocks)
    pub top_blocks: Option<usize>,
    /// 二级路由选取的工具数(默认取 KvbsrConfig::top_tools)
    pub top_tools: Option<usize>,
}

impl RoutingRequest {
    /// 创建新的路由请求,使用默认 Top-K
    pub fn new(clv: nexus_core::CLV) -> Self {
        Self {
            clv,
            top_blocks: None,
            top_tools: None,
        }
    }

    /// 设置一级路由选取的块数
    pub fn with_top_blocks(mut self, n: usize) -> Self {
        self.top_blocks = Some(n);
        self
    }

    /// 设置二级路由选取的工具数
    pub fn with_top_tools(mut self, n: usize) -> Self {
        self.top_tools = Some(n);
        self
    }
}

/// 路由结果 — 两级路由选中的工具列表与延迟指标
///
/// # 字段说明
/// - `selected_tools`:按相似度降序排列的工具 ID 列表(长度 ≤ top_tools)
/// - `scores`:与 `selected_tools` 一一对应的相似度分数 [0.0, 1.0]
/// - `latency_ms`:路由总延迟(毫秒),含两级计算与事件发布
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RoutingResult {
    /// 选中的工具 ID 列表(按相似度降序)
    pub selected_tools: Vec<ToolId>,
    /// 对应的相似度分数 [0.0, 1.0]
    pub scores: Vec<f32>,
    /// 路由总延迟(毫秒)
    pub latency_ms: f32,
}

impl RoutingResult {
    /// 获取选中工具数量
    pub fn routed_count(&self) -> usize {
        self.selected_tools.len()
    }

    /// 获取 Top-1 工具(相似度最高的工具)
    pub fn top_tool(&self) -> Option<&ToolId> {
        self.selected_tools.first()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_vector_new() {
        let tv = ToolVector::new("tool-1", vec![0.1, 0.2, 0.3], 50);
        assert_eq!(tv.tool_id.as_str(), "tool-1");
        assert_eq!(tv.vector, vec![0.1, 0.2, 0.3]);
        assert_eq!(tv.usage_count, 50);
        assert_eq!(tv.dimension(), 3);
    }

    #[test]
    fn test_semantic_block_helpers() {
        let block = SemanticBlock {
            block_id: "blk-1".into(),
            block_vector: vec![0.0; 64],
            tools: vec!["t1".into(), "t2".into(), "t3".into()],
            block_coherence: 0.85,
        };
        assert_eq!(block.tool_count(), 3);
        assert_eq!(block.dimension(), 64);
        assert!(block.contains("t2"));
        assert!(!block.contains("t9"));
    }

    #[test]
    fn test_co_occurrence_matrix_from_pairs() {
        let pairs = vec![
            (ToolId::new("a"), ToolId::new("b")),
            (ToolId::new("b"), ToolId::new("a")), // 去重为 (a, b)
            (ToolId::new("a"), ToolId::new("b")),
        ];
        let matrix = CoOccurrenceMatrix::from_pairs(&pairs);
        // (a, b) 共现 3 次
        assert_eq!(matrix.get("a", "b"), 3);
        assert_eq!(matrix.get("b", "a"), 3); // 顺序无关
        assert_eq!(matrix.get("a", "c"), 0); // 无记录
        assert_eq!(matrix.len(), 1);
    }

    #[test]
    fn test_co_occurrence_matrix_insert() {
        let mut matrix = CoOccurrenceMatrix::new();
        matrix.insert("x", "y", 150);
        assert_eq!(matrix.get("x", "y"), 150);
        assert_eq!(matrix.get("y", "x"), 150);
        assert!(!matrix.is_empty());
    }

    #[test]
    fn test_tool_id_registry_basic() {
        let mut registry = ToolIdRegistry::new();
        assert!(registry.is_empty());

        // 注册新 ID,返回单调递增索引
        let idx_a = registry.register("tool-a");
        let idx_b = registry.register("tool-b");
        let idx_c = registry.register("tool-c");
        assert_eq!(idx_a, 0);
        assert_eq!(idx_b, 1);
        assert_eq!(idx_c, 2);
        assert_eq!(registry.len(), 3);

        // 重复注册同一 ID,返回已有索引(不新增)
        let idx_a_again = registry.register("tool-a");
        assert_eq!(idx_a_again, 0);
        assert_eq!(registry.len(), 3);
    }

    #[test]
    fn test_tool_id_registry_get_and_resolve() {
        let mut registry = ToolIdRegistry::new();
        let idx = registry.register("tool-x");

        // get 查询已注册 ID
        assert_eq!(registry.get("tool-x"), Some(idx));
        // get 查询未注册 ID
        assert_eq!(registry.get("tool-y"), None);

        // resolve 解析索引回 ID
        assert_eq!(registry.resolve(idx), Some(&ToolId::new("tool-x")));
        // resolve 越界索引
        assert_eq!(registry.resolve(999), None);
    }

    #[test]
    fn test_co_occurrence_matrix_increment() {
        let mut matrix = CoOccurrenceMatrix::new();
        // 增量递增
        matrix.increment("a", "b");
        matrix.increment("a", "b");
        matrix.increment("b", "a"); // 顺序无关
        assert_eq!(matrix.get("a", "b"), 3);
        assert_eq!(matrix.len(), 1);
    }

    #[test]
    fn test_co_occurrence_matrix_iter_pairs() {
        let mut matrix = CoOccurrenceMatrix::new();
        matrix.insert("a", "b", 100);
        matrix.insert("b", "c", 200);

        let mut pairs: Vec<(&ToolId, &ToolId, u32)> = matrix.iter_pairs().collect();
        pairs.sort_by_key(|p| p.2);
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0].2, 100);
        assert_eq!(pairs[1].2, 200);
    }

    #[test]
    fn test_routing_request_builders() {
        let clv = nexus_core::CLV::zero();
        let req = RoutingRequest::new(clv)
            .with_top_blocks(5)
            .with_top_tools(10);
        assert_eq!(req.top_blocks, Some(5));
        assert_eq!(req.top_tools, Some(10));
    }

    #[test]
    fn test_routing_result_helpers() {
        let result = RoutingResult {
            selected_tools: vec!["t1".into(), "t2".into()],
            scores: vec![0.9, 0.8],
            latency_ms: 1.5,
        };
        assert_eq!(result.routed_count(), 2);
        assert_eq!(result.top_tool(), Some(&ToolId::new("t1")));
    }
}
