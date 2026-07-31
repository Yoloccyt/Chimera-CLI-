//! Wiki 配置模块 — 提供便捷的配置构造函数
//!
//! 对应架构层:L5 Knowledge
//!
//! `WikiConfig` 核心定义在 `types.rs`,本模块提供常用场景的便捷构造函数。

use std::path::PathBuf;

use crate::search::HybridSearchConfig;
use crate::types::{HnswConfig, WikiConfig};

impl WikiConfig {
    /// 创建指定路径的配置,使用默认维度(512)、WAL 启用、读连接池大小 2
    pub fn with_path(db_path: impl Into<PathBuf>) -> Self {
        Self {
            db_path: db_path.into(),
            vector_dim: 512,
            wal_enabled: true,
            read_pool_size: 2,
            fts_enabled: true,
            hnsw: HnswConfig::default(),
            hybrid_search: HybridSearchConfig::default(),
        }
    }

    /// 设置向量维度(builder 风格)
    pub fn vector_dim(mut self, dim: usize) -> Self {
        self.vector_dim = dim;
        self
    }

    /// 设置 WAL 启用状态(builder 风格)
    pub fn wal_enabled(mut self, enabled: bool) -> Self {
        self.wal_enabled = enabled;
        self
    }

    /// 设置只读连接池大小(builder 风格)
    pub fn read_pool_size(mut self, size: usize) -> Self {
        self.read_pool_size = size;
        self
    }

    /// 设置 FTS5 全文索引启用状态(builder 风格)
    ///
    /// 设为 false 可禁用 FTS5,强制 `search_fulltext` 走 LIKE 降级路径。
    pub fn fts_enabled(mut self, enabled: bool) -> Self {
        self.fts_enabled = enabled;
        self
    }

    /// 设置 HNSW 索引参数(builder 风格)
    ///
    /// P2-5: 允许通过 builder 链式调用自定义 HNSW 参数(M/ef_construction/ef_search 等),
    /// 替代原硬编码常量。未调用此方法时使用 `HnswConfig::default()`。
    ///
    /// v2.9.0-omega: `HnswConfig::new(.., ef_search: usize)` 内部转 `Some(ef_search)`
    /// (显式模式);`HnswConfig::default()` 的 `ef_search` 为 `None`(自适应模式)。
    ///
    /// # 示例
    /// ```
    /// use repo_wiki::types::{HnswConfig, WikiConfig};
    ///
    /// let config = WikiConfig::with_path("wiki.db")
    ///     .hnsw_config(HnswConfig::new(32, 50_000, 20, 300, 100));
    /// assert_eq!(config.hnsw.max_nb_connection, 32);
    /// assert_eq!(config.hnsw.ef_search, Some(100));
    /// ```
    pub fn hnsw_config(mut self, config: HnswConfig) -> Self {
        self.hnsw = config;
        self
    }

    /// 设置混合检索融合参数(builder 风格)
    ///
    /// 控制 HNSW(dense)与 FTS5(sparse)检索结果的 RRF 融合参数。
    /// 未调用此方法时使用 `HybridSearchConfig::default()`(rrf_k=60,等权融合)。
    ///
    /// # 示例
    /// ```
    /// use repo_wiki::search::HybridSearchConfig;
    /// use repo_wiki::WikiConfig;
    ///
    /// let config = WikiConfig::with_path("wiki.db")
    ///     .hybrid_search_config(HybridSearchConfig::new(30, 1.5, 0.8));
    /// assert_eq!(config.hybrid_search.rrf_k, 30);
    /// assert!((config.hybrid_search.dense_weight - 1.5).abs() < 1e-6);
    /// ```
    pub fn hybrid_search_config(mut self, config: HybridSearchConfig) -> Self {
        self.hybrid_search = config;
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_with_path_builder() {
        let config = WikiConfig::with_path("/tmp/test.db")
            .vector_dim(256)
            .wal_enabled(false)
            .read_pool_size(4);
        assert_eq!(config.db_path, PathBuf::from("/tmp/test.db"));
        assert_eq!(config.vector_dim, 256);
        assert!(!config.wal_enabled);
        assert_eq!(config.read_pool_size, 4);
    }

    #[test]
    fn test_with_path_defaults() {
        let config = WikiConfig::with_path("wiki.db");
        assert_eq!(config.vector_dim, 512);
        assert!(config.wal_enabled);
        assert_eq!(config.read_pool_size, 2);
        // P2-5: 默认 HNSW 参数
        assert_eq!(config.hnsw.max_nb_connection, 16);
        assert_eq!(config.hnsw.ef_construction, 200);
        // v2.9.0-omega: 默认 ef_search = None(自适应模式)
        assert_eq!(config.hnsw.ef_search, None);
        // Task 3: 默认混合检索配置
        assert_eq!(config.hybrid_search, HybridSearchConfig::default());
        assert_eq!(config.hybrid_search.rrf_k, 60);
    }

    #[test]
    fn test_hnsw_config_builder() {
        let config =
            WikiConfig::with_path("wiki.db").hnsw_config(HnswConfig::new(32, 50_000, 20, 300, 100));
        assert_eq!(config.hnsw.max_nb_connection, 32);
        assert_eq!(config.hnsw.max_elements, 50_000);
        assert_eq!(config.hnsw.max_layer, 20);
        assert_eq!(config.hnsw.ef_construction, 300);
        // v2.9.0-omega: HnswConfig::new(.., 100) 内部转 Some(100)(显式模式)
        assert_eq!(config.hnsw.ef_search, Some(100));
    }

    #[test]
    fn test_hnsw_config_serde_roundtrip() {
        let config = WikiConfig::with_path("wiki.db")
            .hnsw_config(HnswConfig::new(48, 100_000, 24, 500, 150));
        let json = serde_json::to_string(&config).unwrap();
        let de: WikiConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(de.hnsw, config.hnsw);
    }

    #[test]
    fn test_hnsw_config_backward_compatibility() {
        // P2-5: 旧配置文件(无 hnsw 段)应反序列化为默认 HNSW 参数
        let old_json = r#"{
            "db_path": "wiki.db",
            "vector_dim": 512,
            "wal_enabled": true,
            "read_pool_size": 2,
            "fts_enabled": true
        }"#;
        let config: WikiConfig = serde_json::from_str(old_json).unwrap();
        assert_eq!(config.hnsw, HnswConfig::default());
        // Task 3: 旧配置文件(无 hybrid_search 段)也应反序列化为默认值
        assert_eq!(config.hybrid_search, HybridSearchConfig::default());
    }

    #[test]
    fn test_hybrid_search_config_builder() {
        let config = WikiConfig::with_path("wiki.db")
            .hybrid_search_config(HybridSearchConfig::new(30, 1.5, 0.8));
        assert_eq!(config.hybrid_search.rrf_k, 30);
        assert!((config.hybrid_search.dense_weight - 1.5).abs() < 1e-6);
        assert!((config.hybrid_search.sparse_weight - 0.8).abs() < 1e-6);
    }

    #[test]
    fn test_hybrid_search_config_serde_roundtrip() {
        let config = WikiConfig::with_path("wiki.db")
            .hybrid_search_config(HybridSearchConfig::new(45, 1.2, 0.9));
        let json = serde_json::to_string(&config).unwrap();
        let de: WikiConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(de.hybrid_search, config.hybrid_search);
    }
}
