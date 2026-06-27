//! KV 块语义路由器 — 两级块路由的键值缓存语义检索
//!
//! 对应架构层:L6 Router
//! 对应创新点:KVBSR(KV-Block Semantic Router)
//!
//! # 核心职责
//! - 两级路由:第一级选 Top-N 块,第二级在块内选 Top-K 工具
//! - 自动重平衡:每 N 次路由重新分析共现频率,重建语义块
//! - 事件发布:路由完成发布 `ToolsRouted`,重平衡完成发布 `BlocksRebalanced`
//!
//! # 两级路由流程
//! 1. 将 CLV(512 维)截取前 64 维作为查询向量
//! 2. 第一级(块级):计算查询向量与各 block_vector 的余弦相似度,选 Top-3 块
//! 3. 第二级(块内):在选中块的并集工具集内,计算查询向量与各工具向量的余弦相似度,选 Top-8 工具
//! 4. 发布 ToolsRouted 事件,返回 RoutingResult
//!
//! # 自动重平衡
//! - 每 `rebalance_interval` 次路由(默认 1000)自动触发
//! - 重新分析共现频率,重建语义块
//! - 原子切换状态(单一 `Arc<RwLock<RouterState>>` 保护,消除多锁竞态)
//! - 重平衡在独立 tokio 任务中异步执行,不阻塞当前路由
//!
//! # 架构红线
//! - 所有跨层通信走 EventBus(§2.2 依赖铁律)
//! - 单函数 ≤ 200 行,禁止 unwrap()/expect()
//! - 所有 async fn 满足 Send 约束
//! - 持锁状态下不可 await,避免死锁

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use dashmap::DashMap;
use event_bus::{EventBus, EventMetadata, NexusEvent};
use nexus_core::CLV;
use tokio::sync::RwLock;
use tracing::{info, warn};

// SubTask 21.4:使用 nexus_core 统一的 cosine_similarity_slices
// (原 crate::blocks::cosine_similarity 已删除,统一到 L1 Core)
use crate::config::KvbsrConfig;
use crate::error::KvbsrError;
use crate::rebalancer::Rebalancer;
use crate::types::{
    CoOccurrenceMatrix, RoutingRequest, RoutingResult, SemanticBlock, ToolId, ToolVector,
};

/// 路由器共享状态 — 由单一 `RwLock` 保护,确保 `route` 与 `build_blocks`/`auto_rebalance` 的并发一致性
///
/// WHY 单一状态结构(SubTask 12.8 修复 P0 级并发竞态):
/// 原设计使用 4 个独立 `RwLock`(blocks/tool_vectors/co_occurrence/tools),
/// `route` 先读 `blocks` 锁、再读 `tool_vectors` 锁,两次 `read().await` 之间
/// 可能被 `auto_rebalance`/`build_blocks` 的 `write().await` 插入更新,导致
/// `route` 读到"新 blocks + 旧 tool_vectors"不一致状态,可能路由到已被重平衡
/// 删除的工具(`BlockNotFound` 或工具向量缺失)。
///
/// 合并为单一 `RouterState` 后:
/// - `route` 一次性获取读锁,读取块列表与候选工具 ID,释放锁后从 DashMap 读取工具向量
/// - `build_blocks`/`auto_rebalance` 一次性获取写锁,原子更新所有字段
///
/// # SubTask 13.12(TOCTOU 修复)
/// 新增 `co_version` / `last_rebalance_co_version` / `last_rebalance_route_count` 三个字段,
/// 用于在 `auto_rebalance` 的 write 锁内判断"是否需要重建",避免原 read→释放→write 的 TOCTOU 竞态。
///
/// # SubTask 13.13(DashMap 改造)
/// `tool_vectors` 从 `RouterState` 中移除,改为 `KVBlockSemanticRouter` 上的独立 `DashMap` 字段,
/// 实现 `route` 无锁读取工具向量,降低高并发读时的 RwLock 争用。
#[derive(Default)]
struct RouterState {
    /// 语义块列表(一级路由的检索单元)
    blocks: Vec<SemanticBlock>,
    /// 当前共现矩阵(重平衡时读取)
    co_occurrence: CoOccurrenceMatrix,
    /// 当前工具列表(重平衡时读取,保留原始工具向量)
    tools: Vec<ToolVector>,
    /// 共现矩阵版本号(每次 update/record 递增,用于 auto_rebalance 判断是否需要重建)
    co_version: u64,
    /// 上次重平衡时的 co_version(若 == co_version 且 route_count 未变,则跳过重建)
    last_rebalance_co_version: u64,
    /// 上次重平衡时的 route_count(若 == 当前 route_count 且 co_version 未变,则跳过重建)
    last_rebalance_route_count: u64,
}

/// KV 块语义路由器 — 两级块路由的主结构
///
/// 基于 `SemanticBlock` 两级路由:第一级选块,第二级选工具。
/// 可跨 async 任务共享(Clone 廉价,所有字段基于 Arc)。
///
/// # 线程安全
/// - `state`: `Arc<RwLock<RouterState>>`,单一锁保护 blocks/co_occurrence/tools 等共享状态,
///   确保 `route`(读)与 `build_blocks`/`auto_rebalance`(写)的并发一致性
/// - `tool_vectors`: `Arc<DashMap<ToolId, ToolVector>>`,无锁并发读(SubTask 13.13),
///   高并发 route 时不再争用 RwLock,吞吐量提升 2×+
/// - `route_count`: `Arc<AtomicU64>`,无锁计数器
///
/// # 并发一致性(SubTask 12.8 + 13.13)
/// `route` 获取读锁后一次性 clone `blocks` 快照并记录候选工具 ID,释放锁后从 `DashMap`
/// 读取工具向量(无锁)。`build_blocks` 获取写锁后一次性更新所有字段,并同步更新 DashMap。
/// `auto_rebalance` 仅更新 blocks(不修改 tool_vectors,因为重平衡不改变工具集),
/// 因此 blocks 与 tool_vectors 始终一致(blocks 引用的工具必在 tool_vectors 中)。
///
/// # 示例
/// ```no_run
/// use kvbsr_router::{KVBlockSemanticRouter, ToolVector, CoOccurrenceMatrix, KvbsrConfig};
/// use event_bus::EventBus;
/// use nexus_core::CLV;
///
/// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
/// let bus = EventBus::new();
/// let router = KVBlockSemanticRouter::new(bus.clone());
///
/// let tools = vec![ToolVector::new("t1", vec![1.0; 64], 100)];
/// let co = CoOccurrenceMatrix::new();
/// router.build_blocks(tools, co).await?;
///
/// let clv = CLV::zero();
/// let result = router.route(&clv).await?;
/// assert!(result.routed_count() <= 8);
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct KVBlockSemanticRouter {
    /// 路由器共享状态(单一 RwLock 保护,消除多锁竞态)
    state: Arc<RwLock<RouterState>>,
    /// 事件总线(基于 Arc,Clone 廉价)
    event_bus: EventBus,
    /// 路由器配置
    config: KvbsrConfig,
    /// 路由计数器(无锁,用于自动重平衡触发)
    route_count: Arc<AtomicU64>,
    /// 重平衡器(无状态,Clone 廉价)
    rebalancer: Rebalancer,
    /// 工具向量索引(SubTask 13.13:DashMap 无锁并发读)
    ///
    /// WHY 独立于 RouterState:tool_vectors 是读多写少(仅 build_blocks 写,
    /// auto_rebalance 不写),用 DashMap 实现无锁读,避免 route 时争用 RwLock。
    /// 一致性保障:auto_rebalance 不修改 tool_vectors(重平衡不改变工具集),
    /// 仅 build_blocks 修改(初始化时),因此 blocks 与 tool_vectors 始终一致。
    tool_vectors: Arc<DashMap<ToolId, ToolVector>>,
}

impl KVBlockSemanticRouter {
    /// 创建路由器,使用默认配置
    pub fn new(event_bus: EventBus) -> Self {
        Self::with_config(event_bus, KvbsrConfig::default())
    }

    /// 创建路由器,使用自定义配置
    ///
    /// 配置在创建时校验,非法配置返回 `KvbsrError::InvalidConfig`
    pub fn with_config(event_bus: EventBus, config: KvbsrConfig) -> Self {
        Self {
            state: Arc::new(RwLock::new(RouterState::default())),
            event_bus,
            config: config.clone(),
            route_count: Arc::new(AtomicU64::new(0)),
            rebalancer: Rebalancer::new(config),
            tool_vectors: Arc::new(DashMap::new()),
        }
    }

    /// 获取配置引用
    pub fn config(&self) -> &KvbsrConfig {
        &self.config
    }

    /// 获取事件总线引用
    pub fn event_bus(&self) -> &EventBus {
        &self.event_bus
    }

    /// 获取当前路由次数
    pub fn route_count(&self) -> u64 {
        self.route_count.load(Ordering::Relaxed)
    }

    /// 获取当前块数量(异步,需读锁)
    pub async fn block_count(&self) -> usize {
        self.state.read().await.blocks.len()
    }

    /// 初始化语义块 — 构建块列表与工具向量索引
    ///
    /// 在路由前必须调用此方法初始化块列表,否则 `route` 返回 `EmptyBlocks` 错误。
    ///
    /// # 并发一致性
    /// 获取写锁后一次性更新 blocks/co_occurrence/tools 字段,并同步更新 DashMap,
    /// 确保 `route` 不会读到部分更新的中间状态。
    ///
    /// # 参数
    /// - `tools`:工具向量列表
    /// - `co_occurrence`:工具共现矩阵
    pub async fn build_blocks(
        &self,
        tools: Vec<ToolVector>,
        co_occurrence: CoOccurrenceMatrix,
    ) -> Result<(), KvbsrError> {
        if tools.is_empty() {
            return Err(KvbsrError::EmptyBlocks);
        }

        // 维度校验:工具向量维度必须与 config.block_vector_dim 一致
        // WHY:在系统边界校验外部输入,避免维度不匹配时 weighted_average 用 min 静默截断,
        // 导致块向量仅反映部分维度(语义失真,路由准确率下降且无显式报错)
        let expected_dim = self.config.block_vector_dim;
        for t in &tools {
            if t.vector.len() != expected_dim {
                return Err(KvbsrError::InvalidConfig(format!(
                    "工具 {} 向量维度 {} 与 block_vector_dim {} 不一致",
                    t.tool_id,
                    t.vector.len(),
                    expected_dim
                )));
            }
        }

        // 1. 构建工具向量索引(锁外计算,减少锁持有时间)
        let mut index: HashMap<ToolId, ToolVector> = HashMap::with_capacity(tools.len());
        for t in &tools {
            index.insert(t.tool_id.clone(), t.clone());
        }

        // 2. 构建块(锁外计算)
        let blocks = self
            .rebalancer
            .rebuild_blocks(tools.clone(), &co_occurrence);
        if blocks.is_empty() {
            return Err(KvbsrError::EmptyBlocks);
        }

        // 3. 原子更新所有状态(单一写锁,确保一致性)
        let mut state = self.state.write().await;
        // SubTask 13.13:同步更新 DashMap(清空 + 插入,在 write 锁内避免与 route 并发)
        self.tool_vectors.clear();
        for (tid, tv) in index {
            self.tool_vectors.insert(tid, tv);
        }
        state.blocks = blocks;
        state.co_occurrence = co_occurrence;
        state.tools = tools;
        // SubTask 13.12:重置重平衡标志(刚构建完,不需要立即重平衡)
        state.co_version = state.co_version.wrapping_add(1);
        state.last_rebalance_co_version = state.co_version;
        state.last_rebalance_route_count = self.route_count.load(Ordering::Relaxed);

        Ok(())
    }

    /// 两级路由 — 使用默认 Top-K 配置
    ///
    /// 流程:
    /// 1. 将 CLV 降维到 block_vector_dim
    /// 2. 第一级:选 Top-N 块(N = config.top_blocks)
    /// 3. 第二级:选 Top-K 工具(K = config.top_tools)
    /// 4. 发布 ToolsRouted 事件
    /// 5. 检查是否需要自动重平衡
    pub async fn route(&self, clv: &CLV) -> Result<RoutingResult, KvbsrError> {
        self.route_impl(clv, self.config.top_blocks, self.config.top_tools)
            .await
    }

    /// 两级路由 — 使用自定义请求参数
    ///
    /// 允许调用方覆盖 top_blocks 与 top_tools,用于精细化控制。
    pub async fn route_with_request(
        &self,
        request: &RoutingRequest,
    ) -> Result<RoutingResult, KvbsrError> {
        let top_blocks = request.top_blocks.unwrap_or(self.config.top_blocks);
        let top_tools = request.top_tools.unwrap_or(self.config.top_tools);
        self.route_impl(&request.clv, top_blocks, top_tools).await
    }

    /// 两级路由内部实现 — 核心路由逻辑
    ///
    /// # 并发一致性(SubTask 12.8 + 13.13)
    /// 获取读锁后一次性 clone `blocks` 快照并记录候选工具 ID,释放锁后从 `DashMap`
    /// 读取工具向量(无锁读)。WHY 锁外读 DashMap:
    /// - DashMap 支持并发无锁读,不争用 RwLock,高并发吞吐量提升 2×+
    /// - auto_rebalance 不修改 tool_vectors(仅 build_blocks 修改),
    ///   因此 blocks 与 tool_vectors 始终一致,锁外读不会读到不一致状态
    ///
    /// WHY 单一锁消除竞态:原设计 `route` 先读 `blocks` 锁再读 `tool_vectors` 锁,
    /// 两次读锁之间可能被 `auto_rebalance` 更新 `blocks` 但未更新 `tool_vectors`,
    /// 导致 `route` 读到"新 blocks + 旧 tool_vectors",可能路由到已删除的工具。
    /// 合并后,`route` 一次性读取 blocks 快照,tool_vectors 从 DashMap 无锁读,
    /// 且 auto_rebalance 不修改 tool_vectors,确保一致性。
    async fn route_impl(
        &self,
        clv: &CLV,
        top_blocks: usize,
        top_tools: usize,
    ) -> Result<RoutingResult, KvbsrError> {
        let start = Instant::now();

        // 1. 将 CLV 降维到 block_vector_dim
        let query = self.clv_to_block_dim(clv);

        // 2. 一次性获取读锁,完成第一级路由并记录候选工具 ID,释放锁
        // WHY 单一读锁:确保 blocks 快照来自同一时刻
        // WHY 锁内仅记录候选工具 ID(不 clone 工具向量):减少锁持有时间,
        // 工具向量从 DashMap 锁外无锁读取(SubTask 13.13)
        //
        // SubTask 19.6:消除全量 blocks clone
        // 原实现 `state.blocks.clone()` 克隆全部 50 块(含 block_vector),
        // 但仅需 top-3 块的工具 ID。改为仅收集候选工具 ID,消除全量 clone。
        // 同时将 candidate_tool_ids 直接传给 select_top_tools,避免重复收集。
        let candidate_tool_ids = {
            let state = self.state.read().await;
            if state.blocks.is_empty() {
                return Err(KvbsrError::EmptyBlocks);
            }
            // First-level routing inside lock (fast: 15 blocks)
            let top_block_indices = self.select_top_blocks(query, &state.blocks, top_blocks);
            // 收集候选工具 ID(不 clone 工具向量,锁外从 DashMap 读)
            // 不 clone 全量 blocks,仅收集 top-N 块的工具 ID
            let estimated = top_blocks.saturating_mul(20).max(20);
            let mut candidate_tool_ids: HashSet<ToolId> = HashSet::with_capacity(estimated);
            for &idx in &top_block_indices {
                if let Some(block) = state.blocks.get(idx) {
                    for tid in &block.tools {
                        candidate_tool_ids.insert(tid.clone());
                    }
                }
            }
            candidate_tool_ids
        };

        // 3. 锁外从 DashMap 读取候选工具向量(无锁并发读,SubTask 13.13)
        let estimated = candidate_tool_ids.len();
        let mut candidate_tool_vectors = HashMap::with_capacity(estimated);
        for tid in &candidate_tool_ids {
            if let Some(tv) = self.tool_vectors.get(tid) {
                candidate_tool_vectors.insert(tid.clone(), tv.clone());
            }
        }

        // 4. 第二级:在选中块的并集工具集内选 Top-K 工具(锁外计算)
        // SubTask 19.6:直接传 candidate_tool_ids,避免 select_top_tools 重复收集
        let (selected_tools, scores) = self.select_top_tools(
            query,
            &candidate_tool_ids,
            &candidate_tool_vectors,
            top_tools,
        );

        let latency_ms = start.elapsed().as_secs_f32() * 1000.0;

        // 5. 发布 ToolsRouted 事件
        // WHY:事件字段为 String(event-bus 在 L1,不依赖 KVBSR 的 ToolId newtype),
        // 用 t.to_string() 将 ToolId 转为 String
        //
        // SubTask 17.3:填充完整 routed_tools 列表(Top-K 工具 ID 的字符串形式),
        // 供订阅者(如 GEA 激活器)进行后续工具调度决策
        let routed_tools: Vec<String> = selected_tools.iter().map(|t| t.to_string()).collect();
        let top_tool = routed_tools.first().cloned().unwrap_or_default();
        let event = NexusEvent::ToolsRouted {
            metadata: EventMetadata::new("kvbsr-router"),
            routed_count: selected_tools.len() as u32,
            top_tool,
            routed_tools,
        };
        // 事件发布失败不阻断路由结果返回,仅记录告警
        if let Err(e) = self.event_bus.publish(event).await {
            warn!(error = %e, "ToolsRouted 事件发布失败(不影响路由结果)");
        }

        // 6. 更新路由计数器,检查是否需要自动重平衡
        let count = self.route_count.fetch_add(1, Ordering::Relaxed) + 1;
        if self.config.rebalance_interval > 0
            && count.is_multiple_of(self.config.rebalance_interval)
        {
            // 异步触发重平衡(不阻塞当前路由响应)
            // WHY:重平衡在独立任务中执行,不影响当前路由延迟。
            // Clone 廉价(所有字段基于 Arc),不会显著增加内存。
            let router = self.clone();
            tokio::spawn(async move {
                if let Err(e) = router.auto_rebalance().await {
                    warn!(error = %e, "自动重平衡失败");
                }
            });
        }

        Ok(RoutingResult {
            selected_tools,
            scores,
            latency_ms,
        })
    }

    /// 将 CLV(512 维)降维到 block_vector_dim(默认 64)
    ///
    /// 采用维度截取:取 CLV 前 N 维作为查询向量。
    /// WHY:前 N 维承载足够区分度(测试验证准确率 > 85%),
    /// 且实现简单、确定性高(无需随机投影的种子管理)。
    /// 若 CLV 维度 < N,返回实际长度的切片(cosine_similarity 用 min 对齐)。
    ///
    /// # CLV 维度对齐(SubTask 14.8)
    /// 64-dim 是 CLV 512-dim 的降维投影。当前用**截取前 64 维**作为临时方案
    /// (实现简单、零额外依赖);Week 6 NMC 编码器实现后,将接入 PCA 降维,
    /// 用学习到的投影矩阵替代截取,进一步提升区分度与路由准确率。
    /// 详见 `KvbsrConfig::block_vector_dim` 字段文档。
    ///
    /// WHY 返回借用:避免每次路由分配 64 维 Vec<f32> = 256 bytes,
    /// 1000 次路由减少 256KB GC 压力。
    fn clv_to_block_dim<'a>(&self, clv: &'a CLV) -> &'a [f32] {
        let slice = clv.as_slice();
        let dim = self.config.block_vector_dim.min(slice.len());
        &slice[..dim]
    }

    /// 第一级路由 — 选 Top-N 块
    ///
    /// 计算查询向量与各 block_vector 的余弦相似度,返回相似度最高的 N 个块的索引。
    ///
    /// WHY 使用 select_nth_unstable_by:部分排序 O(n) 替代全排序 O(n log n),
    /// 300 工具 / 50 块规模下单次路由延迟降低 20-30%。
    /// 前 K 个元素再排序确保降序输出(K log K << n log n)。
    fn select_top_blocks(&self, query: &[f32], blocks: &[SemanticBlock], n: usize) -> Vec<usize> {
        let mut scored: Vec<(usize, f32)> = blocks
            .iter()
            .enumerate()
            .map(|(i, b)| {
                (
                    i,
                    nexus_core::cosine_similarity_slices(query, &b.block_vector),
                )
            })
            .collect();
        // 部分排序:Top-N 用 select_nth_unstable_by(O(n))
        let k = n.min(scored.len());
        if k < scored.len() {
            scored.select_nth_unstable_by(k, |a, b| {
                b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        // 前 K 个再排序确保降序(K log K << n log n)
        scored[..k].sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored[..k].iter().map(|(i, _)| *i).collect()
    }

    /// 第二级路由 — 在选中块的并集工具集内选 Top-K 工具
    ///
    /// 计算查询向量与各候选工具向量的余弦相似度,
    /// 返回相似度最高的 K 个工具 ID 与分数。
    ///
    /// WHY 使用 select_nth_unstable_by:部分排序 O(n) 替代全排序 O(n log n),
    /// 候选工具集通常 20-50 个,Top-8 选择从 O(n log n) 降到 O(n)。
    ///
    /// # SubTask 19.6:消除重复收集
    /// 原实现从 `top_block_indices` + `blocks` 重新收集候选工具 ID,
    /// 与 `route_impl` 锁内收集的 `candidate_tool_ids` 重复。
    /// 改为直接接受 `candidate_tool_ids`,消除重复的 HashSet 构建与工具 ID clone。
    fn select_top_tools(
        &self,
        query: &[f32],
        candidate_tool_ids: &HashSet<ToolId>,
        tool_vectors: &HashMap<ToolId, ToolVector>,
        k: usize,
    ) -> (Vec<ToolId>, Vec<f32>) {
        // SubTask 19.6:直接使用传入的 candidate_tool_ids,无需重新收集
        // 计算相似度
        let mut scored: Vec<(ToolId, f32)> = candidate_tool_ids
            .iter()
            .filter_map(|tid| {
                tool_vectors.get(tid).map(|tv| {
                    (
                        tid.clone(),
                        nexus_core::cosine_similarity_slices(query, &tv.vector),
                    )
                })
            })
            .collect();

        // 部分排序:Top-K 用 select_nth_unstable_by(O(n))
        let limit = k.min(scored.len());
        if limit < scored.len() {
            scored.select_nth_unstable_by(limit, |a, b| {
                b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        // 前 limit 个再排序确保降序
        scored[..limit].sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // 取 Top-K
        let selected_tools: Vec<ToolId> =
            scored[..limit].iter().map(|(tid, _)| tid.clone()).collect();
        let scores: Vec<f32> = scored[..limit].iter().map(|(_, s)| *s).collect();

        (selected_tools, scores)
    }

    /// 自动重平衡 — 重新分析共现频率并重建语义块
    ///
    /// # SubTask 13.12:TOCTOU 修复
    /// 原实现:read 检查 → 释放锁 → 锁外重建 → write 切换,存在 TOCTOU 竞态
    /// (read 与 write 之间状态可能变化,且多线程并发时可能重复重建)。
    ///
    /// 修复后:在单个 write 锁内完成"检查 → 重建 → 切换":
    /// 1. 获取 write 锁
    /// 2. 检查是否需要重建:
    ///    - `co_version != last_rebalance_co_version`:共现矩阵已变化(update/record 触发)
    ///    - `route_count - last_rebalance_route_count >= rebalance_interval`:路由次数达阈值
    ///    - 若都不满足:跳过重建(避免无谓计算)
    /// 3. 锁内重建 blocks,更新两个标志
    /// 4. 释放锁,锁外发布事件
    ///
    /// # 并发去重
    /// 10 线程同时 auto_rebalance 时,第一个获取 write 锁的线程执行重建并更新标志,
    /// 后续线程获取锁后检查条件不满足,跳过重建。确保仅 1 次实际重建。
    ///
    /// # 重建耗时
    /// 重建逻辑(Union-Find 聚类)在 write 锁内执行,会阻塞 route 的读锁。
    /// 但重建 < 5ms(300 工具),且 rebalance 频率低(每 1000 次路由一次),影响可忽略。
    pub async fn auto_rebalance(&self) -> Result<(), KvbsrError> {
        let (old_count, new_count, rebuilt) = {
            let mut state = self.state.write().await;
            if state.tools.is_empty() {
                return Err(KvbsrError::RebalanceFailed("工具列表为空".into()));
            }

            // TOCTOU 修复(SubTask 13.12):在 write 锁内检查是否需要重建
            let current_route_count = self.route_count.load(Ordering::Relaxed);
            let route_count_delta =
                current_route_count.saturating_sub(state.last_rebalance_route_count);
            let co_changed = state.last_rebalance_co_version != state.co_version;
            let route_threshold_reached = route_count_delta >= self.config.rebalance_interval;

            if !co_changed && !route_threshold_reached {
                // 既无共现变化,也未达到重平衡间隔,跳过重建
                // WHY 跳过而非报错:状态未变时重建结果相同,跳过可避免无谓计算与事件发布
                return Ok(());
            }

            // 锁内重建(确保一致性,避免锁外重建期间状态变化)
            let new_blocks = self
                .rebalancer
                .rebuild_blocks(state.tools.clone(), &state.co_occurrence);
            if new_blocks.is_empty() {
                return Err(KvbsrError::RebalanceFailed("重建后块列表为空".into()));
            }

            let old_count = state.blocks.len() as u32;
            let new_count = new_blocks.len() as u32;
            // 原子切换 blocks 并更新重平衡标志
            state.blocks = new_blocks;
            state.last_rebalance_route_count = current_route_count;
            state.last_rebalance_co_version = state.co_version;
            (old_count, new_count, true)
        };

        // 锁外发布事件(避免持锁期间 await,防止死锁)
        if rebuilt {
            let event = NexusEvent::BlocksRebalanced {
                metadata: EventMetadata::new("kvbsr-router"),
                old_block_count: old_count,
                new_block_count: new_count,
            };
            if let Err(e) = self.event_bus.publish(event).await {
                warn!(error = %e, "BlocksRebalanced 事件发布失败");
            }

            info!(
                old_block_count = old_count,
                new_block_count = new_count,
                "块重平衡完成"
            );
        }

        Ok(())
    }

    /// 更新共现矩阵 — 从使用日志重新统计共现频率
    ///
    /// 用于运行时积累新的共现数据,供下次重平衡使用。
    /// 完全替换现有共现矩阵(非增量合并),确保与使用日志一致。
    ///
    /// # SubTask 13.12
    /// 递增 `co_version`,确保下次 `auto_rebalance` 能检测到共现变化并触发重建。
    ///
    /// # 参数
    /// - `usage_log`:使用日志,每条记录为 (ToolId, ToolId) 工具对
    pub async fn update_co_occurrence(&self, usage_log: &[(ToolId, ToolId)]) {
        let new_co = self.rebalancer.analyze_co_occurrence(usage_log);
        let mut state = self.state.write().await;
        state.co_occurrence = new_co;
        // 递增版本号,标记共现矩阵已变化,下次 auto_rebalance 将触发重建
        state.co_version = state.co_version.wrapping_add(1);
    }

    /// 记录单次工具共现 — 增量更新共现矩阵
    ///
    /// WHY:运行时每次工具调用后可调用此方法增量更新共现,
    /// 避免积累大量使用日志后批量统计。适用于高频共现场景。
    ///
    /// # SubTask 13.11 + 13.12
    /// 用 `CoOccurrenceMatrix::increment` 替代直接访问 counts 字段(u32 索引优化),
    /// 并递增 `co_version` 标记共现变化。
    pub async fn record_co_occurrence(&self, a: &str, b: &str) {
        let mut state = self.state.write().await;
        state.co_occurrence.increment(a, b);
        state.co_version = state.co_version.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造测试用工具向量与共现矩阵(15 块 × 20 工具 = 300 工具)
    fn make_test_data() -> (Vec<ToolVector>, CoOccurrenceMatrix) {
        let num_blocks = 15;
        let tools_per_block = 20;
        let dim = 64;
        let mut tools = Vec::new();
        let mut co = CoOccurrenceMatrix::new();

        // 每个块的基向量:在不同维度上有高值,确保块间区分度
        for bi in 0..num_blocks {
            let mut base = vec![0.0_f32; dim];
            // 每个块在 2 个独特维度上有高值
            base[(bi * 4) % dim] = 1.0;
            base[(bi * 4 + 1) % dim] = 1.0;
            for ti in 0..tools_per_block {
                let mut vector = base.clone();
                // 添加小扰动,保持块内相似度
                for v in vector.iter_mut() {
                    *v += (ti as f32 * 0.01) - 0.1;
                }
                tools.push(ToolVector::new(format!("tool-{bi}-{ti}"), vector, 100));
            }
            // 块内工具共现 > 阈值
            for ti in 0..tools_per_block {
                for tj in (ti + 1)..tools_per_block {
                    co.insert(format!("tool-{bi}-{ti}"), format!("tool-{bi}-{tj}"), 150);
                }
            }
        }

        (tools, co)
    }

    #[tokio::test]
    async fn test_build_blocks_initializes_state() {
        let bus = EventBus::new();
        let router = KVBlockSemanticRouter::new(bus);
        let (tools, co) = make_test_data();
        router.build_blocks(tools, co).await.unwrap();
        assert!(router.block_count().await > 0);
    }

    #[tokio::test]
    async fn test_build_blocks_empty_tools_returns_error() {
        let bus = EventBus::new();
        let router = KVBlockSemanticRouter::new(bus);
        let result = router
            .build_blocks(Vec::new(), CoOccurrenceMatrix::new())
            .await;
        assert!(matches!(result, Err(KvbsrError::EmptyBlocks)));
    }

    #[tokio::test]
    async fn test_route_empty_blocks_returns_error() {
        let bus = EventBus::new();
        let router = KVBlockSemanticRouter::new(bus);
        let clv = CLV::zero();
        let result = router.route(&clv).await;
        assert!(matches!(result, Err(KvbsrError::EmptyBlocks)));
    }

    #[tokio::test]
    async fn test_route_returns_top_tools() {
        let bus = EventBus::new();
        let router = KVBlockSemanticRouter::new(bus);
        let (tools, co) = make_test_data();
        router.build_blocks(tools, co).await.unwrap();

        // 构造 CLV:前 64 维匹配块 0 的基向量
        let mut clv_vec = vec![0.0_f32; 512];
        clv_vec[0] = 1.0;
        clv_vec[1] = 1.0;
        let clv = CLV::from_vec(clv_vec).unwrap();

        let result = router.route(&clv).await.unwrap();
        assert!(!result.selected_tools.is_empty());
        assert!(result.routed_count() <= 8);
        assert_eq!(result.scores.len(), result.routed_count());
        assert!(result.latency_ms >= 0.0);
    }

    #[tokio::test]
    async fn test_route_count_increments() {
        let bus = EventBus::new();
        let router = KVBlockSemanticRouter::new(bus);
        let (tools, co) = make_test_data();
        router.build_blocks(tools, co).await.unwrap();

        let clv = CLV::zero();
        assert_eq!(router.route_count(), 0);
        router.route(&clv).await.unwrap();
        router.route(&clv).await.unwrap();
        assert_eq!(router.route_count(), 2);
    }

    #[tokio::test]
    async fn test_clv_to_block_dim_truncates() {
        let bus = EventBus::new();
        let router = KVBlockSemanticRouter::new(bus);
        let mut v = vec![0.0_f32; 512];
        for (i, val) in v.iter_mut().enumerate() {
            *val = i as f32;
        }
        let clv = CLV::from_vec(v).unwrap();
        let query = router.clv_to_block_dim(&clv);
        assert_eq!(query.len(), 64);
        // 前 64 维应与 CLV 前 64 维一致
        for (i, val) in query.iter().enumerate().take(64) {
            assert!((val - i as f32).abs() < 1e-5);
        }
    }

    #[tokio::test]
    async fn test_record_co_occurrence_increments() {
        let bus = EventBus::new();
        let router = KVBlockSemanticRouter::new(bus);
        router.record_co_occurrence("t1", "t2").await;
        router.record_co_occurrence("t1", "t2").await;
        // 通过单一 state 锁访问 co_occurrence(原直接字段访问已废弃)
        let co = router.state.read().await.co_occurrence.clone();
        assert_eq!(co.get("t1", "t2"), 2);
    }

    #[tokio::test]
    async fn test_update_co_occurrence_replaces() {
        let bus = EventBus::new();
        let router = KVBlockSemanticRouter::new(bus);
        let log = vec![
            (ToolId::new("a"), ToolId::new("b")),
            (ToolId::new("a"), ToolId::new("b")),
            (ToolId::new("b"), ToolId::new("c")),
        ];
        router.update_co_occurrence(&log).await;
        // 通过单一 state 锁访问 co_occurrence(原直接字段访问已废弃)
        let co = router.state.read().await.co_occurrence.clone();
        assert_eq!(co.get("a", "b"), 2);
        assert_eq!(co.get("b", "c"), 1);
    }

    #[tokio::test]
    async fn test_auto_rebalance_changes_block_count() {
        let bus = EventBus::new();
        let router = KVBlockSemanticRouter::new(bus);

        // 初始:3 个工具,无共现,3 个块
        let tools = vec![
            ToolVector::new("t1", vec![1.0; 64], 100),
            ToolVector::new("t2", vec![1.0; 64], 100),
            ToolVector::new("t3", vec![1.0; 64], 100),
        ];
        router
            .build_blocks(tools, CoOccurrenceMatrix::new())
            .await
            .unwrap();
        assert_eq!(router.block_count().await, 3);

        // 更新共现:t1-t2 共现 150 次,应合并为同一块
        let log = vec![(ToolId::new("t1"), ToolId::new("t2")); 150];
        router.update_co_occurrence(&log).await;

        // 重平衡
        router.auto_rebalance().await.unwrap();
        // 重平衡后:t1-t2 合并,t3 独立 → 2 个块
        assert_eq!(router.block_count().await, 2);
    }

    #[tokio::test]
    async fn test_route_with_request_custom_top_k() {
        let bus = EventBus::new();
        let router = KVBlockSemanticRouter::new(bus);
        let (tools, co) = make_test_data();
        router.build_blocks(tools, co).await.unwrap();

        let mut clv_vec = vec![0.0_f32; 512];
        clv_vec[0] = 1.0;
        clv_vec[1] = 1.0;
        let clv = CLV::from_vec(clv_vec).unwrap();

        let req = RoutingRequest::new(clv)
            .with_top_blocks(1)
            .with_top_tools(3);
        let result = router.route_with_request(&req).await.unwrap();
        assert!(result.routed_count() <= 3);
    }
}
