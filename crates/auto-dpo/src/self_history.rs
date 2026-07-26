//! 自比较历史持久化 — RHI-CG 通道 A 的记忆侧（P5.1.3）
//!
//! 对应架构层: L5 Knowledge（依赖 L2 Memory）
//! 对应 ADR: ADR-044（P5 工程实施）决策 3
//! 对应设计源: `NEXUS-OMEGA_v5.0_系统性完整设计文档.md` §7.4（RHI-CG 双通道）
//! 对应任务: **P5.1.3**（自比较历史持久化到 mlc-engine L2 语义记忆）
//!
//! # 核心职责
//!
//! 将 RHI-CG 通道 A 生成的 `PreferencePair` + `JudgeVerdict` 元数据持久化到
//! `mlc_engine::SemanticMemory`（L2 语义记忆），供后续 spec 进化决策时
//! 按版本对检索历史比较结果，避免重复评判、支持趋势分析。
//!
//! # 架构定位
//!
//! ```text
//! RHI-CG 通道 A 主流程（rhi_channel_a.rs）:
//!   spec_v_i / spec_v_i_minus_1
//!     ↓ JudgeClient::judge()
//!   JudgeVerdict
//!     ↓ PreferencePair::from_adjacent_specs()
//!   PreferencePair
//!     ↓ SelfComparisonHistory::store()    ←── 本文件
//!   mlc-engine L2 SemanticMemory
//!     ↓ SelfComparisonHistory::recall_by_pair_id()
//!   下游 gsoe-evolution 检索历史
//! ```
//!
//! # 设计决策（WHY）
//!
//! ## 1. 复用 mlc-engine SemanticMemory（C2 决策）
//!
//! 不新建存储后端，直接 wrap `SemanticMemory`：
//! - 已有 `RwLock` + FIFO 驱逐 + CLV 池共享（SubTask 13.1），避免重复实现
//! - 与既有记忆体系一致，未来可通过 `MlcEngine` 聚合查询
//! - `Arc<SemanticMemory>` 允许跨任务共享（RhiChannelA 在 async 任务间传递）
//!
//! ## 2. 确定性 CLV 生成
//!
//! `pair_id` 唯一标识偏好对，需生成确定性 CLV（相同 pair_id → 相同 CLV）：
//! - 对每个维度 `i ∈ 0..512`，hash `(pair_id, i)` 得到 `u64`
//! - 取高 24 位映射到 `[-1.0, 1.0]` 浮点区间（f32 精度足够）
//! - 512 维向量保证 CLV 维度合法
//!
//! WHY 不使用语义编码：P5.1.3 阶段仅需稳定检索键，不需要真实语义。
//! 未来如需"按 spec 内容相似度检索"，可扩展 `SelfComparisonHistory::store_with_clv`
//! 接受外部 CLV 参数。
//!
//! ## 3. JSON 序列化存储
//!
//! `SelfComparisonRecord` 序列化为 JSON 字符串存入 `MemoryEntry.content`，
//! 反序列化时从 `content` 解析。`MemoryEntry.id = pair_id` 保证唯一性。
//!
//! WHY JSON 而非 MessagePack：
//! - 自比较记录无需跨进程传输，JSON 可读性优势更大（便于调试）
//! - `serde_json` 已是 workspace 依赖，无需新增
//! - 与 ADR-004（MessagePack 仅用于跨层通信）不冲突：本持久化是单 crate 内存储
//!
//! ## 4. 容量受 SemanticMemory 控制
//!
//! 默认容量 `DEFAULT_CAPACITY = 1024`（自比较记录规模远小于通用语义记忆），
//! 超出时按 FIFO 驱逐最旧记录。调用方可通过 `with_capacity` 自定义。
//!
//! ## 5. 不变量保护
//!
//! - **pair_id 唯一性**：相同 pair_id 重复存储会覆盖（SemanticMemory 的 `insert`
//!   检测到已存在条目时先移除旧向量再插入新向量，不触发驱逐）
//! - **CLV 一致性**：相同 pair_id 总是生成相同 CLV，保证召回稳定性
//! - **时间戳单调性**：`created_at` 由 `Utc::now()` 生成，单调递增（UTC 时钟保证）
//!
//! # 学习不在关键路径（设计 §7.1）
//!
//! `SelfComparisonHistory::store()` 是异步触发的，但调用方（RHI-CG 编排器）
//! **不在请求关键路径**：
//! - 编排器在 spec 版本切换后异步触发持久化
//! - 失败仅记日志，不阻塞主流程（"沿用上一版本 spec" 是 fallback）
//! - 调用方应使用 `tokio::spawn` 包装 `store()` 调用，失败仅记 tracing::warn

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use mlc_engine::{MemoryEntry, MemoryId, MemoryTier, MlcError, SemanticMemory};
use nexus_core::CLV;
use serde::{Deserialize, Serialize};

use crate::error::AutoDpoError;
use crate::rhi_channel_a::JudgeVerdict;
use crate::types::PreferencePair;

// ============================================================
// 默认配置常量
// ============================================================

/// 自比较历史默认容量 — 1024 条记录
///
/// WHY 1024 而非 4096（L2 默认容量）：
/// - 自比较记录每次 spec 版本切换才产生一条，频率远低于通用语义记忆
/// - 1024 条记录覆盖约 1024 次版本演进（远超实际演进频率）
/// - 内存占用：1024 × (record_json ~500B + CLV 2KB) ≈ 2.5MB，可控
pub const DEFAULT_CAPACITY: usize = 1024;

// ============================================================
// SelfComparisonRecord — 自比较历史记录
// ============================================================

/// 自比较历史记录 — 封装偏好对与评判元数据
///
/// # 字段语义
///
/// | 字段 | 类型 | 含义 |
/// |------|------|------|
/// | `pair` | `PreferencePair` | 偏好对（chosen/rejected/scores/quality） |
/// | `confidence` | `f32` | 评判器置信度 [0.0, 1.0]，下游加权训练使用 |
/// | `rationale` | `String` | 评判理由（人类可读，用于审计与调试） |
/// | `created_at` | `DateTime<Utc>` | 记录创建时间（UTC） |
///
/// # 设计决策（WHY 独立结构而非直接存 PreferencePair）
///
/// - `PreferencePair` 是数据载体，不带评判元数据（confidence/rationale）
/// - 历史查询需要按时间排序，需要 `created_at` 字段
/// - 后续可能扩展（如来源标记：LLM 评判/stub 评判），独立结构便于演进
///
/// # 序列化
///
/// 派生 `Serialize`/`Deserialize`，序列化为 JSON 存入 `MemoryEntry.content`。
/// `created_at` 使用 `chrono::DateTime<Utc>` 的 RFC3339 格式，便于跨时区解析。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SelfComparisonRecord {
    /// 偏好对（包含 pair_id/chosen/rejected/scores/quality）
    pub pair: PreferencePair,
    /// 评判置信度 [0.0, 1.0]（来自 JudgeVerdict.confidence）
    pub confidence: f32,
    /// 评判理由（人类可读，来自 JudgeVerdict.rationale）
    pub rationale: String,
    /// 记录创建时间（UTC，存储时生成）
    pub created_at: DateTime<Utc>,
}

impl SelfComparisonRecord {
    /// 从偏好对与评判结果构造记录
    ///
    /// # 参数
    /// - `pair`: 偏好对（由 `PreferencePair::from_adjacent_specs` 生成）
    /// - `verdict`: 评判结果（提供 confidence 与 rationale）
    ///
    /// # 返回
    /// 新的 `SelfComparisonRecord`，`created_at` 自动设为当前 UTC 时间
    ///
    /// # 设计决策（WHY 从 JudgeVerdict 提取而非整体存储）
    ///
    /// `JudgeVerdict` 包含 `winner`/`winner_score`/`loser_score` 字段，
    /// 这些信息已通过 `PreferencePair::from_adjacent_specs` 反映在 `pair`
    /// 的 `chosen`/`chosen_score`/`rejected_score` 中（chosen = 胜出版本）。
    /// 此处仅提取 `confidence` 与 `rationale`，避免冗余存储。
    pub fn from_pair_and_verdict(pair: PreferencePair, verdict: &JudgeVerdict) -> Self {
        Self {
            pair,
            confidence: verdict.confidence,
            rationale: verdict.rationale.clone(),
            created_at: Utc::now(),
        }
    }

    /// 返回 pair_id（便捷访问器，避免 `record.pair.pair_id` 嵌套）
    pub fn pair_id(&self) -> &str {
        &self.pair.pair_id
    }

    /// 偏好信号强度（winner_score - loser_score，来自 PreferencePair）
    ///
    /// WHY 提供便捷访问器：下游加权训练按 score_gap 排序选样本
    pub fn score_gap(&self) -> f32 {
        self.pair.score_gap()
    }
}

// ============================================================
// 确定性 CLV 生成
// ============================================================

/// 生成确定性 CLV — 基于 pair_id 哈希
///
/// # 算法
///
/// 对每个维度 `i ∈ 0..512`：
/// 1. 使用 `DefaultHasher` 哈希 `(pair_id, i as u64)`
/// 2. 取哈希值的高 24 位（`h >> 40`）
/// 3. 映射到 `[-1.0, 1.0]`：`normalized = (bits / 2^24) * 2.0 - 1.0`
///
/// # 不变量
///
/// - **确定性**：相同 `pair_id` 总是产生相同 CLV
/// - **维度合法**：固定生成 512 维向量，`CLV::from_vec` 不会失败
/// - **分布均匀**：`DefaultHasher`（SipHash-1-3）在 `[-1.0, 1.0]` 上分布均匀
///
/// # 设计决策（WHY 高 24 位）
///
/// `f32` 有效精度约 23 位尾数，取 24 位（含 1 符号位）刚好覆盖 `f32` 精度范围，
/// 避免高位被截断导致的精度损失。`u64 >> 40` 留下高 24 位。
///
/// # 参数
/// - `pair_id`: 偏好对唯一标识（如 "rhi-pair-47-46"）
///
/// # 返回
/// - `Ok(CLV)`: 512 维确定性 CLV
/// - `Err(AutoDpoError::StorageError)`: 仅在 CLV 维度不变量被违反时（理论不可能）
///
/// # 示例
///
/// ```
/// use auto_dpo::self_history::generate_deterministic_clv;
///
/// let clv1 = generate_deterministic_clv("rhi-pair-2-1").unwrap();
/// let clv2 = generate_deterministic_clv("rhi-pair-2-1").unwrap();
/// let clv3 = generate_deterministic_clv("rhi-pair-3-2").unwrap();
///
/// // 确定性：相同 pair_id 产生相同 CLV
/// assert_eq!(clv1.as_slice(), clv2.as_slice());
/// // 区分性：不同 pair_id 产生不同 CLV
/// assert_ne!(clv1.as_slice(), clv3.as_slice());
/// ```
pub fn generate_deterministic_clv(pair_id: &str) -> Result<CLV, AutoDpoError> {
    let mut vec = Vec::with_capacity(CLV::DIMENSION);

    for i in 0..CLV::DIMENSION {
        let mut hasher = DefaultHasher::new();
        pair_id.hash(&mut hasher);
        // WHY 将 i 转为 u64 再哈希：避免不同类型 hash 实现差异
        (i as u64).hash(&mut hasher);
        let h = hasher.finish();
        // 取高 24 位（h >> 40），映射到 [0.0, 1.0] 再变换到 [-1.0, 1.0]
        let bits = h >> 40;
        let normalized = ((bits as f32) / ((1u64 << 24) as f32)) * 2.0 - 1.0;
        vec.push(normalized);
    }

    // CLV::from_vec 校验维度为 512，此处保证维度合法
    // WHY map_err 而非 expect：项目规则禁止 expect()，所有失败路径返回 Result
    CLV::from_vec(vec).map_err(|e| AutoDpoError::StorageError {
        reason: format!("CLV dimension invariant violated in deterministic generation: {e:?}"),
    })
}

// ============================================================
// SelfComparisonHistory — 自比较历史持久化器
// ============================================================

/// 自比较历史持久化器 — wrap `mlc_engine::SemanticMemory`
///
/// # 职责
///
/// 1. 将 `SelfComparisonRecord` 序列化为 JSON 存入 `MemoryEntry.content`
/// 2. 生成确定性 CLV（基于 pair_id）作为 L2 向量索引键
/// 3. 提供 `store` / `get` / `recall_by_pair_id` / `list_recent` 接口
/// 4. 容量满时由 `SemanticMemory` FIFO 驱逐最旧记录
///
/// # 线程安全
///
/// - `Arc<SemanticMemory>` 包装，`SemanticMemory` 内部 `RwLock` 保证线程安全
/// - `SelfComparisonHistory` 是 `Send + Sync`，可在 async 任务间共享
/// - `store` / `get` 等方法均为 `&self`（无内部可变状态）
///
/// # 示例
///
/// ```
/// use auto_dpo::self_history::{SelfComparisonHistory, SelfComparisonRecord};
/// use auto_dpo::{PreferencePair, JudgeVerdict, SpecVersion};
///
/// // 创建历史持久化器（容量 1024）
/// let history = SelfComparisonHistory::new(1024);
///
/// // 构造测试记录
/// let pair = PreferencePair::new("rhi-pair-2-1", "chosen", "rejected", 0.85, 0.40);
/// let verdict = JudgeVerdict::new(
///     SpecVersion::Current, 0.85, 0.40, 0.90, "v2 wins on clarity",
/// ).unwrap();
/// let record = SelfComparisonRecord::from_pair_and_verdict(pair, &verdict);
///
/// // 存储
/// history.store(record.clone()).unwrap();
///
/// // 按 pair_id 检索
/// let retrieved = history.get("rhi-pair-2-1").unwrap();
/// assert_eq!(retrieved, Some(record));
/// ```
pub struct SelfComparisonHistory {
    /// 内部 L2 语义记忆（共享 Arc，允许跨任务复用）
    ///
    /// WHY `Arc<SemanticMemory>` 而非 `SemanticMemory`：
    /// - 多个 `SelfComparisonHistory` 实例可共享同一存储（如 MlcEngine 聚合查询）
    /// - `RhiChannelA` 在 async 任务间传递时需 `Arc` 共享
    /// - `SemanticMemory` 的 `insert`/`get` 接收 `&self`，无需 `&mut self`
    inner: Arc<SemanticMemory>,
}

impl SelfComparisonHistory {
    /// 创建新的自比较历史持久化器，指定容量上限
    ///
    /// # 参数
    /// - `capacity`: L2 语义记忆容量上限（超出时 FIFO 驱逐最旧记录）
    ///
    /// # 示例
    /// ```
    /// use auto_dpo::self_history::SelfComparisonHistory;
    ///
    /// let history = SelfComparisonHistory::new(512);
    /// assert_eq!(history.capacity(), 512);
    /// ```
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: Arc::new(SemanticMemory::new(capacity)),
        }
    }

    /// 创建默认容量的自比较历史持久化器（`DEFAULT_CAPACITY = 1024`）
    ///
    /// WHY 提供便捷构造器：大多数场景使用默认容量，避免调用方记住魔法数字
    pub fn with_default_capacity() -> Self {
        Self::new(DEFAULT_CAPACITY)
    }

    /// 从已有 `SemanticMemory` 构造（共享 Arc）
    ///
    /// # 使用场景
    /// - `MlcEngine` 已聚合 `SemanticMemory`，自比较历史复用同一存储
    /// - 多个 `SelfComparisonHistory` 实例共享同一存储（如测试与生产共用）
    ///
    /// # 参数
    /// - `memory`: 已有的 `SemanticMemory`（`Arc` 包装）
    pub fn from_semantic_memory(memory: Arc<SemanticMemory>) -> Self {
        Self { inner: memory }
    }

    /// 返回容量上限
    pub fn capacity(&self) -> usize {
        self.inner.capacity()
    }

    /// 返回当前记录数
    pub fn len(&self) -> Result<usize, AutoDpoError> {
        self.inner
            .len()
            .map_err(|e| AutoDpoError::StorageError {
                reason: format!("L2 SemanticMemory len() failed: {e}"),
            })
    }

    /// 是否为空
    pub fn is_empty(&self) -> Result<bool, AutoDpoError> {
        self.inner
            .is_empty()
            .map_err(|e| AutoDpoError::StorageError {
                reason: format!("L2 SemanticMemory is_empty() failed: {e}"),
            })
    }

    /// 返回内部 `SemanticMemory` 的 Arc 引用（便于与 MlcEngine 聚合）
    ///
    /// WHY 暴露内部引用：调用方可能需要将自比较历史与其他记忆统一管理
    pub fn semantic_memory(&self) -> &Arc<SemanticMemory> {
        &self.inner
    }

    /// 返回累计驱逐次数（用于监控与容量调优）
    pub fn evictions(&self) -> u64 {
        self.inner.evictions()
    }

    /// 持久化一条自比较记录
    ///
    /// # 流程
    /// 1. 生成确定性 CLV（基于 `record.pair.pair_id`）
    /// 2. 序列化 `record` 为 JSON 字符串
    /// 3. 构造 `MemoryEntry`（id = pair_id，content = JSON，tier = L2Semantic）
    /// 4. 调用 `SemanticMemory::insert` 存储
    ///
    /// # 参数
    /// - `record`: 自比较记录（包含偏好对、评判元数据、时间戳）
    ///
    /// # 返回
    /// - `Ok(Option<SelfComparisonRecord>)`: 被驱逐的旧记录（若有，容量满时触发 FIFO 驱逐）
    /// - `Err(AutoDpoError::StorageError)`: CLV 生成失败、序列化失败或存储失败
    ///
    /// # 重复存储语义
    ///
    /// 相同 `pair_id` 重复存储会**覆盖**旧记录：
    /// - `SemanticMemory::insert` 检测到 `MemoryId` 已存在时，先移除旧向量再插入新向量
    /// - 旧记录的 `MemoryEntry` 被丢弃，返回值为 `None`（非覆盖式驱逐）
    /// - 此语义保证 `pair_id` 唯一性，更新操作幂等
    ///
    /// # 不在关键路径
    ///
    /// 此方法是同步的（`SemanticMemory::insert` 非 async），但调用方应使用
    /// `tokio::spawn` 包装以避免阻塞 async runtime。失败仅记日志，不阻塞主流程。
    pub fn store(&self, record: SelfComparisonRecord) -> Result<Option<SelfComparisonRecord>, AutoDpoError> {
        // 步骤 1: 生成确定性 CLV
        let clv = generate_deterministic_clv(&record.pair.pair_id)?;

        // 步骤 2: 序列化 record 为 JSON
        let content = serde_json::to_string(&record).map_err(|e| AutoDpoError::StorageError {
            reason: format!("serialize SelfComparisonRecord failed: {e}"),
        })?;

        // 步骤 3: 构造 MemoryEntry
        // WHY MemoryId::from(pair_id): pair_id 是唯一标识，直接作为 MemoryId
        let entry = MemoryEntry::new(
            MemoryId::from(record.pair.pair_id.as_str()),
            content,
            MemoryTier::L2Semantic,
        )
        .with_clv(clv);

        // 步骤 4: 调用 SemanticMemory::insert
        // 返回值是被驱逐的 MemoryEntry（容量满时 FIFO 驱逐最旧）
        let evicted_entry = self
            .inner
            .insert(entry)
            .map_err(|e| AutoDpoError::StorageError {
                reason: format!("L2 SemanticMemory insert failed: {e}"),
            })?;

        // 反序列化被驱逐的记录（若有）
        let evicted_record = evicted_entry
            .map(|e| deserialize_record(&e.content, &e.id))
            .transpose()?;

        if let Some(ref evicted) = evicted_record {
            tracing::debug!(
                evicted_pair_id = %evicted.pair.pair_id,
                new_pair_id = %record.pair.pair_id,
                "SelfComparisonHistory: FIFO 驱逐旧记录"
            );
        }

        tracing::info!(
            pair_id = %record.pair.pair_id,
            confidence = record.confidence,
            score_gap = record.score_gap(),
            "SelfComparisonHistory: record stored"
        );

        Ok(evicted_record)
    }

    /// 按 pair_id 获取记录
    ///
    /// # 参数
    /// - `pair_id`: 偏好对唯一标识（作为 `MemoryId` 查询键）
    ///
    /// # 返回
    /// - `Ok(Some(record))`: 找到记录
    /// - `Ok(None)`: 未找到（`MlcError::EntryNotFound` 转换为 `None`）
    /// - `Err(AutoDpoError::StorageError)`: 其他存储错误（如锁毒化）
    ///
    /// # 设计决策（WHY EntryNotFound 特判）
    ///
    /// `SemanticMemory::get` 在未找到时返回 `MlcError::EntryNotFound`，
    /// 但从语义上"未找到历史记录"是合法状态（首次存储前的查询），
    /// 不应作为错误向上传播。`get` 将其转换为 `Ok(None)`。
    pub fn get(&self, pair_id: &str) -> Result<Option<SelfComparisonRecord>, AutoDpoError> {
        match self.inner.get(pair_id) {
            Ok(entry) => deserialize_record(&entry.content, &entry.id).map(Some),
            Err(MlcError::EntryNotFound(_)) => Ok(None),
            Err(e) => Err(AutoDpoError::StorageError {
                reason: format!("L2 SemanticMemory get failed: {e}"),
            }),
        }
    }

    /// 按 CLV 召回 Top-K 最相似记录
    ///
    /// 使用 `pair_id` 生成确定性 CLV，调用 `SemanticMemory::recall_by_clv` 进行 KNN 召回。
    /// 相同 `pair_id` 总是召回自身（相似度 1.0），不同 `pair_id` 的相似度由
    /// CLV 哈希分布决定（理论接近随机，无真实语义）。
    ///
    /// # 参数
    /// - `query_pair_id`: 查询偏好对 ID（生成 CLV 用于 KNN 召回）
    /// - `top_k`: 返回 Top-K 最相似记录
    ///
    /// # 返回
    /// - `Ok(Vec<(String, f32)>)`: `(pair_id, similarity)` 列表，按相似度降序
    /// - `Err(AutoDpoError::StorageError)`: CLV 生成失败或召回失败
    ///
    /// # 使用场景
    ///
    /// - 检索"相似版本对"的历史比较结果（如查询 `rhi-pair-5-4` 时召回 `rhi-pair-6-5`）
    /// - 统计历史趋势（如最近 K 次比较的胜率分布）
    pub fn recall_by_pair_id(
        &self,
        query_pair_id: &str,
        top_k: usize,
    ) -> Result<Vec<(String, f32)>, AutoDpoError> {
        let clv = generate_deterministic_clv(query_pair_id)?;

        let results = self
            .inner
            .recall_by_clv(&clv, top_k)
            .map_err(|e| AutoDpoError::StorageError {
                reason: format!("L2 SemanticMemory recall_by_clv failed: {e}"),
            })?;

        // 将 MemoryId 转换为 pair_id 字符串
        Ok(results
            .into_iter()
            .map(|(memory_id, sim)| (memory_id.to_string(), sim))
            .collect())
    }

    /// 列出最近 N 条记录（按插入顺序逆序）
    ///
    /// # 参数
    /// - `n`: 返回的最大记录数（若存储少于 N 条，返回全部）
    ///
    /// # 返回
    /// - `Ok(Vec<SelfComparisonRecord>)`: 按 `created_at` 降序的记录列表
    /// - `Err(AutoDpoError::StorageError)`: 列表或反序列化失败
    ///
    /// # 设计决策（WHY 按 created_at 而非插入顺序）
    ///
    /// `SemanticMemory::list_all` 返回 `HashMap` 迭代顺序（不确定），
    /// 需在调用方按 `created_at` 排序以保证"最近 N 条"语义。
    /// 复杂度：O(n log n)（排序）+ O(n)（反序列化），n 为当前记录数。
    /// 1024 条记录下耗时 < 10ms，可接受。
    pub fn list_recent(&self, n: usize) -> Result<Vec<SelfComparisonRecord>, AutoDpoError> {
        let entries = self
            .inner
            .list_all()
            .map_err(|e| AutoDpoError::StorageError {
                reason: format!("L2 SemanticMemory list_all failed: {e}"),
            })?;

        // 反序列化所有记录
        let mut records: Vec<SelfComparisonRecord> = entries
            .into_iter()
            .map(|entry| deserialize_record(&entry.content, &entry.id))
            .collect::<Result<Vec<_>, _>>()?;

        // 按 created_at 降序排序（最近在前）
        // WHY sort_by_key + Reverse: clippy unnecessary_sort_by lint 要求使用 sort_by_key，
        // Reverse 实现 Ord 的逆序，等价于 `b.cmp(&a)` 但符合 clippy 规范
        records.sort_by_key(|r| std::cmp::Reverse(r.created_at));

        // 截断为前 N 条
        records.truncate(n);

        Ok(records)
    }

    /// 移除指定 pair_id 的记录
    ///
    /// # 参数
    /// - `pair_id`: 要移除的偏好对 ID
    ///
    /// # 返回
    /// - `Ok(Some(record))`: 移除成功，返回被移除的记录
    /// - `Ok(None)`: 记录不存在
    /// - `Err(AutoDpoError::StorageError)`: 存储错误
    pub fn remove(&self, pair_id: &str) -> Result<Option<SelfComparisonRecord>, AutoDpoError> {
        match self.inner.remove(pair_id) {
            Ok(Some(entry)) => deserialize_record(&entry.content, &entry.id).map(Some),
            Ok(None) => Ok(None),
            Err(e) => Err(AutoDpoError::StorageError {
                reason: format!("L2 SemanticMemory remove failed: {e}"),
            }),
        }
    }

    /// 清空所有自比较历史记录
    pub fn clear(&self) -> Result<(), AutoDpoError> {
        self.inner.clear().map_err(|e| AutoDpoError::StorageError {
            reason: format!("L2 SemanticMemory clear failed: {e}"),
        })
    }
}

// ============================================================
// 内部辅助函数
// ============================================================

/// 从 `MemoryEntry.content` 反序列化为 `SelfComparisonRecord`
///
/// # 参数
/// - `content`: JSON 字符串（由 `SelfComparisonRecord::store` 序列化生成）
/// - `id`: `MemoryId`，仅用于错误上下文（不参与反序列化）
///
/// # 返回
/// - `Ok(SelfComparisonRecord)`: 反序列化成功
/// - `Err(AutoDpoError::StorageError)`: 反序列化失败（数据损坏或格式不兼容）
///
/// # 设计决策（WHY 内部函数而非 MemoryEntry 方法）
///
/// `MemoryEntry` 是 mlc-engine 的通用载体，不应依赖 auto-dpo 的具体类型。
/// 反序列化由 SelfComparisonHistory 内部负责，保持依赖方向：auto-dpo → mlc-engine。
fn deserialize_record(content: &str, id: &MemoryId) -> Result<SelfComparisonRecord, AutoDpoError> {
    serde_json::from_str(content).map_err(|e| AutoDpoError::StorageError {
        reason: format!(
            "deserialize SelfComparisonRecord failed (id={}, content_len={}): {e}",
            id,
            content.len()
        ),
    })
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rhi_channel_a::{JudgeVerdict, SpecVersion};
    use crate::types::PreferencePair;

    // ============================================================
    // 测试辅助函数
    // ============================================================

    /// 构造测试用偏好对
    fn make_test_pair(pair_id: &str, chosen_score: f32, rejected_score: f32) -> PreferencePair {
        PreferencePair::new(
            pair_id,
            format!("chosen-content-{pair_id}"),
            format!("rejected-content-{pair_id}"),
            chosen_score,
            rejected_score,
        )
    }

    /// 构造测试用评判结果
    fn make_test_verdict(winner: SpecVersion) -> JudgeVerdict {
        let (winner_score, loser_score) = match winner {
            SpecVersion::Current => (0.85, 0.40),
            SpecVersion::Previous => (0.80, 0.35),
        };
        JudgeVerdict::new(
            winner,
            winner_score,
            loser_score,
            0.90,
            format!("test verdict for {winner}"),
        )
        .expect("test verdict should be valid")
    }

    /// 构造测试用自比较记录
    fn make_test_record(pair_id: &str) -> SelfComparisonRecord {
        let pair = make_test_pair(pair_id, 0.85, 0.40);
        let verdict = make_test_verdict(SpecVersion::Current);
        SelfComparisonRecord::from_pair_and_verdict(pair, &verdict)
    }

    // ============================================================
    // SelfComparisonRecord 测试
    // ============================================================

    #[test]
    fn test_record_from_pair_and_verdict() {
        let pair = make_test_pair("rhi-pair-2-1", 0.85, 0.40);
        let verdict = make_test_verdict(SpecVersion::Current);

        let record = SelfComparisonRecord::from_pair_and_verdict(pair.clone(), &verdict);

        assert_eq!(record.pair, pair);
        assert!((record.confidence - 0.90).abs() < 1e-6);
        assert_eq!(record.rationale, "test verdict for current");
        assert!(record.created_at <= Utc::now());
    }

    #[test]
    fn test_record_pair_id_accessor() {
        let record = make_test_record("rhi-pair-3-2");
        assert_eq!(record.pair_id(), "rhi-pair-3-2");
    }

    #[test]
    fn test_record_score_gap() {
        let record = make_test_record("rhi-pair-2-1");
        // 0.85 - 0.40 = 0.45
        assert!((record.score_gap() - 0.45).abs() < 1e-6);
    }

    #[test]
    fn test_record_serde_roundtrip() {
        let record = make_test_record("rhi-pair-5-4");
        let json = serde_json::to_string(&record).unwrap();
        let restored: SelfComparisonRecord = serde_json::from_str(&json).unwrap();

        assert_eq!(restored, record);
    }

    // ============================================================
    // generate_deterministic_clv 测试
    // ============================================================

    #[test]
    fn test_deterministic_clv_dimension() {
        let clv = generate_deterministic_clv("rhi-pair-1-0").unwrap();
        assert_eq!(clv.as_slice().len(), CLV::DIMENSION);
        assert_eq!(CLV::DIMENSION, 512);
    }

    #[test]
    fn test_deterministic_clv_stable() {
        // 确定性：相同 pair_id 产生相同 CLV
        let clv1 = generate_deterministic_clv("rhi-pair-2-1").unwrap();
        let clv2 = generate_deterministic_clv("rhi-pair-2-1").unwrap();
        assert_eq!(clv1.as_slice(), clv2.as_slice());
    }

    #[test]
    fn test_deterministic_clv_different_pair_ids() {
        // 区分性：不同 pair_id 产生不同 CLV
        let clv1 = generate_deterministic_clv("rhi-pair-2-1").unwrap();
        let clv2 = generate_deterministic_clv("rhi-pair-3-2").unwrap();
        assert_ne!(clv1.as_slice(), clv2.as_slice());
    }

    #[test]
    fn test_deterministic_clv_value_range() {
        // 值域检查：所有维度应在 [-1.0, 1.0] 范围内
        let clv = generate_deterministic_clv("rhi-pair-10-9").unwrap();
        for &v in clv.as_slice() {
            assert!(
                (-1.0..=1.0).contains(&v),
                "CLV value {v} out of range [-1.0, 1.0]"
            );
        }
    }

    #[test]
    fn test_deterministic_clv_non_zero() {
        // 非零向量检查：避免所有维度都是 0.0（会导致余弦相似度 NaN）
        let clv = generate_deterministic_clv("rhi-pair-1-0").unwrap();
        let has_nonzero = clv.as_slice().iter().any(|&v| v != 0.0);
        assert!(has_nonzero, "CLV should have at least one non-zero dimension");
    }

    // ============================================================
    // SelfComparisonHistory 基本功能测试
    // ============================================================

    #[test]
    fn test_history_new_capacity() {
        let history = SelfComparisonHistory::new(512);
        assert_eq!(history.capacity(), 512);
        assert!(history.is_empty().unwrap());
        assert_eq!(history.len().unwrap(), 0);
    }

    #[test]
    fn test_history_with_default_capacity() {
        let history = SelfComparisonHistory::with_default_capacity();
        assert_eq!(history.capacity(), DEFAULT_CAPACITY);
        assert_eq!(history.capacity(), 1024);
    }

    #[test]
    fn test_history_store_and_get() {
        let history = SelfComparisonHistory::with_default_capacity();
        let record = make_test_record("rhi-pair-2-1");

        // 存储
        let evicted = history.store(record.clone()).unwrap();
        assert!(evicted.is_none(), "首次存储不应触发驱逐");
        assert_eq!(history.len().unwrap(), 1);

        // 检索
        let retrieved = history.get("rhi-pair-2-1").unwrap();
        assert_eq!(retrieved, Some(record));
    }

    #[test]
    fn test_history_get_nonexistent() {
        let history = SelfComparisonHistory::with_default_capacity();

        // 查询不存在的 pair_id 返回 Ok(None)
        let result = history.get("rhi-pair-99-98").unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_history_store_overwrite_same_pair_id() {
        let history = SelfComparisonHistory::with_default_capacity();

        // 存储第一条
        let record1 = make_test_record("rhi-pair-2-1");
        history.store(record1.clone()).unwrap();
        assert_eq!(history.len().unwrap(), 1);

        // 用相同 pair_id 覆盖存储
        let mut record2 = make_test_record("rhi-pair-2-1");
        record2.confidence = 0.75; // 修改 confidence 验证覆盖
        history.store(record2.clone()).unwrap();

        // 应仍只有 1 条记录（覆盖而非新增）
        assert_eq!(history.len().unwrap(), 1);

        // 检索到的应是新记录
        let retrieved = history.get("rhi-pair-2-1").unwrap().unwrap();
        assert!((retrieved.confidence - 0.75).abs() < 1e-6);
    }

    #[test]
    fn test_history_store_multiple_records() {
        let history = SelfComparisonHistory::with_default_capacity();

        for i in 1..=10 {
            let pair_id = format!("rhi-pair-{i}-{}", i - 1);
            let record = make_test_record(&pair_id);
            history.store(record).unwrap();
        }

        assert_eq!(history.len().unwrap(), 10);
    }

    // ============================================================
    // 容量与驱逐测试
    // ============================================================

    #[test]
    fn test_history_capacity_eviction() {
        // 容量 3，存储 5 条记录，应驱逐前 2 条
        let history = SelfComparisonHistory::new(3);

        let mut evicted_count = 0;
        for i in 1..=5 {
            let pair_id = format!("rhi-pair-{i}-{}", i - 1);
            let record = make_test_record(&pair_id);
            if history.store(record).unwrap().is_some() {
                evicted_count += 1;
            }
        }

        // 容量 3，存储 5 条，应驱逐 2 条
        assert_eq!(evicted_count, 2);
        assert_eq!(history.len().unwrap(), 3);
        assert_eq!(history.evictions(), 2);

        // 最旧的 2 条已被驱逐
        assert!(history.get("rhi-pair-1-0").unwrap().is_none());
        assert!(history.get("rhi-pair-2-1").unwrap().is_none());

        // 最新的 3 条仍存在
        assert!(history.get("rhi-pair-3-2").unwrap().is_some());
        assert!(history.get("rhi-pair-4-3").unwrap().is_some());
        assert!(history.get("rhi-pair-5-4").unwrap().is_some());
    }

    // ============================================================
    // recall_by_pair_id 测试
    // ============================================================

    #[test]
    fn test_history_recall_self_first() {
        let history = SelfComparisonHistory::with_default_capacity();

        // 存储 3 条记录
        history.store(make_test_record("rhi-pair-2-1")).unwrap();
        history.store(make_test_record("rhi-pair-3-2")).unwrap();
        history.store(make_test_record("rhi-pair-4-3")).unwrap();

        // 召回 Top-3，自身应排第一（相似度 ~1.0）
        let results = history.recall_by_pair_id("rhi-pair-3-2", 3).unwrap();

        assert_eq!(results.len(), 3);
        // 第一条应是自身
        assert_eq!(results[0].0, "rhi-pair-3-2");
        // 相似度应为 1.0（或非常接近，浮点误差容忍）
        assert!((results[0].1 - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_history_recall_top_k() {
        let history = SelfComparisonHistory::with_default_capacity();

        for i in 1..=5 {
            let pair_id = format!("rhi-pair-{i}-{}", i - 1);
            history.store(make_test_record(&pair_id)).unwrap();
        }

        // 召回 Top-2
        let results = history.recall_by_pair_id("rhi-pair-3-2", 2).unwrap();
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_history_recall_empty_storage() {
        let history = SelfComparisonHistory::with_default_capacity();

        // 空存储召回应返回空列表
        let results = history.recall_by_pair_id("rhi-pair-1-0", 5).unwrap();
        assert!(results.is_empty());
    }

    // ============================================================
    // list_recent 测试
    // ============================================================

    #[test]
    fn test_history_list_recent_ordering() {
        let history = SelfComparisonHistory::with_default_capacity();

        // 按顺序存储 5 条记录（created_at 递增）
        for i in 1..=5 {
            let pair_id = format!("rhi-pair-{i}-{}", i - 1);
            history.store(make_test_record(&pair_id)).unwrap();
            // 短暂 sleep 确保 created_at 严格递增（DateTime<Utc> 精度为纳秒，
            // 但快速连续调用可能产生相同时间戳）
            std::thread::sleep(std::time::Duration::from_millis(2));
        }

        // list_recent(3) 应返回最近 3 条（按 created_at 降序）
        let recent = history.list_recent(3).unwrap();
        assert_eq!(recent.len(), 3);

        // 验证降序排序
        assert!(recent[0].created_at >= recent[1].created_at);
        assert!(recent[1].created_at >= recent[2].created_at);
    }

    #[test]
    fn test_history_list_recent_more_than_stored() {
        let history = SelfComparisonHistory::with_default_capacity();

        history.store(make_test_record("rhi-pair-2-1")).unwrap();
        history.store(make_test_record("rhi-pair-3-2")).unwrap();

        // 请求 10 条，但只存了 2 条
        let recent = history.list_recent(10).unwrap();
        assert_eq!(recent.len(), 2);
    }

    #[test]
    fn test_history_list_recent_empty() {
        let history = SelfComparisonHistory::with_default_capacity();
        let recent = history.list_recent(5).unwrap();
        assert!(recent.is_empty());
    }

    // ============================================================
    // remove 与 clear 测试
    // ============================================================

    #[test]
    fn test_history_remove_existing() {
        let history = SelfComparisonHistory::with_default_capacity();
        history.store(make_test_record("rhi-pair-2-1")).unwrap();
        assert_eq!(history.len().unwrap(), 1);

        let removed = history.remove("rhi-pair-2-1").unwrap();
        assert!(removed.is_some());
        assert_eq!(removed.unwrap().pair.pair_id, "rhi-pair-2-1");

        assert_eq!(history.len().unwrap(), 0);
        assert!(history.get("rhi-pair-2-1").unwrap().is_none());
    }

    #[test]
    fn test_history_remove_nonexistent() {
        let history = SelfComparisonHistory::with_default_capacity();
        let removed = history.remove("rhi-pair-99-98").unwrap();
        assert!(removed.is_none());
    }

    #[test]
    fn test_history_clear() {
        let history = SelfComparisonHistory::with_default_capacity();

        for i in 1..=5 {
            let pair_id = format!("rhi-pair-{i}-{}", i - 1);
            history.store(make_test_record(&pair_id)).unwrap();
        }
        assert_eq!(history.len().unwrap(), 5);

        history.clear().unwrap();
        assert_eq!(history.len().unwrap(), 0);
        assert!(history.is_empty().unwrap());
    }

    // ============================================================
    // from_semantic_memory 共享测试
    // ============================================================

    #[test]
    fn test_history_from_semantic_memory_shares_storage() {
        use std::sync::Arc;

        // 创建一个共享的 SemanticMemory
        let shared_memory = Arc::new(SemanticMemory::new(1024));

        // 两个 SelfComparisonHistory 共享同一存储
        let history1 = SelfComparisonHistory::from_semantic_memory(Arc::clone(&shared_memory));
        let history2 = SelfComparisonHistory::from_semantic_memory(Arc::clone(&shared_memory));

        // history1 存储的记录应能被 history2 检索到
        history1.store(make_test_record("rhi-pair-2-1")).unwrap();
        let retrieved = history2.get("rhi-pair-2-1").unwrap();
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().pair.pair_id, "rhi-pair-2-1");

        // 验证 Arc 引用计数（共享存储）
        assert_eq!(Arc::strong_count(&shared_memory), 3); // shared_memory + history1 + history2
    }

    // ============================================================
    // 集成测试：RhiChannelA → SelfComparisonHistory
    // ============================================================

    #[test]
    fn test_integration_rhi_channel_a_to_history() {
        use crate::rhi_channel_a::{RhiChannelA, StubJudgeClient};
        use nexus_contracts::{ContractSpec, HarnessMeta, HopSpec, RetryPolicy};
        use std::sync::Arc;

        // 构造两个相邻 spec 版本
        fn make_spec(version: u32) -> nexus_contracts::HarnessSpec {
            nexus_contracts::HarnessSpec {
                meta: HarnessMeta {
                    name: format!("test-spec-{version}"),
                    version,
                    immutable: false,
                    parent: if version > 1 { Some(version - 1) } else { None },
                    task_type: Some("code_refactor".to_string()),
                },
                contracts: vec![ContractSpec {
                    name: "no_panic".to_string(),
                    property: "must_not_panic".to_string(),
                    description: None,
                    from: None,
                    to: None,
                    fields: Vec::new(),
                }],
                hops: vec![HopSpec {
                    name: "execute".to_string(),
                    input_type: None,
                    output_type: None,
                    contracts: Vec::new(),
                    description: None,
                    order: Vec::new(),
                    on_veto: None,
                    fallback: None,
                }],
                retry: RetryPolicy::default(),
                auxiliary: None,
            }
        }

        // 创建 RhiChannelA（使用 StubJudgeClient，Current 胜出）
        let judge = Arc::new(StubJudgeClient::current_wins());
        let channel_a = RhiChannelA::new(judge);

        // 创建历史持久化器
        let history = SelfComparisonHistory::with_default_capacity();

        // 模拟两次 spec 版本切换（v1→v2, v2→v3）
        let rt = tokio::runtime::Runtime::new().unwrap();

        rt.block_on(async {
            // 第一次：v1 → v2
            let spec_v1 = make_spec(1);
            let spec_v2 = make_spec(2);
            let pair1 = channel_a.generate_preference_pair(&spec_v2, &spec_v1).await.unwrap();

            // 持久化（需要 JudgeVerdict，但 RhiChannelA 只返回 PreferencePair）
            // 实际场景中 JudgeVerdict 由 channel_a 内部产生，但接口未暴露
            // 这里构造一个 stub verdict 用于测试
            let verdict = JudgeVerdict::new(
                SpecVersion::Current,
                pair1.chosen_score,
                pair1.rejected_score,
                0.90,
                "stub verdict for integration test",
            )
            .unwrap();
            let record1 = SelfComparisonRecord::from_pair_and_verdict(pair1, &verdict);
            history.store(record1).unwrap();

            // 第二次：v2 → v3
            let spec_v3 = make_spec(3);
            let pair2 = channel_a.generate_preference_pair(&spec_v3, &spec_v2).await.unwrap();
            let verdict2 = JudgeVerdict::new(
                SpecVersion::Current,
                pair2.chosen_score,
                pair2.rejected_score,
                0.85,
                "stub verdict 2 for integration test",
            )
            .unwrap();
            let record2 = SelfComparisonRecord::from_pair_and_verdict(pair2, &verdict2);
            history.store(record2).unwrap();
        });

        // 验证两条记录都已持久化
        assert_eq!(history.len().unwrap(), 2);

        // 验证可按 pair_id 检索
        let r1 = history.get("rhi-pair-2-1").unwrap();
        assert!(r1.is_some());
        assert_eq!(r1.unwrap().pair.pair_id, "rhi-pair-2-1");

        let r2 = history.get("rhi-pair-3-2").unwrap();
        assert!(r2.is_some());
        assert_eq!(r2.unwrap().pair.pair_id, "rhi-pair-3-2");

        // 验证 list_recent 返回两条
        let recent = history.list_recent(10).unwrap();
        assert_eq!(recent.len(), 2);
    }
}
