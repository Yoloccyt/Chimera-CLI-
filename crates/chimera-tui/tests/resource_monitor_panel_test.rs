//! ResourceMonitor 面板集成测试 — P8
//!
//! 验证 ResourceMonitorPanel 正确渲染 CPU/内存/磁盘/网络四区域指标。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::collections::HashSet;

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

// ============================================================
// F-39/F-48:数据链接线(网络趋势消费 / 磁盘孤柱移除)与选中指示
// ============================================================

/// 渲染面板到 80×24 TestBackend 并返回整帧 RowMajor 拼接串
///
/// 全展开时四区块由 `Constraint::Min(3)` 均分:每块 6 行,CPU/RAM/Disk/Net
/// 标题分别落在 y=0/6/12/18,其 sparkline 区为标题行之下 5 行。
fn render_frame(panel: &mut ResourceMonitorPanel, state: &TuiState) -> String {
    let backend = TestBackend::new(80, 24);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            let area = f.area();
            panel.render(state, area, f.buffer_mut());
        })
        .unwrap();
    let buffer = terminal.backend().buffer();
    buffer
        .content()
        .iter()
        .map(|c| c.symbol().chars().next().unwrap_or(' '))
        .collect()
}

/// 提取 sparkline 柱状字符种类数('█'/'▄'/'▀')
fn block_char_kinds(area: &str) -> HashSet<char> {
    area.chars().filter(|c| "█▄▀".contains(*c)).collect()
}

/// F-39:喂数后网络 sparkline 反映时间趋势(多元素)
///
/// 修复前:渲染每帧以瞬时值构造单元素历史,速率>0 渲染满格色块、=0 全空白,
/// push_net_sample 全仓零调用(喂数链路两端皆断)。
#[test]
fn test_net_sparkline_renders_time_trend_after_push() {
    let mut panel = ResourceMonitorPanel::new();
    // 交替速率喂数(单位 B/s):RX 高 TX 低 ↔ RX 低 TX 高;中位数滤波后仍
    // 交替,sparkline 列间高度分化,RX/TX 高低错落产生 ≥2 种 block 字符
    for i in 0..7 {
        if i % 2 == 0 {
            panel.push_net_sample(1000 * 1024, 500 * 1024);
        } else {
            panel.push_net_sample(500 * 1024, 1000 * 1024);
        }
    }
    let state = TuiState::new();

    let content = render_frame(&mut panel, &state);
    let net_area = &content[18 * 80..24 * 80];
    let kinds = block_char_kinds(net_area);
    assert!(
        kinds.len() >= 2,
        "喂数后网络 sparkline 应呈现多元素趋势(≥2 种 block 字符),got {kinds:?}:\n{net_area}"
    );
}

/// F-39:生产喂数路径 —— 面板自身 history 为空时回退快照推入的速率历史
///
/// DataPipeline 重 tick 与 sysinfo 采集同节奏推 state.net_rx_history /
/// net_tx_history(单位 KB/s),网络 sparkline 必须消费该历史恢复时间趋势。
#[test]
fn test_net_sparkline_falls_back_to_snapshot_history() {
    let mut panel = ResourceMonitorPanel::new();
    let mut state = TuiState::new();
    // 面板 history 保持为空;state 历史交替(KB/s)
    state.net_rx_history = vec![1000, 500, 1000, 500, 1000, 500, 1000];
    state.net_tx_history = vec![500, 1000, 500, 1000, 500, 1000, 500];

    let content = render_frame(&mut panel, &state);
    let net_area = &content[18 * 80..24 * 80];
    let kinds = block_char_kinds(net_area);
    assert!(
        kinds.len() >= 2,
        "快照喂数路径下网络 sparkline 应呈现趋势(≥2 种 block 字符),got {kinds:?}:\n{net_area}"
    );
}

/// F-39:磁盘孤柱 sparkline 移除(诚实路径)—— 标题数值保留,不再渲染
/// 伪造趋势(sysinfo 0.32 无磁盘 IO 速率源,单元素历史不反映时间趋势)
#[test]
fn test_disk_sparkline_removed_honest_path() {
    let _locale_guard = chimera_tui::i18n::locale_test_guard();
    chimera_tui::set_locale(chimera_tui::Locale::En);
    let mut panel = ResourceMonitorPanel::new();
    let mut state = TuiState::new();
    // 构造非零速率:旧行为会以瞬时值渲染满格色块(单元素 max 归一化自指)
    state.sys_metrics.disk = DiskMetrics {
        read_bytes_per_sec: 5000 * 1024,
        write_bytes_per_sec: 5000 * 1024,
        ..Default::default()
    };

    let content = render_frame(&mut panel, &state);
    let disk_area = &content[12 * 80..18 * 80];
    assert!(
        !disk_area.chars().any(|c| "█▄▀".contains(c)),
        "磁盘孤柱 sparkline 应已移除(诚实路径),got:\n{disk_area}"
    );
    assert!(
        disk_area.contains(&title_prefix("panel.resources.disk_title")),
        "磁盘标题数值展示应保留"
    );
    chimera_tui::set_locale(chimera_tui::Locale::Zh);
}

/// F-48:选中区块标题以 "> " 前缀指示,未选中区块为等宽空格(标题列对齐)
#[test]
fn test_selected_section_title_indicator() {
    let _locale_guard = chimera_tui::i18n::locale_test_guard();
    chimera_tui::set_locale(chimera_tui::Locale::En);
    let mut panel = ResourceMonitorPanel::new();
    let mut state = TuiState::new();
    // Down ×2:selected 0→1→2(磁盘区块)
    panel.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &mut state);
    panel.handle_key(KeyEvent::new(KeyCode::Down, KeyModifiers::NONE), &mut state);

    let content = render_frame(&mut panel, &state);
    let (cpu_row, ram_row, disk_row, net_row) = (
        &content[0..2],
        &content[6 * 80..6 * 80 + 2],
        &content[12 * 80..12 * 80 + 2],
        &content[18 * 80..18 * 80 + 2],
    );
    assert_eq!(disk_row, "> ", "选中区块(磁盘)标题应以 '> ' 前缀指示");
    assert_eq!(cpu_row, "  ", "未选中区块标题前缀应为等宽空格");
    assert_eq!(ram_row, "  ", "未选中区块标题前缀应为等宽空格");
    assert_eq!(net_row, "  ", "未选中区块标题前缀应为等宽空格");
    chimera_tui::set_locale(chimera_tui::Locale::Zh);
}
