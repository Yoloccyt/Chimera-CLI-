//! engine::layout::node — 布局树(arena)与递归区域计算(ADR-029,v3.1 M1.4)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! - **arena 存储**:节点存于 `Vec<BoxNode>`,子节点用 `NodeId`(下标)引用而非
//!   `Box`,避免递归树的堆碎片与所有权纠缠,`Clone` 廉价、缓存友好。
//! - **约束在子、方向在父**:每个节点携带"在父容器主轴上的约束",容器携带排列
//!   方向;`compute` 递归对每个容器调用 `flex::split` 切分,叶子获得最终区域。
//! - **三阶段折叠**:约束显式给定,measure 平凡;本引擎执行 layout(递归切分);
//!   paint(组件在区域内绘制)由 M2 组件层负责,不属布局引擎职责。

use crate::engine::layout::constraint::{Constraint, Direction};
use crate::engine::layout::flex::split;
use crate::engine::rect::Rect;

/// 节点标识 —— arena 下标
pub type NodeId = usize;

/// 布局盒节点 —— 叶子(无子节点)或容器(带方向 + 子节点)
#[derive(Debug, Clone)]
pub struct BoxNode {
    /// 本节点在父容器主轴上的尺寸约束
    pub constraint: Constraint,
    /// 容器排列方向(叶子节点忽略此字段)
    pub direction: Direction,
    /// 子节点 id 列表(叶子为空)
    pub children: Vec<NodeId>,
}

/// 布局树 —— arena 存储节点,递归切分计算各节点区域
#[derive(Debug, Clone, Default)]
pub struct LayoutTree {
    nodes: Vec<BoxNode>,
}

impl LayoutTree {
    /// 创建空树
    pub fn new() -> Self {
        Self::default()
    }

    /// 添加叶子节点,返回其 id(direction 占位,叶子不使用)
    pub fn add_leaf(&mut self, constraint: Constraint) -> NodeId {
        self.push(BoxNode {
            constraint,
            direction: Direction::Vertical,
            children: Vec::new(),
        })
    }

    /// 添加容器节点(排列方向 + 主轴约束 + 子节点),返回其 id
    pub fn add_container(
        &mut self,
        direction: Direction,
        constraint: Constraint,
        children: Vec<NodeId>,
    ) -> NodeId {
        self.push(BoxNode {
            constraint,
            direction,
            children,
        })
    }

    fn push(&mut self, node: BoxNode) -> NodeId {
        let id = self.nodes.len();
        self.nodes.push(node);
        id
    }

    /// 节点总数
    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    /// 树是否为空
    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    /// 从 `root` 起按 `viewport` 递归计算每个节点区域,返回按 `NodeId` 索引的 Rect 表
    ///
    /// 越界 root 返回全零 Rect 表(防御边界)。
    pub fn compute(&self, root: NodeId, viewport: Rect) -> Vec<Rect> {
        let mut rects = vec![Rect::default(); self.nodes.len()];
        if root < self.nodes.len() {
            self.layout_node(root, viewport, &mut rects);
        }
        rects
    }

    /// 递归:本节点占据 `area`,若为容器则按方向切分并递归子节点
    fn layout_node(&self, id: NodeId, area: Rect, rects: &mut [Rect]) {
        rects[id] = area;
        let node = &self.nodes[id];
        if node.children.is_empty() {
            return;
        }
        let constraints: Vec<Constraint> = node
            .children
            .iter()
            .map(|&c| self.nodes[c].constraint)
            .collect();
        let child_areas = split(area, node.direction, &constraints);
        for (&child, child_area) in node.children.iter().zip(child_areas) {
            self.layout_node(child, child_area, rects);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn single_leaf_gets_full_viewport() {
        let mut tree = LayoutTree::new();
        let leaf = tree.add_leaf(Constraint::Flex(1));
        let rects = tree.compute(leaf, Rect::new(0, 0, 80, 24));
        assert_eq!(rects[leaf], Rect::new(0, 0, 80, 24));
    }

    #[test]
    fn vertical_container_splits_children() {
        // 根:垂直排列 header(Fixed 3) + body(Flex) + footer(Fixed 1)
        let mut tree = LayoutTree::new();
        let header = tree.add_leaf(Constraint::Fixed(3));
        let body = tree.add_leaf(Constraint::Flex(1));
        let footer = tree.add_leaf(Constraint::Fixed(1));
        let root = tree.add_container(
            Direction::Vertical,
            Constraint::Flex(1),
            vec![header, body, footer],
        );

        let rects = tree.compute(root, Rect::new(0, 0, 80, 24));
        assert_eq!(rects[header], Rect::new(0, 0, 80, 3));
        assert_eq!(rects[body], Rect::new(0, 3, 80, 20));
        assert_eq!(rects[footer], Rect::new(0, 23, 80, 1));
    }

    #[test]
    fn nested_containers_partition_viewport_exactly() {
        // 根水平二分:左侧栏(Fixed 20)+ 右主区;右主区再垂直分为 body + status
        let mut tree = LayoutTree::new();
        let sidebar = tree.add_leaf(Constraint::Fixed(20));
        let body = tree.add_leaf(Constraint::Flex(1));
        let status = tree.add_leaf(Constraint::Fixed(1));
        let main = tree.add_container(Direction::Vertical, Constraint::Flex(1), vec![body, status]);
        let root = tree.add_container(
            Direction::Horizontal,
            Constraint::Flex(1),
            vec![sidebar, main],
        );

        let viewport = Rect::new(0, 0, 100, 30);
        let rects = tree.compute(root, viewport);

        // 叶子区域应精确划分 viewport(面积之和 == viewport 面积,无重叠无遗漏)
        let leaf_area: u32 = [sidebar, body, status]
            .iter()
            .map(|&id| rects[id].area())
            .sum();
        assert_eq!(leaf_area, viewport.area());
        assert_eq!(rects[sidebar], Rect::new(0, 0, 20, 30));
        assert_eq!(rects[body], Rect::new(20, 0, 80, 29));
        assert_eq!(rects[status], Rect::new(20, 29, 80, 1));
    }
}
