//! WI-C1′ — 召回预算裁剪（`TokenBudget`）集成测试
//!
//! # 覆盖
//! 1. `None` = 现行为（与充足预算逐位对拍）;
//! 2. 分数序贪心装包:装入后不超预算的保留、装不下的**跳过继续**（保序）;
//! 3. L0 快速路径（MinimalRecall）同受预算约束;
//! 4. proptest 不变式:总成本 ≤ 预算、保序子序列、单项可容性。
//!
//! 成本口径 = `MemoryEntry::token_estimate`（字符数/3+1，与 RSB `KeyFact` 同式）。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code

use event_bus::EventBus;
use mlc_engine::{MemoryEntry, MemoryId, MemoryTier, MlcEngine, TokenBudget};
use nexus_contracts::MemoryStrategy;
use nexus_core::CLV;
use proptest::prelude::*;

fn clv_from(v0: f32, v1: f32) -> CLV {
    let mut vec = vec![0.0f32; CLV::DIMENSION];
    vec[0] = v0;
    vec[1] = v1;
    CLV::from_vec(vec).expect("CLV 构造")
}

fn l2_entry(id: &str, content_len: usize, v1: f32) -> MemoryEntry {
    MemoryEntry::new(id, "a".repeat(content_len), MemoryTier::L2Semantic)
        .with_clv(clv_from(1.0, v1))
}

/// `None` = 现行为:与"充足预算"逐位对拍（回滚位语义锁定）
#[tokio::test]
async fn budget_none_equals_generous_budget() {
    let engine = MlcEngine::new_in_memory(EventBus::new()).unwrap();
    for (i, len) in [5usize, 35, 8].iter().enumerate() {
        engine
            .l2()
            .insert(l2_entry(&format!("m-{i}"), *len, i as f32))
            .unwrap();
    }
    let q = clv_from(1.0, 0.0);
    let out_none = engine.recall_by_clv(&q, 10, None).await.unwrap();
    let out_huge = engine
        .recall_by_clv(&q, 10, Some(TokenBudget::new(usize::MAX)))
        .await
        .unwrap();
    assert_eq!(out_none, out_huge, "None 与充足预算必须逐位一致");
    assert_eq!(out_none.len(), 3);
}

/// 贪心装包 + 跳过继续（保序）:预算容 e1+e3 而不容 e2 → 输出 [e1, e3]
///
/// 成本:len 5→2、len 35→12、len 8→3（chars/3+1）;预算 5:2 → 跳过 12 → +3=5。
#[tokio::test]
async fn greedy_pack_skips_and_keeps_order() {
    let engine = MlcEngine::new_in_memory(EventBus::new()).unwrap();
    engine.l2().insert(l2_entry("m-1", 5, 0.0)).unwrap(); // sim=1.0
    engine.l2().insert(l2_entry("m-2", 35, 1.0)).unwrap(); // sim≈0.707
    engine.l2().insert(l2_entry("m-3", 8, 2.0)).unwrap(); // sim≈0.447
    let q = clv_from(1.0, 0.0);

    let out = engine
        .recall_by_clv(&q, 10, Some(TokenBudget::new(5)))
        .await
        .unwrap();
    let ids: Vec<&str> = out.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(ids, vec!["m-1", "m-3"], "装不下的中项跳过,保序继续");
    let total: usize = out
        .iter()
        .map(|(id, _)| engine.l2().get(id.as_str()).unwrap().token_estimate())
        .sum();
    assert_eq!(total, 5, "两个保留项成本恰为预算");
}

/// L0 快速路径（MinimalRecall）同受预算约束:None = 1 条;预算不足 = 空
#[tokio::test]
async fn l0_fast_path_respects_budget() {
    let engine = MlcEngine::new_in_memory(EventBus::new()).unwrap();
    engine
        .store(MemoryEntry::new(
            "w-1",
            "a".repeat(30),
            MemoryTier::L0Working,
        ))
        .await
        .unwrap();
    let q = clv_from(1.0, 0.0);

    let none = engine
        .recall_by_clv_with_strategy(&q, MemoryStrategy::MinimalRecall, None)
        .await
        .unwrap();
    assert_eq!(none.len(), 1, "MinimalRecall take(1)");

    let budgeted = engine
        .recall_by_clv_with_strategy(&q, MemoryStrategy::MinimalRecall, Some(TokenBudget::new(5)))
        .await
        .unwrap();
    assert!(
        budgeted.is_empty(),
        "单项成本 11 > 预算 5 → 空（L0 路径同样受约束）"
    );
}

/// 分数序贪心装包路径的 id 序 — 供 proptest 复用
fn ordered_ids(pairs: &[(MemoryId, f32)]) -> Vec<MemoryId> {
    pairs.iter().map(|(id, _)| id.clone()).collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// 不变式:总成本 ≤ 预算 + 保序子序列 + 单项可容性（任意内容长度×预算）
    #[test]
    fn prop_budget_invariants(
        lens in proptest::collection::vec(1usize..60, 1..10),
        budget in 0usize..160,
    ) {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| TestCaseError::fail(format!("runtime: {e}")))?;
        rt.block_on(async {
            let engine = MlcEngine::new_in_memory(EventBus::new())
                .map_err(|e| TestCaseError::fail(format!("engine: {e}")))?;
            for (i, len) in lens.iter().enumerate() {
                engine
                    .l2()
                    .insert(l2_entry(&format!("m-{i}"), *len, i as f32))
                    .map_err(|e| TestCaseError::fail(format!("insert: {e}")))?;
            }
            let q = clv_from(1.0, 0.0);
            let out = engine
                .recall_by_clv(&q, lens.len().max(1), Some(TokenBudget::new(budget)))
                .await
                .map_err(|e| TestCaseError::fail(format!("recall: {e}")))?;
            let full = engine
                .recall_by_clv(&q, lens.len().max(1), None)
                .await
                .map_err(|e| TestCaseError::fail(format!("recall: {e}")))?;

            // (a) 保序子序列
            let out_ids = ordered_ids(&out);
            let full_ids = ordered_ids(&full);
            let mut cursor = 0usize;
            for id in &out_ids {
                let mut found = false;
                while cursor < full_ids.len() {
                    cursor += 1;
                    if &full_ids[cursor - 1] == id {
                        found = true;
                        break;
                    }
                }
                prop_assert!(found, "裁剪结果必须为原序的保序子序列: {out_ids:?} vs {full_ids:?}");
            }

            // (b) 总成本 ≤ 预算 + 单项可容性
            let mut total = 0usize;
            for (id, _) in &out {
                let e = engine
                    .l2()
                    .get(id.as_str())
                    .map_err(|e| TestCaseError::fail(format!("get: {e}")))?;
                let cost = e.token_estimate();
                prop_assert!(cost <= budget, "单项可容性: {cost} > {budget}");
                total += cost;
            }
            prop_assert!(total <= budget, "总成本 {total} > 预算 {budget}");
            Ok::<(), TestCaseError>(())
        })?;
    }
}
