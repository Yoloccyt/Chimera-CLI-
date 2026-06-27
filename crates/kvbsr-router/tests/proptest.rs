//! KVBSR 路由器属性测试 — 验证两级路由结果的不变量
//!
//! 对应 SubTask 20.4:补充 kvbsr-router proptest
//!
//! # 验证的不变量
//! 1. 路由结果数 ≤ top_k(自定义 Top-K 上限)
//! 2. 路由结果数 ≤ 候选集大小(总工具数)
//! 3. 重平衡后块数量 ≤ 工具数
//! 4. 路由分数 ∈ [-1.0, 1.0](余弦相似度范围)
//! 5. 路由结果非空(有工具时)
//!
//! # 策略
//! - 生成随机工具向量集合(随机块数、随机每块工具数)
//! - 构建块后用随机 CLV 路由,验证不变量
//! - 对不同 top_k 值验证结果数上限
//!
//! # 实现说明
//! `prop_assert!` 宏通过 early return(`return Err(...)`)报告失败,而非返回 `Result`。
//! 因此不能在 `async` 块内使用 `prop_assert!(...)?`。正确模式:
//! 1. 在 `rt.block_on(async { ... })` 内完成异步操作并收集结果数据
//! 2. 在 `proptest!` 闭包顶层使用 `prop_assert!`(无需 `?`,early return 自动生效)

#![forbid(unsafe_code)]

use event_bus::EventBus;
use kvbsr_router::{CoOccurrenceMatrix, KVBlockSemanticRouter, RoutingRequest, ToolVector};
use nexus_core::CLV;
use proptest::prelude::*;

/// 块/工具向量维度(与 KvbsrConfig 默认值一致)
const VECTOR_DIM: usize = 64;

/// 生成随机工具向量集合与共现矩阵
///
/// 每个块在 2 个独特维度上有高值,块内工具向量 = 基向量 + 小扰动,
/// 块内共现 > 阈值(150),块间无共现。
fn generate_random_tools(
    num_blocks: usize,
    tools_per_block: usize,
) -> (Vec<ToolVector>, CoOccurrenceMatrix) {
    let mut tools = Vec::with_capacity(num_blocks * tools_per_block);
    let mut co = CoOccurrenceMatrix::new();

    for bi in 0..num_blocks {
        // 块基向量:在 2 个独特维度上有高值(确保块间区分度)
        let mut base = vec![0.0_f32; VECTOR_DIM];
        let d1 = (bi * 4) % VECTOR_DIM;
        let d2 = (bi * 4 + 1) % VECTOR_DIM;
        base[d1] = 1.0;
        base[d2] = 1.0;

        for ti in 0..tools_per_block {
            let mut vector = base.clone();
            for v in vector.iter_mut() {
                *v += (ti as f32 * 0.01) - 0.1;
            }
            tools.push(ToolVector::new(format!("tool-{bi}-{ti}"), vector, 100));
        }

        // 块内工具共现 > 阈值(150 > 100)
        for ti in 0..tools_per_block {
            for tj in (ti + 1)..tools_per_block {
                co.insert(format!("tool-{bi}-{ti}"), format!("tool-{bi}-{tj}"), 150);
            }
        }
    }

    (tools, co)
}

/// 生成随机 CLV(前 64 维有值,其余为 0)
fn make_random_clv(query_vec: Vec<f32>) -> CLV {
    let mut full = vec![0.0_f32; CLV::DIMENSION];
    for (i, v) in query_vec.iter().take(VECTOR_DIM).enumerate() {
        full[i] = *v;
    }
    // 确保非零向量(避免归一化除零)
    if full.iter().all(|&v| v == 0.0) {
        full[0] = 1.0;
    }
    CLV::from_vec(full).unwrap_or_else(|_| CLV::zero())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// 不变量 1:路由结果数 ≤ top_k(自定义 Top-K 上限)
    ///
    /// 对任意工具集合与任意 top_k,路由返回的工具数不超过 top_k
    #[test]
    fn test_route_result_le_top_k(
        num_blocks in 1usize..=10,
        tools_per_block in 1usize..=10,
        top_tools in 1usize..=20,
        query_vec in prop::collection::vec(-1.0f32..=1.0f32, VECTOR_DIM),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (tools, co) = generate_random_tools(num_blocks, tools_per_block);
        let total_tools = tools.len();

        // 异步收集路由结果(不在 async 块内使用 prop_assert!)
        let result = rt.block_on(async {
            let bus = EventBus::new();
            let router = KVBlockSemanticRouter::new(bus);
            router.build_blocks(tools, co).await.unwrap();
            let clv = make_random_clv(query_vec);
            let req = RoutingRequest::new(clv).with_top_tools(top_tools);
            router.route_with_request(&req).await.unwrap()
        });

        let effective_top = top_tools.min(total_tools);
        prop_assert!(
            result.routed_count() <= effective_top,
            "routed_count {} should be <= min(top_tools={}, total_tools={})",
            result.routed_count(), top_tools, total_tools
        );
    }

    /// 不变量 2:路由结果数 ≤ 候选集大小(总工具数)
    ///
    /// 路由返回的工具数不应超过总工具数
    #[test]
    fn test_route_result_le_total_tools(
        num_blocks in 1usize..=10,
        tools_per_block in 5usize..=15,
        query_vec in prop::collection::vec(-1.0f32..=1.0f32, VECTOR_DIM),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (tools, co) = generate_random_tools(num_blocks, tools_per_block);
        let total_tools = tools.len();

        let result = rt.block_on(async {
            let bus = EventBus::new();
            let router = KVBlockSemanticRouter::new(bus);
            router.build_blocks(tools, co).await.unwrap();
            let clv = make_random_clv(query_vec);
            router.route(&clv).await.unwrap()
        });

        prop_assert!(
            result.routed_count() <= total_tools,
            "routed_count {} should be <= total_tools {}",
            result.routed_count(), total_tools
        );
    }

    /// 不变量 3:重平衡后块数量 ≤ 工具数
    ///
    /// 重平衡不会产生比工具数更多的块(每块至少含 1 个工具)
    #[test]
    fn test_rebalance_block_count_le_tools(
        num_blocks in 2usize..=10,
        tools_per_block in 2usize..=10,
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (tools, co) = generate_random_tools(num_blocks, tools_per_block);
        let total_tools = tools.len();

        let (before, after) = rt.block_on(async {
            let bus = EventBus::new();
            let router = KVBlockSemanticRouter::new(bus);
            router.build_blocks(tools, co).await.unwrap();
            let before = router.block_count().await;
            router.auto_rebalance().await.unwrap();
            let after = router.block_count().await;
            (before, after)
        });

        prop_assert!(
            before <= total_tools,
            "block_count before {} should be <= tools {}",
            before, total_tools
        );
        prop_assert!(
            after <= total_tools,
            "block_count after {} should be <= tools {}",
            after, total_tools
        );
    }

    /// 不变量 4:路由分数 ∈ [-1.0, 1.0]
    ///
    /// 所有返回的余弦相似度分数应在 [-1.0, 1.0] 范围内
    /// (cosine_similarity_slices 内部 clamp 到 [-1.0, 1.0])
    #[test]
    fn test_route_scores_in_unit_range(
        num_blocks in 1usize..=8,
        tools_per_block in 3usize..=10,
        query_vec in prop::collection::vec(-1.0f32..=1.0f32, VECTOR_DIM),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (tools, co) = generate_random_tools(num_blocks, tools_per_block);

        let scores = rt.block_on(async {
            let bus = EventBus::new();
            let router = KVBlockSemanticRouter::new(bus);
            router.build_blocks(tools, co).await.unwrap();
            let clv = make_random_clv(query_vec);
            let result = router.route(&clv).await.unwrap();
            result.scores
        });

        for (i, &score) in scores.iter().enumerate() {
            prop_assert!(
                (-1.0..=1.0).contains(&score),
                "score[{}] = {} not in [-1.0, 1.0]",
                i, score
            );
        }
    }

    /// 不变量 5:路由结果非空(有工具时)
    ///
    /// 当工具集非空时,路由应返回至少 1 个工具
    #[test]
    fn test_route_result_non_empty_when_tools_exist(
        num_blocks in 1usize..=5,
        tools_per_block in 3usize..=8,
        query_vec in prop::collection::vec(0.0f32..=1.0f32, VECTOR_DIM),
    ) {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let (tools, co) = generate_random_tools(num_blocks, tools_per_block);

        let (selected_count, scores_count) = rt.block_on(async {
            let bus = EventBus::new();
            let router = KVBlockSemanticRouter::new(bus);
            router.build_blocks(tools, co).await.unwrap();
            let clv = make_random_clv(query_vec);
            let result = router.route(&clv).await.unwrap();
            (result.selected_tools.len(), result.scores.len())
        });

        prop_assert!(
            selected_count > 0,
            "route result should not be empty when tools exist"
        );
        prop_assert_eq!(
            selected_count, scores_count,
            "selected_tools and scores length should match"
        );
    }
}
