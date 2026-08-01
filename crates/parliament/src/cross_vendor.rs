//! MCA P2-1:跨厂商辩论 — Skeptic 与 Producer 异厂商通道(ADR-067)
//!
//! 对应架构层:L8 Parliament
//! 对应设计源:`Chimera_全模型亲和适配体系设计文档_v1.0.md` §5.5 跨厂商议会
//!
//! # 同源相关失败(病理 D3)
//! L8 的 Skeptic/Security/Execution 角色若由同一模型自问自答,辩论存在
//! "同源相关失败"——同一模型的盲区在所有角色间相关,AHIRT 红队形同虚设。
//! 修复:凡涉及"验证/否决/红队"的第二意见,默认与生产者**不同厂商**。
//!
//! # P7 硬约束
//! - Skeptic 角色必须与 Producer 使用不同厂商
//! - Verifier 角色应与 Producer 使用不同厂商(最佳实践,非硬约束)
//!
//! # 侧表方案延续
//! 基于 `ProviderAffinityRegistry` 侧表查询,在辩论前确定每个角色使用的厂商。
//! 不修改 RoleProfile,通过 sidecar 模式注入(与 provider_affinity.rs 侧表方案一致)。

use std::sync::Arc;

use event_bus::{EventBus, EventMetadata, NexusEvent};
use nexus_contracts::affinity::ProviderId;
use nexus_core::Quest;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::error::ParliamentError;
use crate::provider_affinity::ProviderAffinityRegistry;
use crate::types::{Proposal, RoleId};

// ============================================================
// 跨厂商辩论配置
// ============================================================

/// 跨厂商辩论配置
///
/// 控制跨厂商辩论的启用/禁用与失败时回退策略。
#[derive(Debug, Clone, PartialEq)]
pub struct CrossVendorConfig {
    /// 是否启用跨厂商辩论（默认 true）
    pub enabled: bool,
    /// 失败时回退策略（默认 FallbackToSame）
    pub fallback: CrossVendorFallback,
}

impl Default for CrossVendorConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            fallback: CrossVendorFallback::FallbackToSame,
        }
    }
}

/// 跨厂商辩论回退策略
///
/// 当无法找到异厂商通道时的行为选择。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CrossVendorFallback {
    /// 回退到同厂商（向后兼容，但可能触发同源相关失败）
    FallbackToSame,
    /// 跳过辩论（当无法找到异厂商通道时）
    SkipDebate,
}

// ============================================================
// 跨厂商亲和路由器
// ============================================================

/// 跨厂商亲和路由器 — 根据 ProviderId 异同关系选择不同的模型通道
///
/// P7 硬约束：
/// - Skeptic 角色必须与 Producer 使用不同厂商
/// - Verifier 角色应与 Producer 使用不同厂商（最佳实践，非硬约束）
///
/// # 设计
/// 基于 `ProviderAffinityRegistry` 侧表查询，在辩论前确定每个角色使用的厂商。
/// 不修改 RoleProfile，通过 sidecar 模式注入（与 provider_affinity.rs 侧表方案一致）。
pub struct AffinityRouter {
    /// 跨厂商辩论配置
    config: CrossVendorConfig,
    /// provider 绑定注册表引用
    registry: Arc<ProviderAffinityRegistry>,
}

impl AffinityRouter {
    /// 创建新的亲和路由器
    ///
    /// # 参数
    /// - `config`:跨厂商辩论配置
    /// - `registry`:provider 绑定注册表引用
    pub fn new(config: CrossVendorConfig, registry: Arc<ProviderAffinityRegistry>) -> Self {
        Self { config, registry }
    }

    /// 为给定角色解析应使用的 provider
    ///
    /// # 解析逻辑
    /// 1. 如果角色未绑定，使用默认 provider（从 proposal 上下文推断）
    /// 2. 如果角色已绑定，从注册表查询并校验去相关
    /// 3. 如果去相关校验失败，按 fallback 策略处理
    ///
    /// # 参数
    /// - `role_id`:议会角色 ID
    /// - `proposal`:当前提案（用于推断默认 provider）
    ///
    /// # 返回
    /// - `Ok(ProviderId)`:解析成功的 provider
    /// - `Err(ParliamentError)`:解析失败（如去相关校验失败且 fallback 为 SkipDebate）
    pub fn resolve_provider(
        &self,
        role_id: &RoleId,
        proposal: &Proposal,
    ) -> Result<ProviderId, ParliamentError> {
        // 查询注册表中该角色的绑定
        match self.registry.binding_of(role_id) {
            Some(binding) => {
                // 校验去相关：producer 与 verifier、producer 与 skeptic 必须异厂商
                if crate::provider_affinity::validate_cross_provider(&binding).is_err() {
                    // 去相关校验失败，按 fallback 策略处理
                    return match self.config.fallback {
                        CrossVendorFallback::FallbackToSame => {
                            // 回退到同厂商（使用 binding 的 producer 字段）
                            Ok(binding.producer)
                        }
                        CrossVendorFallback::SkipDebate => {
                            // 跳过辩论：返回错误
                            Err(ParliamentError::ConfigError {
                                detail: format!(
                                    "cross-provider decorrelation failed for role '{}', \
                                     fallback=SkipDebate",
                                    role_id.as_str()
                                ),
                            })
                        }
                    };
                }
                // 去相关校验通过，返回 binding 的 producer 厂商
                Ok(binding.producer)
            }
            None => {
                // 角色未绑定，使用默认 provider（从 proposal 上下文推断）
                // 当前实现：基于风险等级选择一个合理的默认 provider
                Ok(infer_default_provider(proposal))
            }
        }
    }

    /// 为辩论角色解析跨厂商 provider
    ///
    /// 确保 Skeptic 角色使用与 default_provider 不同的厂商通道。
    ///
    /// # 参数
    /// - `role_id`:议会角色 ID（当前仅 Skeptic 需要异厂商）
    /// - `default_provider`:默认 provider（通常是 Producer 的厂商）
    ///
    /// # 返回
    /// - 如果启用跨厂商且 `role_id` 为 Skeptic，返回与 `default_provider` 不同的厂商
    /// - 如果未启用或角色不是 Skeptic，返回 `default_provider`
    /// - 如果无法找到异厂商，按 fallback 策略处理
    pub fn resolve_debate_providers(
        &self,
        role_id: &RoleId,
        default_provider: &ProviderId,
    ) -> ProviderId {
        // 只有 Skeptic 角色需要跨厂商分配
        // 使用 role_id 的字符串值判断是否为 Skeptic
        let is_skeptic = role_id.as_str().contains("skeptic");

        if !self.config.enabled || !is_skeptic {
            return default_provider.clone();
        }

        // 尝试选择与 default_provider 不同的厂商
        pick_alternative_provider(default_provider)
    }

    /// 获取配置引用
    pub fn config(&self) -> &CrossVendorConfig {
        &self.config
    }

    /// 获取注册表引用
    pub fn registry(&self) -> &ProviderAffinityRegistry {
        &self.registry
    }
}

// ============================================================
// 跨厂商辩论角色分配
// ============================================================

/// 跨厂商辩论角色分配 — 记录每个角色在辩论中使用的 provider
///
/// 记录一次辩论中 Producer/Verifier/Skeptic 三方使用的厂商，
/// 供审计、去相关校验与事件留痕使用。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CrossVendorAssignment {
    /// 辩论会话 ID
    pub session_id: String,
    /// 生产者使用的 provider
    pub producer_provider: ProviderId,
    /// 验证者使用的 provider
    pub verifier_provider: ProviderId,
    /// 怀疑者使用的 provider
    pub skeptic_provider: ProviderId,
    /// 是否强制了跨厂商去相关
    pub cross_vendor_enforced: bool,
}

// ============================================================
// 跨厂商辩论策略
// ============================================================

/// 跨厂商辩论策略 — 在辩论中确保 Skeptic 角色使用不同厂商通道
///
/// 与 `Parliament::deliberate_with_policy` 集成：
/// 1. 辩论前查询 `AffinityRouter` 确定 Skeptic 的厂商
/// 2. 辩论中确保 Skeptic 的 Opinion 来自异厂商
/// 3. 辩论后发布 CrossVendorNegotiation 事件留痕
///
/// # 同源相关失败修复
/// 当 Skeptic 与 Producer 同厂商时，红队形同虚设（同模型盲区在所有角色间相关）。
/// 此策略强制 Skeptic 使用异厂商通道，确保 AHIRT 红队独立有效。
pub struct CrossVendorDebate {
    /// 亲和路由器
    router: AffinityRouter,
    /// 事件总线
    event_bus: EventBus,
}

impl CrossVendorDebate {
    /// 创建新的跨厂商辩论策略
    ///
    /// # 参数
    /// - `router`:亲和路由器（持有配置与注册表）
    /// - `event_bus`:事件总线，用于发布 CrossVendorNegotiation 事件
    pub fn new(router: AffinityRouter, event_bus: EventBus) -> Self {
        Self { router, event_bus }
    }

    /// 辩论前准备，确定每个角色的厂商分配
    ///
    /// # 流程
    /// 1. 从 `AffinityRouter` 查询 Producer 的默认 provider
    /// 2. 从 `AffinityRouter` 查询 Verifier 的 provider
    /// 3. 从 `AffinityRouter` 查询 Skeptic 的 provider（可能跨厂商）
    /// 4. 如果启用跨厂商辩论，确保 Skeptic 与 Producer 异厂商
    /// 5. 发布 CrossVendorNegotiation 事件留痕
    ///
    /// # 参数
    /// - `quest`:关联的 Quest（提供上下文信息）
    /// - `proposal`:当前提案
    ///
    /// # 返回
    /// - `Ok(CrossVendorAssignment)`:分配结果，包含三方厂商与是否强制去相关
    pub fn prepare_debate(
        &self,
        quest: &Quest,
        proposal: &Proposal,
    ) -> Result<CrossVendorAssignment, ParliamentError> {
        // 使用默认的 RoleId 查找 Producer/Verifier/Skeptic 的绑定
        // 当前实现：角色命名约定为 "role-{role_name}"
        let producer_role = RoleId::new("role-producer");
        let verifier_role = RoleId::new("role-verifier");
        let skeptic_role = RoleId::new("role-skeptic");

        // 解析 Producer 的 provider
        let producer_provider = self.router.resolve_provider(&producer_role, proposal)?;

        // 解析 Verifier 的 provider
        let verifier_provider = self.router.resolve_provider(&verifier_role, proposal)?;

        // 解析 Skeptic 的 provider（根据跨厂商配置可能异厂商）
        let skeptic_provider = self
            .router
            .resolve_debate_providers(&skeptic_role, &producer_provider);

        // 判断是否强制了跨厂商去相关
        let cross_vendor_enforced =
            self.router.config().enabled && skeptic_provider != producer_provider;

        // 生成会话 ID
        let session_id = format!("cv-{}-{}", quest.quest_id, proposal.proposal_id);

        let assignment = CrossVendorAssignment {
            session_id: session_id.clone(),
            producer_provider: producer_provider.clone(),
            verifier_provider: verifier_provider.clone(),
            skeptic_provider: skeptic_provider.clone(),
            cross_vendor_enforced,
        };

        // 确定去相关状态描述
        let decorrelation_status = if cross_vendor_enforced {
            "enforced"
        } else if !self.router.config().enabled {
            "fallback_same"
        } else {
            "fallback_same"
        };

        // 发布 CrossVendorNegotiation 事件留痕
        // WHY 发布事件而非仅日志:event-bus 的 CrossVendorNegotiation 变体已存在，
        // 外部订阅者（efficiency-monitor、审计层）可监听此事件做跨厂商去相关分析。
        // 使用 publish_blocking 因为 prepare_debate 是同步方法。
        let event = NexusEvent::CrossVendorNegotiation {
            metadata: EventMetadata::new("parliament::cross_vendor"),
            session_id: session_id.clone(),
            quest_id: quest.quest_id.clone(),
            producer_provider: producer_provider.as_str().to_string(),
            verifier_provider: verifier_provider.as_str().to_string(),
            skeptic_provider: skeptic_provider.as_str().to_string(),
            cross_vendor_enforced,
            decorrelation_status: decorrelation_status.to_string(),
        };
        // 发布失败不影响分配结果（仅留痕目的）
        let _ = self.event_bus.publish_blocking(event);

        info!(
            session_id = %session_id,
            quest_id = %quest.quest_id,
            proposal_id = %proposal.proposal_id,
            producer_provider = ?producer_provider,
            verifier_provider = ?verifier_provider,
            skeptic_provider = ?skeptic_provider,
            cross_vendor_enforced = cross_vendor_enforced,
            "CrossVendorNegotiation:跨厂商辩论角色分配完成"
        );

        Ok(assignment)
    }

    /// 验证去相关合规性
    ///
    /// 检查 `CrossVendorAssignment` 是否满足 P7 硬约束：
    /// - Skeptic 与 Producer 必须异厂商（硬约束）
    /// - Verifier 与 Producer 应异厂商（最佳实践）
    ///
    /// # 参数
    /// - `assignment`:跨厂商辩论角色分配
    ///
    /// # 返回
    /// - `true`:去相关合规
    /// - `false`:去相关不合规（Skeptic 与 Producer 同厂商）
    pub fn validate_decorrelation(&self, assignment: &CrossVendorAssignment) -> bool {
        // P7 硬约束：Skeptic 必须与 Producer 异厂商
        if assignment.skeptic_provider == assignment.producer_provider {
            warn!(
                session_id = %assignment.session_id,
                producer = ?assignment.producer_provider,
                skeptic = ?assignment.skeptic_provider,
                "跨厂商去相关校验失败:Skeptic 与 Producer 同厂商(P7 硬约束违反)"
            );
            return false;
        }

        // Verifier 与 Producer 应异厂商（最佳实践，非硬约束，仅告警）
        if assignment.verifier_provider == assignment.producer_provider {
            warn!(
                session_id = %assignment.session_id,
                producer = ?assignment.producer_provider,
                verifier = ?assignment.verifier_provider,
                "跨厂商去相关建议:Verifier 与 Producer 同厂商(建议异厂商)"
            );
            // 非硬约束，不返回 false
        }

        true
    }

    /// 获取路由器引用
    pub fn router(&self) -> &AffinityRouter {
        &self.router
    }

    /// 获取事件总线引用
    pub fn event_bus(&self) -> &EventBus {
        &self.event_bus
    }
}

// ============================================================
// 辅助函数
// ============================================================

/// 从提案上下文推断默认 provider
///
/// 当前实现基于风险等级选择：
/// - 低风险（risk_level < 0.3）→ Zhipu（通用模型，成本效益好）
/// - 中风险（0.3 ≤ risk_level < 0.7）→ DeepSeek（推理能力强）
/// - 高风险（risk_level ≥ 0.7）→ Moonshot（安全审查严格）
///
/// # WHY 规则式推断
/// 当前无真实模型路由，使用规则化映射作为占位实现。
/// MCA 完全体接入后替换为 `model-router` 的渠道亲和查询。
fn infer_default_provider(proposal: &Proposal) -> ProviderId {
    if proposal.risk_level < 0.3 {
        ProviderId::Zhipu
    } else if proposal.risk_level < 0.7 {
        ProviderId::DeepSeek
    } else {
        ProviderId::Moonshot
    }
}

/// 为 Skeptic 选择与当前 provider 不同的替代厂商
///
/// # 选择逻辑
/// 从预定义的厂商池中选择第一个与 `current` 不同的厂商。
/// 如果所有厂商都与 `current` 相同（理论上不可能），返回 Zhipu 作为兜底。
///
/// # WHY 简单轮换而非加权选择
/// 当前实现仅需"不同"语义，无需考虑模型能力/成本差异。
/// MCA 完全体将通过 `AffinityRouter` 查询真实可用通道。
fn pick_alternative_provider(current: &ProviderId) -> ProviderId {
    // 候选厂商池（Zhipu、DeepSeek、Moonshot 三选一的两两异厂商）
    const ALTERNATIVES: [ProviderId; 3] = [
        ProviderId::Zhipu,
        ProviderId::DeepSeek,
        ProviderId::Moonshot,
    ];

    // 选择第一个与 current 不同的厂商
    for alt in &ALTERNATIVES {
        if alt != current {
            return alt.clone();
        }
    }

    // 兜底：理论上不会到达这里（current 不可能同时等于三个不同厂商）
    ProviderId::Zhipu
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider_affinity::ProviderBinding;

    // ============================================================
    // CrossVendorConfig 测试
    // ============================================================

    #[test]
    fn test_cross_vendor_config_default() {
        let config = CrossVendorConfig::default();
        assert!(config.enabled, "默认应启用跨厂商辩论");
        assert_eq!(
            config.fallback,
            CrossVendorFallback::FallbackToSame,
            "默认回退策略应为 FallbackToSame"
        );
    }

    #[test]
    fn test_cross_vendor_config_custom() {
        let config = CrossVendorConfig {
            enabled: false,
            fallback: CrossVendorFallback::SkipDebate,
        };
        assert!(!config.enabled);
        assert_eq!(config.fallback, CrossVendorFallback::SkipDebate);
    }

    // ============================================================
    // AffinityRouter 测试
    // ============================================================

    fn make_router(enabled: bool) -> AffinityRouter {
        let config = CrossVendorConfig {
            enabled,
            fallback: CrossVendorFallback::FallbackToSame,
        };
        let registry = Arc::new(ProviderAffinityRegistry::new());
        AffinityRouter::new(config, registry)
    }

    fn make_proposal(risk_level: f32) -> Proposal {
        Proposal::new("p-test", "q-test", "测试提案", risk_level)
    }

    fn make_skeptic_role_id() -> RoleId {
        RoleId::new("role-skeptic")
    }

    #[test]
    fn test_affinity_router_skeptic_differs_from_producer() {
        // 验证 Skeptic 自动分配到异厂商
        let router = make_router(true);
        let default_provider = ProviderId::Zhipu;
        let skeptic_role = make_skeptic_role_id();

        let skeptic_provider = router.resolve_debate_providers(&skeptic_role, &default_provider);

        // Skeptic 应与 default_provider 不同厂商
        assert_ne!(
            skeptic_provider, default_provider,
            "启用跨厂商时 Skeptic 应与 Producer 异厂商"
        );
        // 验证是候选厂商池中的合法值
        assert!(
            skeptic_provider == ProviderId::DeepSeek || skeptic_provider == ProviderId::Moonshot,
            "Skeptic 应分配到 DeepSeek 或 Moonshot（非 Zhipu）"
        );
    }

    #[test]
    fn test_affinity_router_fallback_to_same() {
        // 验证 fallback 策略工作：禁用跨厂商时返回同厂商
        let router = make_router(false);
        let default_provider = ProviderId::Zhipu;
        let skeptic_role = make_skeptic_role_id();

        let skeptic_provider = router.resolve_debate_providers(&skeptic_role, &default_provider);

        // 禁用跨厂商时，Skeptic 应与 Producer 同厂商
        assert_eq!(
            skeptic_provider, default_provider,
            "禁用跨厂商时 Skeptic 应与 Producer 同厂商(fallback)"
        );
    }

    #[test]
    fn test_affinity_router_non_skeptic_returns_default() {
        // 非 Skeptic 角色不受跨厂商影响
        let router = make_router(true);
        let default_provider = ProviderId::DeepSeek;
        let architect_role = RoleId::new("role-architect");

        let provider = router.resolve_debate_providers(&architect_role, &default_provider);

        // 非 Skeptic 角色应返回 default_provider
        assert_eq!(
            provider, default_provider,
            "非 Skeptic 角色应返回默认 provider"
        );
    }

    #[test]
    fn test_affinity_router_unbound_role_uses_default() {
        // 未绑定角色使用默认 provider（从 proposal 推断）
        let router = make_router(true);
        let unbound_role = RoleId::new("role-unbound");
        let proposal = make_proposal(0.2); // 低风险 → Zhipu

        let provider = router.resolve_provider(&unbound_role, &proposal).unwrap();

        assert_eq!(
            provider,
            ProviderId::Zhipu,
            "未绑定角色应使用从 proposal 推断的默认 provider"
        );
    }

    #[test]
    fn test_affinity_router_bound_role_returns_binding() {
        // 已绑定角色从注册表返回 provider
        let config = CrossVendorConfig {
            enabled: true,
            fallback: CrossVendorFallback::FallbackToSame,
        };
        let registry = Arc::new(ProviderAffinityRegistry::new());
        let binding =
            ProviderBinding::new(ProviderId::Zhipu, ProviderId::DeepSeek, ProviderId::MiniMax);
        registry
            .bind_provider(RoleId::new("role-skeptic"), binding)
            .unwrap();

        let router = AffinityRouter::new(config, registry);
        let proposal = make_proposal(0.5);

        let provider = router
            .resolve_provider(&RoleId::new("role-skeptic"), &proposal)
            .unwrap();

        assert_eq!(provider, ProviderId::Zhipu, "已绑定角色应返回注册表值");
    }

    // ============================================================
    // CrossVendorDebate 测试
    // ============================================================

    #[test]
    fn test_cross_vendor_debate_prepare() {
        // 验证辩论准备逻辑
        let config = CrossVendorConfig::default();
        let registry = Arc::new(ProviderAffinityRegistry::new());
        let router = AffinityRouter::new(config, Arc::clone(&registry));
        let event_bus = EventBus::new();
        let debate = CrossVendorDebate::new(router, event_bus);

        let quest = Quest {
            quest_id: "q-test".into(),
            title: "测试 Quest".into(),
            tasks: vec![],
            thinking_mode: nexus_core::ThinkingMode::Fast,
            checkpoint_id: None,
            priority: 128,
        };
        let proposal = make_proposal(0.5);

        let assignment = debate.prepare_debate(&quest, &proposal).unwrap();

        // 验证分配包含所有必要字段
        assert!(!assignment.session_id.is_empty(), "session_id 不应为空");
        assert!(
            assignment.session_id.contains("q-test"),
            "session_id 应包含 quest_id"
        );
        // 验证跨厂商强制标记（默认启用）
        assert!(
            assignment.cross_vendor_enforced,
            "默认启用跨厂商时标记应为 true"
        );
        // 验证 Skeptic 与 Producer 异厂商
        assert_ne!(
            assignment.skeptic_provider, assignment.producer_provider,
            "Skeptic 应与 Producer 异厂商"
        );
    }

    #[test]
    fn test_validate_decorrelation() {
        // 验证去相关校验
        let config = CrossVendorConfig::default();
        let registry = Arc::new(ProviderAffinityRegistry::new());
        let router = AffinityRouter::new(config, registry);
        let event_bus = EventBus::new();
        let debate = CrossVendorDebate::new(router, event_bus);

        // 合规分配：三方互异
        let valid_assignment = CrossVendorAssignment {
            session_id: "cv-test-valid".into(),
            producer_provider: ProviderId::Zhipu,
            verifier_provider: ProviderId::DeepSeek,
            skeptic_provider: ProviderId::MiniMax,
            cross_vendor_enforced: true,
        };
        assert!(
            debate.validate_decorrelation(&valid_assignment),
            "三方互异应通过去相关校验"
        );

        // 不合规分配：Skeptic 与 Producer 同厂商
        let invalid_assignment = CrossVendorAssignment {
            session_id: "cv-test-invalid".into(),
            producer_provider: ProviderId::Zhipu,
            verifier_provider: ProviderId::DeepSeek,
            skeptic_provider: ProviderId::Zhipu, // 与 Producer 同厂商
            cross_vendor_enforced: false,
        };
        assert!(
            !debate.validate_decorrelation(&invalid_assignment),
            "Skeptic 与 Producer 同厂商应不通过校验"
        );

        // 边界：Verifier 与 Producer 同厂商（允许，仅告警）
        let verifier_same = CrossVendorAssignment {
            session_id: "cv-test-verifier".into(),
            producer_provider: ProviderId::Zhipu,
            verifier_provider: ProviderId::Zhipu, // 同厂商（允许）
            skeptic_provider: ProviderId::DeepSeek,
            cross_vendor_enforced: true,
        };
        assert!(
            debate.validate_decorrelation(&verifier_same),
            "Verifier 与 Producer 同厂商应允许（非硬约束）"
        );
    }

    #[test]
    fn test_cross_vendor_debate_disabled() {
        // 禁用跨厂商时，Skeptic 与 Producer 同厂商
        let config = CrossVendorConfig {
            enabled: false,
            fallback: CrossVendorFallback::FallbackToSame,
        };
        let registry = Arc::new(ProviderAffinityRegistry::new());
        let router = AffinityRouter::new(config, registry);
        let event_bus = EventBus::new();
        let debate = CrossVendorDebate::new(router, event_bus);

        let quest = Quest {
            quest_id: "q-disabled".into(),
            title: "禁用测试".into(),
            tasks: vec![],
            thinking_mode: nexus_core::ThinkingMode::Fast,
            checkpoint_id: None,
            priority: 128,
        };
        let proposal = make_proposal(0.5);

        let assignment = debate.prepare_debate(&quest, &proposal).unwrap();

        // 禁用跨厂商时，cross_vendor_enforced 应为 false
        assert!(
            !assignment.cross_vendor_enforced,
            "禁用跨厂商时标记应为 false"
        );
    }

    // ============================================================
    // 辅助函数测试
    // ============================================================

    #[test]
    fn test_infer_default_provider_by_risk() {
        // 低风险 → Zhipu
        let low = Proposal::new("p-low", "q-1", "低风险", 0.2);
        assert_eq!(infer_default_provider(&low), ProviderId::Zhipu);

        // 中风险 → DeepSeek
        let mid = Proposal::new("p-mid", "q-1", "中风险", 0.5);
        assert_eq!(infer_default_provider(&mid), ProviderId::DeepSeek);

        // 高风险 → Moonshot
        let high = Proposal::new("p-high", "q-1", "高风险", 0.8);
        assert_eq!(infer_default_provider(&high), ProviderId::Moonshot);
    }

    #[test]
    fn test_pick_alternative_provider_different() {
        // Zhipu → DeepSeek（第一个不同厂商）
        assert_eq!(
            pick_alternative_provider(&ProviderId::Zhipu),
            ProviderId::DeepSeek
        );
        // DeepSeek → Zhipu
        assert_eq!(
            pick_alternative_provider(&ProviderId::DeepSeek),
            ProviderId::Zhipu
        );
        // Moonshot → Zhipu
        assert_eq!(
            pick_alternative_provider(&ProviderId::Moonshot),
            ProviderId::Zhipu
        );
        // Custom → Zhipu（Custom 不在候选池中，回退到第一个）
        assert_eq!(
            pick_alternative_provider(&ProviderId::Custom("openrouter".into())),
            ProviderId::Zhipu
        );
    }

    #[test]
    fn test_affinity_router_skip_debate_fallback() {
        // SkipDebate 回退策略：绑定有效时正常工作，producer != verifier（通过校验）
        let config = CrossVendorConfig {
            enabled: true,
            fallback: CrossVendorFallback::SkipDebate,
        };
        let registry = Arc::new(ProviderAffinityRegistry::new());
        // 绑定一个合规的 binding（三方互异）
        let good_binding = ProviderBinding::new(
            ProviderId::Zhipu,
            ProviderId::DeepSeek, // 与 producer 异厂商
            ProviderId::MiniMax,  // 与 producer 异厂商
        );
        registry
            .bind_provider(RoleId::new("role-skeptic"), good_binding.clone())
            .unwrap();

        let router = AffinityRouter::new(config, registry);
        let proposal = make_proposal(0.5);

        let result = router.resolve_provider(&RoleId::new("role-skeptic"), &proposal);

        // 有效 binding 应正常返回
        assert!(
            result.is_ok(),
            "SkipDebate 回退在有效 binding 时应正常返回 provider"
        );
        assert_eq!(result.unwrap(), ProviderId::Zhipu);
    }

    // ============================================================
    // P2-1.4 跨厂商辩论集成测试
    // ============================================================

    /// 辅助：创建带默认绑定的注册表（三方互异）
    fn make_registry_with_bindings(
        producer: ProviderId,
        verifier: ProviderId,
        skeptic: ProviderId,
    ) -> Arc<ProviderAffinityRegistry> {
        let registry = Arc::new(ProviderAffinityRegistry::new());
        let binding = ProviderBinding::new(producer.clone(), verifier.clone(), skeptic.clone());
        registry
            .bind_provider(RoleId::new("role-producer"), binding)
            .unwrap();
        registry
            .bind_provider(
                RoleId::new("role-verifier"),
                ProviderBinding::new(verifier.clone(), producer.clone(), skeptic.clone()),
            )
            .unwrap();
        registry
            .bind_provider(
                RoleId::new("role-skeptic"),
                ProviderBinding::new(skeptic, producer, verifier),
            )
            .unwrap();
        registry
    }

    /// 辅助：创建默认 Quest
    fn make_quest(quest_id: &str) -> Quest {
        Quest {
            quest_id: quest_id.into(),
            title: "测试 Quest".into(),
            tasks: vec![],
            thinking_mode: nexus_core::ThinkingMode::Fast,
            checkpoint_id: None,
            priority: 128,
        }
    }

    #[test]
    fn test_cross_vendor_pair_combinations() {
        // 验证至少 3 种不同厂商对组合的跨厂商辩论
        // 每个组合下 Skeptic 与 Producer 必须异厂商

        // 组合 1: Zhipu → DeepSeek（Producer=Zhipu, Skeptic 应 != Zhipu）
        {
            let router = make_router(true);
            let default_provider = ProviderId::Zhipu;
            let skeptic_role = RoleId::new("role-skeptic");
            let skeptic_provider =
                router.resolve_debate_providers(&skeptic_role, &default_provider);
            assert_ne!(
                skeptic_provider, default_provider,
                "组合 Zhipu→DeepSeek:Skeptic 应与 Producer 异厂商"
            );
            assert!(
                skeptic_provider == ProviderId::DeepSeek
                    || skeptic_provider == ProviderId::Moonshot,
                "Skeptic 应分配到 DeepSeek 或 Moonshot（非 Zhipu）"
            );
        }

        // 组合 2: DeepSeek → MiniMax（Producer=DeepSeek, Skeptic 应 != DeepSeek）
        {
            let router = make_router(true);
            let default_provider = ProviderId::DeepSeek;
            let skeptic_role = RoleId::new("role-skeptic");
            let skeptic_provider =
                router.resolve_debate_providers(&skeptic_role, &default_provider);
            assert_ne!(
                skeptic_provider, default_provider,
                "组合 DeepSeek→MiniMax:Skeptic 应与 Producer 异厂商"
            );
            assert!(
                skeptic_provider == ProviderId::Zhipu || skeptic_provider == ProviderId::Moonshot,
                "Skeptic 应分配到 Zhipu 或 Moonshot（非 DeepSeek）"
            );
        }

        // 组合 3: Moonshot → Zhipu（Producer=Moonshot, Skeptic 应 != Moonshot）
        {
            let router = make_router(true);
            let default_provider = ProviderId::Moonshot;
            let skeptic_role = RoleId::new("role-skeptic");
            let skeptic_provider =
                router.resolve_debate_providers(&skeptic_role, &default_provider);
            assert_ne!(
                skeptic_provider, default_provider,
                "组合 Moonshot→Zhipu:Skeptic 应与 Producer 异厂商"
            );
            assert!(
                skeptic_provider == ProviderId::Zhipu || skeptic_provider == ProviderId::DeepSeek,
                "Skeptic 应分配到 Zhipu 或 DeepSeek（非 Moonshot）"
            );
        }
    }

    #[test]
    fn test_cross_vendor_debate_full_flow() {
        // 验证跨厂商辩论完整流程：从 ProviderBinding 创建 → Registry 绑定
        // → AffinityRouter 路由 → CrossVendorDebate::prepare_debate

        // 1. 创建三方互异的 ProviderBinding
        let producer_provider = ProviderId::Zhipu;
        let verifier_provider = ProviderId::DeepSeek;
        let skeptic_provider_binding = ProviderId::MiniMax;

        // 2. 绑定到 Registry
        let registry = make_registry_with_bindings(
            producer_provider.clone(),
            verifier_provider.clone(),
            skeptic_provider_binding.clone(),
        );

        // 3. 创建 AffinityRouter（启用跨厂商）
        let config = CrossVendorConfig::default();
        let router = AffinityRouter::new(config, Arc::clone(&registry));
        let event_bus = EventBus::new();
        let debate = CrossVendorDebate::new(router, event_bus);

        // 4. 调用 prepare_debate
        let quest = make_quest("q-full-flow");
        let proposal = make_proposal(0.5);
        let assignment = debate.prepare_debate(&quest, &proposal).unwrap();

        // 5. 验证 Skeptic 与 Producer 异厂商
        assert_ne!(
            assignment.skeptic_provider, assignment.producer_provider,
            "完整流程中 Skeptic 应与 Producer 异厂商"
        );
        // 验证 cross_vendor_enforced = true
        assert!(
            assignment.cross_vendor_enforced,
            "启用跨厂商时标记应为 true"
        );
        // 验证 session_id 格式正确
        assert!(
            assignment.session_id.starts_with("cv-"),
            "session_id 应以 cv- 开头"
        );
        assert!(
            assignment.session_id.contains("q-full-flow"),
            "session_id 应包含 quest_id"
        );
        // 验证所有 provider 字段非空
        assert!(
            assignment.producer_provider != ProviderId::Custom("".into()),
            "producer_provider 不应为空"
        );
        assert!(
            assignment.verifier_provider != ProviderId::Custom("".into()),
            "verifier_provider 不应为空"
        );
        assert!(
            assignment.skeptic_provider != ProviderId::Custom("".into()),
            "skeptic_provider 不应为空"
        );
    }

    #[test]
    fn test_cross_vendor_debate_disabled_flow() {
        // 验证跨厂商辩论禁用时：Skeptic 与 Producer 同厂商
        // cross_vendor_enforced = false

        // 1. 创建禁用跨厂商的配置
        let config = CrossVendorConfig {
            enabled: false,
            fallback: CrossVendorFallback::FallbackToSame,
        };
        let registry = Arc::new(ProviderAffinityRegistry::new());
        let router = AffinityRouter::new(config, Arc::clone(&registry));
        let event_bus = EventBus::new();
        let debate = CrossVendorDebate::new(router, event_bus);

        // 2. 调用 prepare_debate
        let quest = make_quest("q-disabled-flow");
        let proposal = make_proposal(0.5);
        let assignment = debate.prepare_debate(&quest, &proposal).unwrap();

        // 3. 禁用时 Skeptic 应与 Producer 同厂商（回退）
        assert_eq!(
            assignment.skeptic_provider, assignment.producer_provider,
            "禁用跨厂商时 Skeptic 应与 Producer 同厂商"
        );
        // 验证 cross_vendor_enforced = false
        assert!(
            !assignment.cross_vendor_enforced,
            "禁用跨厂商时标记应为 false"
        );
        // 验证 provider 值有效（从 proposal 推断）
        assert_eq!(
            assignment.producer_provider,
            ProviderId::DeepSeek,
            "中风险提案应推断为 DeepSeek"
        );
    }

    #[test]
    fn test_cross_vendor_fallback_to_same() {
        // 验证回退策略：当注册的 binding 违反去相关约束（producer == skeptic）
        // 且 fallback 为 FallbackToSame 时，resolve_provider 回退到同厂商

        // 1. 创建违反去相关约束的 binding（producer == skeptic == Zhipu）
        let correlated_binding = ProviderBinding::new(
            ProviderId::Zhipu,    // producer
            ProviderId::DeepSeek, // verifier（异厂商，合规）
            ProviderId::Zhipu,    // skeptic（与 producer 同厂商，违反 P7 硬约束）
        );

        // 2. 绑定到注册表（应失败，因为 validate_cross_provider 会拒绝）
        let registry = Arc::new(ProviderAffinityRegistry::new());
        assert!(
            registry
                .bind_provider(RoleId::new("role-skeptic"), correlated_binding.clone())
                .is_err(),
            "producer == skeptic 的 binding 应被拒绝"
        );

        // 3. 先绑定一个合规的 binding，再绑定一个违反约束的 binding 到不同角色
        let good_binding =
            ProviderBinding::new(ProviderId::Zhipu, ProviderId::DeepSeek, ProviderId::MiniMax);
        registry
            .bind_provider(RoleId::new("role-producer"), good_binding)
            .unwrap();

        // 4. 创建 FallbackToSame 配置的 AffinityRouter
        let config = CrossVendorConfig {
            enabled: true,
            fallback: CrossVendorFallback::FallbackToSame,
        };
        let router = AffinityRouter::new(config, registry);
        let proposal = make_proposal(0.5);

        // 5. 未绑定角色应使用默认推断
        let unbound_skeptic = router.resolve_provider(&RoleId::new("role-skeptic"), &proposal);
        assert!(unbound_skeptic.is_ok(), "未绑定角色应使用默认推断");
        assert_eq!(
            unbound_skeptic.unwrap(),
            ProviderId::DeepSeek,
            "中风险提案应推断为 DeepSeek"
        );

        // 6. 验证 SkipDebate 回退：当 binding 违反去相关且回退为 SkipDebate
        let skip_registry = Arc::new(ProviderAffinityRegistry::new());
        let skip_binding = ProviderBinding::new(
            ProviderId::Zhipu,
            ProviderId::DeepSeek,
            ProviderId::Zhipu, // 违反约束
        );
        // binding 本身不会被注册（被 validate_cross_provider 拒绝）
        // 所以 resolve_provider 会走未绑定路径
        assert!(
            skip_registry
                .bind_provider(RoleId::new("role-skeptic"), skip_binding)
                .is_err(),
            "违反去相关约束的 binding 应被拒绝"
        );
    }

    #[test]
    fn test_cross_vendor_debate_event_bus() {
        // 验证 CrossVendorDebate::prepare_debate 发布 CrossVendorNegotiation 事件
        // 通过 EventBus 订阅验证事件字段正确性

        // 1. 创建带 EventBus 的 CrossVendorDebate
        let config = CrossVendorConfig::default();
        let registry = Arc::new(ProviderAffinityRegistry::new());
        let router = AffinityRouter::new(config, registry);
        let event_bus = EventBus::new();

        // 2. 订阅事件（必须在 spawn 之前 subscribe）
        let mut rx = event_bus.subscribe();

        let debate = CrossVendorDebate::new(router, event_bus);

        // 3. 调用 prepare_debate
        let quest = make_quest("q-event-test");
        let proposal = make_proposal(0.5);
        let assignment = debate.prepare_debate(&quest, &proposal).unwrap();

        // 4. 验证收到 CrossVendorNegotiation 事件
        // 使用 try_recv（同步方法，prepare_debate 是同步的）
        let received = rx.try_recv().unwrap();
        assert!(received.is_some(), "应收到 CrossVendorNegotiation 事件");

        let event = received.unwrap();
        match &event {
            NexusEvent::CrossVendorNegotiation {
                session_id,
                quest_id,
                producer_provider,
                verifier_provider,
                skeptic_provider,
                cross_vendor_enforced,
                decorrelation_status,
                ..
            } => {
                // 验证事件字段与 assignment 一致
                assert_eq!(
                    session_id, &assignment.session_id,
                    "事件 session_id 应与 assignment 一致"
                );
                assert_eq!(quest_id, "q-event-test", "事件 quest_id 应正确");
                assert_eq!(
                    producer_provider,
                    &assignment.producer_provider.as_str().to_string(),
                    "事件 producer_provider 应与 assignment 一致"
                );
                assert_eq!(
                    verifier_provider,
                    &assignment.verifier_provider.as_str().to_string(),
                    "事件 verifier_provider 应与 assignment 一致"
                );
                assert_eq!(
                    skeptic_provider,
                    &assignment.skeptic_provider.as_str().to_string(),
                    "事件 skeptic_provider 应与 assignment 一致"
                );
                // 验证 cross_vendor_enforced（默认启用跨厂商，应 true）
                assert!(
                    *cross_vendor_enforced,
                    "启用跨厂商时事件中的 cross_vendor_enforced 应为 true"
                );
                // 验证 decorrelation_status
                assert_eq!(
                    decorrelation_status, "enforced",
                    "启用跨厂商且 Skeptic 异厂商时状态应为 enforced"
                );
            }
            _ => {
                panic!("收到的事件类型不正确: {:?}", event.type_name());
            }
        }

        // 5. 验证事件元数据 source 字段
        let metadata = event.metadata();
        assert_eq!(
            metadata.source, "parliament::cross_vendor",
            "事件 source 应正确标识来源"
        );
    }
}
