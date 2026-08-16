//! Repo Wiki 核心类型 — WikiEntry 与 WikiConfig
//!
//! 对应架构层:L5 Knowledge
//! 对应创新点:ISCM(Inter-Shared Cross Module,跨层共享索引)
//!
//! # 类型职责
//! - `WikiEntry`:Wiki 条目,含标题、内容、标签、嵌入向量、时间戳
//! - `WikiConfig`:Wiki 存储配置,含数据库路径、向量维度、WAL 开关

use chrono::{DateTime, Utc};
use nexus_contracts::{TemporalMeta, TransitionType};
use serde::{Deserialize, Serialize};

use crate::search::HybridSearchConfig;

/// Wiki 条目 — 知识沉淀的最小单元
///
/// `embedding` 支持两条生成路径(P1-1 接入 NMC):
/// - **占位哈希路径**(默认):SHA-256 扩展为 512-dim,与 CLV 对齐
/// - **NMC 语义路径**(`WikiGenerator::with_text_encoder`):ONNX 模型可用时
///   384 维(all-MiniLM-L6-v2),无模型时字节频率降级(text_dim 维)
///
/// 维度契约:占位路径固定 512;NMC 路径维度由感知器决定。
/// 使用 HNSW/混合检索时须保证 `WikiConfig.vector_dim` 与所选路径一致。
///
/// # P3-W11.2 D12 修复:时间有效性维度
///
/// `temporal_meta` 字段为 Wiki 条目附加时间有效性信息(`TemporalMeta`),
/// 用于解决 D12"幽灵记忆"病理——矛盾检测时旧条目被标记为 `Historical`(归档),
/// 而非删除,保留谱系完整性供时间感知召回。
///
/// - `None`(默认):视为 `Current` 状态(向后兼容,老条目无此字段)
/// - `Some(TemporalMeta)`:按 `transition_type` 区分 Current/Historical/Transition
///
/// WHY(P3-W11.2):`#[serde(default)]` 确保老数据(无此字段)反序列化为 `None`,
/// 不破坏现有持久化数据与测试。`None` 在矛盾检测时按 `Current` 处理(向后兼容)。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WikiEntry {
    /// 条目唯一标识(通常为 UUIDv7 字符串)
    pub entry_id: String,
    /// 条目标题(人类可读,建议 ≤ 100 字符)
    pub title: String,
    /// 条目内容(自然语言全文)
    pub content: String,
    /// 标签列表(用于分类与过滤)
    pub tags: Vec<String>,
    /// 嵌入向量(默认 512 维占位哈希;NMC 路径为感知器输出维度,见 `WikiGenerator`)
    pub embedding: Vec<f32>,
    /// 创建时间(UTC,自动生成)
    pub created_at: DateTime<Utc>,
    /// 最后更新时间(UTC,插入/更新时自动刷新)
    pub updated_at: DateTime<Utc>,
    /// 时间元数据 — 记忆的时间有效性信息(P3-W11.2 D12 修复)
    ///
    /// WHY(P3-W11.2 D12):矛盾检测时旧条目被标记为 `Historical`(归档,不删除),
    /// 新条目保持 `Current`。`None` 视为 `Current`(向后兼容)。
    ///
    /// WHY `#[serde(default)]`:确保老数据(无此字段)反序列化为 `None`,
    /// 不破坏现有持久化数据与测试。
    #[serde(default)]
    pub temporal_meta: Option<TemporalMeta>,
}

impl WikiEntry {
    /// 创建新条目,`created_at` 与 `updated_at` 自动设为当前 UTC 时间
    ///
    /// # P3-W11.2 D12 修复
    ///
    /// `temporal_meta` 默认为 `None`(向后兼容)。需要时间有效性时通过
    /// `with_temporal_meta` 链式设置。`None` 在矛盾检测时按 `Current` 处理。
    ///
    /// # 示例
    /// ```
    /// use repo_wiki::WikiEntry;
    /// let entry = WikiEntry::new("e-1", "标题", "内容", vec!["t".into()], vec![0.0; 512]);
    /// assert_eq!(entry.entry_id, "e-1");
    /// assert!(entry.is_current()); // 默认 Current(向后兼容)
    /// ```
    pub fn new(
        entry_id: impl Into<String>,
        title: impl Into<String>,
        content: impl Into<String>,
        tags: Vec<String>,
        embedding: Vec<f32>,
    ) -> Self {
        let now = Utc::now();
        Self {
            entry_id: entry_id.into(),
            title: title.into(),
            content: content.into(),
            tags,
            embedding,
            created_at: now,
            updated_at: now,
            temporal_meta: None, // P3-W11.2: 默认 None(向后兼容,视为 Current)
        }
    }

    /// 附带时间元数据(用于 P3-W11.2 D12 修复 — 矛盾检测标记过渡期)
    ///
    /// WHY(P3-W11.2 D12):为 Wiki 条目附加时间有效性(valid_from/valid_until)、
    /// 时间状态(transition_type)与置信度,使矛盾检测能标记旧条目为 Historical。
    ///
    /// # 示例
    /// ```
    /// use repo_wiki::WikiEntry;
    /// use nexus_contracts::TemporalMeta;
    ///
    /// let meta = TemporalMeta::new(1700000000, 0.9);
    /// let entry = WikiEntry::new("e-1", "标题", "内容", vec![], vec![0.0; 512])
    ///     .with_temporal_meta(meta);
    /// assert!(entry.is_current());
    /// ```
    pub fn with_temporal_meta(mut self, meta: TemporalMeta) -> Self {
        self.temporal_meta = Some(meta);
        self
    }

    /// 判断条目是否为 `Current` 状态(默认召回)
    ///
    /// WHY(P3-W11.2 D12):`temporal_meta` 为 `None` 时视为 `Current`(向后兼容),
    /// 避免老条目(无此字段)被误过滤。
    pub fn is_current(&self) -> bool {
        match &self.temporal_meta {
            None => true, // 向后兼容:无 temporal_meta 视为 Current
            Some(meta) => meta.transition_type.is_current(),
        }
    }

    /// 判断条目是否为 `Historical` 状态(已被矛盾检测归档)
    ///
    /// WHY(P3-W11.2 D12):矛盾检测标记旧条目为 Historical,默认召回跳过,
    /// 保留谱系完整性但不影响当前决策。
    pub fn is_historical(&self) -> bool {
        match &self.temporal_meta {
            None => false,
            Some(meta) => matches!(meta.transition_type, TransitionType::Historical),
        }
    }

    /// 标记条目为 `Historical` 状态(矛盾检测归档,INV-8 单调性)
    ///
    /// WHY(P3-W11.2 D12 + INV-8 单调性):`Current` → `Historical` 单向降级。
    /// 矛盾检测发现旧条目与新条目矛盾时,调用此方法归档旧条目(不删除)。
    /// 若已是 `Historical`,操作为 no-op(幂等)。
    ///
    /// # INV-8 约束
    /// `Historical` → `Current` 禁止(逆向升级违反 INV-8)。
    /// 本方法仅处理 `→ Historical` 方向。
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

    /// 返回时间元数据的引用(若存在)
    pub fn temporal_meta(&self) -> Option<&TemporalMeta> {
        self.temporal_meta.as_ref()
    }
}

/// Wiki 存储配置 — 控制数据库路径、向量维度、WAL 模式与读连接池
///
/// 默认配置:
/// - `db_path`: "wiki.db"(当前目录)
/// - `vector_dim`: 512(与 CLV 对齐)
/// - `wal_enabled`: true(WAL 模式提升并发读写性能)
/// - `read_pool_size`: 2(只读连接池默认大小,配合 WAL 实现并发读)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WikiConfig {
    /// SQLite 数据库文件路径
    pub db_path: std::path::PathBuf,
    /// 嵌入向量维度(默认 512,与 nexus_core::CLV::DIMENSION 对齐)
    ///
    /// WHY(P1-1):接入 NMC 语义编码后,条目嵌入可能为 384 维(ONNX)或
    /// text_dim 维(字节频率降级)。`vector_dim` 必须与条目生成路径的
    /// 嵌入维度一致,否则 HNSW 索引维度不匹配。
    pub vector_dim: usize,
    /// 是否启用 WAL 模式(默认 true)
    pub wal_enabled: bool,
    /// 只读连接池大小(默认 2)
    ///
    /// WHY:SQLite WAL 允许一个写入者与多个读取者并发;
    /// 独立的只读 Connection 绕过 `Mutex` 串行,使读查询真正并行。
    #[serde(default = "default_read_pool_size")]
    pub read_pool_size: usize,

    /// 是否启用 FTS5 全文索引(默认 true)
    ///
    /// WHY:FTS5 提供 O(log n) 全文检索,在大规模文档库(1000+)场景下
    /// 显著优于 LIKE 全表扫描。某些环境(嵌入式平台、旧版 SQLite)可能
    /// 不支持 FTS5,此时 `init_fts_table` 检测失败后自动降级到 LIKE,
    /// 保证功能可用性。显式设为 false 可强制走 LIKE 路径(兼容性/测试)。
    #[serde(default = "default_fts_enabled")]
    pub fts_enabled: bool,

    /// HNSW 索引参数(P2-5 配置化,默认使用 HnswConfig::default)
    ///
    /// WHY:原 HNSW 参数(M/ef_construction/ef_search 等)硬编码为常量,
    /// 无法通过配置调优。P2-5 提升为可配置项,支持不同规模/精度需求的场景。
    #[serde(default)]
    pub hnsw: HnswConfig,

    /// RAG 混合检索融合配置(v2.9.0-omega,Task 3)
    ///
    /// 控制 HNSW(dense)与 FTS5(sparse)检索结果的 RRF 融合参数。
    /// 默认使用 `HybridSearchConfig::default()`(rrf_k=60,dense/sparse 等权)。
    ///
    /// WHY `#[serde(default)]`:旧配置文件(无 hybrid_search 段)反序列化为
    /// 默认值,不破坏现有持久化数据与测试。
    #[serde(default)]
    pub hybrid_search: HybridSearchConfig,
}

/// 默认读连接池大小 — 与 `WikiConfig::default` 保持一致
const fn default_read_pool_size() -> usize {
    2
}

/// 默认 FTS5 启用状态 — 与 `WikiConfig::default` 保持一致
const fn default_fts_enabled() -> bool {
    true
}

/// HNSW 索引参数配置 — 控制 ANN 检索的精度/速度/内存权衡
///
/// P2-5: 将原硬编码常量提升为可配置项,支持通过 WikiConfig 调优。
///
/// # 参数说明(参考 HNSW 论文 Malkov & Yashunin 2016)
/// - `max_nb_connection`(M):每层最大连接数,控制图连通性。M↑ → 召回率↑ 内存↑。
///   论文推荐 M ∈ [16, 48],默认 16。
/// - `max_elements`:预分配容量提示(非硬性限制),仅优化内存分配。默认 10000。
/// - `max_layer`:最大层级,控制层次结构深度。默认 16。
/// - `ef_construction`:构建时 ef 参数,控制索引构建质量。ef↑ → 精度↑ 构建耗时↑。
///   论文推荐 ef_construction ∈ [100, 500],默认 200。
/// - `ef_search`:搜索时 ef 参数,控制搜索宽度,必须 > k。ef↑ → 召回率↑ 延迟↑。
///   **自适应模式**(`None`,默认):根据索引规模动态调整
///   (<10K → 50 / 10K-100K → 100 / >100K → 200),保证 >95% 召回率。
///   **显式模式**(`Some(n)`):用户指定固定值,适用于已知规模的调优场景。
///
/// # 向后兼容
/// 所有字段使用 `#[serde(default)]`,旧配置文件(无 hnsw 段)反序列化为默认值。
/// `ef_search` 字段为 `Option<usize>`:
/// - 旧配置文件中 `ef_search: 50` 反序列化为 `Some(50)`(显式模式)
/// - 缺失字段反序列化为 `None`(自适应模式,与 Default 一致)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HnswConfig {
    /// 每层最大连接数(M 参数),控制图连通性
    pub max_nb_connection: usize,
    /// 预分配容量提示(非硬性限制,仅优化分配)
    pub max_elements: usize,
    /// 最大层级,控制层次结构深度
    pub max_layer: usize,
    /// 构建时 ef 参数,控制索引构建质量
    pub ef_construction: usize,
    /// 搜索时 ef 参数,控制搜索宽度(必须 > k)
    ///
    /// WHY Option<usize>(v2.9.0-omega 自适应):
    /// - `None`(默认)= 自适应模式,根据索引规模动态调整 ef_search,
    ///   解决 100K+ 规模下固定 ef=50 召回率 <95% 的问题
    /// - `Some(n)` = 用户显式指定,适用于已知规模的调优场景
    ///
    /// 自适应档位见 `HnswStore::adaptive_ef_search`:
    /// <10K → 50 / 10K-100K → 100 / >100K → 200
    #[serde(default)]
    pub ef_search: Option<usize>,
}

impl Default for HnswConfig {
    fn default() -> Self {
        Self {
            max_nb_connection: 16,
            max_elements: 10_000,
            max_layer: 16,
            ef_construction: 200,
            // WHY None(自适应模式):默认根据索引规模动态调整 ef_search,
            // <10K 时返回 50,与原固定值 50 行为一致(向后兼容)
            ef_search: None,
        }
    }
}

impl HnswConfig {
    /// 创建自定义 HNSW 参数配置
    ///
    /// `ef_search` 参数内部转为 `Some(ef_search)`(显式模式)。
    /// 若需自适应模式,使用 `HnswConfig::default()` 后修改其他字段,
    /// 或直接构造结构体字面量 `ef_search: None`。
    pub fn new(
        max_nb_connection: usize,
        max_elements: usize,
        max_layer: usize,
        ef_construction: usize,
        ef_search: usize,
    ) -> Self {
        Self {
            max_nb_connection,
            max_elements,
            max_layer,
            ef_construction,
            // WHY 转 Some:保留 usize 签名向后兼容现有调用方,
            // 显式指定的 ef_search 不走自适应路径
            ef_search: Some(ef_search),
        }
    }
}

impl Default for WikiConfig {
    fn default() -> Self {
        Self {
            db_path: std::path::PathBuf::from("wiki.db"),
            vector_dim: 512,
            wal_enabled: true,
            read_pool_size: default_read_pool_size(),
            fts_enabled: default_fts_enabled(),
            hnsw: HnswConfig::default(),
            hybrid_search: HybridSearchConfig::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wiki_entry_new_auto_timestamps() {
        let before = Utc::now();
        let entry = WikiEntry::new("e-1", "标题", "内容", vec!["t".into()], vec![0.0; 512]);
        let after = Utc::now();
        assert_eq!(entry.entry_id, "e-1");
        assert_eq!(entry.title, "标题");
        assert_eq!(entry.content, "内容");
        assert_eq!(entry.tags, vec!["t".to_string()]);
        assert_eq!(entry.embedding.len(), 512);
        assert!(entry.created_at >= before);
        assert!(entry.created_at <= after);
        assert_eq!(entry.created_at, entry.updated_at);
    }

    #[test]
    fn test_wiki_config_default() {
        let config = WikiConfig::default();
        assert_eq!(config.db_path, std::path::PathBuf::from("wiki.db"));
        assert_eq!(config.vector_dim, 512);
        assert!(config.wal_enabled);
        assert_eq!(config.read_pool_size, 2);
    }

    #[test]
    fn test_wiki_entry_serde_roundtrip() {
        let entry = WikiEntry::new(
            "e-1",
            "标题",
            "内容",
            vec!["t1".into(), "t2".into()],
            vec![0.5; 512],
        );
        let json = serde_json::to_string(&entry).unwrap();
        let restored: WikiEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, restored);
    }
}
