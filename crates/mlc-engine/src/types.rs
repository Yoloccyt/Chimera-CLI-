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

impl AsRef<[f32]> for SharedCLV {
    fn as_ref(&self) -> &[f32] {
        &self.0
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
}

/// 记忆条目 — 四级记忆的统一载体
///
/// 跨 L0-L3 复用同一结构,通过 `tier` 字段区分所在层级。
/// 不同层级对字段的填充要求不同:
/// - L0/L1:`content` 必填,`clv` 可选,`quest_id` 可选(L0 通常无)
/// - L2:`content` 必填,`clv` 必填(用于向量召回)
/// - L3:不使用此结构,改用 `ProceduralEntry`(含模式签名与执行统计)
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
}

impl MemoryEntry {
    /// 创建新记忆条目,`created_at` 与 `last_accessed_at` 自动设为当前 UTC
    ///
    /// # 参数
    /// - `id`:条目唯一标识(接受 `MemoryId`/`String`/`&str`,通过 `Into<MemoryId>` 转换)
    /// - `content`:记忆内容
    /// - `tier`:初始层级
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

    /// 标记被访问:更新 `last_accessed_at` 与 `access_count`
    pub fn touch(&mut self) {
        self.last_accessed_at = Utc::now();
        self.access_count = self.access_count.saturating_add(1);
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
