//! Token 效率优化 E2E 测试 — ADR-069 全链路验证
//!
//! 验证五项客户端域优化的端到端闭环:
//! 1. 厂商缓存亲和引擎(vendor_profiles 查表 + CacheHitTracker 统计)
//! 2. Prompt 结构规范(4 层分离 + hash 确定性)
//! 3. 语义响应缓存(插入 → 查询命中 → namespace 隔离)
//! 4. 上下文预算裁剪(trim_to_budget 保留重要条目)
//! 5. 输出预算治理(negotiate_budget 档位映射)

#![forbid(unsafe_code)]

use nexus_contracts::affinity::{
    CapabilitySet, ThinkingPreference, ThinkingSupport, TokenCacheKey,
};
use scc_cache::semantic_cache::{verify_context_ledger, SemanticResponseCache};
use scc_cache::vendor_profiles::{vendor_profile, BreakpointStrategy};
use scc_cache::CacheHitTracker;

// ============================================================
// 1. 厂商缓存亲和引擎
// ============================================================

#[test]
fn e2e_vendor_cache_affinity_six_providers() {
    use nexus_contracts::affinity::ProviderId;

    // 显式控制族:Zhipu/Moonshot/MiniMax
    for provider in [ProviderId::Zhipu, ProviderId::Moonshot, ProviderId::MiniMax] {
        let profile = vendor_profile(&provider);
        assert_eq!(
            profile.breakpoint_strategy,
            BreakpointStrategy::TwoBreakpoint,
            "{provider:?} 应为 TwoBreakpoint"
        );
    }

    // 隐式族:DeepSeek/AlibabaCloud
    for provider in [ProviderId::DeepSeek, ProviderId::AlibabaCloud] {
        let profile = vendor_profile(&provider);
        assert_eq!(
            profile.breakpoint_strategy,
            BreakpointStrategy::StickinessOnly,
            "{provider:?} 应为 StickinessOnly"
        );
        assert!(profile.stickiness_weight > 0.0);
    }

    // 存储费感知:VolcanoArk
    let volcano = vendor_profile(&ProviderId::VolcanoArk);
    assert_eq!(
        volcano.breakpoint_strategy,
        BreakpointStrategy::StorageFeeAware
    );
    assert!(volcano.breakeven_requests > 0);
}

#[test]
fn e2e_cache_hit_tracker_full_cycle() {
    let tracker = CacheHitTracker::new();

    // 模拟 10 次请求:7 次命中,3 次未命中
    for _ in 0..7 {
        tracker.record("zhipu", 900, 1000); // 90% 命中
    }
    for _ in 0..3 {
        tracker.record("zhipu", 0, 1000); // 0% 命中
    }

    // 总命中率:6300/10000 = 63%
    let rate = tracker.hit_rate_percent("zhipu");
    assert_eq!(rate, 63, "厂商缓存命中率应为 63%");
    assert!(rate >= 60, "目标:厂商缓存命中率 >= 60%");
}

// ============================================================
// 2. Prompt 结构规范
// ============================================================

#[test]
fn e2e_prompt_normalization_4layer_hash_stability() {
    use mca_gateway::prompt_norm::{
        compute_system_prompt_hash, compute_tool_schema_hash, NormalizedPrompt,
    };

    let prompt = NormalizedPrompt::new(
        "You are Chimera CLI.",
        r#"[{"name":"read_file"}]"#,
        "repo context here",
        "user: hello",
    );

    // hash 确定性:相同输入 → 相同输出
    let h1 = compute_system_prompt_hash(&prompt);
    let h2 = compute_system_prompt_hash(&prompt);
    assert_eq!(h1, h2, "system_prompt_hash 必须确定性");

    let t1 = compute_tool_schema_hash(r#"[{"name":"read_file"}]"#);
    let t2 = compute_tool_schema_hash(r#"[{"name":"read_file"}]"#);
    assert_eq!(t1, t2, "tool_schema_hash 必须确定性");

    // 变更检测:工具变更 → hash 变化
    let t3 = compute_tool_schema_hash(r#"[{"name":"write_file"}]"#);
    assert_ne!(t1, t3, "工具变更必须导致 hash 变化");
}

// ============================================================
// 3. 语义响应缓存
// ============================================================

#[test]
fn e2e_semantic_cache_insert_lookup_isolation() {
    let cache = SemanticResponseCache::default();
    let key = TokenCacheKey {
        model: "glm-5.2".into(),
        model_version: "2026-07".into(),
        tool_schema_hash: [1u8; 32],
        system_prompt_hash: [2u8; 32],
        thinking_tier: ThinkingPreference::Standard,
        sampling_bucket: 0,
    };
    let clv = vec![0.5f32; 512];

    // 插入 quest-1 命名空间
    cache.insert(
        "quest-1",
        key.clone(),
        clv.clone(),
        "cached answer",
        [0u8; 32],
        1000,
    );

    // 同 namespace 命中
    let hit = cache.lookup("quest-1", &key, &clv);
    assert!(hit.is_some(), "同 namespace 应命中");
    assert_eq!(hit.unwrap().response.as_ref(), "cached answer");

    // 跨 namespace 不命中(隐私隔离)
    let miss = cache.lookup("quest-2", &key, &clv);
    assert!(miss.is_none(), "跨 namespace 禁止命中");
}

#[test]
fn e2e_context_ledger_drift_detection() {
    let context_a = [1u8; 32];
    let context_b = [2u8; 32];

    // 上下文未漂移 → 缓存有效
    assert!(verify_context_ledger(&context_a, &context_a));
    // 上下文漂移 → 缓存失效
    assert!(!verify_context_ledger(&context_a, &context_b));
}

// ============================================================
// 4. 输出预算治理
// ============================================================

#[test]
fn e2e_output_budget_governance_tiers() {
    use mca_gateway::negotiate_budget;

    let mut caps = CapabilitySet::minimal_text(200_000, 200_000);
    caps.thinking = ThinkingSupport::OnOff;

    // Fast 档:8K thinking
    let fast = negotiate_budget(&caps, ThinkingPreference::Fast, None);
    assert_eq!(fast.thinking_budget_tokens, Some(8192));

    // Standard 档:32K thinking
    let std = negotiate_budget(&caps, ThinkingPreference::Standard, None);
    assert_eq!(std.thinking_budget_tokens, Some(32768));

    // Deep 档:128K thinking
    let deep = negotiate_budget(&caps, ThinkingPreference::Deep, None);
    assert_eq!(deep.thinking_budget_tokens, Some(131072));

    // 所有档位 max_output_tokens 不超过厂商上限
    for budget in [fast, std, deep] {
        assert!(budget.max_output_tokens <= 200_000);
    }
}

#[test]
fn e2e_output_budget_cost_guard() {
    use mca_gateway::negotiate_budget;

    let mut caps = CapabilitySet::minimal_text(200_000, 200_000);
    caps.thinking = ThinkingSupport::OnOff;

    // 无成本约束:Deep 档 128K
    let unconstrained = negotiate_budget(&caps, ThinkingPreference::Deep, None);
    assert_eq!(unconstrained.thinking_budget_tokens, Some(131072));

    // 有成本约束:hint=16000 微元 → affordable=1600 < 地板 2048 → 智能降级到地板
    let constrained = negotiate_budget(&caps, ThinkingPreference::Deep, Some(16000));
    assert_eq!(constrained.thinking_budget_tokens, Some(2048));
}

// ============================================================
// 5. CapabilityToken S9 回滚验证
// ============================================================

#[test]
fn e2e_capability_token_s9_rollback() {
    use nexus_contracts::{CapabilityToken, SeamId};

    // S9 接缝存在且可注册
    let mut token = CapabilityToken::new("s9-token-efficiency", SeamId::S9TokenEfficiency);

    // 初始态:Provisional,未达激活阈值
    assert!(!token.allows_learned_policy(0), "初始态不应允许缓存");

    // 渐进授权:多次成功 → 达阈值
    for _ in 0..20 {
        token.record_outcome(true);
        token.maybe_promote();
    }
    assert!(token.allows_learned_policy(0), "授权后应允许缓存");

    // ASA 干预 → Cooldown → 缓存 bypass
    token.trigger_asa_intervention(100);
    assert!(
        !token.allows_learned_policy(100),
        "Cooldown 态应 bypass 缓存"
    );
}
