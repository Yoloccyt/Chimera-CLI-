//! v3-engine M3 全链路渲染路径基准(ADR-061 收口评估)
//!
//! 覆盖 `render_frame_v3` 的完整等价链路段(不含真实终端 IO,以 `Vec<u8>` 代 stdout):
//! 1. `TestBackend::new` + `Terminal::new`(每帧分配开销);
//! 2. `term.draw` 完整面板渲染(ratatui 帧绘制);
//! 3. `buffer().clone()` 整帧克隆;
//! 4. `from_ratatui_buffer` 逐格 compat 转换;
//! 5. `V3Output.render` 全帧 diff + TerminalWriter ANSI 写出。
//!
//! # 基线意义
//! 评估报告(2026-08-07)指出:生产默认 `v3-engine` 路径存在"clone + compat + diff"
//! 三重 O(W×H) 遍历,CPU 开销约为纯 ratatui 的 2-3 倍;本 bench 提供该开销的
//! 可证伪量化基线(`v3_pipeline_full`),优化后新增 `v3_pipeline_diffed` 对比路径,
//! 验证"compat 与 diff 单遍合并 + Terminal 复用"的收益(性能可证伪铁律)。
//!
//! 目标:P95 < 16ms @200×50(60 FPS 帧预算),优化后 `diffed` 应显著低于 `full`。
//! 运行:`cargo bench -p chimera-tui --bench v3_pipeline_bench -- --quick`

#![forbid(unsafe_code)]

use chimera_tui::engine::compat::from_ratatui_buffer;
use chimera_tui::engine::output::V3Output;
use chimera_tui::engine::DirtyTracker;
use chimera_tui::{TuiApp, TuiConfig};
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};
use event_bus::{EventMetadata, NexusEvent};
use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::collections::VecDeque;

/// 构造注入了数据的 TuiApp(Quest 列表 + 事件流,触发完整面板渲染路径)
fn make_app() -> TuiApp {
    let mut app = TuiApp::new(TuiConfig::default()).expect("TuiApp 构造失败");
    let state = app.state_mut();
    state.quest_list = (0..8)
        .map(|i| Quest {
            quest_id: format!("q{i}"),
            title: format!("Quest {i}"),
            tasks: vec![
                Task {
                    task_id: format!("q{i}-t1"),
                    description: "analyze requirements".into(),
                    status: TaskStatus::Completed,
                    dependencies: vec![],
                },
                Task {
                    task_id: format!("q{i}-t2"),
                    description: "implement feature".into(),
                    status: TaskStatus::Running,
                    dependencies: vec![format!("q{i}-t1")],
                },
            ],
            thinking_mode: ThinkingMode::Standard,
            checkpoint_id: None,
            priority: 128,
        })
        .collect();
    state.latest_events = (0..64)
        .map(|i| NexusEvent::CacheHit {
            metadata: EventMetadata::new("bench"),
            cache_key: format!("key-{i}"),
        })
        .collect::<VecDeque<_>>();
    app
}

/// 帧渲染到 ratatui TestBackend(链路段 1-3:分配 + draw + clone)
fn render_frame(app: &mut TuiApp, w: u16, h: u16) -> ratatui::buffer::Buffer {
    let backend = TestBackend::new(w, h);
    let mut term = Terminal::new(backend).expect("Terminal 构造失败");
    term.draw(|f| app.render(f)).expect("draw 失败");
    term.backend().buffer().clone()
}

/// 完整当前路径:TestBackend 新建 + draw + clone + compat + diff 写出
///
/// WHY 每次 iter 新建后端:复刻 `render_frame_v3`(event_loop.rs) 每帧分配行为,
/// 测量"含分配"的真实生产开销;`app` 在 iter 外创建保证数据稳定。
fn v3_pipeline_full(c: &mut Criterion) {
    let sizes = [(80u16, 24u16), (200, 50)];
    let mut group = c.benchmark_group("v3_pipeline_full");
    for (w, h) in sizes {
        let mut app = make_app();
        let mut out = V3Output::new();
        group.bench_with_input(BenchmarkId::new("size", format!("{w}x{h}")), &(), |b, _| {
            b.iter(|| {
                // 链路段 1-2:每帧新建 TestBackend + Terminal 并完整绘制
                let backend = TestBackend::new(w, h);
                let mut term = Terminal::new(backend).expect("Terminal 构造失败");
                term.draw(|f| app.render(f)).expect("draw 失败");
                // 链路段 3:整帧克隆
                let rb = term.backend().buffer().clone();
                // 链路段 4:逐格 compat 转换
                let back = from_ratatui_buffer(&rb);
                // 链路段 5:diff + ANSI 写出
                let mut sink = Vec::new();
                out.render(back, &mut sink).expect("v3 output render");
                black_box(&sink);
            });
        });
    }
    group.finish();
}

/// 面板渲染(不含输出)单独测量 — 定位 draw 自身开销,与输出链路段分离
fn v3_pipeline_draw_only(c: &mut Criterion) {
    let sizes = [(80u16, 24u16), (200, 50)];
    let mut group = c.benchmark_group("v3_pipeline_draw_only");
    for (w, h) in sizes {
        let mut app = make_app();
        group.bench_with_input(BenchmarkId::new("size", format!("{w}x{h}")), &(), |b, _| {
            b.iter(|| {
                let rb = render_frame(&mut app, w, h);
                black_box(&rb);
            });
        });
    }
    group.finish();
}

/// 优化后路径(评估报告 P0-1):复用 Terminal + `render_diffed` 单遍合并
///
/// 与 `v3_pipeline_full`(每帧新建 + clone + 独立 diff)同等工作量对比,
/// 量化单遍合并与 Terminal 复用的收益(性能可证伪铁律)。
fn v3_pipeline_diffed(c: &mut Criterion) {
    let sizes = [(80u16, 24u16), (200, 50)];
    let mut group = c.benchmark_group("v3_pipeline_diffed");
    for (w, h) in sizes {
        let mut app = make_app();
        let mut out = V3Output::new();
        let mut term = Terminal::new(TestBackend::new(w, h)).expect("Terminal 构造失败");
        // 全行 dirty:与 full 同等工作量(数据变化帧),仅对比路径差异
        let mut dirty = DirtyTracker::new(h);
        dirty.mark_all();
        group.bench_with_input(BenchmarkId::new("size", format!("{w}x{h}")), &(), |b, _| {
            b.iter(|| {
                term.draw(|f| app.render(f)).expect("draw 失败");
                let rb = term.backend().buffer().clone();
                let mut sink = Vec::new();
                out.render_diffed(&rb, &dirty, &mut sink)
                    .expect("v3 output render");
                black_box(&sink);
            });
        });
    }
    group.finish();
}

/// 优化后路径 + 静默帧(评估报告 P0-1 DirtyTracker 接线):仅 status_bar 行 dirty
///
/// 复刻生产 `quiescent_frame` 判定(无事件 + 数据未变):主面板区域行全部跳过
/// compat 转换与 diff 比较,量化行级跳过机制的收益(80x24 下约 92% 格免转换)。
fn v3_pipeline_quiet(c: &mut Criterion) {
    let sizes = [(80u16, 24u16), (200, 50)];
    let mut group = c.benchmark_group("v3_pipeline_quiet");
    for (w, h) in sizes {
        let mut app = make_app();
        let mut out = V3Output::new();
        let mut term = Terminal::new(TestBackend::new(w, h)).expect("Terminal 构造失败");
        // 首帧全量(diffed 内部处理),后续帧仅 status_bar 行比较
        let mut dirty = DirtyTracker::new(h);
        if h >= 2 {
            dirty.mark(h - 2);
        }
        let mut prime = Vec::new();
        term.draw(|f| app.render(f)).expect("draw 失败");
        let rb0 = term.backend().buffer().clone();
        out.render_diffed(&rb0, &dirty, &mut prime)
            .expect("首帧全量");
        group.bench_with_input(BenchmarkId::new("size", format!("{w}x{h}")), &(), |b, _| {
            b.iter(|| {
                term.draw(|f| app.render(f)).expect("draw 失败");
                let rb = term.backend().buffer().clone();
                let mut sink = Vec::new();
                out.render_diffed(&rb, &dirty, &mut sink)
                    .expect("v3 output render");
                black_box(&sink);
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    v3_pipeline_full,
    v3_pipeline_draw_only,
    v3_pipeline_diffed,
    v3_pipeline_quiet
);
criterion_main!(benches);
