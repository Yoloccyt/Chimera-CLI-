//! engine::compat 等价验证集成测试(ADR-029,v3.1 M1.1)
//!
//! 对应架构层:L10 Interface
//!
//! # 测试目标
//! 验证「现有面板渲染到 ratatui Buffer」→「compat 翻译为自研 engine Buffer」
//! 后,**字符网格与样式逐格等价**——这是 M2 将渲染路径切换到自研引擎的正确性
//! 前提(自研引擎输出必须与 ratatui 等价,不产生视觉差异)。
//!
//! # 设计约束(WHY)
//! - **直接渲染真实面板**:用 `QuestPanel`(含 fg/bg/BOLD/反色等多样式)渲染到
//!   ratatui `Buffer`,比构造合成 Buffer 更贴近生产渲染路径。
//! - **不启真实终端**:直接对 ratatui `Buffer::empty` 渲染,无需 TTY,CI 可跑。
//! - **不修改现有测试**:本文件为新增等价测试,现有 ~40 测试零改动。

#![forbid(unsafe_code)]

use chimera_tui::engine::compat::{from_ratatui_buffer, from_ratatui_modifier};
use chimera_tui::engine::Buffer as EngineBuffer;
use chimera_tui::panels::{Panel, QuestPanel};
use chimera_tui::TuiState;
use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};
use ratatui::buffer::Buffer as RatBuffer;
use ratatui::layout::{Position as RatPosition, Rect as RatRect};

/// 逐格断言 ratatui Buffer 与翻译后的 engine Buffer 等价(字符 + fg/bg/modifier)
///
/// WHY 逐格:等价性必须在每个单元格成立,任一格字符或样式不一致都会在真实
/// 终端产生可见差异,故不做采样,全量比对。
fn assert_grid_equivalent(rb: &RatBuffer, eng: &EngineBuffer) {
    let area = rb.area;
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            let rcell = rb
                .cell(RatPosition::new(x, y))
                .expect("ratatui cell 应存在");
            let ecell = eng.get(x, y).expect("engine cell 应存在");

            // 字符:取 ratatui symbol 首 char(skip cell 空串 → 空格),与 engine 对齐
            let rsym = rcell.symbol().chars().next().unwrap_or(' ');
            assert_eq!(rsym, ecell.symbol, "字符不一致 @({x},{y})");

            // 样式:fg/bg/modifier 经 compat 映射后应完全一致
            let rstyle = rcell.style();
            assert_eq!(
                ecell.style.fg,
                rstyle
                    .fg
                    .map(chimera_tui::engine::compat::from_ratatui_color),
                "前景色不一致 @({x},{y})"
            );
            assert_eq!(
                ecell.style.bg,
                rstyle
                    .bg
                    .map(chimera_tui::engine::compat::from_ratatui_color),
                "背景色不一致 @({x},{y})"
            );
            assert_eq!(
                ecell.style.modifier,
                from_ratatui_modifier(rstyle.add_modifier),
                "修饰不一致 @({x},{y})"
            );
        }
    }
}

/// 构造带任务的测试用 Quest(触发 Quest 面板多样式渲染路径)
fn sample_quest(id: &str, title: &str) -> Quest {
    Quest {
        quest_id: id.into(),
        title: title.into(),
        tasks: vec![
            Task {
                task_id: format!("{id}-t1"),
                description: "analyze".into(),
                status: TaskStatus::Completed,
                dependencies: vec![],
            },
            Task {
                task_id: format!("{id}-t2"),
                description: "implement".into(),
                status: TaskStatus::Running,
                dependencies: vec![],
            },
        ],
        thinking_mode: ThinkingMode::Standard,
        checkpoint_id: None,
        priority: 128,
    }
}

#[test]
fn quest_panel_render_translates_equivalently() {
    // 渲染带数据的 Quest 面板到 ratatui Buffer(含标题高亮/进度/灰色元信息等多样式)
    let area = RatRect::new(0, 0, 70, 20);
    let mut rb = RatBuffer::empty(area);
    let mut state = TuiState::new();
    state.quest_list = vec![
        sample_quest("q1", "Implement OSA coordinator"),
        sample_quest("q2", "Optimize routing"),
    ];
    let mut panel = QuestPanel::new();
    panel.render(&state, area, &mut rb);

    // 翻译并逐格断言等价
    let eng = from_ratatui_buffer(&rb);
    assert_eq!(eng.area, chimera_tui::engine::Rect::new(0, 0, 70, 20));
    assert_grid_equivalent(&rb, &eng);
}

#[test]
fn empty_panel_render_translates_equivalently() {
    // 空数据面板(No active quests 分支)同样应逐格等价
    let area = RatRect::new(0, 0, 40, 10);
    let mut rb = RatBuffer::empty(area);
    let state = TuiState::new();
    let mut panel = QuestPanel::new();
    panel.render(&state, area, &mut rb);

    let eng = from_ratatui_buffer(&rb);
    assert_grid_equivalent(&rb, &eng);
}

#[test]
fn offset_area_translation_preserves_coordinates() {
    // 非零原点区域(x/y 偏移):翻译后坐标语义须保持一致
    let area = RatRect::new(5, 3, 30, 8);
    let mut rb = RatBuffer::empty(area);
    let mut state = TuiState::new();
    state.quest_list = vec![sample_quest("q1", "Offset test")];
    let mut panel = QuestPanel::new();
    panel.render(&state, area, &mut rb);

    let eng = from_ratatui_buffer(&rb);
    assert_eq!(eng.area, chimera_tui::engine::Rect::new(5, 3, 30, 8));
    assert_grid_equivalent(&rb, &eng);
}
