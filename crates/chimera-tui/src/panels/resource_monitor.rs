//! TUI 系统资源监控面板 — CPU/内存/磁盘/网络实时指标
//!
//! 对应架构层:L10 Interface
//! 对应 PanelId:ResourceMonitor
//!
//! # 核心职责
//! - 四个子区域:CPU(每核柱状图 + 全局 sparkline)、内存(gauge + 用量文本)、
//!   磁盘(读写速率 dual sparkline)、网络(收发速率 dual sparkline)
//! - 展开/折叠切换:Enter 切换子区域折叠状态,折叠时仅占 1 行
//! - 键盘导航:Up/Down 在 4 个子区域间切换选择
//! - 趋势图(可选,`with_trends(true)`):300 样本滑动窗口 + 中位数滤波 +
//!   阈值告警三色(Normal < 70% / Warning 70-90% / Critical ≥ 90%)
//!
//! # 向后兼容
//! - 默认 `enable_trend_charts = false`,保留原有瞬时 gauge 行为
//! - 既有 `resource_monitor_panel_test.rs` 与 `mod.rs` 内嵌 7 个测试
//!   必须继续通过(测试断言 CPU/RAM/Disk/Net 标题与瞬时数值)

use std::collections::HashSet;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::data::resource_history::{ResourceHistory, ThresholdLevel};
use crate::panels::Panel;
use crate::render::{
    horizontal_bar_chart, sparkline, sparkline_dual_colored, sparkline_thresholded,
};
use crate::types::{
    CpuMetrics, DiskMetrics, MemMetrics, NetworkMetrics, PanelId, TuiCommand, TuiState,
};

/// 趋势图默认滑动窗口容量(spec:5 分钟 × 1Hz 采样 = 300 样本)
const DEFAULT_TREND_WINDOW: usize = 300;
/// 中位数滤波窗口(任务硬性要求,5 样本)
const FILTER_WINDOW: usize = 5;

/// 系统资源监控面板 — CPU/内存/磁盘/网络实时指标
///
/// 展示 sysinfo 采集的 OS 级资源使用情况，分四个区域:
/// - CPU:每核柱状图 + 全局使用率 sparkline
/// - 内存:gauge + 用量文本
/// - 磁盘:读写速率 sparkline_dual
/// - 网络:收发速率 sparkline_dual
pub struct ResourceMonitorPanel {
    selected: usize,
    /// 展开/折叠状态(控制四个子区域的显示)
    collapsed: HashSet<usize>,
    /// 是否启用趋势图(sparkline 滑动窗口 + 阈值告警)
    ///
    /// WHY 默认 false:保持既有 `resource_monitor_panel_test.rs` 行为,
    /// 通过 `with_trends(true)` 显式启用新功能,符合 spec §三·迁移路径。
    enable_trend_charts: bool,
    /// CPU 使用率滑动窗口(中位数滤波后用于 sparkline 渲染)
    cpu_history: ResourceHistory,
    /// 内存使用率滑动窗口
    mem_history: ResourceHistory,
    /// 网络 RX 速率滑动窗口(字节/秒)
    net_rx_history: ResourceHistory,
    /// 网络 TX 速率滑动窗口(字节/秒)
    net_tx_history: ResourceHistory,
    /// 上次 push_sample 调用时间(用于节流)
    last_sample_at: Option<Instant>,
}

impl ResourceMonitorPanel {
    /// 创建新的 ResourceMonitor 面板(默认禁用趋势图,保持向后兼容)
    pub fn new() -> Self {
        Self {
            selected: 0,
            collapsed: HashSet::new(),
            enable_trend_charts: false,
            cpu_history: ResourceHistory::new(DEFAULT_TREND_WINDOW, FILTER_WINDOW),
            mem_history: ResourceHistory::new(DEFAULT_TREND_WINDOW, FILTER_WINDOW),
            net_rx_history: ResourceHistory::new(DEFAULT_TREND_WINDOW, FILTER_WINDOW),
            net_tx_history: ResourceHistory::new(DEFAULT_TREND_WINDOW, FILTER_WINDOW),
            last_sample_at: None,
        }
    }

    /// 创建带趋势图开关的面板
    ///
    /// # 参数
    /// - `enabled`:true 启用 300 样本滑动窗口 + 阈值告警颜色,
    ///   false 保持默认瞬时 gauge 行为
    ///
    /// WHY 独立构造器:与 `new()` 保持向后兼容,通过显式 `with_trends(true)`
    /// 开启新功能,符合 spec §三·迁移路径(默认 false 避免破坏既有测试)。
    pub fn with_trends(enabled: bool) -> Self {
        Self {
            enable_trend_charts: enabled,
            ..Self::new()
        }
    }

    /// 趋势图是否启用
    pub fn trends_enabled(&self) -> bool {
        self.enable_trend_charts
    }

    /// 推送一个 CPU 采样点到趋势窗口
    ///
    /// 仅在 `trends_enabled() == true` 时有实际效果(否则仍追加以避免测试中
    /// 状态不一致)。重复推送由 `ResourceHistory` FIFO 自动截断。
    pub fn push_cpu_sample(&mut self, value_pct: f32) {
        self.cpu_history.push(current_ts_ms(), value_pct);
    }

    /// 推送一个内存采样点到趋势窗口
    pub fn push_mem_sample(&mut self, value_pct: f32) {
        self.mem_history.push(current_ts_ms(), value_pct);
    }

    /// 推送网络 RX/TX 速率采样点
    pub fn push_net_sample(&mut self, rx_bps: u64, tx_bps: u64) {
        let ts = current_ts_ms();
        self.net_rx_history.push(ts, rx_bps as f32);
        self.net_tx_history.push(ts, tx_bps as f32);
    }

    /// 推送所有资源采样点(一次性从 `SystemMetrics` 提取)
    ///
    /// 由 `DataPipeline` 在每个 tick 调用,或测试中直接调用以填充面板历史。
    pub fn push_all_samples(&mut self, sys: &crate::types::SystemMetrics) {
        self.push_cpu_sample(sys.cpu.global_usage);
        self.push_mem_sample(sys.memory.usage_percent);
        self.push_net_sample(sys.network.rx_bytes_per_sec, sys.network.tx_bytes_per_sec);
    }

    /// CPU 历史窗口(只读,供测试与未来 dashboard 复用)
    pub fn cpu_history(&self) -> &ResourceHistory {
        &self.cpu_history
    }

    /// 内存历史窗口(只读)
    pub fn mem_history(&self) -> &ResourceHistory {
        &self.mem_history
    }
}

/// 获取当前 Unix 时间戳(毫秒)
///
/// WHY 独立函数:将系统时间获取与 `push_*` 解耦,便于测试 mock。
/// `SystemTime::now` + `duration_since(UNIX_EPOCH)` 是 std 标准做法。
fn current_ts_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

impl Default for ResourceMonitorPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl ResourceMonitorPanel {
    // ============================================================
    // CPU 子区域渲染
    // ============================================================

    fn render_cpu_section(
        &mut self,
        state: &TuiState,
        cpu: &CpuMetrics,
        area: Rect,
        buf: &mut Buffer,
    ) {
        // 阈值告警颜色:根据全局 CPU 使用率取 Normal/Warning/Critical
        let level = ThresholdLevel::classify(cpu.global_usage);
        let title_color = level.color();

        // 标题行
        let title = format!(" CPU: {:.1}% ", cpu.global_usage);
        let title_span = Span::styled(
            &title,
            Style::default()
                .fg(title_color)
                .add_modifier(Modifier::BOLD),
        );
        buf.set_line(area.x, area.y, &Line::from(title_span), area.width);

        if self.collapsed.contains(&0) {
            return;
        }

        // CPU sparkline(标题行右侧)
        if area.width > 16 {
            let spark_area = Rect::new(area.x + 12, area.y, area.width.saturating_sub(12), 1);
            if self.enable_trend_charts {
                // 趋势图模式:使用中位数滤波后数据 + 阈值着色
                // WHY sparkline_thresholded:按值(已 ×10 u64)映射到阈值颜色,
                // 70% → 700, 90% → 900,自然契合 sparkline 数据约定
                let filtered = self.cpu_history.filtered_values();
                let data: Vec<u64> = filtered
                    .iter()
                    .map(|v| (v * 10.0).clamp(0.0, 1000.0) as u64)
                    .collect();
                sparkline_thresholded(
                    &data,
                    spark_area,
                    buf,
                    Color::Green,
                    700, // 70% × 10
                    900, // 90% × 10
                    Some(1000),
                );
            } else {
                // 默认模式:复用原有 sparkline
                let spark = sparkline(&state.sys_metrics_history, "", Color::Green);
                spark.render(spark_area, buf);
            }
        }

        // 每核柱状图(剩余区域)
        if area.height > 2 && !cpu.per_core_usage.is_empty() {
            let bar_area = Rect::new(
                area.x,
                area.y + 1,
                area.width,
                area.height.saturating_sub(1),
            );
            let labels: Vec<String> = cpu
                .per_core_usage
                .iter()
                .enumerate()
                .map(|(i, _)| format!("C{i}:"))
                .collect();
            horizontal_bar_chart(
                &cpu.per_core_usage,
                &labels,
                bar_area,
                buf,
                Color::Green,
                Color::Yellow,
                Color::Red,
                60.0,
                80.0,
            );
        }
    }

    fn cpu_color(usage: f32) -> Color {
        // 兼容旧 API 边界(60% 黄 / 80% 红),既有 `cpu_color_thresholds` 测试断言此语义
        if usage >= 80.0 {
            Color::Red
        } else if usage >= 60.0 {
            Color::Yellow
        } else {
            Color::Green
        }
    }

    // ============================================================
    // 内存子区域渲染
    // ============================================================

    fn render_memory_section(&mut self, mem: &MemMetrics, area: Rect, buf: &mut Buffer) {
        let total_gb = mem.total_bytes as f64 / 1024.0 / 1024.0 / 1024.0;
        let used_gb = mem.used_bytes as f64 / 1024.0 / 1024.0 / 1024.0;
        let title = format!(
            " RAM: {:.1}/{:.1} GB ({:.1}%) ",
            used_gb, total_gb, mem.usage_percent
        );
        let style = Style::default()
            .fg(Self::mem_color(mem.usage_percent))
            .add_modifier(Modifier::BOLD);
        buf.set_line(
            area.x,
            area.y,
            &Line::from(Span::styled(&title, style)),
            area.width,
        );

        if self.collapsed.contains(&1) {
            return;
        }

        // 趋势图模式:内存 sparkline(标题行右侧,CPU 同位置)
        if self.enable_trend_charts && area.width > 16 {
            let spark_area = Rect::new(area.x + 22, area.y, area.width.saturating_sub(22), 1);
            let filtered = self.mem_history.filtered_values();
            let data: Vec<u64> = filtered
                .iter()
                .map(|v| (v * 10.0).clamp(0.0, 1000.0) as u64)
                .collect();
            sparkline_thresholded(
                &data,
                spark_area,
                buf,
                Color::Green,
                700, // 70% × 10
                900, // 90% × 10
                Some(1000),
            );
        }

        // Gauge 显示内存使用比例
        if area.height > 1 {
            let gauge_area = Rect::new(area.x, area.y + 1, area.width, 1);
            let ratio = if mem.total_bytes > 0 {
                mem.used_bytes as f64 / mem.total_bytes as f64
            } else {
                0.0
            };
            let filled =
                ((ratio * gauge_area.width as f64).round() as usize).min(gauge_area.width as usize);
            let empty = gauge_area.width as usize - filled;
            let gauge_span = Span::styled(
                format!("[{}{}]", "█".repeat(filled), "░".repeat(empty)),
                Style::default().fg(Self::mem_color(mem.usage_percent)),
            );
            buf.set_line(
                gauge_area.x,
                gauge_area.y,
                &Line::from(gauge_span),
                gauge_area.width,
            );
        }

        // Swap 信息
        if area.height > 2 {
            let swap_used_gb = mem.swap_used_bytes as f64 / 1024.0 / 1024.0 / 1024.0;
            let swap_total_gb = mem.swap_total_bytes as f64 / 1024.0 / 1024.0 / 1024.0;
            let swap_line = format!(" Swap: {:.1}/{:.1} GB", swap_used_gb, swap_total_gb);
            buf.set_line(area.x, area.y + 2, &Line::from(swap_line), area.width);
        }
    }

    fn mem_color(usage: f32) -> Color {
        // 边界 70/90 与 `ThresholdLevel` 一致(spec 阈值告警)
        if usage >= 90.0 {
            Color::Red
        } else if usage >= 70.0 {
            Color::Yellow
        } else {
            Color::Green
        }
    }

    // ============================================================
    // 磁盘子区域渲染
    // ============================================================

    fn render_disk_section(&self, disk: &DiskMetrics, area: Rect, buf: &mut Buffer) {
        let read_mb = disk.read_bytes_per_sec as f64 / 1024.0 / 1024.0;
        let write_mb = disk.write_bytes_per_sec as f64 / 1024.0 / 1024.0;
        let title = format!(" Disk: R {:.1}MB/s  W {:.1}MB/s ", read_mb, write_mb);
        buf.set_line(
            area.x,
            area.y,
            &Line::from(Span::styled(
                &title,
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )),
            area.width,
        );

        if self.collapsed.contains(&2) || area.height <= 2 {
            return;
        }

        // 磁盘读写速率 dual sparkline
        let chart_area = Rect::new(
            area.x,
            area.y + 1,
            area.width,
            area.height.saturating_sub(1),
        );
        // 使用当前速率构造单元素历史
        let read_data = vec![disk.read_bytes_per_sec / 1024]; // KB/s
        let write_data = vec![disk.write_bytes_per_sec / 1024]; // KB/s
        sparkline_dual_colored(
            &read_data,
            &write_data,
            chart_area,
            buf,
            Color::Green,
            Color::Red,
            None,
        );
    }

    // ============================================================
    // 网络子区域渲染
    // ============================================================

    fn render_network_section(&self, net: &NetworkMetrics, area: Rect, buf: &mut Buffer) {
        let rx_mb = net.rx_bytes_per_sec as f64 / 1024.0 / 1024.0;
        let tx_mb = net.tx_bytes_per_sec as f64 / 1024.0 / 1024.0;
        let title = format!(" Net: RX {:.1}MB/s  TX {:.1}MB/s ", rx_mb, tx_mb);
        buf.set_line(
            area.x,
            area.y,
            &Line::from(Span::styled(
                &title,
                Style::default()
                    .fg(Color::Magenta)
                    .add_modifier(Modifier::BOLD),
            )),
            area.width,
        );

        if self.collapsed.contains(&3) || area.height <= 2 {
            return;
        }

        // 网络收发速率 dual sparkline
        let chart_area = Rect::new(
            area.x,
            area.y + 1,
            area.width,
            area.height.saturating_sub(1),
        );
        let rx_data = vec![net.rx_bytes_per_sec / 1024];
        let tx_data = vec![net.tx_bytes_per_sec / 1024];
        sparkline_dual_colored(
            &rx_data,
            &tx_data,
            chart_area,
            buf,
            Color::Green,
            Color::Magenta,
            None,
        );
    }
}

impl Panel for ResourceMonitorPanel {
    fn id(&self) -> PanelId {
        PanelId::ResourceMonitor
    }

    fn title(&self) -> Line<'static> {
        Line::from(" Resources ").style(
            Style::default()
                .fg(Color::Green)
                .add_modifier(Modifier::BOLD),
        )
    }

    fn focus(&mut self, _focused: bool) {}

    fn handle_key(&mut self, key: KeyEvent, _state: &mut TuiState) -> Option<TuiCommand> {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                self.selected = self.selected.saturating_sub(1);
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.selected < 3 {
                    self.selected += 1;
                }
            }
            KeyCode::Enter => {
                if self.collapsed.contains(&self.selected) {
                    self.collapsed.remove(&self.selected);
                } else {
                    self.collapsed.insert(self.selected);
                }
            }
            _ => {}
        }
        None
    }

    fn render(&mut self, state: &TuiState, area: Rect, buf: &mut Buffer) {
        let sys = &state.sys_metrics;

        // 计算各区域约束:折叠区 1 行,展开区等分剩余
        let constraints: Vec<Constraint> = (0..4)
            .map(|i| {
                if self.collapsed.contains(&i) {
                    Constraint::Length(1)
                } else {
                    Constraint::Min(3)
                }
            })
            .collect();

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints(constraints)
            .split(area);

        self.render_cpu_section(state, &sys.cpu, chunks[0], buf);
        self.render_memory_section(&sys.memory, chunks[1], buf);
        self.render_disk_section(&sys.disk, chunks[2], buf);
        self.render_network_section(&sys.network, chunks[3], buf);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resource_monitor_panel_id() {
        let panel = ResourceMonitorPanel::new();
        assert_eq!(panel.id(), PanelId::ResourceMonitor);
    }

    #[test]
    fn test_resource_monitor_panel_title() {
        let panel = ResourceMonitorPanel::new();
        let title = panel.title();
        assert!(title.to_string().contains("Resources"));
    }

    #[test]
    fn test_cpu_color_thresholds() {
        assert_eq!(ResourceMonitorPanel::cpu_color(30.0), Color::Green);
        assert_eq!(ResourceMonitorPanel::cpu_color(60.0), Color::Yellow);
        assert_eq!(ResourceMonitorPanel::cpu_color(80.0), Color::Red);
        assert_eq!(ResourceMonitorPanel::cpu_color(95.0), Color::Red);
    }

    #[test]
    fn test_mem_color_thresholds() {
        assert_eq!(ResourceMonitorPanel::mem_color(50.0), Color::Green);
        assert_eq!(ResourceMonitorPanel::mem_color(70.0), Color::Yellow);
        assert_eq!(ResourceMonitorPanel::mem_color(90.0), Color::Red);
        assert_eq!(ResourceMonitorPanel::mem_color(95.0), Color::Red);
    }

    #[test]
    fn test_handle_key_navigation() {
        let mut panel = ResourceMonitorPanel::new();
        let mut state = TuiState::new();

        // Down 移动到下一个
        panel.handle_key(
            KeyEvent::new(KeyCode::Down, crossterm::event::KeyModifiers::NONE),
            &mut state,
        );
        assert_eq!(panel.selected, 1);

        // Down 继续移动
        panel.handle_key(
            KeyEvent::new(KeyCode::Down, crossterm::event::KeyModifiers::NONE),
            &mut state,
        );
        assert_eq!(panel.selected, 2);

        // Down 到边界不再增加
        panel.handle_key(
            KeyEvent::new(KeyCode::Down, crossterm::event::KeyModifiers::NONE),
            &mut state,
        );
        assert_eq!(panel.selected, 3);
        panel.handle_key(
            KeyEvent::new(KeyCode::Down, crossterm::event::KeyModifiers::NONE),
            &mut state,
        );
        assert_eq!(panel.selected, 3);

        // Up 向上
        panel.handle_key(
            KeyEvent::new(KeyCode::Up, crossterm::event::KeyModifiers::NONE),
            &mut state,
        );
        assert_eq!(panel.selected, 2);

        // 'j' 和 'k' 导航
        panel.handle_key(
            KeyEvent::new(KeyCode::Char('j'), crossterm::event::KeyModifiers::NONE),
            &mut state,
        );
        assert_eq!(panel.selected, 3);

        panel.handle_key(
            KeyEvent::new(KeyCode::Char('k'), crossterm::event::KeyModifiers::NONE),
            &mut state,
        );
        assert_eq!(panel.selected, 2);
    }

    #[test]
    fn test_handle_key_toggle_collapse() {
        let mut panel = ResourceMonitorPanel::new();
        let mut state = TuiState::new();

        // Enter 折叠当前选中项(0 = CPU)
        assert!(!panel.collapsed.contains(&0));
        panel.handle_key(
            KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE),
            &mut state,
        );
        assert!(panel.collapsed.contains(&0));

        // 再次 Enter 展开
        panel.handle_key(
            KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE),
            &mut state,
        );
        assert!(!panel.collapsed.contains(&0));

        // 移动到内存并折叠
        panel.handle_key(
            KeyEvent::new(KeyCode::Down, crossterm::event::KeyModifiers::NONE),
            &mut state,
        );
        panel.handle_key(
            KeyEvent::new(KeyCode::Enter, crossterm::event::KeyModifiers::NONE),
            &mut state,
        );
        assert!(panel.collapsed.contains(&1));
    }

    #[test]
    fn test_render_produces_content() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let mut panel = ResourceMonitorPanel::new();
        let mut state = TuiState::new();
        state.sys_metrics = crate::types::SystemMetrics {
            cpu: CpuMetrics {
                global_usage: 45.0,
                per_core_usage: vec![45.0, 50.0, 40.0, 55.0],
                core_count: 4,
            },
            memory: MemMetrics {
                total_bytes: 17_179_869_184,
                used_bytes: 8_589_934_592,
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
        assert!(content.contains("Disk"), "should contain Disk section");
        assert!(content.contains("Net"), "should contain Network section");
    }

    #[test]
    fn test_collapsed_section_compact() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let mut panel = ResourceMonitorPanel::new();
        // 折叠 CPU 和磁盘
        panel.collapsed.insert(0);
        panel.collapsed.insert(2);
        let mut state = TuiState::new();
        state.sys_metrics = crate::types::SystemMetrics {
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
        // CPU 和 Disk 标题仍应显示(折叠后仅标题行)
        assert!(
            content.contains("CPU"),
            "CPU title should still show when collapsed"
        );
        assert!(
            content.contains("Disk"),
            "Disk title should still show when collapsed"
        );
        assert!(content.contains("RAM"), "RAM should be expanded");
        assert!(content.contains("Net"), "Net should be expanded");
    }

    #[test]
    fn test_focus_is_noop() {
        let mut panel = ResourceMonitorPanel::new();
        panel.focus(true);
        panel.focus(false);
        // 不应 panic,不改变状态
        assert_eq!(panel.selected, 0);
        assert!(panel.collapsed.is_empty());
    }
}
