//! 原子帧写出性能基准(Concord W7 T7.5,ADR-079)
//!
//! 对应架构层:L10 Interface(`chimera-tui::engine::atomic_frame`)
//!
//! # 基准项与目标
//! - `flush_disabled_80x24`:Disabled 模式下"帧缓冲累积 + 单次提交"的开销,
//!   量化批量单写本身的常数成本(目标:相对 writer 直写无结构性回归)。
//! - `flush_probed_80x24`:Probed 模式叠加 CSI ? 2026 h/l 包裹的开销
//!   (目标:每帧仅两条常量序列,O(1) 增量)。
//! - `flush_incremental_5pct`:典型增量帧(约 5% 变化)经原子写出的耗时。
//!
//! # 设计理由(WHY)
//! - **CountingWriter 内存 sink**:排除真实终端 syscall 噪声,同时可断言
//!   "每帧恰好一次 write"(批量单写不变量的 bench 侧证据;单测侧见
//!   atomic_frame.rs `finish_frame_is_single_write`)。
//! - **changes 预计算**:diff 成本由 diff_engine_bench 覆盖,本 bench
//!   仅测"编码进缓冲 + 提交"段。
//!
//! # 门槛(方案 §9.2 / 计划性能基准要求)
//! 两模式开销差应 ≤ 单帧 100µs 量级(实际为常数 memcpy);render 全链路
//! <16ms 与 diff <100µs 门槛由 v3_pipeline_bench / diff_engine_bench 守护。

#![forbid(unsafe_code)]

use chimera_tui::engine::atomic_frame::AtomicFrameWriter;
use chimera_tui::engine::sync_probe::SyncMode;
use chimera_tui::engine::{Buffer, Change, Color, DiffEngine, Rect, Style, TerminalWriter};
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use std::io::{self, Write};

/// 计数写出器:记录 write 调用次数(断言单帧单 write 不变量)
struct CountingWriter {
    inner: Vec<u8>,
    writes: usize,
}

impl Write for CountingWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.writes += 1;
        self.inner.extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// 构造一帧全变化的 changes(80×24,隔行样式,与 writer_ansi_bench 同构)
fn full_frame_changes() -> Vec<Change> {
    let front = Buffer::empty(Rect::new(0, 0, 80, 24));
    let mut back = Buffer::empty(Rect::new(0, 0, 80, 24));
    let row = "x".repeat(80);
    for y in 0..24u16 {
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

/// 单帧原子写出:changes 经 TerminalWriter 编码进帧缓冲后单次提交
fn bench_atomic_flush(c: &mut Criterion) {
    let full = full_frame_changes();
    let incr = incremental_changes();
    let mut group = c.benchmark_group("atomic_frame");

    group.bench_function("flush_disabled_80x24", |b| {
        let mut afw = AtomicFrameWriter::new(SyncMode::Disabled);
        let mut sink = CountingWriter {
            inner: Vec::new(),
            writes: 0,
        };
        b.iter(|| {
            afw.begin_frame();
            {
                let mut w = TerminalWriter::new(afw.buffer_mut());
                w.render(black_box(&full)).expect("render");
            }
            afw.finish_frame(black_box(&mut sink)).expect("flush");
        });
        // 批量单写不变量:每帧恰好一次 write(criterion 迭代次数即帧数)
        assert!(sink.writes > 0, "应至少提交一帧");
        assert!(!sink.inner.is_empty(), "帧字节应已提交到 sink");
    });

    group.bench_function("flush_probed_80x24", |b| {
        let mut afw = AtomicFrameWriter::new(SyncMode::Probed);
        let mut sink = CountingWriter {
            inner: Vec::new(),
            writes: 0,
        };
        b.iter(|| {
            afw.begin_frame();
            {
                let mut w = TerminalWriter::new(afw.buffer_mut());
                w.render(black_box(&full)).expect("render");
            }
            afw.finish_frame(black_box(&mut sink)).expect("flush");
        });
        assert!(sink.writes > 0, "应至少提交一帧");
        assert!(
            sink.inner.starts_with(b"\x1b[?2026h"),
            "Probed 帧应以同步序列开头"
        );
    });

    group.bench_function("flush_incremental_5pct", |b| {
        let mut afw = AtomicFrameWriter::new(SyncMode::Probed);
        let mut sink = CountingWriter {
            inner: Vec::new(),
            writes: 0,
        };
        b.iter(|| {
            afw.begin_frame();
            {
                let mut w = TerminalWriter::new(afw.buffer_mut());
                w.render(black_box(&incr)).expect("render");
            }
            afw.finish_frame(black_box(&mut sink)).expect("flush");
        });
        assert!(!sink.inner.is_empty(), "增量帧字节应已提交");
    });

    group.finish();
}

criterion_group!(benches, bench_atomic_flush);
criterion_main!(benches);
