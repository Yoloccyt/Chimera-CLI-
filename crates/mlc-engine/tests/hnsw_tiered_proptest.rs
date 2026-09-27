//! L2 分层检索属性测试 — ADR-192 的不变量面(结果集契约 / 图-主表一致性 / top_k 钳制)
//!
//! WHY 属性测试而非例测:分层路由把「同一语义契约」交到两条实现路径上,
//! 例测只能钉住抽样点;三条不变量在随机语料/随机顺序/随机 top_k 下必须恒成立。
//! 阈值取小值(8)以在 debug 下把建图成本压到可承受,不变量与规模无关。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use mlc_engine::{MemoryEntry, MemoryTier, SemanticMemory};
use nexus_core::CLV;
use proptest::prelude::*;

/// 任意 f32 向量 → 合法 CLV(全零向量会被 `from_vec` 拒绝,故加单位偏置)
fn clv_from(values: Vec<f32>) -> CLV {
    let v: Vec<f32> = values.iter().map(|x| x.clamp(-1.0, 1.0)).collect();
    CLV::from_vec(v).expect("CLV 构造应成功")
}

fn arb_clv() -> impl Strategy<Value = CLV> {
    prop::collection::vec(-1.0f32..=1.0f32, CLV::DIMENSION).prop_map(clv_from)
}

fn entry(id: usize, clv: CLV) -> MemoryEntry {
    MemoryEntry::new(
        format!("m-{id}"),
        format!("content-{id}"),
        MemoryTier::L2Semantic,
    )
    .with_clv(clv)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// 不变量 1:近似层/精确层共用同一结果集契约
    /// (长度 = min(top_k, 条目数)、分数降序且 ∈ [0,1]、id 唯一且存活)
    #[test]
    fn test_tiered_result_set_contract(
        entry_clvs in prop::collection::vec(arb_clv(), 1..40),
        query in arb_clv(),
        top_k in 1usize..60,
    ) {
        let mem = SemanticMemory::with_hnsw_threshold(64, 8);
        for (i, clv) in entry_clvs.iter().enumerate() {
            mem.insert(entry(i, clv.clone()))?;
        }
        let n = mem.len()?;
        let results = mem.recall_by_clv(&query, top_k)?;

        prop_assert_eq!(results.len(), top_k.min(n), "长度契约");
        prop_assert!(results.len() <= 60);
        for w in results.windows(2) {
            prop_assert!(w[0].1 >= w[1].1, "降序契约:{:?} < {:?}", w[0].1, w[1].1);
        }
        for (id, score) in &results {
            prop_assert!((0.0..=1.0).contains(score), "分数 clamp:{score}");
            prop_assert!(mem.get(id.as_str()).is_ok(), "结果含非存活 id:{}", id.as_str());
        }
        let unique: std::collections::HashSet<&str> =
            results.iter().map(|(id, _)| id.as_str()).collect();
        prop_assert_eq!(unique.len(), results.len(), "结果 id 不得重复");
    }

    /// 不变量 2:插入序列置换(含随机删除)下图侧存活点恒等于主表条目数,
    /// 且置换不改变条目数与删除后的驱逐/墓碑一致性
    #[test]
    fn test_graph_consistency_under_permutation(
        entry_clvs in prop::collection::vec(arb_clv(), 8..40),
        drop_count in 0usize..8,
    ) {
        let n = entry_clvs.len();
        // 两个真不同置换:正序 vs 倒序再轮转(轮转量由 drop_count 驱动)
        let mut order: Vec<usize> = (0..n).rev().collect();
        order.rotate_left(drop_count % n);

        let a = SemanticMemory::with_hnsw_threshold(64, 8);
        let b = SemanticMemory::with_hnsw_threshold(64, 8);
        for (i, clv) in entry_clvs.iter().enumerate() {
            a.insert(entry(i, clv.clone()))?;
        }
        for &i in &order {
            b.insert(entry(i, entry_clvs[i].clone()))?;
        }
        // 触发两侧建图(建图是一次全量插入的派生,与顺序无关)
        let q = clv_from(vec![0.5_f32; CLV::DIMENSION]);
        a.recall_by_clv(&q, 5)?;
        b.recall_by_clv(&q, 5)?;

        prop_assert_eq!(a.len()?, n);
        prop_assert_eq!(b.len()?, n);
        prop_assert_eq!(a.hnsw_tier_snapshot()?.live_points, a.len()?);
        prop_assert_eq!(b.hnsw_tier_snapshot()?.live_points, b.len()?);

        // 删除后仍一致(逆序/轮转插入不影响图侧存活集)
        let expected_tombstones = drop_count.min(n);
        for i in 0..expected_tombstones {
            a.remove(&format!("m-{i}"))?;
        }
        let snap = a.hnsw_tier_snapshot()?;
        prop_assert_eq!(snap.live_points, a.len()?, "删除后图-主表一致性");
        prop_assert_eq!(
            snap.tombstones, expected_tombstones,
            "每次删除应留下恰好一个墓碑"
        );
    }

    /// 不变量 4（ADR-192 Phase 2 第三条字面项）：被删除（墓碑化）的条目
    /// **绝不出现在结果里**，且墓碑化后图侧存活点仍 == 主表条目数。
    ///
    /// WHY 先强制建图再删：hnsw_rs 不可删点，只有图已在场时 remove 才产生真实
    /// 墓碑；若未建图则只是主表操作，这条性质退化且不测到图侧过滤逻辑。
    #[test]
    fn test_tombstoned_entries_never_returned(
        entry_clvs in prop::collection::vec(arb_clv(), 9..40),
        kill_idx in prop::collection::vec(0usize..44, 1..8),
        top_k in 1usize..20,
    ) {
        let mem = SemanticMemory::with_hnsw_threshold(64, 8);
        for (i, clv) in entry_clvs.iter().enumerate() {
            mem.insert(entry(i, clv.clone()))?;
        }
        let query = clv_from(vec![0.4_f32; CLV::DIMENSION]);
        // 先建图，后续删除均成为真墓碑
        mem.recall_by_clv(&query, 1)?;
        prop_assert!(mem.hnsw_tier_snapshot()?.built, "达阈首次召回应已建图");

        let mut killed: std::collections::HashSet<String> = std::collections::HashSet::new();
        for k in &kill_idx {
            let id = format!("m-{k}");
            if mem.remove(&id)?.is_some() {
                killed.insert(id);
            }
        }
        let snap = mem.hnsw_tier_snapshot()?;
        prop_assert_eq!(
            snap.live_points,
            mem.len()?,
            "墓碑化后图-主表必须一致（存活集不得残留已删条目）"
        );

        let results = mem.recall_by_clv(&query, top_k)?;
        for (id, _) in &results {
            prop_assert!(
                !killed.contains(id.as_str()),
                "被墓碑化的条目重新出现在结果中: {}", id.as_str()
            );
            prop_assert!(
                mem.get(id.as_str()).is_ok(),
                "结果含主表外 id: {}", id.as_str()
            );
        }
    }

    /// 不变量 3：top_k 超界钳制 —— 结果长度只随条目数收敛，不因分层路径而变
    #[test]
    fn test_top_k_over_bound_clamped(
        entry_clvs in prop::collection::vec(arb_clv(), 1..30),
        top_k in 100usize..400,
    ) {
        let tiered = SemanticMemory::with_hnsw_threshold(64, 8);
        let off = SemanticMemory::with_hnsw_threshold(64, 0);
        for (i, clv) in entry_clvs.iter().enumerate() {
            tiered.insert(entry(i, clv.clone()))?;
            off.insert(entry(i, clv.clone()))?;
        }
        let query = clv_from(vec![0.25_f32; CLV::DIMENSION]);
        let n = tiered.len()?;
        let got = tiered.recall_by_clv(&query, top_k)?;
        let base = off.recall_by_clv(&query, top_k)?;
        prop_assert_eq!(got.len(), n, "top_k 超界应钳到条目数(近似层)");
        prop_assert_eq!(base.len(), n, "top_k 超界应钳到条目数(精确层)");
    }
}
