//! HCW 配置实现 — HcwConfig 的 impl 块
//!
//! 对应架构层:L2 Memory
//!
//! # 设计决策(WHY)
//! - **结构体定义在 types.rs**:SubTask 2.1 要求 types.rs 定义所有核心类型(含 HcwConfig),
//!   impl 块放此文件,实现定义与行为分离
//! - **l0 < l1 < l2 < l3 严格递增**:四级窗口容量必须递增,否则窗口切换无意义
//! - **compression_threshold ∈ (0.0, 1.0]**:0.9 表示容量利用率达 90% 触发压缩,
//!   留 10% 余量避免频繁压缩;1.0 表示仅在溢出时压缩
//! - **effective_capacity_for(L3) = l3_capacity / 8**:1M 等效通过 128K 实际加载
//!   + 8× 稀疏化压缩比实现,避免暴力加载(架构红线)

use crate::error::HcwError;
use crate::types::{HcwConfig, WindowTier};

impl HcwConfig {
    /// 创建默认配置(架构手册推荐值)
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置 L0 窗口容量(链式 builder)
    pub fn with_l0_capacity(mut self, cap: usize) -> Self {
        self.l0_capacity = cap;
        self
    }

    /// 设置 L1 窗口容量(链式 builder)
    pub fn with_l1_capacity(mut self, cap: usize) -> Self {
        self.l1_capacity = cap;
        self
    }

    /// 设置 L2 窗口容量(链式 builder)
    pub fn with_l2_capacity(mut self, cap: usize) -> Self {
        self.l2_capacity = cap;
        self
    }

    /// 设置 L3 窗口等效容量(链式 builder)
    pub fn with_l3_capacity(mut self, cap: usize) -> Self {
        self.l3_capacity = cap;
        self
    }

    /// 设置压缩触发阈值(链式 builder)
    pub fn with_compression_threshold(mut self, threshold: f32) -> Self {
        self.compression_threshold = threshold;
        self
    }

    /// 校验配置合法性,返回 HcwError 描述具体问题
    ///
    /// 校验规则:
    /// - l0/l1/l2/l3 容量均 > 0
    /// - l0 < l1 < l2 < l3 严格递增(四级窗口容量必须递增)
    /// - compression_threshold ∈ (0.0, 1.0](0 表示永不压缩,1 表示仅溢出时压缩)
    pub fn validate(&self) -> Result<(), HcwError> {
        if self.l0_capacity == 0 {
            return Err(HcwError::InvalidConfig("l0_capacity 不能为 0".into()));
        }
        if self.l1_capacity == 0 {
            return Err(HcwError::InvalidConfig("l1_capacity 不能为 0".into()));
        }
        if self.l2_capacity == 0 {
            return Err(HcwError::InvalidConfig("l2_capacity 不能为 0".into()));
        }
        if self.l3_capacity == 0 {
            return Err(HcwError::InvalidConfig("l3_capacity 不能为 0".into()));
        }
        if self.l0_capacity >= self.l1_capacity {
            return Err(HcwError::InvalidConfig(format!(
                "l0_capacity ({}) 必须 < l1_capacity ({})",
                self.l0_capacity, self.l1_capacity
            )));
        }
        if self.l1_capacity >= self.l2_capacity {
            return Err(HcwError::InvalidConfig(format!(
                "l1_capacity ({}) 必须 < l2_capacity ({})",
                self.l1_capacity, self.l2_capacity
            )));
        }
        if self.l2_capacity >= self.l3_capacity {
            return Err(HcwError::InvalidConfig(format!(
                "l2_capacity ({}) 必须 < l3_capacity ({})",
                self.l2_capacity, self.l3_capacity
            )));
        }
        if !(0.0 < self.compression_threshold && self.compression_threshold <= 1.0) {
            return Err(HcwError::InvalidConfig(format!(
                "compression_threshold = {} 超出 (0.0, 1.0]",
                self.compression_threshold
            )));
        }
        // 校验 compressor_weights 为非负且和接近 1.0
        let (rw, fw, rlw) = self.compressor_weights;
        if rw < 0.0 || fw < 0.0 || rlw < 0.0 {
            return Err(HcwError::InvalidConfig(
                "compressor_weights 不能为负".into(),
            ));
        }
        let sum = rw + fw + rlw;
        if (sum - 1.0).abs() > 1e-3 {
            return Err(HcwError::InvalidConfig(format!(
                "compressor_weights 之和必须 ≈ 1.0, 当前: {sum}"
            )));
        }
        Ok(())
    }

    /// 获取指定层级的标称容量(含 L3 的 1M 等效值)
    pub fn capacity_for(&self, tier: WindowTier) -> usize {
        tier.capacity(self)
    }

    /// 获取指定层级的实际加载容量
    ///
    /// WHY:L3 的实际加载容量 = l3_capacity / 8 = 128K,
    /// 通过 OSA 稀疏化(8× 压缩比)实现 1M 等效,避免暴力加载(架构红线)。
    /// L0/L1/L2 的实际容量 = 标称容量(无稀疏化)
    pub fn effective_capacity_for(&self, tier: WindowTier) -> usize {
        tier.effective_capacity(self)
    }
}

impl Default for HcwConfig {
    /// 默认配置(对应架构手册 §HCW 四级窗口)
    fn default() -> Self {
        Self {
            l0_capacity: 4096,          // 4K Token,快速响应
            l1_capacity: 32768,         // 32K Token,常规任务
            l2_capacity: 131072,        // 128K Token,复杂任务
            l3_capacity: 1048576,       // 1M Token 等效(128K 实际加载 + 8× 稀疏化)
            compression_threshold: 0.9, // 容量利用率达 90% 触发压缩
            compressor_weights: (0.4, 0.3, 0.3),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = HcwConfig::default();
        assert_eq!(config.l0_capacity, 4096);
        assert_eq!(config.l1_capacity, 32768);
        assert_eq!(config.l2_capacity, 131072);
        assert_eq!(config.l3_capacity, 1048576);
        assert!((config.compression_threshold - 0.9).abs() < 1e-6);
    }

    #[test]
    fn test_validate_valid() {
        let config = HcwConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_zero_capacity() {
        let config = HcwConfig::default().with_l0_capacity(0);
        let err = config.validate().unwrap_err();
        assert!(matches!(err, HcwError::InvalidConfig(_)));
    }

    #[test]
    fn test_validate_non_increasing_capacities() {
        let config = HcwConfig::default().with_l1_capacity(4096); // l1 == l0
        let err = config.validate().unwrap_err();
        assert!(matches!(err, HcwError::InvalidConfig(_)));
    }

    #[test]
    fn test_validate_invalid_threshold_zero() {
        let config = HcwConfig::default().with_compression_threshold(0.0);
        let err = config.validate().unwrap_err();
        assert!(matches!(err, HcwError::InvalidConfig(_)));
    }

    #[test]
    fn test_validate_invalid_threshold_over_one() {
        let config = HcwConfig::default().with_compression_threshold(1.5);
        let err = config.validate().unwrap_err();
        assert!(matches!(err, HcwError::InvalidConfig(_)));
    }

    #[test]
    fn test_validate_threshold_one_is_valid() {
        let config = HcwConfig::default().with_compression_threshold(1.0);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_capacity_for() {
        let config = HcwConfig::default();
        assert_eq!(config.capacity_for(WindowTier::L0), 4096);
        assert_eq!(config.capacity_for(WindowTier::L1), 32768);
        assert_eq!(config.capacity_for(WindowTier::L2), 131072);
        assert_eq!(config.capacity_for(WindowTier::L3), 1048576);
    }

    #[test]
    fn test_effective_capacity_for() {
        let config = HcwConfig::default();
        // L0/L1/L2:实际容量 = 标称容量
        assert_eq!(config.effective_capacity_for(WindowTier::L0), 4096);
        assert_eq!(config.effective_capacity_for(WindowTier::L1), 32768);
        assert_eq!(config.effective_capacity_for(WindowTier::L2), 131072);
        // L3:实际加载容量 = 1M / 8 = 128K(通过 8× 稀疏化实现 1M 等效)
        assert_eq!(config.effective_capacity_for(WindowTier::L3), 131072);
    }

    #[test]
    fn test_builder_chain() {
        let config = HcwConfig::new()
            .with_l0_capacity(2048)
            .with_l1_capacity(16384)
            .with_l2_capacity(65536)
            .with_l3_capacity(524288)
            .with_compression_threshold(0.8);
        assert_eq!(config.l0_capacity, 2048);
        assert_eq!(config.l3_capacity, 524288);
        assert!((config.compression_threshold - 0.8).abs() < 1e-6);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_validate_compressor_weights_negative() {
        let config = HcwConfig {
            compressor_weights: (-0.1, 0.5, 0.6),
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(matches!(err, HcwError::InvalidConfig(_)));
    }

    #[test]
    fn test_validate_compressor_weights_sum_not_one() {
        let config = HcwConfig {
            compressor_weights: (0.5, 0.5, 0.5),
            ..Default::default()
        };
        let err = config.validate().unwrap_err();
        assert!(matches!(err, HcwError::InvalidConfig(_)));
    }
}
