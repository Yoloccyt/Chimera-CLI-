//! v3-engine 零拷贝重构 — 每帧整帧克隆代价基准(对照基准 / 回归守卫)
//!
//! # 背景(为什么需要这个 bench)
//! `benches/v3_pipeline_bench.rs::v3_pipeline_full` 只复刻了 `draw + clone A + compat + diff`
//! 一条整帧克隆(对应 `src/app/event_loop.rs:2370` 的 `term.backend().buffer().clone()`),
//! 但生产代码在每交互帧实际执行 **3 次** `O(W×H)` 整帧克隆,在静默帧执行 **1 次**:
//!
//! 全量帧(交互帧,`render_full_v3_frame` + 调用处):
//!   - `:2370` `let rb = term.backend().buffer().clone();`                  ← 克隆 A(必要:取出本帧)
//!   - `:2373` `self.v3_cached_frame = Some(rb.clone());`                   ← 克隆 B(写入缓存留作下帧)
//!   - `:2314` `self.v3_cached_frame.clone().expect("cache just written")`   ← 克隆 C(从缓存取出供 diff)
//!     其余 `:2309` 位于缓存缺失回退分支(`_ => render_full_v3_frame`),等价于克隆 C 的同一种
//!     "从缓存取用"模式,故全量帧 = 3 次克隆。
//!
//! 静默帧(命中缓存分支,`event_loop.rs:2288-2304`):
//!   - `:2288` `self.v3_cached_frame.take()`                              (无克隆,move 出)
//!   - `:2300` `self.render_status_bar(&mut cached, area)`                (原地改状态行,0 克隆)
//!   - `:2303` `self.v3_cached_frame = Some(cached.clone());`             ← 克隆(回写缓存)
//!   - `:2304` `cached`                                                  (move 出,供 diff,0 克隆)
//!     故静默帧 = 1 次克隆(`:2303`)。
//!
//! 零拷贝重构目标:全量帧 3→1(仅留克隆 A,缓存用同一缓冲 move/引用,去除 B、C)、
//! 静默帧 1→0(去除 `:2303`,move 回写而非克隆)。
//! 200×50 ≈ 10000 格,每格约 16 字节 ⇒ 全量帧每帧多 2 次克隆 ≈ 320KB 无谓拷贝;
//! 静默帧每帧多 1 次克隆 ≈ 160KB。本 bench 直接量化"克隆次数"这一单一变量的代价,
//! 作为防止日后回归重引入 clone 的守卫。
//!
//! # 四组对照(每组均真实走 `V3Output::render_diffed` 下游,避免只测 memcpy 高估收益占比)
//! 1. `frame_clone_x1`(重构后/理想,全量帧):`draw` + **1 次** `buffer().clone()` → 复刻仅克隆 A。
//! 2. `frame_clone_x3`(重构前/现状,全量帧):同上 + **2 次** `rb.clone()` → 精确复刻 `:2370 + :2373 + :2314` 的 3 次克隆。
//! 3. `quiescent_with_clone`(重构前,静默帧):`take()` → 原地改状态行 → **1 次** `clone()` 回写缓存 → move 出用 → 复刻 `:2303`。
//! 4. `quiescent_zero_copy`(重构后,静默帧):`take()` → 原地改状态行 → **0 次**克隆,直接用 → move 回写。
//!
//! # 阈值与回归基线(对照基准,非产品级 SLO)
//! 本 bench 用途是**量化与守卫**,不是产品级 SLO;目标阈值来自 `v3_pipeline_bench.rs` 头注释
//! (P95 < 16ms @200×50)。关注比值而非绝对值:
//!   - `frame_clone_x3 / frame_clone_x1` 应 **显著 > 1**(量化全量帧多 2 次克隆的代价);
//!     实测回归基线见下方"运行输出"表,CI 应断言该比值不低于基线某个下限
//!     (建议:一旦重构合并后该比值应回落到 ≈1.0,若日后重引入克隆会重新抬升)。
//!   - `quiescent_with_clone / quiescent_zero_copy` 应 **显著 > 1**(量化静默帧 1 次克隆的代价)。
//!     若重构生效,这两组比值会从">1"回落逼近 1.0——这正是守卫的观测点。
//!
//! # 运行
//! `cargo bench -p chimera-tui --bench v3_frame_clone_cost_bench -- --quick`

#![forbid(unsafe_code)]

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
    let mut app = TuiApp::new(TuiConfig {
        default_view_mode: chimera_tui::ViewMode::Dashboard,
        persist_state: false,
        ..Default::default()
    })
    .expect("TuiApp 构造失败");
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
    state.latest_events = std::sync::Arc::new(
        (0..64)
            .map(|i| NexusEvent::CacheHit {
                metadata: EventMetadata::new("bench"),
                cache_key: format!("key-{i}"),
            })
            .collect::<VecDeque<_>>(),
    );
    app
}

/// 全量帧对照:1 次克隆(重构后理想) vs 3 次克隆(重构前现状)
///
/// 两组下游工作完全相同(均 `mark_all` + `render_diffed`),唯一差异是整帧克隆次数,
/// 因此差值纯粹归因于"克隆次数",可直接量化零拷贝重构在全量路径上的收益。
fn frame_clone_full(c: &mut Criterion) {
    let sizes = [(80u16, 24u16), (200, 50)];
    let mut group = c.benchmark_group("frame_clone");

    for (w, h) in sizes {
        // ---- 重构后/理想:1 次整帧克隆(:2370 同种的必要克隆) ----
        group.bench_with_input(BenchmarkId::new("x1", format!("{w}x{h}")), &(), |b, _| {
            let mut app = make_app();
            let mut term = Terminal::new(TestBackend::new(w, h)).expect("Terminal 构造失败");
            let mut out = V3Output::new();
            // 预热:建立 render_diffed 首帧基线(first=false)
            term.draw(|f| app.render(f)).expect("draw 失败");
            let rb0 = term.backend().buffer().clone();
            let mut prime = Vec::new();
            let mut d0 = DirtyTracker::new(h);
            d0.mark_all();
            out.render_diffed(&rb0, &d0, &mut prime).expect("首帧全量");

            b.iter(|| {
                term.draw(|f| app.render(f)).expect("draw 失败");
                // 克隆 A:必要的整帧取出(重构后仅留这 1 次)
                let rb = term.backend().buffer().clone();
                let mut dirty = DirtyTracker::new(h);
                dirty.mark_all();
                let mut sink = Vec::new();
                out.render_diffed(&rb, &dirty, &mut sink)
                    .expect("v3 output render");
                black_box(&sink);
            });
        });

        // ---- 重构前/现状:3 次整帧克隆(:2370 + :2373 + :2314) ----
        group.bench_with_input(BenchmarkId::new("x3", format!("{w}x{h}")), &(), |b, _| {
            let mut app = make_app();
            let mut term = Terminal::new(TestBackend::new(w, h)).expect("Terminal 构造失败");
            let mut out = V3Output::new();
            term.draw(|f| app.render(f)).expect("draw 失败");
            let rb0 = term.backend().buffer().clone();
            let mut prime = Vec::new();
            let mut d0 = DirtyTracker::new(h);
            d0.mark_all();
            out.render_diffed(&rb0, &d0, &mut prime).expect("首帧全量");

            b.iter(|| {
                term.draw(|f| app.render(f)).expect("draw 失败");
                // 克隆 A(:2370):必要的整帧取出
                let rb = term.backend().buffer().clone();
                // 克隆 B(:2373):写入缓存留作下帧
                let cached = rb.clone();
                // 克隆 C(:2314):从缓存取出供 diff 使用
                let rb_use = cached.clone();
                let mut dirty = DirtyTracker::new(h);
                dirty.mark_all();
                let mut sink = Vec::new();
                out.render_diffed(&rb_use, &dirty, &mut sink)
                    .expect("v3 output render");
                black_box(&sink);
            });
        });
    }
    group.finish();
}

/// 静默帧对照:1 次克隆(重构前) vs 0 次克隆(重构后)
///
/// 静默帧 SKIP widget 重绘(无 `term.draw`),仅 `take()` 出缓存帧 → 原地改状态行 →
/// 走单行 dirty 的 `render_diffed`。两组下游工作完全相同,唯一差异是回写缓存时的克隆次数。
fn quiescent_clone(c: &mut Criterion) {
    let sizes = [(80u16, 24u16), (200, 50)];
    let mut group = c.benchmark_group("quiescent_clone");

    for (w, h) in sizes {
        // 预渲染一帧作为缓存(生产中由前一次全量帧写入)
        let mut app = make_app();
        let mut term = Terminal::new(TestBackend::new(w, h)).expect("Terminal 构造失败");
        term.draw(|f| app.render(f)).expect("draw 失败");
        let cached0 = term.backend().buffer().clone();

        // 状态行区域(与 v3_pipeline 系列一致:status 行 = h-2)
        let status_row = h.saturating_sub(2);
        let mut dirty = DirtyTracker::new(h);
        dirty.mark(status_row);

        // ---- 重构前/现状:1 次整帧克隆(:2303 回写缓存) ----
        group.bench_with_input(
            BenchmarkId::new("with_clone", format!("{w}x{h}")),
            &(),
            |b, _| {
                let mut out = V3Output::new();
                // 预热首帧
                let mut p = Vec::new();
                out.render_diffed(&cached0, &dirty, &mut p)
                    .expect("首帧全量");
                let mut cache = Some(cached0.clone());
                let tick = std::cell::Cell::new(0u8);
                b.iter(|| {
                    // 复刻生产命中缓存分支:take() 出 → 原地改状态行 → clone() 回写 → move 出用
                    let mut cached = cache.take().expect("cache 应存在");
                    tick.set(tick.get().wrapping_add(1));
                    cached[(0, status_row)].set_char((b'0' + tick.get() % 10) as char);
                    // 克隆(:2303)回写缓存
                    let restore = cached.clone();
                    // move 出供 diff 用(0 额外克隆)
                    out.render_diffed(&cached, &dirty, &mut Vec::new())
                        .expect("v3 output render");
                    cache = Some(restore);
                    black_box(&cache);
                });
            },
        );

        // ---- 重构后/理想:0 次整帧克隆(move 回写,无 clone) ----
        group.bench_with_input(
            BenchmarkId::new("zero_copy", format!("{w}x{h}")),
            &(),
            |b, _| {
                let mut out = V3Output::new();
                let mut p = Vec::new();
                out.render_diffed(&cached0, &dirty, &mut p)
                    .expect("首帧全量");
                let mut cache = Some(cached0.clone());
                let tick = std::cell::Cell::new(0u8);
                b.iter(|| {
                    // 零拷贝:take() → 原地改状态行 → 直接用(move)→ move 回写(0 克隆)
                    let mut cached = cache.take().expect("cache 应存在");
                    tick.set(tick.get().wrapping_add(1));
                    cached[(0, status_row)].set_char((b'0' + tick.get() % 10) as char);
                    out.render_diffed(&cached, &dirty, &mut Vec::new())
                        .expect("v3 output render");
                    // move 回写缓存(0 克隆)
                    cache = Some(cached);
                    black_box(&cache);
                });
            },
        );
    }
    group.finish();
}

criterion_group!(benches, frame_clone_full, quiescent_clone);
criterion_main!(benches);
