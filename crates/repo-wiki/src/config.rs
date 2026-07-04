//! Wiki 配置模块 — 提供便捷的配置构造函数
//!
//! 对应架构层:L5 Knowledge
//!
//! `WikiConfig` 核心定义在 `types.rs`,本模块提供常用场景的便捷构造函数。

use std::path::PathBuf;

use crate::types::WikiConfig;

impl WikiConfig {
    /// 创建内存数据库配置(仅用于测试)
    ///
    /// WHY:SQLite 支持 `:memory:` 内存数据库,适合单元测试,
    /// 避免文件系统 IO 影响测试速度与隔离性。
    #[cfg(test)]
    pub fn in_memory() -> Self {
        Self {
            db_path: PathBuf::from(":memory:"),
            vector_dim: 512,
            wal_enabled: false,
        }
    }

    /// 创建指定路径的配置,使用默认维度(512)与 WAL 启用
    pub fn with_path(db_path: impl Into<PathBuf>) -> Self {
        Self {
            db_path: db_path.into(),
            vector_dim: 512,
            wal_enabled: true,
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_with_path_builder() {
        let config = WikiConfig::with_path("/tmp/test.db")
            .vector_dim(256)
            .wal_enabled(false);
        assert_eq!(config.db_path, PathBuf::from("/tmp/test.db"));
        assert_eq!(config.vector_dim, 256);
        assert!(!config.wal_enabled);
    }

    #[test]
    fn test_with_path_defaults() {
        let config = WikiConfig::with_path("wiki.db");
        assert_eq!(config.vector_dim, 512);
        assert!(config.wal_enabled);
    }
}
