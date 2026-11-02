//! 四路召回混合融合（`MlcEngine::recall_hybrid`）集成测试 — v2.3 §(d)⑨ 二期启动
//!
//! 验证引擎级编排：语义/情节/认识三路 + 可选第四路的 RRF 融合（k=60、
//! rank 1-based、f32 手工推导对拍），以及非破坏退化边界。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code

use event_bus::EventBus;
use mlc_engine::memory_graph::{MemoryGraph, MemoryNode, MemoryNodeType};
use mlc_engine::recall_fusion::FusionTuning;
use mlc_engine::{MemoryEntry, MemoryId, MemoryTier, MlcEngine};
use nexus_contracts::recall_fusion::DEFAULT_RRF_K;
use nexus_contracts::EpistemicType;
use nexus_core::CLV;

fn clv_dim0(v: f32) -> CLV {
    let mut vec = vec![0.0f32; CLV::DIMENSION];
    vec[0] = v;
    CLV::from_vec(vec).expect("CLV 构造")
}

fn l2_entry(id: &str, epistemic: EpistemicType) -> MemoryEntry {
    MemoryEntry::new(id, format!("content-{id}"), MemoryTier::L2Semantic)
        .with_clv(clv_dim0(1.0))
        .with_epistemic(epistemic)
}

fn mid(s: &str) -> MemoryId {
    MemoryId::from(s.to_string())
}

fn default_tuning() -> FusionTuning {
    FusionTuning {
        rrf_k: DEFAULT_RRF_K,
        top_k: 10,
        graph_weight: 1.0,
    }
}

fn clv_dim1(v: f32) -> CLV {
    let mut vec = vec![0.0f32; CLV::DIMENSION];
    vec[1] = v;
    CLV::from_vec(vec).expect("CLV 构造")
}

fn graph_node(id: &str, clv: CLV) -> MemoryNode {
    MemoryNode {
        node_id: id.to_string(),
        content: format!("content-{id}"),
        embedding: clv,
        node_type: MemoryNodeType::Solution,
        success_associated: true,
    }
}

#[tokio::test]
async fn no_extra_inputs_degrades_to_semantic_order() {
    // quest_id=None 且无第四路：全 Fact 时认识路为空（仅 Observation 计份）、
    // 情节路为空 → 单路命中，融合序 = 语义序（非破坏退化边界）
    let engine = MlcEngine::new_in_memory(EventBus::new()).unwrap();
    engine
        .l2()
        .insert(l2_entry("m-1", EpistemicType::Fact))
        .unwrap();
    engine
        .l2()
        .insert(l2_entry("m-2", EpistemicType::Fact))
        .unwrap();

    let out = engine
        .recall_hybrid(&clv_dim0(1.0), None, None, 2, default_tuning())
        .await
        .unwrap();
    assert_eq!(out.len(), 2);
    assert_eq!(out[0].id, "m-1");
    assert_eq!(out[1].id, "m-2");
    assert_eq!(out[0].list_hits, 1);
}

#[tokio::test]
async fn extra_fourth_path_promotes_cross_path_agreement() {
    // 第四路(图扩模拟)把 m-2 顶前：m-2 = semantic rank2 + extra rank1
    // 对拍(f32, k=60)：1/62 + 1/61 ≈ 0.032522 > m-1 单路 1/61 ≈ 0.016393
    let engine = MlcEngine::new_in_memory(EventBus::new()).unwrap();
    engine
        .l2()
        .insert(l2_entry("m-1", EpistemicType::Fact))
        .unwrap();
    engine
        .l2()
        .insert(l2_entry("m-2", EpistemicType::Fact))
        .unwrap();

    let extra = vec![mid("m-2"), mid("m-9")]; // m-9 仅第四路命中
    let out = engine
        .recall_hybrid(
            &clv_dim0(1.0),
            None,
            Some(extra.as_slice()),
            2,
            default_tuning(),
        )
        .await
        .unwrap();
    assert_eq!(out[0].id, "m-2", "多路共同命中应居首");
    assert_eq!(out[0].list_hits, 2);
    assert_eq!(out[0].best_rank, 1);
}

#[tokio::test]
async fn epistemic_path_promotes_observation_with_exact_rrf_math() {
    // 认识路仅对 Observation 计份（§(b.3) 主线③分离语义；Fact 已由语义路代表）：
    // m-2(Observation) = semantic rank2 + epistemic rank1 → 反超单路 Fact m-1
    let engine = MlcEngine::new_in_memory(EventBus::new()).unwrap();
    engine
        .l2()
        .insert(l2_entry("m-1", EpistemicType::Fact))
        .unwrap();
    engine
        .l2()
        .insert(l2_entry("m-2", EpistemicType::Observation))
        .unwrap();

    let out = engine
        .recall_hybrid(&clv_dim0(1.0), None, None, 2, default_tuning())
        .await
        .unwrap();
    assert_eq!(out[0].id, "m-2", "派生结论经认识路计份后应反超单路 Fact");
    assert_eq!(out[0].list_hits, 2, "semantic + epistemic 双路");

    // 叠加 episodic 路（quest 关联 m-2）→ 三路命中分数更高，居先稳定
    let mut e = MemoryEntry::new("m-2", "obs", MemoryTier::L1Episodic)
        .with_epistemic(EpistemicType::Observation);
    e.quest_id = Some(mlc_engine::QuestId::new("q-1"));
    engine.l1().insert(e).unwrap();
    let out2 = engine
        .recall_hybrid(&clv_dim0(1.0), Some("q-1"), None, 2, default_tuning())
        .await
        .unwrap();
    let m2 = out2.iter().find(|f| f.id == "m-2").unwrap();
    assert_eq!(m2.list_hits, 3, "语义+情节+认识三路均命中 m-2");
    assert_eq!(out2[0].id, "m-2");
}

/// WI-G9:真实图扩链路 — `MemoryGraph::recall_with_graph_ids` → `recall_hybrid` 第四路
///
/// 覆盖"graph 路成为 recall_hybrid 生产可达"：种子向量检索（m-2 与 query 同向）
/// + 共现边扩展（m-9 语义不相似但共现直连）→ 融合就绪 id 序 → RRF 融合。
#[tokio::test]
async fn graph_path_via_recall_with_graph_ids_promotes() {
    let engine = MlcEngine::new_in_memory(EventBus::new()).unwrap();
    engine
        .l2()
        .insert(l2_entry("m-1", EpistemicType::Fact))
        .unwrap();
    engine
        .l2()
        .insert(l2_entry("m-2", EpistemicType::Fact))
        .unwrap();

    // 图侧：m-2 种子（dim0 同向 sim=1.0）；m-9 正交（dim1）不可成种子,仅共现边可达
    let mut graph = MemoryGraph::new();
    graph.insert_node(graph_node("m-2", clv_dim0(1.0)));
    graph.insert_node(graph_node("m-9", clv_dim1(1.0)));
    graph.build_cooccurrence_edges(&[vec!["m-2".to_string(), "m-9".to_string()]]);

    let graph_ids = graph.recall_with_graph_ids(&clv_dim0(1.0), 1, 8);
    assert_eq!(
        graph_ids,
        vec![mid("m-2"), mid("m-9")],
        "seed 首位 + BFS 序（位置即 rank）"
    );

    let out = engine
        .recall_hybrid(
            &clv_dim0(1.0),
            None,
            Some(graph_ids.as_slice()),
            3,
            default_tuning(),
        )
        .await
        .unwrap();
    assert_eq!(out[0].id, "m-2", "semantic+graph 双路共同命中应居首");
    assert_eq!(out[0].list_hits, 2);
    assert!(
        out.iter().any(|f| f.id == "m-9"),
        "图独有命中（共现扩展）应入融合结果"
    );
}

/// WI-G9:`graph_weight` 语义 — 高权重顶前 / `<=0` 等效断路（回滚锚逐位对拍）
#[tokio::test]
async fn graph_weight_knobs_and_zero_equals_disconnect() {
    let engine = MlcEngine::new_in_memory(EventBus::new()).unwrap();
    engine
        .l2()
        .insert(l2_entry("m-1", EpistemicType::Fact))
        .unwrap();
    let extra = vec![mid("m-9")]; // 图独有（不在 L2）

    // 高权重：3/61 > m-1 单路 1/61 → 图 rank1 顶前（hcw rerank 接入点实证）
    let tuned = FusionTuning {
        graph_weight: 3.0,
        ..default_tuning()
    };
    let out = engine
        .recall_hybrid(&clv_dim0(1.0), None, Some(extra.as_slice()), 2, tuned)
        .await
        .unwrap();
    assert_eq!(out[0].id, "m-9", "高权重图路 rank1 应胜单路 m-1");

    // 等效断路：graph_weight=0.0 与不传第四路逐位一致（回滚锚）
    let zero = FusionTuning {
        graph_weight: 0.0,
        ..default_tuning()
    };
    let with_zero = engine
        .recall_hybrid(&clv_dim0(1.0), None, Some(extra.as_slice()), 2, zero)
        .await
        .unwrap();
    let disconnected = engine
        .recall_hybrid(&clv_dim0(1.0), None, None, 2, default_tuning())
        .await
        .unwrap();
    assert_eq!(
        with_zero, disconnected,
        "权重≤0 必须与不传第四路逐位一致（回滚锚）"
    );
}
