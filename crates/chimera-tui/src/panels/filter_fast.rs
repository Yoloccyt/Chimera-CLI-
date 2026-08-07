//! 变体级快速关键字匹配(评估报告 v2 P1-3)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - **避免 JSON 全量序列化**:`event_matches_keyword` 原实现经
//!   `serde_json::to_string` 全量序列化每条事件后再子串匹配,万级事件 +
//!   关键字切换时冷路径 O(n×序列化)(panels_scale_bench 证实)。本模块对
//!   高频业务变体直接比较关键字段值,未命中直接返回 `Some(false)`,
//!   免去序列化;其余变体返回 `None` 由调用方回退 JSON 兜底。
//! - **语义边界(快路径 false 即拒绝)**:快速路径覆盖
//!   `type_name + metadata.source + 变体全部业务字段值`;metadata 时间戳/
//!   severity 等元字段与 JSON 字段名字符串**不**作为匹配目标——实际检索
//!   语义是业务内容检索,匹配时间戳/字段名的场景罕见且无检索价值。
//!   快路径 `Some(true)` 与 JSON 路径必然一致(拼接文本是 JSON 的子集),
//!   无误报风险。

use event_bus::NexusEvent;

/// 快速关键字命中判定(大小写不敏感)
///
/// # 返回
/// - `Some(true)`:命中(拼接文本是 JSON 序列化文本的子集,与慢路径一致);
/// - `Some(false)`:未命中(该变体的可搜索业务文本已全覆盖,可安全拒绝);
/// - `None`:本变体无快速路径,调用方应回退 `event_search_text` JSON 兜底。
pub fn event_keyword_hit_fast(event: &NexusEvent, keyword: &str) -> Option<bool> {
    let kw_lower = keyword.to_lowercase();
    let hit = |s: &str| s.to_lowercase().contains(&kw_lower);
    let source = event.metadata().source.as_str();
    match event {
        // 缓存命中/未命中:type_name + source + cache_key
        NexusEvent::CacheHit { cache_key, .. } | NexusEvent::CacheMiss { cache_key, .. } => {
            Some(hit(event.type_name()) || hit(source) || hit(cache_key))
        }
        // Quest 创建:type_name + source + quest_id + title
        NexusEvent::QuestCreated {
            quest_id, title, ..
        } => Some(hit(event.type_name()) || hit(source) || hit(quest_id) || hit(title)),
        // 议会投票:type_name + source + proposal_id + voter
        NexusEvent::VoteCast {
            proposal_id, voter, ..
        } => Some(hit(event.type_name()) || hit(source) || hit(proposal_id) || hit(voter)),
        // 其余变体:无快速路径,回退 JSON 序列化兜底
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use event_bus::EventMetadata;

    fn cache_hit(key: &str) -> NexusEvent {
        NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: key.into(),
        }
    }

    #[test]
    fn cache_hit_matches_key_value_case_insensitive() {
        assert_eq!(
            event_keyword_hit_fast(&cache_hit("alpha-key"), "ALPHA"),
            Some(true)
        );
        assert_eq!(
            event_keyword_hit_fast(&cache_hit("alpha-key"), "alpha"),
            Some(true)
        );
    }

    #[test]
    fn cache_hit_matches_source_or_type_name() {
        // source "scc-cache" 与 type_name "CacheHit" 均可匹配
        assert_eq!(
            event_keyword_hit_fast(&cache_hit("k1"), "scc"),
            Some(true),
            "source 子串应命中"
        );
        assert_eq!(
            event_keyword_hit_fast(&cache_hit("k1"), "cachehit"),
            Some(true),
            "type_name 子串应命中(大小写不敏感)"
        );
    }

    #[test]
    fn cache_hit_miss_is_fast_false() {
        assert_eq!(
            event_keyword_hit_fast(&cache_hit("k1"), "nonexistent"),
            Some(false),
            "未命中应直接返回 false(免 JSON 序列化)"
        );
    }

    #[test]
    fn unknown_variant_falls_back_to_none() {
        // 未覆盖变体(BudgetExceeded)返回 None,调用方走 JSON 兜底
        let event = NexusEvent::BudgetExceeded {
            metadata: EventMetadata::new("decb-governor"),
            budget_type: "token".into(),
            current: 9500,
            limit: 10000,
        };
        assert_eq!(event_keyword_hit_fast(&event, "token"), None);
    }

    #[test]
    fn fast_hit_implies_slow_hit_equivalence() {
        // 关键不变量:快路径 true 时,JSON 兜底路径(慢路径)必然也 true
        // (快路径拼接文本是慢路径序列化文本的子集)
        let event = cache_hit("alpha-key");
        if let Some(true) = event_keyword_hit_fast(&event, "alpha") {
            let meta = event.metadata();
            let haystack = format!(
                "{} {} {}",
                event.type_name(),
                meta.source,
                serde_json::to_string(&event).unwrap_or_default()
            )
            .to_lowercase();
            assert!(haystack.contains("alpha"));
        }
    }
}
