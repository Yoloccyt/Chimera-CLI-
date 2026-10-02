//! 跨面板滚动可见性不变量 proptest(批次三 B3-V)
//!
//! # 目的
//! R5 F-11 型"上缓冲推挤"缺陷曾同族存在于 Parliament / Log / EventStream /
//! Timeline / OsaSparse / ClvVector 六面板:共享 `virtual_scroll_window` 的
//! ±VIRTUAL_SCROLL_BUFFER(5) 缓冲原样进入无 `.scroll` 补偿的 Text,把可视带
//! 整体下移 —— 稳态下滚(`adjust_scroll` 钉选中于 Δ=visible-1)时选中行落在
//! 可视带之外恒不可见。前四面板已在批次一/二按"窗口基 = scroll_offset + 末端
//! 回填"收口,本批收口最后两处(osa_sparse / clv_vector),并以属性测试对
//! 五面板(quest 例外:全量文本 + Paragraph::scroll,另一套语义)一次性锁定:
//!
//! **属性**:对任意 (count, selected, viewport_height) 组合,面板窗口化后的
//! 渲染缓冲必含选中行(选中行标记为逐行唯一的种子标识,`contains` 为真判别:
//! 标记只能来自选中行本身,该断言对修复前的推挤行为必红)。
//!
//! # WHY 属性测试而非枚举样例
//! (count × selected × viewport) 是三维组合空间且窗口算术边界不连续
//! (窗口满/贴底回填/短列表三态),枚举样例易漏;proptest 随机覆盖 + 失败时
//! 自动收缩到最小反例,与 panel_degenerate_size_proptest 的既有范式一致。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::sync::Arc;

use chimera_tui::panels::{
    ClvVectorPanel, EventStreamPanel, LogPanel, OsaSparsePanel, Panel, ParliamentPanel,
    TimelinePanel,
};
use chimera_tui::render::scroll_offset_window;
use chimera_tui::types::{TimelineSnapshot, TuiState};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use event_bus::{EventMetadata, NexusEvent};
use proptest::prelude::*;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

/// 面板渲染区统一宽度(≥ 各面板最小宽度守卫 + 边框,留足行文本不横向截断)
const WIDTH: u16 = 60;

/// 收集缓冲区全部单元格文本(逐 cell 取首字符,与
/// panel_degenerate_size_proptest 同口径;选中行标记均为 ASCII 等宽 token,
/// chrome 里的 CJK 宽字符续格不影响标记匹配)
fn buffer_text(buf: &Buffer) -> String {
    buf.content()
        .iter()
        .map(|c| c.symbol().chars().next().unwrap_or(' '))
        .collect()
}

/// 构造 n 条 VoteCast 事件,source / proposal_id 逐条唯一(行渲染含该标识,
/// 作为选中行可见性标记;三位零填充等宽,互不为子串)
fn vote_cast_events(prefix: &str, n: usize) -> VecDeque<NexusEvent> {
    (0..n)
        .map(|i| NexusEvent::VoteCast {
            metadata: EventMetadata::new(format!("{prefix}-{i:03}")),
            proposal_id: format!("{prefix}-{i:03}"),
            voter: "v".into(),
            vote: true,
        })
        .collect()
}

/// 连按 Down `times` 次(经 list_state::handle_key_navigation 移动选中项,
/// 与用户滚动路径一致;move_selection 自带 count-1 钳位)
fn press_down(panel: &mut impl Panel, state: &mut TuiState, times: usize) {
    let down = KeyEvent::new(KeyCode::Down, KeyModifiers::NONE);
    for _ in 0..times {
        panel.handle_key(down, state);
    }
}

/// 贴底滞留 → 数据收缩 → 退化高度渲染(B3-R 下溢带复现序列)
///
/// `scroll_to_bottom` 是 G 键(RouteTarget::ScrollBottom)的真实落点:直接置
/// `selected = scroll_offset = count-1`,不经渲染钳制;随后数据收缩使 total
/// 变小,再以 visible_rows==0 带高度渲染 —— adjust_scroll 在该带提前原样返回
/// 滞留 offset(list_state.rs:29-31),修复前窗口回填算术 `end - start` 在
/// offset > total 时 usize 下溢(debug 构建 panic)。
fn render_tiny_after_shrink<P: Panel>(
    panel: &mut P,
    state: &mut TuiState,
    set_count: impl Fn(&mut TuiState, usize),
    initial_count: usize,
    shrunk_count: usize,
    tiny_height: u16,
) {
    set_count(state, initial_count);
    panel.scroll_to_bottom(state);
    set_count(state, shrunk_count);
    let area = Rect::new(0, 0, WIDTH, tiny_height);
    let mut buf = Buffer::empty(area);
    panel.render(state, area, &mut buf);
}

proptest! {
    // ============================================================
    // OsaSparse:context 活跃文件列表(行渲染含 file_id)
    // ============================================================

    #[test]
    fn osa_sparse_windowed_render_contains_selected_row(
        count in 0..=48usize,
        selected in 0..=48usize,
        area_height in 10u16..=60,
    ) {
        let mut state = TuiState::new();
        state.osa_context_mask = (0..count).map(|i| format!("osa-{i:03}")).collect();
        let mut panel = OsaSparsePanel::new();

        // 面板 selected 语义:Down 连按后钳位到 count-1(与 move_selection 一致)
        let eff = selected.min(count.saturating_sub(1));
        press_down(&mut panel, &mut state, eff);

        // 高度 ≥ 10 保证 inner ≥ 8(降级两段路径列表区即渲染;≥ 22 走全量四段)
        let area = Rect::new(0, 0, WIDTH, area_height);
        let mut buf = Buffer::empty(area);
        panel.render(&state, area, &mut buf);

        if count > 0 {
            let marker = format!("osa-{eff:03}");
            prop_assert!(
                buffer_text(&buf).contains(&marker),
                "osa_sparse: count={count} selected={eff} h={area_height} \
                 渲染缓冲不含选中行标记 \"{marker}\"(F-11 型上缓冲推挤回归?)"
            );
        }
    }

    // ============================================================
    // ClvVector:Top 维度表(行渲染 `dim={:<3}`,维度号取 100+i 三位等宽)
    // ============================================================

    #[test]
    fn clv_vector_windowed_render_contains_selected_row(
        count in 0..=48usize,
        selected in 0..=48usize,
        area_height in 20u16..=60,
    ) {
        let mut state = TuiState::new();
        state.clv_summary = Some(event_bus::ClvSummary {
            block_means: vec![0.1; 8],
            l2_norm: 2.5,
            top_dims: (0..count).map(|i| (100 + i, 0.5_f32)).collect(),
        });
        let mut panel = ClvVectorPanel::new();

        let eff = selected.min(count.saturating_sub(1));
        press_down(&mut panel, &mut state, eff);

        // 高度 ≥ 20 保证 inner ≥ 18:3(Gauge)+9(热图)+Min(4) 布局下
        // Top 维度表至少获得 6 行,窗口算术在既定布局假设内受检
        let area = Rect::new(0, 0, WIDTH, area_height);
        let mut buf = Buffer::empty(area);
        panel.render(&state, area, &mut buf);

        if count > 0 {
            // 三位等宽维度号保证 `dim=1XY` 标记互不为子串(contains 无假阳性)
            let marker = format!("dim={}", 100 + eff);
            prop_assert!(
                buffer_text(&buf).contains(&marker),
                "clv_vector: count={count} selected={eff} h={area_height} \
                 渲染缓冲不含选中行标记 \"{marker}\"(F-11 型上缓冲推挤回归?)"
            );
        }
    }

    // ============================================================
    // Log:已过滤事件列表(批次一已收口,回归锁定;行渲染含 [source])
    // ============================================================

    #[test]
    fn log_windowed_render_contains_selected_row(
        count in 0..=48usize,
        selected in 0..=48usize,
        area_height in 10u16..=60,
    ) {
        let mut state = TuiState::new();
        state.latest_events = Arc::new(vote_cast_events("lg", count));
        let mut panel = LogPanel::new();

        let eff = selected.min(count.saturating_sub(1));
        press_down(&mut panel, &mut state, eff);

        // 高度 ≥ 10 保证 content_height = inner-5 ≥ 3,窗口/回填/提示行算术受检
        let area = Rect::new(0, 0, WIDTH, area_height);
        let mut buf = Buffer::empty(area);
        panel.render(&state, area, &mut buf);

        if count > 0 {
            // Log 列表为最新在前(rev):filtered[eff] = latest_events[count-1-eff]
            let marker = format!("lg-{:03}", count - 1 - eff);
            prop_assert!(
                buffer_text(&buf).contains(&marker),
                "log: count={count} selected={eff} h={area_height} \
                 渲染缓冲不含选中行标记 \"{marker}\"(F-11 回归?)"
            );
        }
    }

    // ============================================================
    // EventStream:事件流(批次二已收口,回归锁定;行渲染含 [source])
    // ============================================================

    #[test]
    fn event_stream_windowed_render_contains_selected_row(
        count in 0..=48usize,
        selected in 0..=48usize,
        area_height in 10u16..=60,
    ) {
        let mut state = TuiState::new();
        // WHY 关 auto_scroll:默认 true 时 render 自动跟随到末项,
        // 任意 selected 的可见性属性退化为恒真(只剩末项一种选中态)
        state.auto_scroll = false;
        state.latest_events = Arc::new(vote_cast_events("es", count));
        let mut panel = EventStreamPanel::new();

        let eff = selected.min(count.saturating_sub(1));
        press_down(&mut panel, &mut state, eff);

        // 高度 ≥ 10 保证 content_height = inner-6 ≥ 2,三条件行最坏组合不溢出
        let area = Rect::new(0, 0, WIDTH, area_height);
        let mut buf = Buffer::empty(area);
        panel.render(&state, area, &mut buf);

        if count > 0 {
            // EventStream 过滤索引为正序:filtered[eff] = latest_events[eff]
            let marker = format!("es-{eff:03}");
            prop_assert!(
                buffer_text(&buf).contains(&marker),
                "event_stream: count={count} selected={eff} h={area_height} \
                 渲染缓冲不含选中行标记 \"{marker}\"(F-11 回归?)"
            );
        }
    }

    // ============================================================
    // Parliament:审议事件列表(批次一已收口,回归锁定;行渲染含 proposal_id)
    // ============================================================

    #[test]
    fn parliament_windowed_render_contains_selected_row(
        count in 0..=48usize,
        selected in 0..=48usize,
        area_height in 10u16..=60,
    ) {
        let mut state = TuiState::new();
        state.latest_events = Arc::new(vote_cast_events("vp", count));
        let mut panel = ParliamentPanel::new();

        let eff = selected.min(count.saturating_sub(1));
        press_down(&mut panel, &mut state, eff);

        // 高度 ≥ 10 保证 content_height = inner-6 ≥ 2(头部 2 行 + chrome 4 行之外仍有列表行)
        let area = Rect::new(0, 0, WIDTH, area_height);
        let mut buf = Buffer::empty(area);
        panel.render(&state, area, &mut buf);

        if count > 0 {
            // parliament_events 为最新在前(rev):filtered[eff] = latest_events[count-1-eff]
            let marker = format!("vp-{:03}", count - 1 - eff);
            prop_assert!(
                buffer_text(&buf).contains(&marker),
                "parliament: count={count} selected={eff} h={area_height} \
                 渲染缓冲不含选中行标记 \"{marker}\"(F-11 回归?)"
            );
        }
    }

    // ============================================================
    // 退化高度 + 数据收缩:窗口算术不 panic(B3-R)
    // ============================================================
    //
    // 既有五属性的 area_height ≥ 10/20 域刻意绕开了极小高度带:该带
    // visible_rows==0,选中行本就无从可见,可见性断言不适用;但正是该带
    // (log content_height=inner-5≤0 / event_stream·parliament inner-6≤0 /
    // clv·timeline 列段高度 ≤1 / osa_sparse inner<8 早退)使 adjust_scroll
    // 早退、滞留 offset 直通窗口回填算术。本属性以真实用户路径
    // (scroll_to_bottom)钉住贴底态,数据收缩后全带高度渲染,锁定"不 panic"。
    // 六面板全覆盖(osa_sparse 该带虽不可达,早退路径同受检)。

    #[test]
    fn degenerate_height_after_shrink_render_no_panic(
        initial_count in 1..=48usize,
        shrunk_count in 0..=8usize,
        tiny_height in 0u16..=12u16,
    ) {
        let mut state = TuiState::new();
        render_tiny_after_shrink(
            &mut OsaSparsePanel::new(),
            &mut state,
            |s, n| s.osa_context_mask = (0..n).map(|i| format!("osa-{i:03}")).collect(),
            initial_count,
            shrunk_count,
            tiny_height,
        );

        let mut state = TuiState::new();
        render_tiny_after_shrink(
            &mut ClvVectorPanel::new(),
            &mut state,
            |s, n| {
                s.clv_summary = Some(event_bus::ClvSummary {
                    block_means: vec![0.1; 8],
                    l2_norm: 2.5,
                    top_dims: (0..n).map(|i| (100 + i, 0.5_f32)).collect(),
                });
            },
            initial_count,
            shrunk_count,
            tiny_height,
        );

        let mut state = TuiState::new();
        render_tiny_after_shrink(
            &mut LogPanel::new(),
            &mut state,
            |s, n| s.latest_events = Arc::new(vote_cast_events("lg", n)),
            initial_count,
            shrunk_count,
            tiny_height,
        );

        let mut state = TuiState::new();
        render_tiny_after_shrink(
            &mut EventStreamPanel::new(),
            &mut state,
            |s, n| s.latest_events = Arc::new(vote_cast_events("es", n)),
            initial_count,
            shrunk_count,
            tiny_height,
        );

        let mut state = TuiState::new();
        render_tiny_after_shrink(
            &mut ParliamentPanel::new(),
            &mut state,
            |s, n| s.latest_events = Arc::new(vote_cast_events("vp", n)),
            initial_count,
            shrunk_count,
            tiny_height,
        );

        let mut state = TuiState::new();
        render_tiny_after_shrink(
            &mut TimelinePanel::new(),
            &mut state,
            |s, n| s.timeline_snapshots = (0..n).map(|_| TimelineSnapshot::default()).collect(),
            initial_count,
            shrunk_count,
            tiny_height,
        );
    }

    // ============================================================
    // scroll_offset_window 纯函数不变量(B4-W)
    // ============================================================
    //
    // 六面板内联窗口算术(B4-W 提取为 render.rs scroll_offset_window)的
    // 函数级守护:上方五面板可见性属性经真实渲染路径间接覆盖同一算术,此处
    // 直接锁定其收敛域与贴底语义。域取与既有属性同量级的有界区间而非全域
    // usize:全域下"贴底"(end==total)分支命中概率趋零,断言将退化为近恒真;
    // 有界域内空表/越界/贴底/中部四态均高频受检(实现端以 saturating_add
    // 对全域输入免 panic,见 helper 注释)。

    #[test]
    fn scroll_offset_window_pure_function_invariants(
        total in 0..=64usize,
        scroll_offset in 0..=96usize,
        visible_rows in 0..=32usize,
    ) {
        let (start, end) = scroll_offset_window(total, scroll_offset, visible_rows);
        // 半开区间收敛:六面板以 start..end 切窗口,start <= end <= total
        // 是不越界、不下溢的前提
        prop_assert!(
            start <= end,
            "start={start} > end={end} \
             (total={total} offset={scroll_offset} visible={visible_rows})"
        );
        prop_assert!(
            end <= total,
            "end={end} > total={total} \
             (offset={scroll_offset} visible={visible_rows})"
        );
        // 贴底(窗口末端贴住列表末端)时窗口填满:起点回退到
        // total-visible_rows 后窗口长 = min(visible_rows, total)
        // (total < visible_rows 时整表返回,窗口已最大)。
        // WHY prop_assert! 而非 prop_assert_eq!:后者消息串经 concat! 展开,
        // 不支持内联变量捕获(proptest 宏约束)。
        if end == total {
            let window_len = end - start;
            let expected = visible_rows.min(total);
            prop_assert!(
                window_len == expected,
                "贴底窗口未填满: len={window_len} expected={expected} \
                 (total={total} offset={scroll_offset} visible={visible_rows})"
            );
        }
    }
}
