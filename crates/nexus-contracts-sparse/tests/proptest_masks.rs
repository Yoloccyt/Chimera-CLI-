//! 属性测试（W2b）— SparseMask::select_top_k 稀疏度/成员/序等价不变量
//!
//! WHY: select_top_k 是 §6.2 红线「Top-K 必须 select_nth_unstable」的落地形态
//! （xts_top_k_by 同族）。本属性用严格递增评分（无并列）钉死三件事：
//! sparsity 算术、active 集合 == 全排序参考实现的 Top-K、active_ids 按评分降序。
#![forbid(unsafe_code)]
#![allow(clippy::unwrap_used, clippy::expect_used)]
// 集成测试惯用 expect/unwrap（G-82 文档化豁免：integration-test idiom 整文件豁免）

use nexus_contracts_sparse::sparse::masks::SparseMask;
use proptest::prelude::*;

proptest! {
    /// 严格递增评分下：sparsity == 1 - k/n；active 集合 == 分数最高的 k 个；
    /// active_ids 顺序 == 评分降序（与全排序参考实现逐点等价）。
    #[test]
    fn select_top_k_matches_full_sort(n in 2usize..64, k_raw in 1usize..64) {
        let k = k_raw.min(n - 1).max(1);
        let ids: Vec<u32> = (0..n as u32).collect();
        // 严格递增、无并列 ⇒ Top-K 成员与序唯一确定
        let scores: Vec<f32> = (0..n).map(|i| i as f32).collect();

        let mask = SparseMask::select_top_k(&ids, &scores, k);

        prop_assert!((mask.sparsity_ratio - (1.0 - k as f32 / n as f32)).abs() < 1e-6);
        prop_assert_eq!(mask.active_ids.len(), k);

        // 参考实现：全排序取前 k（分数降序）
        let mut ranked = ids.clone();
        ranked.sort_by(|&a, &b| {
            scores[b as usize]
                .partial_cmp(&scores[a as usize])
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        ranked.truncate(k);
        prop_assert_eq!(mask.active_ids.clone(), ranked);

        // 成员性：每个 active id 都来自候选集
        for id in &mask.active_ids {
            prop_assert!(ids.contains(id));
        }
    }
}
