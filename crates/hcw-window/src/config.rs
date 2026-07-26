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
use nexus_contracts::SelectorPolicy;

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

    /// 设置选择器策略(链式 builder)— P3-W10.3 D1 修复
    ///
    /// 注入 `SelectorPolicy::Static(常量)` 或 `SelectorPolicy::Learned { 版本号, 权重 }`。
    /// 默认值 = `SelectorPolicy::default()` = `Static(0.4, 0.3, 0.3)`(fallback 编译进二进制)。
    ///
    /// WHY(P4 接缝):`omega-learner` Bandit 算法(P4-W13.3 S4 接缝)异步下发
    /// `SelectorPolicy::Learned { version, weights }` 替代默认 Static 常量,
    /// learner panic/超时时调用方本地 fallback 到 `SelectorPolicy::Static`。
    ///
    /// # 示例
    /// ```no_run
    /// use hcw_window::HcwConfig;
    /// use nexus_contracts::{SelectorPolicy, SelectorWeights};
    ///
    /// // 静态策略(自定义权重)
    /// let config = HcwConfig::default()
    ///     .with_selector_policy(SelectorPolicy::static_policy(
    ///         SelectorWeights::new(0.5, 0.3, 0.2),
    ///     ));
    ///
    /// // 学习策略(omega-learner 异步下发)
    /// let config = HcwConfig::default()
    ///     .with_selector_policy(SelectorPolicy::learned(42, SelectorWeights::new(0.5, 0.3, 0.2)));
    /// ```
    pub fn with_selector_policy(mut self, policy: SelectorPolicy) -> Self {
        self.selector_policy = policy;
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
        // 校验 selector_policy 权重为非负且和接近 1.0(P3-W10.3 D1 修复)
        // 委托给 SelectorPolicy::is_valid()(内部校验 SelectorWeights 非负 + 和 ≈ 1.0)
        if !self.selector_policy.is_valid() {
            return Err(HcwError::InvalidConfig(format!(
                "selector_policy 权重非法(非负或和 ≠ 1.0): {:?}",
                self.selector_policy.weights()
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
    ///
    /// WHY(P3-W10.3 D1 修复):`selector_policy` 默认 = `SelectorPolicy::default()`
    /// = `Static(SelectorWeights::DEFAULT)` = `Static(0.4, 0.3, 0.3)`,
    /// 等于原 `compressor_weights: (0.4, 0.3, 0.3)` 默认值(fallback 编译进二进制,C4 合规)。
    fn default() -> Self {
        Self {
            l0_capacity: 4096,                          // 4K Token,快速响应
            l1_capacity: 32768,                         // 32K Token,常规任务
            l2_capacity: 131072,                        // 128K Token,复杂任务
            l3_capacity: 1048576,                       // 1M Token 等效(128K 实际加载 + 8× 稀疏化)
            compression_threshold: 0.9,                 // 容量利用率达 90% 触发压缩
            selector_policy: SelectorPolicy::default(), // P3-W10.3: Static(0.4, 0.3, 0.3) fallback
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
    fn test_validate_selector_policy_negative_weights() {
        // P3-W10.3: selector_policy 权重为负值应校验失败
        let config = HcwConfig::default().with_selector_policy(SelectorPolicy::static_policy(
            nexus_contracts::SelectorWeights::new(-0.1, 0.5, 0.6),
        ));
        let err = config.validate().unwrap_err();
        assert!(matches!(err, HcwError::InvalidConfig(_)));
    }

    #[test]
    fn test_validate_selector_policy_sum_not_one() {
        // P3-W10.3: selector_policy 权重和 ≠ 1.0 应校验失败
        let config = HcwConfig::default().with_selector_policy(SelectorPolicy::static_policy(
            nexus_contracts::SelectorWeights::new(0.5, 0.5, 0.5),
        ));
        let err = config.validate().unwrap_err();
        assert!(matches!(err, HcwError::InvalidConfig(_)));
    }

    #[test]
    fn test_validate_selector_policy_default_valid() {
        // P3-W10.3: 默认 selector_policy(Static 0.4, 0.3, 0.3)应校验通过
        let config = HcwConfig::default();
        assert!(config.validate().is_ok());
        assert!(config.selector_policy.is_static());
    }

    #[test]
    fn test_validate_selector_policy_learned_valid() {
        // P3-W10.3: Learned 策略(合法权重)应校验通过
        let config = HcwConfig::default().with_selector_policy(SelectorPolicy::learned(
            1,
            nexus_contracts::SelectorWeights::new(0.5, 0.3, 0.2),
        ));
        assert!(config.validate().is_ok());
        assert!(config.selector_policy.is_learned());
    }

    #[test]
    fn test_with_selector_policy_builder() {
        // P3-W10.3: with_selector_policy builder 链式调用
        let policy =
            SelectorPolicy::learned(42, nexus_contracts::SelectorWeights::new(0.5, 0.3, 0.2));
        let config = HcwConfig::default().with_selector_policy(policy);
        assert_eq!(config.selector_policy, policy);
        assert_eq!(config.selector_policy.version(), Some(42));
    }

    // ============================================================
    // P3-W10.3 D1 修复验收测试（spec.md:421 验证标准）
    // ============================================================

    #[test]
    fn test_d1_static_fallback_default_matches_const() {
        // spec.md:426 "默认静态值 = 当前常量，fallback 编译进同一二进制"
        // HcwConfig::default().selector_policy 应 = Static(0.4, 0.3, 0.3)
        let config = HcwConfig::default();
        assert!(config.selector_policy.is_static());

        // 验证 fallback 值 = 当前 hcw-window 硬编码常量 (0.4, 0.3, 0.3)
        let w = config.selector_policy.weights();
        assert!((w.recency - 0.4).abs() < 1e-6, "recency 应为 0.4");
        assert!((w.frequency - 0.3).abs() < 1e-6, "frequency 应为 0.3");
        assert!((w.relevance - 0.3).abs() < 1e-6, "relevance 应为 0.3");
    }

    #[test]
    fn test_d1_fallback_equals_selector_policy_default() {
        // HcwConfig::default().selector_policy == SelectorPolicy::default()
        let config = HcwConfig::default();
        assert_eq!(config.selector_policy, SelectorPolicy::default());
        // SelectorPolicy::default() == SelectorPolicy::fallback()（C4 合规）
        assert_eq!(config.selector_policy, SelectorPolicy::fallback());
    }

    #[test]
    fn test_d1_learner_panic_local_fallback() {
        // spec.md:289-290 "learner panic/超时时调用方本地 fallback 到 Static(常量)"
        // 模拟:omega-learner 下发 Learned 值后 panic，调用方本地 fallback 到 Static
        use nexus_contracts::SelectorWeights;

        // 1. omega-learner 异步下发 Learned 值
        let learned_weights = SelectorWeights::new(0.5, 0.3, 0.2);
        let learned_policy = SelectorPolicy::learned(1, learned_weights);
        let config_with_learned = HcwConfig::default().with_selector_policy(learned_policy);
        assert!(config_with_learned.selector_policy.is_learned());
        assert_eq!(config_with_learned.selector_policy.version(), Some(1));

        // 2. learner panic/超时 → 调用方本地 fallback 到 Static(常量)
        // WHY(C4 合规):fallback 在调用方(hcw-window)本地完成,不依赖 omega-learner crate 可用性
        let fallback_policy = SelectorPolicy::fallback();
        let config_fallback = HcwConfig::default().with_selector_policy(fallback_policy);
        assert!(config_fallback.selector_policy.is_static());
        assert_ne!(
            config_fallback.selector_policy.weights(),
            config_with_learned.selector_policy.weights()
        );

        // 3. fallback 后权重恢复为默认常量 (0.4, 0.3, 0.3)
        let w = config_fallback.selector_policy.weights();
        assert!((w.recency - 0.4).abs() < 1e-6);
        assert!((w.frequency - 0.3).abs() < 1e-6);
        assert!((w.relevance - 0.3).abs() < 1e-6);
    }

    #[test]
    fn test_d1_no_cross_crate_flag_propagation() {
        // spec.md:290 "无跨 crate 旗标"
        // SelectorPolicy 通过值注入(Copy 语义),不依赖全局 static 或 feature flag
        // 验证:不同 HcwConfig 实例的 selector_policy 互不影响(无全局状态)
        use nexus_contracts::SelectorWeights;

        let config_a = HcwConfig::default().with_selector_policy(SelectorPolicy::learned(
            1,
            SelectorWeights::new(0.5, 0.3, 0.2),
        ));
        let config_b = HcwConfig::default(); // 默认 Static

        // config_a 用 Learned,config_b 用 Static,互不影响
        assert!(config_a.selector_policy.is_learned());
        assert!(config_b.selector_policy.is_static());
        assert_ne!(config_a.selector_policy, config_b.selector_policy);
    }

    #[test]
    fn test_d1_serde_backward_compat_old_config_without_selector_policy() {
        // P3-W10.3 serde 向后兼容:旧配置文件(无 selector_policy 字段)反序列化时用默认值
        // 模拟旧配置 JSON(只有 l0/l1/l2/l3/compression_threshold,无 selector_policy)
        let old_config_json = r#"{
            "l0_capacity": 4096,
            "l1_capacity": 32768,
            "l2_capacity": 131072,
            "l3_capacity": 1048576,
            "compression_threshold": 0.9
        }"#;
        let config: HcwConfig = serde_json::from_str(old_config_json).unwrap();
        // selector_policy 字段缺失 → serde default → SelectorPolicy::default() = Static(0.4, 0.3, 0.3)
        assert!(config.selector_policy.is_static());
        let w = config.selector_policy.weights();
        assert!((w.recency - 0.4).abs() < 1e-6);
        assert!((w.frequency - 0.3).abs() < 1e-6);
        assert!((w.relevance - 0.3).abs() < 1e-6);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_d1_serde_roundtrip_learned_policy() {
        // P3-W10.3 serde 往返:Learned 策略序列化/反序列化保持一致
        use nexus_contracts::SelectorWeights;
        let config = HcwConfig::default().with_selector_policy(SelectorPolicy::learned(
            42,
            SelectorWeights::new(0.5, 0.3, 0.2),
        ));
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: HcwConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, deserialized);
        assert!(deserialized.selector_policy.is_learned());
        assert_eq!(deserialized.selector_policy.version(), Some(42));
    }
}
