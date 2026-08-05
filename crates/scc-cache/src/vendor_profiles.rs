//! 厂商缓存亲和配置 — 六厂商差异化缓存策略数据化（ADR-069 Token 效率优化）
//!
//! 对应架构层: L3 Storage（scc-cache）
//! 对应设计源: `Chimera_全模型亲和适配体系设计文档_v1.0.md` §5.2 缓存亲和
//!
//! # 六厂商缓存策略矩阵
//!
//! | 厂商 | CacheSupport | 断点策略 | 核心机制 |
//! |------|-------------|---------|---------|
//! | Zhipu (GLM) | ExplicitControl | TwoBreakpoint | Anthropic 路径 cache_control |
//! | Moonshot (Kimi) | ExplicitControl | TwoBreakpoint | Anthropic 路径原生 |
//! | MiniMax | ExplicitControl | TwoBreakpoint | Anthropic 路径双协议 |
//! | DeepSeek | Implicit | StickinessOnly | 上下文缓存自动命中 |
//! | VolcanoArk (豆包) | Implicit | StorageFeeAware | 缓存命中价 + 存储费回本 |
//! | AlibabaCloud (Qwen) | Implicit | StickinessOnly | 会话粘性最大化命中 |
//!
//! # 依赖方向（§2.2 铁律）
//! 本模块依赖 L0 `nexus_contracts::affinity::{CacheSupport, ProviderId}`（L3 → L0 合法）。

use nexus_contracts::affinity::{CacheSupport, ProviderId};

/// 断点策略 — 显式缓存族的 cache_control 断点放置方式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakpointStrategy {
    /// Anthropic 族两断点：system_prompt + tool_declarations
    ///
    /// 适用：Zhipu/Moonshot/MiniMax（走 Anthropic 路径 cache_control）
    TwoBreakpoint,
    /// 隐式族会话粘性：无显式断点，靠路由层同通道优先
    ///
    /// 适用：DeepSeek/AlibabaCloud（厂商自动缓存，客户端仅做粘性路由）
    StickinessOnly,
    /// 存储费感知：隐式缓存 + 显式缓存存储费回本判定
    ///
    /// 适用：豆包（缓存命中价优惠，但显式缓存存储需回本计算）
    /// WHY 独立策略：豆包的显式缓存需预付存储费，命中次数不够则亏损
    StorageFeeAware,
    /// 无缓存策略
    None,
}

/// 厂商缓存亲和配置 — 数据化单厂商的缓存行为参数
///
/// 纯数据（无逻辑），由 `vendor_profile()` 查表返回。
/// 路由层与 codec 层据此决定：是否打断点、粘性权重、回本阈值。
#[derive(Debug, Clone, PartialEq)]
pub struct VendorCacheProfile {
    /// 厂商标识
    pub provider: ProviderId,
    /// 缓存支持度（与 CapabilitySet.prompt_caching 一致）
    pub cache_support: CacheSupport,
    /// 断点策略
    pub breakpoint_strategy: BreakpointStrategy,
    /// 会话粘性权重 [0.0, 1.0]（隐式族路由调优用；显式族 = 0.0）
    pub stickiness_weight: f32,
    /// 缓存写入溢价百分比（显式族写入成本 = 输入价 × (100 + premium) / 100）
    ///
    /// WHY: Anthropic 路径 cache_control 写入有 25% 溢价（首次写入比正常输入贵），
    /// 命中后节省 90%。回本公式：breakeven = premium / (100 - hit_discount)
    pub cache_write_premium_percent: u16,
    /// 回本请求数（写入溢价被缓存命中折扣抵消的最小请求次数）
    ///
    /// 公式：ceil(premium_percent / (100 - hit_price_percent_of_input))
    /// 例：Anthropic 溢价 25%，命中价 = 输入价 10% → ceil(25/90) = 1 次即回本
    pub breakeven_requests: u32,
}

/// 查询厂商缓存亲和配置 — 闭集查表（未知厂商返回 None 策略）
///
/// WHY 查表而非 spec 携带：缓存策略参数（溢价/回本）随厂商定价漂移，
/// 集中管理比分散在每张 TOML 卡片更易维护；且这些参数是客户端优化决策
/// 的内部数据，不属于厂商能力描述（CapabilitySet）。
pub fn vendor_profile(provider: &ProviderId) -> VendorCacheProfile {
    match provider {
        // 显式控制族（Anthropic 路径 cache_control）
        ProviderId::Zhipu => VendorCacheProfile {
            provider: ProviderId::Zhipu,
            cache_support: CacheSupport::ExplicitControl,
            breakpoint_strategy: BreakpointStrategy::TwoBreakpoint,
            stickiness_weight: 0.0,
            cache_write_premium_percent: 25,
            breakeven_requests: 1,
        },
        ProviderId::Moonshot => VendorCacheProfile {
            provider: ProviderId::Moonshot,
            cache_support: CacheSupport::ExplicitControl,
            breakpoint_strategy: BreakpointStrategy::TwoBreakpoint,
            stickiness_weight: 0.0,
            cache_write_premium_percent: 25,
            breakeven_requests: 1,
        },
        ProviderId::MiniMax => VendorCacheProfile {
            provider: ProviderId::MiniMax,
            cache_support: CacheSupport::ExplicitControl,
            breakpoint_strategy: BreakpointStrategy::TwoBreakpoint,
            stickiness_weight: 0.0,
            cache_write_premium_percent: 25,
            breakeven_requests: 1,
        },
        // 隐式自动族（厂商自动缓存，客户端做会话粘性）
        ProviderId::DeepSeek => VendorCacheProfile {
            provider: ProviderId::DeepSeek,
            cache_support: CacheSupport::Implicit,
            breakpoint_strategy: BreakpointStrategy::StickinessOnly,
            stickiness_weight: 0.8,
            cache_write_premium_percent: 0,
            breakeven_requests: 0,
        },
        ProviderId::AlibabaCloud => VendorCacheProfile {
            provider: ProviderId::AlibabaCloud,
            cache_support: CacheSupport::Implicit,
            breakpoint_strategy: BreakpointStrategy::StickinessOnly,
            stickiness_weight: 0.7,
            cache_write_premium_percent: 0,
            breakeven_requests: 0,
        },
        // 存储费感知族（豆包：缓存命中价优惠 + 显式缓存存储费回本）
        ProviderId::VolcanoArk => VendorCacheProfile {
            provider: ProviderId::VolcanoArk,
            cache_support: CacheSupport::Implicit,
            breakpoint_strategy: BreakpointStrategy::StorageFeeAware,
            stickiness_weight: 0.6,
            // 豆包显式缓存存储费：约 0.5× 输入价 / 天，需 ≥3 次命中回本
            cache_write_premium_percent: 50,
            breakeven_requests: 3,
        },
        // 其他厂商（StepFun / Custom）：无缓存策略
        _ => VendorCacheProfile {
            provider: provider.clone(),
            cache_support: CacheSupport::None,
            breakpoint_strategy: BreakpointStrategy::None,
            stickiness_weight: 0.0,
            cache_write_premium_percent: 0,
            breakeven_requests: 0,
        },
    }
}

/// 判断厂商缓存策略是否值得启用显式缓存（回本判定）
///
/// 对于 StorageFeeAware 族（豆包），会话预期轮次 < breakeven_requests 时
/// 不启用显式缓存（存储费无法回本），退化为纯粘性路由。
pub fn should_enable_explicit_cache(profile: &VendorCacheProfile, expected_turns: u32) -> bool {
    match profile.breakpoint_strategy {
        BreakpointStrategy::TwoBreakpoint => true,
        BreakpointStrategy::StorageFeeAware => expected_turns >= profile.breakeven_requests,
        // 隐式族与无缓存族不启用显式缓存
        BreakpointStrategy::StickinessOnly | BreakpointStrategy::None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_control_vendors_have_two_breakpoint() {
        for provider in [ProviderId::Zhipu, ProviderId::Moonshot, ProviderId::MiniMax] {
            let profile = vendor_profile(&provider);
            assert_eq!(profile.cache_support, CacheSupport::ExplicitControl);
            assert_eq!(
                profile.breakpoint_strategy,
                BreakpointStrategy::TwoBreakpoint
            );
            assert_eq!(profile.stickiness_weight, 0.0);
        }
    }

    #[test]
    fn implicit_vendors_have_stickiness() {
        for provider in [ProviderId::DeepSeek, ProviderId::AlibabaCloud] {
            let profile = vendor_profile(&provider);
            assert_eq!(profile.cache_support, CacheSupport::Implicit);
            assert_eq!(
                profile.breakpoint_strategy,
                BreakpointStrategy::StickinessOnly
            );
            assert!(profile.stickiness_weight > 0.0);
        }
    }

    #[test]
    fn volcano_ark_storage_fee_aware() {
        let profile = vendor_profile(&ProviderId::VolcanoArk);
        assert_eq!(
            profile.breakpoint_strategy,
            BreakpointStrategy::StorageFeeAware
        );
        assert_eq!(profile.breakeven_requests, 3);
    }

    #[test]
    fn unknown_vendor_gets_none_strategy() {
        let profile = vendor_profile(&ProviderId::StepFun);
        assert_eq!(profile.cache_support, CacheSupport::None);
        assert_eq!(profile.breakpoint_strategy, BreakpointStrategy::None);
    }

    #[test]
    fn breakeven_logic_for_storage_fee_aware() {
        let profile = vendor_profile(&ProviderId::VolcanoArk);
        // 预期 2 轮 < 回本阈值 3 → 不启用显式缓存
        assert!(!should_enable_explicit_cache(&profile, 2));
        // 预期 3 轮 >= 回本阈值 3 → 启用
        assert!(should_enable_explicit_cache(&profile, 3));
    }

    #[test]
    fn explicit_control_always_enabled() {
        let profile = vendor_profile(&ProviderId::Zhipu);
        // 显式族无论预期轮次多少都启用（写入溢价 1 次即回本）
        assert!(should_enable_explicit_cache(&profile, 1));
    }

    #[test]
    fn implicit_never_enables_explicit() {
        let profile = vendor_profile(&ProviderId::DeepSeek);
        assert!(!should_enable_explicit_cache(&profile, 100));
    }
}
