//! TUI 事件循环 tick 基准 — 键盘事件处理 + 帧计数 + 面板刷新入口
//!
//! 覆盖用户指定热路径：
//! - `event_loop.rs`：`TuiApp::handle_key_event`（每键一次的事件路由）
//! - `types.rs`：`TuiState::tick_frame`（每帧一次的计数增量）
//! - 面板刷新：`EventStreamPanel::filtered_events` / `LogPanel::content`
//!   的万级事件成本由既有 `panels_scale_bench` 覆盖（本基准不重复建场景，
//!   报告直接引用其真实读数，避免两套口径）。
//!
//! 运行：
//! ```powershell
//! cargo bench -p chimera-tui --bench tui_tick_bench
//! ```
#![allow(clippy::unwrap_used, clippy::expect_used)] // bench code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use chimera_tui::{TuiApp, TuiConfig, TuiState};
use criterion::{criterion_group, criterion_main, Criterion};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

fn bench_tick(c: &mut Criterion) {
    let mut app = TuiApp::new(TuiConfig::default()).expect("TuiApp 构造失败");
    let mut state = TuiState::new();

    let mut group = c.benchmark_group("tui_tick");
    group.sample_size(50);

    // 每键一次的主事件路由（输入态字符键）
    group.bench_function("key_event_char", |b| {
        b.iter(|| {
            app.handle_key_event(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        });
    });

    // 每帧一次的帧计数增量（事件循环 tick 的固定开销下界）
    group.bench_function("tick_frame", |b| {
        b.iter(|| {
            state.tick_frame();
        });
    });

    group.finish();
}

criterion_group!(benches, bench_tick);
criterion_main!(benches);
