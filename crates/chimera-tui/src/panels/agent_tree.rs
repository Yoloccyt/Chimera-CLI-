//! Agent 谱系面板 — 多智能体委托谱系树(Concord W10 T10.4,ADR-082)
//!
//! 对应架构层:L10 Interface
//!
//! # 职责
//! 以编排者(puppeteer)为根、spawn 谱系为边渲染委托树;节点显示完成/失败
//! 统计与最近任务(线程级运行时标注语义,方案 §7.2);Enter 钻取节点详情
//! (复用 `PopupKind::Detail`,panel.drill_down 既有钻取规则)。
//!
//! # 数据来源
//! `state.agent_tree`(AgentTreeSync 消费 AgentTaskDelegated/Completed/Failed
//! 事件维护);零管道侵入——仅读快照派生字段。
//!
//! # 布局算法
//! DFS 自根展开(无入边节点即根;孤立节点列于尾部),visited 集合防环;
//! 纯函数 `build_tree_lines` 与渲染解耦,单测直接断言行结构。

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use crate::data::sync::AgentTreeSnapshot;
use crate::panels::Panel;
use crate::popup::PopupKind;
use crate::types::{PanelId, TuiCommand, TuiState};

/// 树行 — DFS 展开后的单行(深度 + 节点下标)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TreeLine {
    /// 树深度(根为 0)
    pub depth: usize,
    /// `AgentTreeSnapshot::nodes` 下标
    pub node_index: usize,
}

/// 将谱系快照展开为 DFS 行序列(纯函数)
///
/// # 规则
/// - 根 = 无入边节点(编排者),按 nodes 首次出现序;
/// - 子节点 = 该节点作为 from 的边的 to,按边出现序;
/// - 孤立节点(无入边也无出边)同样视为根,自然列于序列;
/// - visited 集合防环(委托事件异常成环时不死循环,诚实截断)。
pub fn build_tree_lines(snapshot: &AgentTreeSnapshot) -> Vec<TreeLine> {
    let mut lines = Vec::new();
    let mut visited = vec![false; snapshot.nodes.len()];
    // 入边存在性:roots 判定
    let has_incoming: Vec<bool> = snapshot
        .nodes
        .iter()
        .map(|n| snapshot.edges.iter().any(|(_, to)| *to == n.agent_id))
        .collect();

    fn visit(
        snapshot: &AgentTreeSnapshot,
        node_index: usize,
        depth: usize,
        visited: &mut Vec<bool>,
        lines: &mut Vec<TreeLine>,
    ) {
        if visited[node_index] {
            return; // 防环:异常成环时截断,不重复展开
        }
        visited[node_index] = true;
        lines.push(TreeLine { depth, node_index });
        let parent_id = &snapshot.nodes[node_index].agent_id;
        for (from, to) in &snapshot.edges {
            if from != parent_id {
                continue;
            }
            if let Some(child_idx) = snapshot.nodes.iter().position(|n| n.agent_id == *to) {
                visit(snapshot, child_idx, depth + 1, visited, lines);
            }
        }
    }

    for (i, has_in) in has_incoming.iter().enumerate() {
        if !has_in {
            visit(snapshot, i, 0, &mut visited, &mut lines);
        }
    }
    // 二次遍历:纯环节点(全部有入边且未被根展开)以深度 0 诚实展开,
    // visited 截断保证不死循环(委托事件异常成环时谱系仍可见)。
    for i in 0..snapshot.nodes.len() {
        if !visited[i] {
            visit(snapshot, i, 0, &mut visited, &mut lines);
        }
    }
    lines
}

/// Agent 谱系面板
#[derive(Debug, Default)]
pub struct AgentTreePanel {
    /// 当前选中的行下标(展平行序列内)
    selected: usize,
    /// 垂直滚动偏移
    scroll: u16,
}

impl AgentTreePanel {
    /// 创建面板
    pub fn new() -> Self {
        Self::default()
    }

    /// 选中行钳制到合法域(行序列长度变化后调用)
    fn clamp_selected(&mut self, line_count: usize) {
        if line_count == 0 {
            self.selected = 0;
        } else {
            self.selected = self.selected.min(line_count - 1);
        }
    }

    /// 构建选中节点的钻取详情弹窗内容(纯函数,测试可直接断言)
    fn node_detail_content(snapshot: &AgentTreeSnapshot, node_index: usize) -> String {
        let Some(node) = snapshot.nodes.get(node_index) else {
            return String::new();
        };
        let children: Vec<&str> = snapshot
            .edges
            .iter()
            .filter(|(from, _)| *from == node.agent_id)
            .map(|(_, to)| to.as_str())
            .collect();
        let parent = snapshot
            .edges
            .iter()
            .find(|(_, to)| *to == node.agent_id)
            .map(|(from, _)| from.as_str());
        let mut out = String::new();
        out.push_str(&format!("Agent: {}\n", node.agent_id));
        out.push_str(&format!(
            "parent: {}\n",
            parent.unwrap_or("- (orchestrator root)")
        ));
        out.push_str(&format!("delegated: {}\n", node.delegated_out));
        out.push_str(&format!("completed: {}\n", node.completed));
        out.push_str(&format!("failed: {}\n", node.failed));
        out.push_str(&format!(
            "last_task: {}\n",
            node.last_task.as_deref().unwrap_or("-")
        ));
        out.push_str(&format!("children: {}", children.join(", ")));
        out
    }
}

impl Panel for AgentTreePanel {
    fn id(&self) -> PanelId {
        PanelId::AgentTree
    }

    fn title(&self) -> Line<'static> {
        Line::from(Span::styled(
            crate::t!("panel.agent_tree.title"),
            Style::default().fg(Color::Magenta),
        ))
    }

    fn render(&mut self, state: &TuiState, area: Rect, buf: &mut Buffer) {
        let block = Block::default().borders(Borders::ALL).title(self.title());
        let inner = block.inner(area);
        block.render(area, buf);
        if inner.height == 0 || inner.width == 0 {
            return;
        }

        let snapshot = &state.agent_tree;
        if snapshot.nodes.is_empty() {
            let empty = Paragraph::new(crate::t!("panel.agent_tree.empty").to_string())
                .style(Style::default().fg(Color::DarkGray));
            empty.render(inner, buf);
            return;
        }

        let lines = build_tree_lines(snapshot);
        self.clamp_selected(lines.len());
        // 滚动跟随选中行(简单钳制:选中行不可见时移动视口)
        if (self.selected as u16) < self.scroll {
            self.scroll = self.selected as u16;
        } else if (self.selected as u16) >= self.scroll + inner.height {
            self.scroll = self.selected as u16 - inner.height + 1;
        }

        for (row, line) in lines
            .iter()
            .skip(self.scroll as usize)
            .take(inner.height as usize)
            .enumerate()
        {
            let node = &snapshot.nodes[line.node_index];
            let indent = "  ".repeat(line.depth);
            let connector = if line.depth == 0 { "" } else { "└─ " };
            let label = format!(
                "{}{}{} [✓{} ✗{} ⇢{}]",
                indent, connector, node.agent_id, node.completed, node.failed, node.delegated_out
            );
            // 全局行下标 = 滚动偏移 + 视口内行号
            let is_selected = self.scroll as usize + row == self.selected;
            let style = if is_selected {
                Style::default().fg(Color::Black).bg(Color::Gray)
            } else if node.failed > 0 {
                Style::default().fg(Color::Red)
            } else {
                Style::default().fg(Color::Green)
            };
            let line = Line::from(Span::styled(label, style));
            let y = inner.y + row as u16;
            buf.set_line(inner.x, y, &line, inner.width);
        }
    }

    fn handle_key(&mut self, key: KeyEvent, state: &mut TuiState) -> Option<TuiCommand> {
        if key.kind != KeyEventKind::Press {
            return None;
        }
        let lines = build_tree_lines(&state.agent_tree);
        self.clamp_selected(lines.len());
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if !lines.is_empty() {
                    self.selected = (self.selected + 1).min(lines.len() - 1);
                }
            }
            // 钻取:Enter 打开选中节点详情弹窗(panel.drill_down 既有规则)
            KeyCode::Enter => {
                if let Some(line) = lines.get(self.selected) {
                    let node_id = state.agent_tree.nodes[line.node_index].agent_id.clone();
                    return Some(TuiCommand::OpenPopup(PopupKind::Detail {
                        title: node_id,
                        content: Self::node_detail_content(&state.agent_tree, line.node_index),
                        scroll: 0,
                    }));
                }
            }
            _ => {}
        }
        None
    }

    fn scroll_to_top(&mut self, _state: &mut TuiState) {
        self.selected = 0;
        self.scroll = 0;
    }

    fn scroll_to_bottom(&mut self, state: &mut TuiState) {
        let lines = build_tree_lines(&state.agent_tree);
        self.selected = lines.len().saturating_sub(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::data::sync::AgentNodeView;

    fn snapshot_with_chain() -> AgentTreeSnapshot {
        AgentTreeSnapshot {
            edges: vec![
                ("orchestrator".into(), "worker-1".into()),
                ("worker-1".into(), "worker-2".into()),
            ],
            nodes: vec![
                AgentNodeView {
                    agent_id: "orchestrator".into(),
                    delegated_out: 1,
                    completed: 0,
                    failed: 0,
                    last_task: None,
                },
                AgentNodeView {
                    agent_id: "worker-1".into(),
                    delegated_out: 1,
                    completed: 1,
                    failed: 0,
                    last_task: Some("t-1".into()),
                },
                AgentNodeView {
                    agent_id: "worker-2".into(),
                    delegated_out: 0,
                    completed: 0,
                    failed: 1,
                    last_task: Some("t-2".into()),
                },
            ],
        }
    }

    #[test]
    fn tree_lines_dfs_from_roots() {
        let lines = build_tree_lines(&snapshot_with_chain());
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].depth, 0);
        assert_eq!(lines[1].depth, 1);
        assert_eq!(lines[2].depth, 2);
        // 根为 orchestrator(无入边)
        assert_eq!(lines[0].node_index, 0);
    }

    #[test]
    fn tree_lines_cycle_safe() {
        // 异常成环:a→b→a 不得死循环,visited 截断
        let snap = AgentTreeSnapshot {
            edges: vec![("a".into(), "b".into()), ("b".into(), "a".into())],
            nodes: vec![
                AgentNodeView {
                    agent_id: "a".into(),
                    delegated_out: 1,
                    completed: 0,
                    failed: 0,
                    last_task: None,
                },
                AgentNodeView {
                    agent_id: "b".into(),
                    delegated_out: 1,
                    completed: 0,
                    failed: 0,
                    last_task: None,
                },
            ],
        };
        let lines = build_tree_lines(&snap);
        assert_eq!(lines.len(), 2, "成环时每节点仅展开一次");
    }

    #[test]
    fn empty_snapshot_yields_no_lines() {
        assert!(build_tree_lines(&AgentTreeSnapshot::default()).is_empty());
    }

    #[test]
    fn node_detail_contains_stats_and_lineage() {
        let snap = snapshot_with_chain();
        let detail = AgentTreePanel::node_detail_content(&snap, 1);
        assert!(detail.contains("worker-1"));
        assert!(detail.contains("parent: orchestrator"));
        assert!(detail.contains("children: worker-2"));
        assert!(detail.contains("last_task: t-1"));
    }

    #[test]
    fn root_node_detail_marks_orchestrator() {
        let snap = snapshot_with_chain();
        let detail = AgentTreePanel::node_detail_content(&snap, 0);
        assert!(
            detail.contains("(orchestrator root)"),
            "根节点无父边应标注编排者"
        );
    }
}
