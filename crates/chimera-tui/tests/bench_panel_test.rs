//! BenchPanel 集成测试 — F-59 专项测试补齐(R5)
//!
//! 覆盖: 面板注册可达(REGISTERED_FOCUS_ORDER)/ next/prev 往返 / i18n 标题键 /
//! 未接线诚实展示(面板级 + app 级)/ with_provider 渲染(面板级端到端)/
//! app 级渲染(边框标题 + tab 栏)/ trait 对象分派。范式: experience_card_viz_panel_test.rs。
//!
//! # WHY 专项文件(F-59)
//! Bench/ReviewQueue/Blackboard 三新面板此前在 tests/ 无任何专项文件(仅内联单测),
//! R4「27 面板均有专属测试文件」基线失真。
//!
//! # 范围注记(F-60 关联)
//! app 级 provider 注入(组合根接线)属 F-60 Wave-3 待办,本文件 with_provider
//! 端到端为面板级真实渲染路径(Panel::render → content() → provider);
//! app 级渲染走 TuiApp 内部 `BenchPanel::new()`(app/mod.rs `::new()` 装配点),
//! 恰好锁定「未接线诚实空态」这一设计内状态。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code
#![forbid(unsafe_code)]

mod common;

use common::mock::MockDataSource;
use std::sync::Arc;

use chimera_tui::panels::{BenchPanel, BenchStats, BenchStatsProvider, Panel};
use chimera_tui::types::{PanelId, TuiState, ViewMode};
use chimera_tui::{TuiApp, TuiConfig};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::Terminal;

/// Mock 提供者 — 返回固定基准快照
///
/// (与 bench.rs 内联 tests 的 MockProvider 同形,但分属不同测试二进制,
/// 各自保留为独立文件自足性;非跨文件复用。)
#[derive(Debug, Default)]
struct MockProvider {
    stats: BenchStats,
}

impl BenchStatsProvider for MockProvider {
    fn bench_stats(&self) -> BenchStats {
        self.stats
    }
}

// ----------------------------------------------------------
// 面板注册闭环(D-5 红线)
// ----------------------------------------------------------

#[test]
fn panel_registered_in_focus_order() {
    assert!(
        PanelId::REGISTERED_FOCUS_ORDER.contains(&PanelId::Bench),
        "Bench 应注册进焦点环"
    );
}

#[test]
fn panel_next_prev_roundtrip() {
    let panel = PanelId::Bench;
    assert_eq!(panel.next().prev(), panel);
    assert_eq!(panel.prev().next(), panel);
}

#[test]
fn panel_type_name_and_title() {
    // PanelId::title() 为静态 ASCII 表(types.rs),无需 pin locale
    assert_eq!(PanelId::Bench.as_str(), "Bench");
    assert_eq!(PanelId::Bench.title(), " Bench ");
}

#[test]
fn i18n_title_key_exists() {
    // 中英双语标题键存在(i18n_completeness 红线)
    assert!(
        !chimera_tui::i18n::tr("panel.border.bench").is_empty(),
        "panel.border.bench 键应存在"
    );
}

// ----------------------------------------------------------
// 渲染闭环
// ----------------------------------------------------------

#[test]
fn panel_id_and_honest_display_without_provider() {
    let mut panel = BenchPanel::new();
    assert_eq!(panel.id(), PanelId::Bench);
    // 未接线诚实展示(虚假数据治理红线:不渲染虚假基准数字)。
    // WHY 双语 OR 而非 pin:本测试不写全局 locale,避免与并行测试的 locale
    // 写点产生窗口竞态(log_panel_test i18n 纪律同源)。
    let area = Rect::new(0, 0, 60, 12);
    let mut buf = Buffer::empty(area);
    panel.render(&TuiState::new(), area, &mut buf);
    let content: String = buf
        .content()
        .iter()
        .map(|c| c.symbol().chars().next().unwrap_or(' '))
        .collect();
    assert!(
        content.contains("未接线") || content.contains("Awaiting bench provider"),
        "无提供者应诚实展示等待提示, got: {content}"
    );
}

#[test]
fn with_provider_renders_stats_end_to_end() {
    // 面板级端到端:with_provider 注入 → Panel::render 真实渲染路径(Buffer)
    // → provider.bench_stats() → 格式化行逐项落进缓冲。
    // WHY pin En:行前缀文案走 t!(),En 值为 ASCII,逐行断言可靠且可读。
    let _locale_guard = chimera_tui::i18n::locale_test_guard();
    chimera_tui::set_locale(chimera_tui::Locale::En);

    let mut panel = BenchPanel::with_provider(Arc::new(MockProvider {
        stats: BenchStats {
            total_benches: 117,
            slo_thresholds_total: 46,
            slo_breaches: Some(2),
            retrieval_p99_ms: Some(2.11),
            agreement_rate: Some(0.93),
            certifiable_ratio: Some(0.88),
        },
    }));
    let area = Rect::new(0, 0, 60, 12);
    let mut buf = Buffer::empty(area);
    panel.render(&TuiState::new(), area, &mut buf);
    let content: String = buf
        .content()
        .iter()
        .map(|c| c.symbol().chars().next().unwrap_or(' '))
        .collect();

    // 边框标题走 panel.border.bench 键(En " Bench ")
    assert!(
        content.contains(" Bench "),
        "边框标题应渲染, got: {content}"
    );
    // 行级断言(前缀+数值整行匹配,非裸字符近恒真)
    assert!(
        content.contains("Benches registered: 117"),
        "应渲染基准总数行, got: {content}"
    );
    assert!(
        content.contains("SLO thresholds total: 46"),
        "应渲染阈值总数行, got: {content}"
    );
    assert!(
        content.contains("Retrieval P99 (ms): 2.11"),
        "应渲染检索 P99 行, got: {content}"
    );
    assert!(
        content.contains("93.0%") && content.contains("88.0%"),
        "应渲染一致率/可认证比例(统一助手百分位), got: {content}"
    );
    // 有提供者时不得再显示等待提示(诚实状态互斥)
    assert!(
        !content.contains("Awaiting bench provider"),
        "接线后不得残留未接线提示, got: {content}"
    );
}

#[test]
fn app_level_renders_border_title_and_tab_bar() {
    // app 级渲染(F-59 ①):Dashboard 模式下边框标题 + tab 栏可见。
    // app 内部 BenchPanel 为 ::new() 装配(未接线)→ 同帧应见诚实空态提示。
    // WHY pin En:全帧含多面板 chrome,En 值 ASCII 逐串可靠(log_panel_test 先例)。
    let _locale_guard = chimera_tui::i18n::locale_test_guard();
    chimera_tui::set_locale(chimera_tui::Locale::En);

    let ds = MockDataSource::new(chimera_tui::data::DataSnapshot::default());
    let mut app = TuiApp::with_data_source(
        TuiConfig {
            default_view_mode: ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
        Box::new(ds),
    )
    .expect("with_data_source should succeed");
    app.switch_panel_to(PanelId::Bench);
    app.update();

    let backend = ratatui::backend::TestBackend::new(100, 30);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal.draw(|f| app.render(f)).expect("draw");
    let content: String = terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol().chars().next().unwrap_or(' '))
        .collect();

    // 边框标题走 panel.border.bench 键
    assert!(
        content.contains(" Bench "),
        "聚焦面板边框标题应渲染, got: {content}"
    );
    // tab 栏可见:Tabs 块标题 " Panels " + 首个 tab 名(as_str 渲染,render_tabs)
    assert!(content.contains("Panels"), "tab 栏应渲染, got: {content}");
    assert!(
        content.contains("Quest"),
        "tab 栏应含面板名, got: {content}"
    );
    // app 装配点为 ::new()(F-60 Wave-3 待接线)→ 诚实空态同帧可见
    assert!(
        content.contains("Awaiting bench provider"),
        "未接线应诚实展示等待提示(设计内状态), got: {content}"
    );
}

#[test]
fn panel_trait_object_dispatch() {
    // Box<dyn Panel> 动态分派验证(工厂 make_panel 同款路径)
    let panel: Box<dyn Panel> = Box::new(BenchPanel::new());
    assert_eq!(panel.id(), PanelId::Bench);
    assert!(!panel.shortcuts().is_empty());
}
