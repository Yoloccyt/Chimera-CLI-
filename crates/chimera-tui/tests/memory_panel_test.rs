//! Memory 面板集成测试 — M2
//!
//! 验证 MemoryPanel 在自定义数据下正确渲染命中率、上下文窗口、压缩率与层级。

#![forbid(unsafe_code)]

use chimera_tui::{
    set_locale, DataSnapshot, DataSourceConfig, Locale, MemoryMetrics, PanelId, TuiApp, TuiConfig,
    TuiDataSource, TuiError,
};
use event_bus::{EventMetadata, NexusEvent};
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

/// locale 串行化锁 — 与 i18n_chrome_test 同模式,避免 En 固定窗口被并行测试复位
static LOCALE_LOCK: Mutex<()> = Mutex::new(());

/// 测试数据源 — 返回预设 Memory 指标
#[derive(Debug)]
struct MemoryTestSource {
    snapshot: DataSnapshot,
    config: DataSourceConfig,
}

impl MemoryTestSource {
    fn new(snapshot: DataSnapshot) -> Self {
        Self {
            snapshot,
            config: DataSourceConfig::default(),
        }
    }
}

impl TuiDataSource for MemoryTestSource {
    fn snapshot(&self) -> Result<std::sync::Arc<DataSnapshot>, TuiError> {
        Ok(std::sync::Arc::new(self.snapshot.clone()))
    }

    fn config(&self) -> &DataSourceConfig {
        &self.config
    }
}

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

#[test]
fn test_memory_panel_renders_with_sample_data() {
    let snapshot = DataSnapshot {
        memory_metrics: MemoryMetrics {
            hit_rate_percent: 92.5,
            evictions: 3,
            context_window_size: 8192,
            compressed_ratio: 0.65,
            cache_hits: 500,
            cache_misses: 42,
            tier: "L2".into(),
        },
        budget_history: vec![70, 75, 80, 85, 90, 92],
        memory_history: vec![80, 82, 85, 88, 90, 92],
        ..Default::default()
    };

    let mut app = TuiApp::with_data_source(
        TuiConfig {
            default_view_mode: chimera_tui::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
        Box::new(MemoryTestSource::new(snapshot)),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Memory);

    let content = render_to_string(&mut app, 80, 24);
    assert!(
        content.contains("Memory"),
        "Memory panel title should be rendered"
    );
    assert!(
        content.contains("92.5%"),
        "hit rate should be rendered, got: {}",
        &content[..content.len().min(300)]
    );
    assert!(
        content.contains("8192 bytes"),
        "context window size should be rendered"
    );
    assert!(
        content.contains("65.0%") || content.contains("65%"),
        "compressed ratio should be rendered"
    );
    assert!(content.contains("L2"), "tier should be rendered");
}

#[test]
fn test_memory_panel_empty_data_renders_defaults() {
    let snapshot = DataSnapshot {
        memory_metrics: MemoryMetrics::default(),
        ..Default::default()
    };

    let mut app = TuiApp::with_data_source(
        TuiConfig {
            default_view_mode: chimera_tui::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
        Box::new(MemoryTestSource::new(snapshot)),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Memory);

    let content = render_to_string(&mut app, 80, 24);
    assert!(
        content.contains("Memory"),
        "Memory panel should render even with default data"
    );
    assert!(content.contains("L0"));
}

// ============================================================
// Phase 6 D-6 治理: 四层存储分布事件驱动化测试
// ============================================================
//
// 原测试固化已删除的 cmt_tiering::tier_distribution() 全局占位函数
// (虚假数据固化,断言恒零)。治理后面板从 latest_events 事件流派生
// 最近 CapabilityTierStatsReported 事件;无事件时诚实显示 N/A。

#[test]
fn test_memory_panel_displays_tier_distribution() {
    let _locale_guard = chimera_tui::i18n::locale_test_guard();
    // i18n(U-3):面板正文标签随 locale 切换,固定 En 断言英文 "Storage:" 行
    let _guard = LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_locale(Locale::En);
    // 注入 CapabilityTierStatsReported 事件,验证面板从事件流派生四层分布
    let snapshot = DataSnapshot {
        memory_metrics: MemoryMetrics {
            hit_rate_percent: 92.5,
            evictions: 3,
            context_window_size: 8192,
            compressed_ratio: 0.65,
            cache_hits: 500,
            cache_misses: 42,
            tier: "L2".into(),
        },
        memory_history: vec![80, 82, 85, 88, 90, 92],
        latest_events: Arc::new(VecDeque::from(vec![
            NexusEvent::CapabilityTierStatsReported {
                metadata: EventMetadata::new("cmt-tiering"),
                hot: 5,
                warm: 3,
                cold: 2,
                ice: 1,
            },
        ])),
        ..Default::default()
    };

    let mut app = TuiApp::with_data_source(
        TuiConfig {
            default_view_mode: chimera_tui::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
        Box::new(MemoryTestSource::new(snapshot)),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Memory);

    let content = render_to_string(&mut app, 80, 24);
    // 面板应包含事件派生的四层条目数分布
    assert!(
        content.contains("Storage: Hot:5"),
        "面板应从事件流派生四层存储分布,实际内容前 300 字符: {}",
        &content[..content.len().min(300)]
    );
    set_locale(Locale::Zh); // 复位默认中文
}

#[test]
fn test_memory_panel_no_tier_event_shows_na() {
    let _locale_guard = chimera_tui::i18n::locale_test_guard();
    // 无 CapabilityTierStatsReported 事件时诚实显示 N/A(不虚报恒零)
    let _guard = LOCALE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    set_locale(Locale::En);
    let snapshot = DataSnapshot {
        memory_metrics: MemoryMetrics::default(),
        ..Default::default()
    };

    let mut app = TuiApp::with_data_source(
        TuiConfig {
            default_view_mode: chimera_tui::ViewMode::Dashboard,
            persist_state: false,
            ..Default::default()
        },
        Box::new(MemoryTestSource::new(snapshot)),
    )
    .unwrap();
    app.update();
    app.switch_panel_to(PanelId::Memory);

    let content = render_to_string(&mut app, 80, 24);
    assert!(
        content.contains("Storage: N/A"),
        "无事件时应诚实显示 N/A,实际内容前 300 字符: {}",
        &content[..content.len().min(300)]
    );
    set_locale(Locale::Zh);
}
