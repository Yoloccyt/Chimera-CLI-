//! v3-engine M3 输出路径基准 — V3Output diff + TerminalWriter 写出
//!
//! 覆盖:相同帧(增量路径零输出开销)与 5% 变化帧(diff + ANSI 写出)。
//! 运行:`cargo bench -p chimera-tui --bench v3_output_bench -- --quick`

use chimera_tui::engine::buffer::{Buffer, Cell};
use chimera_tui::engine::output::V3Output;
use chimera_tui::engine::rect::Rect;
use criterion::{criterion_group, criterion_main, Criterion};

/// 构造一帧:按行列号取模填充字符,保证相邻帧差异可控
fn frame(width: u16, height: u16, seed: u8) -> Buffer {
    let mut buf = Buffer::empty(Rect::new(0, 0, width, height));
    for y in 0..height {
        for x in 0..width {
            let ch = ((x as u8)
                .wrapping_mul(31)
                .wrapping_add(y as u8)
                .wrapping_add(seed))
                % 95
                + 32;
            buf.set(x, y, Cell::new(ch as char));
        }
    }
    buf
}

fn v3_output_bench(c: &mut Criterion) {
    let mut group = c.benchmark_group("v3_output");

    // 相同帧:diff 为空,测增量路径固定开销(每帧换 sink,模拟 stdout flush)
    group.bench_function("identical_80x24", |b| {
        let mut out = V3Output::new();
        let f = frame(80, 24, 1);
        out.render(f.clone(), &mut Vec::new()).unwrap();
        b.iter(|| {
            let mut sink = Vec::new();
            out.render(f.clone(), &mut sink).unwrap();
            std::hint::black_box(&sink);
        });
    });

    // 5% 变化帧:seed 不同 → 全部格变化,测 diff+写出最坏路径
    group.bench_function("changed_120x40", |b| {
        let mut out = V3Output::new();
        let f = frame(120, 40, 1);
        out.render(f.clone(), &mut Vec::new()).unwrap();
        let f2 = frame(120, 40, 2);
        b.iter(|| {
            let mut sink = Vec::new();
            out.render(f2.clone(), &mut sink).unwrap();
            std::hint::black_box(&sink);
        });
    });

    group.finish();
}

criterion_group!(benches, v3_output_bench);
criterion_main!(benches);
