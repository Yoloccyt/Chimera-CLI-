//! Repo Wiki 核心类型 — WikiEntry 与 WikiConfig
//!
//! 对应架构层:L5 Knowledge
//! 对应创新点:ISCM(Inter-Shared Cross Module,跨层共享索引)
//!
//! # 类型职责
//! - `WikiEntry`:Wiki 条目,含标题、内容、标签、嵌入向量、时间戳
//! - `WikiConfig`:Wiki 存储配置,含数据库路径、向量维度、WAL 开关

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Wiki 条目 — 知识沉淀的最小单元
///
/// `embedding` 在 Week 2 阶段为占位哈希向量(SHA-256 扩展为 512-dim),
/// Week 6 NMC 编码器实现后替换为真实 CLV 嵌入。
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
    /// 嵌入向量(512-dim f32,Week 2 占位,Week 6 替换为 CLV)
    pub embedding: Vec<f32>,
    /// 创建时间(UTC,自动生成)
    pub created_at: DateTime<Utc>,
    /// 最后更新时间(UTC,插入/更新时自动刷新)
    pub updated_at: DateTime<Utc>,
}

impl WikiEntry {
    /// 创建新条目,`created_at` 与 `updated_at` 自动设为当前 UTC 时间
    ///
    /// # 示例
    /// ```
    /// use repo_wiki::WikiEntry;
    /// let entry = WikiEntry::new("e-1", "标题", "内容", vec!["t".into()], vec![0.0; 512]);
    /// assert_eq!(entry.entry_id, "e-1");
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
        }
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
    pub vector_dim: usize,
    /// 是否启用 WAL 模式(默认 true)
    pub wal_enabled: bool,
    /// 只读连接池大小(默认 2)
    ///
    /// WHY:SQLite WAL 允许一个写入者与多个读取者并发;
    /// 独立的只读 Connection 绕过 `Mutex` 串行,使读查询真正并行。
    #[serde(default = "default_read_pool_size")]
    pub read_pool_size: usize,
}

/// 默认读连接池大小 — 与 `WikiConfig::default` 保持一致
const fn default_read_pool_size() -> usize {
    2
}

impl Default for WikiConfig {
    fn default() -> Self {
        Self {
            db_path: std::path::PathBuf::from("wiki.db"),
            vector_dim: 512,
            wal_enabled: true,
            read_pool_size: default_read_pool_size(),
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
