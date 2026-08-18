//! 搜索树管理器 — OpenMLE 经验卡片进化树（设计文档 §14.1）
//!
//! 对应架构层: **L9 Quest**（quest-engine 子模块）
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_统一架构设计与Rust侧实现规范_二十三篇论文融合权威版.md` §14.1
//! 对应论文: 清华 OpenMLE（搜索树扩展/选择/剪枝/最优路径）
//! 对应 ADR: ADR-049 决策 1（规范字面独立 crate search-tree-manager，内嵌落点）
//!
//! # 核心职责
//!
//! OpenMLE 经验卡片进化树管理（节点 = L0 [`ExperienceCard`]）：
//! - `create_root`: 任务根节点（Draft 算子 + default_root 三因子）
//! - `expand_node`: 扩展子节点（max_depth 门控 + best_node 追踪）
//! - `get_best_path`: 经 parent_id 回溯最优路径（根→最佳叶）
//! - `prune`: 低分叶节点剪枝（保留有子节点与根节点）
//! - `get_stats`: 树统计（节点数/深度/最佳分/叶节点数）
//!
//! # 语义边界（与 dag.rs）
//!
//! - 本模块：经验卡片进化树（OpenMLE 搜索语义，节点=ExperienceCard）
//! - `dag.rs`：任务 DAG（Task 依赖图，节点=任务）——两者零交互并存
//!
//! # 落层偏差记录
//!
//! 1. 原型 `current_depth += 1` 全局递增语义偏差 → 实现为节点深度
//!    parent_depth + 1（工程准确性，max_depth 门控语义正确）
//! 2. 原型 ExperienceCard 字段 String → 实际 L0 全部 `Box<str>`（Box::from 适配）
//!
//! # 设计约束（铁律）
//!
//! - **铁律3**: 卡片只读消费（树不修改卡片内容，版本化由 L0 保证）
//! - **铁律4**: prune/get_stats 纯函数语义（确定性）
//!
//! # 长时程信用分配协同边界（D-6）
//!
//! 规范 §14 无长时程信用分配条款；该语义由既有组件覆盖：
//! L8 `parliament::sharp`（SHARP Shapley 精确归因）+ L1 `SegmentAwarePER`
//! （分段奖励传播）+ 本 crate `trajectory_exporter`（铁律6 轨迹导出）。

use std::collections::HashMap;

use chrono::Utc;
use nexus_contracts::experience_card::{
    AtomicOperator, CardMetadata, ExecutionStatus, ThreeFactorScore,
};
use nexus_contracts::ExperienceCard;
use thiserror::Error;

/// 搜索树错误（库层 thiserror，§4.1）
#[derive(Debug, Error, PartialEq)]
pub enum TreeError {
    /// 已达最大深度
    #[error("Max depth reached")]
    MaxDepthReached,
    /// 父节点不存在
    #[error("Parent not found")]
    ParentNotFound,
}

/// 搜索树统计（规范 §14.1 TreeStats）
#[derive(Clone, Debug, PartialEq)]
pub struct TreeStats {
    /// 节点总数
    pub total_nodes: usize,
    /// 当前最大节点深度
    pub max_depth: u32,
    /// 最佳节点评分
    pub best_score: f32,
    /// 叶节点数
    pub leaf_nodes: usize,
}

/// 搜索树管理器 — OpenMLE 经验卡片进化树（规范 §14.1）
#[derive(Debug, Default)]
pub struct SearchTreeManager {
    /// 节点表: node_id → 经验卡片（铁律3 只读承载）
    nodes: HashMap<String, ExperienceCard>,
    /// 子节点表: node_id → 子 node_id 列表
    children: HashMap<String, Vec<String>>,
    /// 节点深度表: node_id → 深度（根=0，D-3 偏差适配）
    node_depth: HashMap<String, u32>,
    /// 最大深度门控
    max_depth: u32,
    /// 当前最佳节点 ID
    best_node_id: Option<String>,
}

impl SearchTreeManager {
    /// 创建搜索树管理器（max_depth 深度门控）
    pub fn new(max_depth: u32) -> Self {
        Self {
            max_depth,
            ..Default::default()
        }
    }

    /// 创建任务根节点（规范 §14.1 create_root）
    ///
    /// 根卡片：Draft 算子 + `default_root()` 三因子（无父本参照）+
    /// method_family="root"；根节点即初始最佳节点。
    pub fn create_root(&mut self, task_id: &str) -> String {
        let root_id = format!("root_{task_id}");
        let root = ExperienceCard {
            card_id: Box::from(format!("card_root_{task_id}")),
            task_id: Box::from(task_id),
            node_id: Box::from(root_id.as_str()),
            parent_id: None,
            created_at: Utc::now(),
            operator: AtomicOperator::Draft,
            score: 0.0,
            delta_vs_parent: 0.0,
            method_family: Box::from("root"),
            error_signature: None,
            three_factor: ThreeFactorScore::default_root(),
            execution_status: ExecutionStatus::Success,
            token_evidence_ids: Vec::new(),
            segment_id: None,
            metadata: CardMetadata::default(),
        };
        self.nodes.insert(root_id.clone(), root);
        self.children.insert(root_id.clone(), Vec::new());
        self.node_depth.insert(root_id.clone(), 0);
        self.best_node_id = Some(root_id.clone());
        root_id
    }

    /// 扩展子节点（规范 §14.1 expand_node）
    ///
    /// 门控：max_depth（D-3: 子节点深度 = parent_depth + 1）+
    /// 父节点存在性；扩展后追踪 best_node。
    pub fn expand_node(
        &mut self,
        parent_id: &str,
        card: ExperienceCard,
    ) -> Result<String, TreeError> {
        let parent_depth = self
            .node_depth
            .get(parent_id)
            .ok_or(TreeError::ParentNotFound)?;
        let child_depth = parent_depth + 1;
        if child_depth > self.max_depth {
            return Err(TreeError::MaxDepthReached);
        }
        let child_id = card.node_id.to_string();
        self.children
            .entry(parent_id.to_string())
            .or_default()
            .push(child_id.clone());
        self.nodes.insert(child_id.clone(), card);
        self.children.insert(child_id.clone(), Vec::new());
        self.node_depth.insert(child_id.clone(), child_depth);
        self.update_best_node(&child_id);
        Ok(child_id)
    }

    /// 最优路径 — 根 → 最佳节点（规范 §14.1 get_best_path）
    pub fn get_best_path(&self) -> Vec<&ExperienceCard> {
        match &self.best_node_id {
            Some(id) => self.trace_path(id),
            None => Vec::new(),
        }
    }

    /// 低分叶节点剪枝（规范 §14.1 prune）
    ///
    /// 剪枝条件：score < threshold 且为叶节点（无子节点）；
    /// 根节点与有子节点不剪（保护树结构完整性）。
    pub fn prune(&mut self, threshold: f32) {
        let root_id = self.root_id();
        let to_remove: Vec<String> = self
            .nodes
            .values()
            .filter(|n| {
                let is_leaf = self
                    .children
                    .get(n.node_id.as_ref())
                    .map(|c| c.is_empty())
                    .unwrap_or(true);
                let is_root = root_id.as_deref() == Some(n.node_id.as_ref());
                n.score < threshold && is_leaf && !is_root
            })
            .map(|n| n.node_id.to_string())
            .collect();
        for node_id in to_remove {
            self.nodes.remove(&node_id);
            self.children.remove(&node_id);
            self.node_depth.remove(&node_id);
            // 从父节点子列表中移除
            for children in self.children.values_mut() {
                children.retain(|c| c != &node_id);
            }
            // 被剪节点若为 best（低分 best 仅可能出现在根，根不剪，此处防御）
            if self.best_node_id.as_deref() == Some(node_id.as_str()) {
                self.best_node_id = self.root_id();
            }
        }
    }

    /// 树统计（规范 §14.1 get_stats）
    pub fn get_stats(&self) -> TreeStats {
        TreeStats {
            total_nodes: self.nodes.len(),
            max_depth: self.node_depth.values().copied().max().unwrap_or(0),
            best_score: self
                .best_node_id
                .as_ref()
                .and_then(|id| self.nodes.get(id))
                .map(|n| n.score)
                .unwrap_or(0.0),
            leaf_nodes: self
                .nodes
                .keys()
                .filter(|id| {
                    self.children
                        .get(id.as_str())
                        .map(|c| c.is_empty())
                        .unwrap_or(true)
                })
                .count(),
        }
    }

    /// 节点只读访问（铁律3 只读消费）
    pub fn get_node(&self, node_id: &str) -> Option<&ExperienceCard> {
        self.nodes.get(node_id)
    }

    /// 节点深度只读访问（D-3 偏差适配的可观测性）
    pub fn node_depth(&self, node_id: &str) -> Option<u32> {
        self.node_depth.get(node_id).copied()
    }

    /// 经 parent_id 回溯路径并反转（根 → 目标节点）
    fn trace_path(&self, node_id: &str) -> Vec<&ExperienceCard> {
        let mut path = Vec::new();
        let mut current = node_id;
        // 防御环: 深度上限即迭代上限（parent 链不可能超过节点深度）
        let mut guard = self.nodes.len() + 1;
        while let Some(card) = self.nodes.get(current) {
            path.push(card);
            match &card.parent_id {
                Some(parent) => current = parent.as_ref(),
                None => break,
            }
            guard -= 1;
            if guard == 0 {
                break;
            }
        }
        path.reverse();
        path
    }

    /// 追踪最佳节点（分数严格大于当前最佳才更新）
    fn update_best_node(&mut self, node_id: &str) {
        if let Some(new_card) = self.nodes.get(node_id) {
            let should_update = self
                .best_node_id
                .as_ref()
                .and_then(|id| self.nodes.get(id))
                .map(|best| new_card.score > best.score)
                .unwrap_or(true);
            if should_update {
                self.best_node_id = Some(node_id.to_string());
            }
        }
    }

    /// 根节点 ID（node_depth=0 的唯一节点）
    fn root_id(&self) -> Option<String> {
        self.node_depth
            .iter()
            .find(|(_, depth)| **depth == 0)
            .map(|(id, _)| id.clone())
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn create_root_semantics() {
        let mut tree = SearchTreeManager::new(10);
        let root_id = tree.create_root("task-1");
        assert_eq!(root_id, "root_task-1");
        let root = tree.get_node(&root_id).expect("根节点存在");
        assert_eq!(root.operator, AtomicOperator::Draft);
        assert_eq!(root.method_family.as_ref(), "root");
        // default_root 三因子：quality/progress=0，novelty=1
        assert_eq!(root.three_factor.novelty, 1.0);
        assert!(root.parent_id.is_none());
        assert_eq!(tree.node_depth(&root_id), Some(0));
    }

    #[test]
    fn expand_tracks_depth_and_best() {
        let mut tree = SearchTreeManager::new(10);
        let root_id = tree.create_root("task-1");
        let child_id = tree
            .expand_node(&root_id, card("n1", Some(root_id.as_str()), 0.8))
            .expect("扩展成功");
        assert_eq!(tree.node_depth(&child_id), Some(1), "D-3: 深度 = 父+1");
        // 子节点分数 > 根(0.0) → best 更新
        assert_eq!(tree.get_stats().best_score, 0.8);
    }

    #[test]
    fn expand_max_depth_gating() {
        let mut tree = SearchTreeManager::new(1);
        let root_id = tree.create_root("task-1");
        let child_id = tree
            .expand_node(&root_id, card("n1", Some(root_id.as_str()), 0.5))
            .expect("深度 1 允许");
        // 深度 2 > max_depth=1 → 拒绝
        let err = tree
            .expand_node(&child_id, card("n2", Some(child_id.as_str()), 0.6))
            .expect_err("应拒绝");
        assert_eq!(err, TreeError::MaxDepthReached);
    }

    #[test]
    fn expand_parent_not_found() {
        let mut tree = SearchTreeManager::new(10);
        let err = tree
            .expand_node("ghost", card("n1", None, 0.5))
            .expect_err("父节点不存在");
        assert_eq!(err, TreeError::ParentNotFound);
    }

    #[test]
    fn best_path_traces_to_root() {
        let mut tree = SearchTreeManager::new(10);
        let root_id = tree.create_root("task-1");
        let n1 = tree
            .expand_node(&root_id, card("n1", Some(root_id.as_str()), 0.5))
            .expect("扩展");
        let n2 = tree
            .expand_node(&n1, card("n2", Some(n1.as_str()), 0.9))
            .expect("扩展");
        let path = tree.get_best_path();
        // 路径：root → n1 → n2（best = n2）
        assert_eq!(path.len(), 3);
        assert_eq!(path[0].node_id.as_ref(), root_id.as_str());
        assert_eq!(path[2].node_id.as_ref(), n2.as_str());
    }

    #[test]
    fn prune_removes_low_score_leaves_only() {
        let mut tree = SearchTreeManager::new(10);
        let root_id = tree.create_root("task-1");
        // 低分叶 + 高分叶 + 低分有子节点
        tree.expand_node(&root_id, card("low-leaf", Some(root_id.as_str()), 0.1))
            .expect("扩展");
        tree.expand_node(&root_id, card("high-leaf", Some(root_id.as_str()), 0.9))
            .expect("扩展");
        let low_parent = tree
            .expand_node(&root_id, card("low-parent", Some(root_id.as_str()), 0.2))
            .expect("扩展");
        tree.expand_node(
            &low_parent,
            card("child-of-low", Some(low_parent.as_str()), 0.8),
        )
        .expect("扩展");

        tree.prune(0.5);
        // low-leaf 被剪；high-leaf 保留（分数高）；low-parent 保留（有子）
        assert!(tree.get_node("low-leaf").is_none());
        assert!(tree.get_node("high-leaf").is_some());
        assert!(tree.get_node("low-parent").is_some());
        assert_eq!(tree.get_stats().total_nodes, 4);
    }

    #[test]
    fn prune_preserves_best_node() {
        let mut tree = SearchTreeManager::new(10);
        let root_id = tree.create_root("task-1");
        tree.expand_node(&root_id, card("n1", Some(root_id.as_str()), 0.9))
            .expect("扩展");
        tree.prune(0.5);
        // best_node（n1 0.9）不受剪枝影响
        assert_eq!(tree.get_stats().best_score, 0.9);
    }

    #[test]
    fn stats_math() {
        let mut tree = SearchTreeManager::new(10);
        let root_id = tree.create_root("task-1");
        tree.expand_node(&root_id, card("n1", Some(root_id.as_str()), 0.7))
            .expect("扩展");
        let stats = tree.get_stats();
        assert_eq!(stats.total_nodes, 2);
        assert_eq!(stats.max_depth, 1);
        assert_eq!(stats.best_score, 0.7);
        assert_eq!(stats.leaf_nodes, 1, "n1 为唯一叶（根有子）");
    }

    #[test]
    fn cards_readonly_consumption() {
        // 铁律3: 树仅提供只读访问（get_node 返回 &ExperienceCard）
        let mut tree = SearchTreeManager::new(10);
        let root_id = tree.create_root("task-1");
        let card_ref: &ExperienceCard = tree.get_node(&root_id).expect("存在");
        assert_eq!(card_ref.task_id.as_ref(), "task-1");
    }
}
