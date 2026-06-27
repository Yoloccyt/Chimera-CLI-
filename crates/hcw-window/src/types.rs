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

    /// 返回该层级的实际加载容量(Token 数)
    ///
    /// WHY:L3 的实际加载容量 = l3_capacity / 8 = 128K,
    /// 通过 OSA 稀疏化(8× 压缩比)实现 1M 等效,避免暴力加载(架构红线)。
    /// L0/L1/L2 的实际容量 = 标称容量(无稀疏化)
    pub fn effective_capacity(self, config: &HcwConfig) -> usize {
        match self {
            Self::L0 => config.l0_capacity,
            Self::L1 => config.l1_capacity,
            Self::L2 => config.l2_capacity,
            // L3:1M 等效,128K 实际加载 + 8× 稀疏化压缩比
            Self::L3 => config.l3_capacity / 8,
        }
    }
}

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
/// - `entries: Vec<ContextEntry>`:按插入顺序存储,压缩时按重要性评分排序保留 Top-N。
///   未使用 DashMap 是因为压缩需要全量排序,DashMap 的分片锁不利于全量操作
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
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HcwState {
    /// 当前窗口层级
    pub current_tier: WindowTier,
    /// 上下文条目列表(按插入顺序)
    pub entries: Vec<ContextEntry>,
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
        }
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
    pub fn push_entry(&mut self, entry: ContextEntry) {
        let idx = self.entries.len();
        self.entries_index.insert(entry.id.clone(), idx);
        self.entries.push(entry);
    }

    /// 按 ID 查找条目(只读)— O(1) HashMap 索引查找
    ///
    /// WHY(SubTask 19.5):原实现 `iter().find()` 为 O(n) 线性扫描,
    /// 1000 条目规模约 15μs。改为 HashMap 索引查找 O(1),约 0.1μs。
    pub fn get(&self, id: &str) -> Option<&ContextEntry> {
        // *解引用 usize(Copy 类型),borrow 立即结束,可接着借用 entries
        let pos = *self.entries_index.get(id)?;
        self.entries.get(pos)
    }

    /// 按 ID 查找条目(可变)— O(1) HashMap 索引查找
    ///
    /// WHY(SubTask 19.5):同 get,借用分离 — entries_index 的借用随 usize 复制结束,
    /// 随后可变借用 entries 不冲突。
    pub fn get_mut(&mut self, id: &str) -> Option<&mut ContextEntry> {
        let pos = *self.entries_index.get(id)?;
        self.entries.get_mut(pos)
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
    pub fn remove(&mut self, id: &str) -> Option<ContextEntry> {
        let pos = *self.entries_index.get(id)?;
        // swap_remove:O(1) 删除,将末尾元素移到 pos 位置
        let removed = self.entries.swap_remove(pos);
        // 从索引中移除被删除的条目
        self.entries_index.remove(id);
        // 若 swap_remove 移动了末尾元素(pos 不是原末尾位置),
        // 需更新被移动元素的索引指向新位置 pos
        if pos < self.entries.len() {
            let moved_id = self.entries[pos].id.clone();
            self.entries_index.insert(moved_id, pos);
        }
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
    /// 保留的条目列表(用于调用方替换原始 entries)
    pub retained_entries: Vec<ContextEntry>,
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
    /// 压缩器重要性评分权重 (recency, frequency, relevance)
    ///
    /// 三个权重之和应为 1.0。默认值 (0.4, 0.3, 0.3) 对应架构手册推荐。
    /// 调优示例:对时近性敏感的场景可设为 (0.6, 0.2, 0.2)。
    #[serde(default = "default_compressor_weights")]
    pub compressor_weights: (f32, f32, f32),
}

fn default_compressor_weights() -> (f32, f32, f32) {
    (0.4, 0.3, 0.3)
}

#[cfg(test)]
mod tests {
    use super::*;

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
