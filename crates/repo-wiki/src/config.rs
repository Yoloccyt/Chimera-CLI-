//! Wiki 配置模块 — 提供便捷的配置构造函数
//!
//! 对应架构层:L5 Knowledge
//!
//! `WikiConfig` 核心定义在 `types.rs`,本模块提供常用场景的便捷构造函数。

use std::path::PathBuf;

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
    /// # 示例
    /// ```
    /// use repo_wiki::types::{HnswConfig, WikiConfig};
    ///
    /// let config = WikiConfig::with_path("wiki.db")
    ///     .hnsw_config(HnswConfig::new(32, 50_000, 20, 300, 100));
    /// assert_eq!(config.hnsw.max_nb_connection, 32);
    /// assert_eq!(config.hnsw.ef_search, 100);
    /// ```
    pub fn hnsw_config(mut self, config: HnswConfig) -> Self {
        self.hnsw = config;
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
        assert_eq!(config.hnsw.ef_search, 50);
    }

    #[test]
    fn test_hnsw_config_builder() {
        let config =
            WikiConfig::with_path("wiki.db").hnsw_config(HnswConfig::new(32, 50_000, 20, 300, 100));
        assert_eq!(config.hnsw.max_nb_connection, 32);
        assert_eq!(config.hnsw.max_elements, 50_000);
        assert_eq!(config.hnsw.max_layer, 20);
        assert_eq!(config.hnsw.ef_construction, 300);
        assert_eq!(config.hnsw.ef_search, 100);
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
    }
}
