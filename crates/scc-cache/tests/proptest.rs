//! SCC 推测上下文缓存属性测试 — 验证 LRU 驱逐与命中率不变量
//!
//! 对应 SubTask 29.6:为 scc-cache 补充 proptest
//!
//! # 验证的不变量
//! 1. LRU 驱逐后容量恒定:插入 N > capacity 个条目,len <= capacity
//! 2. 命中率 ∈ [0.0, 1.0]:stats().hit_rate 始终在单位区间
//! 3. 概率和 = 1.0:predict_next 返回的概率之和 ≈ 1.0
//! 4. 预测按概率降序排列
//! 5. access_count 非递减

#![forbid(unsafe_code)]

use event_bus::EventBus;
use proptest::prelude::*;
use scc_cache::{AccessPatternLearner, ContextEntry, ContextId, SccCache, SccConfig};

/// 生成 [1, 20] 范围的容量策略
///
/// WHY 下限 1:容量 0 会导致所有插入被驱逐,无法测试 LRU 行为
fn prop_capacity() -> impl Strategy<Value = usize> {
    1usize..=20
}

/// 生成 [1, 50] 范围的插入数量策略
fn prop_insert_count() -> impl Strategy<Value = usize> {
    1usize..=50
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// 不变量 1:LRU 驱逐后容量恒定
    ///
    /// 插入 N > capacity 个条目(无 Arc 持有),len <= capacity
    /// WHY 容量恒定:LRU 驱逐保证缓存不无限增长,避免内存爆炸
    /// (对应架构红线:1M Token 暴力加载)
    #[test]
    fn test_lru_eviction_capacity_constant(
        capacity in prop_capacity(),
        insert_count in prop_insert_count(),
    ) {
        let cache = SccCache::new(
            SccConfig::default().with_capacity(capacity),
            EventBus::new(),
        );

        // 插入 insert_count 个条目(不持有 Arc,允许驱逐)
        for i in 0..insert_count {
            cache.insert(ContextEntry::new(
                format!("ctx-{i}"),
                format!("content-{i}"),
            ));
        }

        // 核心不变量:len <= capacity(无 Arc 保护时)
        prop_assert!(
            cache.len() <= capacity,
            "缓存条目数 {} 超过容量 {} (insert_count={})",
            cache.len(),
            capacity,
            insert_count
        );

        // 驱逐数应为 max(0, insert_count - capacity)
        let expected_evictions = insert_count.saturating_sub(capacity);
        prop_assert_eq!(
            cache.evictions() as usize,
            expected_evictions,
            "驱逐数应为 insert_count - capacity"
        );
    }

    /// 不变量 2:命中率 ∈ [0.0, 1.0]
    ///
    /// 任意访问序列(命中 + 未命中),stats().hit_rate 始终在 [0, 1]
    /// WHY 命中率区间:命中率用于性能监控,超出 [0, 1] 会导致监控误判
    #[test]
    fn test_hit_rate_always_in_unit_interval(
        n_hits in 0u32..=50,
        n_misses in 0u32..=50,
    ) {
        let cache = SccCache::new(SccConfig::default(), EventBus::new());

        // 插入 n_hits 个条目
        for i in 0..n_hits {
            cache.insert(ContextEntry::new(
                format!("hit-{i}"),
                format!("content-{i}"),
            ));
        }

        // 访问已存在的条目(命中)
        for i in 0..n_hits {
            let _ = cache.get_or_prefetch(&ContextId::new(format!("hit-{i}")));
        }

        // 访问不存在的条目(未命中)
        for i in 0..n_misses {
            let _ = cache.get_or_prefetch(&ContextId::new(format!("miss-{i}")));
        }

        let stats = cache.stats();
        prop_assert!(
            stats.hit_rate.is_finite(),
            "命中率必须为有限值,实际: {}",
            stats.hit_rate
        );
        prop_assert!(
            (0.0..=1.0).contains(&stats.hit_rate),
            "命中率 {} 超出 [0, 1] (hits={}, misses={})",
            stats.hit_rate, n_hits, n_misses
        );
    }

    /// 不变量 3:predict_next 概率和 ≈ 1.0
    ///
    /// 任意访问转移序列,predict_next 返回的概率之和 ≈ 1.0
    /// WHY 概率和为 1:马尔可夫链的概率分布必须归一化,
    /// 否则预取阈值判断会失效
    ///
    /// WHY n_transitions = n_targets + extra:确保每个目标至少被访问一次
    /// (i % n_targets 在 i ∈ 0..n_transitions 时覆盖所有 0..n_targets 残差),
    /// 否则未访问的目标不会出现在预测中,predictions.len() < n_targets
    #[test]
    fn test_predict_next_probabilities_sum_to_one(
        n_targets in 1u32..=5,
        extra_transitions in 0u32..=15,
    ) {
        let n_transitions = n_targets + extra_transitions;
        let learner = AccessPatternLearner::new(EventBus::new(), 0.0);
        let current = ContextId::new("ctx-current");

        // 记录 n_transitions 次转移,目标分布在 n_targets 个上下文
        for i in 0..n_transitions {
            let target_idx = i % n_targets;
            learner.record_access(&current, &ContextId::new(format!("ctx-target-{target_idx}")));
        }

        let predictions = learner.predict_next(&current);

        // 应有 n_targets 个预测(每个目标至少被访问一次,由 n_transitions >= n_targets 保证)
        prop_assert_eq!(
            predictions.len() as u32,
            n_targets,
            "预测数应等于目标数 (n_transitions={}, n_targets={})",
            n_transitions,
            n_targets
        );

        // 概率和应 ≈ 1.0
        let sum: f32 = predictions.iter().map(|(_, p)| p).sum();
        prop_assert!(
            (sum - 1.0).abs() < 1e-5,
            "概率和 {} 应 ≈ 1.0 (n_transitions={}, n_targets={})",
            sum, n_transitions, n_targets
        );

        // 每个概率 ∈ [0, 1]
        for (id, prob) in &predictions {
            prop_assert!(
                (0.0..=1.0).contains(prob),
                "概率 {} 超出 [0, 1] (id={})",
                prob, id
            );
        }
    }

    /// 不变量 4:预测按概率降序排列
    ///
    /// WHY 降序排列:预取从高概率开始,确保最可能的上下文优先预热
    #[test]
    fn test_predict_next_sorted_by_probability_desc(
        n_transitions in 1u32..=30,
        n_targets in 2u32..=6,
    ) {
        let learner = AccessPatternLearner::new(EventBus::new(), 0.0);
        let current = ContextId::new("ctx-current");

        // 记录转移,不同目标有不同计数
        for i in 0..n_transitions {
            let target_idx = i % n_targets;
            learner.record_access(&current, &ContextId::new(format!("ctx-target-{target_idx}")));
        }

        let predictions = learner.predict_next(&current);

        // 验证降序排列
        for i in 1..predictions.len() {
            prop_assert!(
                predictions[i - 1].1 >= predictions[i].1,
                "预测未按概率降序排列:位置 {} 概率 {} < 位置 {} 概率 {}",
                i - 1, predictions[i - 1].1, i, predictions[i].1
            );
        }
    }

    /// 不变量 5:access_count 非递减
    ///
    /// WHY 非递减:access_count 是累计计数器,只增不减,
    /// 递减会导致命中率计算错误
    #[test]
    fn test_access_count_non_decreasing(
        n_accesses in 1u32..=30,
    ) {
        let cache = SccCache::new(SccConfig::default(), EventBus::new());
        cache.insert(ContextEntry::new("ctx-1", "content"));

        let mut prev_count = cache.access_count();
        for i in 0..n_accesses {
            let _ = cache.get_or_prefetch(&ContextId::new(if i % 2 == 0 { "ctx-1" } else { "ctx-miss" }));
            let curr_count = cache.access_count();
            prop_assert!(
                curr_count >= prev_count,
                "access_count 递减: {} -> {} (i={})",
                prev_count, curr_count, i
            );
            prev_count = curr_count;
        }
    }

    /// 不变量 6:未知上下文 predict_next 返回空 Vec
    ///
    /// WHY 边界:未知上下文无转移记录,应返回空而非 panic
    #[test]
    fn test_predict_next_unknown_returns_empty(
        context_id in any::<String>(),
    ) {
        let learner = AccessPatternLearner::new(EventBus::new(), 0.6);
        let unknown = ContextId::new(context_id);

        let predictions = learner.predict_next(&unknown);
        prop_assert!(
            predictions.is_empty(),
            "未知上下文应返回空预测列表"
        );
    }

    /// 不变量 7:相同转移记录的计数正确
    ///
    /// WHY 计数正确:马尔可夫链的概率基于计数,计数错误会导致预取决策失效
    #[test]
    fn test_record_access_count_correct(
        n_records in 1u32..=20,
    ) {
        let learner = AccessPatternLearner::new(EventBus::new(), 0.0);
        let current = ContextId::new("ctx-current");
        let target = ContextId::new("ctx-target");

        // 记录 n_records 次相同转移
        for _ in 0..n_records {
            learner.record_access(&current, &target);
        }

        let pattern = learner.get_pattern(&current);
        prop_assert!(pattern.is_some(), "应有访问模式");

        let pattern = pattern.unwrap();
        prop_assert_eq!(pattern.transitions.len(), 1, "应只有一个目标");
        prop_assert_eq!(
            pattern.transitions[0].1, n_records,
            "转移计数应为 {}",
            n_records
        );

        // 概率应为 1.0(唯一目标)
        let predictions = learner.predict_next(&current);
        prop_assert_eq!(predictions.len(), 1);
        prop_assert!(
            (predictions[0].1 - 1.0).abs() < 1e-5,
            "唯一目标概率应为 1.0,实际: {}",
            predictions[0].1
        );
    }
}
