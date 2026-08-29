//! HCW 核心领域类型 — 分层上下文窗口的统一数据模型
//!
//! 对应架构层:L2 Memory
//! 对应创新点:HCW(Hierarchical Context Window,分层上下文窗口)
//!
//! # 类型职责
//! - `WindowTier`:四级窗口层级(L0=4K/L1=32K/L2=128K/L3=1M 等效)
//! - `ContextEntry`:上下文条目(携带 file_id、token_size、CLV 等,用于重要性评分)
//! - `HcwState`:HCW 内部状态(当前层级、条目列表、最近掩码哈希)
//! - `CompressionReport`:压缩报告(原始/压缩后大小、保留/丢弃条目数、压缩比 compression_ratio)
//! - `HcwConfig`:HCW 配置(四级容量、压缩阈值,impl 块在 config.rs)
//!
//! # 设计决策(WHY)
//! - **WindowTier 四档**:对应架构手册 §HCW 四级窗口,
//!   L0(4K 快速响应)/L1(32K 常规)/L2(128K 复杂)/L3(1M 等效,分层+稀疏化)
//! - **L3 等效容量 = l3_capacity / 8**:1M 等效不通过暴力加载,而是 128K 实际加载
//!   + 8× 压缩比(OSA 稀疏化跳过 87.5% 内容),避免内存爆炸(架构红线)
//! - **ContextEntry 携带 `Option<CLV>`**:任务相关性基于 CLV 余弦相似度计算,
//!   无 CLV 时相关性取中性值 0.5,避免阻塞压缩流程
//! - **CompressionReport.compression_ratio = original/compressed**:压缩比(>1.0,越大压缩越多),
//!   与事件 payload 的 ratio(=compressed/original ∈ `[0,1]`)方向相反,发布事件时转换。
//!   `compressed_size=0` 时取 `f32::MAX`(非 INFINITY,避免序列化失败)

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use nexus_contracts::SelectorPolicy;
use nexus_core::CLV;
use serde::{Deserialize, Serialize};

/// 窗口层级 — 四级上下文窗口
///
/// 对应架构手册 §HCW 四级窗口:
/// - `L0`:4K Token,快速响应(简单任务,complexity < 0.25)
/// - `L1`:32K Token,常规任务(0.25 ≤ complexity < 0.5)
/// - `L2`:128K Token,复杂任务(0.5 ≤ complexity < 0.75)
/// - `L3`:1M Token 等效,超复杂任务(complexity ≥ 0.75)
///
/// WHY:L3 的 1M 等效通过"分层 + 稀疏化"实现,而非暴力加载:
/// 实际加载容量 = l3_capacity / 8 = 128K,通过 OSA 稀疏化(8× 压缩比)
/// 跳过 87.5% 内容,实现 1M 等效(架构红线:禁止 1M 暴力加载)
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum WindowTier {
    /// L0 窗口:4K Token,快速响应(简单任务)
    L0,
    /// L1 窗口:32K Token,常规任务
    L1,
    /// L2 窗口:128K Token,复杂任务
    L2,
    /// L3 窗口:1M Token 等效(128K 实际加载 + 8× 稀疏化压缩比)
    L3,
}

impl WindowTier {
    /// 返回层级名称(用于事件 payload 与日志)
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::L0 => "L0",
            Self::L1 => "L1",
            Self::L2 => "L2",
            Self::L3 => "L3",
        }
    }

    /// 从字符串解析层级(用于事件消费与配置反序列化)
    pub fn parse_tier(s: &str) -> Option<Self> {
        match s {
            "L0" => Some(Self::L0),
            "L1" => Some(Self::L1),
            "L2" => Some(Self::L2),
            "L3" => Some(Self::L3),
            _ => None,
        }
    }

    /// 升级到更高层级(L0→L1→L2→L3),L3 返回 None
    ///
    /// WHY:窗口溢出降级链使用,逐级升级直到容量足够或达 L3
    pub fn upgrade(self) -> Option<Self> {
        match self {
            Self::L0 => Some(Self::L1),
            Self::L1 => Some(Self::L2),
            Self::L2 => Some(Self::L3),
            Self::L3 => None,
        }
    }

    /// 降级到更低层级(L3→L2→L1→L0),L0 返回 None
    pub fn downgrade(self) -> Option<Self> {
        match self {
            Self::L0 => None,
            Self::L1 => Some(Self::L0),
            Self::L2 => Some(Self::L1),
            Self::L3 => Some(Self::L2),
        }
    }

    /// 返回该层级的标称容量(Token 数,含 L3 的 1M 等效值)
    ///
    /// WHY:标称容量用于事件 payload 与监控指标,实际加载容量见 `effective_capacity`
    pub fn capacity(self, config: &HcwConfig) -> usize {
        match self {
            Self::L0 => config.l0_capacity,
            Self::L1 => config.l1_capacity,
            Self::L2 => config.l2_capacity,
            Self::L3 => config.l3_capacity,
        }
    }

    /// 返回该层级的实际加载容量(Token 数,支持 OSA 动态稀疏度)
    ///
    /// WHY(Task 4 HCW L3 动态容量):L3 的实际加载容量不再硬编码 `/8`,
    /// 而是根据 OSA 实时稀疏度自适应:
    /// - `sparsity=Some(s)`(动态模式):容量 = `l3_capacity × (1.0 - s)`,
    ///   稀疏度越高实际加载越少,实现 1M 等效(架构红线:禁止 1M 暴力加载)。
    /// - `sparsity=None`(fallback 模式):容量 = `l3_capacity / 8`(硬编码 8× 压缩比),
    ///   对应 OSA 尚未下发掩码的初始状态,等价于稀疏度 0.875(87.5% 跳过)。
    ///
    /// L0/L1/L2 的实际容量 = 标称容量(无稀疏化,忽略 `sparsity` 参数)。
    ///
    /// # 参数
    /// - `sparsity`:OSA 动态稀疏度 `Option<f32>`,`Some(s)` ∈ [0.0, 1.0]:
    ///   - `0.0`:无稀疏,全加载(L3 容量 = l3_capacity)
    ///   - `0.875`:8× 压缩比(L3 容量 = l3_capacity / 8,与 fallback 一致)
    ///   - `0.99`:最大稀疏,仅加载 1%(clamp 上限,避免容量为 0)
    ///   - `None`:fallback 到硬编码 `l3_capacity / 8`
    ///
    /// # 安全约束
    /// `sparsity` 被 clamp 到 `[0.0, 0.99]`,确保 L3 容量至少为 `l3_capacity` 的 1%,
    /// 避免 100% 稀疏导致空窗口(实际 OSA 不会 100% 稀疏,此处为防御性边界)。
    pub fn effective_capacity(self, config: &HcwConfig, sparsity: Option<f32>) -> usize {
        match self {
            Self::L0 => config.l0_capacity,
            Self::L1 => config.l1_capacity,
            Self::L2 => config.l2_capacity,
            // L3:动态容量 — 优先用 OSA 实时稀疏度,fallback 到硬编码 8× 压缩比
            Self::L3 => match sparsity {
                Some(s) if !s.is_nan() => {
                    // clamp 到 [0.0, 0.99]:避免负值/超界/100% 稀疏导致空窗口
                    // WHY NaN 检查:f32::NAN.clamp 不生效(NaN 比较全为 false),
                    // 会导致 1.0 - NaN = NaN,容量 = NaN as usize = 0(空窗口)。
                    // NaN 视为异常值,走下面的 None fallback 分支
                    let clamped = s.clamp(0.0, 0.99);
                    // 容量 = l3_capacity × (1 - 稀疏度),全程 f32 计算后转 usize
                    // WHY f32 全程:sparsity 是 f32,中间转 f64 会引入精度膨胀(§4.4 教训 #6)
                    let load_ratio = 1.0_f32 - clamped;
                    ((config.l3_capacity as f32) * load_ratio) as usize
                }
                _ => {
                    // fallback:OSA 尚未下发掩码(None)或 sparsity 为 NaN(异常值),
                    // 用硬编码 8× 压缩比(等价于 sparsity=0.875)
                    config.l3_capacity / 8
                }
            },
        }
    }

    // === PROBE P3.1: 有效窗口折减（装窗/检索分流判定）===

    /// 有效窗口折减 — 模型宣称窗口 × 60%
    ///
    /// # 参数
    /// - `model_claimed`: 模型宣称的上下文窗口（token，如 1M）
    ///
    /// # 返回
    /// `model_claimed × EFFECTIVE_FOLD_FACTOR`（f32 中间值防溢出——
    /// u64/usize 大数百分比必须用 f32 中间值，红线 §4.3）
    ///
    /// # 语义（P3 设计：结构兜底）
    /// 只影响**装窗/检索分流判定**：语料 > 有效窗口 → 走 P3.2 两级检索
    /// 兜底链（kvbsr 候选 → repo-wiki 精排 → 三区装窗）；
    /// **L3 实际加载语义零变化**（加载仍由 `effective_capacity` 决定）。
    ///
    /// 与 `effective_capacity_for` 正交叠加取 min：
    /// `min(effective_capacity_for(tier, sparsity), effective_fold(claimed))`
    /// ——折减是模型侧上限（宣称不可全信），稀疏度是系统侧上限（OSA 预算），
    /// 两者语义正交，取 min 即最保守可用窗口。
    pub fn effective_fold(model_claimed: usize) -> usize {
        ((model_claimed as f32) * EFFECTIVE_FOLD_FACTOR) as usize
    }
}

// === PROBE P3.1: 有效窗口折减常量 ===

/// 有效窗口折减系数（PROBE P3.1）— 模型宣称窗口 × 60%
///
/// WHY 60%: 宣称窗口含系统/格式/推理预留，实际可用约 6 成
/// （设计文档 §2.5 P3.1：宣称×60%，保守结构兜底）
pub const EFFECTIVE_FOLD_FACTOR: f32 = 0.6;

/// 上下文条目 — HCW 管理的最小单元
///
/// 携带 file_id(用于 OSA 掩码稀疏化)、token_size(用于容量计算)、
/// CLV(用于任务相关性评分)等字段,支持重要性评分压缩。
///
/// # 设计决策(WHY)
/// - `token_size` 由调用方指定:Week 3 阶段用简单估算(如 content.len() / 4),
///   Week 6 NMC 接入后由 tokenizer 精确计算
/// - `clv: Option<CLV>`:无 CLV 时相关性取中性值 0.5,避免阻塞压缩流程
/// - `access_count` 与 `last_accessed_at`:重要性评分的频次与时近性维度
/// - `content: Arc<str>`(SubTask 13.6):大字段 Arc 共享,克隆仅增加引用计数,
///   避免压缩/快照场景下大字符串的深拷贝(原 `String` 克隆 O(n) → `Arc<str>` 克隆 O(1))
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ContextEntry {
    /// 条目唯一标识
    pub id: String,
    /// 所属文件 ID(用于 OSA context_mask 稀疏化,仅保留活跃 file_id 的条目)
    pub file_id: String,
    /// 上下文内容(文本)— `Arc<str>` 共享,克隆廉价(引用计数)
    ///
    /// WHY(SubTask 13.6):content 可能很大(数 KB),压缩/快照场景需克隆条目,
    /// `String` 克隆 O(n) 深拷贝,`Arc<str>` 克隆 O(1) 引用计数。
    /// serde 对 `Arc<str>` 的序列化与 `String` 兼容(都序列化为 JSON 字符串)
    pub content: Arc<str>,
    /// Token 数量(用于容量计算与压缩目标)
    pub token_size: usize,
    /// 访问次数(重要性评分的频次维度,0.3 权重)
    pub access_count: u32,
    /// 最后访问时间(重要性评分的时近性维度,0.4 权重)
    pub last_accessed_at: DateTime<Utc>,
    /// 创建时间(用于时近性归一化的时间跨度计算)
    pub created_at: DateTime<Utc>,
    /// 上下文潜在向量(重要性评分的任务相关性维度,0.3 权重,基于 CLV 余弦相似度)
    ///
    /// WHY:Option 而非直接 CLV — 部分上下文(如系统提示)无语义向量,
    /// 无 CLV 时相关性取中性值 0.5,避免阻塞压缩流程
    pub clv: Option<CLV>,
}

impl ContextEntry {
    /// 创建新上下文条目
    ///
    /// # 参数
    /// - `id`:条目唯一标识
    /// - `file_id`:所属文件 ID(用于 OSA 掩码稀疏化)
    /// - `content`:上下文内容文本(将转为 `Arc<str>` 共享)
    /// - `token_size`:Token 数量(由调用方估算或精确计算)
    pub fn new(
        id: impl Into<String>,
        file_id: impl Into<String>,
        content: impl Into<String>,
        token_size: usize,
    ) -> Self {
        let now = Utc::now();
        Self {
            id: id.into(),
            file_id: file_id.into(),
            // WHY(SubTask 13.6):Arc::from(String) 转移堆内存所有权,无额外拷贝
            content: Arc::from(content.into()),
            token_size,
            access_count: 0,
            last_accessed_at: now,
            created_at: now,
            clv: None,
        }
    }

    /// 设置 CLV(链式调用)
    pub fn with_clv(mut self, clv: CLV) -> Self {
        self.clv = Some(clv);
        self
    }

    /// 更新最后访问时间为当前(用于 LRU 语义)
    pub fn touch(&mut self) {
        self.last_accessed_at = Utc::now();
    }

    /// 递增访问次数并更新访问时间
    pub fn increment_access(&mut self) {
        self.access_count = self.access_count.saturating_add(1);
        self.touch();
    }
}

/// HCW 内部状态 — 受 `RwLock<HcwState>` 保护
///
/// 包含当前窗口层级、上下文条目列表、最近接收的 OSA 掩码信息。
/// 所有字段在 HcwWindow 的 async 方法中通过 RwLock 读写。
///
/// # 设计决策(WHY)
/// - `entries: Vec<Arc<ContextEntry>>`(M-01/M-02 修复):按插入顺序存储,
///   压缩时按重要性评分排序保留 Top-N。未使用 DashMap 是因为压缩需要全量排序,
///   DashMap 的分片锁不利于全量操作。
///   WHY 用 `Vec<Arc<ContextEntry>>` 而非 `Vec<ContextEntry>`(M-01/M-02 热路径深拷贝优化):
///   原实现 `get_arc` 内部 `Arc::new(entry.clone())` 等价于先深拷贝再包 Arc,
///   多消费者场景下 content String 被反复深拷贝。改为 `Vec<Arc<ContextEntry>>` 后:
///   - `get_arc` 返回 `Arc::clone(&entries[idx])`(O(1) 引用计数,真零拷贝)
///   - `get_ref` 返回 `&Arc<ContextEntry>`(完全零拷贝引用访问)
///   - `get_mut` 内部用 `Arc::make_mut`(CoW,无外部引用时零分配)
///   - `remove` 返回 `Arc<ContextEntry>`(直接移交所有权,无需 clone)
///
///   Arc<T> 实现 Deref<Target=T>,原有 `e.token_size`、`e.id` 等字段访问代码无需改动。
/// - `entries_index: HashMap<String, usize>`(SubTask 19.5):id → entries 索引的 HashMap,
///   使 `get`/`get_mut`/`remove` 从 O(n) 线性扫描降为 O(1) 哈希查找。
///   1000 条目规模下 get 延迟从 ~15μs 降到 ~0.1μs。
///   WHY 用 HashMap 而非在 entries 中二分查找:条目无序(压缩后按重要性重排),
///   二分查找需先排序 O(n log n),HashMap 直接 O(1) 查找。
///   索引一致性:每次 entries 结构性变更(push/remove/retain/替换)后同步更新索引。
/// - `last_mask_hash`/`last_sparsity`:记录最近接收的 OSA 掩码信息,
///   用于监控与调试,实际稀疏化通过 `apply_sparse_mask` 显式触发
/// - `pending_context_mask: Option<Vec<String>>`(SubTask 17.1):OSA→HCW 事件驱动稀疏化链路的
///   桥接字段。listener 收到 `OmniSparseMasksComputed` 事件后将 `context_mask` 存入此字段,
///   随后由 listener(立即)或 insert/select_window(惰性兜底)调用 `apply_sparse_mask` 消费。
///   WHY 用 Option:事件未携带 context_mask 时为 None,避免空 Vec 误触发稀疏化
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HcwState {
    /// 当前窗口层级
    pub current_tier: WindowTier,
    /// 上下文条目列表(按插入顺序)— Arc 共享,避免热路径深拷贝(M-01/M-02 修复)
    ///
    /// WHY(M-01/M-02):`get_arc`/`get` 是热路径,原 `Vec<ContextEntry>` 导致:
    /// - get_arc 内部 `Arc::new(entry.clone())` 等价先深拷贝再包 Arc
    /// - get 返回 `entry.clone()` 深拷贝 content String
    ///
    /// 改为 `Vec<Arc<ContextEntry>>` 后,get_arc 返回 `Arc::clone`(O(1) 引用计数),
    /// get_ref 返回 `&Arc`(完全零拷贝),get_mut 用 `Arc::make_mut`(CoW)。
    pub entries: Vec<Arc<ContextEntry>>,
    /// 条目 ID → entries 索引的 HashMap(SubTask 19.5:O(1) 查找替代 O(n) 扫描)
    ///
    /// WHY(SubTask 19.5):1000 条目 get 从 ~15μs 降到 ~0.1μs。
    /// 索引一致性:每次 entries 结构性变更后通过 `rebuild_index` 或增量更新维护。
    /// 序列化包含此字段(冗余但简单),反序列化后索引与 entries 一致。
    pub entries_index: HashMap<String, usize>,
    /// 最近接收的 OSA 掩码哈希(用于去重与监控)
    pub last_mask_hash: Option<String>,
    /// 最近接收的 OSA 稀疏度(用于监控与稀疏化决策)
    pub last_sparsity: Option<f32>,
    /// 待应用的 OSA context_mask(活跃文件 ID 列表)
    ///
    /// WHY(SubTask 17.1):listener 收到 `OmniSparseMasksComputed` 事件后存入此字段,
    /// 随后由 `apply_pending_mask` 消费(取走并设为 None)。
    /// 取走而非读取:确保同一掩码仅应用一次,避免重复稀疏化
    pub pending_context_mask: Option<Vec<String>>,
    // === PROBE P1.5: repr_clv 写时预计算缓存 ===
    /// Block ID → 块代表向量(CLV)缓存 — 写入即缓存,查询期零嵌入计算
    ///
    /// WHY(设计文档 §4.2.4): 打分从 O(语料×嵌入) 降为 O(语料×512 维点积)。
    /// - `#[serde(skip)]`: 缓存是运行时派生数据,反序列化后为空可惰性重建,
    ///   避免 Arc<CLV> 序列化冗余与 PartialEq 派生含缓存导致相等性误判
    /// - `Arc<CLV>` 共享: 256 块 ≈512KB / L3 5000 块 ≈10MB,引用计数防复制放大
    /// - 失效策略: 任何 entries 结构性变更调用 [`HcwState::invalidate_repr_clv_cache`]
    ///   (保守全清,避免逐点失效遗漏——R9 缓存一致性红线)
    #[serde(skip)]
    pub repr_clv_cache: HashMap<String, Arc<CLV>>,
}

impl PartialEq for HcwState {
    /// 手动实现 PartialEq — 忽略 `repr_clv_cache`（运行时派生冗余）
    ///
    /// WHY 不派生: derive 会把缓存字段纳入相等性判定,两个 entries 相同但
    /// 缓存状态不同的 state 会被误判不等（如反序列化后缓存为空 vs 热路径缓存全）;
    /// 忽略缓存后与 P1.5 之前的派生行为完全一致（既有测试零回归）
    fn eq(&self, other: &Self) -> bool {
        self.current_tier == other.current_tier
            && self.entries == other.entries
            && self.entries_index == other.entries_index
            && self.last_mask_hash == other.last_mask_hash
            && self.last_sparsity == other.last_sparsity
            && self.pending_context_mask == other.pending_context_mask
    }
}

impl HcwState {
    /// 创建新状态,指定初始层级
    pub fn new(tier: WindowTier) -> Self {
        Self {
            current_tier: tier,
            entries: Vec::new(),
            // SubTask 19.5:初始化空索引,后续 push_entry/rebuild_index 维护
            entries_index: HashMap::new(),
            last_mask_hash: None,
            last_sparsity: None,
            pending_context_mask: None,
            // PROBE P1.5: 空缓存初始化
            repr_clv_cache: HashMap::new(),
        }
    }

    /// 使 repr_clv 缓存整体失效（entries 结构性变更后调用，保守全清）
    ///
    /// WHY 全清而非逐点失效: entries 变更点分散（remove/retain/compress/掩码应用），
    /// 逐点维护易遗漏导致陈旧向量污染打分（R9）；缓存重建成本 O(N) 点积,
    /// 惰性重算不阻塞装窗（速度红线）,全清是正确性优先的保守策略
    pub fn invalidate_repr_clv_cache(&mut self) {
        self.repr_clv_cache.clear();
    }

    /// 更新单条 repr_clv 缓存（insert/更新路径调用）
    ///
    /// # 参数
    /// - `id`: 条目 ID
    /// - `clv`: 条目 CLV（`None` 时移除缓存——无向量条目不缓存）
    pub fn update_repr_clv_cache(&mut self, id: &str, clv: Option<&CLV>) {
        match clv {
            Some(v) => {
                self.repr_clv_cache
                    .insert(id.to_string(), Arc::new(v.clone()));
            }
            None => {
                self.repr_clv_cache.remove(id);
            }
        }
    }

    /// 读取 repr_clv 缓存（查询路径，O(1)）
    ///
    /// # 返回
    /// 缓存命中时返回 CLV 引用；miss 返回 None（调用方走惰性重算）
    pub fn repr_clv(&self, id: &str) -> Option<&CLV> {
        self.repr_clv_cache.get(id).map(|v| v.as_ref())
    }

    /// 计算所有条目的总 Token 大小
    pub fn total_size(&self) -> usize {
        self.entries.iter().map(|e| e.token_size).sum()
    }

    /// 全量重建 entries_index(id → entries 索引)
    ///
    /// WHY(SubTask 19.5):entries 发生结构性变更(retain/全量替换/批量删除)后,
    /// 索引可能失效(索引指向的位置已不是原条目)。此时全量重建最简单且正确。
    /// 复杂度 O(n),仅在结构性变更时调用,不影响单次 get/remove 的 O(1) 性能。
    pub fn rebuild_index(&mut self) {
        self.entries_index.clear();
        self.entries_index.reserve(self.entries.len());
        for (i, e) in self.entries.iter().enumerate() {
            self.entries_index.insert(e.id.clone(), i);
        }
    }

    /// 追加条目并同步更新索引(O(1) 增量更新)
    ///
    /// WHY(SubTask 19.5):封装 push + index 更新,避免调用方直接操作 entries
    /// 后忘记维护索引。insert 路径应统一使用此方法。
    ///
    /// WHY(M-01/M-02):内部用 `Arc::new(entry)` 包装,使后续 `get_arc`/`get_ref`
    /// 能通过 `Arc::clone` 以 O(1) 引用计数共享条目,避免热路径深拷贝。
    pub fn push_entry(&mut self, entry: ContextEntry) {
        let idx = self.entries.len();
        // PROBE P1.5: 写入即缓存（entry 有 CLV 时；None 时移除旧缓存）
        let entry_clv = entry.clv.clone();
        let id = entry.id.clone();
        self.entries_index.insert(id.clone(), idx);
        self.entries.push(Arc::new(entry));
        self.update_repr_clv_cache(&id, entry_clv.as_ref());
    }

    /// 按 ID 查找条目(只读)— O(1) HashMap 索引查找
    ///
    /// WHY(SubTask 19.5):原实现 `iter().find()` 为 O(n) 线性扫描,
    /// 1000 条目规模约 15μs。改为 HashMap 索引查找 O(1),约 0.1μs。
    ///
    /// WHY(M-01/M-02):entries 是 `Vec<Arc<ContextEntry>>`,`.get(pos)` 返回
    /// `Option<&Arc<ContextEntry>>`,通过 `.map(|arc| arc.as_ref())` 解引用为
    /// `&ContextEntry`,保持原签名兼容。调用方需 Arc 所有权时用 `get_ref`。
    pub fn get(&self, id: &str) -> Option<&ContextEntry> {
        // *解引用 usize(Copy 类型),borrow 立即结束,可接着借用 entries
        let pos = *self.entries_index.get(id)?;
        self.entries.get(pos).map(|arc| arc.as_ref())
    }

    /// 按 ID 查找条目,返回 `&Arc<ContextEntry>`(零拷贝引用访问)
    ///
    /// WHY(M-01/M-02 热路径深拷贝优化):与 `get` 的区别 —
    /// `get` 返回 `&ContextEntry`(通过 Deref,无法直接获取 Arc 所有权);
    /// `get_ref` 返回 `&Arc<ContextEntry>`,调用方可通过 `Arc::clone(arc_ref)`
    /// 以 O(1) 引用计数获取共享所有权,避免深拷贝 content String。
    /// 适用于多消费者共享场景(如 PVL 并行验证同一上下文)。
    pub fn get_ref(&self, id: &str) -> Option<&Arc<ContextEntry>> {
        let pos = *self.entries_index.get(id)?;
        self.entries.get(pos)
    }

    /// 按 ID 查找条目(可变)— O(1) HashMap 索引查找
    ///
    /// WHY(SubTask 19.5):同 get,借用分离 — entries_index 的借用随 usize 复制结束,
    /// 随后可变借用 entries 不冲突。
    ///
    /// WHY(M-01/M-02):entries 是 `Vec<Arc<ContextEntry>>`,`.get_mut(pos)` 返回
    /// `Option<&mut Arc<ContextEntry>>`,用 `Arc::make_mut` 获取 `&mut ContextEntry`。
    /// CoW 语义:无外部 Arc 引用时零分配(直接返回可变引用);有外部引用时深拷贝一份,
    /// 保证外部 Arc 不被意外修改。在 HcwWindow 内部 entries 是唯一所有者时,
    /// `Arc::make_mut` 退化为 O(1)。
    pub fn get_mut(&mut self, id: &str) -> Option<&mut ContextEntry> {
        let pos = *self.entries_index.get(id)?;
        self.entries.get_mut(pos).map(Arc::make_mut)
    }

    /// 按 ID 移除条目 — O(1) swap_remove + 索引更新
    ///
    /// WHY(SubTask 19.5):原实现 `iter().position()` + `remove(pos)` 为 O(n)
    /// (position 线性扫描 + remove 后移位)。改为:
    /// 1. HashMap O(1) 查找 pos
    /// 2. `swap_remove` O(1) 删除(将末尾元素移到 pos)
    /// 3. 更新被移动元素的索引(仅 1 次插入)
    ///
    /// 注意:swap_remove 改变元素顺序,但 HCW 的 entries 顺序无语义
    /// (压缩按重要性评分排序,不依赖插入顺序)。
    ///
    /// WHY(M-01/M-02):返回 `Arc<ContextEntry>` 而非 `ContextEntry`,
    /// 直接移交所有权(无需 clone)。调用方需 `ContextEntry` 所有权时
    /// 可 `(*arc).clone()` 或 `Arc::try_unwrap(arc).unwrap_or_else(|a| (*a).clone())`。
    pub fn remove(&mut self, id: &str) -> Option<Arc<ContextEntry>> {
        let pos = *self.entries_index.get(id)?;
        // swap_remove:O(1) 删除,将末尾元素移到 pos 位置
        // WHY(M-01/M-02):Vec<Arc<ContextEntry>> 的 swap_remove 直接返回 Arc,零拷贝
        let removed = self.entries.swap_remove(pos);
        // 从索引中移除被删除的条目
        self.entries_index.remove(id);
        // 若 swap_remove 移动了末尾元素(pos 不是原末尾位置),
        // 需更新被移动元素的索引指向新位置 pos
        if pos < self.entries.len() {
            let moved_id = self.entries[pos].id.clone();
            self.entries_index.insert(moved_id, pos);
        }
        // PROBE P1.5: 删除条目后失效其缓存（保守移除单条）
        self.repr_clv_cache.remove(id);
        Some(removed)
    }

    /// 仅保留 file_id 在活跃列表中的条目,返回移除数量
    ///
    /// WHY:OSA context_mask 稀疏化的核心操作 — 仅加载活跃文件上下文,
    /// 其余稀疏化跳过,验证 1M 等效不通过暴力加载(架构红线)
    ///
    /// # 性能优化(SubTask 13.8)
    /// 原实现 `active_file_ids.iter().any(|f| f == &e.file_id)` 为 O(n×m),
    /// 1000 文件 × 10000 条目需 10⁷ 次比较。改为先将 `file_ids` 转为 `HashSet`,
    /// O(1) 查找,总复杂度降为 O(n + m),1000×10000 场景延迟 < 5ms(原约 50ms)
    ///
    /// # 索引维护(SubTask 19.5)
    /// retain 后条目位置变化,全量重建索引确保一致性。
    pub fn retain_by_file_ids(&mut self, active_file_ids: &[String]) -> usize {
        let original_count = self.entries.len();
        // WHY(SubTask 13.8):HashSet 构建 O(m),查找 O(1),避免 Vec 线性扫描 O(m)
        let active_set: HashSet<&String> = active_file_ids.iter().collect();
        self.entries.retain(|e| active_set.contains(&e.file_id));
        let removed = original_count - self.entries.len();
        // SubTask 19.5:retain 后索引失效,全量重建
        self.rebuild_index();
        // PROBE P1.5: 结构性变更后保守全清缓存（R9 一致性红线）
        self.invalidate_repr_clv_cache();
        removed
    }
}

/// 压缩报告 — 记录压缩前后的容量与条目变化
///
/// 由 `ContextCompressor::compress` 与 `HcwWindow::apply_sparse_mask` 返回,
/// 用于监控压缩效果与发布 `ContextCompressed` 事件。
///
/// # compression_ratio 定义(SubTask 14.6 命名澄清)
/// `compression_ratio = original_size / compressed_size`(压缩比,> 1.0,越大压缩越多)。
/// 任务要求"压缩率 > 3×"即 compression_ratio > 3.0(100K → 32K,ratio = 3.125)。
///
/// ## 边界处理
/// - `compressed_size == 0`(全部稀疏化):`compression_ratio = f32::MAX`
///   WHY:用 `f32::MAX` 而非 `f32::INFINITY`,因为 `INFINITY` 在 serde_json 序列化时
///   会输出 `null`(非标准 JSON),导致反序列化失败。`f32::MAX` 是有限值,可安全序列化。
/// - `original_size == 0 && compressed_size == 0`(无数据):`compression_ratio = 1.0`(无压缩)
///
/// ## 与事件 payload ratio 的区别
/// 发布 `ContextCompressed` 事件时,事件 payload 的 `ratio = compressed/original ∈ [0, 1]`,
/// 方向与本字段相反(事件 ratio 越小压缩越多,本字段越大压缩越多)。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CompressionReport {
    /// 压缩前总 Token 大小
    pub original_size: usize,
    /// 压缩后总 Token 大小
    pub compressed_size: usize,
    /// 压缩比 = original_size / compressed_size(> 1.0,越大压缩越多)
    ///
    /// WHY(SubTask 14.6):原字段名 `ratio` 语义模糊(压缩率 vs 压缩比),
    /// 重命名为 `compression_ratio` 明确表示"压缩比"(original/compressed)。
    /// `compressed_size == 0` 时返回 `f32::MAX`(非 INFINITY,避免序列化失败)
    pub compression_ratio: f32,
    /// 压缩前条目数
    pub original_count: usize,
    /// 压缩后保留条目数
    pub retained_count: usize,
    /// 丢弃条目数
    pub dropped_count: usize,
    /// 保留的条目列表(用于调用方替换原始 entries)— Arc 共享,避免压缩路径深拷贝
    ///
    /// WHY(M-01/M-02):改为 `Vec<Arc<ContextEntry>>` 后,compressor 内部
    /// 从 `&[Arc<ContextEntry>]` 借用,retained 用 `Arc::clone` 推入(零拷贝),
    /// 调用方 `state.entries = report.retained_entries` 也是零拷贝移动赋值。
    pub retained_entries: Vec<Arc<ContextEntry>>,
    /// 压缩算法名称(如 "importance-top-n"、"sparse-mask")
    pub algorithm: String,
}

/// HCW 配置 — 四级窗口容量与压缩阈值
///
/// 结构体定义在此,impl 块(Default/new/builder/validate)在 `config.rs`。
///
/// # 默认值(对应架构手册 §HCW 四级窗口)
/// - `l0_capacity`:4096(4K Token,快速响应)
/// - `l1_capacity`:32768(32K Token,常规任务)
/// - `l2_capacity`:131072(128K Token,复杂任务)
/// - `l3_capacity`:1048576(1M Token 等效,128K 实际加载 + 8× 稀疏化)
/// - `compression_threshold`:0.9(容量利用率达 90% 触发压缩,留 10% 余量)
/// - `selector_policy`:`Static(0.4, 0.3, 0.3)`(P3-W10.3 D1 修复,默认 fallback 编译进二进制)
/// - `parallel_compress`:true(P1-T14 压缩评分段间并行,默认开启)
///
/// # D1 修复(P3-W10.3)
/// `selector_policy` 字段取代原 `compressor_weights: (f32, f32, f32)` 常量,
/// 将重要性评分权重 `w1/w2/w3` 从硬编码升级为注入式 `SelectorPolicy` 策略:
/// - **Static 变体**(默认):编译进二进制的常量(fallback,C4 合规)
/// - **Learned 变体**:`omega-learner` 异步下发的版本化权重(P4 Bandit 接缝 S4)
///
/// `SelectorPolicy::default()` = `Static(SelectorWeights::DEFAULT)` = `Static(0.4, 0.3, 0.3)`,
/// 等于原 `compressor_weights` 默认值,行为零变化(仅字段名变更,向后兼容)。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HcwConfig {
    /// L0 窗口容量(默认 4096 = 4K Token)
    pub l0_capacity: usize,
    /// L1 窗口容量(默认 32768 = 32K Token)
    pub l1_capacity: usize,
    /// L2 窗口容量(默认 131072 = 128K Token)
    pub l2_capacity: usize,
    /// L3 窗口等效容量(默认 1048576 = 1M Token,实际加载 128K)
    pub l3_capacity: usize,
    /// 压缩触发阈值(默认 0.9,容量利用率达 90% 触发压缩)
    pub compression_threshold: f32,
    /// 选择器策略 — 重要性评分权重(recency/frequency/relevance)的注入式策略(P3-W10.3 D1 修复)
    ///
    /// 取代原 `compressor_weights: (f32, f32, f32)` 常量,将 `w1/w2/w3` 从硬编码
    /// 升级为可注入策略。默认 `SelectorPolicy::default()` = `Static(0.4, 0.3, 0.3)`,
    /// 等于原常量值(fallback 编译进二进制,C4 合规)。
    ///
    /// WHY(C4 合规):`omega-learner` panic/超时时调用方本地 fallback 到
    /// `SelectorPolicy::Static(常量)`,无跨 crate 旗标传播(spec.md:289-290)。
    ///
    /// WHY(serde default):用 `#[serde(default)]` 标注,旧配置文件(无此字段)
    /// 反序列化时自动用 `SelectorPolicy::default()`,向后兼容。
    #[serde(default)]
    pub selector_policy: SelectorPolicy,
    /// P1-T14: 压缩并行开关(默认 true)
    ///
    /// 经 `nexus_core::compute::ComputeBridge`(L-a 全局 rayon 池)在压缩评分阶段
    /// **段间并行**(段内保序,段间按序拼接,结果与串行逐元素一致)
    /// (v4.0 §7.5.1 L-a: 四层级窗口选择 / 压缩段间 2-3×【待验证】)。
    /// 关闭方式(强制串行回退):配置 false 或环境变量 `CHIMERA_NO_PARALLEL_HCW`
    /// 设置为 "1"/"true"/"on"(启动期 OnceLock 一次读取,不在热路径)。
    ///
    /// WHY 默认 true:并行收益为正的批量场景(> CscCollapseScore 阈值 200)才触发
    /// rayon,小批量走 `DispatchPlan::Inline` 串行,零开销开关默认开启安全。
    ///
    /// WHY(serde default):与 `selector_policy` 同策略——旧配置文件(无此字段)
    /// 反序列化时自动用默认值 true,向后兼容。
    #[serde(default = "default_true")]
    pub parallel_compress: bool,
}

/// serde 默认值 — P1-T14 并行开关默认开启(与 faae/nmc 注入模式一致)
pub(crate) fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造带 CLV 的条目（SplitMix64 确定性，非零）
    fn entry_with_clv(id: &str, seed: u64) -> ContextEntry {
        let v: Vec<f32> = (0..512)
            .map(|j| {
                let mut z = seed.wrapping_add((j as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
                z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
                z ^= z >> 31;
                ((z >> 11) as f32) / (1u64 << 53) as f32 * 2.0 - 1.0
            })
            .collect();
        let clv = CLV::from_vec(v).expect("512 dims");
        ContextEntry::new(id, id, format!("content-{id}"), 100).with_clv(clv)
    }

    // ============================================================
    // PROBE P1.5: repr_clv 缓存测试
    // ============================================================

    #[test]
    fn test_push_entry_caches_clv() {
        // 写入即缓存：带 CLV 的条目 push 后缓存命中
        let mut state = HcwState::new(WindowTier::L0);
        state.push_entry(entry_with_clv("a", 1));
        assert!(state.repr_clv("a").is_some(), "带 CLV 条目应写入缓存");
        // 缓存值 = 条目 CLV
        assert_eq!(
            state.repr_clv("a"),
            state.get("a").and_then(|e| e.clv.as_ref())
        );
    }

    #[test]
    fn test_push_entry_without_clv_not_cached() {
        // 无 CLV 条目不缓存（中性值路径）
        let mut state = HcwState::new(WindowTier::L0);
        state.push_entry(ContextEntry::new("b", "b", "content-b", 100));
        assert!(state.repr_clv("b").is_none(), "无 CLV 条目不应缓存");
    }

    #[test]
    fn test_remove_invalidates_cache() {
        // 删除条目后缓存失效（单条移除）
        let mut state = HcwState::new(WindowTier::L0);
        state.push_entry(entry_with_clv("a", 1));
        state.push_entry(entry_with_clv("c", 2));
        assert!(state.repr_clv("a").is_some());
        let removed = state.remove("a");
        assert!(removed.is_some());
        assert!(state.repr_clv("a").is_none(), "remove 后缓存应失效");
        // 未删除条目缓存保留
        assert!(state.repr_clv("c").is_some());
    }

    #[test]
    fn test_retain_invalidates_all_cache() {
        // 结构性变更（retain）后保守全清
        let mut state = HcwState::new(WindowTier::L0);
        state.push_entry(entry_with_clv("a", 1));
        state.push_entry(entry_with_clv("c", 2));
        state.retain_by_file_ids(&["c".to_string()]);
        assert!(state.repr_clv("a").is_none(), "retain 后缓存应全清");
        assert!(
            state.repr_clv("c").is_none(),
            "retain 后缓存应全清（惰性重建）"
        );
        // entries 正确保留
        assert_eq!(state.entries.len(), 1);
        assert_eq!(state.entries[0].id, "c");
    }

    #[test]
    fn test_partial_eq_ignores_cache() {
        // 手动 PartialEq 忽略缓存：同 entries 不同缓存状态相等
        let mut s1 = HcwState::new(WindowTier::L0);
        let mut s2 = HcwState::new(WindowTier::L0);
        // 共享同一 entry 构造（时间戳一致，entries 才能相等）
        let e = entry_with_clv("a", 1);
        s1.push_entry(e.clone());
        s2.push_entry(e);
        // s2 缓存手动清除（模拟反序列化后空缓存）
        s2.invalidate_repr_clv_cache();
        assert!(s1.repr_clv("a").is_some());
        assert!(s2.repr_clv("a").is_none());
        assert_eq!(s1, s2, "缓存状态不应影响 HcwState 相等性");
    }

    #[test]
    fn test_window_tier_as_str() {
        assert_eq!(WindowTier::L0.as_str(), "L0");
        assert_eq!(WindowTier::L1.as_str(), "L1");
        assert_eq!(WindowTier::L2.as_str(), "L2");
        assert_eq!(WindowTier::L3.as_str(), "L3");
    }

    #[test]
    fn test_window_tier_parse_tier() {
        assert_eq!(WindowTier::parse_tier("L0"), Some(WindowTier::L0));
        assert_eq!(WindowTier::parse_tier("L3"), Some(WindowTier::L3));
        assert_eq!(WindowTier::parse_tier("L4"), None);
    }

    #[test]
    fn test_window_tier_upgrade() {
        assert_eq!(WindowTier::L0.upgrade(), Some(WindowTier::L1));
        assert_eq!(WindowTier::L1.upgrade(), Some(WindowTier::L2));
        assert_eq!(WindowTier::L2.upgrade(), Some(WindowTier::L3));
        assert_eq!(WindowTier::L3.upgrade(), None);
    }

    #[test]
    fn test_window_tier_downgrade() {
        assert_eq!(WindowTier::L0.downgrade(), None);
        assert_eq!(WindowTier::L1.downgrade(), Some(WindowTier::L0));
        assert_eq!(WindowTier::L3.downgrade(), Some(WindowTier::L2));
    }

    #[test]
    fn test_context_entry_new() {
        let entry = ContextEntry::new("e-1", "file-1", "content", 100);
        assert_eq!(entry.id, "e-1");
        assert_eq!(entry.file_id, "file-1");
        assert_eq!(entry.token_size, 100);
        assert_eq!(entry.access_count, 0);
        assert!(entry.clv.is_none());
    }

    #[test]
    fn test_context_entry_increment_access() {
        let mut entry = ContextEntry::new("e-1", "file-1", "content", 100);
        let original_time = entry.last_accessed_at;
        entry.increment_access();
        assert_eq!(entry.access_count, 1);
        assert!(entry.last_accessed_at >= original_time);
    }

    #[test]
    fn test_hcw_state_total_size() {
        let mut state = HcwState::new(WindowTier::L0);
        // SubTask 19.5:用 push_entry 替代 entries.push,同步维护索引
        state.push_entry(ContextEntry::new("e-1", "f-1", "a", 100));
        state.push_entry(ContextEntry::new("e-2", "f-2", "b", 200));
        assert_eq!(state.total_size(), 300);
    }

    #[test]
    fn test_hcw_state_retain_by_file_ids() {
        let mut state = HcwState::new(WindowTier::L0);
        state.push_entry(ContextEntry::new("e-1", "f-1", "a", 100));
        state.push_entry(ContextEntry::new("e-2", "f-2", "b", 200));
        state.push_entry(ContextEntry::new("e-3", "f-3", "c", 300));

        let removed = state.retain_by_file_ids(&["f-1".into(), "f-3".into()]);
        assert_eq!(removed, 1);
        assert_eq!(state.entries.len(), 2);
        assert_eq!(state.entries[0].id, "e-1");
        assert_eq!(state.entries[1].id, "e-3");
    }

    #[test]
    fn test_hcw_state_get_via_index() {
        // SubTask 19.5:验证 HashMap 索引查找正确性
        let mut state = HcwState::new(WindowTier::L0);
        state.push_entry(ContextEntry::new("e-1", "f-1", "a", 100));
        state.push_entry(ContextEntry::new("e-2", "f-2", "b", 200));
        state.push_entry(ContextEntry::new("e-3", "f-3", "c", 300));

        // get 返回正确条目
        assert_eq!(state.get("e-2").unwrap().token_size, 200);
        assert_eq!(state.get("e-3").unwrap().id, "e-3");
        // 不存在的 id 返回 None
        assert!(state.get("nonexistent").is_none());
    }

    #[test]
    fn test_hcw_state_get_mut_via_index() {
        // SubTask 19.5:验证可变借用索引查找正确性
        let mut state = HcwState::new(WindowTier::L0);
        state.push_entry(ContextEntry::new("e-1", "f-1", "a", 100));

        state.get_mut("e-1").unwrap().token_size = 500;
        assert_eq!(state.get("e-1").unwrap().token_size, 500);
    }

    #[test]
    fn test_hcw_state_remove_via_index() {
        // SubTask 19.5:验证 swap_remove + 索引更新正确性
        let mut state = HcwState::new(WindowTier::L0);
        state.push_entry(ContextEntry::new("e-1", "f-1", "a", 100));
        state.push_entry(ContextEntry::new("e-2", "f-2", "b", 200));
        state.push_entry(ContextEntry::new("e-3", "f-3", "c", 300));

        // 删除中间元素 e-2(swap_remove 会将 e-3 移到 e-2 的位置)
        let removed = state.remove("e-2").unwrap();
        assert_eq!(removed.id, "e-2");
        assert_eq!(state.entries.len(), 2);

        // 验证被移动元素的索引已更新:e-3 现在应在 pos=1
        assert!(state.get("e-1").is_some(), "e-1 仍应可查");
        assert!(state.get("e-3").is_some(), "e-3 仍应可查(索引已更新)");
        assert!(state.get("e-2").is_none(), "e-2 已删除,应返回 None");

        // 删除不存在的 id 返回 None
        assert!(state.remove("nonexistent").is_none());
    }

    #[test]
    fn test_hcw_state_remove_last_element() {
        // SubTask 19.5:验证删除末尾元素(无 swap 移动)的正确性
        let mut state = HcwState::new(WindowTier::L0);
        state.push_entry(ContextEntry::new("e-1", "f-1", "a", 100));
        state.push_entry(ContextEntry::new("e-2", "f-2", "b", 200));

        // 删除末尾元素 e-2(无元素被移动)
        let removed = state.remove("e-2").unwrap();
        assert_eq!(removed.id, "e-2");
        assert_eq!(state.entries.len(), 1);
        assert!(state.get("e-1").is_some());
        assert!(state.get("e-2").is_none());
    }

    #[test]
    fn test_hcw_state_rebuild_index() {
        // SubTask 19.5:验证全量重建索引的正确性
        let mut state = HcwState::new(WindowTier::L0);
        state.push_entry(ContextEntry::new("e-1", "f-1", "a", 100));
        state.push_entry(ContextEntry::new("e-2", "f-2", "b", 200));

        // 模拟索引失效:直接清空索引
        state.entries_index.clear();
        assert!(state.get("e-1").is_none(), "索引清空后应查不到");

        // 重建索引后应能正常查找
        state.rebuild_index();
        assert!(state.get("e-1").is_some());
        assert!(state.get("e-2").is_some());
    }
}
