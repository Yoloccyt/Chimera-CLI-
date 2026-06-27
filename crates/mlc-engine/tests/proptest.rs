//! MLC L2 语义记忆属性测试 — 验证 recall_by_clv 返回的相似度分数不变量
//!
//! 对应 SubTask 15.12:引入 proptest 属性测试
//!
//! # 验证的不变量
//! L2 `recall_by_clv` 返回的所有相似度分数 ∈ [0.0, 1.0]
//!
//! # 策略
//! - 生成随机 CLV 向量(512 维,值 ∈ [-1.0, 1.0],覆盖负值以测试 clamp 下界)
//! - 插入若干条目后用随机 query 召回
//! - 断言所有返回的分数 ∈ [0.0, 1.0]
//!
//! # WHY 使用 [-1.0, 1.0] 而非 [0.0, 1.0]
//! 非负值向量的余弦相似度天然 ∈ [0.0, 1.0],无法测试 clamp 到 0.0 的路径。
//! 包含负值后,余弦相似度可能为负(语义相反),clamp 到 0.0 的逻辑才会被触发。

#![forbid(unsafe_code)]

use mlc_engine::{MemoryEntry, MemoryTier, SemanticMemory};
use nexus_core::CLV;
use proptest::prelude::*;

/// 将任意可显示错误转换为 TestCaseError(避免 unwrap,用 ? 传播)
fn fail<E: std::fmt::Display>(e: E) -> TestCaseError {
    TestCaseError::fail(format!("{e}"))
}

/// 从 Vec<f32> 构造 CLV,失败时返回 TestCaseError
fn make_clv(v: Vec<f32>) -> Result<CLV, TestCaseError> {
    CLV::from_vec(v).map_err(fail)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// 不变量:recall_by_clv 返回的所有相似度分数 ∈ [0.0, 1.0]
    ///
    /// 生成随机 query 与若干随机条目向量(含负值),插入后召回,
    /// 验证每个返回分数都被正确 clamp 到 [0.0, 1.0]
    #[test]
    fn test_recall_by_clv_scores_in_unit_range(
        query_vec in prop::collection::vec(-1.0f32..=1.0f32, CLV::DIMENSION),
        entry_vecs in prop::collection::vec(
            prop::collection::vec(-1.0f32..=1.0f32, CLV::DIMENSION),
            1..20
        ),
        top_k in 1usize..=50,
    ) {
        let mem = SemanticMemory::new(64);
        let query = make_clv(query_vec)?;

        // 插入随机条目(每个携带随机 CLV)
        for (i, v) in entry_vecs.into_iter().enumerate() {
            let clv = make_clv(v)?;
            let entry = MemoryEntry::new(
                format!("m-{i}"),
                format!("content-{i}"),
                MemoryTier::L2Semantic,
            )
            .with_clv(clv);
            mem.insert(entry).map_err(fail)?;
        }

        // 召回 Top-K
        let results = mem.recall_by_clv(&query, top_k).map_err(fail)?;

        // 核心不变量:所有分数 ∈ [0.0, 1.0]
        for (id, score) in &results {
            prop_assert!(
                (0.0..=1.0).contains(score),
                "条目 {} 的相似度分数 {} 超出 [0.0, 1.0]",
                id,
                score
            );
        }
    }

    /// 不变量:recall_by_clv 返回的分数按降序排列
    ///
    /// Top-K 召回应按相似度从高到低排序
    #[test]
    fn test_recall_by_clv_scores_descending(
        query_vec in prop::collection::vec(-1.0f32..=1.0f32, CLV::DIMENSION),
        entry_vecs in prop::collection::vec(
            prop::collection::vec(-1.0f32..=1.0f32, CLV::DIMENSION),
            2..20
        ),
        top_k in 1usize..=20,
    ) {
        let mem = SemanticMemory::new(64);
        let query = make_clv(query_vec)?;

        for (i, v) in entry_vecs.into_iter().enumerate() {
            let clv = make_clv(v)?;
            let entry = MemoryEntry::new(
                format!("m-{i}"),
                format!("content-{i}"),
                MemoryTier::L2Semantic,
            )
            .with_clv(clv);
            mem.insert(entry).map_err(fail)?;
        }

        let results = mem.recall_by_clv(&query, top_k).map_err(fail)?;

        // 验证降序排列(允许相邻元素相等)
        for window in results.windows(2) {
            prop_assert!(
                window[0].1 >= window[1].1,
                "分数未按降序排列: {} < {}",
                window[0].1,
                window[1].1
            );
        }
    }

    /// 不变量:recall_by_clv 返回数量 ≤ top_k 且 ≤ 条目总数
    #[test]
    fn test_recall_by_clv_result_count_bounded(
        query_vec in prop::collection::vec(-1.0f32..=1.0f32, CLV::DIMENSION),
        entry_count in 1usize..=30,
        top_k in 1usize..=50,
    ) {
        let mem = SemanticMemory::new(64);
        let query = make_clv(query_vec)?;

        for i in 0..entry_count {
            // 用固定种子构造 CLV,确保非零
            let v = vec![i as f32 * 0.01; CLV::DIMENSION];
            let clv = make_clv(v)?;
            let entry = MemoryEntry::new(
                format!("m-{i}"),
                format!("content-{i}"),
                MemoryTier::L2Semantic,
            )
            .with_clv(clv);
            mem.insert(entry).map_err(fail)?;
        }

        let results = mem.recall_by_clv(&query, top_k).map_err(fail)?;

        let expected_max = top_k.min(entry_count);
        prop_assert_eq!(
            results.len(),
            expected_max,
            "返回数量应为 min(top_k={}, entries={}) = {}, 实际 {}",
            top_k,
            entry_count,
            expected_max,
            results.len()
        );
    }
}
