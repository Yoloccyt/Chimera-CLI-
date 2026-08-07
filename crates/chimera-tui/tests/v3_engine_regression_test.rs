//! v3-engine 渲染路径回归测试(评估报告 P0-3:render_diffed 单遍合并路径)
//!
//! 对应架构层:L10 Interface(engine 模块)
//!
//! # 覆盖点
//! - 首帧全量输出(含 CJK 字节,无 NUL 哨兵泄漏);
//! - 相同帧零输出(增量路径);
//! - 变化帧仅输出 delta;
//! - `render_diffed`(单遍 compat+diff)与 `render`(完整 Buffer)输出等价;
//! - `from_ratatui_buffer_diffed` 与 `from_ratatui_buffer + compute` 结果等价;
//! - dirty 行跳过语义(clean 行零开销,不变量由调用方保证);
//! - resize 全量重绘。

#![forbid(unsafe_code)]

use chimera_tui::engine::buffer::DirtyTracker;
use chimera_tui::engine::output::V3Output;
use chimera_tui::engine::{
    from_ratatui_buffer, from_ratatui_buffer_diffed, Buffer, DiffEngine, Rect,
};
use ratatui::buffer::Buffer as RatBuffer;
use ratatui::layout::Rect as RatRect;
use ratatui::style::{Color, Style};

/// 构造指定尺寸的空 ratatui 帧
fn frame(w: u16, h: u16) -> RatBuffer {
    RatBuffer::empty(RatRect::new(0, 0, w, h))
}

/// 全行 dirty 标记
fn all_dirty(h: u16) -> DirtyTracker {
    let mut d = DirtyTracker::new(h);
    d.mark_all();
    d
}

fn styled() -> Style {
    Style::default().fg(Color::Green)
}

// ============================================================
// A. 首帧全量输出
// ============================================================

#[test]
fn first_frame_writes_full_content_without_nul() {
    let mut out = V3Output::new();
    let mut rb = frame(20, 4);
    rb.set_string(0, 0, "中文", styled());
    rb.set_string(5, 1, "ascii", styled());
    let mut sink = Vec::new();
    out.render_diffed(&rb, &all_dirty(4), &mut sink).unwrap();

    assert!(
        sink.windows(3).any(|w| w == "中".as_bytes()),
        "首帧应输出 CJK 字符"
    );
    assert!(
        sink.windows(4).any(|w| w == b"asci"),
        "首帧应输出 ASCII 内容"
    );
    assert!(
        !sink.contains(&0u8),
        "输出不应包含 NUL(宽字符续格哨兵被跳过)"
    );
    // front 应为最新帧
    assert_eq!(out.front().get(0, 0).unwrap().symbol, '中');
}

// ============================================================
// B. 增量路径:相同帧零输出 / 变化帧仅输出 delta
// ============================================================

#[test]
fn identical_frame_produces_no_output() {
    let mut out = V3Output::new();
    let mut rb = frame(10, 3);
    rb.set_string(0, 0, "static", styled());
    out.render_diffed(&rb, &all_dirty(3), &mut Vec::new())
        .unwrap();

    let mut sink = Vec::new();
    out.render_diffed(&rb, &all_dirty(3), &mut sink).unwrap();
    assert!(sink.is_empty(), "相同帧不应输出任何变化");
}

#[test]
fn changed_frame_outputs_only_delta() {
    let mut out = V3Output::new();
    let mut rb = frame(10, 3);
    rb.set_string(0, 0, "AAAA", styled());
    out.render_diffed(&rb, &all_dirty(3), &mut Vec::new())
        .unwrap();

    let mut rb2 = frame(10, 3);
    rb2.set_string(0, 0, "BBBB", styled());
    let mut sink = Vec::new();
    out.render_diffed(&rb2, &all_dirty(3), &mut sink).unwrap();
    assert!(sink.windows(4).any(|w| w == b"BBBB"), "应输出变化后的内容");
    assert!(
        !sink.windows(4).any(|w| w == b"AAAA"),
        "不应重复输出未变化的旧内容"
    );
    // front 原地演进为最新帧
    assert_eq!(out.front().get(0, 0).unwrap().symbol, 'B');
}

// ============================================================
// C. render_diffed 与 render 双路径输出等价
// ============================================================

#[test]
fn render_diffed_equivalent_to_render() {
    // 同一帧序列分别经两条路径输出,ANSI 字节应完全一致
    let mut rb = frame(30, 5);
    rb.set_string(2, 1, "hello", styled());
    rb.set_string(0, 3, "中文", styled());

    // 路径 A:render(完整 engine Buffer)
    let mut out_a = V3Output::new();
    let mut sink_a = Vec::new();
    let back_a = from_ratatui_buffer(&rb);
    out_a.render(back_a.clone(), &mut sink_a).unwrap();

    // 路径 B:render_diffed(ratatui Buffer + 全 dirty)
    let mut out_b = V3Output::new();
    let mut sink_b = Vec::new();
    out_b
        .render_diffed(&rb, &all_dirty(5), &mut sink_b)
        .unwrap();

    assert_eq!(sink_a, sink_b, "两路径首帧输出应逐字节一致");

    // 第二帧:变化后仍应一致
    let mut rb2 = frame(30, 5);
    rb2.set_string(2, 1, "world", styled());
    rb2.set_string(0, 3, "文字", styled());

    let mut sink_a2 = Vec::new();
    out_a
        .render(from_ratatui_buffer(&rb2), &mut sink_a2)
        .unwrap();
    let mut sink_b2 = Vec::new();
    out_b
        .render_diffed(&rb2, &all_dirty(5), &mut sink_b2)
        .unwrap();
    assert_eq!(sink_a2, sink_b2, "两路径增量帧输出应逐字节一致");
}

// ============================================================
// D. from_ratatui_buffer_diffed 与手动管线结果等价
// ============================================================

#[test]
fn diffed_matches_manual_pipeline() {
    // 单遍合并应与"转换 + 独立 diff"产出完全相同的 Change 列表
    let mut rb = frame(20, 4);
    rb.set_string(0, 0, "ab", styled());
    rb.set_string(8, 2, "xy", styled());

    let front = Buffer::empty(Rect::new(0, 0, 20, 4));
    let manual = DiffEngine::compute(&front, &from_ratatui_buffer(&rb));
    let diffed = from_ratatui_buffer_diffed(&front, &rb, &all_dirty(4));
    assert_eq!(manual, diffed, "单遍合并应与手动管线结果一致");
}

#[test]
fn diffed_skips_clean_rows() {
    // clean 行:即使内容与 front 不同也不产生 Change(不变量由调用方保证:
    // clean 行内容必须与已呈现帧相同,此处仅验证跳过机制本身)
    // WHY front 经同一转换路径构造:engine Buffer::empty 的样式语义(fg=None)
    // 与 ratatui 转换结果(fg=Some(Reset))不同,直接比较会产生全帧假差异;
    // V3Output 生产路径的 front 恒由 compat 转换维护,故测试用同源 front。
    let front = from_ratatui_buffer(&frame(10, 3));
    let mut rb = frame(10, 3);
    rb.set_string(0, 1, "changed", styled()); // 第 1 行内容与 front 不同

    // DirtyTracker::new 初始全脏,需 clear 后再 mark 指定行
    let mut dirty = DirtyTracker::new(3);
    dirty.clear();
    dirty.mark(1); // 仅第 1 行脏
    let changes = from_ratatui_buffer_diffed(&front, &rb, &dirty);
    // 只有第 1 行的变化被产出;第 0/2 行 clean 被跳过
    assert!(!changes.is_empty(), "脏行变化应被产出");
    let covered: usize = changes
        .iter()
        .map(|c| match c {
            chimera_tui::engine::Change::Cell { .. } => 1,
            chimera_tui::engine::Change::Span { cells, .. } => cells.len(),
        })
        .sum();
    assert!(covered <= 7, "仅脏行内容应被覆盖,实际覆盖 {covered} 格");

    // 全 clean(dirty 全 false)时零 Change
    let mut all_clean = DirtyTracker::new(3);
    all_clean.clear();
    let changes_none = from_ratatui_buffer_diffed(&front, &rb, &all_clean);
    assert!(changes_none.is_empty(), "全 clean 行应零变化");
}

// ============================================================
// E. resize 全量重绘
// ============================================================

#[test]
fn resize_forces_full_redraw() {
    let mut out = V3Output::new();
    let mut rb = frame(10, 3);
    rb.set_string(0, 0, "old", styled());
    out.render_diffed(&rb, &all_dirty(3), &mut Vec::new())
        .unwrap();

    // 尺寸变化(10x3 → 15x4):应全量输出新区域内容
    let mut rb2 = frame(15, 4);
    rb2.set_string(5, 2, "NEW", styled());
    let mut sink = Vec::new();
    out.render_diffed(&rb2, &all_dirty(4), &mut sink).unwrap();
    assert!(
        sink.windows(3).any(|w| w == b"NEW"),
        "resize 后应输出新内容"
    );
    // front 区域应更新为新尺寸
    assert_eq!(out.front().area, Rect::new(0, 0, 15, 4));
}
