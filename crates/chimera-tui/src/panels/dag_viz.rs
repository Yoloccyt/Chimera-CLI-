//! TUI DagViz 面板 — Quest 任务 DAG 可视化(polish-v2.7 closure Stage B-10)
//!
//! 对应架构层:L10 Interface
//! 对应 ADR:ADR-049 决策 1(dag-viz-panel 落点 chimera-tui)
//! 对应设计源:`chimera_ultimate_polish_v2.7.md` §14.1(北大 DataFlow DAG 可视化)
//!
//! # 设计决策(WHY)
//! - **数据从 `quest_list` 派生**:QuestSync 已消费 `QuestListUpdated` 事件维护
//!   `TuiState.quest_list`,本面板只读渲染,零 DataPipeline/TuiState 字段侵入
//!   (复刻 SelfAssessmentPanel 的零管道侵入模式)
//! - **文本层级树而非图形绘制**:任务 DAG 按依赖深度分层缩进展示
//!   (`Task.dependencies` 表达边),与 TUI 既有文本视觉语言一致;
//!   方案 §14.1 的 Canvas 双模态交互依赖 TUI v3 引擎接线,作为后续增量
//! - **深度计算 = 最长依赖链**:任务的层级 = 其依赖的最大层级 + 1,
//!   环依赖(理论上被 quest-engine 拒绝)防御性截断为 0 层,避免渲染死循环

use crossterm::event::KeyEvent;
use nexus_core::{Task, TaskStatus};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};
use std::collections::HashMap;

use crate::panels::Panel;
use crate::types::{PanelId, TuiCommand, TuiState};

/// 单面板最多展示的 Quest 数(防大列表撑爆面板高度)
const MAX_QUESTS_SHOWN: usize = 3;

/// 深度计算的最大追溯层级(环依赖防御:超过即截断)
///
/// WHY 32:实际 Quest 任务链深度远小于此(HCW 分层哲学下 DAG 深度 <10);
/// 32 提供充分余量,同时保证异常数据下深度计算 O(n×32) 有界终止。
const MAX_DEPTH: usize = 32;

/// DagViz 面板 — Quest 任务 DAG 层级树可视化
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct DagVizPanel;

impl DagVizPanel {
    /// 创建新的 DagViz 面板
    pub fn new() -> Self {
        Self
    }

    /// 计算任务的 DAG 深度(最长依赖链,迭代求解防环)
    ///
    /// 返回 task_id → 深度的映射:无依赖任务深度 0,
    /// 有依赖任务深度 = max(依赖深度) + 1。
    /// 迭代至不动点或 MAX_DEPTH 轮(环依赖时未收敛项保持当前值,不会死循环)。
    fn task_depths(tasks: &[Task]) -> HashMap<&str, usize> {
        let mut depths: HashMap<&str, usize> =
            tasks.iter().map(|t| (t.task_id.as_str(), 0usize)).collect();

        // 迭代松弛:每轮至少确定一层,MAX_DEPTH 轮后必然终止
        for _ in 0..MAX_DEPTH {
            let mut changed = false;
            for task in tasks {
                let dep_max = task
                    .dependencies
                    .iter()
                    .filter_map(|d| depths.get(d.as_str()).copied())
                    .max();
                if let Some(dep_max) = dep_max {
                    let new_depth = (dep_max + 1).min(MAX_DEPTH);
                    let entry = depths.entry(task.task_id.as_str()).or_insert(0);
                    if *entry != new_depth {
                        *entry = new_depth;
                        changed = true;
                    }
                }
            }
            if !changed {
                break;
            }
        }
        depths
    }

    /// 任务状态 → 视觉标记与颜色
    fn status_marker(status: &TaskStatus) -> (&'static str, Color) {
        match status {
            TaskStatus::Pending => ("○", Color::Gray),
            TaskStatus::Running => ("◐", Color::Yellow),
            TaskStatus::Completed => ("●", Color::Green),
            TaskStatus::Failed => ("✗", Color::Red),
            TaskStatus::Cancelled => ("⊗", Color::Red),
            TaskStatus::Paused => ("⏸", Color::Yellow),
        }
    }

    /// 构建面板文本内容 — 每个 Quest 一棵层级树
    pub fn content(state: &TuiState) -> Text<'static> {
        let mut lines: Vec<Line<'static>> = vec![
            Line::from("Quest Task DAG (depth-indented)"),
            Line::from("──────────────────────────────────────"),
        ];

        // Task 3.5: L5 Knowledge 协同 — 显示谱系 DAG 节点/边计数
        // 调用 gsoe_evolution::spec_dag_snapshot() 获取 GSOE 谱系演化图,
        // 实现 L10 Panel ↔ L5 Knowledge 真实数据闭环。
        // WHY 在 quest_list 判断之前:空 quest 时仍显示谱系信息,确保面板始终有内容。
        {
            let snapshot = gsoe_evolution::spec_dag_snapshot();
            lines.push(Line::from(Span::styled(
                format!(
                    "Spec DAG: {} nodes, {} edges",
                    snapshot.nodes.len(),
                    snapshot.edges.len()
                ),
                Style::default().fg(Color::Gray),
            )));
            lines.push(Line::from(""));
        }

        if state.quest_list.is_empty() {
            lines.push(Line::from(Span::styled(
                "Awaiting QuestListUpdated...",
                Style::default().fg(Color::Gray),
            )));
            return Text::from(lines);
        }

        for quest in state.quest_list.iter().take(MAX_QUESTS_SHOWN) {
            // Quest 头行:标题 + 完成度
            let completed = quest
                .tasks
                .iter()
                .filter(|t| t.status == TaskStatus::Completed)
                .count();
            lines.push(Line::from(Span::styled(
                format!("▶ {} [{}/{}]", quest.title, completed, quest.tasks.len()),
                Style::default().fg(Color::Cyan),
            )));

            // 任务按深度缩进展示(同深度保持原始顺序,稳定可预期)
            let depths = Self::task_depths(&quest.tasks);
            for task in &quest.tasks {
                let depth = depths.get(task.task_id.as_str()).copied().unwrap_or(0);
                let (marker, color) = Self::status_marker(&task.status);
                let indent = "  ".repeat(depth + 1);
                // 依赖标注:有依赖时显示 "← dep1,dep2"(Agent 可读的边表达)
                let deps_note = if task.dependencies.is_empty() {
                    String::new()
                } else {
                    format!(" ← {}", task.dependencies.join(","))
                };
                lines.push(Line::from(vec![
                    Span::from(indent),
                    Span::styled(marker.to_string(), Style::default().fg(color)),
                    Span::from(format!(" {}{}", task.description, deps_note)),
                ]));
            }
            lines.push(Line::from(""));
        }

        if state.quest_list.len() > MAX_QUESTS_SHOWN {
            lines.push(Line::from(Span::styled(
                format!(
                    "... {} more quests (see Quest panel)",
                    state.quest_list.len() - MAX_QUESTS_SHOWN
                ),
                Style::default().fg(Color::Gray),
            )));
        }

        Text::from(lines)
    }
}

impl Panel for DagVizPanel {
    fn id(&self) -> PanelId {
        PanelId::DagViz
    }

    fn title(&self) -> Line<'static> {
        Line::from(crate::t!("panel.border.dag_viz"))
    }

    fn render(&mut self, state: &TuiState, area: Rect, buf: &mut Buffer) {
        let block = Block::default().borders(Borders::ALL).title(self.title());
        let paragraph = Paragraph::new(Self::content(state)).block(block);
        paragraph.render(area, buf);
    }

    fn handle_key(&mut self, _key: KeyEvent, _state: &mut TuiState) -> Option<TuiCommand> {
        // 展示型面板不处理专属按键(同 SelfAssessment 面板)
        None
    }

    fn shortcuts(&self) -> Vec<(&'static str, &'static str)> {
        vec![("Tab", crate::t!("shortcut.switch_panel"))]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_core::{Quest, ThinkingMode};

    fn task(id: &str, desc: &str, status: TaskStatus, deps: &[&str]) -> Task {
        Task {
            task_id: id.to_string(),
            description: desc.to_string(),
            status,
            dependencies: deps.iter().map(|d| d.to_string()).collect(),
        }
    }

    fn quest_with_tasks(tasks: Vec<Task>) -> Quest {
        Quest {
            quest_id: "q1".to_string(),
            title: "demo quest".to_string(),
            tasks,
            thinking_mode: ThinkingMode::Standard,
            checkpoint_id: None,
            priority: 128,
        }
    }

    #[test]
    fn test_dag_viz_panel_id() {
        assert_eq!(DagVizPanel::new().id(), PanelId::DagViz);
    }

    #[test]
    fn test_content_awaiting_when_no_quests() {
        let state = TuiState::new();
        let content = DagVizPanel::content(&state).to_string();
        assert!(content.contains("Awaiting QuestListUpdated"));
    }

    #[test]
    fn test_task_depths_linear_chain() {
        // a → b → c 线性链:深度 0/1/2
        let tasks = vec![
            task("a", "root", TaskStatus::Completed, &[]),
            task("b", "mid", TaskStatus::Running, &["a"]),
            task("c", "leaf", TaskStatus::Pending, &["b"]),
        ];
        let depths = DagVizPanel::task_depths(&tasks);
        assert_eq!(depths["a"], 0);
        assert_eq!(depths["b"], 1);
        assert_eq!(depths["c"], 2);
    }

    #[test]
    fn test_task_depths_diamond() {
        // 菱形:a → (b,c) → d,d 深度 = 2
        let tasks = vec![
            task("a", "root", TaskStatus::Completed, &[]),
            task("b", "left", TaskStatus::Completed, &["a"]),
            task("c", "right", TaskStatus::Running, &["a"]),
            task("d", "join", TaskStatus::Pending, &["b", "c"]),
        ];
        let depths = DagVizPanel::task_depths(&tasks);
        assert_eq!(depths["d"], 2);
    }

    #[test]
    fn test_task_depths_cycle_terminates() {
        // 环依赖(异常数据):深度计算必须有界终止,不 panic 不死循环
        let tasks = vec![
            task("a", "x", TaskStatus::Pending, &["b"]),
            task("b", "y", TaskStatus::Pending, &["a"]),
        ];
        let depths = DagVizPanel::task_depths(&tasks);
        // 环内深度被 MAX_DEPTH 截断(具体值不重要,有界即可)
        assert!(depths["a"] <= MAX_DEPTH);
        assert!(depths["b"] <= MAX_DEPTH);
    }

    #[test]
    fn test_content_renders_quest_tree_with_deps() {
        let mut state = TuiState::new();
        state.quest_list.push(quest_with_tasks(vec![
            task("t1", "setup env", TaskStatus::Completed, &[]),
            task("t2", "run tests", TaskStatus::Running, &["t1"]),
        ]));
        let content = DagVizPanel::content(&state).to_string();
        assert!(content.contains("demo quest [1/2]"));
        assert!(content.contains("setup env"));
        // 依赖边标注
        assert!(content.contains("run tests ← t1"));
    }

    #[test]
    fn test_content_truncates_quest_list() {
        let mut state = TuiState::new();
        for i in 0..5 {
            let mut q = quest_with_tasks(vec![]);
            q.quest_id = format!("q{i}");
            q.title = format!("quest {i}");
            state.quest_list.push(q);
        }
        let content = DagVizPanel::content(&state).to_string();
        assert!(content.contains("2 more quests"));
    }
}
