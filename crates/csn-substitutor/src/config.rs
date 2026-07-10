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

    /// P0-14:启用LSH-ANN索引的阈值
    ///
    /// 当注册能力数≥此值时启用LSH-ANN近似最近邻索引,
    /// 低于此值时使用线性扫描(小数据量时LSH开销不划算)。
    /// 默认20,与mlc-engine的LSH_ENABLE_THRESHOLD(1000)不同,
    /// 因CSN规模较小(100能力),更早启用索引收益更大。
    pub lsh_enable_threshold: usize,

    /// P0-14:LSH索引表数
    ///
    /// 更多表=更高召回率+更多内存。默认8(平衡召回与内存)。
    pub lsh_num_tables: usize,

    /// P0-14:LSH每表哈希bit数
    ///
    /// 更多bit=更精确+更少碰撞。默认16(平衡精度与候选集大小)。
    pub lsh_hash_bits: usize,
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
            lsh_enable_threshold: 20,
            lsh_num_tables: 8,
            lsh_hash_bits: 16,
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
        assert_eq!(config.lsh_enable_threshold, 20);
        assert_eq!(config.lsh_num_tables, 8);
        assert_eq!(config.lsh_hash_bits, 16);
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
            lsh_enable_threshold: 10,
            lsh_num_tables: 4,
            lsh_hash_bits: 8,
        };
        let json = serde_json::to_string(&config).expect("序列化失败");
        let restored: CsnConfig = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(restored.vector_dimension, 64);
        assert_eq!(restored.registry_capacity, 200);
        assert_eq!(restored.default_degradation_levels.len(), 4);
        assert_eq!(restored.lsh_enable_threshold, 10);
        assert_eq!(restored.lsh_num_tables, 4);
        assert_eq!(restored.lsh_hash_bits, 8);
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
        assert_eq!(config.lsh_enable_threshold, cloned.lsh_enable_threshold);
        assert_eq!(config.lsh_num_tables, cloned.lsh_num_tables);
        assert_eq!(config.lsh_hash_bits, cloned.lsh_hash_bits);
    }
}
