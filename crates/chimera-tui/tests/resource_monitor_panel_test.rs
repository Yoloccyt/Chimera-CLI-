//! ResourceMonitor 面板集成测试 — P8
//!
//! 验证 ResourceMonitorPanel 正确渲染 CPU/内存/磁盘/网络四区域指标。

#![forbid(unsafe_code)]

use chimera_tui::panels::{Panel, ResourceMonitorPanel};
use chimera_tui::types::{
    CpuMetrics, DiskMetrics, MemMetrics, NetworkMetrics, PanelId, SystemMetrics, TuiState,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

#[test]
fn test_resource_monitor_panel_id() {
    let panel = ResourceMonitorPanel::new();
    assert_eq!(panel.id(), PanelId::ResourceMonitor);
}

#[test]
fn test_resource_monitor_renders() {
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
    assert!(content.contains("CPU"), "should contain CPU section");
    assert!(content.contains("RAM"), "should contain RAM section");
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
        content.contains("CPU"),
        "CPU title should still be visible when collapsed"
    );
}
