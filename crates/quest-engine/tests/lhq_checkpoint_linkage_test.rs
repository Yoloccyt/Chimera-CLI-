//! LHQP 检查点联动集成测试 — 序列化往返（v3.4.0 §14 二次审查增强 Wave 2）
//!
//! 覆盖: SearchTreeManager/LongTaskMap to_bytes/from_bytes 往返 /
//! 检查点保存/恢复场景 / 空树/空地图边界 / proptest 序列化往返不变量

#![forbid(unsafe_code)]

use chrono::Utc;
use nexus_contracts::experience_card::{
    AtomicOperator, CardMetadata, ExecutionStatus, ThreeFactorScore,
};
use nexus_contracts::{domain::Task, task::TaskStatus, ExperienceCard, Quest, ThinkingMode};
use proptest::prelude::*;
use quest_engine::{LongTaskMap, SearchTreeManager, StepResult, TaskMapRef};

fn card(node_id: &str, parent_id: Option<&str>, score: f32) -> ExperienceCard {
    ExperienceCard {
        card_id: Box::from(format!("card_{node_id}")),
        task_id: Box::from("task-1"),
        node_id: Box::from(node_id),
        parent_id: parent_id.map(Box::from),
        created_at: Utc::now(),
        operator: AtomicOperator::Improve,
        score,
        delta_vs_parent: 0.1,
        method_family: Box::from("test"),
        error_signature: None,
        three_factor: ThreeFactorScore {
            quality: score,
            progress: 0.1,
            novelty: 0.5,
        },
        execution_status: ExecutionStatus::Success,
        token_evidence_ids: Vec::new(),
        segment_id: None,
        metadata: CardMetadata::default(),
    }
}

fn quest() -> Quest {
    Quest {
        quest_id: "q-1".to_string(),
        title: "重构任务".to_string(),
        tasks: vec![Task {
            task_id: "t-1".to_string(),
            description: "分析".to_string(),
            status: TaskStatus::Pending,
            dependencies: Vec::new(),
        }],
        thinking_mode: ThinkingMode::Standard,
        checkpoint_id: None,
        priority: 128,
    }
}

fn step(state: &str) -> StepResult {
    StepResult {
        state: state.to_string(),
        detail: format!("{state} 详情"),
        next_action: "continue".to_string(),
        action: "analyze".to_string(),
        success: true,
    }
}

// ----------------------------------------------------------
// SearchTreeManager 序列化往返
// ----------------------------------------------------------

#[test]
fn search_tree_roundtrip_preserves_structure() {
    let mut tree = SearchTreeManager::new(10);
    let root_id = tree.create_root("task-1");
    let n1 = tree
        .expand_node(&root_id, card("n1", Some(root_id.as_str()), 0.7))
        .expect("扩展");
    tree.expand_node(&n1, card("n2", Some(n1.as_str()), 0.9))
        .expect("扩展");

    // 序列化 → 反序列化
    let bytes = tree.to_bytes().expect("序列化成功");
    let restored = SearchTreeManager::from_bytes(&bytes).expect("反序列化成功");

    // 节点/深度/best 保留
    assert_eq!(restored.get_stats().total_nodes, 3);
    assert_eq!(restored.get_stats().best_score, 0.9);
    assert_eq!(restored.node_depth("n2"), Some(2));
    // best_path 链完整
    let path = restored.get_best_path();
    assert_eq!(path.len(), 3);
    assert_eq!(path[2].node_id.as_ref(), "n2");
}

#[test]
fn search_tree_empty_roundtrip() {
    let tree = SearchTreeManager::new(5);
    let bytes = tree.to_bytes().expect("序列化空树");
    let restored = SearchTreeManager::from_bytes(&bytes).expect("反序列化空树");
    assert_eq!(restored.get_stats().total_nodes, 0);
}

#[test]
fn search_tree_from_bytes_corrupt_fails() {
    // 损坏的 bytes → SerializationError
    let corrupt = vec![0xFF, 0xFF, 0xFF, 0xFF];
    let result = SearchTreeManager::from_bytes(&corrupt);
    assert!(result.is_err(), "损坏数据应返回错误");
}

// ----------------------------------------------------------
// LongTaskMap 序列化往返
// ----------------------------------------------------------

#[test]
fn long_task_map_roundtrip_preserves_nodes_edges() {
    let mut map = LongTaskMap::default();
    let map_ref: TaskMapRef = map.create_map(&quest());
    map.record_step(&map_ref, &step("步骤一"));
    map.record_step(&map_ref, &step("步骤二"));

    // 序列化 → 反序列化
    let bytes = map.to_bytes().expect("序列化成功");
    let restored = LongTaskMap::from_bytes(&bytes).expect("反序列化成功");

    // 节点/边保留（root + 2 steps = 3 nodes, 2 edges）
    assert_eq!(restored.node_count(), 3);
    assert_eq!(restored.edge_count(), 2);
    assert_eq!(restored.get_node("root").unwrap().state_summary, "重构任务");
    assert_eq!(restored.get_node("node_1").unwrap().state_summary, "步骤一");
}

#[test]
fn long_task_map_empty_roundtrip() {
    let map = LongTaskMap::default();
    let bytes = map.to_bytes().expect("序列化空地图");
    let restored = LongTaskMap::from_bytes(&bytes).expect("反序列化空地图");
    assert_eq!(restored.node_count(), 0);
    assert_eq!(restored.edge_count(), 0);
}

#[test]
fn long_task_map_from_bytes_corrupt_fails() {
    let corrupt = vec![0xAB, 0xCD];
    let result = LongTaskMap::from_bytes(&corrupt);
    assert!(result.is_err(), "损坏数据应返回错误");
}

// ----------------------------------------------------------
// 检查点保存/恢复场景（模拟调用方关联存储）
// ----------------------------------------------------------

#[test]
fn checkpoint_linkage_scenario() {
    // 模拟：检查点保存时序列化搜索树/任务地图，恢复时重建
    let mut tree = SearchTreeManager::new(10);
    let root_id = tree.create_root("quest-x");
    tree.expand_node(&root_id, card("n1", Some(root_id.as_str()), 0.8))
        .expect("扩展");
    let tree_bytes = tree.to_bytes().expect("保存搜索树");

    let mut map = LongTaskMap::default();
    let map_ref = map.create_map(&quest());
    map.record_step(&map_ref, &step("进行中"));
    let map_bytes = map.to_bytes().expect("保存任务地图");

    // 模拟恢复：从 bytes 重建（关联 Checkpoint 由调用方负责）
    let restored_tree = SearchTreeManager::from_bytes(&tree_bytes).expect("恢复搜索树");
    let restored_map = LongTaskMap::from_bytes(&map_bytes).expect("恢复任务地图");
    assert_eq!(restored_tree.get_stats().total_nodes, 2);
    assert_eq!(restored_map.node_count(), 2);
}

// ----------------------------------------------------------
// proptest: 序列化往返不变量
// ----------------------------------------------------------

proptest! {
    /// 任意节点数的搜索树：序列化往返后节点数/best_score 不变
    #[test]
    fn search_tree_roundtrip_invariant(
        n_nodes in 1usize..8,
        score in 0.0f32..1.0,
    ) {
        let mut tree = SearchTreeManager::new(20);
        let root_id = tree.create_root("task-1");
        let mut parent = root_id.clone();
        for i in 0..n_nodes {
            let child = format!("n{i}");
            match tree.expand_node(&parent, card(&child, Some(parent.as_str()), score)) {
                Ok(id) => parent = id,
                Err(_) => break,
            }
        }
        let bytes = tree.to_bytes().expect("序列化");
        let restored = SearchTreeManager::from_bytes(&bytes).expect("反序列化");
        prop_assert_eq!(restored.get_stats().total_nodes, tree.get_stats().total_nodes);
        prop_assert_eq!(restored.get_stats().best_score, tree.get_stats().best_score);
    }
}
