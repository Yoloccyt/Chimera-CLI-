//! HCW 分层上下文窗口属性测试 — 验证压缩与窗口选择的不变量
//!
//! 对应 SubTask 20.3:补充 hcw-window proptest
//!
//! # 验证的不变量
//! 1. 压缩率 ≥ 1.0(压缩不会增加大小)
//! 2. 窗口选择单调性(complexity ↑ → tier ↑)
//! 3. 压缩后大小 ≤ 原始大小
//! 4. 保留条目数 ≤ 原始条目数
//! 5. 空条目集压缩为无操作(algorithm == "none")
//!
//! # 策略
//! - 生成随机条目集合(随机 token_size、随机条目数)
//! - 对不同 target_size 调用 compress,验证不变量
//! - 生成随机 complexity 值,验证窗口选择单调性

#![forbid(unsafe_code)]

use chrono::Utc;
use hcw_window::{ContextCompressor, ContextEntry, HcwConfig, WindowSelector};
use proptest::prelude::*;

/// 生成随机上下文条目集合
///
/// 每个条目的 token_size ∈ [10, 1000],确保压缩测试有足够的数据量
fn make_entries(count: usize, token_sizes: Vec<usize>) -> Vec<ContextEntry> {
    token_sizes
        .into_iter()
        .take(count)
        .enumerate()
        .map(|(i, ts)| {
            ContextEntry::new(
                format!("e-{i}"),
                format!("file-{i}"),
                format!("content-{i}"),
                ts,
            )
        })
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// 不变量 1:压缩率 ≥ 1.0(compressed_size ≤ original_size)
    ///
    /// 对任意条目集合与任意 target_size,压缩后的 compression_ratio ≥ 1.0。
    /// 压缩只会减少或保持大小,不会增加。
    #[test]
    fn test_compression_ratio_always_ge_one(
        token_sizes in prop::collection::vec(10usize..=1000, 1..50),
        target_size in 0usize..=50000,
    ) {
        let entries = make_entries(token_sizes.len(), token_sizes);
        let now = Utc::now();
        let report = ContextCompressor::compress(&HcwConfig::default(), &entries, target_size, None, now);

        prop_assert!(
            report.compression_ratio >= 1.0,
            "compression_ratio {} 应 ≥ 1.0(压缩不应增加大小)",
            report.compression_ratio
        );
        prop_assert!(
            report.compressed_size <= report.original_size,
            "compressed_size {} 应 ≤ original_size {}",
            report.compressed_size,
            report.original_size
        );
    }

    /// 不变量 2:窗口选择单调性(complexity ↑ → tier 不降)
    ///
    /// 对任意 c1 < c2,select(c1) <= select(c2)(层级序:L0 < L1 < L2 < L3)
    #[test]
    fn test_window_selection_monotonicity(
        c1 in 0.0f32..=1.0,
        c2 in 0.0f32..=1.0,
    ) {
        // 确保 c1 <= c2,若 c1 > c2 则交换
        let (lo, hi) = if c1 <= c2 { (c1, c2) } else { (c2, c1) };

        let tier_lo = WindowSelector::select(lo);
        let tier_hi = WindowSelector::select(hi);

        // WindowTier 派生了 PartialOrd + Ord,L0 < L1 < L2 < L3
        prop_assert!(
            tier_lo <= tier_hi,
            "complexity {} → {:?} 应 ≤ complexity {} → {:?}(单调性)",
            lo, tier_lo, hi, tier_hi
        );
    }

    /// 不变量 3:保留条目数 ≤ 原始条目数
    ///
    /// 压缩后的 retained_count 不应超过 original_count
    #[test]
    fn test_retained_count_le_original_count(
        token_sizes in prop::collection::vec(10usize..=1000, 1..50),
        target_size in 0usize..=50000,
    ) {
        let entries = make_entries(token_sizes.len(), token_sizes);
        let now = Utc::now();
        let report = ContextCompressor::compress(&HcwConfig::default(), &entries, target_size, None, now);

        prop_assert!(
            report.retained_count <= report.original_count,
            "retained_count {} 应 ≤ original_count {}",
            report.retained_count,
            report.original_count
        );
        // dropped + retained = original(守恒)
        prop_assert_eq!(
            report.retained_count + report.dropped_count,
            report.original_count
        );
    }

    /// 不变量 4:空条目集压缩为无操作
    ///
    /// 空条目集调用 compress 应返回 algorithm == "none",不产生保留条目
    #[test]
    fn test_empty_entries_no_compression(
        target_size in 0usize..=50000,
    ) {
        let entries: Vec<ContextEntry> = Vec::new();
        let now = Utc::now();
        let report = ContextCompressor::compress(&HcwConfig::default(), &entries, target_size, None, now);

        prop_assert_eq!(report.algorithm, "none");
        prop_assert_eq!(report.original_count, 0);
        prop_assert_eq!(report.retained_count, 0);
        prop_assert_eq!(report.dropped_count, 0);
        prop_assert_eq!(report.original_size, 0);
        prop_assert_eq!(report.compressed_size, 0);
    }

    /// 不变量 5:原始大小 ≤ target_size 时不压缩(algorithm == "none")
    ///
    /// 当原始大小不超过目标大小时,无需压缩,algorithm 应为 "none"
    #[test]
    fn test_no_compression_when_under_target(
        token_sizes in prop::collection::vec(10usize..=100, 1..10),
    ) {
        let entries = make_entries(token_sizes.len(), token_sizes.clone());
        let original_size: usize = token_sizes.iter().sum();
        // target_size 设为原始大小(刚好不触发压缩)
        let target_size = original_size;
        let now = Utc::now();
        let report = ContextCompressor::compress(&HcwConfig::default(), &entries, target_size, None, now);

        prop_assert_eq!(report.algorithm, "none");
        prop_assert_eq!(report.compression_ratio, 1.0);
        prop_assert_eq!(report.dropped_count, 0);
    }
}
