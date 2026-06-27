//! GEA 配置类型 — 门控计算权重、阈值与缓存参数
//!
//! 对应架构层:L6 Router
//! 对应创新点:GEA(Gated Expert Activation)
//!
//! # 设计决策(WHY)
//! - 权重 w1+w2+w3 默认和为 1.0,保证门控值归一化
//! - `activation_threshold` 默认 0.5:sigmoid 中点,平衡激活与抑制
//! - `cache_capacity` 默认 128:LRU 容量,平衡内存与命中率
//! - `overlap_threshold` 默认 0.8:专家向量重叠度高于此值视为功能冗余

use serde::{Deserialize, Serialize};

use crate::error::GeaError;

/// GEA 激活器配置
///
/// 所有字段均有合理默认值,可通过 `Default::default()` 快速构造。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeaConfig {
    /// 复杂度权重 w1(任务复杂度对门控值的影响)
    pub w1: f32,
    /// 相关性权重 w2(任务 CLV 与专家向量的余弦相似度)
    pub w2: f32,
    /// 亲和度权重 w3(能力标签匹配度)
    pub w3: f32,
    /// 门控偏置 bias(越大越难激活)
    pub bias: f32,
    /// 激活阈值:门控值 >= 此值才激活
    pub activation_threshold: f32,
    /// 激活缓存容量(LRU,条目数)
    pub cache_capacity: usize,
    /// 专家功能重叠阈值:CLV 余弦相似度 > 此值视为冗余
    pub overlap_threshold: f32,
    /// Top-K:最终激活的专家数量上限
    pub top_k: usize,
    /// 激活缓存 TTL(秒),默认 5 秒
    ///
    /// 缓存命中可跳过门控计算流程,直接返回缓存的激活结果。
    /// 调优示例:高并发场景可设为 10-30 秒,低延迟场景可设为 1-2 秒。
    #[serde(default = "default_cache_ttl_secs")]
    pub cache_ttl_secs: u64,
}

impl Default for GeaConfig {
    fn default() -> Self {
        Self {
            w1: 0.4,
            w2: 0.3,
            w3: 0.3,
            bias: 0.5,
            activation_threshold: 0.5,
            cache_capacity: 128,
            overlap_threshold: 0.8,
            top_k: 3,
            cache_ttl_secs: 5,
        }
    }
}

impl GeaConfig {
    /// 校验配置合法性
    ///
    /// WHY:在构造 GeaActivator 时调用,提前暴露配置错误,
    /// 避免运行时门控计算产生 NaN 或负值导致排序异常
    pub fn validate(&self) -> Result<(), GeaError> {
        // 权重应为非负,且和接近 1.0(允许浮点误差)
        if self.w1 < 0.0 || self.w2 < 0.0 || self.w3 < 0.0 {
            return Err(GeaError::ConfigError {
                detail: "weights must be non-negative".into(),
            });
        }
        let sum = self.w1 + self.w2 + self.w3;
        if (sum - 1.0).abs() > 1e-3 {
            return Err(GeaError::ConfigError {
                detail: format!("weights sum must be ~1.0, got {sum}"),
            });
        }
        if self.activation_threshold < 0.0 || self.activation_threshold > 1.0 {
            return Err(GeaError::ConfigError {
                detail: "activation_threshold must be in [0.0, 1.0]".into(),
            });
        }
        if self.overlap_threshold < 0.0 || self.overlap_threshold > 1.0 {
            return Err(GeaError::ConfigError {
                detail: "overlap_threshold must be in [0.0, 1.0]".into(),
            });
        }
        if self.cache_capacity == 0 {
            return Err(GeaError::ConfigError {
                detail: "cache_capacity must be > 0".into(),
            });
        }
        if self.top_k == 0 {
            return Err(GeaError::ConfigError {
                detail: "top_k must be > 0".into(),
            });
        }
        if self.cache_ttl_secs == 0 {
            return Err(GeaError::ConfigError {
                detail: "cache_ttl_secs must be > 0".into(),
            });
        }
        Ok(())
    }
}

fn default_cache_ttl_secs() -> u64 {
    5
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_values() {
        let cfg = GeaConfig::default();
        assert!((cfg.w1 - 0.4).abs() < 1e-6);
        assert!((cfg.w2 - 0.3).abs() < 1e-6);
        assert!((cfg.w3 - 0.3).abs() < 1e-6);
        assert!((cfg.bias - 0.5).abs() < 1e-6);
        assert!((cfg.activation_threshold - 0.5).abs() < 1e-6);
        assert_eq!(cfg.cache_capacity, 128);
        assert!((cfg.overlap_threshold - 0.8).abs() < 1e-6);
        assert_eq!(cfg.top_k, 3);
    }

    #[test]
    fn test_validate_ok() {
        let cfg = GeaConfig::default();
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn test_validate_negative_weight() {
        let cfg = GeaConfig {
            w1: -0.1,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_weights_sum_not_one() {
        let cfg = GeaConfig {
            w1: 0.5,
            w2: 0.5,
            w3: 0.5,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_threshold_out_of_range() {
        let cfg = GeaConfig {
            activation_threshold: 1.5,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_zero_cache_capacity() {
        let cfg = GeaConfig {
            cache_capacity: 0,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_validate_zero_top_k() {
        let cfg = GeaConfig {
            top_k: 0,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }

    #[test]
    fn test_serde_roundtrip() {
        let cfg = GeaConfig::default();
        let json = serde_json::to_string(&cfg).unwrap();
        let restored: GeaConfig = serde_json::from_str(&json).unwrap();
        assert!((cfg.w1 - restored.w1).abs() < 1e-6);
        assert_eq!(cfg.cache_capacity, restored.cache_capacity);
    }

    #[test]
    fn test_validate_zero_cache_ttl() {
        let cfg = GeaConfig {
            cache_ttl_secs: 0,
            ..Default::default()
        };
        assert!(cfg.validate().is_err());
    }
}
