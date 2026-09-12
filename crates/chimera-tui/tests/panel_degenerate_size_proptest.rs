//! PS-2 U-4:面板退化尺寸守卫 —— 属性测试(proptest)
//!
//! # 目的(评估报告 U-4)
//! 此前多数面板**没有尺寸守卫**:小终端下内容被切成碎片,用户看不出原因。
//! 本文件以**属性测试**证明两件事:
//! 1. **健壮性**:任意小尺寸(1..48 列 × 1..16 行)下渲染**全部注册面板**
//!    都不会 panic(退化区不得触发算术下溢/索引越界等);
//! 2. **一致性**:退化区(`crate::panels::degenerate`)渲染统一提示,
//!    而非内容碎片。
//!
//! # WHY 用属性测试而非枚举样例
//! 尺寸组合是二维空间且边界不连续(宽/高各自独立触发退化),枚举样例易漏;
//! proptest 覆盖随机组合,把"某个尺寸下崩"这类缺陷从偶发变为必现。

#![forbid(unsafe_code)]

use chimera_tui::panels::{degenerate, MIN_PANEL_H, MIN_PANEL_W};
use chimera_tui::{
    DataSnapshot, DataSourceConfig, PanelId, TuiApp, TuiConfig, TuiDataSource, TuiError,
};
use proptest::prelude::*;
use ratatui::backend::TestBackend;
use ratatui::layout::Rect;
use ratatui::Terminal;
use std::sync::Arc;

/// 测试数据源 — 返回空快照(面板走各自的"无数据"降级路径)
#[derive(Debug)]
struct DegenerateSizeTestSource {
    snapshot: DataSnapshot,
    config: DataSourceConfig,
}

impl DegenerateSizeTestSource {
    fn new() -> Self {
        Self {
            snapshot: DataSnapshot::default(),
            config: DataSourceConfig::default(),
        }
    }
}

impl TuiDataSource for DegenerateSizeTestSource {
    fn snapshot(&self) -> Result<Arc<DataSnapshot>, TuiError> {
        Ok(Arc::new(self.snapshot.clone()))
    }

    fn config(&self) -> &DataSourceConfig {
        &self.config
    }
}

/// 构造测试用 TuiApp(persist_state=false:不落盘)
fn make_app() -> TuiApp {
    TuiApp::with_data_source(
        TuiConfig {
            persist_state: false,
            ..Default::default()
        },
        Box::new(DegenerateSizeTestSource::new()),
    )
    .expect("with_data_source should succeed")
}

/// 在指定尺寸下渲染整机,返回缓冲区文本
///
/// WHY 整机渲染而非单面板:同时覆盖布局层(Layout/Constraint)与面板层,
/// 退化尺寸下两者都可能出问题;整机渲染是真实调用路径。
fn render_to_string(app: &mut TuiApp, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("terminal");
    terminal.draw(|f| app.render(f)).expect("draw");
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol().chars().next().unwrap_or(' '))
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// 任意小尺寸 × 任意面板:渲染不得 panic
    ///
    /// 尺寸范围刻意压在退化区附近(1..48 列 / 1..16 行),使宽、高各自
    /// 跨越 `MIN_PANEL_W` / `MIN_PANEL_H` 边界;上界略高于阈值,
    /// 兼顾"恰好非退化"的一侧。
    #[test]
    fn rendering_any_panel_at_any_small_size_never_panics(
        w in 1u16..48u16,
        h in 1u16..16u16,
    ) {
        let mut app = make_app();
        app.update();
        for id in PanelId::REGISTERED_FOCUS_ORDER {
            app.switch_panel_to(*id);
            // 不 panic 即通过;返回值(文本)此处不校验内容
            let _ = render_to_string(&mut app, w, h);
        }
    }
}

// ============================================================
// 判据与提示的确定性断言(与上面的属性测试互补)
// ============================================================

#[test]
fn degenerate_threshold_boundaries_are_exact() {
    // 宽/高各自独立触发退化:任一低于阈值即为退化
    assert!(degenerate(Rect::new(0, 0, MIN_PANEL_W - 1, MIN_PANEL_H)));
    assert!(degenerate(Rect::new(0, 0, MIN_PANEL_W, MIN_PANEL_H - 1)));
    // 恰好达到阈值:非退化
    assert!(!degenerate(Rect::new(0, 0, MIN_PANEL_W, MIN_PANEL_H)));
    // 常规尺寸:非退化
    assert!(!degenerate(Rect::new(0, 0, 200, 50)));
}

/// 压缩文本:去除所有空白
///
/// WHY 需要:TestBackend 的缓冲对**宽字符(CJK)会占两个 cell** —— 宽字符本身
/// 占一个 cell,其后的续格是空白。直接 `map(|c| c.symbol())` 得到的字符串会
/// 呈现为 "面 板 区 域"(字符间夹空格),导致 `contains("面板区域")` 误判为不命中。
/// 断言前压缩空白即可稳定匹配(纯 ASCII 文案不受影响)。
fn compact_text(content: &str) -> String {
    content.chars().filter(|c| !c.is_whitespace()).collect()
}

#[test]
fn degenerate_area_renders_unified_hint() {
    // WHY 直接渲染单个面板(而非整机):整机渲染时面板拿到的是布局切分后的子区域,
    // 极小终端下该子区域可能高度为 0,提示会被裁掉 —— 那是布局层的正常裁剪,
    // 不是面板守卫的契约。本用例隔离验证**面板层**契约:退化区必须早退并提示。
    use chimera_tui::panels::{Panel, QuestPanel};

    let state = chimera_tui::TuiState::new();
    let area = Rect::new(0, 0, MIN_PANEL_W - 1, MIN_PANEL_H - 1);
    let mut buf = ratatui::buffer::Buffer::empty(area);
    let mut panel = QuestPanel::new();
    panel.render(&state, area, &mut buf);
    let content: String = buf.content().iter().map(|c| c.symbol()).collect();
    let compact = compact_text(&content);
    assert!(
        compact.contains("面板区域过小") || compact.contains("Panelareatoosmall"),
        "退化区应渲染统一提示(panel.too_small),实际: {content:?}"
    );
}

#[test]
fn normal_size_does_not_render_hint() {
    let mut app = make_app();
    app.update();
    app.switch_panel_to(PanelId::Quest);
    let content = render_to_string(&mut app, 100, 30);
    // WHY 必须用 compact_text:否则宽字符续格会让 contains 永远为假,
    // 使"不应出现提示"这一断言退化为恒真(空断言)。
    let compact = compact_text(&content);
    assert!(
        !compact.contains("面板区域过小") && !compact.contains("Panelareatoosmall"),
        "常规尺寸不应出现退化提示,实际: {content:?}"
    );
    // 反向哨兵:常规尺寸下确实渲染了面板内容(证明上一条不是"整屏空白"导致的恒真)
    assert!(!compact.is_empty(), "常规尺寸下应有渲染内容,实际为空屏");
}
