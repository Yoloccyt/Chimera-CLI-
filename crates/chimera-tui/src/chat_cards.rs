//! 会话流内嵌卡片(Concord W3 · T3.3)
//!
//! 对应架构层:L10 Interface
//!
//! # 设计决策(WHY)
//! Chat 模式的信息密度靠卡片而非面板堆叠:
//! - [`QuestPlanCard`] — 从 `quest_list` 首个 Quest 的任务列表派生计划步骤
//!   (状态符号 + 依赖深度缩进),数据零管道侵入(同 DagViz 派生模式);
//! - [`ReflectionCard`] — 从 `latest_events` 最近一次 `QuestCompleted{Failed/
//!   Cancelled}` 派生失败复盘卡,仅失败后出现。
//!
//! 渲染位置:Chat 模式会话流区底部附着块(先 Clear 再绘,不遮挡 composer)。
//! ReflectionCard 优先级高于 QuestPlanCard(失败告警优先于计划展示)。

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Paragraph, Widget};

use event_bus::{NexusEvent, QuestStatus};
use nexus_core::{Quest, TaskStatus};

use crate::types::TuiState;

/// 计划卡片单步(任务派生)
#[derive(Debug, Clone, PartialEq)]
pub struct PlanStep {
    /// 依赖深度缩进(0=根任务)
    pub depth: usize,
    /// 任务描述
    pub description: String,
    /// 任务状态
    pub status: TaskStatus,
}

/// Quest 计划卡片 — 当前 Quest 的任务步骤概览
#[derive(Debug, Clone, PartialEq)]
pub struct QuestPlanCard {
    /// 来源 Quest ID
    pub quest_id: String,
    /// Quest 标题
    pub title: String,
    /// 任务步骤(保持原始顺序,深度缩进表达依赖层级)
    pub steps: Vec<PlanStep>,
}

/// 失败复盘卡片 — 最近一次 Quest 失败/取消的可读复盘
#[derive(Debug, Clone, PartialEq)]
pub struct ReflectionCard {
    /// 失败 Quest ID
    pub quest_id: String,
    /// 终态(Failed / Cancelled)
    pub status: QuestStatus,
    /// 可折叠标记(方案既定项):折叠时仅呈现单行摘要,展开时含建议提示
    ///
    /// WHY 默认展开:失败告警优先可见;折叠态为后续交互波次(W4+)预留。
    pub collapsed: bool,
}

/// 从状态派生计划卡片(无 Quest 或无任务时为 None)
///
/// # 参数
/// - `state`:当前 TUI 状态(`quest_list` 由数据管道同步)
///
/// # 选取策略
/// 首个 Quest(quest_list 按事件序到达;多 Quest 时展示队首,
/// 与 Quest 面板首行一致,避免选择歧义)。
pub fn derive_quest_plan_card(state: &TuiState) -> Option<QuestPlanCard> {
    let quest = state.quest_list.first()?;
    if quest.tasks.is_empty() {
        return None;
    }
    let depths = task_depths(quest);
    let steps = quest
        .tasks
        .iter()
        .map(|t| PlanStep {
            depth: depths.get(t.task_id.as_str()).copied().unwrap_or(0),
            description: t.description.clone(),
            status: t.status,
        })
        .collect();
    Some(QuestPlanCard {
        quest_id: quest.quest_id.clone(),
        title: quest.title.clone(),
        steps,
    })
}

/// 从状态派生失败复盘卡(无失败/取消事件时为 None)
///
/// # 参数
/// - `state`:当前 TUI 状态(`latest_events` 为事件日志流)
///
/// # 扫描策略
/// 逆序扫描(最新优先),首个 `QuestCompleted{Failed|Cancelled}` 即返回。
pub fn derive_reflection_card(state: &TuiState) -> Option<ReflectionCard> {
    for ev in state.latest_events.iter().rev() {
        if let NexusEvent::QuestCompleted {
            quest_id, status, ..
        } = ev
        {
            if matches!(status, QuestStatus::Failed | QuestStatus::Cancelled) {
                return Some(ReflectionCard {
                    quest_id: quest_id.clone(),
                    status: *status,
                    collapsed: false,
                });
            }
        }
    }
    None
}

/// 渲染计划卡片(会话流底部附着块;区域过小时自动跳过不 panic)
pub fn render_quest_plan_card(card: &QuestPlanCard, area: Rect, buf: &mut Buffer) {
    if area.height < 3 || area.width < 10 {
        return;
    }
    let mut lines: Vec<Line<'_>> = Vec::with_capacity(card.steps.len() + 1);
    lines.push(Line::from(vec![
        Span::styled(
            format!("{} ", crate::t!("chat.plan_card.title")),
            Style::default().add_modifier(Modifier::BOLD),
        ),
        Span::styled(card.title.clone(), Style::default().fg(Color::Cyan)),
    ]));
    for step in &card.steps {
        let (marker, color) = status_marker(step.status);
        let indent = "  ".repeat(step.depth + 1);
        lines.push(Line::from(vec![
            Span::raw(indent),
            Span::styled(marker, Style::default().fg(color)),
            Span::raw(" "),
            Span::raw(step.description.clone()),
        ]));
    }
    Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(format!(" {} ", card.quest_id)),
        )
        .render(area, buf);
}

/// 渲染失败复盘卡(会话流底部附着块;区域过小时自动跳过不 panic)
pub fn render_reflection_card(card: &ReflectionCard, area: Rect, buf: &mut Buffer) {
    if area.height < 3 || area.width < 10 {
        return;
    }
    let status_label = match card.status {
        QuestStatus::Failed => crate::t!("chat.reflection_card.failed"),
        QuestStatus::Cancelled => crate::t!("chat.reflection_card.cancelled"),
        QuestStatus::Completed => crate::t!("chat.reflection_card.completed"),
    };
    // 可折叠标记(方案既定项):折叠态仅单行摘要,展开态含建议提示
    let lines: Vec<Line<'_>> = if card.collapsed {
        vec![Line::from(vec![
            Span::styled(
                format!("{} ", crate::t!("chat.reflection_card.title")),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            ),
            Span::raw(format!("Quest: {} ", card.quest_id)),
            Span::styled(status_label, Style::default().fg(Color::Yellow)),
        ])]
    } else {
        vec![
            Line::from(Span::styled(
                crate::t!("chat.reflection_card.title"),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
            )),
            Line::from(vec![
                Span::styled(format!("Quest: {}", card.quest_id), Style::default()),
                Span::raw("  "),
                Span::styled(status_label, Style::default().fg(Color::Yellow)),
            ]),
            Line::from(Span::styled(
                crate::t!("chat.reflection_card.hint"),
                Style::default().add_modifier(Modifier::DIM),
            )),
        ]
    };
    Paragraph::new(lines)
        .block(Block::default().borders(Borders::ALL))
        .render(area, buf);
}

/// 状态符号与颜色(与 DagViz 语义对齐:✓ 绿 / ▶ 黄 / ✗ 红 / ○ 灰)
fn status_marker(status: TaskStatus) -> (&'static str, Color) {
    match status {
        TaskStatus::Completed => ("✓", Color::Green),
        TaskStatus::Running => ("▶", Color::Yellow),
        TaskStatus::Failed => ("✗", Color::Red),
        _ => ("○", Color::Gray),
    }
}

/// 计算任务依赖深度(BFS 拓扑;环时深度封顶防无限循环)
///
/// WHY 独立实现:与 DagVizPanel::task_depths 语义一致,但卡片模块
/// 不依赖面板实现(模块边界清晰);任务规模小(单 Quest 数十级),
/// 复杂度 O(V+E) 无性能顾虑。
fn task_depths(quest: &Quest) -> std::collections::HashMap<String, usize> {
    let mut depths: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    // 根任务显式入表(深度 0);WHY 必须初始化:松弛循环仅在深度增大时插入,
    // 无依赖任务 new_depth=0 不会触发插入,后续依赖者查不到会误判为 0 深
    for task in &quest.tasks {
        depths.entry(task.task_id.clone()).or_insert(0);
    }
    // 迭代松弛:每轮用依赖深度+1 更新,封顶任务数防环
    let max_rounds = quest.tasks.len();
    for _ in 0..max_rounds {
        let mut changed = false;
        for task in &quest.tasks {
            let dep_max = task
                .dependencies
                .iter()
                .filter_map(|d| depths.get(d.as_str()).copied())
                .max();
            let new_depth = dep_max.map(|d| d + 1).unwrap_or(0);
            if depths.get(task.task_id.as_str()).copied().unwrap_or(0) < new_depth {
                depths.insert(task.task_id.clone(), new_depth);
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    depths
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_core::Task;

    /// 构造最小 TuiState 测试夹具
    fn state_with_quest(tasks: Vec<Task>) -> TuiState {
        let mut state = TuiState::new();
        state.quest_list.push(Quest {
            quest_id: "q-1".into(),
            title: "Demo Quest".into(),
            tasks,
            ..Default::default()
        });
        state
    }

    fn task(id: &str, deps: &[&str]) -> Task {
        Task {
            task_id: id.into(),
            description: format!("task {id}"),
            status: TaskStatus::Pending,
            dependencies: deps.iter().map(|s| s.to_string()).collect(),
        }
    }

    // === 派生测试 ===

    #[test]
    fn plan_card_none_without_quest() {
        let state = TuiState::new();
        assert!(derive_quest_plan_card(&state).is_none());
    }

    #[test]
    fn plan_card_none_without_tasks() {
        let state = state_with_quest(vec![]);
        assert!(derive_quest_plan_card(&state).is_none());
    }

    #[test]
    fn plan_card_derives_steps_with_depths() {
        let state = state_with_quest(vec![
            task("t1", &[]),
            task("t2", &["t1"]),
            task("t3", &["t2"]),
        ]);
        let card = derive_quest_plan_card(&state).expect("应派生卡片");
        assert_eq!(card.quest_id, "q-1");
        assert_eq!(card.title, "Demo Quest");
        assert_eq!(card.steps.len(), 3);
        // 依赖链 t1→t2→t3 深度递增
        let depth_of = |id: &str| {
            card.steps
                .iter()
                .position(|s| s.description == format!("task {id}"))
                .map(|i| card.steps[i].depth)
        };
        assert_eq!(depth_of("t1"), Some(0));
        assert_eq!(depth_of("t2"), Some(1));
        assert_eq!(depth_of("t3"), Some(2));
    }

    #[test]
    fn reflection_card_none_without_failure() {
        let mut state = TuiState::new();
        state.latest_events.push_back(NexusEvent::QuestCompleted {
            metadata: event_bus::EventMetadata::new("test"),
            quest_id: "q-ok".into(),
            status: QuestStatus::Completed,
        });
        assert!(derive_reflection_card(&state).is_none(), "成功不派生复盘卡");
    }

    #[test]
    fn reflection_card_derives_latest_failure() {
        let mut state = TuiState::new();
        state.latest_events.push_back(NexusEvent::QuestCompleted {
            metadata: event_bus::EventMetadata::new("test"),
            quest_id: "q-old".into(),
            status: QuestStatus::Failed,
        });
        state.latest_events.push_back(NexusEvent::QuestCompleted {
            metadata: event_bus::EventMetadata::new("test"),
            quest_id: "q-new".into(),
            status: QuestStatus::Cancelled,
        });
        let card = derive_reflection_card(&state).expect("应派生复盘卡");
        // 逆序扫描:最新失败优先
        assert_eq!(card.quest_id, "q-new");
        assert_eq!(card.status, QuestStatus::Cancelled);
    }

    // === 渲染冒烟 ===

    #[test]
    fn render_cards_smoke_and_small_area_no_panic() {
        let card = QuestPlanCard {
            quest_id: "q-1".into(),
            title: "Demo".into(),
            steps: vec![PlanStep {
                depth: 0,
                description: "step".into(),
                status: TaskStatus::Running,
            }],
        };
        let mut buf = Buffer::empty(Rect::new(0, 0, 40, 6));
        render_quest_plan_card(&card, Rect::new(0, 0, 40, 6), &mut buf);
        // 极小区域不 panic
        let mut tiny = Buffer::empty(Rect::new(0, 0, 4, 1));
        render_quest_plan_card(&card, Rect::new(0, 0, 4, 1), &mut tiny);

        let refl = ReflectionCard {
            quest_id: "q-2".into(),
            status: QuestStatus::Failed,
            collapsed: false,
        };
        let mut buf2 = Buffer::empty(Rect::new(0, 0, 40, 5));
        render_reflection_card(&refl, Rect::new(0, 0, 40, 5), &mut buf2);
        // 折叠态同样渲染不 panic
        let refl_collapsed = ReflectionCard {
            quest_id: "q-3".into(),
            status: QuestStatus::Cancelled,
            collapsed: true,
        };
        let mut buf3 = Buffer::empty(Rect::new(0, 0, 40, 3));
        render_reflection_card(&refl_collapsed, Rect::new(0, 0, 40, 3), &mut buf3);
        let mut tiny2 = Buffer::empty(Rect::new(0, 0, 2, 1));
        render_reflection_card(&refl, Rect::new(0, 0, 2, 1), &mut tiny2);
    }

    #[test]
    fn task_depths_cycle_capped() {
        // 环依赖不无限循环(深度封顶于任务数轮次)
        let quest = Quest {
            quest_id: "q".into(),
            title: "cyc".into(),
            tasks: vec![task("a", &["b"]), task("b", &["a"])],
            ..Default::default()
        };
        let depths = task_depths(&quest);
        // 环内深度有界(松弛轮次封顶于任务数,深度 ≤ 2×任务数),且函数终止
        assert!(depths.values().all(|&d| d <= 4));
    }
}
