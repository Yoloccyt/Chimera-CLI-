//! 通道化路由决策单元 — 整合 RouteTarget 与路由元数据(ADR-065 M3)
//!
//! 对应架构层:L1 Core(model-router)
//! 对应设计源:`Chimera_全模型亲和适配体系设计文档_v1.0.md` §5.4 路由亲和
//!
//! # 核心职责
//! `ModelRoute` 是 model-router 路由决策的最终输出单元，替代旧的 model_id 字符串。
//! 每个 ModelRoute 对应一张 affinity.d/*.toml 卡片，包含路由所需全部元数据。
//!
//! # 依赖方向
//! 本模块依赖 L0 `nexus_contracts::affinity`(L1 → L0 合规)，不依赖 L10。

use crate::route_target::RouteTarget;
use nexus_contracts::affinity::{ModelAffinitySpec, ThinkingPreference};
use serde::{Deserialize, Serialize};

/// 通道化路由决策单元 — 整合 RouteTarget 与路由元数据(ADR-065 M3)
///
/// 是 model-router 路由决策的最终输出单元，替代旧的 model_id 字符串。
/// 每个 ModelRoute 对应一张 affinity.d/*.toml 卡片，包含路由所需全部元数据。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelRoute {
    /// 路由目标三元组(provider × model × thinking_mode)
    pub target: RouteTarget,
    /// 每千 token 成本(美元)
    pub cost_per_1k_tokens: f64,
    /// 平均延迟(毫秒)
    pub avg_latency_ms: u64,
    /// 最大上下文长度(token 数)
    pub max_context: u32,
    /// 质量评分 [0.0, 1.0]
    pub quality_score: f32,
    /// 是否支持工具调用
    pub tool_calling: bool,
    /// 是否支持思考
    pub thinking_supported: bool,
}

impl ModelRoute {
    /// 构造一个新的 ModelRoute
    pub fn new(
        target: RouteTarget,
        cost_per_1k_tokens: f64,
        avg_latency_ms: u64,
        max_context: u32,
        quality_score: f32,
        tool_calling: bool,
        thinking_supported: bool,
    ) -> Self {
        Self {
            target,
            cost_per_1k_tokens,
            avg_latency_ms,
            max_context,
            quality_score,
            tool_calling,
            thinking_supported,
        }
    }

    /// 通道路由键 — 委托给 `target.route_key()`
    ///
    /// 返回 `provider/model` 格式，与 mca-gateway 通道注册表键一致。
    pub fn route_key(&self) -> String {
        self.target.route_key()
    }

    /// 学习臂标识 — 委托给 `target.arm_id()`
    ///
    /// 返回 `provider/model/mode` 三段编码，供 omega-learner LinUCB 臂空间使用。
    pub fn arm_id(&self) -> String {
        self.target.arm_id()
    }

    /// 预估成本(美分)
    ///
    /// 公式: `(tokens / 1000) * cost_per_1k_tokens * 100`
    /// 与 `strategies::estimate_cost` 一致，确保成本计算单一来源。
    pub fn estimate_cost(&self, tokens: u32) -> u64 {
        let cost_usd = (tokens as f64 / 1000.0) * self.cost_per_1k_tokens;
        (cost_usd * 100.0).round() as u64
    }

    /// 从亲和 spec 构造 ModelRoute
    ///
    /// 根据 `ModelAffinitySpec` 与 `ThinkingPreference` 生成路由决策单元。
    /// `thinking_supported` 从 spec 的能力集推导：`ThinkingSupport` 非 None 即可。
    /// `tool_calling` 从 spec 的 `capabilities.tool_calling` 直接映射。
    /// 成本取 pricing 的 input 价(未含峰谷系数)，后续由 CACR 做精细调整。
    pub fn from_spec(spec: &ModelAffinitySpec, thinking: ThinkingPreference) -> Self {
        let target = RouteTarget::new(spec.provider.clone(), spec.model.clone(), thinking);

        // 成本取 input 价(微元/百万 token)转为美元/千 token
        // 公式: micro_per_mtok / 1_000_000 * 1000 / 1000 = micro_per_mtok / 1_000_000_000
        // 简化: (micro_per_mtok as f64) / 1_000_000.0 (美元/百万 token)，再除以 1000 得美元/千 token
        // 但这里直接转为美元/千 token 的 f64
        let cost_per_1k = (spec.pricing.input_micro_per_mtok as f64) / 1_000_000.0 / 1000.0;

        Self {
            target,
            cost_per_1k_tokens: cost_per_1k,
            avg_latency_ms: spec.endpoint.timeout_ms / 2, // 保守估计：超时的一半
            max_context: spec.capabilities.context_window,
            quality_score: if spec.capabilities.tool_calling {
                0.8
            } else {
                0.6
            },
            tool_calling: spec.capabilities.tool_calling,
            thinking_supported: spec.capabilities.thinking.is_supported(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_contracts::affinity::{
        CapabilitySet, ModelAffinitySpec, ProtocolDialect, ProviderId, ThinkingSupport,
    };

    fn sample_spec() -> ModelAffinitySpec {
        ModelAffinitySpec::minimal(ProviderId::Zhipu, "glm-5.2", ProtocolDialect::OpenAiChat)
    }

    fn spec_with_thinking() -> ModelAffinitySpec {
        let mut spec = sample_spec();
        spec.capabilities = CapabilitySet {
            streaming: true,
            tool_calling: true,
            thinking: ThinkingSupport::EffortLevels(vec![
                "none".into(),
                "medium".into(),
                "xhigh".into(),
                "max".into(),
            ]),
            context_window: 1_000_000,
            max_output: 128_000,
            prompt_caching: nexus_contracts::affinity::CacheSupport::ExplicitControl,
            service_tiers: Vec::new(),
            state_preservation: nexus_contracts::affinity::StatePreservationPolicy::None,
            modalities: vec![nexus_contracts::affinity::ModalityKind::Text],
            structured_output: true,
        };
        spec
    }

    #[test]
    fn test_model_route_from_spec() {
        let spec = spec_with_thinking();
        let route = ModelRoute::from_spec(&spec, ThinkingPreference::Deep);

        // 验证 route_key 和 arm_id 委托正确
        assert_eq!(route.route_key(), "zhipu/glm-5.2");
        assert_eq!(route.arm_id(), "zhipu/glm-5.2/deep");

        // 验证能力映射
        assert!(route.tool_calling);
        assert!(route.thinking_supported);
        assert_eq!(route.max_context, 1_000_000);
    }

    #[test]
    fn test_model_route_from_spec_minimal() {
        let spec = sample_spec();
        let route = ModelRoute::from_spec(&spec, ThinkingPreference::Fast);

        // 最小 spec 不支持工具调用与思考
        assert!(!route.tool_calling);
        assert!(!route.thinking_supported);
        assert_eq!(route.max_context, 4096);
        assert_eq!(route.arm_id(), "zhipu/glm-5.2/fast");
    }

    #[test]
    fn test_estimate_cost() {
        let target = RouteTarget::new(
            ProviderId::DeepSeek,
            "deepseek-v4-flash",
            ThinkingPreference::Standard,
        );
        let route = ModelRoute::new(target.clone(), 0.001, 100, 32768, 0.9, true, true);

        // 1000 tokens * $0.001/1k * 100 = 0.1 美分 -> 0
        assert_eq!(route.estimate_cost(1000), 0);
        // 10000 tokens * $0.001/1k * 100 = 1 美分
        assert_eq!(route.estimate_cost(10000), 1);
        // 1000 tokens * $0.015/1k * 100 = 1.5 -> 2
        let route2 = ModelRoute::new(target, 0.015, 100, 32768, 0.9, true, true);
        assert_eq!(route2.estimate_cost(1000), 2);
    }

    #[test]
    fn test_serde_roundtrip() {
        let target = RouteTarget::new(
            ProviderId::MiniMax,
            "MiniMax-M3",
            ThinkingPreference::Standard,
        );
        let route = ModelRoute::new(target, 0.002, 200, 128000, 0.85, true, true);

        // MessagePack 序列化/反序列化
        let bytes = rmp_serde::to_vec(&route).unwrap();
        let back: ModelRoute = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(route.target, back.target);
        assert!((route.cost_per_1k_tokens - back.cost_per_1k_tokens).abs() < 1e-12);
        assert_eq!(route.avg_latency_ms, back.avg_latency_ms);
        assert_eq!(route.max_context, back.max_context);
        assert_eq!(route.tool_calling, back.tool_calling);
        assert_eq!(route.thinking_supported, back.thinking_supported);
    }

    #[test]
    fn test_route_key_delegation() {
        let target = RouteTarget::new(
            ProviderId::Custom("openrouter".into()),
            "anthropic/claude-x",
            ThinkingPreference::Standard,
        );
        let route = ModelRoute::new(target, 0.005, 150, 100000, 0.95, true, true);
        assert_eq!(route.route_key(), "openrouter/anthropic/claude-x");
        assert_eq!(route.arm_id(), "openrouter/anthropic/claude-x/standard");
    }
}
