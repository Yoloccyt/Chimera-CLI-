//! EventStream 面板集成测试 — P2.2 TUI v1.7-omega
//!
//! 验证 EventStreamPanel 正确渲染事件流、支持万级事件虚拟滚动(帧时间 < 16ms)、
//! auto_scroll 自动跟随新事件、用户导航时关闭 auto_scroll。
//!
//! # 设计决策(WHY)
//! - 使用 `Buffer::empty` + `Rect::new` 直接调用 `Panel::render`,
//!   无需 TestBackend/Terminal,测试更轻量且可测量纯渲染时间。
//! - 帧时间断言 < 16ms(60fps):虚拟滚动只渲染可见区域 + 上下 5 行缓冲,
//!   10000 事件的实际渲染行数约 50 行,远低于全量渲染。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use chimera_tui::{EventStreamPanel, Panel, PopupKind, TuiCommand, TuiState};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use event_bus::{EventMetadata, NexusEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use std::collections::VecDeque;
use std::time::Instant;

/// 构造带预设事件的 TuiState
fn make_state_with_events(events: Vec<NexusEvent>) -> TuiState {
    let mut state = TuiState::new();
    state.latest_events = std::sync::Arc::new(VecDeque::from(events));
    state
}

/// 构造一个 CacheHit 事件(用于批量生成测试数据)
fn cache_hit_event(id: usize) -> NexusEvent {
    NexusEvent::CacheHit {
        metadata: EventMetadata::new("scc-cache"),
        cache_key: format!("k{id}"),
    }
}

/// 标准 120x40 渲染区域(与 TUI 默认窗口接近)
fn standard_area() -> Rect {
    Rect::new(0, 0, 120, 40)
}

// ============================================================
// 测试 1:EventStream 面板渲染 latest_events 中的事件
// ============================================================

#[test]
fn event_stream_panel_renders_events() {
    let state = make_state_with_events(vec![
        NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k1".into(),
        },
        NexusEvent::BudgetExceeded {
            metadata: EventMetadata::new("decb-governor"),
            budget_type: "token".into(),
            current: 9500,
            limit: 10000,
        },
    ]);

    let content = EventStreamPanel::content(&state, 0).to_string();
    assert!(
        content.contains("CacheHit"),
        "content should contain event type name 'CacheHit', got: {content}"
    );
    assert!(
        content.contains("BudgetExceeded"),
        "content should contain event type name 'BudgetExceeded', got: {content}"
    );
}

// ============================================================
// 测试 2:虚拟滚动 — 10000 条事件不 panic 且帧时间 < 16ms
// ============================================================

#[test]
fn event_stream_panel_virtual_scroll_10000_events_no_panic() {
    // 构造 10000 条事件,验证虚拟滚动不 panic 且帧时间 < 16ms
    let events: Vec<NexusEvent> = (0..10000).map(cache_hit_event).collect();
    let state = make_state_with_events(events);

    let mut panel = EventStreamPanel::new();
    let area = standard_area();
    let mut buf = Buffer::empty(area);

    // 测量纯渲染时间(不含 Buffer::empty 的初始化开销)
    let start = Instant::now();
    panel.render(&state, area, &mut buf);
    let elapsed = start.elapsed();

    // 60fps 对应 16.67ms,断言 < 16ms(虚拟滚动只渲染 ~50 行,应远低于此)
    assert!(
        elapsed.as_millis() < 16,
        "render 10000 events took {:?}, expected < 16ms (virtual scroll should only render visible rows)",
        elapsed
    );
}

// ============================================================
// 测试 3:auto_scroll=true 时,新事件到达后选中项跟随到最新
// ============================================================

#[test]
fn event_stream_panel_auto_scroll_follows_new_events() {
    let mut state = make_state_with_events(vec![
        NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k1".into(),
        },
        NexusEvent::CacheMiss {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k2".into(),
        },
    ]);
    state.auto_scroll = true;

    let mut panel = EventStreamPanel::new();
    let area = standard_area();

    // 第一次渲染:auto_scroll=true,selected 应跟随到最后一条事件(索引 1)
    let mut buf = Buffer::empty(area);
    panel.render(&state, area, &mut buf);
    assert_eq!(
        panel.selected(),
        1,
        "auto_scroll=true should follow to last event (index 1)"
    );

    // 新事件到达(Arc 化后经 make_mut COW 注入)
    std::sync::Arc::make_mut(&mut state.latest_events).push_back(NexusEvent::CacheHit {
        metadata: EventMetadata::new("scc-cache"),
        cache_key: "k3".into(),
    });

    // 第二次渲染:auto_scroll 仍为 true,selected 应跟随到最新事件(索引 2)
    let mut buf2 = Buffer::empty(area);
    panel.render(&state, area, &mut buf2);
    assert_eq!(
        panel.selected(),
        2,
        "auto_scroll=true should follow new event (index 2)"
    );
}

// ============================================================
// 测试 4:用户按 Up/Down 时 auto_scroll 设为 false
// ============================================================

#[test]
fn event_stream_panel_key_navigation_disables_auto_scroll() {
    let mut state = make_state_with_events(vec![
        NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k1".into(),
        },
        NexusEvent::CacheMiss {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k2".into(),
        },
    ]);
    state.auto_scroll = true;

    let mut panel = EventStreamPanel::new();

    // 用户按 Down,应关闭 auto_scroll(用户接管导航)
    panel.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &mut state);
    assert!(
        !state.auto_scroll,
        "Down key should disable auto_scroll (user takes over navigation)"
    );

    // 重新启用 auto_scroll,验证 Up 也会关闭
    state.auto_scroll = true;
    panel.handle_key(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE), &mut state);
    assert!(
        !state.auto_scroll,
        "Up key should disable auto_scroll (user takes over navigation)"
    );
}

// ============================================================
// 测试 5:Enter 打开详情弹窗(复用 Log 面板模式)
// ============================================================

#[test]
fn event_stream_panel_enter_opens_detail_popup() {
    let mut panel = EventStreamPanel::new();
    let mut state = make_state_with_events(vec![NexusEvent::CacheHit {
        metadata: EventMetadata::new("scc-cache"),
        cache_key: "k1".into(),
    }]);

    let cmd = panel.handle_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut state,
    );

    match cmd {
        Some(TuiCommand::OpenPopup(PopupKind::EventDetail {
            title,
            payload_decoded,
            ..
        })) => {
            assert!(
                title.contains("CacheHit"),
                "detail popup title should contain event type name"
            );
            assert!(
                payload_decoded.contains("scc-cache"),
                "detail popup content should contain event source"
            );
        }
        _ => panic!("expected EventDetail popup command, got {cmd:?}"),
    }
}

// ============================================================
// 测试 6:filter_topic / filter_level 过滤生效
// ============================================================

#[test]
fn event_stream_panel_filter_by_topic() {
    let mut state = make_state_with_events(vec![
        NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k1".into(),
        },
        NexusEvent::SkepticVeto {
            metadata: EventMetadata::new("parliament"),
            quest_id: "q1".into(),
            veto_reason: "unsafe".into(),
            frozen_capabilities: vec![],
        },
    ]);
    state.filter_topic = Some("security".into());

    let filtered = EventStreamPanel::filtered_events(&state);
    assert_eq!(
        filtered.len(),
        1,
        "security topic should filter to SkepticVeto only"
    );
    assert_eq!(filtered[0].type_name(), "SkepticVeto");
}

#[test]
fn event_stream_panel_filter_by_level_critical() {
    let mut state = make_state_with_events(vec![
        NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k1".into(),
        },
        NexusEvent::BudgetExceeded {
            metadata: EventMetadata::new("decb-governor"),
            budget_type: "token".into(),
            current: 9500,
            limit: 10000,
        },
    ]);
    state.filter_level = Some("critical".into());

    let filtered = EventStreamPanel::filtered_events(&state);
    assert_eq!(
        filtered.len(),
        1,
        "critical level should filter to BudgetExceeded only"
    );
    assert_eq!(filtered[0].type_name(), "BudgetExceeded");
}

// ============================================================
// 测试 14:US-02 zh locale 渲染断言 — 空状态提示为中文
// ============================================================

#[test]
fn event_stream_panel_zh_locale_renders_chinese_copy() {
    // US-02 i18n 收口:空状态 "[INFO]  No events" 迁移键表后,
    // Zh locale 应渲染中文提示(TDD RED→GREEN:迁移前此处输出英文必红)。
    let _locale_guard = chimera_tui::i18n::locale_test_guard();
    chimera_tui::set_locale(chimera_tui::Locale::Zh);
    let state = TuiState::new();
    let content = EventStreamPanel::content(&state, 0).to_string();
    assert!(
        content.contains("暂无事件"),
        "Zh locale 下空状态应显示中文提示,实际: {}",
        &content[..content.len().min(200)]
    );
}

// ============================================================
// 测试 7:空状态显示 "No events"
// ============================================================

#[test]
fn event_stream_panel_empty_state_shows_no_events() {
    let state = TuiState::new(); // latest_events 为空
    let content = EventStreamPanel::content(&state, 0).to_string();
    assert!(
        content.contains("No events") || content.contains("no events"),
        "empty state should show 'No events', got: {}",
        &content[..content.len().min(200)]
    );
}

// ============================================================
// 测试 8:关键字过滤
// ============================================================

#[test]
fn event_stream_panel_filter_by_keyword() {
    let mut state = make_state_with_events(vec![
        NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "alpha-key".into(),
        },
        NexusEvent::CacheMiss {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "beta-key".into(),
        },
    ]);
    state.filter_keyword = Some("alpha".into());

    let filtered = EventStreamPanel::filtered_events(&state);
    assert_eq!(filtered.len(), 1, "keyword filter should narrow to 1 event");
    assert_eq!(filtered[0].type_name(), "CacheHit");
}

// ============================================================
// 测试 9:Shift+G 跳到底部并恢复 auto_scroll
// ============================================================

#[test]
fn event_stream_panel_scroll_to_bottom_restores_auto_scroll() {
    // WHY 直调 scroll_to_bottom 而非 handle_key(Shift+G):键位治理后面板 g/G
    // arm 已移除,生产路径经 InputRouter → RouteTarget::ScrollBottom 走同一方法。
    let events: Vec<NexusEvent> = (0..10).map(cache_hit_event).collect();
    let mut state = make_state_with_events(events);
    state.auto_scroll = false; // 模拟用户之前手动滚动过

    let mut panel = EventStreamPanel::new();
    // 先按 Down 让 selected != 0
    panel.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &mut state);
    let selected_before = panel.selected();
    assert!(selected_before > 0, "Down should move selection");

    // 滚到底部
    panel.scroll_to_bottom(&mut state);

    let filtered = EventStreamPanel::filtered_events(&state);
    let last_idx = filtered.len().saturating_sub(1);
    assert_eq!(
        panel.selected(),
        last_idx,
        "scroll_to_bottom should select last event"
    );
    assert!(
        state.auto_scroll,
        "scroll_to_bottom should restore auto_scroll=true"
    );
}

// ============================================================
// 测试 10:auto_scroll=false 时显示 "[新事件 N 条]" 提示
// ============================================================

#[test]
fn event_stream_panel_shows_new_events_indicator_when_auto_scroll_off() {
    let events: Vec<NexusEvent> = (0..3).map(cache_hit_event).collect();
    let mut state = make_state_with_events(events);
    state.auto_scroll = false;

    let content = EventStreamPanel::content(&state, 0).to_string();
    // WHY 提示文案:参考 Claude Code 工具调用流式输出 UX
    // 当用户暂停 auto_scroll 后,新事件累积应通过提示告知操作员
    assert!(
        content.contains("新事件") || content.contains("auto-scroll"),
        "should show new events indicator when auto_scroll=false, got: {}",
        &content[..content.len().min(300)]
    );
}

// ============================================================
// 测试 11:鼠标滚轮关闭 auto_scroll
// ============================================================

#[test]
fn event_stream_panel_mouse_scroll_disables_auto_scroll() {
    use crossterm::event::{MouseEvent, MouseEventKind};

    let events: Vec<NexusEvent> = (0..5).map(cache_hit_event).collect();
    let mut state = make_state_with_events(events);
    state.auto_scroll = true;

    let mut panel = EventStreamPanel::new();
    let mouse = MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    };
    panel.handle_mouse(mouse, &mut state);

    assert!(
        !state.auto_scroll,
        "mouse scroll should disable auto_scroll"
    );
}

// ============================================================
// 测试 12:基础 Panel trait 行为
// ============================================================

#[test]
fn event_stream_panel_id_is_registered() {
    let panel = EventStreamPanel::new();
    assert_eq!(panel.id(), chimera_tui::PanelId::EventStream);
}

#[test]
fn event_stream_panel_question_mark_returns_none() {
    let mut panel = EventStreamPanel::new();
    let mut state = TuiState::new();
    let key = KeyEvent::new(KeyCode::Char('?'), KeyModifiers::NONE);
    assert_eq!(
        panel.handle_key(key, &mut state),
        None,
        "'?' should be handled globally by TuiApp as Help overlay"
    );
}

// ============================================================
// 测试 13:P-B — 仅 topic 过滤(无 keyword)+ production revision,
// 二次渲染走过滤缓存且行为不回归(过滤结果 / auto_scroll 语义一致)
// ============================================================

#[test]
fn event_stream_panel_topic_only_filter_second_render_uses_cache() {
    let mut state = make_state_with_events(vec![
        NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k1".into(),
        },
        NexusEvent::BudgetExceeded {
            metadata: EventMetadata::new("decb-governor"),
            budget_type: "token".into(),
            current: 9500,
            limit: 10000,
        },
    ]);
    state.last_snapshot_revision = 1;
    state.filter_topic = Some("memory".into());
    state.auto_scroll = true;

    let mut panel = EventStreamPanel::new();
    let area = standard_area();

    // 第一次渲染:冷路径(建缓存);第二次:命中缓存
    let mut buf1 = Buffer::empty(area);
    panel.render(&state, area, &mut buf1);
    let mut buf2 = Buffer::empty(area);
    panel.render(&state, area, &mut buf2);

    let text_of = |buf: &Buffer| {
        buf.content()
            .iter()
            .map(|c| c.symbol().chars().next().unwrap_or(' '))
            .collect::<String>()
    };
    let text1 = text_of(&buf1);
    let text2 = text_of(&buf2);

    // 过滤语义不回归:仅显示 memory 主题事件(CacheHit),隐藏 BudgetExceeded
    assert!(
        text1.contains("CacheHit") && !text1.contains("BudgetExceeded"),
        "topic=memory 过滤应只显示 CacheHit"
    );
    // 二次渲染命中缓存,输出与首次一致(auto_scroll 跟随选中项也不变)
    assert_eq!(text1, text2, "缓存命中帧的渲染输出应与冷路径一致");
    assert_eq!(panel.selected(), 0, "过滤后仅 1 条事件,selected 应为 0");
}

// ============================================================
// 测试 15:F-22 — 万级事件贴底,总数提示行与页脚不再被裁剪
// ============================================================

/// 宽字符感知的渲染收集(timeline_panel_test.rs / pvl_score_panel_test 同款口径:
/// 跳过 CJK 续格,避免"显 示"被续格空格隔断导致 contains 失配);
/// 按 buffer 列宽插 '\n'(对齐 log_panel_test render_to_string 的 join("\n")
/// 口径),使输出有行结构支撑行级锚定断言(lines().find 定位选中行)
fn render_panel_to_string(
    panel: &mut EventStreamPanel,
    state: &TuiState,
    w: u16,
    h: u16,
) -> String {
    let area = Rect::new(0, 0, w, h);
    let mut buf = Buffer::empty(area);
    panel.render(state, area, &mut buf);
    let mut out = String::new();
    let mut prev_wide = false;
    for (i, cell) in buf.content().iter().enumerate() {
        if i > 0 && i % w as usize == 0 {
            out.push('\n');
        }
        if cell.skip {
            continue;
        }
        let s = cell.symbol();
        if prev_wide {
            prev_wide = false;
            continue;
        }
        if s.is_empty() {
            continue;
        }
        let ch = s.chars().next().unwrap_or(' ');
        prev_wide = unicode_width::UnicodeWidthStr::width(s) >= 2;
        out.push(ch);
    }
    out
}

#[test]
fn event_stream_panel_showing_and_footer_visible_at_bottom_with_10000_events() {
    // WHY pin Zh:任务书要求断言 zh "显示" 提示行文案;配合宽字符感知收集。
    // guard 防并行测试污染全局 locale。
    let _locale_guard = chimera_tui::i18n::locale_test_guard();
    chimera_tui::set_locale(chimera_tui::Locale::Zh);

    // 万级事件 + auto_scroll 默认 true:render 时 selected 自动跟随到末条
    // (贴底主路径),与 R5 F-22 复算场景(120x40 / 10000 事件)同口径。
    let events: Vec<NexusEvent> = (0..10000).map(cache_hit_event).collect();
    let state = make_state_with_events(events);

    let mut panel = EventStreamPanel::new();
    let content = render_panel_to_string(&mut panel, &state, 120, 40);

    assert!(
        content.contains("CacheHit"),
        "事件行应渲染, got: {}",
        &content[..content.len().min(400)]
    );
    // 修复前复算(R5):窗口 38 事件行 + 标题 2 + 提示 1 + 页脚 1 = 42 行
    // > inner 38 行,提示行(第 41)与页脚(第 42)被 Paragraph 系统性裁剪。
    assert!(
        content.contains("显示"),
        "总数提示行应渲染在视口内(F-22 修复前被系统性裁剪), got: {}",
        &content[..content.len().min(400)]
    );
    assert!(
        content.contains("Press Tab to switch panels"),
        "页脚应渲染在视口内(F-22 修复前被系统性裁剪), got: {}",
        &content[..content.len().min(400)]
    );
}

#[test]
fn event_stream_panel_showing_and_selected_visible_mid_scroll() {
    // WHY pin Zh:同上方贴底用例口径(断言 zh "显示" 提示行文案)。
    let _locale_guard = chimera_tui::i18n::locale_test_guard();
    chimera_tui::set_locale(chimera_tui::Locale::Zh);

    // WHY 关 auto_scroll:默认 true 时 render 自动跟随到末条(贴底),
    // Down 无法停在滚动中部;关闭后选中才可任意驻留。
    // WHY source 逐条唯一:行级锚定需选中行文本全帧唯一(cache_hit_event
    // 的 source 恒为 "scc-cache",全帧事件行同文,find 无法定位选中行)
    let events: Vec<NexusEvent> = (0..10000)
        .map(|i| NexusEvent::CacheHit {
            metadata: EventMetadata::new(format!("src-{i:04}")),
            cache_key: format!("k{i}"),
        })
        .collect();
    let mut state = make_state_with_events(events);
    state.auto_scroll = false;

    let mut panel = EventStreamPanel::new();
    // Down×60:120x40 → inner 38 → content_height 32 → selected=60 落在滚动
    // 中部(offset=29,窗口 [29,61));修复前 ±5 缓冲窗口 [24,66) 使选中行
    // 渲染在第 40 行 ≥ 视口 38 行恒不可见,本用例的选中前缀断言必红。
    for _ in 0..60 {
        panel.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &mut state);
    }
    let content = render_panel_to_string(&mut panel, &state, 120, 40);

    assert!(
        content.contains("显示"),
        "滚动中部总数提示行应渲染在视口内, got: {}",
        &content[..content.len().min(400)]
    );
    // 选中行可见(行级锚定,对齐 log_panel_test 范式):EventStream 过滤索引
    // 正序,filtered[60] = latest_events[60] = src-0060;带方括号标记全帧唯一
    let selected_line = content
        .lines()
        .find(|l| l.contains("[src-0060] CacheHit"))
        .expect("选中事件 src-0060 行应渲染在视口内");
    assert!(
        selected_line.contains("> "),
        "滚动中部选中行应带 > 前缀(与提示行同帧可见), got: {selected_line}"
    );
}
