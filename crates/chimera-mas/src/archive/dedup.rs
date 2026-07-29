//! 跨 Agent 去重引擎 — P3-W11.2.3 D12 修复
//!
//! 架构层归属: L9 Quest(chimera-mas/archive 子模块)
//! 核心职责: 识别多个 Agent 产生的重复记忆条目,去重合并,目标去重率 >80%
//!
//! ## 设计决策(WHY)
//!
//! - **独立 DedupEntry struct**(ADR-033 + §2.2 铁律):
//!   L9 chimera-mas 不能依赖 L5 repo-wiki(向上依赖禁止)。DedupEngine 定义自己的
//!   输入类型 `DedupEntry`,调用方(repo-wiki/上层编排器)自行映射 WikiEntry → DedupEntry。
//!   类型定义零跨层依赖,API 简洁,去重逻辑内聚。
//!
//! - **纯计算单元**(职责单一):
//!   DedupEngine 仅返回 `DedupResult` 关系列表,不修改条目状态、不持久化。
//!   调用方决定如何标记 Historical(通过 event-bus 或 archive API)。
//!   这样 DedupEngine 易于单元测试(无 I/O 依赖),且不耦合存储层。
//!
//! - **必须跨 Agent 判定**(P3-W11.2.3 核心语义):
//!   仅当 `kept.agent_id != removed.agent_id` 时才判定为跨 Agent 去重。
//!   单 Agent 内的重复留给 Agent 自身处理(避免越权)。
//!   `dedup_rate = 跨 Agent 去重数 / 输入条目总数`。
//!
//! - **三级去重阈值**(与 repo-wiki 矛盾检测阈值区分):
//!   - 精确去重(content_hash 相同)→ `ExactDuplicate`(O(1) 比较,快速路径)
//!   - 语义去重(cosine_similarity >= 0.95)→ `SemanticDuplicate`
//!   - 近似去重(cosine_similarity >= 0.85)→ `NearDuplicate`
//!   WHY 0.95 / 0.85 而非矛盾检测的 0.9:
//!   矛盾检测关注"冲突"(相似但可能矛盾,0.9 已足够敏感);
//!   去重关注"重复"(相似且表达同一事实,需更高阈值避免误删独立条目)。
//!
//! - **保留策略**(稳定性优先):
//!   同组/同对中保留 `confidence` 最高的;若 `confidence` 相同(差异 < `CONFIDENCE_EQUAL_TOLERANCE`),
//!   保留 `created_at` 最早的(稳定可预测,避免随机性)。
//!
//! ## 与 INV-8 归档单调性的关系
//!
//! 去重不删除条目。调用方收到 `DedupResult` 后,应将 `removed` 列表中的条目标记为
//! `Historical`(通过 `nexus_contracts::TemporalMeta`),保留谱系完整性
//! (与 P3-W11.2.2 矛盾检测标记 Historical 一致,INV-8 单向降级)。
//!
//! ## 红线对齐
//!
//! - §2.2: L9 自包含,不依赖 L5 repo-wiki(独立 DedupEntry 类型)
//! - §4.1: 库层 thiserror,无 unwrap/expect(边界用 `?` / `match` / 默认值)
//! - §4.4 反模式 6: f32 禁止隐式转 f64,全程 f32(embedding 一致)
//! - §6.1: 单函数 ≤ 200 行(算法拆分为 `dedup_exact` + `dedup_semantic` 两个子方法)
//! - §6.2: Top-K 用 `select_nth_unstable`(本模块无 Top-K 需求,排序用 `sort_by` 合理)
//! - `#![forbid(unsafe_code)]`: crate 级已在 lib.rs 声明,本模块无需重复

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
// 统一使用 nexus-core 权威实现,避免多副本优化不一致
use nexus_core::cosine_similarity_slices;

// ============================================================
// 常量 — 三级去重阈值
// ============================================================

/// 语义去重阈值 — cosine_similarity >= 此值判定为 `SemanticDuplicate`
///
/// WHY 0.95:对应"几乎完全相同"的语义。与 repo-wiki 矛盾检测阈值 0.9 区分:
/// 矛盾检测关注"冲突"(相似但可能矛盾),去重关注"重复"(相似且表达同一事实)。
/// 0.95 避免误将独立但相关的条目去重。
pub const SEMANTIC_DEDUP_THRESHOLD: f32 = 0.95;

/// 近似去重阈值 — cosine_similarity >= 此值判定为 `NearDuplicate`
///
/// WHY 0.85:对应"高度相似",可能存在部分重复(如不同角度描述同一实体)。
/// 低于此值视为独立条目,不去重。
pub const NEAR_DEDUP_THRESHOLD: f32 = 0.85;

/// confidence 相等判定容差 — 差异 < 此值视为相等
///
/// WHY: f32 浮点精度,避免微小差异导致保留策略不稳定(如 0.9 与 0.9000001)。
/// 与 §4.4 反模式 6 一致:全程 f32,比较时用容差而非精确相等。
pub const CONFIDENCE_EQUAL_TOLERANCE: f32 = 1e-6;

// ============================================================
// 输入/输出类型
// ============================================================

/// 去重条目 — DedupEngine 的输入单元
///
/// 调用方(repo-wiki/上层编排器)负责将内部条目类型映射为 `DedupEntry`:
/// - `entry_id`:原始条目唯一标识(如 WikiEntry::entry_id)
/// - `agent_id`:产出该条目的 Agent ID(用于跨 Agent 判定)
/// - `embedding`:语义向量(512-dim CLV,与 WikiEntry::embedding 同源)
/// - `content_hash`:内容哈希(精确去重快速路径,调用方用 SHA-256 / xxHash 计算)
/// - `confidence`:置信度(去重时保留高置信度,来源 TemporalMeta::confidence 或 GSOE 反馈)
/// - `created_at`:创建时间(置信度相同时保留最早创建的,稳定性)
///
/// # 跨层映射示例(repo-wiki → DedupEntry,L5 → L9 边界)
///
/// ```no_run
/// use chimera_mas::archive::DedupEntry;
///
/// fn map_wiki_to_dedup(
///     entry_id: String,
///     agent_id: String,
///     embedding: Vec<f32>,
///     content: &str,
///     confidence: f32,
///     created_at: chrono::DateTime<chrono::Utc>,
/// ) -> DedupEntry {
///     // content_hash 由调用方计算(DedupEngine 不引入哈希依赖)
///     let content_hash = compute_content_hash(content);
///     DedupEntry::new(entry_id, agent_id, embedding, content_hash, confidence, created_at)
/// }
/// # fn compute_content_hash(_: &str) -> u64 { 0 }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DedupEntry {
    /// 条目唯一标识(跨层映射自 WikiEntry::entry_id 等)
    pub entry_id: String,
    /// 产出该条目的 Agent ID(跨 Agent 去重判定依据)
    pub agent_id: String,
    /// 语义向量(512-dim CLV,与 WikiEntry::embedding 同源)
    pub embedding: Vec<f32>,
    /// 内容哈希(精确去重快速路径;0 表示未计算,跳过精确去重)
    ///
    /// WHY u64:调用方用 xxHash / FNV 等 64 位哈希;DedupEngine 不计算哈希(零依赖)
    pub content_hash: u64,
    /// 置信度 [0.0, 1.0](去重时保留高置信度,来源 TemporalMeta::confidence 或 GSOE 反馈)
    pub confidence: f32,
    /// 创建时间(UTC;置信度相同时保留最早创建的,稳定性)
    pub created_at: DateTime<Utc>,
}

impl DedupEntry {
    /// 创建新的去重条目
    ///
    /// # 参数
    ///
    /// - `entry_id`:条目唯一标识
    /// - `agent_id`:产出 Agent ID
    /// - `embedding`:语义向量(建议 512-dim CLV)
    /// - `content_hash`:内容哈希(0 表示未计算,跳过精确去重)
    /// - `confidence`:置信度 [0.0, 1.0]
    /// - `created_at`:创建时间(UTC)
    pub fn new(
        entry_id: impl Into<String>,
        agent_id: impl Into<String>,
        embedding: Vec<f32>,
        content_hash: u64,
        confidence: f32,
        created_at: DateTime<Utc>,
    ) -> Self {
        Self {
            entry_id: entry_id.into(),
            agent_id: agent_id.into(),
            embedding,
            content_hash,
            confidence,
            created_at,
        }
    }
}

/// 去重原因 — 三级分类(精确 / 语义 / 近似)
///
/// WHY 分类:便于调用方按原因决定后续处理(如精确重复可直接归档,
/// 近似重复可能需要人工确认或保留两者)。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DedupReason {
    /// 精确重复 — content_hash 完全相同(O(1) 快速路径)
    ExactDuplicate,
    /// 语义重复 — cosine_similarity >= `SEMANTIC_DEDUP_THRESHOLD` (0.95)
    SemanticDuplicate,
    /// 近似重复 — cosine_similarity >= `NEAR_DEDUP_THRESHOLD` (0.85)
    NearDuplicate,
}

impl DedupReason {
    /// 返回原因名称(用于日志与事件 payload)
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ExactDuplicate => "ExactDuplicate",
            Self::SemanticDuplicate => "SemanticDuplicate",
            Self::NearDuplicate => "NearDuplicate",
        }
    }
}

/// 去重关系 — 记录哪条被哪条替代(保留谱系完整性,INV-8)
///
/// 调用方据此将 `removed_id` 对应条目标记为 `Historical`(通过 TemporalMeta),
/// 并记录 `kept_id → removed_id` 的替代关系,确保审计可追溯。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DedupRelation {
    /// 保留的条目 ID(高 confidence 或早创建)
    pub kept_id: String,
    /// 被去重的条目 ID(调用方应标记为 Historical)
    pub removed_id: String,
    /// 保留条目的 Agent ID
    pub kept_agent_id: String,
    /// 被去重条目的 Agent ID(必不同于 kept_agent_id,跨 Agent 判定)
    pub removed_agent_id: String,
    /// 相似度 [0.0, 1.0](精确重复为 1.0,语义/近似为实际 cosine_similarity)
    pub similarity: f32,
    /// 去重原因(精确 / 语义 / 近似)
    pub reason: DedupReason,
}

/// 去重结果 — DedupEngine 的输出
///
/// # 字段说明
///
/// - `retained`:保留的条目 ID 列表(未被去重)
/// - `removed`:被去重的条目关系列表(调用方据此标记 Historical)
/// - `dedup_rate`:去重率 = `removed.len() / (retained.len() + removed.len())`
///
/// # 目标 >80% 的含义
///
/// 在有重复的输入集中,`dedup_rate` 应 >= 0.80。
/// 若输入无重复,`dedup_rate` = 0.0(正常,非失败)。
#[derive(Debug, Clone, PartialEq)]
pub struct DedupResult {
    /// 保留的条目 ID 列表(未被去重,顺序与输入无关)
    pub retained: Vec<String>,
    /// 被去重的条目关系列表(调用方据此标记 Historical)
    pub removed: Vec<DedupRelation>,
    /// 去重率 [0.0, 1.0] = removed / (retained + removed)
    pub dedup_rate: f32,
}

impl DedupResult {
    /// 判断是否达到 >80% 去重目标
    ///
    /// WHY: P3-W11.2.3 验收标准 — 跨 Agent 重复输入集去重率 >= 0.80
    pub fn meets_target(&self) -> bool {
        self.dedup_rate >= 0.80
    }
}

// ============================================================
// 去重引擎
// ============================================================

/// 跨 Agent 去重引擎 — P3-W11.2.3 D12 修复
///
/// 纯计算单元,无 I/O 依赖。接受 `&[DedupEntry]`,返回 `DedupResult`。
///
/// # 算法
///
/// 1. **精确去重**(快速路径):按 `content_hash` 分组,同组内跨 Agent 完全重复 → `ExactDuplicate`
/// 2. **语义去重**:对未精确匹配的,计算 embedding 余弦相似度:
///    - >= `semantic_threshold`(默认 0.95)→ `SemanticDuplicate`
///    - >= `near_threshold`(默认 0.85)→ `NearDuplicate`
///    - 仅跨 Agent 配对(`kept.agent_id != removed.agent_id`)
/// 3. **保留策略**:同组/同对中保留 `confidence` 最高的;若相同,保留 `created_at` 最早的
///
/// # 示例
///
/// ```
/// use chimera_mas::archive::dedup::{DedupEngine, DedupEntry};
/// use chrono::Utc;
///
/// let now = Utc::now();
/// // 两个不同 Agent 产出完全相同的条目(content_hash 相同)
/// let entries = vec![
///     DedupEntry::new("e-1", "agent-A", vec![0.1, 0.2], 12345, 0.9, now),
///     DedupEntry::new("e-2", "agent-B", vec![0.1, 0.2], 12345, 0.8, now),
/// ];
///
/// let engine = DedupEngine::new();
/// let result = engine.dedup(&entries);
///
/// // 跨 Agent 精确去重:保留 e-1(高 confidence),去重 e-2
/// assert_eq!(result.retained.len(), 1);
/// assert_eq!(result.removed.len(), 1);
/// assert_eq!(result.removed[0].kept_id, "e-1");    // 保留高 confidence
/// assert_eq!(result.removed[0].removed_id, "e-2");
/// // 注:meets_target() 验证 >=80% 去重率,需大型数据集;此处仅 2 条(rate=0.5)
/// ```
#[derive(Debug, Clone)]
pub struct DedupEngine {
    /// 语义去重阈值(cosine_similarity >= 此值 → SemanticDuplicate)
    semantic_threshold: f32,
    /// 近似去重阈值(cosine_similarity >= 此值 → NearDuplicate)
    near_threshold: f32,
}

impl Default for DedupEngine {
    fn default() -> Self {
        Self {
            semantic_threshold: SEMANTIC_DEDUP_THRESHOLD,
            near_threshold: NEAR_DEDUP_THRESHOLD,
        }
    }
}

impl DedupEngine {
    /// 创建默认阈值的去重引擎(semantic=0.95, near=0.85)
    pub fn new() -> Self {
        Self::default()
    }

    /// 创建自定义阈值的去重引擎
    ///
    /// # 参数
    ///
    /// - `semantic_threshold`:语义去重阈值(应 > near_threshold)
    /// - `near_threshold`:近似去重阈值
    ///
    /// # Panic
    ///
    /// 若 `semantic_threshold < near_threshold`,调用方传入矛盾阈值时,
    /// 构造器自动交换两者确保 `semantic >= near`(避免运行时 panic,符合 §4.1 无 panic 原则)
    pub fn with_thresholds(semantic_threshold: f32, near_threshold: f32) -> Self {
        // WHY 自动交换而非 panic:调用方可能从配置文件读取阈值,顺序不确定。
        // 自动交换保证 invariant(semantic >= near),避免运行时 panic(§4.1 红线)。
        let (semantic, near) = if semantic_threshold >= near_threshold {
            (semantic_threshold, near_threshold)
        } else {
            (near_threshold, semantic_threshold)
        };
        Self {
            semantic_threshold: semantic,
            near_threshold: near,
        }
    }

    /// 执行跨 Agent 去重
    ///
    /// # 参数
    ///
    /// - `entries`:待去重的条目切片
    ///
    /// # 返回
    ///
    /// `DedupResult` — 保留条目 ID 列表 + 被去重关系列表 + 去重率
    ///
    /// # 算法
    ///
    /// 1. 精确去重(content_hash 分组)
    /// 2. 语义去重(cosine_similarity 配对)
    /// 3. 计算去重率
    ///
    /// # 红线对齐
    ///
    /// - §6.1: 单函数 ≤ 200 行(本函数仅做编排,具体逻辑在 `dedup_exact` / `dedup_semantic`)
    pub fn dedup(&self, entries: &[DedupEntry]) -> DedupResult {
        if entries.is_empty() {
            return DedupResult {
                retained: Vec::new(),
                removed: Vec::new(),
                dedup_rate: 0.0,
            };
        }

        // 跟踪已被去重的 entry_id(精确去重后不再参与语义去重,避免链式去重)
        let mut removed_set: HashSet<String> = HashSet::new();
        let mut relations: Vec<DedupRelation> = Vec::new();

        // 1. 精确去重(O(n) 分组)
        self.dedup_exact(entries, &mut removed_set, &mut relations);

        // 2. 语义去重(O(n²) 配对,n 为剩余未去重条目)
        self.dedup_semantic(entries, &removed_set, &mut relations);

        // 3. 构造保留列表(未被去重的条目)
        let retained: Vec<String> = entries
            .iter()
            .filter(|e| !removed_set.contains(&e.entry_id))
            .map(|e| e.entry_id.clone())
            .collect();

        // 4. 计算去重率(§4.4 反模式 6:全程 f32)
        let total = entries.len() as f32;
        let removed_count = relations.len() as f32;
        let dedup_rate = if total > 0.0 {
            removed_count / total
        } else {
            0.0
        };

        DedupResult {
            retained,
            removed: relations,
            dedup_rate,
        }
    }

    /// 精确去重 — 按 content_hash 分组,同组内跨 Agent 完全重复
    ///
    /// # 保留策略
    ///
    /// 同组中保留 `confidence` 最高的;若 `confidence` 相同(差异 < `CONFIDENCE_EQUAL_TOLERANCE`),
    /// 保留 `created_at` 最早的。
    ///
    /// # 跨 Agent 判定
    ///
    /// 仅当 `kept.agent_id != removed.agent_id` 时才记录去重关系。
    /// 同 Agent 内的精确重复留给 Agent 自身处理(避免越权)。
    ///
    /// # 红线对齐
    ///
    /// - §6.1: 单函数 ≤ 200 行(本函数 ~60 行)
    fn dedup_exact(
        &self,
        entries: &[DedupEntry],
        removed_set: &mut HashSet<String>,
        relations: &mut Vec<DedupRelation>,
    ) {
        // 按 content_hash 分组(hash=0 的跳过,表示未计算)
        let mut groups: HashMap<u64, Vec<usize>> = HashMap::new();
        for (idx, entry) in entries.iter().enumerate() {
            if entry.content_hash != 0 {
                groups.entry(entry.content_hash).or_default().push(idx);
            }
        }

        // 对每个 hash 组(>=2 个条目)执行精确去重
        for (_, mut indices) in groups {
            if indices.len() < 2 {
                continue;
            }

            // 找出保留条目(最高 confidence,最早 created_at)。
            // WHY 仅找 Top-1 而非全排序:后续只需 indices[0] 作为 kept 条目,
            // indices[1..] 仅用于遍历检查跨 Agent 去重,无需有序。
            // 用 O(n) 线性扫描替代 O(n log n) 全排序(§6.2: Top-K 用 select_nth)。
            let is_better = |candidate: usize, current_best: usize| -> bool {
                let ec = &entries[candidate];
                let eb = &entries[current_best];
                let conf_diff = ec.confidence - eb.confidence;
                if conf_diff.abs() > CONFIDENCE_EQUAL_TOLERANCE {
                    return conf_diff > 0.0; // 高 confidence 更好
                }
                ec.created_at < eb.created_at // 早创建更好
            };
            let mut best_pos = 0;
            for pos in 1..indices.len() {
                if is_better(indices[pos], indices[best_pos]) {
                    best_pos = pos;
                }
            }
            // 将最佳移到 indices[0]
            if best_pos != 0 {
                indices.swap(0, best_pos);
            }

            // 保留 indices[0](最佳),其余跨 Agent 的标记去重
            let kept_idx = indices[0];
            let kept = &entries[kept_idx];
            for &ridx in indices.iter().skip(1) {
                let removed = &entries[ridx];
                // 跨 Agent 判定(核心语义)
                if kept.agent_id == removed.agent_id {
                    continue; // 同 Agent 内重复,留给 Agent 自身处理
                }
                // 避免重复标记(可能已被其他 hash 组去重,虽然概率低)
                if removed_set.contains(&removed.entry_id) {
                    continue;
                }
                removed_set.insert(removed.entry_id.clone());
                relations.push(DedupRelation {
                    kept_id: kept.entry_id.clone(),
                    removed_id: removed.entry_id.clone(),
                    kept_agent_id: kept.agent_id.clone(),
                    removed_agent_id: removed.agent_id.clone(),
                    similarity: 1.0, // content_hash 相同 → 完全相同
                    reason: DedupReason::ExactDuplicate,
                });
            }
        }
    }

    /// 语义去重 — 对未精确匹配的条目,计算 cosine_similarity 配对
    ///
    /// # 算法
    ///
    /// 1. 收集未去重的条目索引
    /// 2. 两两计算跨 Agent 配对的相似度
    /// 3. 按相似度降序排序
    /// 4. 贪心选择:若两个都未被去重,去重 confidence 较低的
    ///
    /// # 复杂度
    ///
    /// O(n²) 配对 + O(n² log n) 排序。n 为剩余条目数(通常 <1000,可接受)。
    ///
    /// # 红线对齐
    ///
    /// - §6.1: 单函数 ≤ 200 行(本函数 ~80 行)
    fn dedup_semantic(
        &self,
        entries: &[DedupEntry],
        removed_set: &HashSet<String>,
        relations: &mut Vec<DedupRelation>,
    ) {
        // 收集未去重的条目索引
        let candidates: Vec<usize> = entries
            .iter()
            .enumerate()
            .filter(|(_, e)| !removed_set.contains(&e.entry_id))
            .map(|(i, _)| i)
            .collect();

        if candidates.len() < 2 {
            return;
        }

        // 计算所有跨 Agent 配对的相似度
        let mut pairs: Vec<(usize, usize, f32, DedupReason)> = Vec::new();
        for i in 0..candidates.len() {
            for j in (i + 1)..candidates.len() {
                let a = &entries[candidates[i]];
                let b = &entries[candidates[j]];
                // 跨 Agent 判定(核心语义)
                if a.agent_id == b.agent_id {
                    continue;
                }
                let sim = cosine_similarity_slices(&a.embedding, &b.embedding);
                if sim >= self.semantic_threshold {
                    pairs.push((
                        candidates[i],
                        candidates[j],
                        sim,
                        DedupReason::SemanticDuplicate,
                    ));
                } else if sim >= self.near_threshold {
                    pairs.push((
                        candidates[i],
                        candidates[j],
                        sim,
                        DedupReason::NearDuplicate,
                    ));
                }
            }
        }

        // 按相似度降序排序(高相似度优先去重)。
        // WHY 此处必须全排序而非 Top-K:贪心去重算法依赖按相似度降序处理配对,
        // 确保最高相似度的配对优先去重,避免低相似度配对抢先占用条目导致
        // 高相似度配对被跳过(去重质量下降)。全排序 O(n² log n) 是算法正确性所需。
        pairs.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));

        // 贪心去重:遍历配对,若两个都未被去重,去重 confidence 较低的
        let mut local_removed: HashSet<String> = HashSet::new();
        for (i_idx, j_idx, sim, reason) in pairs {
            let a = &entries[i_idx];
            let b = &entries[j_idx];
            // 跳过已被去重的(避免链式去重)
            if local_removed.contains(&a.entry_id) || local_removed.contains(&b.entry_id) {
                continue;
            }

            // 保留 confidence 高的(若相同,保留 created_at 早的)
            let (kept, removed) = if should_keep_over(a, b) {
                (a, b)
            } else {
                (b, a)
            };

            local_removed.insert(removed.entry_id.clone());
            relations.push(DedupRelation {
                kept_id: kept.entry_id.clone(),
                removed_id: removed.entry_id.clone(),
                kept_agent_id: kept.agent_id.clone(),
                removed_agent_id: removed.agent_id.clone(),
                similarity: sim,
                reason,
            });
        }
    }
}

// ============================================================
// 辅助函数
// ============================================================

// 统一使用 nexus-core 权威实现,避免多副本优化不一致
// cosine_similarity_slices 已覆盖:零向量返回 0.0、clamp [-1.0, 1.0]、不等长输入兼容

/// 判断 `a` 是否应保留而非 `b`(保留策略)
///
/// 返回 `true` 表示 `a` 保留、`b` 去重;返回 `false` 表示 `b` 保留、`a` 去重。
///
/// # 策略
///
/// 1. `confidence` 高的保留(降序)
/// 2. 若 `confidence` 相同(差异 < `CONFIDENCE_EQUAL_TOLERANCE`),`created_at` 早的保留(升序)
/// 3. 若都相同,保留 `entry_id` 字典序小的(确定性,避免随机性)
fn should_keep_over(a: &DedupEntry, b: &DedupEntry) -> bool {
    // confidence 降序(高优先)
    let conf_diff = a.confidence - b.confidence;
    if conf_diff.abs() > CONFIDENCE_EQUAL_TOLERANCE {
        return conf_diff > 0.0;
    }
    // created_at 升序(早优先)
    if a.created_at != b.created_at {
        return a.created_at < b.created_at;
    }
    // entry_id 字典序(确定性)
    a.entry_id <= b.entry_id
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 创建测试用 DedupEntry 的辅助函数
    fn make_entry(
        id: &str,
        agent: &str,
        embedding: Vec<f32>,
        hash: u64,
        confidence: f32,
    ) -> DedupEntry {
        DedupEntry::new(id, agent, embedding, hash, confidence, Utc::now())
    }

    // ============================================================
    // 常量与类型测试
    // ============================================================

    #[test]
    fn test_dedup_reason_as_str() {
        assert_eq!(DedupReason::ExactDuplicate.as_str(), "ExactDuplicate");
        assert_eq!(DedupReason::SemanticDuplicate.as_str(), "SemanticDuplicate");
        assert_eq!(DedupReason::NearDuplicate.as_str(), "NearDuplicate");
    }

    #[test]
    fn test_dedup_entry_new() {
        let entry = DedupEntry::new("e-1", "agent-A", vec![0.1, 0.2], 12345, 0.9, Utc::now());
        assert_eq!(entry.entry_id, "e-1");
        assert_eq!(entry.agent_id, "agent-A");
        assert_eq!(entry.embedding, vec![0.1, 0.2]);
        assert_eq!(entry.content_hash, 12345);
        assert!((entry.confidence - 0.9).abs() < 1e-6);
    }

    #[test]
    fn test_dedup_result_meets_target() {
        // 去重率 0.85 → 达标
        let result = DedupResult {
            retained: vec!["e-1".into()],
            removed: vec![],
            dedup_rate: 0.85,
        };
        assert!(result.meets_target());

        // 去重率 0.80 → 达标(边界)
        let result = DedupResult {
            retained: vec!["e-1".into()],
            removed: vec![],
            dedup_rate: 0.80,
        };
        assert!(result.meets_target());

        // 去重率 0.79 → 未达标
        let result = DedupResult {
            retained: vec!["e-1".into()],
            removed: vec![],
            dedup_rate: 0.79,
        };
        assert!(!result.meets_target());
    }

    // ============================================================
    // cosine_similarity_slices 测试
    // ============================================================

    #[test]
    fn test_cosine_similarity_identical() {
        let a = vec![0.1, 0.2, 0.3];
        let sim = cosine_similarity_slices(&a, &a);
        assert!(
            (sim - 1.0).abs() < 1e-5,
            "相同向量相似度应为 1.0,实际: {sim}"
        );
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        let a = vec![1.0, 0.0];
        let b = vec![0.0, 1.0];
        let sim = cosine_similarity_slices(&a, &b);
        assert!(sim.abs() < 1e-5, "正交向量相似度应为 0,实际: {sim}");
    }

    #[test]
    fn test_cosine_similarity_empty() {
        assert_eq!(cosine_similarity_slices(&[], &[]), 0.0);
        assert_eq!(cosine_similarity_slices(&[1.0], &[]), 0.0);
    }

    #[test]
    fn test_cosine_similarity_unequal_length() {
        // 权威实现取 min 长度计算:仅用首元素,dot=1.0, norm_a=1.0, norm_b=1.0 → 1.0
        let sim = cosine_similarity_slices(&[1.0, 2.0], &[1.0]);
        assert!(
            (sim - 1.0).abs() < 1e-5,
            "不等长输入取 min 长度,预期 1.0,实际: {sim}"
        );
    }

    #[test]
    fn test_cosine_similarity_zero_vector() {
        assert_eq!(cosine_similarity_slices(&[0.0, 0.0], &[1.0, 1.0]), 0.0);
    }

    // ============================================================
    // 精确去重测试(ExactDuplicate)
    // ============================================================

    #[test]
    fn test_dedup_exact_cross_agent() {
        // 两个不同 Agent,完全相同 content_hash → 跨 Agent 精确去重
        let now = Utc::now();
        let entries = vec![
            DedupEntry::new("e-1", "agent-A", vec![0.1, 0.2], 12345, 0.9, now),
            DedupEntry::new("e-2", "agent-B", vec![0.1, 0.2], 12345, 0.8, now),
        ];
        let result = DedupEngine::new().dedup(&entries);

        assert_eq!(result.retained.len(), 1, "应保留 1 条");
        assert_eq!(result.removed.len(), 1, "应去重 1 条");
        assert_eq!(result.removed[0].reason, DedupReason::ExactDuplicate);
        assert_eq!(
            result.removed[0].kept_id, "e-1",
            "应保留高 confidence 的 e-1"
        );
        assert_eq!(result.removed[0].removed_id, "e-2");
        assert_ne!(
            result.removed[0].kept_agent_id, result.removed[0].removed_agent_id,
            "必须跨 Agent"
        );
        assert!((result.removed[0].similarity - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_dedup_exact_same_agent_not_deduped() {
        // 同 Agent 内的精确重复 → 不去重(留给 Agent 自身处理)
        let now = Utc::now();
        let entries = vec![
            DedupEntry::new("e-1", "agent-A", vec![0.1, 0.2], 12345, 0.9, now),
            DedupEntry::new("e-2", "agent-A", vec![0.1, 0.2], 12345, 0.8, now),
        ];
        let result = DedupEngine::new().dedup(&entries);

        assert_eq!(result.retained.len(), 2, "同 Agent 不去重,全保留");
        assert_eq!(result.removed.len(), 0);
        assert!((result.dedup_rate - 0.0).abs() < 1e-5);
    }

    #[test]
    fn test_dedup_exact_keeps_higher_confidence() {
        // 保留 confidence 高的(即使后创建)
        let early = Utc::now();
        let late = early + chrono::Duration::seconds(10);
        let entries = vec![
            DedupEntry::new("e-1", "agent-A", vec![0.1], 999, 0.7, late),
            DedupEntry::new("e-2", "agent-B", vec![0.1], 999, 0.9, early),
        ];
        let result = DedupEngine::new().dedup(&entries);

        assert_eq!(
            result.retained,
            vec!["e-2".to_string()],
            "应保留高 confidence 的 e-2"
        );
        assert_eq!(result.removed[0].removed_id, "e-1");
    }

    #[test]
    fn test_dedup_exact_tie_keeps_earlier_created() {
        // confidence 相同时,保留 created_at 早的
        let early = Utc::now();
        let late = early + chrono::Duration::seconds(10);
        let entries = vec![
            DedupEntry::new("e-1", "agent-A", vec![0.1], 999, 0.9, late),
            DedupEntry::new("e-2", "agent-B", vec![0.1], 999, 0.9, early),
        ];
        let result = DedupEngine::new().dedup(&entries);

        assert_eq!(
            result.retained,
            vec!["e-2".to_string()],
            "confidence 相同时应保留 created_at 早的 e-2"
        );
    }

    #[test]
    fn test_dedup_hash_zero_skipped() {
        // content_hash=0 → 跳过精确去重(但可能被语义去重捕获)
        let entries = vec![
            make_entry("e-1", "agent-A", vec![1.0, 0.0], 0, 0.9),
            make_entry("e-2", "agent-B", vec![1.0, 0.0], 0, 0.8),
        ];
        let result = DedupEngine::new().dedup(&entries);

        // hash=0 跳过精确去重,但向量完全相同 → 语义去重捕获
        assert_eq!(result.removed.len(), 1);
        assert_eq!(result.removed[0].reason, DedupReason::SemanticDuplicate);
    }

    // ============================================================
    // 语义/近似去重测试
    // ============================================================

    #[test]
    fn test_dedup_semantic_high_similarity() {
        // cosine_similarity = 1.0(向量相同)→ SemanticDuplicate
        let entries = vec![
            make_entry("e-1", "agent-A", vec![1.0, 0.5, 0.3], 0, 0.9),
            make_entry("e-2", "agent-B", vec![1.0, 0.5, 0.3], 0, 0.8),
        ];
        let result = DedupEngine::new().dedup(&entries);

        assert_eq!(result.removed.len(), 1);
        assert_eq!(result.removed[0].reason, DedupReason::SemanticDuplicate);
        assert!((result.removed[0].similarity - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_dedup_near_duplicate() {
        // cosine_similarity 在 [0.85, 0.95) → NearDuplicate
        // 构造相似度 ≈ 0.914 的向量:
        //   a=[1.0, 0.0, 0.0], b=[0.9, 0.4, 0.0]
        //   cos = 0.9 / (1.0 × sqrt(0.81+0.16)) ≈ 0.914 ∈ [0.85, 0.95)
        let entries = vec![
            make_entry("e-1", "agent-A", vec![1.0, 0.0, 0.0], 0, 0.9),
            make_entry("e-2", "agent-B", vec![0.9, 0.4, 0.0], 0, 0.8),
        ];
        let result = DedupEngine::new().dedup(&entries);

        // 验证被去重且为 NearDuplicate(相似度应在 [0.85, 0.95))
        assert_eq!(result.removed.len(), 1, "应去重 1 条");
        assert_eq!(
            result.removed[0].reason,
            DedupReason::NearDuplicate,
            "相似度 {} 应判定为 NearDuplicate",
            result.removed[0].similarity
        );
        assert!(result.removed[0].similarity >= NEAR_DEDUP_THRESHOLD);
        assert!(result.removed[0].similarity < SEMANTIC_DEDUP_THRESHOLD);
    }

    #[test]
    fn test_dedup_below_threshold_not_deduped() {
        // cosine_similarity < 0.85 → 不去重(独立条目)
        let entries = vec![
            make_entry("e-1", "agent-A", vec![1.0, 0.0, 0.0], 0, 0.9),
            make_entry("e-2", "agent-B", vec![0.0, 1.0, 0.0], 0, 0.8),
        ];
        let result = DedupEngine::new().dedup(&entries);

        assert_eq!(result.retained.len(), 2, "独立条目不去重");
        assert_eq!(result.removed.len(), 0);
    }

    #[test]
    fn test_dedup_semantic_same_agent_not_deduped() {
        // 同 Agent 的高相似度 → 不去重(跨 Agent 判定核心)
        let entries = vec![
            make_entry("e-1", "agent-A", vec![1.0, 0.5, 0.3], 0, 0.9),
            make_entry("e-2", "agent-A", vec![1.0, 0.5, 0.3], 0, 0.8),
        ];
        let result = DedupEngine::new().dedup(&entries);

        assert_eq!(result.retained.len(), 2, "同 Agent 不去重");
        assert_eq!(result.removed.len(), 0);
    }

    // ============================================================
    // 去重率与目标测试(>80% 影子集)
    // ============================================================

    #[test]
    fn test_dedup_rate_meets_target_80_percent() {
        // 影子集:100 条,其中 85 条是跨 Agent 重复 → 去重率 85% > 80%
        let mut entries = Vec::new();
        let base_vec = vec![0.5; 8];

        // 15 条独立条目(不同向量)
        for i in 0..15 {
            let mut v = base_vec.clone();
            v[0] = i as f32; // 差异化
            entries.push(make_entry(
                &format!("unique-{i}"),
                "agent-A",
                v,
                1000 + i as u64,
                0.9,
            ));
        }

        // 85 条跨 Agent 重复(与某条独立条目 content_hash 相同)
        for i in 0..85 {
            let agent = if i % 2 == 0 { "agent-B" } else { "agent-C" };
            entries.push(make_entry(
                &format!("dup-{i}"),
                agent,
                base_vec.clone(),
                1000, // 与 unique-0 同 hash
                0.5,  // 低 confidence,会被去重
            ));
        }

        let result = DedupEngine::new().dedup(&entries);

        // 85 条重复应被去重(跨 Agent)
        assert_eq!(
            result.removed.len(),
            85,
            "应去重 85 条跨 Agent 重复,实际 {}",
            result.removed.len()
        );
        assert!(
            result.meets_target(),
            "去重率 {} 应 >= 0.80",
            result.dedup_rate
        );
    }

    #[test]
    fn test_dedup_rate_zero_when_no_duplicates() {
        // 无重复输入 → 去重率 0(正常,非失败)
        let entries = vec![
            make_entry("e-1", "agent-A", vec![1.0, 0.0], 1, 0.9),
            make_entry("e-2", "agent-B", vec![0.0, 1.0], 2, 0.8),
        ];
        let result = DedupEngine::new().dedup(&entries);

        assert!((result.dedup_rate - 0.0).abs() < 1e-5);
        assert_eq!(result.retained.len(), 2);
    }

    #[test]
    fn test_dedup_empty_input() {
        let result = DedupEngine::new().dedup(&[]);
        assert!(result.retained.is_empty());
        assert!(result.removed.is_empty());
        assert!((result.dedup_rate - 0.0).abs() < 1e-5);
    }

    // ============================================================
    // 引擎配置测试
    // ============================================================

    #[test]
    fn test_with_thresholds_auto_swap() {
        // 传入矛盾阈值(semantic < near)→ 自动交换
        let engine = DedupEngine::with_thresholds(0.80, 0.95);
        assert!(engine.semantic_threshold >= engine.near_threshold);
        assert!((engine.semantic_threshold - 0.95).abs() < 1e-5);
        assert!((engine.near_threshold - 0.80).abs() < 1e-5);
    }

    #[test]
    fn test_with_thresholds_normal() {
        let engine = DedupEngine::with_thresholds(0.95, 0.85);
        assert!((engine.semantic_threshold - 0.95).abs() < 1e-5);
        assert!((engine.near_threshold - 0.85).abs() < 1e-5);
    }

    #[test]
    fn test_custom_thresholds_affect_dedup() {
        // 提高阈值到 0.99 → 原来 0.95 相似度的条目不再去重
        let entries = vec![
            make_entry("e-1", "agent-A", vec![1.0, 0.5, 0.3], 0, 0.9),
            make_entry("e-2", "agent-B", vec![1.0, 0.5, 0.3], 0, 0.8),
        ];

        // 默认阈值 0.95 → 去重
        let result_default = DedupEngine::new().dedup(&entries);
        assert_eq!(result_default.removed.len(), 1);

        // 提高两个阈值到 1.01(超过 1.0,cosine_similarity ≤ 1.0 永不触发)→ 不去重
        // WHY 两个都 1.01:near=1.00 时 sim=1.0 >= 1.00 仍触发 NearDuplicate
        let engine_strict = DedupEngine::with_thresholds(1.01, 1.01);
        let result_strict = engine_strict.dedup(&entries);
        assert_eq!(result_strict.removed.len(), 0, "严格阈值不应去重");
    }

    // ============================================================
    // 贪心去重测试(避免链式去重)
    // ============================================================

    #[test]
    fn test_greedy_no_chain_dedup() {
        // 三条条目:A-B 相似 0.99,B-C 相似 0.99,A-C 相似 0.5
        // 贪心应去重 B(保留 A),C 不应被链式去重(A-C 相似度低)
        let entries = vec![
            make_entry("A", "agent-A", vec![1.0, 0.0], 0, 0.9),
            make_entry("B", "agent-B", vec![1.0, 0.01], 0, 0.5), // 与 A 极相似
            make_entry("C", "agent-C", vec![0.0, 1.0], 0, 0.5),  // 与 A 正交
        ];
        let result = DedupEngine::new().dedup(&entries);

        // B 应被去重(与 A 跨 Agent 高相似)
        assert!(result.removed.iter().any(|r| r.removed_id == "B"));
        // C 不应被去重(与 A 正交,且 A-B 去重后 B 不再参与配对)
        assert!(!result.removed.iter().any(|r| r.removed_id == "C"));
        assert!(result.retained.iter().any(|id| id == "A"));
        assert!(result.retained.iter().any(|id| id == "C"));
    }

    #[test]
    fn test_dedup_relation_cross_agent_invariant() {
        // 验证所有去重关系都满足跨 Agent 不变量
        let entries = vec![
            make_entry("e-1", "agent-A", vec![1.0, 0.0], 1, 0.9),
            make_entry("e-2", "agent-B", vec![1.0, 0.0], 1, 0.8),
            make_entry("e-3", "agent-C", vec![1.0, 0.0], 1, 0.7),
            make_entry("e-4", "agent-A", vec![0.0, 1.0], 2, 0.9),
        ];
        let result = DedupEngine::new().dedup(&entries);

        for rel in &result.removed {
            assert_ne!(
                rel.kept_agent_id, rel.removed_agent_id,
                "去重关系必须跨 Agent: {:?}",
                rel
            );
        }
    }

    #[test]
    fn test_dedup_serde_roundtrip() {
        let now = Utc::now();
        let entry = DedupEntry::new("e-1", "agent-A", vec![0.1, 0.2], 12345, 0.9, now);
        let json = serde_json::to_string(&entry).expect("序列化失败");
        let restored: DedupEntry = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(entry, restored);
    }
}
