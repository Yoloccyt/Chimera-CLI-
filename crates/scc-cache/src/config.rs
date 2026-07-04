//! SCC 配置 — 推测上下文缓存的容量与预取参数
//!
//! 对应架构层:L3 Storage
//!
//! # 设计决策(WHY)
//! - **capacity 默认 256**:与 cmt-tiering HotTier 保持一致,平衡命中率与内存占用。
//!   每条目约 1KB(content `Arc<str>`),256 条目约 256KB,可容纳当前活跃上下文集合
//! - **prefetch_threshold 默认 0.6**:平衡预取命中率与预取消耗。阈值过低(如 0.3)
//!   导致大量低概率预取浪费资源;阈值过高(如 0.9)导致预取过于保守,
//!   错失预取机会。0.6 来自 spec.md 的设计决策
//! - **prefetch_enabled 默认 true**:预取是 SCC 的核心创新点,默认启用

use serde::{Deserialize, Serialize};

/// SCC 引擎配置 — 容量与预取参数
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SccConfig {
    /// 缓存容量上限(默认 256)
    ///
    /// WHY:256 条目平衡命中率与内存占用。超出时按 LRU 策略驱逐最久未访问条目,
    /// 但 Arc::strong_count > 1 的条目(被 Producer/Verifier 引用)不会被驱逐
    pub capacity: usize,

    /// 预取概率阈值(默认 0.6)
    ///
    /// WHY:访问模式预测概率 > 此阈值的上下文会被异步预取(预热)到缓存。
    /// 0.6 来自 spec.md 设计决策,平衡预取命中率与预取消耗
    pub prefetch_threshold: f32,

    /// 是否启用推测性预取(默认 true)
    ///
    /// WHY:预取是 SCC 核心创新点,默认启用。可在调试或性能测试时关闭
    pub prefetch_enabled: bool,
}

impl SccConfig {
    /// 创建默认配置
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置缓存容量
    pub fn with_capacity(mut self, capacity: usize) -> Self {
        self.capacity = capacity;
        self
    }

    /// 设置预取阈值
    pub fn with_prefetch_threshold(mut self, threshold: f32) -> Self {
        self.prefetch_threshold = threshold;
        self
    }

    /// 设置是否启用预取
    pub fn with_prefetch_enabled(mut self, enabled: bool) -> Self {
        self.prefetch_enabled = enabled;
        self
    }
}

impl Default for SccConfig {
    fn default() -> Self {
        Self {
            capacity: 256,
            prefetch_threshold: 0.6,
            prefetch_enabled: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = SccConfig::default();
        assert_eq!(config.capacity, 256);
        assert!((config.prefetch_threshold - 0.6).abs() < f32::EPSILON);
        assert!(config.prefetch_enabled);
    }

    #[test]
    fn test_builder_chain() {
        let config = SccConfig::new()
            .with_capacity(128)
            .with_prefetch_threshold(0.8)
            .with_prefetch_enabled(false);
        assert_eq!(config.capacity, 128);
        assert!((config.prefetch_threshold - 0.8).abs() < f32::EPSILON);
        assert!(!config.prefetch_enabled);
    }

    #[test]
    fn test_config_serialization_roundtrip() {
        let config = SccConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let decoded: SccConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(config, decoded);
    }
}
