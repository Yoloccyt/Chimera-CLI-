//! 公共 Top-K 收敛工具 — 红线 #8(WS-2 C1)
//!
//! 提供 `xts_top_k` / `xts_top_k_by`,以 **O(n) 部分排序**
//! (`select_nth_unstable_by`) + **O(k log k) 局部排序**取代"全排后截断"的
//! `sort_by` 全排序 (O(n log n)),用于**降序**取 Top-K。
//!
//! # 设计
//!
//! - `k` 自动钳制到切片长度(`k.min(len)`),因此 `k >= len` 等价于全量降序排序。
//! - 分区(选第 `k-1` 大)后,`[0..k)` 为降序意义下的 Top-k 无序集合;
//!   再对 `[0..k)` 做 **稳定** `sort_by(降序)`,保证返回段严格降序且
//!   与原有"全排后截断"的相等元素相对顺序完全一致(WS-2 等价性要求)。
//! - 复杂度 O(n + k log k),满足红线 #8(select_nth_unstable_by 分区)。
//!
//! # 用法
//!
//! ```
//! use nexus_contracts::util::{xts_top_k, xts_top_k_by};
//!
//! let mut v = vec![3u32, 1, 4, 1, 5, 9, 2];
//! let top = xts_top_k(&mut v, 3);
//! assert_eq!(top, &[9, 5, 4]);
//!
//! // by 版本(自定义比较器,支持 f32 等非 Ord 类型)
//! let mut scored = vec![("a", 0.1f32), ("b", 0.9), ("c", 0.5)];
//! let top = xts_top_k_by(&mut scored, 2, |a, b| {
//!     b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
//! });
//! assert_eq!(top[0].0, "b");
//! assert_eq!(top[1].0, "c");
//! ```
//!
//! > ADR-033"纯类型 + 零逻辑"约束下的纯函数工具先例(与 `test_scale` /
//! > `archive_monotonicity` / `mcsm` 同级,无 IO 无状态变更)。

use std::cmp::Ordering;

/// 降序取前 `k` 个元素(需 `T: Ord`),返回前 `k` 段切片(严格降序)。
///
/// - `k` 钳制到 `items.len()`:若 `k >= len` 则等价于全量降序排序。
/// - `k == 0` 返回空切片。
/// - 复杂度:`select_nth_unstable_by` O(n) + 前 k 段 `sort_unstable_by` O(k log k)。
///
/// f32/f64 等非 `Ord` 类型请使用 [`xts_top_k_by`] 并提供 `partial_cmp` 比较器。
#[inline]
pub fn xts_top_k<T: Ord>(items: &mut [T], k: usize) -> &mut [T] {
    xts_top_k_by(items, k, |a, b| b.cmp(a))
}

/// 降序取前 `k` 个元素(自定义比较器),返回前 `k` 段切片(按 `cmp` 序)。
///
/// - `k` 钳制到 `items.len()`:若 `k >= len` 则等价于全量降序排序。
/// - `k == 0` 返回空切片。
/// - 复杂度:`select_nth_unstable_by` O(n) + 前 k 段 `sort_unstable_by` O(k log k)。
///
/// 比较器需表达"降序"(如 f32 评分:`|a, b| b.1.partial_cmp(&a.1)...`),
/// 与原有 `sort_by` 全排后截断的语义等价。
#[inline]
pub fn xts_top_k_by<T, F>(items: &mut [T], k: usize, mut cmp: F) -> &mut [T]
where
    F: FnMut(&T, &T) -> Ordering,
{
    let n = items.len();
    let k = k.min(n);

    if k == 0 {
        return &mut items[..0];
    }

    // 分区:[0..k) 为降序意义下最大的 k 个,内部无序
    items.select_nth_unstable_by(k - 1, &mut cmp);

    // 前 k 段稳定排序为严格降序(相等元素保持相对顺序,与原 sort_by 全排语义一致)
    items[..k].sort_by(cmp);

    &mut items[..k]
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    // ============================================================
    // 单测
    // ============================================================

    #[test]
    fn k_zero_returns_empty() {
        let mut v = vec![5, 3, 8, 1];
        let top = xts_top_k(&mut v, 0);
        assert!(top.is_empty());
    }

    #[test]
    fn k_equals_len_sorts_all_desc() {
        let mut v = vec![5, 3, 8, 1, 3];
        let top = xts_top_k(&mut v, 5);
        assert_eq!(top, &[8, 5, 3, 3, 1]);
    }

    #[test]
    fn k_exceeds_len_clamps_to_len() {
        let mut v = vec![2, 9, 4];
        let top = xts_top_k(&mut v, 10);
        assert_eq!(top, &[9, 4, 2]);
    }

    #[test]
    fn single_element() {
        let mut v = vec![42];
        // k=1
        assert_eq!(xts_top_k(&mut v, 1), &[42]);
        // k=0
        assert!(xts_top_k(&mut v, 0).is_empty());
        // k>len
        assert_eq!(xts_top_k(&mut v, 3), &[42]);
    }

    #[test]
    fn empty_slice() {
        let mut v: Vec<i32> = Vec::new();
        assert!(xts_top_k(&mut v, 0).is_empty());
        assert!(xts_top_k(&mut v, 5).is_empty());
    }

    #[test]
    fn all_duplicates() {
        let mut v = vec![7, 7, 7, 7, 7];
        let top = xts_top_k(&mut v, 3);
        assert_eq!(top, &[7, 7, 7]);
        assert_eq!(top.len(), 3);
    }

    #[test]
    fn already_descending_input() {
        let mut v = vec![10, 9, 8, 7, 1];
        let top = xts_top_k(&mut v, 3);
        assert_eq!(top, &[10, 9, 8]);
    }

    #[test]
    fn already_ascending_input() {
        let mut v = vec![1, 2, 3, 4, 9];
        let top = xts_top_k(&mut v, 3);
        assert_eq!(top, &[9, 4, 3]);
    }

    #[test]
    fn by_version_f32_desc() {
        let mut scored = vec![("a", 0.1f32), ("b", 0.9), ("c", 0.5), ("d", 0.9)];
        let top = xts_top_k_by(&mut scored, 3, |a, b| {
            b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
        });
        // 评分降序:0.9, 0.9, 0.5
        assert_eq!(top[0].1, 0.9);
        assert_eq!(top[1].1, 0.9);
        assert_eq!(top[2].1, 0.5);
    }

    #[test]
    fn by_version_k_clamped() {
        let mut scored = vec![(1, 0.3f32), (2, 0.1)];
        let top = xts_top_k_by(&mut scored, 100, |a, b| {
            b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
        });
        assert_eq!(top.len(), 2);
        assert_eq!(top[0].1, 0.3);
        assert!(top.iter().all(|w| w.1 == 0.3 || w.1 == 0.1));
    }

    // ============================================================
    // proptest 属性:(1) 前 k 降序 (2) 前 k 包含全库 top-k 集合 (3) i<k≤j → v[i]≥v[j]
    // ============================================================

    proptest::proptest! {
        #![proptest_config(
            proptest::prelude::ProptestConfig { max_shrink_iters: 512, ..Default::default() }
        )]

        #[test]
        fn prop_ordering_and_partition(v: Vec<i32>, k in 0usize..40) {
            let mut v = v;
            let n = v.len();
            let k = k.min(n);

            xts_top_k(&mut v, k);

            // 性质 1:前 k 段严格降序
            if k >= 2 {
                prop_assert!(v[..k].windows(2).all(|w| w[0] >= w[1]));
            }

            // 性质 3:跨边界支配 i<k ≤ j → v[i] >= v[j]
            if k > 0 && k < n {
                let head_min = *v[..k].iter().min().unwrap();
                let tail_max = *v[k..].iter().max().unwrap();
                prop_assert!(head_min >= tail_max);
            }

            // 性质 2:前 k 集合 == 全库 top-k 集合(与全排后截断的多重集合一致)
            let mut sorted = v.clone();
            sorted.sort_unstable_by(|a, b| b.cmp(a));
            let expected = &sorted[..k];
            let mut got: Vec<i32> = v[..k].to_vec();
            got.sort_unstable_by(|a, b| b.cmp(a));
            prop_assert_eq!(&got, expected);
        }
    }

    // 与 sort_by + truncate 语义等价(批量 1000,含 by 版本)
    #[test]
    fn equivalent_to_sort_by_truncate_ord() {
        let config = proptest::test_runner::Config {
            cases: 1000,
            ..Default::default()
        };
        proptest::proptest!(config, |(v in any::<Vec<i32>>(), k in 0usize..30usize)| {
            let n = v.len();
            let k = k.min(n);

            // 参考:全排(降序)后截断
            let mut expected = v.clone();
            expected.sort_unstable_by(|a, b| b.cmp(a));
            expected.truncate(k);

            // 被测:xts_top_k
            let mut got_v = v.clone();
            let got = xts_top_k(&mut got_v, k);

            // 归一化(两者均已降序),逐元素比对
            let mut got_sorted = got.to_vec();
            got_sorted.sort_unstable_by(|a, b| b.cmp(a));
            prop_assert_eq!(&got_sorted, &expected);
        });
    }

    // 与 sort_by + truncate 语义等价(by 版本,1000 批,自定义比较器)
    #[test]
    fn equivalent_to_sort_by_truncate_by() {
        let config = proptest::test_runner::Config {
            cases: 1000,
            ..Default::default()
        };
        proptest::proptest!(config, |(v in proptest::collection::vec(any::<i32>(), 0..40), k in 0usize..30usize)| {
            // 用 (id, score) 测试 by 版,score 为 -v 以制造区分度
            let data: Vec<(usize, i32)> = v.iter().copied().enumerate().collect();
            let n = data.len();
            let k = k.min(n);
            let cmp = |a: &(usize, i32), b: &(usize, i32)| {
                b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
            };

            // 参考:全排(降序)后截断
            let mut expected = data.clone();
            expected.sort_unstable_by(&cmp);
            expected.truncate(k);

            // 被测:xts_top_k_by
            let mut got_data = data.clone();
            let got = xts_top_k_by(&mut got_data, k, cmp);

            let mut got_sorted = got.to_vec();
            got_sorted.sort_unstable_by(&cmp);
            prop_assert_eq!(&got_sorted, &expected);
        });
    }
}
