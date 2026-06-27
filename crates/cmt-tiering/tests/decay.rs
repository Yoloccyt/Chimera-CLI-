//! 衰减计算器单元测试 — 验证指数衰减公式与降级阈值
//!
//! 对应 SubTask 3.15:验证指数衰减公式与降级阈值
//! (τ=24h,Δt=72h 时 priority < 0.1)
//!
//! # 测试覆盖
//! - 衰减公式:priority = access_count × exp(-Δt / τ)
//! - 边界条件:access_count=0、Δt=0、负 Δt(时钟漂移)
//! - 降级阈值:priority < 0.1 触发降级
//! - 任务基准:τ=24h,Δt=72h 时 priority ≈ 0.0498 < 0.1
//! - 高访问次数的条目衰减更慢
//! - 从配置创建衰减计算器

use chrono::{DateTime, Duration, Utc};
use cmt_tiering::{CmtConfig, CmtError, DecayCalculator, Tier, DEMOTION_THRESHOLD};

/// 构造指定访问次数与最后访问时间的测试条目
fn make_entry_with_access(
    id: &str,
    access_count: u64,
    last_accessed_at: DateTime<Utc>,
) -> cmt_tiering::CapabilityEntry {
    let mut entry = cmt_tiering::CapabilityEntry::new(id, format!("content-{id}"), Tier::Hot);
    entry.access_count = access_count;
    entry.last_accessed_at = last_accessed_at;
    entry
}

#[test]
fn test_decay_new_valid_tau() {
    let calc = DecayCalculator::new(86400).unwrap();
    assert_eq!(calc.tau_seconds(), 86400);
}

#[test]
fn test_decay_new_zero_tau_returns_error() {
    let result = DecayCalculator::new(0);
    assert!(matches!(result, Err(CmtError::InvalidConfig(_))));
}

#[test]
fn test_decay_from_config() {
    let config = CmtConfig::default();
    let calc = DecayCalculator::from_config(&config).unwrap();
    assert_eq!(calc.tau_seconds(), 86400); // 默认 τ=24h
}

#[test]
fn test_decay_from_config_custom_tau() {
    let config = CmtConfig::default().with_decay_tau_seconds(3600); // 1 小时
    let calc = DecayCalculator::from_config(&config).unwrap();
    assert_eq!(calc.tau_seconds(), 3600);
}

#[test]
fn test_compute_priority_zero_access_count() {
    // 从未访问的条目优先级为 0
    let calc = DecayCalculator::new(86400).unwrap();
    let now = Utc::now();
    let entry = make_entry_with_access("cap-1", 0, now);

    let priority = calc.compute_priority(&entry, now);
    assert_eq!(priority, 0.0);
}

#[test]
fn test_compute_priority_zero_delta_t() {
    // Δt = 0 时,priority = access_count × exp(0) = access_count × 1
    let calc = DecayCalculator::new(86400).unwrap();
    let now = Utc::now();
    let entry = make_entry_with_access("cap-1", 10, now);

    let priority = calc.compute_priority(&entry, now);
    assert!((priority - 10.0).abs() < 1e-6);
}

#[test]
fn test_compute_priority_tau_24h_delta_t_24h() {
    // τ=24h,Δt=24h 时,exp(-1) ≈ 0.3679
    let calc = DecayCalculator::new(86400).unwrap();
    let now = Utc::now();
    let entry = make_entry_with_access("cap-1", 1, now - Duration::hours(24));

    let priority = calc.compute_priority(&entry, now);
    // exp(-1) ≈ 0.36787944
    assert!((priority - 0.36787944).abs() < 1e-4);
    // priority > 0.1,不应触发降级
    assert!(!calc.should_demote(&entry, now));
}

#[test]
fn test_compute_priority_tau_24h_delta_t_72h_demotion() {
    // 任务要求:τ=24h,Δt=72h 时 priority < 0.1 触发降级
    let calc = DecayCalculator::new(86400).unwrap();
    let now = Utc::now();
    let entry = make_entry_with_access("cap-1", 1, now - Duration::hours(72));

    let priority = calc.compute_priority(&entry, now);
    // exp(-3) ≈ 0.04978707
    assert!((priority - 0.04978707).abs() < 1e-4);
    // priority < 0.1,应触发降级
    assert!(priority < DEMOTION_THRESHOLD);
    assert!(calc.should_demote(&entry, now));
}

#[test]
fn test_compute_priority_high_access_count() {
    // 高访问次数的条目即使长时间未访问,优先级也可能高于阈值
    let calc = DecayCalculator::new(86400).unwrap();
    let now = Utc::now();
    // access_count = 100,Δt = 72h
    let entry = make_entry_with_access("cap-1", 100, now - Duration::hours(72));

    let priority = calc.compute_priority(&entry, now);
    // priority = 100 × exp(-3) ≈ 100 × 0.0498 ≈ 4.98
    assert!((priority - 4.98).abs() < 0.1);
    // priority > 0.1,不应触发降级
    assert!(!calc.should_demote(&entry, now));
}

#[test]
fn test_should_demote_at_threshold() {
    // priority 刚好等于阈值 0.1 时不触发降级(严格小于)
    let calc = DecayCalculator::new(86400).unwrap();
    let now = Utc::now();

    // priority = access_count × exp(-Δt / τ) = 0.1
    // exp(-Δt / τ) = 0.1 / access_count
    // Δt = -τ × ln(0.1 / access_count)
    // 取 access_count = 1,Δt = -86400 × ln(0.1) ≈ 86400 × 2.3026 ≈ 198934 秒 ≈ 55.26 小时
    let delta_seconds = -86400.0 * (0.1_f64).ln();
    let delta_duration = Duration::seconds(delta_seconds as i64);
    let entry = make_entry_with_access("cap-1", 1, now - delta_duration);

    let priority = calc.compute_priority(&entry, now);
    // priority 应接近 0.1(浮点误差允许)
    assert!((priority - 0.1).abs() < 1e-4);
}

#[test]
fn test_compute_priority_negative_delta_t() {
    // last_accessed_at 在未来(时钟漂移),Δt 应视为 0
    let calc = DecayCalculator::new(86400).unwrap();
    let now = Utc::now();
    let entry = make_entry_with_access("cap-1", 5, now + Duration::hours(1));

    let priority = calc.compute_priority(&entry, now);
    // Δt 视为 0,priority = access_count × 1 = 5
    assert!((priority - 5.0).abs() < 1e-6);
}

#[test]
fn test_demotion_threshold_constant() {
    // 验证降级阈值常量为 0.1
    assert_eq!(DEMOTION_THRESHOLD, 0.1);
}

#[test]
fn test_compute_priority_decay_monotonic() {
    // 衰减单调性:Δt 越大,priority 越小
    let calc = DecayCalculator::new(86400).unwrap();
    let now = Utc::now();
    let access_count = 10;

    let p1 = calc.compute_priority(
        &make_entry_with_access("cap-1", access_count, now - Duration::hours(1)),
        now,
    );
    let p6 = calc.compute_priority(
        &make_entry_with_access("cap-2", access_count, now - Duration::hours(6)),
        now,
    );
    let p24 = calc.compute_priority(
        &make_entry_with_access("cap-3", access_count, now - Duration::hours(24)),
        now,
    );
    let p72 = calc.compute_priority(
        &make_entry_with_access("cap-4", access_count, now - Duration::hours(72)),
        now,
    );

    // 单调递减
    assert!(p1 > p6);
    assert!(p6 > p24);
    assert!(p24 > p72);
}

#[test]
fn test_compute_priority_access_count_linear() {
    // 固定 Δt 时,priority 与 access_count 成正比
    let calc = DecayCalculator::new(86400).unwrap();
    let now = Utc::now();
    let last_accessed = now - Duration::hours(12);

    let p1 = calc.compute_priority(&make_entry_with_access("cap-1", 1, last_accessed), now);
    let p10 = calc.compute_priority(&make_entry_with_access("cap-2", 10, last_accessed), now);
    let p100 = calc.compute_priority(&make_entry_with_access("cap-3", 100, last_accessed), now);

    // 线性关系:p10 = 10 × p1,p100 = 100 × p1
    assert!((p10 - 10.0 * p1).abs() < 1e-4);
    assert!((p100 - 100.0 * p1).abs() < 1e-3);
}

#[test]
fn test_compute_priority_custom_tau() {
    // 自定义 τ 验证:τ 越大衰减越慢
    let now = Utc::now();
    let entry = make_entry_with_access("cap-1", 1, now - Duration::hours(24));

    // τ=12h(衰减更快)
    let calc_fast = DecayCalculator::new(43200).unwrap();
    let priority_fast = calc_fast.compute_priority(&entry, now);

    // τ=24h(默认)
    let calc_default = DecayCalculator::new(86400).unwrap();
    let priority_default = calc_default.compute_priority(&entry, now);

    // τ=48h(衰减更慢)
    let calc_slow = DecayCalculator::new(172800).unwrap();
    let priority_slow = calc_slow.compute_priority(&entry, now);

    // τ 越大,priority 越高(衰减越慢)
    assert!(priority_fast < priority_default);
    assert!(priority_default < priority_slow);
}

#[test]
fn test_should_demote_recently_accessed() {
    // 刚访问的条目不应触发降级
    let calc = DecayCalculator::new(86400).unwrap();
    let now = Utc::now();
    let entry = make_entry_with_access("cap-1", 1, now);

    assert!(!calc.should_demote(&entry, now));
}

#[test]
fn test_should_demote_never_accessed() {
    // 从未访问的条目应触发降级(access_count=0,priority=0 < 0.1)
    let calc = DecayCalculator::new(86400).unwrap();
    let now = Utc::now();
    let entry = make_entry_with_access("cap-1", 0, now);

    assert!(calc.should_demote(&entry, now));
}
