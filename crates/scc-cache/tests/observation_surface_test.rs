//! WI-G13 — SCC 只读观测面验证（命中率 / 断点分布 / 缓存统计）
//!
//! # 定位（报告型 WI 的本地可复现实体）
//! 真实 LLM 请求面（mca-gateway codec 注入 cache_control + UsageReport 回填
//! `CacheHitTracker`）在本环境无凭据/无流量,不可测 → 观测口径由本测试以
//! **确定性本地实体**固化;将来真实请求面上线后可按同口径直接对齐读数。
//!
//! # 只读声明（不改行为）
//! 全部调用为既有只读/记录 API（`stats()`/`access_count()`/`hit_rate` 族/
//! `plan_breakpoints` 纯函数）;本文件为测试新增,不进生产路径,零默认变更。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code

use event_bus::EventBus;
use nexus_contracts::affinity::CacheSupport;
use scc_cache::affinity::plan_breakpoints;
use scc_cache::semantic_cache::{DEFAULT_MAX_ENTRIES_PER_NS, DEFAULT_SIMILARITY_THRESHOLD};
use scc_cache::{
    CacheHitTracker, CachePrefix, PrefixKind, SccCache, SccConfig, SessionAffinityTracker,
};

/// 面 1:命中率观测 — per-provider / global / semantic 三口径（u8 百分数契约锁定）
#[test]
fn hit_rate_face_reports_three_scopes() {
    let tracker = CacheHitTracker::new();
    // 真实请求面口径:每次 invoke() 回填 (hit_tokens, total_input_tokens)
    tracker.record("anthropic", 1200, 2000); // 60%
    tracker.record("deepseek", 100, 1000); // 10%

    assert_eq!(tracker.hit_rate_percent("anthropic"), 60);
    assert_eq!(tracker.hit_rate_percent("deepseek"), 10);
    assert_eq!(
        tracker.hit_rate_percent("unknown-provider"),
        0,
        "无数据厂商 = 0（非 panic）"
    );
    // 全局 = 汇总 token 加权:(1200+100)*100/3000 = 43 %
    assert_eq!(tracker.global_hit_rate_percent(), 43);
    // 浮点口径（遥测聚合用）
    assert!((tracker.hit_rate("anthropic").unwrap() - 0.6).abs() < 1e-6);
    assert_eq!(tracker.tracked_providers(), 2);
    let all = tracker.all_hit_rates();
    assert_eq!(all.len(), 2);

    // 语义缓存命中率面（独立计数）
    for _ in 0..4 {
        tracker.record_request();
    }
    tracker.record_semantic_hit();
    assert_eq!(tracker.semantic_hit_rate_percent(), 25);
    assert_eq!(tracker.total_requests(), 4);
}

/// 面 2:断点分布观测 — `plan_breakpoints` 逐族行为 + PrefixKind 分布
#[test]
fn breakpoint_distribution_face() {
    // ExplicitControl + 长前缀链:累计跨 1024 阈值处打断点
    let prefixes = [
        CachePrefix {
            kind: PrefixKind::SystemPrompt,
            token_len: 600,
        },
        CachePrefix {
            kind: PrefixKind::RepoWikiRetrieval,
            token_len: 800,
        },
    ];
    let bps = plan_breakpoints(CacheSupport::ExplicitControl, &prefixes);
    assert_eq!(bps.len(), 1, "600<1024、600+800≥1024 → 恰 1 个断点");
    assert_eq!(bps[0].after_prefix, PrefixKind::RepoWikiRetrieval);
    assert_eq!(bps[0].cumulative_tokens, 1400);

    // 短前缀（不足阈值）→ 零断点（配额不浪费）
    let short = [CachePrefix {
        kind: PrefixKind::SystemPrompt,
        token_len: 100,
    }];
    assert!(plan_breakpoints(CacheSupport::ExplicitControl, &short).is_empty());
    // 非显式族 → 零断点（隐式族靠会话粘性）
    assert!(plan_breakpoints(CacheSupport::Implicit, &prefixes).is_empty());
    assert!(plan_breakpoints(CacheSupport::None, &prefixes).is_empty());

    // 分布快照:计数按 PrefixKind 归类（报告口径）
    let mut by_kind: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for bp in &bps {
        let key = format!("{:?}", bp.after_prefix);
        *by_kind.entry(key).or_insert(0) += 1;
    }
    assert_eq!(by_kind.get("RepoWikiRetrieval"), Some(&1));
    assert_eq!(by_kind.get("SystemPrompt"), None);
}

/// 面 3:缓存统计观测 — 装配态零流量基线（与 doctor scc 探针读数同源）
#[test]
fn cache_stats_face_zero_traffic_baseline() {
    let cache = SccCache::new(SccConfig::new(), EventBus::new());
    let stats = cache.stats();
    assert_eq!(stats.entry_count, 0, "装配初期零条目");
    assert_eq!(stats.hit_rate, 0.0, "零流量命中率 = 0.0（非 NaN）");
    assert_eq!(cache.access_count(), 0);
    assert_eq!(cache.len(), 0);

    // 语义缓存面阈值常量锁（报告引用的观测参数）
    assert_eq!(DEFAULT_SIMILARITY_THRESHOLD, 0.92);
    assert_eq!(DEFAULT_MAX_ENTRIES_PER_NS, 256);

    // 会话粘性面:空跟踪器
    let affinity = SessionAffinityTracker::new();
    assert_eq!(affinity.tracked_sessions(), 0);
    assert_eq!(affinity.preferred("quest-x"), None);
}
