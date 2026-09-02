//! HiLS-Attention 集成测试 — HCW 窗口选择器集成（v3.4.0 §7.4）
//!
//! 覆盖: 顶层 API 可达性 / HiLSWindowSelector 与 WindowSelector 互补 /
//! 长上下文块选择 / 批量查询 / proptest 块选择不变量

#![forbid(unsafe_code)]

use hcw_window::{Chunk, HiLSAttention, HiLSWindowSelector, WindowSelector, WindowTier};
use nexus_core::CLV;
use proptest::prelude::*;

fn unit_clv(dim: usize) -> CLV {
    // 算法体已收敛到 L1 `nexus_core::CLV::basis`(单一权威构造器),
    // 此处仅保留本地签名,避免改动本文件内数十处调用点。
    // basis 越界返回 None;夹具若下标非法则该测试无效,直接 expect 暴露。
    CLV::basis(dim).expect("测试夹具:下标须在 CLV::DIMENSION 内")
}

fn chunk(id: usize, dim: usize, entropy: f32) -> Chunk {
    Chunk::new(id, unit_clv(dim), 128, entropy)
}

// ----------------------------------------------------------
// 顶层 API 可达性
// ----------------------------------------------------------

#[test]
fn top_level_api_accessible() {
    use hcw_window::prelude::*;
    let hils = HiLSAttention::default();
    let selector = HiLSWindowSelector::new(hils);
    let query = unit_clv(0);
    let chunks = vec![chunk(0, 0, 0.0)];
    let top = selector.select_top_chunks(&query, &chunks);
    assert_eq!(top.len(), 1);
    let _output = AttentionOutput::new();
}

// ----------------------------------------------------------
// HiLSWindowSelector 与 WindowSelector 互补
// ----------------------------------------------------------

#[test]
fn hils_complements_window_selector() {
    // WindowSelector: 复杂度 → 窗口层级（L0/L1/L2/L3）
    let tier = WindowSelector::select(0.9);
    assert_eq!(tier, WindowTier::L3, "高复杂度应选 L3 窗口");
    // HiLSWindowSelector: 窗口层级内的稀疏块选择
    let hils = HiLSWindowSelector::new(HiLSAttention::new(128, 4, 256));
    let query = unit_clv(0);
    let chunks: Vec<Chunk> = (0..10).map(|i| chunk(i, i % 4, 0.1)).collect();
    let top = hils.select_top_chunks(&query, &chunks);
    assert!(top.len() <= 4, "HiLS 应稀疏选择 ≤ top_k 块");
    assert_eq!(top[0], 0, "最相关块（dim 0）应排第一");
}

#[test]
fn window_selector_still_pure_function() {
    // 既有 WindowSelector::select 纯函数不受 HiLS 影响（O(1) 决策保持）
    assert_eq!(WindowSelector::select(0.1), WindowTier::L0);
    assert_eq!(WindowSelector::select(0.3), WindowTier::L1);
    assert_eq!(WindowSelector::select(0.6), WindowTier::L2);
    assert_eq!(WindowSelector::select(0.9), WindowTier::L3);
}

// ----------------------------------------------------------
// 长上下文块选择
// ----------------------------------------------------------

#[test]
fn long_context_sparse_selection() {
    // 模拟长上下文（64K/512K）: 大量块中稀疏选择 Top-K
    let hils = HiLSAttention::new(128, 8, 512);
    let query = unit_clv(3);
    // 100 块，其中块 3/7/11 与 query 相关（dim 3）
    let chunks: Vec<Chunk> = (0..100)
        .map(|i| {
            let dim = if i % 4 == 3 { 3 } else { i % 512 };
            chunk(i, dim, 0.05)
        })
        .collect();
    let output = hils.forward(&query, &chunks);
    assert_eq!(output.selected_chunk_ids.len(), 8, "应选 Top-8 块");
    // 相关块（dim 3）应优先进入 Top-K
    let selected: Vec<usize> = output.selected_chunk_ids.clone();
    let relevant_in_top = selected.iter().filter(|&&id| id % 4 == 3).count();
    assert!(
        relevant_in_top >= 3,
        "相关块应优先进入 Top-K（实际 {}）",
        relevant_in_top
    );
    assert_eq!(output.local_window_tokens, 512);
}

#[test]
fn batched_queries_independent_selection() {
    let hils = HiLSAttention::new(128, 2, 256);
    // 多个查询打包（高效 kernel，m_query_pack）
    let queries = vec![unit_clv(0), unit_clv(1), unit_clv(2), unit_clv(3)];
    let chunks: Vec<Chunk> = (0..4).map(|i| chunk(i, i, 0.0)).collect();
    let outputs = hils.forward_batched(&queries, &chunks);
    assert_eq!(outputs.len(), 4, "每查询一个输出");
    // 各查询独立选中各自最相关块
    for (i, output) in outputs.iter().enumerate() {
        assert_eq!(output.selected_chunk_ids[0], i, "查询 {} 应选中块 {}", i, i);
    }
}

// ----------------------------------------------------------
// 两级 softmax: 块间 Top-K + 块内权重
// ----------------------------------------------------------

#[test]
fn two_level_softmax_chunk_and_intra() {
    let hils = HiLSAttention::new(128, 2, 256);
    let query = unit_clv(0);
    let chunks = vec![chunk(0, 0, 0.5), chunk(1, 1, 0.5), chunk(2, 2, 0.5)];
    let output = hils.forward(&query, &chunks);
    // 块间 Top-K
    assert_eq!(output.selected_chunk_ids.len(), 2);
    assert_eq!(output.chunk_weights.len(), 2);
    // 块内权重（选中块的 compute_intra_attention 归一化）
    let selected_chunk = &chunks[output.selected_chunk_ids[0]];
    let intra = selected_chunk.compute_intra_attention(&query);
    assert_eq!(intra.len(), 128, "块内 128 token");
    let sum: f32 = intra.iter().sum();
    assert!((sum - 1.0).abs() < 1e-5, "块内权重归一化");
}

// ----------------------------------------------------------
// proptest: 块选择不变量
// ----------------------------------------------------------

proptest! {
    /// 任意 top_k，选中块数 ≤ min(top_k, 总块数)
    #[test]
    fn selected_count_bounded(
        top_k in 1usize..16,
        num_chunks in 1usize..30,
    ) {
        let hils = HiLSAttention::new(128, top_k, 256);
        let query = unit_clv(0);
        let chunks: Vec<Chunk> = (0..num_chunks).map(|i| chunk(i, i % 8, 0.1)).collect();
        let output = hils.forward(&query, &chunks);
        prop_assert!(output.selected_chunk_ids.len() <= top_k);
        prop_assert!(output.selected_chunk_ids.len() <= num_chunks);
    }

    /// 自相关块（与 query 同向）必然在 Top-K 中（当块数 ≥ top_k 时相关性主导）
    #[test]
    fn self_relevant_chunk_ranked_high(
        entropy in 0.0f32..1.0,
    ) {
        let hils = HiLSAttention::new(128, 1, 256); // top_k=1
        let query = unit_clv(5);
        // 块 0 与 query 同向（最相关），块 1/2 正交
        let chunks = vec![
            chunk(0, 5, entropy),
            chunk(1, 100, entropy),
            chunk(2, 200, entropy),
        ];
        let output = hils.forward(&query, &chunks);
        prop_assert_eq!(output.selected_chunk_ids[0], 0, "自相关块应排第一");
    }
}
