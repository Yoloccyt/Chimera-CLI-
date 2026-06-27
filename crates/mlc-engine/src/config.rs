//! MLC 配置 — 四级记忆的容量与路径配置
//!
//! 对应架构层:L2 Memory
//!
//! # 设计决策(WHY)
//! - **容量分级**:L0(64)< L1(1024)< L2(4096)< L3(无限制,SQLite 持久化),
//!   符合"越靠近 CPU 容量越小、延迟越低"的存储层级原理
//! - **metrics_report_interval**:每 100 次操作发布一次 MemoryMetricsReported 事件,
//!   平衡事件流量与监控实时性(过小导致事件风暴,过大导致监控滞后)
//! - **procedural_db_path**:使用 `~` 前缀的字符串,调用方负责展开,
//!   避免引入 `dirs` crate 依赖(遵循"不引入未被任务要求的依赖"原则)

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::MlcError;

// ── 容量上界常量(SubTask 14.7)─────────────────────────────────────────
// WHY 集中定义:上界值有明确的内存预算依据,集中声明便于审查与统一调整。
// 所有上界为闭区间(≤),即"恰好等于上界"的配置合法。

/// L0 工作记忆容量上界(≤ 1024)
///
/// WHY:L0 使用 DashMap 全内存缓存,每条目约 1KB。1024 条目约 1MB,
/// 足以容纳多个 Quest 的活跃上下文;超过此值应改用 L1/L2 分层,
/// 而非无限扩大 L0(否则高并发下 DashMap 内存膨胀导致 OOM)
const L0_CAPACITY_MAX: usize = 1024;

/// L1 情节记忆容量上界(≤ 65536)
///
/// WHY:L1 为内存 FIFO 缓存,每条目约 1KB。65536 条目约 64MB,
/// 足以容纳数月 Quest 历史;超过此值应持久化到 L3 SQLite,
/// 避免内存无限增长
const L1_CAPACITY_MAX: usize = 65536;

/// L2 语义记忆容量上界(≤ 65536)
///
/// WHY:L2 含 512-dim CLV 向量(f32),每条目约 2KB+。65536 条目约 128MB,
/// 线性扫描 KNN 在此规模下仍可接受(< 100ms);超过此值应接入 sqlite-vec
/// (Week 6 后)提升规模,而非继续扩大内存缓存
const L2_CAPACITY_MAX: usize = 65536;

/// 指标上报间隔上界(≤ 1_000_000)
///
/// WHY:过大的间隔会导致监控长期失效(如 100 万次操作才上报一次)。
/// 1_000_000 次操作约 1-10 小时(取决于操作频率),仍能提供有效监控;
/// 超过此值视为配置失误
const METRICS_REPORT_INTERVAL_MAX: u64 = 1_000_000;

/// MLC 引擎配置 — 四级记忆的容量与持久化路径
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MlcConfig {
    /// L0 工作记忆容量上限(默认 64)
    ///
    /// WHY:64 条目平衡命中率与内存占用。每条目约 1KB,64 条目约 64KB,
    /// 可容纳当前 Quest 的活跃上下文;超出时 LRU 驱逐最久未访问的条目
    pub l0_capacity: usize,

    /// L1 情节记忆容量上限(默认 1024)
    ///
    /// WHY:1024 条目约 1MB,可容纳近期 Quest 的完整执行历史;
    /// 超出时 FIFO 驱逐最旧的条目(按 created_at 排序)
    pub l1_capacity: usize,

    /// L2 语义记忆容量上限(默认 4096)
    ///
    /// WHY:4096 条目约 8MB(含 512-dim CLV),线性扫描 KNN 在此规模下
    /// Top-10 召回 < 5ms(基准测试验证);Week 6 后接入 sqlite-vec 提升规模
    pub l2_capacity: usize,

    /// 指标上报间隔(默认 100,每 100 次操作发布 MemoryMetricsReported 事件)
    ///
    /// WHY:100 次操作平衡事件流量与监控实时性。过小(如 10)导致事件风暴,
    /// 过大(如 1000)导致监控滞后。100 次 ≈ 1-10 秒(取决于操作频率)
    pub metrics_report_interval: u64,

    /// L3 程序记忆 SQLite 数据库路径(默认 `~/.aether/memory/procedural.db`)
    ///
    /// WHY:使用 `~` 前缀便于跨平台配置;调用方通过 `expand_tilde` 展开。
    /// 路径形如 `~/.aether/memory/procedural.db`,展开后为绝对路径
    pub procedural_db_path: PathBuf,
}

impl MlcConfig {
    /// 创建默认配置
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置 L0 工作记忆容量
    pub fn with_l0_capacity(mut self, capacity: usize) -> Self {
        self.l0_capacity = capacity;
        self
    }

    /// 设置 L1 情节记忆容量
    pub fn with_l1_capacity(mut self, capacity: usize) -> Self {
        self.l1_capacity = capacity;
        self
    }

    /// 设置 L2 语义记忆容量
    pub fn with_l2_capacity(mut self, capacity: usize) -> Self {
        self.l2_capacity = capacity;
        self
    }

    /// 设置指标上报间隔
    pub fn with_metrics_interval(mut self, interval: u64) -> Self {
        self.metrics_report_interval = interval;
        self
    }

    /// 设置 L3 程序记忆数据库路径
    pub fn with_procedural_db_path(mut self, path: impl Into<PathBuf>) -> Self {
        self.procedural_db_path = path.into();
        self
    }

    /// 校验配置合法性,返回 MlcError 描述具体问题
    ///
    /// 校验规则:
    /// - 容量必须 > 0(L0/L1/L2)
    /// - 容量不得超过上界(防止 OOM,SubTask 14.7):
    ///   - `l0_capacity ≤ 1024`(L0 为 DashMap 内存缓存)
    ///   - `l1_capacity ≤ 65536`(L1 为内存 FIFO 缓存)
    ///   - `l2_capacity ≤ 65536`(L2 含 512-dim CLV 向量)
    /// - `metrics_report_interval` 必须 > 0 且 ≤ 1_000_000(防止监控失效)
    /// - `procedural_db_path` 不能为空
    pub fn validate(&self) -> Result<(), MlcError> {
        // ── 下界校验(原有逻辑,保持不变)──
        if self.l0_capacity == 0 {
            return Err(MlcError::InvalidConfig("l0_capacity 不能为 0".into()));
        }
        if self.l1_capacity == 0 {
            return Err(MlcError::InvalidConfig("l1_capacity 不能为 0".into()));
        }
        if self.l2_capacity == 0 {
            return Err(MlcError::InvalidConfig("l2_capacity 不能为 0".into()));
        }
        if self.metrics_report_interval == 0 {
            return Err(MlcError::InvalidConfig(
                "metrics_report_interval 不能为 0".into(),
            ));
        }
        if self.procedural_db_path.as_os_str().is_empty() {
            return Err(MlcError::InvalidConfig(
                "procedural_db_path 不能为空".into(),
            ));
        }

        // ── 上界校验(SubTask 14.7 新增,防止 OOM 与配置失误)──
        if self.l0_capacity > L0_CAPACITY_MAX {
            return Err(MlcError::InvalidConfig(format!(
                "l0_capacity 超过上界 {L0_CAPACITY_MAX}(当前 {}):\
                 L0 为 DashMap 内存缓存,过大会导致 OOM",
                self.l0_capacity
            )));
        }
        if self.l1_capacity > L1_CAPACITY_MAX {
            return Err(MlcError::InvalidConfig(format!(
                "l1_capacity 超过上界 {L1_CAPACITY_MAX}(当前 {}):\
                 L1 为内存 FIFO 缓存,过大会导致 OOM",
                self.l1_capacity
            )));
        }
        if self.l2_capacity > L2_CAPACITY_MAX {
            return Err(MlcError::InvalidConfig(format!(
                "l2_capacity 超过上界 {L2_CAPACITY_MAX}(当前 {}):\
                 L2 含 512-dim CLV 向量,过大会导致 OOM",
                self.l2_capacity
            )));
        }
        if self.metrics_report_interval > METRICS_REPORT_INTERVAL_MAX {
            return Err(MlcError::InvalidConfig(format!(
                "metrics_report_interval 超过上界 {METRICS_REPORT_INTERVAL_MAX}\
                 (当前 {}):过大会导致监控长期失效",
                self.metrics_report_interval
            )));
        }
        Ok(())
    }

    /// 展开 `~` 为用户主目录
    ///
    /// WHY:SubTask 21.3 — 委托给 `nexus_core::path_util::expand_tilde`,
    /// 消除与 cmt-tiering 的重复实现。保留此方法作为薄包装,保持 API 向后兼容
    /// (测试与外部调用无需修改)。
    pub fn expand_tilde(path: &Path) -> PathBuf {
        nexus_core::path_util::expand_tilde(path)
    }
}

impl Default for MlcConfig {
    fn default() -> Self {
        Self {
            l0_capacity: 64,
            l1_capacity: 1024,
            l2_capacity: 4096,
            metrics_report_interval: 100,
            procedural_db_path: PathBuf::from("~/.aether/memory/procedural.db"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = MlcConfig::default();
        assert_eq!(config.l0_capacity, 64);
        assert_eq!(config.l1_capacity, 1024);
        assert_eq!(config.l2_capacity, 4096);
        assert_eq!(config.metrics_report_interval, 100);
        assert_eq!(
            config.procedural_db_path,
            PathBuf::from("~/.aether/memory/procedural.db")
        );
    }

    #[test]
    fn test_builder_chain() {
        let config = MlcConfig::new()
            .with_l0_capacity(128)
            .with_l1_capacity(2048)
            .with_l2_capacity(8192)
            .with_metrics_interval(50)
            .with_procedural_db_path("/tmp/test.db");
        assert_eq!(config.l0_capacity, 128);
        assert_eq!(config.l1_capacity, 2048);
        assert_eq!(config.l2_capacity, 8192);
        assert_eq!(config.metrics_report_interval, 50);
        assert_eq!(config.procedural_db_path, PathBuf::from("/tmp/test.db"));
    }

    #[test]
    fn test_validate_valid() {
        let config = MlcConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_zero_l0_capacity() {
        let config = MlcConfig::new().with_l0_capacity(0);
        let err = config.validate().unwrap_err();
        assert!(matches!(err, MlcError::InvalidConfig(_)));
    }

    #[test]
    fn test_validate_zero_metrics_interval() {
        let config = MlcConfig::new().with_metrics_interval(0);
        let err = config.validate().unwrap_err();
        assert!(matches!(err, MlcError::InvalidConfig(_)));
    }

    // ── 上界校验测试(SubTask 14.7)──

    #[test]
    fn test_validate_l0_capacity_exceeds_max() {
        let config = MlcConfig::new().with_l0_capacity(L0_CAPACITY_MAX + 1);
        let err = config.validate().unwrap_err();
        assert!(matches!(err, MlcError::InvalidConfig(_)));
        let msg = err.to_string();
        assert!(msg.contains("l0_capacity"), "错误信息应包含字段名");
        assert!(
            msg.contains(&L0_CAPACITY_MAX.to_string()),
            "错误信息应包含上界值"
        );
    }

    #[test]
    fn test_validate_l1_capacity_exceeds_max() {
        let config = MlcConfig::new().with_l1_capacity(L1_CAPACITY_MAX + 1);
        let err = config.validate().unwrap_err();
        assert!(matches!(err, MlcError::InvalidConfig(_)));
        let msg = err.to_string();
        assert!(msg.contains("l1_capacity"), "错误信息应包含字段名");
        assert!(
            msg.contains(&L1_CAPACITY_MAX.to_string()),
            "错误信息应包含上界值"
        );
    }

    #[test]
    fn test_validate_l2_capacity_exceeds_max() {
        let config = MlcConfig::new().with_l2_capacity(L2_CAPACITY_MAX + 1);
        let err = config.validate().unwrap_err();
        assert!(matches!(err, MlcError::InvalidConfig(_)));
        let msg = err.to_string();
        assert!(msg.contains("l2_capacity"), "错误信息应包含字段名");
        assert!(
            msg.contains(&L2_CAPACITY_MAX.to_string()),
            "错误信息应包含上界值"
        );
    }

    #[test]
    fn test_validate_metrics_interval_exceeds_max() {
        let config = MlcConfig::new().with_metrics_interval(METRICS_REPORT_INTERVAL_MAX + 1);
        let err = config.validate().unwrap_err();
        assert!(matches!(err, MlcError::InvalidConfig(_)));
        let msg = err.to_string();
        assert!(
            msg.contains("metrics_report_interval"),
            "错误信息应包含字段名"
        );
        assert!(
            msg.contains(&METRICS_REPORT_INTERVAL_MAX.to_string()),
            "错误信息应包含上界值"
        );
    }

    #[test]
    fn test_validate_boundary_at_max_passes() {
        // 所有字段恰好等于上界(闭区间 ≤),应通过校验
        let config = MlcConfig::new()
            .with_l0_capacity(L0_CAPACITY_MAX)
            .with_l1_capacity(L1_CAPACITY_MAX)
            .with_l2_capacity(L2_CAPACITY_MAX)
            .with_metrics_interval(METRICS_REPORT_INTERVAL_MAX);
        assert!(config.validate().is_ok(), "恰好等于上界的配置应合法");
    }

    #[test]
    fn test_expand_tilde_no_tilde() {
        let path = PathBuf::from("/absolute/path.db");
        let expanded = MlcConfig::expand_tilde(&path);
        assert_eq!(expanded, path);
    }

    #[test]
    fn test_expand_tilde_with_home() {
        // 设置临时 HOME 环境变量
        std::env::set_var("HOME", "/test/home");
        let path = PathBuf::from("~/memory.db");
        let expanded = MlcConfig::expand_tilde(&path);
        assert_eq!(expanded, PathBuf::from("/test/home/memory.db"));
    }

    #[test]
    fn test_expand_tilde_only_tilde() {
        std::env::set_var("HOME", "/test/home");
        let path = PathBuf::from("~");
        let expanded = MlcConfig::expand_tilde(&path);
        assert_eq!(expanded, PathBuf::from("/test/home"));
    }
}
