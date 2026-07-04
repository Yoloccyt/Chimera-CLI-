//! CMT 配置 — 四级存储的容量与路径配置
//!
//! 对应架构层:L3 Storage
//!
//! # 设计决策(WHY)
//! - **容量分级**:Hot(256)< Warm(4096)< Cold(65536)< Ice(无限制),
//!   符合"越靠近顶层容量越小、延迟越低"的存储层级原理
//! - **decay_tau_seconds**:衰减时间常数 τ,默认 86400 秒(24 小时),
//!   衰减公式 `priority = access_count × exp(-Δt / τ)`,τ 越大衰减越慢
//! - **路径使用 `~` 前缀**:调用方负责展开,避免引入 `dirs` crate 依赖
//!   (遵循"不引入未被任务要求的依赖"原则,参考 mlc-engine/config.rs)

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::CmtError;

/// CMT 引擎配置 — 四级存储的容量与持久化路径
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CmtConfig {
    /// Hot 层容量上限(默认 256)
    ///
    /// WHY:256 条目平衡命中率与内存占用。每条目约 1KB,256 条目约 256KB,
    /// 可容纳当前活跃的能力集合;超出时 LRU 驱逐最久未访问的条目到 Warm 层
    pub hot_capacity: usize,

    /// Warm 层容量上限(默认 4096)
    ///
    /// WHY:4096 条目约 4MB,可容纳近期使用的能力;超出时按空闲时间降级到 Cold 层
    pub warm_capacity: usize,

    /// Cold 层容量上限(默认 65536)
    ///
    /// WHY:65536 条目约 64MB,可容纳较长时间未访问的能力;
    /// 超出时按衰减优先级降级到 Ice 层
    pub cold_capacity: usize,

    /// Warm 层 SQLite 数据库路径(默认 `~/.aether/memory/cmt_warm.db`)
    ///
    /// WHY:使用 `~` 前缀便于跨平台配置;调用方通过 `expand_tilde` 展开。
    /// Warm 层使用 SQLite WAL 模式,支持并发读写
    pub warm_db_path: PathBuf,

    /// Cold 层目录(默认 `~/.aether/memory/cold/`)
    ///
    /// WHY:Cold 层使用 SQLite 附加数据库实现(避免新依赖),
    /// 每个附加数据库为一个文件,存放在此目录下
    pub cold_dir: PathBuf,

    /// Ice 层目录(默认 `~/.aether/memory/ice/`)
    ///
    /// WHY:Ice 层使用归档只读文件,每个能力一个 `.bin` 文件,
    /// 路径形如 `<ice_dir>/<cap_id>.bin`
    pub ice_dir: PathBuf,

    /// 衰减时间常数 τ(默认 86400 秒 = 24 小时)
    ///
    /// WHY:衰减公式 `priority = access_count × exp(-Δt / τ)`,
    /// τ 越大衰减越慢。24 小时意味着 1 天前的访问权重降为 1/e ≈ 0.37。
    /// `priority < 0.1` 时触发降级迁移
    pub decay_tau_seconds: u64,
}

impl CmtConfig {
    /// 创建默认配置
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置 Hot 层容量
    pub fn with_hot_capacity(mut self, capacity: usize) -> Self {
        self.hot_capacity = capacity;
        self
    }

    /// 设置 Warm 层容量
    pub fn with_warm_capacity(mut self, capacity: usize) -> Self {
        self.warm_capacity = capacity;
        self
    }

    /// 设置 Cold 层容量
    pub fn with_cold_capacity(mut self, capacity: usize) -> Self {
        self.cold_capacity = capacity;
        self
    }

    /// 设置 Warm 层数据库路径
    pub fn with_warm_db_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.warm_db_path = path.into();
        self
    }

    /// 设置 Cold 层目录
    pub fn with_cold_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.cold_dir = path.into();
        self
    }

    /// 设置 Ice 层目录
    pub fn with_ice_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.ice_dir = path.into();
        self
    }

    /// 设置衰减时间常数 τ(秒)
    pub fn with_decay_tau_seconds(mut self, seconds: u64) -> Self {
        self.decay_tau_seconds = seconds;
        self
    }

    /// 校验配置合法性,返回 CmtError 描述具体问题
    ///
    /// 校验规则:
    /// - 容量必须 > 0(Hot/Warm/Cold)
    /// - decay_tau_seconds 必须 > 0
    /// - 路径不能为空
    pub fn validate(&self) -> Result<(), CmtError> {
        if self.hot_capacity == 0 {
            return Err(CmtError::InvalidConfig("hot_capacity 不能为 0".into()));
        }
        if self.warm_capacity == 0 {
            return Err(CmtError::InvalidConfig("warm_capacity 不能为 0".into()));
        }
        if self.cold_capacity == 0 {
            return Err(CmtError::InvalidConfig("cold_capacity 不能为 0".into()));
        }
        if self.decay_tau_seconds == 0 {
            return Err(CmtError::InvalidConfig("decay_tau_seconds 不能为 0".into()));
        }
        if self.warm_db_path.as_os_str().is_empty() {
            return Err(CmtError::InvalidConfig("warm_db_path 不能为空".into()));
        }
        if self.cold_dir.as_os_str().is_empty() {
            return Err(CmtError::InvalidConfig("cold_dir 不能为空".into()));
        }
        if self.ice_dir.as_os_str().is_empty() {
            return Err(CmtError::InvalidConfig("ice_dir 不能为空".into()));
        }
        Ok(())
    }

    /// 展开 `~` 为用户主目录
    ///
    /// WHY:SubTask 21.3 — 委托给 `nexus_core::path_util::expand_tilde`,
    /// 消除与 mlc-engine 的重复实现。保留此方法作为薄包装,保持 API 向后兼容
    /// (测试与外部调用无需修改)。
    pub fn expand_tilde(path: &Path) -> PathBuf {
        nexus_core::path_util::expand_tilde(path)
    }
}

impl Default for CmtConfig {
    fn default() -> Self {
        Self {
            hot_capacity: 256,
            warm_capacity: 4096,
            cold_capacity: 65536,
            warm_db_path: PathBuf::from("~/.aether/memory/cmt_warm.db"),
            cold_dir: PathBuf::from("~/.aether/memory/cold/"),
            ice_dir: PathBuf::from("~/.aether/memory/ice/"),
            decay_tau_seconds: 86400,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = CmtConfig::default();
        assert_eq!(config.hot_capacity, 256);
        assert_eq!(config.warm_capacity, 4096);
        assert_eq!(config.cold_capacity, 65536);
        assert_eq!(
            config.warm_db_path,
            PathBuf::from("~/.aether/memory/cmt_warm.db")
        );
        assert_eq!(config.cold_dir, PathBuf::from("~/.aether/memory/cold/"));
        assert_eq!(config.ice_dir, PathBuf::from("~/.aether/memory/ice/"));
        assert_eq!(config.decay_tau_seconds, 86400);
    }

    #[test]
    fn test_builder_chain() {
        let config = CmtConfig::new()
            .with_hot_capacity(128)
            .with_warm_capacity(2048)
            .with_cold_capacity(32768)
            .with_warm_db_path("/tmp/warm.db")
            .with_cold_dir("/tmp/cold/")
            .with_ice_dir("/tmp/ice/")
            .with_decay_tau_seconds(3600);
        assert_eq!(config.hot_capacity, 128);
        assert_eq!(config.warm_capacity, 2048);
        assert_eq!(config.cold_capacity, 32768);
        assert_eq!(config.decay_tau_seconds, 3600);
    }

    #[test]
    fn test_validate_valid() {
        let config = CmtConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_zero_hot_capacity() {
        let config = CmtConfig::new().with_hot_capacity(0);
        let err = config.validate().unwrap_err();
        assert!(matches!(err, CmtError::InvalidConfig(_)));
    }

    #[test]
    fn test_validate_zero_decay_tau() {
        let config = CmtConfig::new().with_decay_tau_seconds(0);
        let err = config.validate().unwrap_err();
        assert!(matches!(err, CmtError::InvalidConfig(_)));
    }

    #[test]
    fn test_expand_tilde_no_tilde() {
        let path = PathBuf::from("/absolute/path.db");
        let expanded = CmtConfig::expand_tilde(&path);
        assert_eq!(expanded, path);
    }

    #[test]
    fn test_expand_tilde_with_home() {
        std::env::set_var("HOME", "/test/home");
        let path = PathBuf::from("~/memory.db");
        let expanded = CmtConfig::expand_tilde(&path);
        assert_eq!(expanded, PathBuf::from("/test/home/memory.db"));
    }

    #[test]
    fn test_expand_tilde_only_tilde() {
        std::env::set_var("HOME", "/test/home");
        let path = PathBuf::from("~");
        let expanded = CmtConfig::expand_tilde(&path);
        assert_eq!(expanded, PathBuf::from("/test/home"));
    }
}
