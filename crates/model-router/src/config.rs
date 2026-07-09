//! 路由器配置 — 模型列表、默认策略与 CACR 配置
//!
//! 对应架构:L1 Core,可从 YAML/TOML/JSON 配置文件加载

use serde::{Deserialize, Serialize};

use crate::cacr::CacrConfig;
use crate::types::{ModelInfo, RoutingStrategy};

/// 路由器配置 — 持有模型列表、默认路由策略、CACR 成本保护与 MoE 门控配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouterConfig {
    /// 已注册模型列表
    pub models: Vec<ModelInfo>,
    /// 默认路由策略(当请求未指定策略时使用)
    pub default_strategy: RoutingStrategy,
    /// CACR(Cost-Aware Cognitive Routing)成本保护配置
    ///
    /// WHY:嵌入 RouterConfig 而非独立加载,保证配置单一入口。
    /// 序列化时随 RouterConfig 一起持久化,部署时可通过配置文件调整阈值。
    #[serde(default)]
    pub cacr: CacrConfig,
    /// MoE 稀疏门控触发阈值 — 模型数 < 此值时退化为全量评估(默认 50)
    ///
    /// WHY `#[serde(default)]`:旧配置文件无此字段时使用默认值 50,
    /// 保证向后兼容(与 cacr 字段一致的渐进式设计)。
    #[serde(default = "default_moe_threshold")]
    pub moe_threshold: usize,
    /// MoE Top-K 激活数量 — 门控后保留的候选数(默认 5)
    ///
    /// WHY top_k=5:在保证召回真正 Top-1 的前提下尽量减少完整评估工作量。
    /// 详见 `moe.rs` 模块文档。
    #[serde(default = "default_moe_top_k")]
    pub moe_top_k: usize,
}

/// `moe_threshold` 的 serde 默认值函数(与 `MoeGate::default` 保持一致)
fn default_moe_threshold() -> usize {
    50
}

/// `moe_top_k` 的 serde 默认值函数(与 `MoeGate::default` 保持一致)
fn default_moe_top_k() -> usize {
    5
}

impl Default for RouterConfig {
    /// 默认配置包含三个分层模型,覆盖轻量/效率/高质量场景
    ///
    /// WHY:三模型对应三策略的典型选择,作为开箱即用的基线配置:
    /// - lite-model:本地小模型,成本极低、延迟极低,质量一般
    /// - efficient-model:OpenAI 中端模型,延迟适中,质量较好
    /// - premium-model:Anthropic 旗舰模型,延迟较高,质量最佳
    ///
    /// CACR 配置使用 `CacrConfig::default()`(10000 美元预算,0.8/1.0 阈值)。
    /// MoE 配置使用默认值(threshold=50, top_k=5,3 模型走退化路径)。
    fn default() -> Self {
        Self {
            models: vec![
                ModelInfo {
                    model_id: "lite-model".into(),
                    provider: "local".into(),
                    cost_per_1k_tokens: 0.0001,
                    avg_latency_ms: 100,
                    max_context: 4096,
                    quality_score: 0.6,
                },
                ModelInfo {
                    model_id: "efficient-model".into(),
                    provider: "openai".into(),
                    cost_per_1k_tokens: 0.002,
                    avg_latency_ms: 300,
                    max_context: 16384,
                    quality_score: 0.8,
                },
                ModelInfo {
                    model_id: "premium-model".into(),
                    provider: "anthropic".into(),
                    cost_per_1k_tokens: 0.015,
                    avg_latency_ms: 800,
                    max_context: 200000,
                    quality_score: 0.95,
                },
            ],
            default_strategy: RoutingStrategy::Auto,
            cacr: CacrConfig::default(),
            moe_threshold: 50,
            moe_top_k: 5,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_has_three_models() {
        let config = RouterConfig::default();
        assert_eq!(config.models.len(), 3);
        assert_eq!(config.default_strategy, RoutingStrategy::Auto);
    }

    #[test]
    fn test_default_config_model_ids() {
        let config = RouterConfig::default();
        let ids: Vec<&str> = config.models.iter().map(|m| m.model_id.as_str()).collect();
        assert_eq!(ids, vec!["lite-model", "efficient-model", "premium-model"]);
    }

    #[test]
    fn test_config_serde_roundtrip() {
        let config = RouterConfig::default();
        let json = serde_json::to_string(&config).unwrap();
        let de: RouterConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(de.models.len(), config.models.len());
        assert_eq!(de.default_strategy, config.default_strategy);
    }

    #[test]
    fn test_default_config_has_cacr() {
        let config = RouterConfig::default();
        assert_eq!(config.cacr.budget_limit, 1_000_000);
        assert!((config.cacr.warn_threshold - 0.8).abs() < f32::EPSILON);
        assert!((config.cacr.block_threshold - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_config_serde_preserves_cacr() {
        let config = RouterConfig {
            cacr: CacrConfig {
                budget_limit: 5000,
                warn_threshold: 0.7,
                block_threshold: 0.9,
            },
            ..Default::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let de: RouterConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(de.cacr.budget_limit, 5000);
        assert!((de.cacr.warn_threshold - 0.7).abs() < f32::EPSILON);
        assert!((de.cacr.block_threshold - 0.9).abs() < f32::EPSILON);
    }

    #[test]
    fn test_config_serde_backward_compatible_without_cacr() {
        // WHY:旧配置文件可能没有 cacr 字段,#[serde(default)] 保证向后兼容
        let json = r#"{
            "models": [],
            "default_strategy": "Lite"
        }"#;
        let de: RouterConfig = serde_json::from_str(json).unwrap();
        assert_eq!(de.models.len(), 0);
        assert_eq!(de.default_strategy, RoutingStrategy::Lite);
        // cacr 字段缺失时使用默认值
        assert_eq!(de.cacr.budget_limit, 1_000_000);
    }

    #[test]
    fn test_default_config_has_moe_defaults() {
        let config = RouterConfig::default();
        assert_eq!(config.moe_threshold, 50);
        assert_eq!(config.moe_top_k, 5);
    }

    #[test]
    fn test_config_serde_preserves_moe() {
        let config = RouterConfig {
            moe_threshold: 100,
            moe_top_k: 3,
            ..Default::default()
        };
        let json = serde_json::to_string(&config).unwrap();
        let de: RouterConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(de.moe_threshold, 100);
        assert_eq!(de.moe_top_k, 3);
    }

    #[test]
    fn test_config_serde_backward_compatible_without_moe() {
        // WHY:旧配置文件可能没有 moe 字段,#[serde(default)] 保证向后兼容
        let json = r#"{
            "models": [],
            "default_strategy": "Lite"
        }"#;
        let de: RouterConfig = serde_json::from_str(json).unwrap();
        assert_eq!(de.moe_threshold, 50);
        assert_eq!(de.moe_top_k, 5);
    }
}
