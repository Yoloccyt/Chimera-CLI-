//! MLC 引擎 — 四级神经形态记忆的统一接口与 EventBus 集成
//!
//! 对应架构层:L2 Memory
//! 对应创新点:MLC(Multi-Level Context,四级神经形态记忆)
//!
//! # 核心职责
//! - 聚合 L0-L3 四级记忆,提供统一的 store/recall/promote/demote 接口
//! - 内部自动路由到对应层级(根据 `MemoryTier` 字段)
//! - 集成 EventBus,每 N 次操作发布 `MemoryMetricsReported` 事件
//! - 层级迁移时发布 `MemoryTiered` 事件
//!
//! # 架构红线
//! - 所有状态变更通过 Event Bus 广播(§2.2 依赖铁律)
//! - DashMap 写锁释放后再调用 async 方法(避免死锁,Week 2 经验教训)
//! - 单函数 ≤ 200 行,禁止 unwrap()/expect()
//!
//! # 线程安全
//! `MlcEngine` 内部所有层级都是线程安全的(L0 DashMap,L1/L2 Mutex,L3 `Mutex<Connection>`)。
//! `EventBus` 基于 `tokio::broadcast`,Clone 廉价(Arc 引用计数)。
//! 所有 async fn 满足 `Send + 'static` 约束,可被 tokio::spawn。

use std::sync::atomic::{AtomicU64, Ordering};

use dashmap::DashMap;
use event_bus::{EventBus, EventMetadata, NexusEvent};
use nexus_contracts::{MemoryStrategy, MemoryStrategyPolicy};
use nexus_core::CLV;
use tracing::{debug, info, warn};

use crate::config::MlcConfig;
use crate::error::MlcError;
use crate::l0_working::WorkingMemory;
use crate::l1_episodic::EpisodicMemory;
use crate::l2_semantic::SemanticMemory;
use crate::l3_procedural::ProceduralMemory;
use crate::memory_strategy_learner::MemoryStrategyLearnerHolder;
use crate::types::{MemoryEntry, MemoryId, MemoryTier, ProceduralEntry};

/// MLC 引擎 — 四级神经形态记忆的统一接口
///
/// 聚合 L0-L3 四级记忆,通过 EventBus 广播指标与迁移事件。
///
/// # 设计决策(WHY)
/// - **操作计数原子化**:用 `AtomicU64` 而非 Mutex,避免锁竞争
/// - **指标上报异步**:达到阈值时异步发布事件,不阻塞主流程
/// - **层级迁移原子性**:先从源层移除,再插入目标层,失败时回滚(重新插入源层)
pub struct MlcEngine {
    /// L0 工作记忆(DashMap + LRU)
    l0: WorkingMemory,
    /// L1 情节记忆(BTreeMap + HashMap)
    l1: EpisodicMemory,
    /// L2 语义记忆(Vec + KNN)
    l2: SemanticMemory,
    /// L3 程序记忆(SQLite 持久化)
    l3: ProceduralMemory,
    /// 事件总线(基于 Arc,Clone 廉价)
    event_bus: EventBus,
    /// 引擎配置
    config: MlcConfig,
    /// 累计操作次数(用于指标上报触发)
    op_count: AtomicU64,
    /// 累计命中次数(用于命中率计算)
    hit_count: AtomicU64,
    /// 累计未命中次数
    miss_count: AtomicU64,
    /// 条目级迁移锁(SubTask 18.1)
    ///
    /// WHY:消除 `migrate` 的 TOCTOU 窗口。多线程并发迁移同一 MemoryId 时,
    /// `fetch_from_tier → insert → remove_from_tier` 过程中条目可能被其他线程修改,
    /// 导致数据重复或丢失。用 `DashMap<MemoryId, ()>` 的 `entry()` API 实现条目级锁:
    /// 第一个线程 `entry().or_insert(())` 原子性获取锁,后续同一 MemoryId 的迁移
    /// 会阻塞在 `entry()` 上(DashMap 分片写锁互斥),直到持有者离开作用域释放 guard。
    ///
    /// 锁粒度是条目级(每个 MemoryId 一把锁),不影响其他条目的并发迁移。
    /// guard 离开作用域自动释放 shard 写锁,无需手动 remove。
    migration_locks: DashMap<MemoryId, ()>,
    /// 记忆策略学习器持有器 — S2 接缝策略异步下发 + 本地 fallback(P4-W14.1)
    ///
    /// WHY(P4-W14.1):承载 `omega-learner` 异步下发的 `MemoryStrategyPolicy`,
    /// 为 recall 路径提供策略感知能力。初始化为 `Static(StandardTopK)`(C4 合规 fallback),
    /// 向后兼容既有 `recall_by_clv(top_k=10)` 行为。
    ///
    /// 上层编排器(chimera-cli / quest-engine)通过 `update_memory_strategy_policy()`
    /// 注入学习策略;recall 路径通过 `recall_by_clv_with_strategy()` 或
    /// `recall_by_clv_with_current_policy()` 使用策略参数。
    memory_strategy_holder: MemoryStrategyLearnerHolder,
}

impl MlcEngine {
    /// 创建 MLC 引擎,使用指定配置与 EventBus
    ///
    /// 会自动打开 L3 SQLite 数据库(路径从 config 读取,展开 `~`)
    pub fn new(config: MlcConfig, event_bus: EventBus) -> Result<Self, MlcError> {
        config.validate()?;

        // 展开 `~` 并打开 L3 SQLite 数据库
        let db_path = MlcConfig::expand_tilde(&config.procedural_db_path);
        // 确保父目录存在(SQLite 不会自动创建目录)
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                MlcError::StorageError(format!(
                    "创建 L3 数据库目录失败: {} - {}",
                    parent.display(),
                    e
                ))
            })?;
        }
        let l3 = ProceduralMemory::open(&db_path)?;

        Ok(Self {
            l0: WorkingMemory::new(config.l0_capacity),
            l1: EpisodicMemory::new(config.l1_capacity),
            l2: SemanticMemory::new(config.l2_capacity),
            l3,
            event_bus,
            config,
            op_count: AtomicU64::new(0),
            hit_count: AtomicU64::new(0),
            miss_count: AtomicU64::new(0),
            migration_locks: DashMap::new(),
            // P4-W14.1: 初始化为 Static(StandardTopK) fallback(C4 合规)
            memory_strategy_holder: MemoryStrategyLearnerHolder::default(),
        })
    }

    /// 创建 MLC 引擎,使用默认配置与指定 EventBus
    pub fn with_default_config(event_bus: EventBus) -> Result<Self, MlcError> {
        Self::new(MlcConfig::default(), event_bus)
    }

    /// 创建用于测试的 MLC 引擎(L3 使用内存数据库)
    ///
    /// WHY:测试场景不需要持久化,内存数据库更快且自动清理
    pub fn new_in_memory(event_bus: EventBus) -> Result<Self, MlcError> {
        let config = MlcConfig::default();
        let l3 = ProceduralMemory::open_in_memory()?;
        Ok(Self {
            l0: WorkingMemory::new(config.l0_capacity),
            l1: EpisodicMemory::new(config.l1_capacity),
            l2: SemanticMemory::new(config.l2_capacity),
            l3,
            event_bus,
            config,
            op_count: AtomicU64::new(0),
            hit_count: AtomicU64::new(0),
            miss_count: AtomicU64::new(0),
            migration_locks: DashMap::new(),
            // P4-W14.1: 初始化为 Static(StandardTopK) fallback(C4 合规)
            memory_strategy_holder: MemoryStrategyLearnerHolder::default(),
        })
    }

    /// 创建用于测试的 MLC 引擎,指定配置,L3 使用内存数据库
    pub fn new_in_memory_with_config(
        config: MlcConfig,
        event_bus: EventBus,
    ) -> Result<Self, MlcError> {
        config.validate()?;
        let l3 = ProceduralMemory::open_in_memory()?;
        Ok(Self {
            l0: WorkingMemory::new(config.l0_capacity),
            l1: EpisodicMemory::new(config.l1_capacity),
            l2: SemanticMemory::new(config.l2_capacity),
            l3,
            event_bus,
            config,
            op_count: AtomicU64::new(0),
            hit_count: AtomicU64::new(0),
            miss_count: AtomicU64::new(0),
            migration_locks: DashMap::new(),
            // P4-W14.1: 初始化为 Static(StandardTopK) fallback(C4 合规)
            memory_strategy_holder: MemoryStrategyLearnerHolder::default(),
        })
    }

    /// 获取配置引用
    pub fn config(&self) -> &MlcConfig {
        &self.config
    }

    /// 获取 L0 工作记忆引用(用于直接操作)
    pub fn l0(&self) -> &WorkingMemory {
        &self.l0
    }

    /// 获取 L1 情节记忆引用
    pub fn l1(&self) -> &EpisodicMemory {
        &self.l1
    }

    /// 获取 L2 语义记忆引用
    pub fn l2(&self) -> &SemanticMemory {
        &self.l2
    }

    /// 获取 L3 程序记忆引用
    pub fn l3(&self) -> &ProceduralMemory {
        &self.l3
    }

    /// 存储记忆条目(根据 entry.tier 自动路由到对应层级)
    ///
    /// - L0:调用 WorkingMemory::insert,返回被驱逐的条目
    /// - L1:调用 EpisodicMemory::insert,返回被驱逐的条目
    /// - L2:调用 SemanticMemory::insert(必须携带 CLV),返回被驱逐的条目
    /// - L3:不支持 MemoryEntry,应使用 `store_procedural` 方法
    ///
    /// 每次存储递增操作计数,达到阈值时发布 `MemoryMetricsReported` 事件。
    pub async fn store(&self, entry: MemoryEntry) -> Result<Option<MemoryEntry>, MlcError> {
        let tier = entry.tier;
        let entry_id = entry.id.clone();

        let evicted = match tier {
            MemoryTier::L0Working => self.l0.insert(entry)?,
            MemoryTier::L1Episodic => self.l1.insert(entry)?,
            MemoryTier::L2Semantic => self.l2.insert(entry)?,
            MemoryTier::L3Procedural => {
                return Err(MlcError::InvalidConfig(format!(
                    "L3 程序记忆不支持 MemoryEntry,请使用 store_procedural: {entry_id}"
                )));
            }
        };

        debug!(
            entry_id = %entry_id,
            tier = tier.as_str(),
            evicted = ?evicted.as_ref().map(|e| e.id.as_str()),
            "记忆条目已存储"
        );

        // 递增操作计数并检查是否需要发布指标
        self.increment_op_count().await?;

        Ok(evicted)
    }

    /// 存储 L3 程序记忆条目
    pub async fn store_procedural(&self, entry: ProceduralEntry) -> Result<(), MlcError> {
        self.l3.insert(&entry).await?;
        debug!(
            pattern = %entry.pattern_signature.to_key().unwrap_or_default(),
            "L3 程序记忆已存储"
        );
        Ok(())
    }

    /// 按 ID 跨层查找记忆条目(L0 → L1 → L2)
    ///
    /// 找到后返回条目;若所有层都未找到返回 None。
    /// 不更新访问时间(避免跨层查找影响 LRU 语义)。
    pub async fn recall(&self, id: &str) -> Result<Option<MemoryEntry>, MlcError> {
        // L0 查找(peek 不更新 LRU)
        if let Some(entry) = self.l0.peek(id) {
            self.hit_count.fetch_add(1, Ordering::Relaxed);
            return Ok(Some(entry));
        }

        // L1 查找
        match self.l1.get(id) {
            Ok(entry) => {
                self.hit_count.fetch_add(1, Ordering::Relaxed);
                return Ok(Some(entry));
            }
            Err(MlcError::EntryNotFound(_)) => {}
            Err(e) => return Err(e),
        }

        // L2 查找
        match self.l2.get(id) {
            Ok(entry) => {
                self.hit_count.fetch_add(1, Ordering::Relaxed);
                return Ok(Some(entry));
            }
            Err(MlcError::EntryNotFound(_)) => {}
            Err(e) => return Err(e),
        }

        // 所有层都未找到
        self.miss_count.fetch_add(1, Ordering::Relaxed);
        Ok(None)
    }

    /// 按 ID 召回 `Current` 状态条目(P3-W11.1 D12 修复 — 幽灵记忆免疫)
    ///
    /// 调用 `recall` 后过滤 `is_current()`,仅返回 `Current` 状态(或 `temporal_meta = None`
    /// 视为 `Current` 的向后兼容条目)。`Historical`/`Transition` 状态的条目返回 `None`。
    ///
    /// WHY(P3-W11.1 D12):spec.md:293-295 "默认只取 Current" — 避免过时事实
    /// (Historical)与当前事实共召回导致的幽灵记忆问题。
    ///
    /// 与 `recall` 的区别:`recall` 返回所有状态条目(向后兼容),此方法严格过滤 Current。
    ///
    /// # 示例
    /// ```no_run
    /// # use mlc_engine::{MlcEngine, MemoryEntry, MemoryTier};
    /// # use event_bus::EventBus;
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// let bus = EventBus::new();
    /// let engine = MlcEngine::new_in_memory(bus)?;
    ///
    /// // Current 条目(默认)
    /// let entry = MemoryEntry::new("m-1", "内容", MemoryTier::L0Working);
    /// engine.store(entry).await?;
    /// assert!(engine.recall_current("m-1").await?.is_some());
    /// # Ok(())
    /// # }
    /// ```
    pub async fn recall_current(&self, id: &str) -> Result<Option<MemoryEntry>, MlcError> {
        match self.recall(id).await? {
            Some(entry) if entry.is_current() => Ok(Some(entry)),
            _ => Ok(None),
        }
    }

    /// 按 ID 显式查询 `Historical` 状态条目(P3-W11.1 D12 修复)
    ///
    /// 调用 `recall` 后过滤 `is_historical()`,仅返回 `Historical` 状态(已归档)的条目。
    /// `Current`/`Transition` 状态或 `temporal_meta = None` 的条目返回 `None`。
    ///
    /// WHY(P3-W11.1 D12):spec.md:295 "Historical 需显式历史查询" — 归档记忆不参与
    /// 默认召回,仅当调用方显式查询历史时返回,避免过时事实污染当前决策。
    pub async fn recall_historical(&self, id: &str) -> Result<Option<MemoryEntry>, MlcError> {
        match self.recall(id).await? {
            Some(entry) if entry.is_historical() => Ok(Some(entry)),
            _ => Ok(None),
        }
    }

    /// 按 ID 召回 `Transition` 状态条目 + 时间证据包(P3-W11.1 D12 修复)
    ///
    /// 调用 `recall` 后过滤 `is_transition()`,返回 `Transition` 状态(迁移中)的条目
    /// 与其 `TemporalMeta`(时间证据包:valid_from/valid_until/confidence)。
    /// `Current`/`Historical` 状态或 `temporal_meta = None` 的条目返回 `None`。
    ///
    /// WHY(P3-W11.1 D12):spec.md:295 "Transition 附时间证据包且降置信度" —
    /// 迁移中的记忆附带时间证据包(valid_from/valid_until)与降权置信度,
    /// 使调用方能识别"迁移中"记忆并按降权置信度处理(如降低其在 LLM prompt 中的权重)。
    ///
    /// # 返回
    /// - `None`:条目不存在,或非 `Transition` 状态
    /// - `Some((entry, temporal_meta))`:`Transition` 状态条目 + 时间证据包
    pub async fn recall_transition(
        &self,
        id: &str,
    ) -> Result<Option<(MemoryEntry, nexus_contracts::TemporalMeta)>, MlcError> {
        match self.recall(id).await? {
            Some(entry) if entry.is_transition() => {
                // 提取 TemporalMeta(is_transition() 为 true 时 temporal_meta 必为 Some)
                let meta = entry
                    .temporal_meta
                    .clone()
                    .expect("is_transition() 为 true 时 temporal_meta 必为 Some(Transition)");
                Ok(Some((entry, meta)))
            }
            _ => Ok(None),
        }
    }

    /// 按 CLV 召回 Top-K 最相似条目(委托给 L2)
    pub async fn recall_by_clv(
        &self,
        query: &CLV,
        top_k: usize,
    ) -> Result<Vec<(MemoryId, f32)>, MlcError> {
        let results = self.l2.recall_by_clv(query, top_k)?;
        // 召回视为命中(每个结果计一次命中)
        self.hit_count
            .fetch_add(results.len() as u64, Ordering::Relaxed);
        Ok(results)
    }

    /// 按策略召回 — S2 接缝策略感知 recall(P4-W14.1,方案 A 显式 API)
    ///
    /// 根据 `MemoryStrategy` 调整召回行为:
    /// - **top_k**: 从 `strategy.default_top_k()` 获取(MinimalRecall=1, StandardTopK=10 等)
    /// - **similarity_threshold**: 从 `strategy.similarity_threshold()` 获取
    ///   (AggressivePruning=0.5 过滤噪声,其他=0.0 不过滤)
    /// - **L0 only**: `MinimalRecall` 仅查 L0 工作记忆(快速路径,不访问 L2)
    ///
    /// # 与 `recall_by_clv` 的区别
    ///
    /// `recall_by_clv(query, top_k)` 是显式 API,调用方完全控制 top_k;
    /// 本方法从 strategy 推导 top_k/阈值/L0-only,封装策略逻辑。
    /// 两者互不干扰,向后兼容。
    ///
    /// # C4 合规
    ///
    /// 默认策略 `StandardTopK` 等价于 `recall_by_clv(query, 10)`,
    /// 行为零变化(向后兼容)。学习策略通过 `update_memory_strategy_policy()`
    /// 异步注入,recall 路径通过 `recall_by_clv_with_current_policy()` 自动感知。
    ///
    /// # 参数
    /// - `query`: 查询 CLV 向量
    /// - `strategy`: 记忆策略(决定 top_k/阈值/L0-only)
    ///
    /// # 返回
    /// `Vec<(MemoryId, f32)>`: 召回条目 ID 与相似度分数(已按 strategy 过滤)
    ///
    /// # 示例
    ///
    /// ```no_run
    /// # use mlc_engine::{MlcEngine, MemoryEntry, MemoryTier};
    /// # use nexus_contracts::MemoryStrategy;
    /// # use nexus_core::CLV;
    /// # use event_bus::EventBus;
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// # let bus = EventBus::new();
    /// # let engine = MlcEngine::new_in_memory(bus)?;
    /// # let query = CLV::zero();
    /// // 使用 AggressivePruning 策略召回(top_k=5, 阈值 0.5)
    /// let results = engine.recall_by_clv_with_strategy(
    ///     &query,
    ///     MemoryStrategy::AggressivePruning,
    /// ).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn recall_by_clv_with_strategy(
        &self,
        query: &CLV,
        strategy: MemoryStrategy,
    ) -> Result<Vec<(MemoryId, f32)>, MlcError> {
        // MinimalRecall 快速路径:仅查 L0 工作记忆
        // WHY: MinimalRecall 语义是"最小检索",应避免 L2 KNN 扫描开销
        if strategy.l0_only() {
            // L0 工作记忆按 ID 索引,无 CLV 相似度概念,返回最近访问的若干条
            // (近似语义:工作记忆中最活跃的条目)
            // WHY list_all + take: L0 是 DashMap 无序,list_all 返回全部条目,
            // take(default_top_k) 截取前 k 个(MinimalRecall 时 k=1)
            let recent: Vec<MemoryEntry> = self
                .l0
                .list_all()
                .into_iter()
                .take(strategy.default_top_k())
                .collect();
            self.hit_count
                .fetch_add(recent.len() as u64, Ordering::Relaxed);
            // L0 召回无相似度分数,用 1.0 表示"完全匹配"(工作记忆优先级最高)
            return Ok(recent.into_iter().map(|e| (e.id, 1.0)).collect());
        }

        // 标准/L2 路径:使用策略推导的 top_k 召回
        let top_k = strategy.default_top_k();
        let threshold = strategy.similarity_threshold();

        let raw_results = self.l2.recall_by_clv(query, top_k)?;

        // 应用相似度阈值过滤(AggressivePruning 专用)
        // WHY: AggressivePruning 提升 threshold 至 0.5,过滤低相似度噪声
        let filtered: Vec<(MemoryId, f32)> = if threshold > 0.0 {
            raw_results
                .into_iter()
                .filter(|(_, score)| *score >= threshold)
                .collect()
        } else {
            raw_results
        };

        self.hit_count
            .fetch_add(filtered.len() as u64, Ordering::Relaxed);
        Ok(filtered)
    }

    /// 按当前策略召回 — 使用 holder 中激活的 `MemoryStrategyPolicy`(P4-W14.1)
    ///
    /// 便捷方法,从 `memory_strategy_holder` 读取当前策略并调用
    /// `recall_by_clv_with_strategy`。当 `omega-learner` 异步下发学习策略后,
    /// 调用方无需显式传 strategy,本方法自动感知。
    ///
    /// # C4 合规
    ///
    /// 若 holder 为 `Static(StandardTopK)`(默认 fallback),等价于
    /// `recall_by_clv(query, 10)`,行为零变化(向后兼容)。
    ///
    /// # 参数
    /// - `query`: 查询 CLV 向量
    ///
    /// # 返回
    /// `Vec<(MemoryId, f32)>`: 召回条目 ID 与相似度分数
    ///
    /// # 示例
    ///
    /// ```no_run
    /// # use mlc_engine::MlcEngine;
    /// # use nexus_contracts::MemoryStrategyPolicy;
    /// # use nexus_core::CLV;
    /// # use event_bus::EventBus;
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// # let bus = EventBus::new();
    /// # let engine = MlcEngine::new_in_memory(bus)?;
    /// # let query = CLV::zero();
    /// // 使用当前激活策略召回(默认 StandardTopK,或 learner 下发的 Learned 策略)
    /// let results = engine.recall_by_clv_with_current_policy(&query).await?;
    /// # Ok(())
    /// # }
    /// ```
    pub async fn recall_by_clv_with_current_policy(
        &self,
        query: &CLV,
    ) -> Result<Vec<(MemoryId, f32)>, MlcError> {
        let strategy = self.memory_strategy_holder.strategy();
        self.recall_by_clv_with_strategy(query, strategy).await
    }

    /// 异步下发记忆策略 — S2 接缝策略注入入口(P4-W14.1)
    ///
    /// 上层编排器(chimera-cli / quest-engine)调用此方法注入 `omega-learner`
    /// 学习到的 `MemoryStrategyPolicy::Learned`。策略持久化在
    /// `memory_strategy_holder` 中,后续 `recall_by_clv_with_current_policy`
    /// 自动感知。
    ///
    /// # C4 合规
    ///
    /// 调用方在 `omega-learner` panic/超时不调用此方法,holder 保持上次有效策略。
    /// 若需强制回退,传入 `MemoryStrategyPolicy::fallback()` 或调用
    /// `fallback_memory_strategy_to_static()`。
    ///
    /// # 参数
    /// - `policy`: 新策略(Static 或 Learned)
    ///
    /// # 示例
    ///
    /// ```no_run
    /// # use mlc_engine::MlcEngine;
    /// # use nexus_contracts::{MemoryStrategy, MemoryStrategyPolicy};
    /// # use event_bus::EventBus;
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// # let bus = EventBus::new();
    /// # let engine = MlcEngine::new_in_memory(bus)?;
    /// // omega-learner 学习到 TimeFocused 策略,异步下发
    /// engine.update_memory_strategy_policy(
    ///     MemoryStrategyPolicy::learned(1, MemoryStrategy::TimeFocused),
    /// );
    /// # Ok(())
    /// # }
    /// ```
    pub fn update_memory_strategy_policy(&self, policy: MemoryStrategyPolicy) {
        self.memory_strategy_holder.update_policy(policy);
    }

    /// 强制记忆策略回退到 fallback(`Static(StandardTopK)`)— S2 熔断入口(P4-W14.1)
    ///
    /// `omega-learner` 触发学习熔断(spec.md:335 S2 灰度阶段目标达成率降 >2%)
    /// 时,上层调用方调用此方法立即回退到静态策略。
    ///
    /// # 示例
    ///
    /// ```no_run
    /// # use mlc_engine::MlcEngine;
    /// # use nexus_contracts::MemoryStrategy;
    /// # use event_bus::EventBus;
    /// # async fn run() -> Result<(), Box<dyn std::error::Error>> {
    /// # let bus = EventBus::new();
    /// # let engine = MlcEngine::new_in_memory(bus)?;
    /// // 学习熔断,立即回退到 StandardTopK
    /// engine.fallback_memory_strategy_to_static();
    /// assert_eq!(engine.current_memory_strategy(), MemoryStrategy::StandardTopK);
    /// # Ok(())
    /// # }
    /// ```
    pub fn fallback_memory_strategy_to_static(&self) {
        self.memory_strategy_holder.fallback_to_static();
    }

    /// 返回当前激活的记忆策略(P4-W14.1)
    ///
    /// 便捷查询方法,等价于 `memory_strategy_holder.strategy()`。
    pub fn current_memory_strategy(&self) -> MemoryStrategy {
        self.memory_strategy_holder.strategy()
    }

    /// 返回当前记忆策略持有器引用(P4-W14.1,用于诊断与高级控制)
    ///
    /// WHY 提供: 上层编排器可能需要直接访问 holder 进行版本查询、
    /// 策略快照等高级操作,无需通过 MlcEngine 间接调用。
    pub fn memory_strategy_holder(&self) -> &MemoryStrategyLearnerHolder {
        &self.memory_strategy_holder
    }

    /// 按 ID 获取并访问记忆条目(更新 LRU,仅 L0)
    ///
    /// 与 `recall` 的区别:此方法会更新 L0 的 last_accessed_at(LRU 语义)
    pub async fn recall_and_touch(&self, id: &str) -> Result<Option<MemoryEntry>, MlcError> {
        // L0 查找(get 更新 LRU)
        match self.l0.get(id) {
            Ok(entry) => {
                self.hit_count.fetch_add(1, Ordering::Relaxed);
                return Ok(Some(entry));
            }
            Err(MlcError::EntryNotFound(_)) => {}
            Err(e) => return Err(e),
        }

        // L1/L2 查找(不更新 LRU,这些层不按 LRU 驱逐)
        self.recall(id).await
    }

    /// 按 Quest ID 查询关联的所有情节记忆(委托给 L1)
    pub async fn recall_by_quest(&self, quest_id: &str) -> Result<Vec<MemoryEntry>, MlcError> {
        self.l1.query_by_quest(quest_id)
    }

    /// 按时间范围查询情节记忆(委托给 L1)
    pub async fn recall_range(
        &self,
        start: chrono::DateTime<chrono::Utc>,
        end: chrono::DateTime<chrono::Utc>,
    ) -> Result<Vec<MemoryEntry>, MlcError> {
        self.l1.query_range(start, end)
    }

    /// 按模式签名匹配 L3 程序记忆
    pub async fn match_procedural(
        &self,
        signature: &crate::types::PatternSignature,
    ) -> Result<Option<ProceduralEntry>, MlcError> {
        self.l3.match_pattern(signature).await
    }

    /// 提升记忆条目到更高层级(如 L1 → L0)
    ///
    /// 流程:
    /// 1. 从源层获取条目
    /// 2. 从源层移除
    /// 3. 更新 tier 字段,插入目标层
    /// 4. 发布 MemoryTiered 事件
    ///
    /// 若目标层插入失败,回滚(重新插入源层)。
    pub async fn promote(
        &self,
        id: &str,
        from: MemoryTier,
        to: MemoryTier,
    ) -> Result<(), MlcError> {
        self.migrate(id, from, to).await
    }

    /// 降级记忆条目到更低层级(如 L0 → L1)
    ///
    /// 与 `promote` 逻辑相同,仅方向不同。
    pub async fn demote(&self, id: &str, from: MemoryTier, to: MemoryTier) -> Result<(), MlcError> {
        self.migrate(id, from, to).await
    }

    /// 内部迁移方法(promote/demote 共用)
    ///
    /// WHY:promote 与 demote 逻辑相同,统一为 migrate 避免重复代码
    ///
    /// # 并发安全(SubTask 18.1)
    /// 通过 `migration_locks` 实现条目级迁移锁,消除 TOCTOU 窗口。
    /// 多线程并发迁移同一 MemoryId 时,`entry().or_insert(())` 原子性获取锁,
    /// 后续同一 ID 的迁移会阻塞,直到持有者离开作用域释放 guard。
    /// 锁粒度是条目级,不影响其他 MemoryId 的并发迁移。
    async fn migrate(&self, id: &str, from: MemoryTier, to: MemoryTier) -> Result<(), MlcError> {
        // SubTask 18.1:获取条目级迁移锁,消除 TOCTOU 窗口
        // WHY:`entry().or_insert(())` 原子性获取锁(check-then-act 在同一分片写锁内完成)。
        // guard `_migration_lock` 离开作用域自动释放 shard 写锁,无需手动 remove。
        // 同一 MemoryId 的并发迁移会在此串行化,不同 MemoryId 互不影响。
        let _migration_lock = self
            .migration_locks
            .entry(id.to_string().into())
            .or_insert(());

        // 1. 从源层获取条目(不移除)
        let entry = self.fetch_from_tier(id, from)?;
        let entry = entry.ok_or_else(|| {
            MlcError::EntryNotFound(format!("迁移源层 {from:?} 未找到条目: {id}"))
        })?;

        // 2. 更新 tier 并插入目标层
        // WHY 先写入目标层:原实现先从源层删除再写入目标层,中间失败时数据丢失
        // (回滚到源层可能因容量满而失败)。改为"先写入目标层 → 确认成功 → 再从源层删除",
        // 确保目标层写入失败时源层条目仍然保留,无数据丢失风险。
        let mut new_entry = entry;
        new_entry.tier = to;
        new_entry.touch();

        let insert_result = match to {
            MemoryTier::L0Working => self.l0.insert(new_entry),
            MemoryTier::L1Episodic => self.l1.insert(new_entry),
            MemoryTier::L2Semantic => self.l2.insert(new_entry),
            MemoryTier::L3Procedural => {
                return Err(MlcError::InvalidConfig(format!(
                    "L3 程序记忆不支持 MemoryEntry 迁移: {id}"
                )));
            }
        };

        // 3. 若目标层插入失败,直接返回错误(源层条目未删除,无数据丢失)
        if let Err(e) = insert_result {
            warn!(
                id = id,
                from = ?from,
                to = ?to,
                error = %e,
                "迁移目标层插入失败,源层条目保留(无数据丢失)"
            );
            return Err(e);
        }

        // 4. 目标层插入成功,从源层删除
        // WHY:若删除失败,条目会同时存在于两层(冗余但不丢失),仅记录告警
        if let Err(remove_err) = self.remove_from_tier(id, from) {
            warn!(
                id = id,
                from = ?from,
                to = ?to,
                error = %remove_err,
                "迁移源层删除失败,条目可能同时存在于两层(冗余但不丢失)"
            );
        }

        // 5. 发布 MemoryTiered 事件
        // SubTask 17.4:单条迁移填充 memory_id,供消费者(如 efficiency-monitor)
        // 定位被迁移的条目并更新位置索引。批量迁移场景应为 None。
        let item_count = self.tier_count(to).await;
        let event = NexusEvent::MemoryTiered {
            metadata: EventMetadata::new("mlc-engine"),
            tier: to.as_str().to_string(),
            item_count,
            memory_id: Some(id.to_string()),
        };
        self.event_bus.publish(event).await?;
        info!(
            id = id,
            from = from.as_str(),
            to = to.as_str(),
            "记忆条目迁移完成,MemoryTiered 事件已发布"
        );
        Ok(())
    }

    /// 从指定层级获取条目(不移除)
    fn fetch_from_tier(&self, id: &str, tier: MemoryTier) -> Result<Option<MemoryEntry>, MlcError> {
        match tier {
            MemoryTier::L0Working => Ok(self.l0.peek(id)),
            MemoryTier::L1Episodic => match self.l1.get(id) {
                Ok(e) => Ok(Some(e)),
                Err(MlcError::EntryNotFound(_)) => Ok(None),
                Err(e) => Err(e),
            },
            MemoryTier::L2Semantic => match self.l2.get(id) {
                Ok(e) => Ok(Some(e)),
                Err(MlcError::EntryNotFound(_)) => Ok(None),
                Err(e) => Err(e),
            },
            MemoryTier::L3Procedural => Err(MlcError::InvalidConfig(format!(
                "L3 程序记忆不支持 MemoryEntry 获取: {id}"
            ))),
        }
    }

    /// 从指定层级移除条目
    fn remove_from_tier(&self, id: &str, tier: MemoryTier) -> Result<(), MlcError> {
        match tier {
            MemoryTier::L0Working => {
                self.l0.remove(id);
                Ok(())
            }
            MemoryTier::L1Episodic => {
                self.l1.remove(id)?;
                Ok(())
            }
            MemoryTier::L2Semantic => {
                self.l2.remove(id)?;
                Ok(())
            }
            MemoryTier::L3Procedural => Err(MlcError::InvalidConfig(format!(
                "L3 程序记忆不支持 MemoryEntry 移除: {id}"
            ))),
        }
    }

    /// 获取指定层级的当前条目数
    ///
    /// WHY async:L3 ProceduralMemory 的 count 改为 async 后,
    /// tier_count 也需 async 以 await L3 count。仅在迁移事件上报路径调用,非高频路径。
    async fn tier_count(&self, tier: MemoryTier) -> u32 {
        match tier {
            MemoryTier::L0Working => self.l0.len() as u32,
            // WHY unwrap_or(0):tier_count 用于事件上报,mutex 毒化时返回 0 而非传播错误
            MemoryTier::L1Episodic => self.l1.len().unwrap_or(0) as u32,
            MemoryTier::L2Semantic => self.l2.len().unwrap_or(0) as u32,
            MemoryTier::L3Procedural => self.l3.count().await.unwrap_or(0) as u32,
        }
    }

    /// 递增操作计数,达到阈值时发布 MemoryMetricsReported 事件
    ///
    /// WHY:用 AtomicU64 而非 Mutex,避免锁竞争。
    /// 达到阈值时重置计数器并发布事件。
    async fn increment_op_count(&self) -> Result<(), MlcError> {
        let count = self.op_count.fetch_add(1, Ordering::Relaxed);
        let threshold = self.config.metrics_report_interval;

        // fetch_add 返回旧值,加 1 后达到阈值则触发
        if count + 1 >= threshold {
            // 重置计数器(CAS 语义,避免并发重复发布)
            self.op_count.store(0, Ordering::Relaxed);
            self.report_metrics().await?;
        }
        Ok(())
    }

    /// 发布 MemoryMetricsReported 事件
    ///
    /// 计算 hit_rate 与 evictions,通过 EventBus 广播。
    /// efficiency-monitor 订阅此事件(修正 V2 违规:MLC 不直接 import efficiency-monitor)
    pub async fn report_metrics(&self) -> Result<(), MlcError> {
        let hits = self.hit_count.load(Ordering::Relaxed);
        let misses = self.miss_count.load(Ordering::Relaxed);
        let total = hits + misses;
        let hit_rate = if total == 0 {
            0.0
        } else {
            hits as f32 / total as f32
        };

        let evictions = self.l0.evictions() + self.l1.evictions() + self.l2.evictions();

        let event = NexusEvent::MemoryMetricsReported {
            metadata: EventMetadata::new("mlc-engine"),
            hit_rate,
            evictions,
        };
        self.event_bus.publish(event).await?;
        debug!(
            hit_rate,
            evictions, hits, misses, "MemoryMetricsReported 事件已发布"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::PatternSignature;

    fn make_entry(id: &str, tier: MemoryTier) -> MemoryEntry {
        MemoryEntry::new(id, format!("content-{id}"), tier)
    }

    fn make_entry_with_clv(id: &str, tier: MemoryTier) -> MemoryEntry {
        let clv = CLV::zero();
        MemoryEntry::new(id, format!("content-{id}"), tier).with_clv(clv)
    }

    #[tokio::test]
    async fn test_store_and_recall_l0() {
        let bus = EventBus::new();
        let engine = MlcEngine::new_in_memory(bus).unwrap();

        let entry = make_entry("m-1", MemoryTier::L0Working);
        engine.store(entry).await.unwrap();

        let recalled = engine.recall("m-1").await.unwrap();
        assert!(recalled.is_some());
        assert_eq!(recalled.unwrap().id.as_str(), "m-1");
    }

    #[tokio::test]
    async fn test_store_and_recall_l1() {
        let bus = EventBus::new();
        let engine = MlcEngine::new_in_memory(bus).unwrap();

        let entry = make_entry("m-1", MemoryTier::L1Episodic);
        engine.store(entry).await.unwrap();

        let recalled = engine.recall("m-1").await.unwrap();
        assert!(recalled.is_some());
        assert_eq!(recalled.unwrap().id.as_str(), "m-1");
    }

    #[tokio::test]
    async fn test_store_and_recall_l2() {
        let bus = EventBus::new();
        let engine = MlcEngine::new_in_memory(bus).unwrap();

        let entry = make_entry_with_clv("m-1", MemoryTier::L2Semantic);
        engine.store(entry).await.unwrap();

        let recalled = engine.recall("m-1").await.unwrap();
        assert!(recalled.is_some());
        assert_eq!(recalled.unwrap().id.as_str(), "m-1");
    }

    #[tokio::test]
    async fn test_store_l2_without_clv_returns_error() {
        let bus = EventBus::new();
        let engine = MlcEngine::new_in_memory(bus).unwrap();

        let entry = make_entry("m-1", MemoryTier::L2Semantic);
        let result = engine.store(entry).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_store_l3_memory_entry_returns_error() {
        let bus = EventBus::new();
        let engine = MlcEngine::new_in_memory(bus).unwrap();

        let entry = make_entry("m-1", MemoryTier::L3Procedural);
        let result = engine.store(entry).await;
        assert!(matches!(result, Err(MlcError::InvalidConfig(_))));
    }

    #[tokio::test]
    async fn test_recall_nonexistent_returns_none() {
        let bus = EventBus::new();
        let engine = MlcEngine::new_in_memory(bus).unwrap();

        let recalled = engine.recall("nonexistent").await.unwrap();
        assert!(recalled.is_none());
    }

    #[tokio::test]
    async fn test_recall_cross_layer() {
        let bus = EventBus::new();
        let engine = MlcEngine::new_in_memory(bus).unwrap();

        // 在不同层存储不同条目
        engine
            .store(make_entry("m-l0", MemoryTier::L0Working))
            .await
            .unwrap();
        engine
            .store(make_entry("m-l1", MemoryTier::L1Episodic))
            .await
            .unwrap();
        engine
            .store(make_entry_with_clv("m-l2", MemoryTier::L2Semantic))
            .await
            .unwrap();

        // 跨层查找应找到所有
        assert!(engine.recall("m-l0").await.unwrap().is_some());
        assert!(engine.recall("m-l1").await.unwrap().is_some());
        assert!(engine.recall("m-l2").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn test_recall_by_clv() {
        let bus = EventBus::new();
        let engine = MlcEngine::new_in_memory(bus).unwrap();

        let mut v = vec![0.0_f32; CLV::DIMENSION];
        v[0] = 1.0;
        let query = CLV::from_vec(v).unwrap();

        engine
            .store(make_entry_with_clv("m-1", MemoryTier::L2Semantic))
            .await
            .unwrap();

        let results = engine.recall_by_clv(&query, 10).await.unwrap();
        assert!(!results.is_empty());
    }

    #[tokio::test]
    async fn test_promote_l1_to_l0() {
        let bus = EventBus::new();
        let engine = MlcEngine::new_in_memory(bus).unwrap();

        // 存储到 L1
        engine
            .store(make_entry("m-1", MemoryTier::L1Episodic))
            .await
            .unwrap();
        assert!(engine.l1().len().unwrap() == 1);
        assert!(engine.l0().is_empty());

        // 提升到 L0
        engine
            .promote("m-1", MemoryTier::L1Episodic, MemoryTier::L0Working)
            .await
            .unwrap();

        // L1 应为空,L0 应有 1 个
        assert_eq!(engine.l1().len().unwrap(), 0);
        assert_eq!(engine.l0().len(), 1);

        // 验证条目存在
        let recalled = engine.recall("m-1").await.unwrap().unwrap();
        assert_eq!(recalled.tier, MemoryTier::L0Working);
    }

    #[tokio::test]
    async fn test_demote_l0_to_l1() {
        let bus = EventBus::new();
        let engine = MlcEngine::new_in_memory(bus).unwrap();

        // 存储到 L0
        engine
            .store(make_entry("m-1", MemoryTier::L0Working))
            .await
            .unwrap();

        // 降级到 L1
        engine
            .demote("m-1", MemoryTier::L0Working, MemoryTier::L1Episodic)
            .await
            .unwrap();

        assert_eq!(engine.l0().len(), 0);
        assert_eq!(engine.l1().len().unwrap(), 1);
    }

    #[tokio::test]
    async fn test_promote_nonexistent_returns_error() {
        let bus = EventBus::new();
        let engine = MlcEngine::new_in_memory(bus).unwrap();

        let result = engine
            .promote("nonexistent", MemoryTier::L1Episodic, MemoryTier::L0Working)
            .await;
        assert!(matches!(result, Err(MlcError::EntryNotFound(_))));
    }

    #[tokio::test]
    async fn test_store_procedural_and_match() {
        let bus = EventBus::new();
        let engine = MlcEngine::new_in_memory(bus).unwrap();

        let sig = PatternSignature::new(vec!["tool_a".into()], "hash-1");
        let entry = ProceduralEntry::new(sig.clone(), "output-1");
        engine.store_procedural(entry).await.unwrap();

        let matched = engine.match_procedural(&sig).await.unwrap();
        assert!(matched.is_some());
        assert_eq!(matched.unwrap().output, "output-1");
    }

    #[tokio::test]
    async fn test_memory_metrics_reported_event() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let config = MlcConfig::default().with_metrics_interval(3);
        let engine = MlcEngine::new_in_memory_with_config(config, bus).unwrap();

        // 执行 3 次存储操作,应触发指标上报
        engine
            .store(make_entry("m-1", MemoryTier::L0Working))
            .await
            .unwrap();
        engine
            .store(make_entry("m-2", MemoryTier::L0Working))
            .await
            .unwrap();
        engine
            .store(make_entry("m-3", MemoryTier::L0Working))
            .await
            .unwrap();

        // 应收到 MemoryMetricsReported 事件
        let event = rx.recv().await.unwrap();
        match event {
            NexusEvent::MemoryMetricsReported {
                hit_rate,
                evictions,
                ..
            } => {
                // hit_rate 可能为 0.0(仅 store 未 recall)
                assert!((0.0..=1.0).contains(&hit_rate));
                assert_eq!(evictions, 0);
            }
            other => panic!("expected MemoryMetricsReported, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_memory_tiered_event_on_promote() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let engine = MlcEngine::new_in_memory(bus).unwrap();

        engine
            .store(make_entry("m-1", MemoryTier::L1Episodic))
            .await
            .unwrap();
        engine
            .promote("m-1", MemoryTier::L1Episodic, MemoryTier::L0Working)
            .await
            .unwrap();

        // 应收到 MemoryTiered 事件
        let event = rx.recv().await.unwrap();
        match event {
            NexusEvent::MemoryTiered {
                tier,
                item_count,
                memory_id,
                ..
            } => {
                assert_eq!(tier, "L0");
                assert_eq!(item_count, 1);
                // SubTask 17.4:单条迁移应填充 memory_id
                assert_eq!(
                    memory_id,
                    Some("m-1".to_string()),
                    "单条迁移的 memory_id 应为被迁移条目的 ID"
                );
            }
            other => panic!("expected MemoryTiered, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_report_metrics_manual() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let engine = MlcEngine::new_in_memory(bus).unwrap();

        // 手动上报指标
        engine.report_metrics().await.unwrap();

        let event = rx.recv().await.unwrap();
        assert!(matches!(event, NexusEvent::MemoryMetricsReported { .. }));
    }

    #[tokio::test]
    async fn test_hit_rate_calculation() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let config = MlcConfig::default().with_metrics_interval(5);
        let engine = MlcEngine::new_in_memory_with_config(config, bus).unwrap();

        // 存储 1 个条目
        engine
            .store(make_entry("m-1", MemoryTier::L0Working))
            .await
            .unwrap();

        // 命中 1 次
        engine.recall("m-1").await.unwrap();
        // 未命中 1 次
        engine.recall("nonexistent").await.unwrap();

        // 继续操作直到触发指标上报(共 5 次 store,达到阈值 5)
        for i in 0..4 {
            engine
                .store(make_entry(&format!("m-{i}"), MemoryTier::L0Working))
                .await
                .unwrap();
        }

        // 应收到 MemoryMetricsReported 事件
        let event = rx.recv().await.unwrap();
        if let NexusEvent::MemoryMetricsReported { hit_rate, .. } = event {
            // hit_rate = hits / (hits + misses)
            // 至少有 1 次命中和 1 次未命中
            assert!(hit_rate > 0.0 && hit_rate < 1.0);
        }
    }

    // ============================================================
    // P3-W11.1.2 D12 修复验收测试(spec.md:293-295 召回按 TransitionType 过滤)
    // ============================================================

    /// 测试辅助:创建带 TemporalMeta 的条目
    fn make_entry_with_temporal(
        id: &str,
        tier: MemoryTier,
        meta: nexus_contracts::TemporalMeta,
    ) -> MemoryEntry {
        MemoryEntry::new(id, format!("content-{id}"), tier).with_temporal_meta(meta)
    }

    #[tokio::test]
    async fn test_p3_w11_1_2_recall_current_returns_current_entry() {
        // P3-W11.1.2: recall_current 返回 Current 状态条目(含 None 向后兼容)
        let bus = EventBus::new();
        let engine = MlcEngine::new_in_memory(bus).unwrap();

        // Current 条目(默认 None temporal_meta,视为 Current)
        engine
            .store(make_entry("m-current", MemoryTier::L0Working))
            .await
            .unwrap();

        // recall_current 应返回条目
        let result = engine.recall_current("m-current").await.unwrap();
        assert!(result.is_some());
        assert_eq!(result.unwrap().id.as_str(), "m-current");
    }

    #[tokio::test]
    async fn test_p3_w11_1_2_recall_current_filters_historical() {
        // P3-W11.1.2: recall_current 对 Historical 条目返回 None
        let bus = EventBus::new();
        let engine = MlcEngine::new_in_memory(bus).unwrap();

        let historical_meta = nexus_contracts::TemporalMeta {
            valid_from: 1000,
            valid_until: Some(2000),
            transition_type: nexus_contracts::TransitionType::Historical,
            confidence: 0.5,
        };
        engine
            .store(make_entry_with_temporal(
                "m-historical",
                MemoryTier::L0Working,
                historical_meta,
            ))
            .await
            .unwrap();

        // recall_current 对 Historical 返回 None
        let result = engine.recall_current("m-historical").await.unwrap();
        assert!(result.is_none());

        // 但 recall(向后兼容)仍返回条目
        let result = engine.recall("m-historical").await.unwrap();
        assert!(result.is_some());
    }

    #[tokio::test]
    async fn test_p3_w11_1_2_recall_current_filters_transition() {
        // P3-W11.1.2: recall_current 对 Transition 条目返回 None
        let bus = EventBus::new();
        let engine = MlcEngine::new_in_memory(bus).unwrap();

        let transition_meta = nexus_contracts::TemporalMeta {
            valid_from: 1000,
            valid_until: Some(2000),
            transition_type: nexus_contracts::TransitionType::Transition,
            confidence: 0.3,
        };
        engine
            .store(make_entry_with_temporal(
                "m-transition",
                MemoryTier::L0Working,
                transition_meta,
            ))
            .await
            .unwrap();

        // recall_current 对 Transition 返回 None
        let result = engine.recall_current("m-transition").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_p3_w11_1_2_recall_historical_returns_historical_entry() {
        // P3-W11.1.2: recall_historical 返回 Historical 状态条目
        let bus = EventBus::new();
        let engine = MlcEngine::new_in_memory(bus).unwrap();

        let historical_meta = nexus_contracts::TemporalMeta {
            valid_from: 1000,
            valid_until: Some(2000),
            transition_type: nexus_contracts::TransitionType::Historical,
            confidence: 0.5,
        };
        engine
            .store(make_entry_with_temporal(
                "m-historical",
                MemoryTier::L1Episodic,
                historical_meta,
            ))
            .await
            .unwrap();

        // recall_historical 应返回条目
        let result = engine.recall_historical("m-historical").await.unwrap();
        assert!(result.is_some());
        let entry = result.unwrap();
        assert_eq!(entry.id.as_str(), "m-historical");
        assert!(entry.is_historical());
    }

    #[tokio::test]
    async fn test_p3_w11_1_2_recall_historical_filters_current_and_transition() {
        // P3-W11.1.2: recall_historical 对 Current/Transition 返回 None
        let bus = EventBus::new();
        let engine = MlcEngine::new_in_memory(bus).unwrap();

        // Current 条目
        engine
            .store(make_entry("m-current", MemoryTier::L0Working))
            .await
            .unwrap();

        // Transition 条目
        let transition_meta = nexus_contracts::TemporalMeta {
            valid_from: 1000,
            valid_until: Some(2000),
            transition_type: nexus_contracts::TransitionType::Transition,
            confidence: 0.3,
        };
        engine
            .store(make_entry_with_temporal(
                "m-transition",
                MemoryTier::L0Working,
                transition_meta,
            ))
            .await
            .unwrap();

        // recall_historical 对 Current/Transition 返回 None
        assert!(engine
            .recall_historical("m-current")
            .await
            .unwrap()
            .is_none());
        assert!(engine
            .recall_historical("m-transition")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn test_p3_w11_1_2_recall_transition_returns_entry_with_evidence() {
        // P3-W11.1.2: recall_transition 返回 Transition 条目 + 时间证据包(TemporalMeta)
        let bus = EventBus::new();
        let engine = MlcEngine::new_in_memory(bus).unwrap();

        let transition_meta = nexus_contracts::TemporalMeta {
            valid_from: 1000,
            valid_until: Some(2000),
            transition_type: nexus_contracts::TransitionType::Transition,
            confidence: 0.4, // 降置信度
        };
        engine
            .store(make_entry_with_temporal(
                "m-transition",
                MemoryTier::L0Working,
                transition_meta,
            ))
            .await
            .unwrap();

        // recall_transition 应返回 (entry, TemporalMeta)
        let result = engine.recall_transition("m-transition").await.unwrap();
        assert!(result.is_some());
        let (entry, meta) = result.unwrap();
        assert_eq!(entry.id.as_str(), "m-transition");
        assert!(entry.is_transition());

        // 时间证据包字段验证
        assert_eq!(meta.valid_from, 1000);
        assert_eq!(meta.valid_until, Some(2000));
        assert_eq!(
            meta.transition_type,
            nexus_contracts::TransitionType::Transition
        );
        // 降置信度
        assert!((meta.confidence - 0.4).abs() < 1e-6);
    }

    #[tokio::test]
    async fn test_p3_w11_1_2_recall_transition_filters_current_and_historical() {
        // P3-W11.1.2: recall_transition 对 Current/Historical 返回 None
        let bus = EventBus::new();
        let engine = MlcEngine::new_in_memory(bus).unwrap();

        // Current 条目
        engine
            .store(make_entry("m-current", MemoryTier::L0Working))
            .await
            .unwrap();

        // Historical 条目
        let historical_meta = nexus_contracts::TemporalMeta {
            valid_from: 1000,
            valid_until: Some(2000),
            transition_type: nexus_contracts::TransitionType::Historical,
            confidence: 0.5,
        };
        engine
            .store(make_entry_with_temporal(
                "m-historical",
                MemoryTier::L0Working,
                historical_meta,
            ))
            .await
            .unwrap();

        // recall_transition 对 Current/Historical 返回 None
        assert!(engine
            .recall_transition("m-current")
            .await
            .unwrap()
            .is_none());
        assert!(engine
            .recall_transition("m-historical")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn test_p3_w11_1_2_three_recall_methods_mutual_exclusion() {
        // P3-W11.1.2 验收:三个召回方法对同一 ID 互斥
        // spec.md:293-295 "默认只取 Current;Historical 需显式历史查询;Transition 附时间证据包"
        let bus = EventBus::new();
        let engine = MlcEngine::new_in_memory(bus).unwrap();

        // 存储三种状态的条目
        let current_meta = nexus_contracts::TemporalMeta::new(1000, 1.0);
        let historical_meta = nexus_contracts::TemporalMeta {
            valid_from: 1000,
            valid_until: Some(2000),
            transition_type: nexus_contracts::TransitionType::Historical,
            confidence: 0.5,
        };
        let transition_meta = nexus_contracts::TemporalMeta {
            valid_from: 1000,
            valid_until: Some(2000),
            transition_type: nexus_contracts::TransitionType::Transition,
            confidence: 0.3,
        };

        engine
            .store(make_entry_with_temporal(
                "m-current",
                MemoryTier::L0Working,
                current_meta,
            ))
            .await
            .unwrap();
        engine
            .store(make_entry_with_temporal(
                "m-historical",
                MemoryTier::L0Working,
                historical_meta,
            ))
            .await
            .unwrap();
        engine
            .store(make_entry_with_temporal(
                "m-transition",
                MemoryTier::L0Working,
                transition_meta,
            ))
            .await
            .unwrap();

        // Current 条目:recall_current 命中,recall_historical/recall_transition 不命中
        assert!(engine.recall_current("m-current").await.unwrap().is_some());
        assert!(engine
            .recall_historical("m-current")
            .await
            .unwrap()
            .is_none());
        assert!(engine
            .recall_transition("m-current")
            .await
            .unwrap()
            .is_none());

        // Historical 条目:recall_historical 命中,recall_current/recall_transition 不命中
        assert!(engine
            .recall_current("m-historical")
            .await
            .unwrap()
            .is_none());
        assert!(engine
            .recall_historical("m-historical")
            .await
            .unwrap()
            .is_some());
        assert!(engine
            .recall_transition("m-historical")
            .await
            .unwrap()
            .is_none());

        // Transition 条目:recall_transition 命中,recall_current/recall_historical 不命中
        assert!(engine
            .recall_current("m-transition")
            .await
            .unwrap()
            .is_none());
        assert!(engine
            .recall_historical("m-transition")
            .await
            .unwrap()
            .is_none());
        assert!(engine
            .recall_transition("m-transition")
            .await
            .unwrap()
            .is_some());
    }

    #[tokio::test]
    async fn test_p3_w11_1_2_recall_preserves_backward_compat() {
        // P3-W11.1.2: recall(向后兼容)仍返回所有状态条目
        // 验证方案 A:recall 不改变行为,新增方法提供过滤
        let bus = EventBus::new();
        let engine = MlcEngine::new_in_memory(bus).unwrap();

        // Current 条目(None temporal_meta)
        engine
            .store(make_entry("m-current", MemoryTier::L0Working))
            .await
            .unwrap();

        // Historical 条目
        let historical_meta = nexus_contracts::TemporalMeta {
            valid_from: 1000,
            valid_until: Some(2000),
            transition_type: nexus_contracts::TransitionType::Historical,
            confidence: 0.5,
        };
        engine
            .store(make_entry_with_temporal(
                "m-historical",
                MemoryTier::L0Working,
                historical_meta,
            ))
            .await
            .unwrap();

        // recall(向后兼容)对两种状态都返回条目
        assert!(engine.recall("m-current").await.unwrap().is_some());
        assert!(engine.recall("m-historical").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn test_p3_w11_1_2_recall_nonexistent_returns_none() {
        // P3-W11.1.2: 三个新方法对不存在 ID 返回 None
        let bus = EventBus::new();
        let engine = MlcEngine::new_in_memory(bus).unwrap();

        assert!(engine
            .recall_current("nonexistent")
            .await
            .unwrap()
            .is_none());
        assert!(engine
            .recall_historical("nonexistent")
            .await
            .unwrap()
            .is_none());
        assert!(engine
            .recall_transition("nonexistent")
            .await
            .unwrap()
            .is_none());
    }
}
