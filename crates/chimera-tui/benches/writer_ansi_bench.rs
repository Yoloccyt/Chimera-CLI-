//! writer 输出性能基准(ADR-029,v3.1 M1.2)
//!
//! 对应架构层:L10 Interface(`chimera-tui::engine::writer`)
//!
//! # 基准项与目标
//! - `render_full_80x24`:一帧全变化(80×24,多段样式)经 diff 合并为 Span 后,
//!   `TerminalWriter` 渲染到内存 sink 的耗时。量化 M1.2 样式去重 + Span 合并的
//!   端到端输出成本(低带宽场景关键路径)。
//! - `render_incremental_5pct`:典型增量帧(约 5% 变化)的渲染耗时。
//!
//! # 设计理由(WHY)
//! - **内存 sink(Vec<u8>)**:排除真实终端 syscall 噪声,精确测量 writer 自身
//!   的样式去重 + ANSI 编码 CPU 成本;字节数缩减收益由 writer.rs 单测断言。
//! - **precompute changes**:diff 计算由 diff_engine_bench 单独覆盖,本 bench
//!   仅测 writer,changes 在 iter 外预计算。

#![forbid(unsafe_code)]

use chimera_tui::engine::{Buffer, Change, Color, DiffEngine, Rect, Style, TerminalWriter};
use criterion::{black_box, criterion_group, criterion_main, Criterion};

/// 构造一帧全变化的 changes(80×24,隔行不同前景色,触发多段样式切换)
fn full_frame_changes() -> Vec<Change> {
    let front = Buffer::empty(Rect::new(0, 0, 80, 24));
    let mut back = Buffer::empty(Rect::new(0, 0, 80, 24));
    let row = "x".repeat(80);
    for y in 0..24u16 {
        // 隔行切换前景色,模拟真实面板的多样式行,考验 writer 样式去重
        let style = if y % 2 == 0 {
            Style::new().fg(Color::Cyan)
        } else {
            Style::new().fg(Color::Gray)
        };
        back.set_string(0, y, &row, style);
    }
    DiffEngine::compute(&front, &back)
}

/// 构造增量帧 changes(约 5% 变化:每 20 格改 1 格)
fn incremental_changes() -> Vec<Change> {
    let front = Buffer::empty(Rect::new(0, 0, 80, 24));
    let mut back = Buffer::empty(Rect::new(0, 0, 80, 24));
    let total = 80usize * 24;
    for i in (0..total).step_by(20) {
        back.cells[i] = chimera_tui::engine::Cell::new('#');
    }
    DiffEngine::compute(&front, &back)
}

/// 全帧渲染:80×24 多段样式,measure writer 渲染到内存 sink 耗时
fn render_full_80x24(c: &mut Criterion) {
    let changes = full_frame_changes();
    let mut group = c.benchmark_group("writer_ansi");
    group.bench_function("render_full_80x24", |b| {
        b.iter(|| {
            let mut w = TerminalWriter::new(Vec::<u8>::new());
            w.render(black_box(&changes)).expect("render 失败");
            black_box(w.into_inner());
        });
    });
    group.finish();
}

/// 增量帧渲染:约 5% 变化,measure writer 渲染耗时
fn render_incremental_5pct(c: &mut Criterion) {
    let changes = incremental_changes();
    let mut group = c.benchmark_group("writer_ansi");
    group.bench_function("render_incremental_5pct", |b| {
        b.iter(|| {
            let mut w = TerminalWriter::new(Vec::<u8>::new());
            w.render(black_box(&changes)).expect("render 失败");
            black_box(w.into_inner());
        });
    });
    group.finish();
}

criterion_group!(benches, render_full_80x24, render_incremental_5pct);
criterion_main!(benches);
