//! 搜索树管理器集成测试 — L0 契约消费 + 树操作闭环（v3.4.0 §14.1）
//!
//! 覆盖: 顶层 API 可达性（re-export 验证）/ L0 ExperienceCard 契约消费闭环 /
//! 树操作端到端（root→expand→best_path→prune）/ proptest 深度门控不变量

#![forbid(unsafe_code)]

use chrono::Utc;
use nexus_contracts::experience_card::{
    AtomicOperator, CardMetadata, ExecutionStatus, ThreeFactorScore,
};
use nexus_contracts::ExperienceCard;
use proptest::prelude::*;
use quest_engine::{SearchTreeManager, TreeError, TreeStats};

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

// ----------------------------------------------------------
// 顶层 API 可达性（re-export 验证）
// ----------------------------------------------------------

#[test]
fn top_level_api_accessible() {
    let tree = SearchTreeManager::new(10);
    assert_eq!(tree.get_stats().total_nodes, 0);
}

// ----------------------------------------------------------
// L0 契约消费闭环
// ----------------------------------------------------------

#[test]
fn l0_contract_consumption_closed_loop() {
    let mut tree = SearchTreeManager::new(10);
    let root_id = tree.create_root("task-1");
    // 根节点消费 L0 契约（default_root 三因子 + Box<str> 字段）
    let root = tree.get_node(&root_id).expect("根存在");
    assert_eq!(root.three_factor, ThreeFactorScore::default_root());
    assert_eq!(root.operator, AtomicOperator::Draft);
    assert!(root.parent_id.is_none());
    // 扩展节点消费 parent_id 回溯
    let n1 = tree
        .expand_node(&root_id, card("n1", Some(root_id.as_str()), 0.6))
        .expect("扩展");
    let n2 = tree
        .expand_node(&n1, card("n2", Some(n1.as_str()), 0.9))
        .expect("扩展");
    // best_path 链完整性（parent_id 回溯）
    let path = tree.get_best_path();
    assert_eq!(path.len(), 3);
    assert_eq!(path[0].node_id.as_ref(), "root_task-1");
    assert_eq!(path[2].node_id.as_ref(), n2.as_str());
    // 每节点 parent_id 指向路径前驱
    assert_eq!(path[1].parent_id.as_deref(), Some(root_id.as_str()));
}

// ----------------------------------------------------------
// 树操作端到端（root → expand → prune → stats）
// ----------------------------------------------------------

#[test]
fn tree_operations_end_to_end() {
    let mut tree = SearchTreeManager::new(3);
    let root_id = tree.create_root("task-1");
    // 展开 2 层
    let n1 = tree
        .expand_node(&root_id, card("n1", Some(root_id.as_str()), 0.4))
        .expect("扩展");
    let n2 = tree
        .expand_node(&n1, card("n2", Some(n1.as_str()), 0.9))
        .expect("扩展");
    // 低分叶扩展（待剪）
    tree.expand_node(&n2, card("n-low", Some(n2.as_str()), 0.1))
        .expect("扩展");
    let stats_before: TreeStats = tree.get_stats();
    assert_eq!(stats_before.total_nodes, 4);
    assert_eq!(stats_before.max_depth, 3);
    // 剪枝低分叶
    tree.prune(0.5);
    let stats_after = tree.get_stats();
    assert_eq!(stats_after.total_nodes, 3, "n-low 被剪");
    // best（n2 0.9）保留
    assert_eq!(stats_after.best_score, 0.9);
    assert!(tree.get_node("n-low").is_none());
}

// ----------------------------------------------------------
// proptest：深度门控不变量
// ----------------------------------------------------------

proptest! {
    /// 任意扩展序列: 节点深度恒 ≤ max_depth；stats 一致性（total=节点数）
    #[test]
    fn depth_gating_invariant(
        max_depth in 1u32..8,
        n_expansions in 1usize..20,
    ) {
        let mut tree = SearchTreeManager::new(max_depth);
        let root_id = tree.create_root("task-1");
        let mut parent = root_id.clone();
        let mut ok = 0;
        for i in 0..n_expansions {
            let child = format!("n{i}");
            match tree.expand_node(&parent, card(&child, Some(parent.as_str()), 0.5)) {
                Ok(id) => {
                    ok += 1;
                    parent = id;
                }
                Err(TreeError::MaxDepthReached) => break,
                Err(_) => panic!("仅允许 MaxDepthReached"),
            }
        }
        // 成功扩展数 ≤ max_depth（每层至多 1 条链）
        prop_assert!(ok <= max_depth);
        // 深度恒 ≤ max_depth
        let stats = tree.get_stats();
        prop_assert!(stats.max_depth <= max_depth);
        prop_assert_eq!(stats.total_nodes, ok as usize + 1, "节点数 = 扩展数 + 根");
    }
}
