//! 自研渲染引擎差异计算基准(ADR-029,v3.1 M0 benchmark-first)
//!
//! 对应架构层:L10 Interface(`chimera-tui::engine::diff`)
//!
//! # 基准项与目标(RED-first)
//! - `diff_incremental`:典型增量帧(约 5% 单元格变化),目标 P95 < 100µs @80×24。
//! - `diff_full_redraw`:全量重绘(100% 变化),测量最坏路径上界。
//! - 三种终端尺寸(80×24 / 120×40 / 200×50)观察规模伸缩。
//!
//! # 设计理由(WHY)
//! - **纯内存 diff**:不含终端 IO,精确测量 `DiffEngine::compute` 的 CPU 时间,
//!   为 M1 的 Span 合并 / StylePool 优化提供可证伪的对比基线(§性能可证伪铁律)。
//! - **min-of-N 采样**:criterion 默认 sample_size=100 + warmup,等价 min-of-N,
//!   减少 Windows 调度噪声(与既有 render_bench 约定一致)。

#![forbid(unsafe_code)]

use chimera_tui::engine::{Buffer, Cell, DiffEngine, Rect};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};

/// 构造 (front, back):back 在 front 基础上按 `change_ratio` 比例修改单元格
///
/// WHY 定比例修改:模拟真实增量帧——大部分单元格不变,仅少量变化,
/// 覆盖 diff 的"逐格比较 + 少量输出"典型路径。
fn make_pair(width: u16, height: u16, change_ratio: f32) -> (Buffer, Buffer) {
    let area = Rect::new(0, 0, width, height);
    let front = Buffer::empty(area);
    let mut back = Buffer::empty(area);
    let total = (width as usize) * (height as usize);
    // 每隔 step 个单元格改一个,step 由变化比例决定(ratio 越大 step 越小)
    let step = if change_ratio >= 1.0 {
        1
    } else {
        ((1.0 / change_ratio) as usize).max(1)
    };
    for i in (0..total).step_by(step) {
        back.cells[i] = Cell::new('#');
    }
    (front, back)
}

/// 增量帧 diff:约 5% 单元格变化(目标 P95 < 100µs @80×24)
fn diff_incremental(c: &mut Criterion) {
    let sizes = [(80u16, 24u16), (120, 40), (200, 50)];
    let mut group = c.benchmark_group("diff_incremental_5pct");
    for (w, h) in sizes {
        let (front, back) = make_pair(w, h, 0.05);
        group.bench_with_input(BenchmarkId::new("size", format!("{w}x{h}")), &(), |b, _| {
            b.iter(|| {
                let changes = DiffEngine::compute(black_box(&front), black_box(&back));
                black_box(changes);
            });
        });
    }
    group.finish();
}

/// 全量重绘 diff:100% 单元格变化(最坏路径上界)
fn diff_full_redraw(c: &mut Criterion) {
    let sizes = [(80u16, 24u16), (120, 40), (200, 50)];
    let mut group = c.benchmark_group("diff_full_redraw");
    for (w, h) in sizes {
        let (front, back) = make_pair(w, h, 1.0);
        group.bench_with_input(BenchmarkId::new("size", format!("{w}x{h}")), &(), |b, _| {
            b.iter(|| {
                let changes = DiffEngine::compute(black_box(&front), black_box(&back));
                black_box(changes);
            });
        });
    }
    group.finish();
}

criterion_group!(benches, diff_incremental, diff_full_redraw);
criterion_main!(benches);
