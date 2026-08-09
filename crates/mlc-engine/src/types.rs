//! MLC 核心领域类型 — 四级神经形态记忆的统一数据模型
//!
//! 对应架构层:L2 Memory
//! 对应创新点:MLC(Multi-Level Context,四级神经形态记忆)
//!
//! # 类型职责
//! - `MemoryId`/`QuestId`:记忆条目与 Quest 的唯一标识
//! - `MemoryTier`:四级分层标识(L0 Working / L1 Episodic / L2 Semantic / L3 Procedural)
//! - `MemoryEntry`:统一的记忆条目载体,跨四级复用
//! - `PatternSignature`:L3 程序记忆的模式签名(工具序列 + 上下文哈希)
//! - `ProceduralEntry`/`ExecutionStats`:L3 程序记忆条目与执行统计
//!
//! # 设计决策(WHY)
//! - **统一 MemoryEntry**:四级记忆复用同一载体,通过 `tier` 字段区分所在层级,
//!   避免 L0/L1/L2/L3 各自定义不同结构导致的转换开销与一致性维护成本
//! - **CLV 可选**:L0/L1 不强制要求 CLV(工作记忆与情节记忆按时间/Quest 索引),
//!   L2 语义记忆必须携带 CLV 用于向量召回,因此 `clv` 设为 `Option`
//! - **PatternSignature 为结构体**:而非裸 String,便于后续扩展模式匹配算法
//!   (如编辑距离、子序列匹配),Week 3 阶段使用精确匹配

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use nexus_contracts::{ArchiveTier, TemporalMeta, TransitionType};
use nexus_core::CLV;
use serde::{Deserialize, Serialize};

use crate::error::MlcError;

// 使用 nexus_core 共享的 id_newtype! 宏(SubTask 21.1)
// WHY:消除与 osa-coordinator / kvbsr-router 的 newtype 实现重复,
// 统一 ID 类型行为(Deref / AsRef / Borrow / From / Display / serde(transparent))
nexus_core::id_newtype!(MemoryId, "记忆条目唯一标识 — 四级记忆的统一标识");
nexus_core::id_newtype!(
    QuestId,
    "Quest 唯一标识 — L1 EpisodicMemory 的 Quest 索引键"
);

/// 共享 CLV — 通过 `Arc<[f32]>` 共享相同内容 CLV 的内存
///
/// WHY:nexus-core 的 `CLV` 内部是 `Array1<f32>`,每条目独立分配约 2KB
/// (512 × 4 字节)。L2 语义记忆 4096 条目共 8MB,但实际场景中许多条目
/// CLV 内容相同(如默认向量、模板向量、批量编码结果),通过 `Arc` 共享
/// 可将重复 CLV 的内存占用从 O(n × 2KB) 降至 O(k × 2KB)(k 为不同 CLV 数)。
///
/// # 共享机制
/// 通过 `intern` 方法在 CLV 池(`HashMap<u64, Arc<[f32]>>`)中查重:
/// - 内容相同的 CLV 复用同一个 `Arc`(仅增加引用计数,无内存分配)
/// - 内容不同的 CLV 创建新 `Arc` 并入池
///
/// # 哈希策略
/// f32 不实现 `Hash`(因 NaN 有多种位模式),用 `to_bits()` 将 f32 转为 u32
/// 再哈希,避免 `unsafe` 代码(`#![forbid(unsafe_code)]` 约束)。
#[derive(Debug, Clone)]
pub struct SharedCLV(Arc<[f32]>);

impl SharedCLV {
    /// 从 CLV 构造 SharedCLV(拷贝数据到 `Arc<[f32]>`,不共享)
    ///
    /// 用于无需共享的场景(如临时构造 query 向量)。
    pub fn from_clv(clv: &CLV) -> Self {
        let slice = clv.as_slice();
        Self(Arc::from(slice))
    }

    /// 从 CLV 构造 SharedCLV,通过池实现内容去重共享
    ///
    /// - 计算切片内容哈希,查池
    /// - 若池中存在相同哈希且内容完全相同,复用 `Arc`(零拷贝)
    /// - 若不存在或哈希冲突(内容不同),创建新 `Arc` 入池
    ///
    /// 返回构造的 SharedCLV。调用方负责在条目被驱逐时调用 `release_from_pool`
    /// 清理池中无引用的 Arc(避免池无限增长)。
    pub fn intern(clv: &CLV, pool: &mut std::collections::HashMap<u64, Arc<[f32]>>) -> Self {
        let slice = clv.as_slice();
        let hash = hash_f32_slice(slice);
        if let Some(existing) = pool.get(&hash) {
            if existing.as_ref() == slice {
                return Self(existing.clone());
            }
        }
        let arc: Arc<[f32]> = Arc::from(slice);
        pool.insert(hash, arc.clone());
        Self(arc)
    }

    /// 从已有 `Arc<[f32]>` 构造(用于池命中时直接复用)
    pub fn from_arc(arc: Arc<[f32]>) -> Self {
        Self(arc)
    }

    /// 计算与另一个 SharedCLV 的余弦相似度
    ///
    /// 公式:dot(a, b) / (|a| * |b|)
    ///
    /// # 零向量边界
    /// 若任一向量为零向量,返回 0.0(与 CLV::cosine_similarity 语义一致),
    /// 避免除零导致 NaN 污染下游排序。
    pub fn cosine_similarity(&self, other: &Self) -> f32 {
        nexus_core::cosine_similarity_slices(&self.0, &other.0)
    }

    /// 计算与 CLV 的余弦相似度(用于召回时 query 是 CLV)
    pub fn cosine_similarity_clv(&self, clv: &CLV) -> f32 {
        nexus_core::cosine_similarity_slices(&self.0, clv.as_slice())
    }

    /// 返回内部 f32 切片引用
    pub fn as_slice(&self) -> &[f32] {
        &self.0
    }

    /// 返回内部 Arc 引用计数(用于测试与诊断)
    #[cfg(test)]
    pub fn arc_strong_count(&self) -> usize {
        Arc::strong_count(&self.0)
    }

    /// 计算并返回内部 CLV 内容的哈希(用于池清理时查找)
    ///
    /// WHY:池清理需要根据被移除 SharedCLV 的内容哈希查找池条目,
    /// 暴露此方法避免在 l2_semantic.rs 中重复实现哈希逻辑。
    pub fn content_hash(&self) -> u64 {
        hash_f32_slice(&self.0)
    }
}

impl PartialEq for SharedCLV {
    fn eq(&self, other: &Self) -> bool {
        self.0.as_ref() == other.0.as_ref()
    }
}

impl Eq for SharedCLV {}

/// 计算 f32 切片的内容哈希(基于 to_bits,避免 unsafe)
fn hash_f32_slice(slice: &[f32]) -> u64 {
    let mut hasher = DefaultHasher::new();
    for &v in slice {
        v.to_bits().hash(&mut hasher);
    }
    hasher.finish()
}

/// 记忆层级 — 四级神经形态记忆的分层标识
///
/// 对应 MLC 创新点的四级架构:
/// - `L0Working`:工作记忆,容量极小(64),访问延迟 < 1μs,DashMap + LRU
/// - `L1Episodic`:情节记忆,按时间索引与 Quest 关联,BTreeMap + HashMap
/// - `L2Semantic`:语义记忆,按 CLV 向量召回,线性扫描 KNN(Week 6 后接入 sqlite-vec)
/// - `L3Procedural`:程序记忆,SQLite 持久化,模式签名匹配
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum MemoryTier {
    /// L0 工作记忆:当前活跃上下文,容量 64,LRU 驱逐
    L0Working,
    /// L1 情节记忆:按时间与 Quest 索引,容量 1024,FIFO 驱逐
    L1Episodic,
    /// L2 语义记忆:按 CLV 向量召回,容量 4096,Top-K KNN
    L2Semantic,
    /// L3 程序记忆:SQLite 持久化,模式签名匹配,无容量限制
    L3Procedural,
}

impl MemoryTier {
    /// 返回层级名称(用于事件 payload 与日志)
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::L0Working => "L0",
            Self::L1Episodic => "L1",
            Self::L2Semantic => "L2",
            Self::L3Procedural => "L3",
        }
    }

    /// 映射到 L0 契约的归档层级(P0-2 修复,INV-8 判定依据)
    ///
    /// WHY:INV-8 判定逻辑统一在 L0 `nexus-contracts`(独立公共 API),
    /// 本映射使 mlc 的 `MemoryTier` 可直接参与 L0 判定,避免在 L2 重复实现单调性逻辑。
    /// 映射关系:L0Working↔Hot / L1Episodic↔Warm / L2Semantic↔Cold / L3Procedural↔Ice
    /// (工作记忆=最热,程序记忆=最冷持久化,与 CMT 热温冰冷语义一一对应)。
    pub(crate) fn to_archive_tier(self) -> ArchiveTier {
        match self {
            Self::L0Working => ArchiveTier::Hot,
            Self::L1Episodic => ArchiveTier::Warm,
            Self::L2Semantic => ArchiveTier::Cold,
            Self::L3Procedural => ArchiveTier::Ice,
        }
    }
}

/// INV-8 — 归档单调性校验(委托 L0 nexus-contracts 契约,P0-2 修复)
///
/// 验证 `from → to` 不构成回升:L0Working→L1Episodic→L2Semantic→L3Procedural
/// 单向降级(同层保持合法)。供归档/降级入口与第三方调用方使用,
/// **不依赖 L9 chimera-mas**——第三方直接使用本 crate 的 demote API 时,
/// INV-8 仍可独立执行。
///
/// ## 返回
///
/// - `Ok(())`: 合法降级或同层保持
/// - `Err(MlcError::InvariantViolated)`: 回升方向(如 L2→L0),拒绝
///
/// ## 示例
///
/// ```
/// use mlc_engine::{assert_archive_monotonicity, MemoryTier};
///
/// assert!(assert_archive_monotonicity(MemoryTier::L0Working, MemoryTier::L2Semantic).is_ok());
/// assert!(assert_archive_monotonicity(MemoryTier::L2Semantic, MemoryTier::L0Working).is_err());
/// ```
pub fn assert_archive_monotonicity(from: MemoryTier, to: MemoryTier) -> Result<(), MlcError> {
    nexus_contracts::assert_archive_monotonicity(from.to_archive_tier(), to.to_archive_tier())
        .map_err(|v| MlcError::InvariantViolated(v.msg))
}

/// 记忆条目 — 四级记忆的统一载体
///
/// 跨 L0-L3 复用同一结构,通过 `tier` 字段区分所在层级。
/// 不同层级对字段的填充要求不同:
/// - L0/L1:`content` 必填,`clv` 可选,`quest_id` 可选(L0 通常无)
/// - L2:`content` 必填,`clv` 必填(用于向量召回)
/// - L3:不使用此结构,改用 `ProceduralEntry`(含模式签名与执行统计)
///
/// # P3-W11.1 D12 修复:时间有效性维度
///
/// `temporal_meta` 字段为记忆条目附加时间有效性信息(`TemporalMeta`),
/// 用于解决 D12"幽灵记忆"病理——静态稀疏掩码无法区分新旧事实的时间有效性,
/// 导致任务阶段切换时过时事实与当前事实共召回。
///
/// - `None`(默认):视为 `Current` 状态(向后兼容,老条目无此字段)
/// - `Some(TemporalMeta)`:按 `transition_type` 区分 Current/Historical/Transition
///
/// WHY(P3-W11.1):`#[serde(default)]` 确保老数据(无此字段)反序列化为 `None`,
/// 不破坏现有持久化数据与测试。`None` 在召回时按 `Current` 处理(向后兼容)。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryEntry {
    /// 记忆条目唯一标识(UUIDv7,由调用方生成)
    pub id: MemoryId,
    /// 记忆内容(自然语言文本或序列化 JSON)
    pub content: String,
    /// 上下文潜在向量(L2 语义记忆必填,L0/L1 可选)
    ///
    /// WHY:Option 而非必填 — L0 工作记忆与 L1 情节记忆按时间/Quest 索引,
    /// 不强制要求 CLV;L2 语义记忆必须携带 CLV 用于向量召回
    pub clv: Option<CLV>,
    /// 当前所在层级(用于跨层迁移时校验)
    pub tier: MemoryTier,
    /// 关联的 Quest ID(L1 情节记忆必填,L0/L2 可选)
    pub quest_id: Option<QuestId>,
    /// 创建时间(UTC,条目首次写入时设置,不变)
    pub created_at: DateTime<Utc>,
    /// 最后访问时间(UTC,L0 LRU 驱逐依据,每次 get 更新)
    pub last_accessed_at: DateTime<Utc>,
    /// 访问次数(用于热度统计与降级决策)
    pub access_count: u64,
    /// 时间元数据 — 记忆的时间有效性信息(P3-W11.1 D12 修复)
    ///
    /// WHY(P3-W11.1 D12):为每条记忆附加时间区间(valid_from/valid_until)、
    /// 时间状态(transition_type)与置信度,使 HCW-Sparse v2.0 召回能按时间状态过滤,
    /// 避免幽灵记忆(过时事实与当前事实共召回)。
    ///
    /// - `None`(默认):视为 `Current` 状态(向后兼容,老条目无此字段)
    /// - `Some(TemporalMeta)`:按 `transition_type` 区分 Current/Historical/Transition
    ///
    /// WHY `#[serde(default)]`:确保老数据(无此字段)反序列化为 `None`,
    /// 不破坏现有持久化数据与测试。`None` 在召回时按 `Current` 处理(向后兼容)。
    #[serde(default)]
    pub temporal_meta: Option<TemporalMeta>,
}

impl MemoryEntry {
    /// 创建新记忆条目,`created_at` 与 `last_accessed_at` 自动设为当前 UTC
    ///
    /// # 参数
    /// - `id`:条目唯一标识(接受 `MemoryId`/`String`/`&str`,通过 `Into<MemoryId>` 转换)
    /// - `content`:记忆内容
    /// - `tier`:初始层级
    ///
    /// # P3-W11.1 D12 修复
    ///
    /// `temporal_meta` 默认为 `None`(向后兼容)。需要时间有效性时通过
    /// `with_temporal_meta` 链式设置。`None` 在召回时按 `Current` 处理。
    pub fn new(id: impl Into<MemoryId>, content: impl Into<String>, tier: MemoryTier) -> Self {
        let now = Utc::now();
        Self {
            id: id.into(),
            content: content.into(),
            clv: None,
            tier,
            quest_id: None,
            created_at: now,
            last_accessed_at: now,
            access_count: 0,
            temporal_meta: None, // P3-W11.1: 默认 None(向后兼容,视为 Current)
        }
    }

    /// 附带 CLV 向量(用于 L2 语义记忆)
    pub fn with_clv(mut self, clv: CLV) -> Self {
        self.clv = Some(clv);
        self
    }

    /// 附带 Quest 关联(用于 L1 情节记忆)
    pub fn with_quest(mut self, quest_id: impl Into<QuestId>) -> Self {
        self.quest_id = Some(quest_id.into());
        self
    }

    /// 附带时间元数据(用于 P3-W11.1 D12 修复 — 幽灵记忆免疫)
    ///
    /// WHY(P3-W11.1 D12):为记忆条目附加时间有效性(valid_from/valid_until)、
    /// 时间状态(transition_type)与置信度,使召回能按时间状态过滤。
    /// - `Current`:当前有效,默认召回
    /// - `Historical`:历史归档,需显式历史查询
    /// - `Transition`:迁移中,附时间证据包且降置信度
    ///
    /// # 示例
    /// ```
    /// use mlc_engine::{MemoryEntry, MemoryTier};
    /// use nexus_contracts::{TemporalMeta, TransitionType};
    ///
    /// let now_ts = 1700000000_i64; // UTC 秒
    /// let meta = TemporalMeta::with_expiry(now_ts, now_ts + 3600, 0.9);
    /// let entry = MemoryEntry::new("m-1", "内容", MemoryTier::L2Semantic)
    ///     .with_temporal_meta(meta);
    /// assert!(entry.is_current());
    /// ```
    pub fn with_temporal_meta(mut self, meta: TemporalMeta) -> Self {
        self.temporal_meta = Some(meta);
        self
    }

    /// 判断记忆是否为 `Current` 状态(默认召回)
    ///
    /// WHY(P3-W11.1 D12):`temporal_meta` 为 `None` 时视为 `Current`(向后兼容),
    /// 避免老条目(无此字段)被误过滤为 Historical/Transition。
    pub fn is_current(&self) -> bool {
        match &self.temporal_meta {
            None => true, // 向后兼容:无 temporal_meta 视为 Current
            Some(meta) => meta.transition_type.is_current(),
        }
    }

    /// 判断记忆是否为 `Historical` 状态(需显式历史查询)
    ///
    /// WHY(P3-W11.1 D12):Historical 状态的记忆已过期,默认召回跳过,
    /// 仅 `recall_historical` 显式查询时返回。
    pub fn is_historical(&self) -> bool {
        match &self.temporal_meta {
            None => false,
            Some(meta) => matches!(meta.transition_type, TransitionType::Historical),
        }
    }

    /// 判断记忆是否为 `Transition` 状态(迁移中,附时间证据包且降置信度)
    ///
    /// WHY(P3-W11.1 D12):Transition 状态的记忆正在归档迁移中,
    /// 召回时需附时间证据包(valid_from/valid_until)并降低置信度。
    pub fn is_transition(&self) -> bool {
        match &self.temporal_meta {
            None => false,
            Some(meta) => matches!(meta.transition_type, TransitionType::Transition),
        }
    }

    /// 判断在指定时间点(UTC 秒)是否有效
    ///
    /// WHY(P3-W11.1 D12):检查 `valid_from ≤ now < valid_until`(若 `valid_until` 为 None 则永久有效)。
    /// `temporal_meta` 为 `None` 时视为永久有效(向后兼容)。
    ///
    /// # 参数
    /// - `now`:当前时间(UTC 秒)
    pub fn is_valid_at(&self, now: i64) -> bool {
        match &self.temporal_meta {
            None => true, // 向后兼容:无 temporal_meta 视为永久有效
            Some(meta) => meta.is_valid_at(now),
        }
    }

    /// 返回时间元数据,若为 `None` 则返回默认 `Current` 状态的 `TemporalMeta`
    ///
    /// WHY(P3-W11.1 D12):召回路径(如 `recall_transition`)需要访问 `TemporalMeta`
    /// 的 valid_from/valid_until/confidence 字段。`None` 时构造默认 Current 状态,
    /// 避免调用方重复处理 Option 分支。
    ///
    /// # 参数
    /// - `default_valid_from`:`None` 时使用的默认生效时间(UTC 秒,通常为 `created_at` 的 UTC 秒)
    pub fn temporal_meta_or_default(&self, default_valid_from: i64) -> TemporalMeta {
        match &self.temporal_meta {
            None => TemporalMeta::new(default_valid_from, 1.0), // 默认完全可信
            Some(meta) => meta.clone(),
        }
    }

    /// 返回置信度(P3-W11.1 D12)
    ///
    /// WHY:`Transition` 状态的记忆置信度应降低(由调用方在归档迁移时设置)。
    /// `temporal_meta` 为 `None` 时返回 1.0(向后兼容,完全可信)。
    pub fn confidence(&self) -> f32 {
        match &self.temporal_meta {
            None => 1.0,
            Some(meta) => meta.confidence,
        }
    }

    /// 标记被访问:更新 `last_accessed_at` 与 `access_count`
    ///
    /// WHY(P3-W11.1):不修改 `temporal_meta` — 访问时间与时间有效性分离,
    /// `last_accessed_at` 用于 LRU 驱逐,`temporal_meta.transition_type` 用于时间状态过滤。
    pub fn touch(&mut self) {
        self.last_accessed_at = Utc::now();
        self.access_count = self.access_count.saturating_add(1);
    }

    /// 标记为 `Historical` 状态(归档迁移完成)
    ///
    /// WHY(P3-W11.1 D12 + INV-8 单调性):`Current` → `Historical` 单向降级。
    /// 若 `temporal_meta` 为 `None`,先构造默认 Current 再降级。
    /// 若已是 `Historical`,操作为 no-op(幂等)。
    ///
    /// # INV-8 约束
    /// `Historical` → `Current` 禁止(逆向升级违反 INV-8)。
    /// 本方法仅处理 `→ Historical` 方向,不提供 `Historical` → `Current` 升级。
    pub fn mark_historical(&mut self) {
        let now_ts = Utc::now().timestamp();
        let meta = self
            .temporal_meta
            .take()
            .unwrap_or_else(|| TemporalMeta::new(now_ts, 1.0));
        if meta.transition_type != TransitionType::Historical {
            self.temporal_meta = Some(TemporalMeta {
                transition_type: TransitionType::Historical,
                ..meta
            });
        } else {
            self.temporal_meta = Some(meta); // 已是 Historical,no-op
        }
    }

    /// 标记为 `Transition` 状态(归档迁移开始,降置信度)
    ///
    /// WHY(P3-W11.1 D12 + INV-8 单调性):`Current` → `Transition` → `Historical` 迁移链。
    /// `Transition` 状态附时间证据包(valid_from/valid_until)并降低置信度,
    /// 使召回路径能识别"迁移中"记忆并降权处理。
    ///
    /// # 参数
    /// - `new_confidence`:迁移中的降权置信度(通常 < 原 confidence,如 0.5)
    ///
    /// # INV-8 约束
    /// `Historical` → `Transition` 禁止(从终态回退迁移态违反单调性)。
    /// 若当前已是 `Historical`,操作为 no-op(幂等,拒绝逆向)。
    pub fn mark_transition(&mut self, new_confidence: f32) {
        let now_ts = Utc::now().timestamp();
        let meta = self
            .temporal_meta
            .take()
            .unwrap_or_else(|| TemporalMeta::new(now_ts, 1.0));
        if matches!(meta.transition_type, TransitionType::Historical) {
            // INV-8: Historical → Transition 禁止,no-op
            self.temporal_meta = Some(meta);
            return;
        }
        self.temporal_meta = Some(TemporalMeta {
            transition_type: TransitionType::Transition,
            confidence: new_confidence,
            ..meta
        });
    }
}

/// 模式签名 — L3 程序记忆的匹配键
///
/// 由工具调用序列与上下文哈希组成,作为 SQLite 主键的字符串化表示。
/// Week 3 阶段使用精确匹配(序列化字符串相等),Week 6 后可扩展为
/// 编辑距离匹配(允许工具序列部分重合)。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct PatternSignature {
    /// 工具调用序列(如 `["read_file", "edit_file", "run_test"]`)
    pub tool_sequence: Vec<String>,
    /// 上下文哈希(SHA-256 hex,标识触发该模式的上下文特征)
    pub context_hash: String,
}

impl PatternSignature {
    /// 创建新模式签名
    pub fn new(tool_sequence: Vec<String>, context_hash: impl Into<String>) -> Self {
        Self {
            tool_sequence,
            context_hash: context_hash.into(),
        }
    }

    /// 序列化为稳定字符串(作为 SQLite 主键)
    ///
    /// WHY:使用 JSON 序列化而非 Debug 格式,确保字段顺序稳定
    /// (serde_json 默认按结构体字段顺序输出),避免相同签名产生不同字符串
    ///
    /// 返回 `Result<String, MlcError>`:序列化失败时返回 `SerializationFailed`,
    /// 而非静默返回空字符串(空字符串会导致主键冲突与数据覆盖)。
    pub fn to_key(&self) -> Result<String, MlcError> {
        serde_json::to_string(self)
            .map_err(|e| MlcError::SerializationFailed(format!("PatternSignature 序列化失败: {e}")))
    }
}

/// 执行统计 — L3 程序记忆的执行历史指标
///
/// 用于评估模式可靠性,辅助决策是否复用该程序记忆。
/// `success_rate = success_count / total_count`,`total_count > 0` 时有效。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExecutionStats {
    /// 成功执行次数
    pub success_count: u64,
    /// 失败执行次数
    pub failure_count: u64,
    /// 累计执行耗时(毫秒),用于计算平均延迟
    pub total_latency_ms: u64,
    /// 最后执行时间(UTC)
    pub last_executed_at: Option<DateTime<Utc>>,
}

impl ExecutionStats {
    /// 创建空的执行统计
    pub fn new() -> Self {
        Self {
            success_count: 0,
            failure_count: 0,
            total_latency_ms: 0,
            last_executed_at: None,
        }
    }

    /// 总执行次数
    pub fn total_count(&self) -> u64 {
        self.success_count + self.failure_count
    }

    /// 成功率 [0.0, 1.0],总次数为 0 时返回 0.0
    pub fn success_rate(&self) -> f32 {
        let total = self.total_count();
        if total == 0 {
            return 0.0;
        }
        self.success_count as f32 / total as f32
    }

    /// 平均延迟(毫秒),总次数为 0 时返回 0.0
    pub fn avg_latency_ms(&self) -> f64 {
        let total = self.total_count();
        if total == 0 {
            return 0.0;
        }
        self.total_latency_ms as f64 / total as f64
    }

    /// 记录一次执行结果
    pub fn record(&mut self, success: bool, latency_ms: u64) {
        if success {
            self.success_count = self.success_count.saturating_add(1);
        } else {
            self.failure_count = self.failure_count.saturating_add(1);
        }
        self.total_latency_ms = self.total_latency_ms.saturating_add(latency_ms);
        self.last_executed_at = Some(Utc::now());
    }
}

impl Default for ExecutionStats {
    fn default() -> Self {
        Self::new()
    }
}

/// L3 程序记忆条目 — 持久化的可复用执行模式
///
/// 与 `MemoryEntry` 分离的原因:
/// - 程序记忆需要 `PatternSignature` 作为匹配键(而非 ID 查找)
/// - 程序记忆需要 `ExecutionStats` 跟踪可靠性(而非访问时间)
/// - 程序记忆持久化到 SQLite(而非内存)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProceduralEntry {
    /// 模式签名(唯一键,序列化为字符串作为 SQLite 主键)
    pub pattern_signature: PatternSignature,
    /// 执行统计(成功/失败次数、累计延迟)
    pub execution_stats: ExecutionStats,
    /// 模式产出内容(成功执行时的产出,用于直接复用)
    pub output: String,
    /// 创建时间(UTC)
    pub created_at: DateTime<Utc>,
    /// 最后更新时间(UTC,执行统计变更时更新)
    pub updated_at: DateTime<Utc>,
}

impl ProceduralEntry {
    /// 创建新程序记忆条目,时间戳自动设为当前 UTC
    pub fn new(pattern_signature: PatternSignature, output: impl Into<String>) -> Self {
        let now = Utc::now();
        Self {
            pattern_signature,
            execution_stats: ExecutionStats::new(),
            output: output.into(),
            created_at: now,
            updated_at: now,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_memory_tier_as_str() {
        assert_eq!(MemoryTier::L0Working.as_str(), "L0");
        assert_eq!(MemoryTier::L1Episodic.as_str(), "L1");
        assert_eq!(MemoryTier::L2Semantic.as_str(), "L2");
        assert_eq!(MemoryTier::L3Procedural.as_str(), "L3");
    }

    // ============================================================
    // P0-2 修复: INV-8 归档单调性适配器测试(接线验证)
    // ============================================================
    //
    // 验证 mlc 层适配器委托 L0 nexus-contracts 契约的正确性:
    // 合法降级/同层全 Ok,回升全 Err(engine 级接线见 tests/engine.rs)。

    /// 合法方向:6 对降级 + 4 对同层保持,全部 Ok
    #[test]
    fn test_assert_archive_monotonicity_legal_pairs() {
        let legal_pairs = [
            // 降级(严格 level 递增: L0Working→L3Procedural)
            (MemoryTier::L0Working, MemoryTier::L1Episodic),
            (MemoryTier::L0Working, MemoryTier::L2Semantic),
            (MemoryTier::L0Working, MemoryTier::L3Procedural),
            (MemoryTier::L1Episodic, MemoryTier::L2Semantic),
            (MemoryTier::L1Episodic, MemoryTier::L3Procedural),
            (MemoryTier::L2Semantic, MemoryTier::L3Procedural),
            // 同层保持(归档到自身层级为无操作)
            (MemoryTier::L0Working, MemoryTier::L0Working),
            (MemoryTier::L1Episodic, MemoryTier::L1Episodic),
            (MemoryTier::L2Semantic, MemoryTier::L2Semantic),
            (MemoryTier::L3Procedural, MemoryTier::L3Procedural),
        ];
        for (from, to) in legal_pairs {
            let result = assert_archive_monotonicity(from, to);
            assert!(
                result.is_ok(),
                "{from:?} -> {to:?} 为降级或同层,应 Ok,实际: {result:?}"
            );
        }
    }

    /// 回升方向:全部 Err(MlcError::InvariantViolated),且消息含两级名称
    #[test]
    fn test_assert_archive_monotonicity_reverse_rejected() {
        let reverse_pairs = [
            (MemoryTier::L1Episodic, MemoryTier::L0Working),
            (MemoryTier::L2Semantic, MemoryTier::L0Working),
            (MemoryTier::L3Procedural, MemoryTier::L0Working),
            (MemoryTier::L2Semantic, MemoryTier::L1Episodic),
            (MemoryTier::L3Procedural, MemoryTier::L1Episodic),
            (MemoryTier::L3Procedural, MemoryTier::L2Semantic),
        ];
        for (from, to) in reverse_pairs {
            let result = assert_archive_monotonicity(from, to);
            match result {
                Err(MlcError::InvariantViolated(msg)) => {
                    // 消息来自 L0 契约(含 INV-8 标识与方向分隔符,如
                    // "归档层级回升被禁止(INV-8): Cold -> Hot")
                    assert!(msg.contains("INV-8"), "消息应含 INV-8 标识,实际: {msg}");
                    assert!(msg.contains("->"), "消息应包含方向分隔符,实际: {msg}");
                }
                other => panic!("{from:?} -> {to:?} 为回升方向,应 Err,实际: {other:?}"),
            }
        }
    }

    #[test]
    fn test_memory_entry_new_defaults() {
        let entry = MemoryEntry::new("m-1", "内容", MemoryTier::L0Working);
        assert_eq!(entry.id.as_str(), "m-1");
        assert_eq!(entry.content, "内容");
        assert!(entry.clv.is_none());
        assert!(entry.quest_id.is_none());
        assert_eq!(entry.tier, MemoryTier::L0Working);
        assert_eq!(entry.access_count, 0);
    }

    #[test]
    fn test_memory_entry_builder_chain() {
        let clv = CLV::zero();
        let entry = MemoryEntry::new("m-1", "内容", MemoryTier::L2Semantic)
            .with_clv(clv)
            .with_quest("quest-1");
        assert!(entry.clv.is_some());
        assert_eq!(entry.quest_id.as_deref(), Some("quest-1"));
    }

    #[test]
    fn test_memory_entry_touch_increments_access_count() {
        let mut entry = MemoryEntry::new("m-1", "内容", MemoryTier::L0Working);
        assert_eq!(entry.access_count, 0);
        entry.touch();
        entry.touch();
        assert_eq!(entry.access_count, 2);
    }

    // ============================================================
    // P3-W11.1 D12 修复验收测试(spec.md:291 TemporalMeta 全链)
    // ============================================================

    #[test]
    fn test_p3_w11_1_temporal_meta_default_none_backward_compat() {
        // P3-W11.1: 新建条目默认 temporal_meta = None(向后兼容)
        let entry = MemoryEntry::new("m-1", "内容", MemoryTier::L0Working);
        assert!(entry.temporal_meta.is_none());
        // None 视为 Current(向后兼容,老条目无此字段)
        assert!(entry.is_current());
        assert!(!entry.is_historical());
        assert!(!entry.is_transition());
        // None 视为永久有效
        assert!(entry.is_valid_at(0));
        assert!(entry.is_valid_at(i64::MAX));
        // None 的置信度为 1.0(完全可信)
        assert!((entry.confidence() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_p3_w11_1_with_temporal_meta_current() {
        // P3-W11.1: with_temporal_meta 设置 Current 状态
        let meta = TemporalMeta::with_expiry(1000, 2000, 0.9);
        let entry =
            MemoryEntry::new("m-1", "内容", MemoryTier::L2Semantic).with_temporal_meta(meta);
        assert!(entry.temporal_meta.is_some());
        assert!(entry.is_current());
        assert!(!entry.is_historical());
        assert!(!entry.is_transition());
        assert!(entry.is_valid_at(1000));
        assert!(entry.is_valid_at(1500));
        assert!(!entry.is_valid_at(2000)); // exclusive 边界
        assert!(!entry.is_valid_at(999)); // 生效前
        assert!((entry.confidence() - 0.9).abs() < 1e-6);
    }

    #[test]
    fn test_p3_w11_1_with_temporal_meta_historical() {
        // P3-W11.1: Historical 状态(历史归档,需显式历史查询)
        let meta = TemporalMeta {
            valid_from: 1000,
            valid_until: Some(2000),
            transition_type: TransitionType::Historical,
            confidence: 0.5,
        };
        let entry =
            MemoryEntry::new("m-1", "内容", MemoryTier::L1Episodic).with_temporal_meta(meta);
        assert!(!entry.is_current());
        assert!(entry.is_historical());
        assert!(!entry.is_transition());
        assert!((entry.confidence() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn test_p3_w11_1_with_temporal_meta_transition() {
        // P3-W11.1: Transition 状态(迁移中,附时间证据包且降置信度)
        let meta = TemporalMeta {
            valid_from: 1000,
            valid_until: Some(2000),
            transition_type: TransitionType::Transition,
            confidence: 0.3, // 降置信度
        };
        let entry = MemoryEntry::new("m-1", "内容", MemoryTier::L0Working).with_temporal_meta(meta);
        assert!(!entry.is_current());
        assert!(!entry.is_historical());
        assert!(entry.is_transition());
        assert!((entry.confidence() - 0.3).abs() < 1e-6);
    }

    #[test]
    fn test_p3_w11_1_mark_historical_from_current() {
        // P3-W11.1 + INV-8: Current → Historical 单向降级
        let mut entry = MemoryEntry::new("m-1", "内容", MemoryTier::L0Working)
            .with_temporal_meta(TemporalMeta::new(1000, 0.9));
        assert!(entry.is_current());

        entry.mark_historical();
        assert!(!entry.is_current());
        assert!(entry.is_historical());
        // 置信度保留(降级不改置信度,仅改状态)
        assert!((entry.confidence() - 0.9).abs() < 1e-6);
    }

    #[test]
    fn test_p3_w11_1_mark_historical_from_none() {
        // P3-W11.1: temporal_meta = None 时 mark_historical 先构造默认 Current 再降级
        let mut entry = MemoryEntry::new("m-1", "内容", MemoryTier::L0Working);
        assert!(entry.temporal_meta.is_none());

        entry.mark_historical();
        assert!(entry.temporal_meta.is_some());
        assert!(entry.is_historical());
        assert!(!entry.is_current());
    }

    #[test]
    fn test_p3_w11_1_mark_historical_idempotent() {
        // P3-W11.1: 已是 Historical 时 mark_historical 为 no-op(幂等)
        let mut entry = MemoryEntry::new("m-1", "内容", MemoryTier::L0Working).with_temporal_meta(
            TemporalMeta {
                valid_from: 1000,
                valid_until: Some(2000),
                transition_type: TransitionType::Historical,
                confidence: 0.5,
            },
        );
        entry.mark_historical(); // no-op
        assert!(entry.is_historical());
        assert!((entry.confidence() - 0.5).abs() < 1e-6); // 置信度不变
    }

    #[test]
    fn test_p3_w11_1_mark_transition_from_current() {
        // P3-W11.1 + INV-8: Current → Transition(降置信度)
        let mut entry = MemoryEntry::new("m-1", "内容", MemoryTier::L0Working)
            .with_temporal_meta(TemporalMeta::new(1000, 1.0));
        assert!(entry.is_current());

        entry.mark_transition(0.5); // 降置信度到 0.5
        assert!(entry.is_transition());
        assert!(!entry.is_current());
        assert!(!entry.is_historical());
        assert!((entry.confidence() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn test_p3_w11_1_inv8_historical_to_transition_forbidden() {
        // P3-W11.1 + INV-8: Historical → Transition 禁止(逆向升级违反单调性)
        let mut entry = MemoryEntry::new("m-1", "内容", MemoryTier::L0Working).with_temporal_meta(
            TemporalMeta {
                valid_from: 1000,
                valid_until: Some(2000),
                transition_type: TransitionType::Historical,
                confidence: 0.5,
            },
        );
        // 尝试从 Historical 回退到 Transition — 应被拒绝(no-op)
        entry.mark_transition(0.3);
        assert!(entry.is_historical()); // 状态不变
        assert!(!entry.is_transition());
        // 置信度不变(no-op)
        assert!((entry.confidence() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn test_p3_w11_1_inv8_full_migration_chain() {
        // P3-W11.1 + INV-8: 完整迁移链 Current → Transition → Historical
        let mut entry = MemoryEntry::new("m-1", "内容", MemoryTier::L0Working)
            .with_temporal_meta(TemporalMeta::new(1000, 1.0));
        assert!(entry.is_current());

        // Step 1: Current → Transition(降置信度)
        entry.mark_transition(0.4);
        assert!(entry.is_transition());
        assert!((entry.confidence() - 0.4).abs() < 1e-6);

        // Step 2: Transition → Historical(归档完成)
        entry.mark_historical();
        assert!(entry.is_historical());
        assert!(!entry.is_transition());
        // 置信度保留(Transition 的 0.4 延续到 Historical)
        assert!((entry.confidence() - 0.4).abs() < 1e-6);

        // Step 3: Historical → Transition 禁止(INV-8 逆向)
        entry.mark_transition(0.1);
        assert!(entry.is_historical()); // 状态不变
        assert!((entry.confidence() - 0.4).abs() < 1e-6); // 置信度不变
    }

    #[test]
    fn test_p3_w11_1_temporal_meta_or_default() {
        // P3-W11.1: temporal_meta_or_default 返回 TemporalMeta 或默认 Current
        let entry_with = MemoryEntry::new("m-1", "内容", MemoryTier::L0Working)
            .with_temporal_meta(TemporalMeta::with_expiry(1000, 2000, 0.7));
        let meta = entry_with.temporal_meta_or_default(0);
        assert_eq!(meta.valid_from, 1000);
        assert_eq!(meta.valid_until, Some(2000));
        assert!((meta.confidence - 0.7).abs() < 1e-6);

        // None 时返回默认 Current
        let entry_without = MemoryEntry::new("m-2", "内容", MemoryTier::L0Working);
        let meta = entry_without.temporal_meta_or_default(1500);
        assert_eq!(meta.valid_from, 1500);
        assert!(meta.is_permanent());
        assert_eq!(meta.transition_type, TransitionType::Current);
        assert!((meta.confidence - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_p3_w11_1_touch_preserves_temporal_meta() {
        // P3-W11.1: touch 不修改 temporal_meta(访问时间与时间有效性分离)
        let mut entry = MemoryEntry::new("m-1", "内容", MemoryTier::L0Working)
            .with_temporal_meta(TemporalMeta::with_expiry(1000, 2000, 0.8));
        let meta_before = entry.temporal_meta.clone();
        entry.touch();
        // temporal_meta 不变
        assert_eq!(entry.temporal_meta, meta_before);
        // access_count 递增
        assert_eq!(entry.access_count, 1);
    }

    #[test]
    fn test_p3_w11_1_serde_backward_compat_old_data() {
        // P3-W11.1: 老数据(无 temporal_meta 字段)反序列化为 None(向后兼容)
        // 模拟老格式 JSON:无 temporal_meta 字段
        let old_json = r#"{
            "id": "m-1",
            "content": "内容",
            "clv": null,
            "tier": "L0Working",
            "quest_id": null,
            "created_at": "2026-07-24T00:00:00Z",
            "last_accessed_at": "2026-07-24T00:00:00Z",
            "access_count": 0
        }"#;
        let entry: MemoryEntry = serde_json::from_str(old_json).expect("反序列化老格式失败");
        assert!(entry.temporal_meta.is_none());
        // None 视为 Current(向后兼容)
        assert!(entry.is_current());
    }

    #[test]
    fn test_p3_w11_1_serde_roundtrip_with_temporal_meta() {
        // P3-W11.1: 新数据(含 temporal_meta)序列化/反序列化往返一致
        let entry = MemoryEntry::new("m-1", "内容", MemoryTier::L2Semantic)
            .with_temporal_meta(TemporalMeta::with_expiry(1000, 2000, 0.85));
        let json = serde_json::to_string(&entry).expect("序列化失败");
        let restored: MemoryEntry = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(entry, restored);
        assert!(restored.temporal_meta.is_some());
        assert!(restored.is_current());
    }

    #[test]
    fn test_p3_w11_1_three_transition_types_coverage() {
        // P3-W11.1 验收:召回测试覆盖三种 TransitionType
        // 此测试验证三种状态的 is_current/is_historical/is_transition 互斥性
        let current = MemoryEntry::new("m-1", "内容", MemoryTier::L0Working)
            .with_temporal_meta(TemporalMeta::new(1000, 1.0));
        let historical = MemoryEntry::new("m-2", "内容", MemoryTier::L0Working).with_temporal_meta(
            TemporalMeta {
                valid_from: 1000,
                valid_until: Some(2000),
                transition_type: TransitionType::Historical,
                confidence: 0.5,
            },
        );
        let transition = MemoryEntry::new("m-3", "内容", MemoryTier::L0Working).with_temporal_meta(
            TemporalMeta {
                valid_from: 1000,
                valid_until: Some(2000),
                transition_type: TransitionType::Transition,
                confidence: 0.3,
            },
        );

        // Current
        assert!(current.is_current());
        assert!(!current.is_historical());
        assert!(!current.is_transition());

        // Historical
        assert!(!historical.is_current());
        assert!(historical.is_historical());
        assert!(!historical.is_transition());

        // Transition
        assert!(!transition.is_current());
        assert!(!transition.is_historical());
        assert!(transition.is_transition());

        // 互斥性:同一时刻只有一个状态为 true
        for entry in [current, historical, transition] {
            let states = [
                entry.is_current(),
                entry.is_historical(),
                entry.is_transition(),
            ];
            let true_count = states.iter().filter(|&&b| b).count();
            assert_eq!(true_count, 1, "三种状态应互斥,只有一个为 true");
        }
    }

    #[test]
    fn test_pattern_signature_to_key_stable() {
        let sig1 = PatternSignature::new(vec!["a".into(), "b".into()], "hash-1");
        let sig2 = PatternSignature::new(vec!["a".into(), "b".into()], "hash-1");
        assert_eq!(sig1.to_key().unwrap(), sig2.to_key().unwrap());
    }

    #[test]
    fn test_pattern_signature_to_key_differs() {
        let sig1 = PatternSignature::new(vec!["a".into()], "hash-1");
        let sig2 = PatternSignature::new(vec!["b".into()], "hash-1");
        assert_ne!(sig1.to_key().unwrap(), sig2.to_key().unwrap());
    }

    #[test]
    fn test_execution_stats_record_success() {
        let mut stats = ExecutionStats::new();
        stats.record(true, 100);
        stats.record(true, 200);
        assert_eq!(stats.success_count, 2);
        assert_eq!(stats.failure_count, 0);
        assert_eq!(stats.total_count(), 2);
        assert!((stats.success_rate() - 1.0).abs() < 1e-6);
        assert!((stats.avg_latency_ms() - 150.0).abs() < 1e-6);
    }

    #[test]
    fn test_execution_stats_record_mixed() {
        let mut stats = ExecutionStats::new();
        stats.record(true, 100);
        stats.record(false, 50);
        assert_eq!(stats.success_count, 1);
        assert_eq!(stats.failure_count, 1);
        assert!((stats.success_rate() - 0.5).abs() < 1e-6);
    }

    #[test]
    fn test_execution_stats_empty_rate() {
        let stats = ExecutionStats::new();
        assert_eq!(stats.success_rate(), 0.0);
        assert_eq!(stats.avg_latency_ms(), 0.0);
    }

    #[test]
    fn test_procedural_entry_new() {
        let sig = PatternSignature::new(vec!["tool".into()], "hash");
        let entry = ProceduralEntry::new(sig.clone(), "output");
        assert_eq!(entry.pattern_signature, sig);
        assert_eq!(entry.output, "output");
        assert_eq!(entry.execution_stats.total_count(), 0);
    }
}
