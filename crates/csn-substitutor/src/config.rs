//! CSN 配置定义
//!
//! 控制语义向量维度、注册表容量与默认降级层级。
//! 配置项默认值经过权衡,适合大多数 L10 Interface 层能力替代场景。

use serde::{Deserialize, Serialize};

/// CSN 配置 — 控制能力注册表与降级链行为
///
/// 所有字段均有合理默认值,可通过 `Default::default()` 快速创建。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CsnConfig {
    /// 语义向量维度
    ///
    /// 默认 50,与 CSN 设计文档对齐(100 能力 × 50 维 in-memory)。
    /// WHY 50:平衡语义表达力与内存占用,50 维足以区分典型工具能力;
    /// 每个能力约 200 字节,100 能力约 20KB,可全部驻留 L1 缓存。
    pub vector_dimension: usize,

    /// 注册表容量上限
    ///
    /// 默认 100,对应 CSN 设计目标的"100 能力"。
    /// WHY 100:典型 CLI 工具场景下,活跃能力约 50-100 个,
    /// 100 容量可覆盖大多数工作集;超出时调用方应先驱逐冷能力。
    pub registry_capacity: usize,

    /// 默认降级层级列表(≥ 3 级)
    ///
    /// 默认 ["primary", "secondary", "tertiary"],对应 3 级降级。
    /// WHY ≥ 3:架构红线要求降级链深度 ≥ 3 级,确保单点失败后仍有
    /// 多级回退,避免"一次失败即放弃"的脆弱性。
    pub default_degradation_levels: Vec<String>,

    /// 默认 Top-K 值
    ///
    /// 默认 5,从注册表中选出相似度最高的 5 个候选。
    /// WHY 5:与 FaaE/GEA 的 Top-K=8 对齐(略低,因 CSN 仅在失败时触发),
    /// 过大增加选择开销,过小丢失可选替代。
    pub top_k: usize,
}

impl Default for CsnConfig {
    fn default() -> Self {
        Self {
            vector_dimension: 50,
            registry_capacity: 100,
            default_degradation_levels: vec![
                "primary".into(),
                "secondary".into(),
                "tertiary".into(),
            ],
            top_k: 5,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config() {
        let config = CsnConfig::default();
        assert_eq!(config.vector_dimension, 50);
        assert_eq!(config.registry_capacity, 100);
        assert_eq!(
            config.default_degradation_levels.len(),
            3,
            "默认 ≥ 3 级降级"
        );
        assert_eq!(config.top_k, 5);
    }

    #[test]
    fn test_default_degradation_levels_at_least_three() {
        let config = CsnConfig::default();
        assert!(
            config.default_degradation_levels.len() >= 3,
            "降级链深度必须 ≥ 3 级(架构红线)"
        );
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let config = CsnConfig {
            vector_dimension: 64,
            registry_capacity: 200,
            default_degradation_levels: vec!["L1".into(), "L2".into(), "L3".into(), "L4".into()],
            top_k: 10,
        };
        let json = serde_json::to_string(&config).expect("序列化失败");
        let restored: CsnConfig = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(restored.vector_dimension, 64);
        assert_eq!(restored.registry_capacity, 200);
        assert_eq!(restored.default_degradation_levels.len(), 4);
        assert_eq!(restored.top_k, 10);
    }

    #[test]
    fn test_config_clone() {
        let config = CsnConfig::default();
        let cloned = config.clone();
        assert_eq!(config.vector_dimension, cloned.vector_dimension);
        assert_eq!(config.registry_capacity, cloned.registry_capacity);
        assert_eq!(
            config.default_degradation_levels,
            cloned.default_degradation_levels
        );
    }
}
