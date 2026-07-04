//! CMT 衰减计算器属性测试 — 验证 compute_priority 随 Δt 单调递减
//!
//! 对应 SubTask 15.12:引入 proptest 属性测试
//!
//! # 验证的不变量
//! `DecayCalculator::compute_priority` 随 Δt(距上次访问的时间间隔)增大单调递减
//!
//! # 公式
//! `priority = access_count × exp(-Δt / τ)`
//! - Δt = now - last_accessed_at(秒)
//! - τ = 衰减时间常数
//! - exp(-Δt/τ) 是 Δt 的单调递减函数 → priority 随 Δt 增大而递减
//!
//! # 策略
//! - 固定 access_count > 0 与 τ > 0
//! - 生成两个 Δt(t1 ≤ t2)
//! - 断言 priority(t1) >= priority(t2)

#![forbid(unsafe_code)]

use chrono::{Duration, Utc};
use cmt_tiering::{CapabilityEntry, DecayCalculator, Tier};
use proptest::prelude::*;

/// 将任意可显示错误转换为 TestCaseError(避免 unwrap,用 ? 传播)
fn fail<E: std::fmt::Display>(e: E) -> TestCaseError {
    TestCaseError::fail(format!("{e}"))
}

/// 构造指定 access_count 与 last_accessed_at 的 CapabilityEntry
fn make_entry(access_count: u64, last_accessed_at: chrono::DateTime<Utc>) -> CapabilityEntry {
    let mut entry = CapabilityEntry::new("cap-1", "content", Tier::Hot);
    entry.access_count = access_count;
    entry.last_accessed_at = last_accessed_at;
    entry
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// 不变量:compute_priority 随 Δt 单调递减
    ///
    /// 固定 access_count 与 τ,生成 t1 ≤ t2,断言 priority(t1) >= priority(t2)
    #[test]
    fn test_compute_priority_monotonically_decreasing_with_delta_t(
        access_count in 1u64..=10000,
        delta_t1 in 0u64..=1_000_000,
        delta_t2 in 0u64..=1_000_000,
        tau in 1u64..=1_000_000,
    ) {
        // 确保 t1 <= t2
        let (t1, t2) = if delta_t1 <= delta_t2 {
            (delta_t1, delta_t2)
        } else {
            (delta_t2, delta_t1)
        };

        let calc = DecayCalculator::new(tau).map_err(fail)?;
        let now = Utc::now();

        let entry1 = make_entry(access_count, now - Duration::seconds(t1 as i64));
        let entry2 = make_entry(access_count, now - Duration::seconds(t2 as i64));

        let p1 = calc.compute_priority(&entry1, now);
        let p2 = calc.compute_priority(&entry2, now);

        // 核心不变量:Δt 越大,priority 越小(允许相等,因浮点精度或两者都下溢为 0)
        prop_assert!(
            p1 >= p2,
            "priority(Δt={}) = {} 应 >= priority(Δt={}) = {} (access_count={}, τ={})",
            t1, p1, t2, p2, access_count, tau
        );
    }

    /// 不变量:access_count = 0 时 priority 恒为 0.0
    ///
    /// 从未访问的条目优先级为 0,不受 Δt 与 τ 影响
    #[test]
    fn test_compute_priority_zero_access_count_always_zero(
        delta_t in 0u64..=1_000_000,
        tau in 1u64..=1_000_000,
    ) {
        let calc = DecayCalculator::new(tau).map_err(fail)?;
        let now = Utc::now();
        let entry = make_entry(0, now - Duration::seconds(delta_t as i64));

        let priority = calc.compute_priority(&entry, now);
        prop_assert_eq!(priority, 0.0);
    }

    /// 不变量:Δt = 0 时 priority = access_count(刚访问的条目)
    ///
    /// exp(0) = 1,所以 priority = access_count × 1 = access_count
    #[test]
    fn test_compute_priority_zero_delta_t_equals_access_count(
        access_count in 1u64..=10000,
        tau in 1u64..=1_000_000,
    ) {
        let calc = DecayCalculator::new(tau).map_err(fail)?;
        let now = Utc::now();
        let entry = make_entry(access_count, now);

        let priority = calc.compute_priority(&entry, now);
        prop_assert!(
            (priority - access_count as f32).abs() < 1e-3,
            "Δt=0 时 priority = {} 应 ≈ access_count = {}",
            priority,
            access_count
        );
    }

    /// 不变量:priority 始终非负
    ///
    /// access_count ≥ 0 且 exp(-Δt/τ) > 0,所以 priority ≥ 0
    #[test]
    fn test_compute_priority_always_non_negative(
        access_count in 0u64..=10000,
        delta_t in 0u64..=1_000_000,
        tau in 1u64..=1_000_000,
    ) {
        let calc = DecayCalculator::new(tau).map_err(fail)?;
        let now = Utc::now();
        let entry = make_entry(access_count, now - Duration::seconds(delta_t as i64));

        let priority = calc.compute_priority(&entry, now);
        prop_assert!(
            priority >= 0.0,
            "priority = {} 应非负 (access_count={}, Δt={}, τ={})",
            priority, access_count, delta_t, tau
        );
    }

    /// 不变量:固定 Δt,priority 随 access_count 单调递增
    ///
    /// priority = access_count × exp(-Δt/τ),exp 项固定时 priority 与 access_count 正比
    #[test]
    fn test_compute_priority_increases_with_access_count(
        ac1 in 1u64..=5000,
        ac2 in 1u64..=5000,
        delta_t in 0u64..=100_000,
        tau in 1u64..=100_000,
    ) {
        let (ac1, ac2) = if ac1 <= ac2 { (ac1, ac2) } else { (ac2, ac1) };

        let calc = DecayCalculator::new(tau).map_err(fail)?;
        let now = Utc::now();
        let entry1 = make_entry(ac1, now - Duration::seconds(delta_t as i64));
        let entry2 = make_entry(ac2, now - Duration::seconds(delta_t as i64));

        let p1 = calc.compute_priority(&entry1, now);
        let p2 = calc.compute_priority(&entry2, now);

        prop_assert!(
            p1 <= p2,
            "access_count={} → priority={} 应 <= access_count={} → priority={} (Δt={}, τ={})",
            ac1, p1, ac2, p2, delta_t, tau
        );
    }
}
