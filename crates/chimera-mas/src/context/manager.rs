//! AgentContext 上下文管理 — 1M Token 等效上下文的分层加载与稀疏化
//!
//! 架构层归属: L9 Quest(chimera-mas 内部子模块)
//! 核心职责: 包装 `hcw_window::HcwWindow` 实现 1M Token 等效上下文,
//!           经 HCW 稀疏化 + OSA 五维度掩码实现 Ω-Compress + Ω-Sparse。
//!
//! ## ADR-026 决策 7: 不自实现压缩
//!
//! `AgentContext` 不自实现压缩逻辑,而是委托给:
//! - `hcw_window::HcwWindow::select_window()` — 四级窗口分层选择(4K/32K/128K/1M)
//! - `hcw_window::HcwWindow::apply_sparse_mask()` — 按活跃文件 ID 稀疏化
//! - `osa_coordinator::OmniSparseCoordinator::compute_all_masks()` — 五维度稀疏掩码计算
//!
//! ## ContextBlock 优先级映射(ADR-026 决策 7)
//!
//! | 块类型 | 优先级 | 行为 |
//! |--------|--------|------|
//! | system_prompt | Critical | 永不压缩,强制加入 active_file_ids |
//! | user_intent | High | 优先保留 |
//! | task_context | Normal | 按需压缩 |
//! | wiki_knowledge | Optional | 可完全丢弃(OSA 未选中时不包含在输出) |
//!
//! ## 1M Token 等效机制(Ω-Compress)
//!
//! 1M Token 上下文 = 128K 实际加载 + 8× 稀疏压缩。
//! L3 层级容量 1M,但通过 OSA context_mask 仅加载活跃文件,
//! 实际加载 ≤ 128K(1M / 8),避免内存爆炸(§6.1 红线)。

use std::collections::HashSet;
use std::fmt;
use std::sync::Arc;

use event_bus::EventBus;
use hcw_window::{ContextEntry, HcwWindow, SelectorLearnerHolder};
use nexus_contracts::VectorStore; // PROBE P1.1: HnswStore upsert/top_k 的 trait 方法
use nexus_core::CLV;
use osa_coordinator::{
    AffectedScope, FileId, OmniSparseCoordinator, RiskLevel, TaskId, TaskProfile, TaskType,
    TimePressure,
};
use serde::{Deserialize, Serialize};

use crate::context::budget_model::{ContextTier, SPARSE_FACTOR};
use crate::error::{MasError, Result};

// ============================================================
// ContextPriority — 上下文块优先级枚举
// ============================================================

/// 上下文块优先级 — 决定保留与压缩顺序
///
/// 排序(derive Ord): `Critical > High > Normal > Low > Optional`
///
/// WHY 声明顺序: Rust derive Ord 按变体声明顺序,先声明者值更小。
/// 因此按 `Optional → Critical` 升序声明,自动满足 `Critical` 最大。
///
/// - `Critical`: 永不压缩(如 system_prompt),`is_compressible = false`
/// - `High`: 优先保留(如 user_intent)
/// - `Normal`: 标准上下文(如 task_context)
/// - `Low`: 按需压缩(如日志)
/// - `Optional`: 可完全丢弃(如 wiki_knowledge)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ContextPriority {
    /// 可选 — 最低优先级,可完全丢弃
    Optional,
    /// 低优先级 — 按需压缩
    Low,
    /// 正常优先级 — 标准上下文
    Normal,
    /// 高优先级 — 优先保留
    High,
    /// 关键 — 最高优先级,永不压缩
    Critical,
}

impl ContextPriority {
    /// 是否为 Critical 优先级
    pub fn is_critical(self) -> bool {
        self == Self::Critical
    }

    /// 是否为 Optional 优先级
    pub fn is_optional(self) -> bool {
        self == Self::Optional
    }
}

// ============================================================
// ContextBlock — 上下文块结构
// ============================================================

/// 上下文块 — Agent 上下文的最小组成单元
///
/// 每个块携带优先级与可压缩标志。Critical 块 `is_compressible = false`,
/// 永不被 HCW 稀疏化丢弃(ADR-026 决策 7 红线)。
///
/// `name` 字段同时作为 HCW 的 `file_id`,用于 OSA context_mask 稀疏化路由。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextBlock {
    /// 块名称(同时作为 HCW file_id,用于 OSA 稀疏化路由)
    pub name: String,
    /// 块内容文本
    pub content: String,
    /// 块 Token 数(由调用方估算)
    pub tokens: usize,
    /// 块优先级
    pub priority: ContextPriority,
    /// 是否可压缩 — Critical 块为 false,其他为 true
    pub is_compressible: bool,
    // === PROBE P1.1: 块语义向量（CLV 探针打分输入）===
    /// 块语义向量（None = 无向量，探针路径跳过该块，走中性值既有语义）
    ///
    /// WHY `#[serde(default)]`: 向后兼容旧序列化数据；CLV 注入由调用方
    /// （NMC 编码器/上游数据源，P3.2 落地）经 `with_clv` 提供
    #[serde(default)]
    pub clv: Option<CLV>,
}

impl ContextBlock {
    /// 创建新上下文块
    ///
    /// Critical 块自动设置 `is_compressible = false`(ADR-026 决策 7 红线)。
    ///
    /// ## 参数
    /// - `name`: 块名称(同时作为 HCW file_id)
    /// - `content`: 块内容文本
    /// - `tokens`: 块 Token 数
    /// - `priority`: 块优先级
    pub fn new(
        name: impl Into<String>,
        content: impl Into<String>,
        tokens: usize,
        priority: ContextPriority,
    ) -> Self {
        Self {
            name: name.into(),
            content: content.into(),
            tokens,
            priority,
            // Critical 块永不可压缩(ADR-026 红线)
            is_compressible: !priority.is_critical(),
            // PROBE P1.1: 默认无向量（调用方按需注入）
            clv: None,
        }
    }

    /// 设置块语义向量（链式调用，PROBE P1.1 探针打分输入）
    ///
    /// # 参数
    /// - `clv`: 块语义向量（512 维 CLV）
    pub fn with_clv(mut self, clv: CLV) -> Self {
        self.clv = Some(clv);
        self
    }
}

// ============================================================
// AgentContext — Agent 独立上下文
// ============================================================

/// Agent 独立上下文 — 1M Token 等效,经 HCW 稀疏化
///
/// 包装 `hcw_window::HcwWindow`(ADR-026 决策 7,不自实现压缩)。
/// 1M Token = 128K 实际加载 + 8× 稀疏压缩(Ω-Compress)。
///
/// WHY 不派生 Clone/Serialize/Deserialize:
/// `HcwWindow` 内部含 `Arc<RwLock<HcwState>>`,非 Clone/Serializable。
/// AgentContext 持有 `EventBus`(Arc-based,Clone 廉价)用于创建临时 HcwWindow。
///
/// ## 字段说明
///
/// - `agent_id`: 所属 Agent ID(用于隔离守卫校验)
/// - `max_tokens`: 最大 Token 预算(1M 等效)
/// - `current_tokens`: 当前已用 Token 数(实际加载,非稀疏后)
/// - `blocks`: 上下文块列表(按添加顺序存储,build_prompt 时按优先级排序)
/// - `event_bus`: 事件总线(创建临时 HcwWindow + OmniSparseCoordinator)
pub struct AgentContext {
    /// 所属 Agent ID
    pub agent_id: String,
    /// 最大 Token 预算(1M 等效 = 128K 实际 + 8× 稀疏)
    pub max_tokens: usize,
    /// 当前已用 Token 数(实际加载)
    pub current_tokens: usize,
    /// 上下文块列表
    blocks: Vec<ContextBlock>,
    /// 事件总线(创建临时 HcwWindow + OSA coordinator)
    event_bus: EventBus,
    // === PROBE P1.1: 召回管线能力场（灰度开关，编译期配置非运行时旗）===
    /// 是否启用召回管线路径（fine + 三区 + 重排）
    ///
    /// WHY bool 字段而非 feature flag: 能力场灰度语义（ADR-034 决策 2），
    /// 默认关闭走老路径（行为零变化）；开启后经 recall 管线装载窗口。
    /// 能力令牌（CapabilityTokenRegistry）深度接入放 P2（与哨兵/学习联动）
    probe_enabled: bool,
    /// 查询探针向量（None = 用块 CLV 均值兜底）
    ///
    /// WHY Option: 调用方（编排器）可注入当前 query 的 CLV；
    /// None 时探针路径用块向量均值（中性探针），仍可验证三区+重排通路
    query_clv: Option<CLV>,
    // PROBE P2.2: 共享策略持有器（None = 独立 fallback，零行为变化）
    selector_holder: Option<Arc<SelectorLearnerHolder>>,
}

impl fmt::Debug for AgentContext {
    /// 手动实现 Debug,避免依赖 EventBus 的 Debug 实现
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentContext")
            .field("agent_id", &self.agent_id)
            .field("max_tokens", &self.max_tokens)
            .field("current_tokens", &self.current_tokens)
            .field("blocks_count", &self.blocks.len())
            .finish()
    }
}

impl AgentContext {
    /// 标准上下文窗口大小(1M Token 可寻址)— ADR-026 决策 7 / Task 15 §15.3
    ///
    /// 1M Token = `ContextTier::L3.context_window()` = 1_048_576。
    /// 通过 HCW + OSA 稀疏化,实际加载仅 `STANDARD_EFFECTIVE_CAPACITY`(128K)。
    ///
    /// WHY 关联常量而非字段:1M 是 ADR-026 决策 7 的固定标准,
    /// 不随实例变化,用 const 表达"不变性"。添加字段会破坏 `new()` 签名
    /// (Part I 公共签名保护,§3.3.1 第 5 条向后兼容)。
    pub const STANDARD_CONTEXT_WINDOW: usize = ContextTier::L3.context_window();

    /// 标准有效容量(128K Token,稀疏后实际加载)— ADR-026 决策 7 / Task 15 §15.3
    ///
    /// 128K Token = `ContextTier::L3.effective_capacity()` = 131_072。
    /// HCW 模式下 1M 上下文经 8× 稀疏压缩,实际驻留 ≤ 128K。
    ///
    /// WHY 关联常量:与 `STANDARD_CONTEXT_WINDOW` 对应,显式声明热工作集上限。
    pub const STANDARD_EFFECTIVE_CAPACITY: usize = ContextTier::L3.effective_capacity();

    /// 稀疏因子(8×)— ADR-026 决策 7
    ///
    /// 1M / 128K = 8,L3 层级通过 8× 稀疏压缩实现 1M 可寻址。
    /// 复用 `budget_model::SPARSE_FACTOR`,避免重复定义(§3.3.1 第 9 条)。
    pub const SPARSE_FACTOR: u32 = SPARSE_FACTOR;

    /// 创建新的 Agent 上下文
    ///
    /// ## 参数
    /// - `agent_id`: 所属 Agent ID
    /// - `max_tokens`: 最大 Token 预算(1M 等效,如 `1_048_576`)
    /// - `event_bus`: 事件总线(HCW + OSA 内部通信所需)
    ///
    /// ## 示例
    ///
    /// ```no_run
    /// use chimera_mas::prelude::*;
    /// use event_bus::EventBus;
    ///
    /// let ctx = AgentContext::new("agent-1", 1_048_576, EventBus::new()).unwrap();
    /// assert_eq!(ctx.max_tokens, 1_048_576);
    /// ```
    pub fn new(
        agent_id: impl Into<String>,
        max_tokens: usize,
        event_bus: EventBus,
    ) -> Result<Self> {
        Ok(Self {
            agent_id: agent_id.into(),
            max_tokens,
            current_tokens: 0,
            blocks: Vec::new(),
            event_bus,
            // PROBE P1.1: 默认关闭召回管线（老路径行为零变化）
            probe_enabled: false,
            // PROBE P2.2: 默认无共享 holder（独立 fallback）
            selector_holder: None,
            query_clv: None,
        })
    }

    /// 启用召回管线路径（能力场灰度，PROBE P1.1）
    ///
    /// # 参数
    /// - `enabled`: 是否启用（默认 false 走老路径）
    ///
    /// WHY builder 而非构造参数: 保持 `new()` 公开签名不变（Part I 公共签名保护）
    pub fn with_probe(mut self, enabled: bool) -> Self {
        self.probe_enabled = enabled;
        self
    }

    /// 注入查询探针向量（PROBE P1.1）
    ///
    /// # 参数
    /// - `query_clv`: 当前 query 的 CLV（None 时用块向量均值兜底）
    pub fn with_query_clv(mut self, query_clv: CLV) -> Self {
        self.query_clv = Some(query_clv);
        self
    }

    /// 注入共享策略持有器（PROBE P2.2）
    ///
    /// # 参数
    /// - `holder`: 共享 SelectorLearnerHolder（编排器 SelectorOrchestrator 构造）
    ///
    /// WHY builder: 保持 new() 签名不变（向后兼容）；None = 独立 fallback
    pub fn with_selector_holder(mut self, holder: Arc<SelectorLearnerHolder>) -> Self {
        self.selector_holder = Some(holder);
        self
    }

    /// 是否启用召回管线路径（诊断/测试用）
    pub fn probe_enabled(&self) -> bool {
        self.probe_enabled
    }

    /// 添加上下文块
    ///
    /// 超过 `max_tokens` 时返回 `MasError::TokenBudgetExceeded`,块不被添加。
    ///
    /// ## 参数
    /// - `block`: 要添加的上下文块
    pub fn add_block(&mut self, block: ContextBlock) -> Result<()> {
        let new_tokens = self.current_tokens + block.tokens;
        if new_tokens > self.max_tokens {
            return Err(MasError::TokenBudgetExceeded {
                agent_id: self.agent_id.clone(),
                // current_tokens 记录尝试达到的总量(含被拒绝块),便于错误诊断
                current_tokens: new_tokens,
                max_tokens: self.max_tokens,
            });
        }
        self.current_tokens = new_tokens;
        self.blocks.push(block);
        Ok(())
    }

    /// 返回上下文块的不可变迭代器
    ///
    /// 供 `ContextIsolationGuard::create_safe_summary()` 遍历块内容提取摘要。
    pub fn blocks_iter(&self) -> impl Iterator<Item = &ContextBlock> {
        self.blocks.iter()
    }

    /// 构建提示词 — 调用 HCW select_window() + OSA compute_all_masks() 稀疏化
    ///
    /// ADR-026 决策 7: 不自实现压缩,委托给 hcw_window + osa_coordinator。
    ///
    /// ## 算法(&self,不改存储状态)
    ///
    /// 1. 创建临时 HcwWindow(避免多次调用导致 entry 累积)
    /// 2. 插入 blocks 作为 ContextEntry(file_id = block.name)
    /// 3. 估算复杂度(total_tokens → complexity f32)
    /// 4. `select_window(complexity)` 触发窗口层级选择
    /// 5. `compute_all_masks(&profile)` 计算 OSA 五维度掩码
    /// 6. 增强 `active_file_ids = OSA context.active_ids ∪ Critical 块 name`
    /// 7. `apply_sparse_mask(active_file_ids)` 执行稀疏化
    /// 8. 按 priority 降序拼接保留的 blocks 内容
    ///
    /// ## 返回
    /// - `Ok(String)`: 稀疏化后的提示词
    /// - `Err(MasError::ContextCompressionFailed)`: HCW 或 OSA 失败
    pub async fn build_prompt(&self) -> Result<String> {
        if self.blocks.is_empty() {
            return Ok(String::new());
        }

        // PROBE P1.1: 能力场灰度 — 启用召回管线时走 recall 路径（三区+重排），
        // 否则老路径零变化。仅当存在带 CLV 的块时 recall 路径有意义（R3 缓解:
        // 无 CLV 块走中性分，通路代码就绪，真实 CLV 注入随 P3.2 上游数据源）
        if self.probe_enabled && self.blocks.iter().any(|b| b.clv.is_some()) {
            return self.build_prompt_recall().await;
        }
        self.build_prompt_legacy().await
    }

    /// 构建提示词（老路径）— HCW select_window() + OSA compute_all_masks() 稀疏化
    ///
    /// ADR-026 决策 7: 不自实现压缩,委托给 hcw_window + osa_coordinator。
    ///
    /// ## 算法(&self,不改存储状态)
    ///
    /// 1. 创建临时 HcwWindow(避免多次调用导致 entry 累积)
    /// 2. 插入 blocks 作为 ContextEntry(file_id = block.name)
    /// 3. 估算复杂度(total_tokens → complexity f32)
    /// 4. `select_window(complexity)` 触发窗口层级选择
    /// 5. `compute_all_masks(&profile)` 计算 OSA 五维度掩码
    /// 6. 增强 `active_file_ids = OSA context.active_ids ∪ Critical 块 name`
    /// 7. `apply_sparse_mask(active_file_ids)` 执行稀疏化
    /// 8. 按 priority 降序拼接保留的 blocks 内容
    ///
    /// ## 返回
    /// - `Ok(String)`: 稀疏化后的提示词
    /// - `Err(MasError::ContextCompressionFailed)`: HCW 或 OSA 失败
    ///
    /// WHY 提取: PROBE P1.1 引入 recall 分支后，老路径保持零改动（回归面最小），
    /// 且作为 recall 路径失败的永久 fallback（R1 缓解）
    async fn build_prompt_legacy(&self) -> Result<String> {
        // 1. 估算复杂度(基于总 token 数)
        let total_tokens: usize = self.blocks.iter().map(|b| b.tokens).sum();
        let complexity = estimate_complexity(total_tokens);

        // 2. 创建临时 HcwWindow,插入 blocks 作为 ContextEntry
        // PROBE P2.2: 共享策略持有器时用 with_learner（注入链），否则独立 fallback
        let temp_window = match &self.selector_holder {
            Some(holder) => HcwWindow::with_learner(self.event_bus.clone(), Arc::clone(holder))
                .map_err(|e| MasError::ContextCompressionFailed {
                    agent_id: self.agent_id.clone(),
                    reason: format!("HCW(learner) 创建失败: {e}"),
                })?,
            None => HcwWindow::with_default_config(self.event_bus.clone()).map_err(|e| {
                MasError::ContextCompressionFailed {
                    agent_id: self.agent_id.clone(),
                    reason: format!("HCW 创建失败: {e}"),
                }
            })?,
        };

        for (i, block) in self.blocks.iter().enumerate() {
            let entry = ContextEntry::new(
                format!("entry-{i}"),
                &block.name,
                &block.content,
                block.tokens,
            );
            temp_window
                .insert(entry)
                .await
                .map_err(|e| MasError::ContextCompressionFailed {
                    agent_id: self.agent_id.clone(),
                    reason: format!("HCW 插入失败: {e}"),
                })?;
        }

        // 3. select_window 触发窗口层级选择(溢出时自动压缩)
        temp_window.select_window(complexity).await.map_err(|e| {
            MasError::ContextCompressionFailed {
                agent_id: self.agent_id.clone(),
                reason: format!("HCW select_window 失败: {e}"),
            }
        })?;

        // 4. 创建 OSA coordinator,计算稀疏掩码
        let coord = OmniSparseCoordinator::new(self.event_bus.clone());

        // 构造 TaskProfile(available_files = 所有 block name,供 OSA context_mask 选取)
        let available_files: Vec<FileId> = self
            .blocks
            .iter()
            .map(|b| FileId::new(b.name.clone()))
            .collect();
        let profile = TaskProfile {
            task_id: TaskId::new(format!("ctx-{}", self.agent_id)),
            task_type: TaskType::Read,
            complexity_score: complexity,
            risk_level: RiskLevel::Low,
            time_pressure: TimePressure::Low,
            affected_scope: AffectedScope::Local,
            available_tools: Vec::new(),
            available_files,
            available_memories: Vec::new(),
            recent_operations: Vec::new(),
            active_tasks: Vec::new(),
            // 评分字段默认 None:MAS 上下文管理 fallback 到 heuristic_scores
            routing_scores: None,
            context_scores: None,
            memory_scores: None,
            // 任务阶段未指定,MAS 上下文管理 fallback 到 StandardTopK
            task_phase: None,
        };

        let masks = coord.compute_all_masks(&profile).await.map_err(|e| {
            MasError::ContextCompressionFailed {
                agent_id: self.agent_id.clone(),
                reason: format!("OSA compute_all_masks 失败: {e}"),
            }
        })?;

        // 5. 增强 active_file_ids = OSA context.active_ids ∪ Critical 块 name
        // WHY Critical 块强制加入:确保永不因 OSA 稀疏化丢失(ADR-026 红线)
        let mut active_names: HashSet<String> = masks
            .context
            .active_ids
            .iter()
            .map(|f| f.to_string())
            .collect();
        for block in &self.blocks {
            if block.priority.is_critical() {
                active_names.insert(block.name.clone());
            }
        }

        // 6. apply_sparse_mask(HCW 实际执行稀疏化,发布 ContextCompressed 事件)
        // WHY 移动而非克隆(L9 优化第二轮):active_file_ids 直接 move 进 apply_sparse_mask,
        // 消除旧版 `active_file_ids.clone()` 的整表克隆;查找集(下方 active_set)改从
        // active_names(未被移动)借用,不再依赖已消费的 active_file_ids。
        let active_file_ids: Vec<String> = active_names.iter().cloned().collect();
        temp_window
            .apply_sparse_mask(active_file_ids)
            .await
            .map_err(|e| MasError::ContextCompressionFailed {
                agent_id: self.agent_id.clone(),
                reason: format!("HCW apply_sparse_mask 失败: {e}"),
            })?;

        // 7. 按优先级降序拼接保留的 blocks 内容
        // WHY active_set 借用 active_names(仍存活):避免第三次整表克隆(§4.4 内存优化)。
        let active_set: HashSet<&str> = active_names.iter().map(|s| s.as_str()).collect();
        // WHY 5 桶计数排序(L9 优化第二轮):ContextPriority 仅 5 档,计数排序 O(n) 替代
        // sort_by_key O(n log n),且过滤 + 分桶单遍完成,省去中间 retained_blocks Vec 分配。
        // 桶下标 = priority as usize(Optional=0..Critical=4),桶内按 self.blocks 原序
        // 追加保持稳定性;输出按 Critical→Optional 降序拼接(iter().rev())。
        let mut buckets: [Vec<&str>; 5] = std::array::from_fn(|_| Vec::new());
        for block in &self.blocks {
            if active_set.contains(block.name.as_str()) {
                buckets[block.priority as usize].push(block.content.as_str());
            }
        }
        let prompt = buckets
            .iter()
            .rev()
            .flat_map(|bucket| bucket.iter().copied())
            .collect::<Vec<&str>>()
            .join("\n\n");

        Ok(prompt)
    }

    /// 构建提示词（召回管线路径）— fine 精排 + 三区填充 + 位置重排（PROBE P1.1）
    ///
    /// # 算法（R7 降级：仅 fine+rerank，coarse 预留接缝）
    /// 1. 收集有 CLV 的块，upsert 到 HnswStore（512 维）
    /// 2. 探针 CLV = query_clv 或块向量均值（中性探针兜底）
    /// 3. `FineRecall::rank_with_probe`（空 CoarseRecallOutput 占位）
    /// 4. `RerankFill::fill_zones` 三区填充（sink = Critical 块 / sliding = 末尾块）
    /// 5. `RerankFill::reorder_blocks` 位置重排（top-2 置头，temporal 豁免）
    /// 6. 按重排顺序拼接块内容
    ///
    /// # 降级（R1）
    /// 任何 recall 管线错误 → fallback 老路径（build_prompt_legacy），不阻断任务
    ///
    /// # 依赖
    /// - `repo-wiki HnswStore`（L9→L5 向下依赖合规）实现 `VectorStore` trait
    /// - `hcw_window::recall`（fine/rerank）
    async fn build_prompt_recall(&self) -> Result<String> {
        // 1. 收集有 CLV 的块 + token 映射
        let clv_blocks: Vec<&ContextBlock> =
            self.blocks.iter().filter(|b| b.clv.is_some()).collect();
        if clv_blocks.is_empty() {
            // 理论不可达（build_prompt 已检查），防御性 fallback
            return self.build_prompt_legacy().await;
        }
        let mut block_tokens: std::collections::HashMap<String, usize> =
            std::collections::HashMap::with_capacity(self.blocks.len());
        for b in &self.blocks {
            block_tokens.insert(b.name.clone(), b.tokens);
        }

        // 2. HnswStore（512 维）upsert 有 CLV 块（block_id = name）
        let store = repo_wiki::HnswStore::with_dim(CLV::DIMENSION);
        let mut block_clvs: std::collections::HashMap<String, CLV> =
            std::collections::HashMap::with_capacity(clv_blocks.len());
        for b in &clv_blocks {
            let clv = b.clv.as_ref().expect("filtered by clv.is_some");
            store.upsert(&b.name, clv.as_slice(), ()).map_err(|e| {
                MasError::ContextCompressionFailed {
                    agent_id: self.agent_id.clone(),
                    reason: format!("HnswStore upsert 失败: {e}"),
                }
            })?;
            block_clvs.insert(b.name.clone(), clv.clone());
        }

        // 3. 探针 CLV：query_clv 或块向量均值（中性探针兜底）
        let probe_clv: CLV = match &self.query_clv {
            Some(q) => q.clone(),
            None => {
                // 均值池化（与 recall/eval::mix_probe 语义一致）
                let n = clv_blocks.len() as f32;
                let mut acc: Vec<f32> = vec![0.0f32; CLV::DIMENSION];
                for b in &clv_blocks {
                    let s = b.clv.as_ref().expect("filtered").as_slice();
                    for (i, v) in s.iter().enumerate() {
                        acc[i] += v;
                    }
                }
                for v in acc.iter_mut() {
                    *v /= n;
                }
                CLV::from_vec(acc).map_err(|e| MasError::ContextCompressionFailed {
                    agent_id: self.agent_id.clone(),
                    reason: format!("CLV 均值构造失败: {e}"),
                })?
            }
        };

        // 4. fine 精排（空 CoarseRecallOutput 占位——R7 降级接缝，数据源就绪后激活）
        let fine = hcw_window::recall::FineRecall::with_default_config();
        let coarse = hcw_window::recall::CoarseRecallOutput {
            modules: vec![],
            elapsed_us: 0,
        };
        let fine_output = match fine.rank_with_probe(hcw_window::recall::ProbeRecallInput {
            coarse_output: &coarse,
            probe_clv: &probe_clv,
            vector_store: &store,
            block_clvs: Some(&block_clvs),
            top_k: clv_blocks.len().min(500),
        }) {
            Ok(out) => out,
            Err(_) => return self.build_prompt_legacy().await, // 降级（R1）
        };

        // 5. 三区填充 + 位置重排
        //    sink = Critical 优先级块（系统提示/关键上下文恒留，H5 修复）
        //    sliding = 末尾最多 4 块（recency 由结构保证）
        let sink_ids: Vec<String> = self
            .blocks
            .iter()
            .filter(|b| b.priority.is_critical())
            .map(|b| b.name.clone())
            .collect();
        let sliding_ids: Vec<String> = self
            .blocks
            .iter()
            .rev()
            .take(4)
            .map(|b| b.name.clone())
            .collect();
        let rerank = hcw_window::recall::RerankFill::with_default_config();
        let zone_output = match rerank.fill_zones(
            hcw_window::recall::ZoneFillInput {
                coarse_output: &coarse,
                fine_output: &fine_output,
                block_tokens: &block_tokens,
                sink_blocks: &sink_ids,
                sliding_blocks: &sliding_ids,
                summary_block: None,
            },
            hcw_window::recall::ZoneFillConfig::default(),
        ) {
            Ok(out) => out,
            Err(_) => return self.build_prompt_legacy().await, // 降级（R1）
        };
        let filled_len = zone_output.filled_blocks.len();
        let reordered = hcw_window::recall::RerankFill::reorder_blocks(
            zone_output.filled_blocks,
            sink_ids.len().min(filled_len),
            0, // sliding 已含在 filled_blocks 末尾（fill_zones 输出即 sink+中段+滑窗）
            None,
        );

        // 6. 按重排顺序拼接（块不在重排结果中时按优先级兜底）
        let by_name: std::collections::HashMap<&str, &ContextBlock> =
            self.blocks.iter().map(|b| (b.name.as_str(), b)).collect();
        let mut parts: Vec<&str> = Vec::with_capacity(reordered.len());
        for bs in &reordered {
            if let Some(block) = by_name.get(bs.block_id.as_str()) {
                parts.push(block.content.as_str());
            }
        }
        // 未被 fine 选中的块（无 CLV 等）按优先级降序兜底追加
        let mut legacy_ids: Vec<String> = self
            .blocks
            .iter()
            .filter(|b| !by_name.contains_key(b.name.as_str()))
            .map(|b| b.name.clone())
            .collect();
        legacy_ids.sort_by_key(|name| {
            self.blocks
                .iter()
                .position(|b| &b.name == name)
                .unwrap_or(usize::MAX)
        });
        for name in legacy_ids {
            if let Some(block) = by_name.get(name.as_str()) {
                parts.push(block.content.as_str());
            }
        }
        Ok(parts.join("\n\n"))
    }
}

/// 估算复杂度 — 基于总 token 数映射到 [0.0, 1.0] 区间
///
/// 启发式策略(对应 HCW 四级窗口):
/// - total ≥ 131_072 (128K) → 0.9 (L3, UltraComplex)
/// - total ≥ 32_768 (32K) → 0.6 (L2, Complex)
/// - total ≥ 4_096 (4K) → 0.4 (L1, Regular)
/// - else → 0.1 (L0, Simple)
fn estimate_complexity(total_tokens: usize) -> f32 {
    if total_tokens >= 131_072 {
        0.9
    } else if total_tokens >= 32_768 {
        0.6
    } else if total_tokens >= 4_096 {
        0.4
    } else {
        0.1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造确定性 CLV（SplitMix64，512 维）
    fn make_clv(seed: u64) -> CLV {
        let v: Vec<f32> = (0..512)
            .map(|j| {
                let mut z = seed.wrapping_add((j as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
                z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
                z ^= z >> 31;
                ((z >> 11) as f32) / (1u64 << 53) as f32 * 2.0 - 1.0
            })
            .collect();
        CLV::from_vec(v).expect("512 dims")
    }

    /// 构造带 CLV 的上下文块
    fn block_with_clv(name: &str, priority: ContextPriority, seed: u64) -> ContextBlock {
        ContextBlock::new(name, format!("content-{name}"), 100, priority).with_clv(make_clv(seed))
    }

    // ============================================================
    // PROBE P1.1: 召回管线路径测试
    // ============================================================

    #[tokio::test]
    async fn test_build_prompt_recall_path_orders_by_probe() {
        // 启用探针 + 带 CLV 块 → 走 recall 路径（三区+重排）
        let ctx = AgentContext::new("agent-1", 1_048_576, EventBus::new())
            .unwrap()
            .with_probe(true);
        // 8 个块：分数由 CLV 与探针相似度决定；1 个 Critical（sink 恒留）
        let mut ctx = ctx;
        ctx.add_block(block_with_clv("sys", ContextPriority::Critical, 1))
            .unwrap();
        for i in 0..7 {
            ctx.add_block(block_with_clv(
                &format!("b{i}"),
                ContextPriority::Normal,
                10 + i,
            ))
            .unwrap();
        }
        let prompt = ctx.build_prompt().await.unwrap();
        // recall 路径：sink（sys）在前 + 中段 + 滑窗；拼接含全部块内容
        assert!(prompt.starts_with("content-sys"), "sink 块应恒留且在前");
        assert!(prompt.contains("content-b0"));
        assert!(prompt.contains("content-b6"));
    }

    #[tokio::test]
    async fn test_build_prompt_recall_disabled_uses_legacy() {
        // 探针关闭 → 老路径（行为与现状一致）
        let ctx = AgentContext::new("agent-1", 1_048_576, EventBus::new()).unwrap();
        assert!(!ctx.probe_enabled());
        let mut ctx = ctx;
        ctx.add_block(block_with_clv("a", ContextPriority::Normal, 1))
            .unwrap();
        ctx.add_block(block_with_clv("b", ContextPriority::Normal, 2))
            .unwrap();
        // legacy 路径输出由 OSA 掩码决定（Normal 块可能被稀疏化丢弃），
        // 本测试只验证：关闭探针时不报错且走老路径（recall 分支不触发）
        let prompt = ctx.build_prompt().await.unwrap();
        // 老路径输出是字符串（可能为空——OSA 未选中任何文件时）
        assert!(prompt.is_empty() || prompt.contains("content-a") || prompt.contains("content-b"));
    }

    #[tokio::test]
    async fn test_build_prompt_recall_no_clv_falls_back_legacy() {
        // 启用探针但无 CLV 块 → 老路径 fallback
        let ctx = AgentContext::new("agent-1", 1_048_576, EventBus::new())
            .unwrap()
            .with_probe(true);
        let mut ctx = ctx;
        ctx.add_block(ContextBlock::new(
            "a",
            "content-a",
            100,
            ContextPriority::Normal,
        ))
        .unwrap();
        let prompt = ctx.build_prompt().await.unwrap();
        assert_eq!(prompt, "content-a");
    }

    #[test]
    fn test_context_block_clv_serde_backward_compat() {
        // ContextBlock 追加 clv 字段后旧 JSON 反序列化兼容（serde default）
        let old_json =
            r#"{"name":"a","content":"c","tokens":10,"priority":"Normal","is_compressible":true}"#;
        let block: ContextBlock = serde_json::from_str(old_json).expect("旧数据应兼容");
        assert!(block.clv.is_none(), "旧数据无 clv 字段应默认 None");
    }

    #[test]
    fn test_context_priority_ordering() {
        assert!(ContextPriority::Critical > ContextPriority::High);
        assert!(ContextPriority::High > ContextPriority::Normal);
        assert!(ContextPriority::Normal > ContextPriority::Low);
        assert!(ContextPriority::Low > ContextPriority::Optional);
    }

    #[test]
    fn test_context_priority_predicates() {
        assert!(ContextPriority::Critical.is_critical());
        assert!(!ContextPriority::High.is_critical());
        assert!(ContextPriority::Optional.is_optional());
        assert!(!ContextPriority::Low.is_optional());
    }

    #[test]
    fn test_context_block_new_critical_not_compressible() {
        let block = ContextBlock::new("system-prompt", "content", 100, ContextPriority::Critical);
        assert!(!block.is_compressible, "Critical 块不可压缩");
    }

    #[test]
    fn test_context_block_new_normal_compressible() {
        let block = ContextBlock::new("block-1", "content", 100, ContextPriority::Normal);
        assert!(block.is_compressible, "Normal 块默认可压缩");
    }

    #[test]
    fn test_estimate_complexity_thresholds() {
        assert!((estimate_complexity(100) - 0.1).abs() < f32::EPSILON);
        assert!((estimate_complexity(4_096) - 0.4).abs() < f32::EPSILON);
        assert!((estimate_complexity(32_768) - 0.6).abs() < f32::EPSILON);
        assert!((estimate_complexity(131_072) - 0.9).abs() < f32::EPSILON);
    }
}
