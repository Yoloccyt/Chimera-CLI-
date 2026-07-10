//! repo-wiki 不变量属性测试 — KNN 检索返回距离最小的 k 个结果
//!
//! 对应架构层:L5 Knowledge
//! 对应创新点:ISCM(Inter-Shared Cross Module,跨层共享索引)
//!
//! # 测试目标
//! 验证 VectorIndex KNN 检索在随机向量集下满足:
//! 1. **返回数量 = min(k, n_entries)** — 不多不少
//! 2. **结果按相似度降序排列** — 高相似度在前
//! 3. **返回的 k 个确实是距离最小的** — 与暴力搜索结果一致
//!
//! # 设计决策
//! 测试内自带余弦相似度参考实现(不依赖 nexus_core::cosine_similarity_slices),
//! WHY:独立参考实现能捕获"实现与参考同源 bug"的盲区(如两者共享同一精度缺陷)。
//!
//! # 语法约束(§4.4 规则)
//! proptest 1.11+ 用 block-named 语法

#![forbid(unsafe_code)]

use proptest::prelude::*;
use repo_wiki::VectorIndex;

/// 参考余弦相似度实现(独立于 nexus_core,避免同源 bug 盲区)
///
/// 返回值与 nexus_core::cosine_similarity_slices 语义一致:
/// 零向量返回 0.0(非 NaN),值域 [-1.0, 1.0]
fn cosine_sim_ref(a: &[f32], b: &[f32]) -> f32 {
    let len = a.len().min(b.len());
    let dot: f32 = a[..len]
        .iter()
        .zip(b[..len].iter())
        .map(|(x, y)| x * y)
        .sum();
    let norm_a: f32 = a[..len].iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b[..len].iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a == 0.0 || norm_b == 0.0 {
        0.0
    } else {
        dot / (norm_a * norm_b)
    }
}

proptest! {
    #[test]
    fn prop_knn_returns_nearest_k(
        dim in 2usize..=16,
        n_entries in 0usize..=30,
        k in 1usize..=10,
        seed in any::<u64>(),
    ) {
        // 用 seed 驱动简单 LCG 生成 [-1.0, 1.0] 范围的 f32
        // WHY 自定义生成器:避免引入 rand 依赖,且保证可复现
        let mut state = seed.wrapping_add(1); // 避免 seed=0 导致初始全零
        let mut next_f32 = || {
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            let bits = (state >> 33) as i32;
            (bits as f32) / (i32::MAX as f32)
        };

        let idx = VectorIndex::new(dim);
        let mut all_vectors: Vec<Vec<f32>> = Vec::with_capacity(n_entries);
        for i in 0..n_entries {
            let v: Vec<f32> = (0..dim).map(|_| next_f32()).collect();
            idx.upsert(&format!("e-{i}"), &v)
                .expect("upsert 维度匹配,应成功");
            all_vectors.push(v);
        }
        let query: Vec<f32> = (0..dim).map(|_| next_f32()).collect();

        let results = idx.search(&query, k).expect("search 维度匹配,应成功");

        // === 不变量1:返回数量 = min(k, n_entries) ===
        let expected_len = k.min(n_entries);
        prop_assert_eq!(
            results.len(),
            expected_len,
            "返回数量应为 min(k={}, n={}) = {}, got {}",
            k, n_entries, expected_len, results.len()
        );

        if results.is_empty() {
            return Ok(());
        }

        // === 不变量2:结果按相似度降序排列 ===
        for w in results.windows(2) {
            prop_assert!(
                w[0].1 >= w[1].1 || (w[0].1 - w[1].1).abs() < 1e-5,
                "结果应降序排列, got {} then {}",
                w[0].1, w[1].1
            );
        }

        // === 不变量3:返回的 k 个确实是相似度最高的(暴力搜索对照) ===
        let mut brute: Vec<(String, f32)> = all_vectors
            .iter()
            .enumerate()
            .map(|(i, v)| (format!("e-{i}"), cosine_sim_ref(&query, v)))
            .collect();
        // 降序排列(高相似度在前),与 KNN search 的输出顺序一致
        brute.sort_by(|a, b| {
            b.1.partial_cmp(&a.1)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        let expected_topk: Vec<(String, f32)> =
            brute.into_iter().take(k).collect();

        prop_assert_eq!(results.len(), expected_topk.len());
        for (actual, expected) in results.iter().zip(expected_topk.iter()) {
            prop_assert_eq!(
                &actual.0, &expected.0,
                "KNN 返回的 ID 应与暴力搜索一致"
            );
            prop_assert!(
                (actual.1 - expected.1).abs() < 1e-4,
                "相似度应一致: KNN={} vs brute={}",
                actual.1, expected.1
            );
        }
    }
}
