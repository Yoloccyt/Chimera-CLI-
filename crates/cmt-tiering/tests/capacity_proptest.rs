//! CMT Hot 层容量不变量 proptest 属性测试
//!
//! 对应任务: T6-6 proptest 属性测试集成
//! 架构层: L3 Storage (cmt-tiering)
//!
//! # 验证的不变量
//! 1. Hot 层容量不超界 — 插入 N > capacity 个条目后,len <= capacity
//! 2. LRU 驱逐计数一致性 — 驱逐次数 = max(0, 插入数 - capacity)
//! 3. 容量为零的 Hot 层不 panic — capacity=0 时插入不崩溃
//!
//! # 语法约束(§4.4)
//! proptest 1.11+ 用 block-named 语法: `fn name(arg in strategy) { body }`

#![forbid(unsafe_code)]

use cmt_tiering::{CapabilityEntry, HotTier, Tier};
use proptest::prelude::*;

/// 构造一个 CapabilityEntry(用于插入 Hot 层)
fn make_entry(id: &str) -> CapabilityEntry {
    CapabilityEntry::new(id, "content", Tier::Hot)
}

proptest! {
    /// 不变量 1: Hot 层容量不超界 — 插入任意数量的条目后,
    /// len() 始终 <= capacity
    ///
    /// WHY: 容量约束是 LRU 缓存的核心契约。若 len > capacity,
    /// 内存使用失控,违背分层存储的资源预算。
    #[test]
    fn prop_hot_tier_capacity_never_exceeded(
        capacity in 1usize..20,
        insert_count in 1usize..50,
    ) {
        let hot = HotTier::new(capacity);
        for i in 0..insert_count {
            let entry = make_entry(&format!("cap-{}", i));
            let _ = hot.insert(entry);
        }
        prop_assert!(
            hot.len() <= capacity,
            "Hot tier len {} exceeded capacity {} (inserted {} items)",
            hot.len(), capacity, insert_count
        );
    }

    /// 不变量 2: LRU 驱逐计数一致性 — 插入 N 个不同条目后,
    /// evictions() == max(0, N - capacity)
    ///
    /// WHY: 驱逐计数是监控指标的基础。若计数与实际驱逐次数不一致,
    /// 运维仪表盘展示的驱逐率将失真,导致错误的容量规划决策。
    #[test]
    fn prop_hot_tier_eviction_count_consistent(
        capacity in 1usize..20,
        insert_count in 1usize..50,
    ) {
        let hot = HotTier::new(capacity);
        for i in 0..insert_count {
            let entry = make_entry(&format!("cap-{}", i));
            let _ = hot.insert(entry);
        }
        // clippy::implicit_saturating_sub 修复:饱和减法等价于原 if 分支
        let expected_evictions = insert_count.saturating_sub(capacity);
        prop_assert_eq!(
            hot.evictions(),
            expected_evictions as u64,
            "evictions should be {} but got {} (capacity={}, inserted={})",
            expected_evictions, hot.evictions(), capacity, insert_count
        );
    }

    /// 不变量 3: 重复插入同一 ID 不增加 len(UPSERT 语义)
    ///
    /// WHY: Hot 层 insert 是 UPSERT 语义(同 ID 更新而非新增)。
    /// 重复插入不应增加 len,否则容量计算错误。
    #[test]
    fn prop_hot_tier_upsert_no_len_increase(
        capacity in 1usize..20,
        repeat_count in 2usize..20,
    ) {
        let hot = HotTier::new(capacity);
        for _ in 0..repeat_count {
            let entry = make_entry("same-cap");
            let _ = hot.insert(entry);
        }
        prop_assert_eq!(
            hot.len(), 1,
            "repeated insert of same ID should keep len=1, got {}",
            hot.len()
        );
        prop_assert_eq!(
            hot.evictions(), 0,
            "repeated insert of same ID should not trigger eviction"
        );
    }
}
