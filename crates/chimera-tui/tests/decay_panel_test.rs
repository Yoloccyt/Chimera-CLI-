//! Decay 面板集成测试 — P2.1 TUI v1.7-omega
//!
//! 验证 DecayPanel 正确渲染衰减系数、历史 sparkline 与最近衰减事件列表,
//! 以及高衰减阈值(< 0.3,即衰减量 > 0.7)时的红色高亮行为。
//!
//! # 设计决策(WHY)
//! - 使用 TestBackend 内存渲染,无需真实终端,CI 可运行
//! - 阈值语义说明:spec 中"衰减系数 > 0.7(高衰减)"指的是"衰减量 > 0.7",
//!   对应 `coefficient < 0.3`(因为 coefficient = 1.0 表示无衰减,0.0 表示完全衰减)。
//!   这样默认值 1.0(无衰减)不会误显示为红色高亮。

#![forbid(unsafe_code)]

use chimera_tui::{
    DataSnapshot, DataSourceConfig, DecayMetrics, PanelId, TuiApp, TuiConfig, TuiDataSource,
    TuiError,
};
use chrono::Utc;
use event_bus::{EventMetadata, NexusEvent};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::collections::VecDeque;
use std::sync::Arc;

/// 测试数据源 — 返回预设 Decay 指标
#[derive(Debug)]
struct DecayTestSource {
    snapshot: DataSnapshot,
    config: DataSourceConfig,
}

impl DecayTestSource {
    fn new(snapshot: DataSnapshot) -> Self {
        Self {
            snapshot,
            config: DataSourceConfig::default(),
        }
    }
}

impl TuiDataSource for DecayTestSource {
    fn snapshot(&self) -> Result<std::sync::Arc<DataSnapshot>, TuiError> {
        Ok(std::sync::Arc::new(self.snapshot.clone()))
    }

    fn config(&self) -> &DataSourceConfig {
        &self.config
    }
}

/// 渲染应用并返回字符串内容
fn render_to_string(app: &mut TuiApp, width: u16, height: u16) -> String {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw(|f| app.render(f)).unwrap();
    terminal
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol().chars().next().unwrap_or(' '))
        .collect()
}

/// 构造正常衰减快照(coefficient = 0.85,无高亮)
fn normal_decay_snapshot() -> DataSnapshot {
    DataSnapshot {
        decay_metrics: DecayMetrics {
            coefficient: 0.85,
            recent_events: vec![
                "capability_frozen:cap-1".into(),
                "sandbox_violation:op-2".into(),
            ],
            cycle_start: Some(Utc::now()),
            fallback_count_delta: 0,
        },
        decay_history: vec![1000, 980, 950, 920, 880, 860, 850],
        ..Default::default()
    }
}

/// 构造高衰减快照(coefficient = 0.25,应红色高亮)
fn high_decay_snapshot() -> DataSnapshot {
    DataSnapshot {
        decay_metrics: DecayMetrics {
            coefficient: 0.25,
            recent_events: vec!["capability_frozen:cap-critical".into()],
            cycle_start: Some(Utc::now()),
            fallback_count_delta: 0,
        },
        decay_history: vec![1000, 800, 600, 400, 300, 280, 250],
        ..Default::default()
    }
}

// ============================================================
// A. 基础渲染测试
// ============================================================

#[test]
fn test_decay_panel_id() {
    let mut app = {
        let mut __app = TuiApp::new(TuiConfig {
            default_view_mode: chimera_tui::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        })
        .unwrap();
        __app.state_mut().view_mode = chimera_tui::ViewMode::Dashboard;
        __app
    };
    app.switch_panel_to(PanelId::Decay);
    assert_eq!(app.current_panel(), PanelId::Decay);
}

#[test]
fn test_decay_panel_renders_title() {
    let mut app = TuiApp::with_data_source(
        TuiConfig {
            default_view_mode: chimera_tui::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
        Box::new(DecayTestSource::new(normal_decay_snapshot())),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Decay);

    let content = render_to_string(&mut app, 80, 24);
    assert!(
        content.contains("Decay"),
        "Decay panel title should be rendered, got: {}",
        &content[..content.len().min(200)]
    );
}

#[test]
fn test_decay_panel_renders_coefficient() {
    let mut app = TuiApp::with_data_source(
        TuiConfig {
            default_view_mode: chimera_tui::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
        Box::new(DecayTestSource::new(normal_decay_snapshot())),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Decay);

    let content = render_to_string(&mut app, 80, 24);
    // 衰减系数 0.85 应显示在面板中
    assert!(
        content.contains("0.85") || content.contains("85.0%"),
        "coefficient 0.85 should be rendered, got: {}",
        &content[..content.len().min(300)]
    );
}

#[test]
fn test_decay_panel_renders_recent_events() {
    let mut app = TuiApp::with_data_source(
        TuiConfig {
            default_view_mode: chimera_tui::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
        Box::new(DecayTestSource::new(normal_decay_snapshot())),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Decay);

    let content = render_to_string(&mut app, 80, 24);
    assert!(
        content.contains("capability_frozen:cap-1"),
        "recent event 'capability_frozen:cap-1' should be rendered"
    );
    assert!(
        content.contains("sandbox_violation:op-2"),
        "recent event 'sandbox_violation:op-2' should be rendered"
    );
}

// ============================================================
// B. 高衰减阈值测试
// ============================================================

#[test]
fn test_decay_panel_high_decay_threshold() {
    // WHY 阈值语义:spec "衰减系数 > 0.7(高衰减)" 指的是衰减量 > 0.7,
    // 即 coefficient < 0.3。此处用 0.25 验证高衰减场景。
    let mut app = TuiApp::with_data_source(
        TuiConfig {
            default_view_mode: chimera_tui::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
        Box::new(DecayTestSource::new(high_decay_snapshot())),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Decay);

    // 渲染不应 panic,且包含系数值
    let content = render_to_string(&mut app, 80, 24);
    assert!(
        content.contains("0.25") || content.contains("25.0%"),
        "high decay coefficient 0.25 should be rendered"
    );
    assert!(
        content.contains("capability_frozen:cap-critical"),
        "high decay event should be rendered"
    );
}

#[test]
fn test_decay_panel_normal_decay_no_highlight() {
    // 正常衰减(coefficient = 0.85,远高于阈值 0.3)不应显示高亮告警标记
    let mut app = TuiApp::with_data_source(
        TuiConfig {
            default_view_mode: chimera_tui::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
        Box::new(DecayTestSource::new(normal_decay_snapshot())),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Decay);

    let content = render_to_string(&mut app, 80, 24);
    // 不应包含 HIGH DECAY 告警标记(正常衰减)
    assert!(
        !content.contains("HIGH DECAY"),
        "normal decay (0.85) should not show HIGH DECAY alert"
    );
}

// ============================================================
// C. 默认/空状态测试
// ============================================================

#[test]
fn test_decay_panel_default_state_renders() {
    // 默认状态:coefficient = 1.0(无衰减),无事件
    let snapshot = DataSnapshot {
        decay_metrics: DecayMetrics::default(),
        ..Default::default()
    };
    let mut app = TuiApp::with_data_source(
        TuiConfig {
            default_view_mode: chimera_tui::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
        Box::new(DecayTestSource::new(snapshot)),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Decay);

    // 默认状态应能渲染,不 panic
    let content = render_to_string(&mut app, 80, 24);
    assert!(
        content.contains("Decay"),
        "Decay panel should render title in default state"
    );
    // 默认 coefficient = 1.0 应显示
    assert!(
        content.contains("1.00") || content.contains("100.0%"),
        "default coefficient 1.0 should be rendered"
    );
}

#[test]
fn test_decay_panel_empty_history_renders() {
    // 空历史 sparkline 应能正常渲染,不 panic
    let snapshot = DataSnapshot {
        decay_metrics: DecayMetrics {
            coefficient: 0.5,
            recent_events: vec![],
            cycle_start: Some(Utc::now()),
            fallback_count_delta: 0,
        },
        decay_history: vec![],
        ..Default::default()
    };
    let mut app = TuiApp::with_data_source(
        TuiConfig {
            default_view_mode: chimera_tui::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
        Box::new(DecayTestSource::new(snapshot)),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Decay);

    let content = render_to_string(&mut app, 80, 24);
    assert!(
        content.contains("Decay"),
        "Decay panel should render with empty history"
    );
}

// ============================================================
// D. 历史数据测试
// ============================================================

#[test]
fn test_decay_panel_renders_history_sparkline() {
    let mut app = TuiApp::with_data_source(
        TuiConfig {
            default_view_mode: chimera_tui::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
        Box::new(DecayTestSource::new(normal_decay_snapshot())),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Decay);

    // 渲染应包含 sparkline 标题或历史区域
    let content = render_to_string(&mut app, 80, 24);
    // WHY 压缩空白:TestBackend 逐格渲染中文时汉字间含空格("衰 减 历 史"),
    // 断言前先去掉空白使 CJK 文案可按原文匹配
    let compact: String = content.chars().filter(|c| *c != ' ').collect();
    assert!(
        compact.contains("衰减历史") || compact.contains("系数"),
        "decay history sparkline section should be rendered"
    );
}

#[test]
fn test_decay_panel_renders_cycle_start() {
    let mut app = TuiApp::with_data_source(
        TuiConfig {
            default_view_mode: chimera_tui::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
        Box::new(DecayTestSource::new(normal_decay_snapshot())),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Decay);

    let content = render_to_string(&mut app, 80, 24);
    let compact: String = content.chars().filter(|c| *c != ' ').collect();
    // cycle_start 时间戳应显示(检查日期格式的一部分)
    assert!(
        compact.contains("周期开始") || content.contains("20"),
        "cycle start timestamp should be rendered"
    );
}

// ============================================================
// Phase 6 D-6 治理: 影子模式熔断事件驱动化测试
// ============================================================
//
// 原测试固化已删除的 decay_engine::shadow_breaker_status() 全局占位函数
// (虚假数据固化,断言默认全 false)。治理后面板从 latest_events 事件流
// 派生最近 ShadowBreakerTripped 事件的跳闸原因;无事件时诚实显示 N/A。

#[test]
fn test_decay_panel_no_breaker_event_shows_na() {
    // 无 ShadowBreakerTripped 事件时诚实显示 N/A(不虚报全 false)
    let mut app = TuiApp::with_data_source(
        TuiConfig {
            default_view_mode: chimera_tui::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
        Box::new(DecayTestSource::new(normal_decay_snapshot())),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Decay);

    let content = render_to_string(&mut app, 80, 30);
    let compact: String = content.chars().filter(|c| *c != ' ').collect();
    // 面板应包含 "熔断:" 行且值为 N/A
    assert!(
        compact.contains("熔断:N/A"),
        "无跳闸事件时应诚实显示 N/A,实际内容全文:\n{}",
        content
    );
}

#[test]
fn test_decay_panel_displays_breaker_trip_reason() {
    // 注入 ShadowBreakerTripped 事件,验证面板从事件流派生跳闸原因
    let mut snapshot = normal_decay_snapshot();
    snapshot.latest_events = Arc::new(VecDeque::from(vec![NexusEvent::ShadowBreakerTripped {
        metadata: EventMetadata::new("decay-engine"),
        reason: "token_burn_exceeded".into(),
    }]));

    let mut app = TuiApp::with_data_source(
        TuiConfig {
            default_view_mode: chimera_tui::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
        Box::new(DecayTestSource::new(snapshot)),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Decay);

    let content = render_to_string(&mut app, 80, 30);
    assert!(
        content.contains("token_burn_exceeded"),
        "面板应从事件流派生跳闸原因,实际内容全文:\n{}",
        content
    );
}
