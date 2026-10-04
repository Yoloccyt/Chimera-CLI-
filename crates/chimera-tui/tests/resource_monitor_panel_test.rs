//! ResourceMonitor 面板集成测试 — P8
//!
//! 验证 ResourceMonitorPanel 正确渲染 CPU/内存/磁盘/网络四区域指标。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use chimera_tui::panels::{Panel, ResourceMonitorPanel};
use chimera_tui::types::{
    CpuMetrics, DiskMetrics, MemMetrics, NetworkMetrics, PanelId, SystemMetrics, TuiState,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

/// 取键值首个 {} 占位符之前的固定前缀(trim 后即子区标题特征;B6-M 断言范式:
/// 与键表同源,避免断言串与键值双源漂移)
fn title_prefix(key: &'static str) -> String {
    chimera_tui::i18n::tr(key)
        .split("{}")
        .next()
        .unwrap_or_default()
        .trim()
        .to_string()
}

#[test]
fn test_resource_monitor_panel_id() {
    let panel = ResourceMonitorPanel::new();
    assert_eq!(panel.id(), PanelId::ResourceMonitor);
}

#[test]
fn test_resource_monitor_renders() {
    // B6-M:四子区标题迁键;TestBackend 全帧逐 cell 收集下 zh 宽字符会被
    // 续格空格碎裂,故 pin En 后经 t! 前缀派生断言(guard 防 locale 竞争)
    let _locale_guard = chimera_tui::i18n::locale_test_guard();
    chimera_tui::set_locale(chimera_tui::Locale::En);
    let mut panel = ResourceMonitorPanel::new();
    let mut state = TuiState::new();
    // 填充示例系统指标
    state.sys_metrics = SystemMetrics {
        cpu: CpuMetrics {
            global_usage: 45.0,
            per_core_usage: vec![45.0, 50.0, 40.0, 55.0],
            core_count: 4,
        },
        memory: MemMetrics {
            total_bytes: 17_179_869_184, // 16 GB
            used_bytes: 8_589_934_592,   // 8 GB
            available_bytes: 8_589_934_592,
            usage_percent: 50.0,
            swap_total_bytes: 2_147_483_648,
            swap_used_bytes: 536_870_912,
        },
        disk: DiskMetrics::default(),
        network: NetworkMetrics::default(),
    };
    state.sys_metrics_history = vec![400, 420, 450, 430, 460];

    let backend = TestBackend::new(80, 20);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            let area = f.area();
            panel.render(&state, area, f.buffer_mut());
        })
        .unwrap();

    let buffer = terminal.backend().buffer();
    let content: String = buffer
        .content()
        .iter()
        .map(|c| c.symbol().chars().next().unwrap_or(' '))
        .collect();
    assert!(
        content.contains(&title_prefix("panel.resources.cpu_title")),
        "should contain CPU section"
    );
    assert!(
        content.contains(&title_prefix("panel.resources.ram_title")),
        "should contain RAM section"
    );
    chimera_tui::set_locale(chimera_tui::Locale::Zh);
}

#[test]
fn test_resource_monitor_handle_key_navigation() {
    let mut panel = ResourceMonitorPanel::new();
    let mut state = TuiState::new();
    // 按下键移动选择
    let cmd = panel.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &mut state);
    assert!(cmd.is_none()); // 导航不产生命令
}

#[test]
fn test_resource_monitor_toggle_collapse() {
    // B6-M:标题迁键,pin En + 经 t! 前缀派生断言(同 test_resource_monitor_renders)
    let _locale_guard = chimera_tui::i18n::locale_test_guard();
    chimera_tui::set_locale(chimera_tui::Locale::En);
    let mut panel = ResourceMonitorPanel::new();
    let mut state = TuiState::new();

    // Enter 折叠当前项
    panel.handle_key(
        KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
        &mut state,
    );
    // 通过重新渲染验证折叠后标题仍可见但内容缩减
    state.sys_metrics = SystemMetrics {
        cpu: CpuMetrics {
            global_usage: 10.0,
            per_core_usage: vec![10.0],
            core_count: 1,
        },
        memory: MemMetrics::default(),
        disk: DiskMetrics::default(),
        network: NetworkMetrics::default(),
    };

    let backend = TestBackend::new(80, 20);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            let area = f.area();
            panel.render(&state, area, f.buffer_mut());
        })
        .unwrap();

    let buffer = terminal.backend().buffer();
    let content: String = buffer
        .content()
        .iter()
        .map(|c| c.symbol().chars().next().unwrap_or(' '))
        .collect();
    // 折叠后 CPU 标题仍应可见
    assert!(
        content.contains(&title_prefix("panel.resources.cpu_title")),
        "CPU title should still be visible when collapsed"
    );
    chimera_tui::set_locale(chimera_tui::Locale::Zh);
}
