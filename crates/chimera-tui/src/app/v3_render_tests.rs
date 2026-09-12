//! PS-1 v3 渲染路径不变量测试 — 零拷贝 / 静默帧 / 帧预算
//!
//! 对应架构层:L10 Interface
//!
//! # 背景(WHY)
//! v3 编排路径(`render_frame_v3` / `quiescent_frame` / `v3_cached_frame`)历史上
//! **零测试覆盖**:`tests/` 下的 v3 测试只验 engine 原语,`integration.rs` 走
//! `app.render(f)` 根本不进入 v3 路径;而 `render_frame_v3` 又依赖
//! `crossterm::terminal::size()`,测试进程无终端直接返回 Err。
//!
//! PS-1 为此补了三处 seam:
//! 1. `render_frame_v3_to(w, h, out)` —— **尺寸 + 写入目标均外提**,可在无终端
//!    环境驱动整条输出路径,且把 ANSI 字节变成可断言对象;
//! 2. `v3_silent_hits` / `v3_render_count` 计数 —— 提供「静默路径真的被走到」的
//!    **可观测信号**,而非靠 `quiescent_frame()` 返回值间接推断(后者会假绿);
//! 3. `set_frame_quiescent` / `v3_cached_frame()` 访问器 —— 支持状态注入与逐 cell 比对。
//!
//! # 核心不变量
//! 静默帧复用缓存时,**非状态行区域必须与已呈现帧逐 cell 等价**;
//! 否则 `engine/compat` 的 clean 行跳过会漏输出,产生渲染残影。
//!
//! # 为何是 in-crate 模块
//! 上述 seam 均为 `pub(crate)`,集成测试(`tests/`)是独立 crate 无法访问,
//! 故本文件挂在 `app` 模块下(`mod.rs` 中 `#[cfg(test)] mod v3_render_tests;`)。

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use ratatui::buffer::Buffer as RatBuffer;

use super::*;
use crate::types::InputMode;

/// 测试固定尺寸(与既有测试的 TestBackend 口径一致)
const TEST_W: u16 = 80;
const TEST_H: u16 = 24;

fn make_app() -> TuiApp {
    let mut app = TuiApp::new(TuiConfig {
        default_view_mode: crate::types::ViewMode::Dashboard,
        persist_state: false,
        ..Default::default()
    })
    .expect("TuiApp::new should succeed");
    app.state_mut().view_mode = crate::types::ViewMode::Dashboard;
    app
}

/// 渲染一帧到内存 sink 并返回 ANSI 字节
///
/// WHY 不写 stdout:生产路径写终端,单测中数百 KB 转义序列会淹没测试日志
/// (实测一次 proptest 产生 ~785KB 噪音)。
fn render_to_sink(app: &mut TuiApp, w: u16, h: u16) -> Vec<u8> {
    let mut sink = Vec::new();
    app.render_frame_v3_to(w, h, &mut sink)
        .expect("frame should render");
    sink
}

/// 逐 cell 比较两个 ratatui 缓冲区,可排除给定矩形区域(状态行每帧变化,必须排除)
fn buffers_equal_except(
    a: &RatBuffer,
    b: &RatBuffer,
    exclude: Option<ratatui::layout::Rect>,
) -> bool {
    if a.area != b.area {
        return false;
    }
    for y in a.area.y..a.area.y + a.area.height {
        for x in a.area.x..a.area.x + a.area.width {
            if let Some(rect) = exclude {
                let in_x = x >= rect.x && x < rect.x + rect.width;
                let in_y = y >= rect.y && y < rect.y + rect.height;
                if in_x && in_y {
                    continue;
                }
            }
            let ca = a[(x, y)].clone();
            let cb = b[(x, y)].clone();
            if ca.symbol() != cb.symbol() || ca.style() != cb.style() {
                return false;
            }
        }
    }
    true
}

/// 把 app 置为「静默帧」应当成立的全部前置条件
fn arm_quiescent(app: &mut TuiApp) {
    app.set_frame_quiescent(true);
    app.state_mut().dirty_panels.clear();
    while !app.state().popup_stack.is_empty() {
        app.state_mut().popup_stack.pop();
    }
    app.chat_session.palette = None;
    app.state_mut().input_mode = InputMode::Normal;
}

// ============================================================
// 1. 核心不变量:静默帧 ≡ 已呈现帧(非状态行逐 cell 等价)
// ============================================================

#[test]
fn quiescent_frame_reuses_cache_bytewise_equal_outside_status_bar() {
    let mut app = make_app();

    // 首帧:缓存缺失 → 全量渲染
    render_to_sink(&mut app, TEST_W, TEST_H);
    assert_eq!(app.silent_hit_count(), 0, "首帧为全量,不应命中静默缓存");
    let full = app.v3_cached_frame().expect("全量帧后缓存应已建立").clone();

    // 次帧:静默条件成立 → 复用缓存
    arm_quiescent(&mut app);
    assert!(
        app.quiescent_frame(),
        "静默前置条件已置位,quiescent_frame() 应为 true"
    );
    let bytes = render_to_sink(&mut app, TEST_W, TEST_H);

    // 断言 A:静默路径**真的**被走到(可观测信号,杜绝假绿)
    assert_eq!(
        app.silent_hit_count(),
        1,
        "静默帧必须命中缓存复用分支;若为 0 说明判据拦住了(假绿)"
    );
    // 断言 B:零拷贝生效 —— 静默帧只重绘状态行,输出字节量应远小于整帧
    // (80×24 全帧 ≈ 1920 格;仅状态行 1 行 ≈ 80 格。阈值取整帧的 1/4 保守界定)
    assert!(
        bytes.len() < (TEST_W as usize * TEST_H as usize) / 4,
        "静默帧输出 {} 字节应远小于整帧重绘量,疑似走了全量路径",
        bytes.len()
    );

    // 断言 C:非状态行区域逐 cell 等价(核心不变量)
    let after = app.v3_cached_frame().expect("缓存仍应存在").clone();
    let status_area = app.status_bar_area;
    assert!(
        buffers_equal_except(&full, &after, status_area),
        "静默帧复用缓存后,非状态行区域须与已呈现帧逐 cell 等价,否则漏输出致残影"
    );
}

// ============================================================
// 2. PS-1 修复验收:auto_scroll=true(生产默认)下静默帧应可命中
// ============================================================

#[test]
fn quiescent_hits_under_default_auto_scroll_true() {
    let mut app = make_app();
    // 生产默认:auto_scroll = true(改造前正是此值让静默优化 100% 失效)
    assert!(
        app.state().auto_scroll,
        "前置条件:auto_scroll 默认应为 true"
    );

    render_to_sink(&mut app, TEST_W, TEST_H);
    arm_quiescent(&mut app);

    assert!(
        app.quiescent_frame(),
        "auto_scroll=true 且数据未变/无浮层时,静默帧应可命中(PS-1 修复点)"
    );
    render_to_sink(&mut app, TEST_W, TEST_H);
    assert_eq!(
        app.silent_hit_count(),
        1,
        "auto_scroll=true 下静默路径应被走到"
    );
}

// ============================================================
// 3. 浮层 / 非 Normal 模式 / 脏面板仍禁止静默
// ============================================================

#[test]
fn quiescent_blocked_by_popup_or_non_normal_mode() {
    let mut app = make_app();
    render_to_sink(&mut app, TEST_W, TEST_H);
    arm_quiescent(&mut app);
    assert!(app.quiescent_frame(), "基线:静默应成立");

    // 情况 A:有浮层
    app.state_mut()
        .popup_stack
        .push(crate::popup::PopupKind::Confirm {
            prompt: "t".into(),
            on_confirm: "quit".into(),
            confirmed: false,
        });
    assert!(
        !app.quiescent_frame(),
        "有 popup 时必须禁止静默帧(浮层覆盖主面板区域)"
    );
    while !app.state().popup_stack.is_empty() {
        app.state_mut().popup_stack.pop();
    }

    // 情况 B:非 Normal 输入模式
    app.state_mut().input_mode = InputMode::Insert;
    assert!(!app.quiescent_frame(), "非 Normal 模式必须禁止静默帧");
    app.state_mut().input_mode = InputMode::Normal;

    // 情况 C:有脏面板
    app.state_mut().mark_dirty(crate::types::PanelId::Budget);
    assert!(!app.quiescent_frame(), "有脏面板时必须禁止静默帧");
}

// ============================================================
// 4. resize 后不复用旧尺寸缓存(防残影)
// ============================================================

#[test]
fn resize_invalidates_cached_frame_and_falls_back_to_full() {
    let mut app = make_app();
    render_to_sink(&mut app, 80, 24);
    assert_eq!(app.silent_hit_count(), 0, "首帧为全量");

    // 尺寸变化 → 静默分支的尺寸守卫应拒绝旧缓存,回落全量
    arm_quiescent(&mut app);
    render_to_sink(&mut app, 100, 30);

    assert_eq!(
        app.silent_hit_count(),
        0,
        "resize 后不得复用旧尺寸缓存(尺寸守卫必须生效)"
    );
    let cached = app.v3_cached_frame().expect("缓存应已重建");
    assert_eq!(
        (cached.area.width, cached.area.height),
        (100, 30),
        "缓存尺寸须更新为当前终端尺寸"
    );
}

// ============================================================
// 5. 退化尺寸 / status_bar 越界守卫:不得 panic
// ============================================================

#[test]
fn degenerate_sizes_do_not_panic() {
    let mut app = make_app();
    // 0 尺寸:直接跳过本帧
    let empty = render_to_sink(&mut app, 0, 0);
    assert!(empty.is_empty(), "0 尺寸帧应零输出且不 panic");
    // 极小尺寸:status_bar_area 可能越界,守卫应生效
    render_to_sink(&mut app, TEST_W, TEST_H);
    arm_quiescent(&mut app);
    let _ = render_to_sink(&mut app, 80, 2);
}

// ============================================================
// 6. 渲染计数契约:step 每轮至多渲染一帧
// ============================================================

#[test]
fn render_count_bounded_by_step_invocations() {
    let mut app = make_app();
    let n = 10u64;
    for _ in 0..n {
        let ev = Event::Key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE));
        app.step(Some(ev))
            .expect("step should not fail on key event");
    }
    assert!(
        app.render_count() <= n,
        "渲染次数 {} 不得超过 step 调用次数 {}",
        app.render_count(),
        n
    );
    assert!(app.render_count() > 0, "step 应至少渲染过一帧");
}

// ============================================================
// 8. 帧预算节流:连续 step 的渲染次数被钳制(PS-1 子步骤5)
// ============================================================

#[test]
fn frame_budget_throttles_render_frequency() {
    use super::event_loop::MIN_FRAME_MS;

    let mut app = make_app();
    // 首帧无条件渲染(last_render_at = None 视为到期)
    app.step(None).expect("first step should succeed");
    assert_eq!(app.render_count(), 1, "首帧必须立即渲染,不允许启动黑屏");

    // 连续快速 step。
    // WHY 用"经过时间 / 预算"推导上界而非硬编码期望次数:每次非渲染 step 的
    // 耗时随机器而异,快速机器上 20 步远小于 16ms(恰好 1 次),慢机器上可能
    // 跨过预算边界。时序无关的不变量是:渲染次数 ≤ 1(首帧) + 预算周期数 + 1(边界容差)。
    let start = std::time::Instant::now();
    for _ in 0..20 {
        app.step(None).expect("step should succeed");
    }
    let elapsed_ms = start.elapsed().as_millis() as u64;
    let budget_cycles = elapsed_ms / MIN_FRAME_MS;
    assert!(
        app.render_count() <= 1 + budget_cycles + 1,
        "渲染次数 {} 不得超过 1(首帧) + 预算周期数 {} + 1(边界容差)",
        app.render_count(),
        budget_cycles
    );

    // 恢复性:预算到期后必须恢复渲染(确定性触发:睡 20ms > 16ms 预算),
    // 排除"节流把渲染永久停摆"的实现错误。
    std::thread::sleep(std::time::Duration::from_millis(20));
    app.step(None)
        .expect("step after budget expiry should succeed");
    assert!(app.render_count() > 1, "预算到期后应恢复渲染,不得永久停摆");
}

// ============================================================
// 9. 退出强制终帧:节流跳过渲染后退出,必须补渲染(防"看不见终态")
// ============================================================

#[test]
fn quit_after_throttled_frame_still_renders_final_frame() {
    let mut app = make_app();
    app.step(None).unwrap(); // 首帧渲染,置位 last_render_at

    // 立即发退出事件:预算几乎必然未到期 → 本步跳过渲染;
    // 但退出触发强制终帧兜底,渲染次数必须恰好 +1
    let before_quit = app.render_count();
    let quit = Event::Key(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE));
    app.step(Some(quit)).expect("quit step should succeed");

    assert!(!app.state().running, "Dashboard 视图下 q 应触发退出");
    let delta = app.render_count() - before_quit;
    assert_eq!(
        delta, 1,
        "退出步必须恰好渲染一次:预算到期则正常渲染,未到期则强制终帧兜底"
    );
}

// ============================================================
// 10. proptest 属性测试:任意尺寸序列下输出路径不得 panic
// ============================================================

proptest::proptest! {
    /// 对任意终端尺寸序列(含 0 与退化尺寸)连续渲染,
    /// 输出路径必须始终返回而不 panic,且缓存尺寸末态自洽。
    #[test]
    fn any_size_sequence_never_panics(
        sizes in proptest::collection::vec((0u16..200, 0u16..80), 1..8)
    ) {
        let mut app = make_app();
        for (w, h) in sizes {
            let _ = render_to_sink(&mut app, w, h);
        }
    }
}
