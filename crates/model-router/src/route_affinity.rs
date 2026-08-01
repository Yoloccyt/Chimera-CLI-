//! 路由亲和元数据 — 能力协商结果在路由决策中的投射(ADR-065 M3)
//!
//! 对应架构层:L1 Core(model-router)
//! 对应设计源:`Chimera_全模型亲和适配体系设计文档_v1.0.md` §5.4 路由亲和
//!
//! # 核心职责
//! 在路由决策时，将能力协商结果(保真度、思考偏好、缓存策略)纳入权重计算，
//! 使路由能感知不同通道的能力差异。
//!
//! # 设计约束
//! 本模块不依赖 mca-gateway(L10)，仅消费 L0 nexus-contracts 的纯类型。
//! 能力协商结果通过事件(MCA 事件)传递到 model-router。
//! `NegotiationOutcome` 在此本地定义，因为 L0 契约层不含该类型
//! （它是 L10 mca-gateway 的协商产物，但本模块仅取其保真度等标量字段）。

use nexus_contracts::affinity::{
    CacheSupport, ModelAffinitySpec, NegotiationFidelity, ThinkingPreference,
};

/// 本地协商结果 — 能力协商的轻量级投射(不依赖 L10 mca-gateway)
///
/// WHY 本地定义而非从 L0 导入: `NegotiationOutcome` 属于 L10 mca-gateway
/// 的协商产物，但本模块仅需保真度与工具调用能力，不依赖完整协商结果。
/// 定义一个 L1 本地版本，保持依赖方向铁律(L1 → L0 合规，L1 → L10 禁止)。
#[derive(Debug, Clone, PartialEq)]
pub struct NegotiationOutcome {
    /// 三态保真度
    pub fidelity: NegotiationFidelity,
    /// 是否启用工具调用
    pub tool_calling_enabled: bool,
}

/// 路由亲和元数据 — 能力协商结果在路由决策中的投射(ADR-065 M3)
///
/// 在路由决策时，将 `mca-gateway` 的能力协商结果(保真度、思考偏好、缓存策略)
/// 纳入权重计算，使路由能感知不同通道的能力差异。
///
/// # 设计约束
/// 本模块不依赖 mca-gateway(L10)，仅消费 L0 nexus-contracts 的纯类型。
/// 能力协商结果通过事件(MCA 事件)传递到 model-router。
#[derive(Debug, Clone, PartialEq)]
pub struct RouteAffinity {
    /// 能力协商保真度
    pub fidelity: NegotiationFidelity,
    /// 思考偏好(映射后的)
    pub thinking_pref: ThinkingPreference,
    /// 缓存支持度
    pub cache_support: CacheSupport,
    /// 成本预估(微元)
    pub cost_estimate_micro: u64,
    /// 峰谷因子百分比(100 = 标准价)
    pub peak_factor_percent: u16,
    /// 是否启用降级绕过
    pub degraded_bypass: bool,
}

impl RouteAffinity {
    /// 默认构造函数 — FullFidelity, Standard, None, 0, 100, false
    ///
    /// 提供保守默认值，适用于未收到能力协商事件的场景。
    pub fn new() -> Self {
        Self {
            fidelity: NegotiationFidelity::FullFidelity,
            thinking_pref: ThinkingPreference::Standard,
            cache_support: CacheSupport::None,
            cost_estimate_micro: 0,
            peak_factor_percent: 100,
            degraded_bypass: false,
        }
    }

    /// 综合权重 [0.0, 1.0]
    ///
    /// 公式: `保真度×0.5 + 思考能力×0.3 + 缓存能力×0.2`
    ///
    /// # 权重分配说明
    /// - 保真度权重最高(0.5): 核心能力完整性是路由决策的首要因素
    /// - 思考能力(0.3): 仅次于保真度，影响深度推理任务
    /// - 缓存能力(0.2): 影响成本与延迟，但非核心能力
    ///
    /// # 保真度映射
    /// - `FullFidelity` → 1.0
    /// - `DegradedNotified` → 0.5
    /// - `ChannelRejected` → 0.0
    ///
    /// # 思考能力映射
    /// - `Deep` → 1.0
    /// - `Standard` → 0.7
    /// - `Fast` → 0.3
    ///
    /// # 缓存能力映射
    /// - `ExplicitControl` → 1.0
    /// - `Implicit` → 0.6
    /// - `None` → 0.0
    pub fn weight(&self) -> f64 {
        let fidelity_score = match self.fidelity {
            NegotiationFidelity::FullFidelity => 1.0,
            NegotiationFidelity::DegradedNotified => 0.5,
            NegotiationFidelity::ChannelRejected => 0.0,
        };

        let thinking_score = match self.thinking_pref {
            ThinkingPreference::Deep => 1.0,
            ThinkingPreference::Standard => 0.7,
            ThinkingPreference::Fast => 0.3,
        };

        let cache_score = match self.cache_support {
            CacheSupport::ExplicitControl => 1.0,
            CacheSupport::Implicit => 0.6,
            CacheSupport::None => 0.0,
        };

        fidelity_score * 0.5 + thinking_score * 0.3 + cache_score * 0.2
    }

    /// 从协商结果构造 RouteAffinity
    ///
    /// 将 `mca-gateway` 的协商结果(本地 `NegotiationOutcome`)与
    /// `ModelAffinitySpec` 的能力描述符映射为路由亲和元数据。
    ///
    /// # 映射规则
    /// - `fidelity` 直接从协商结果继承
    /// - `thinking_pref` 从请求的 `thinking_pref` 参数映射
    /// - `cache_support` 从 spec 的能力集映射
    /// - `cost_estimate_micro` 从 spec 定价取 input 价(微元/百万 token)
    /// - `peak_factor_percent` 默认 100，后续由 CACR 调整
    /// - `degraded_bypass` 在保真度为 `DegradedNotified` 时允许绕过
    pub fn from_negotiation(
        outcome: &NegotiationOutcome,
        spec: &ModelAffinitySpec,
        thinking_pref: ThinkingPreference,
    ) -> Self {
        Self {
            fidelity: outcome.fidelity,
            thinking_pref,
            cache_support: spec.capabilities.prompt_caching,
            cost_estimate_micro: spec.pricing.input_micro_per_mtok,
            peak_factor_percent: 100,
            degraded_bypass: outcome.fidelity == NegotiationFidelity::DegradedNotified,
        }
    }
}

impl Default for RouteAffinity {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_contracts::affinity::{
        CapabilitySet, ModelAffinitySpec, ProtocolDialect, ProviderId, ThinkingSupport,
    };

    fn sample_spec() -> ModelAffinitySpec {
        let mut spec = ModelAffinitySpec::minimal(
            ProviderId::DeepSeek,
            "deepseek-v4-flash",
            ProtocolDialect::OpenAiChat,
        );
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
            prompt_caching: CacheSupport::ExplicitControl,
            service_tiers: Vec::new(),
            state_preservation: nexus_contracts::affinity::StatePreservationPolicy::None,
            modalities: vec![nexus_contracts::affinity::ModalityKind::Text],
            structured_output: true,
        };
        spec
    }

    #[test]
    fn test_route_affinity_default() {
        let ra = RouteAffinity::new();
        assert_eq!(ra.fidelity, NegotiationFidelity::FullFidelity);
        assert_eq!(ra.thinking_pref, ThinkingPreference::Standard);
        assert_eq!(ra.cache_support, CacheSupport::None);
        assert_eq!(ra.cost_estimate_micro, 0);
        assert_eq!(ra.peak_factor_percent, 100);
        assert!(!ra.degraded_bypass);
    }

    #[test]
    fn test_route_affinity_default_trait() {
        let ra = RouteAffinity::default();
        assert_eq!(ra, RouteAffinity::new());
    }

    #[test]
    fn test_weight_full_fidelity() {
        let ra = RouteAffinity {
            fidelity: NegotiationFidelity::FullFidelity,
            thinking_pref: ThinkingPreference::Deep,
            cache_support: CacheSupport::ExplicitControl,
            cost_estimate_micro: 0,
            peak_factor_percent: 100,
            degraded_bypass: false,
        };
        // 1.0 * 0.5 + 1.0 * 0.3 + 1.0 * 0.2 = 1.0
        let w = ra.weight();
        assert!((w - 1.0).abs() < 1e-10, "Expected 1.0, got {w}");
    }

    #[test]
    fn test_weight_degraded() {
        let ra = RouteAffinity {
            fidelity: NegotiationFidelity::DegradedNotified,
            thinking_pref: ThinkingPreference::Fast,
            cache_support: CacheSupport::None,
            cost_estimate_micro: 0,
            peak_factor_percent: 100,
            degraded_bypass: true,
        };
        // 0.5 * 0.5 + 0.3 * 0.3 + 0.0 * 0.2 = 0.25 + 0.09 = 0.34
        let w = ra.weight();
        assert!((w - 0.34).abs() < 1e-10, "Expected 0.34, got {w}");
    }

    #[test]
    fn test_weight_channel_rejected() {
        let ra = RouteAffinity {
            fidelity: NegotiationFidelity::ChannelRejected,
            thinking_pref: ThinkingPreference::Standard,
            cache_support: CacheSupport::Implicit,
            cost_estimate_micro: 0,
            peak_factor_percent: 100,
            degraded_bypass: false,
        };
        // 0.0 * 0.5 + 0.7 * 0.3 + 0.6 * 0.2 = 0.0 + 0.21 + 0.12 = 0.33
        let w = ra.weight();
        assert!((w - 0.33).abs() < 1e-10, "Expected 0.33, got {w}");
    }

    #[test]
    fn test_weight_standard_mid() {
        // 中等配置: FullFidelity + Standard + Implicit
        let ra = RouteAffinity {
            fidelity: NegotiationFidelity::FullFidelity,
            thinking_pref: ThinkingPreference::Standard,
            cache_support: CacheSupport::Implicit,
            cost_estimate_micro: 0,
            peak_factor_percent: 100,
            degraded_bypass: false,
        };
        // 1.0 * 0.5 + 0.7 * 0.3 + 0.6 * 0.2 = 0.5 + 0.21 + 0.12 = 0.83
        let w = ra.weight();
        assert!((w - 0.83).abs() < 1e-10, "Expected 0.83, got {w}");
    }

    #[test]
    fn test_from_negotiation() {
        let spec = sample_spec();
        let outcome = NegotiationOutcome {
            fidelity: NegotiationFidelity::FullFidelity,
            tool_calling_enabled: true,
        };

        let ra = RouteAffinity::from_negotiation(&outcome, &spec, ThinkingPreference::Deep);

        assert_eq!(ra.fidelity, NegotiationFidelity::FullFidelity);
        assert_eq!(ra.thinking_pref, ThinkingPreference::Deep);
        assert_eq!(ra.cache_support, CacheSupport::ExplicitControl);
        assert_eq!(ra.peak_factor_percent, 100);
        assert!(!ra.degraded_bypass); // FullFidelity 不启用 bypass
                                      // cost_estimate_micro 来自 spec.pricing.input_micro_per_mtok，
                                      // sample_spec 使用 minimal() 零价占位，故此处为 0
        assert_eq!(ra.cost_estimate_micro, 0);
    }

    #[test]
    fn test_from_negotiation_degraded() {
        let spec = sample_spec();
        let outcome = NegotiationOutcome {
            fidelity: NegotiationFidelity::DegradedNotified,
            tool_calling_enabled: true,
        };

        let ra = RouteAffinity::from_negotiation(&outcome, &spec, ThinkingPreference::Standard);

        assert_eq!(ra.fidelity, NegotiationFidelity::DegradedNotified);
        assert!(ra.degraded_bypass); // Degraded 启用 bypass
    }

    #[test]
    fn test_from_negotiation_no_cache() {
        let mut spec = sample_spec();
        spec.capabilities.prompt_caching = CacheSupport::None;
        let outcome = NegotiationOutcome {
            fidelity: NegotiationFidelity::FullFidelity,
            tool_calling_enabled: true,
        };

        let ra = RouteAffinity::from_negotiation(&outcome, &spec, ThinkingPreference::Fast);

        assert_eq!(ra.cache_support, CacheSupport::None);
        // 权重: 1.0 * 0.5 + 0.3 * 0.3 + 0.0 * 0.2 = 0.59
        let w = ra.weight();
        assert!((w - 0.59).abs() < 1e-10, "Expected 0.59, got {w}");
    }
}
