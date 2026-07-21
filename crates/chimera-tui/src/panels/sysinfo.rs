//! TUI 系统信息面板 — 主机与进程信息只读视图
//!
//! 对应架构层:L10 Interface
//! 对应 PanelId:Sysinfo
//!
//! # 核心职责(spec §二 · 系统监控增强)
//! - 主机信息:OS · CPU 型号/核心数 · 总内存 · 启动时间(仅构造时采集)
//! - 进程信息:Chimera 自身 PID · RSS(按 `TuiConfig.sysinfo_refresh_interval_ms` 周期刷新,默认 5s)
//! - 数据源:`sysinfo` 0.32 crate(纯 Rust 跨平台,符合 `forbid(unsafe_code)`)
//!
//! # 设计决策(WHY)
//! - **自管理模式**(区别于 `ResourceMonitorPanel` 的 `push_*_sample` 模式):
//!   SysinfoPanel 持有 `sysinfo::System` 实例,自管理刷新周期。
//!   原因:sysinfo 数据是 OS-level,无法通过 event-bus 跨进程可靠传递;
//!   每个 TUI 实例独立采集,符合"sysinfo 是 OS 直接 API"语义,无需经 L1 通道。
//!   另:`render()` 期间可避免 sysinfo 跨平台差异(Windows/Linux/macOS 行为一致),
//!   sysinfo 0.32 API 在三平台都有相同方法签名。
//! - **构造即采集**:`SysinfoPanel::new()` 立即调用 `System::new_all()` + 首次进程采集,
//!   确保 `render()` 时不阻塞(< 50ms,本地机器通常 < 10ms)。
//! - **进程信息按需刷新**:主机信息(OS/CPU/内存/启动时间)只在构造时采集一次,
//!   进程 RSS 随进程生命周期变化,按 5s 周期刷新。`refresh_if_needed` 通过
//!   `Instant` 比较实现,避免每次 `render()` 都调用 `refresh_processes`(开销较大,
//!   跨平台约 10-50ms)。
//!
//! # 向后兼容
//! - 默认 `refresh_interval_ms = 5000`,与 `TuiConfig.sysinfo_refresh_interval_ms` 默认值一致
//! - Panel trait 8 个方法签名与既有 panel 保持一致(`id`/`title`/`render`/
//!   `handle_key`/`handle_mouse`/`focus`/`scroll_to_top`/`scroll_to_bottom`)
//! - 既有 30+ 测试不受影响(本面板仅新增,不修改既有 panel)

use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use crossterm::event::{KeyEvent, MouseEvent};
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use sysinfo::{MemoryRefreshKind, Pid, ProcessRefreshKind, RefreshKind, System};

use crate::panels::Panel;
use crate::types::{PanelId, TuiCommand, TuiState};

/// 进程信息刷新最小间隔下限(ms)
///
/// WHY 100ms:sysinfo `refresh_processes` 跨平台约 10-50ms,低于 100ms
/// 会导致频繁调用占用 CPU 资源。`TuiConfig::validate` 同样限制 ≥ 100。
const MIN_REFRESH_INTERVAL_MS: u64 = 100;

/// 默认刷新间隔 5s(spec §三 · 任务管理增强 / TuiConfig.sysinfo_refresh_interval_ms)
const DEFAULT_REFRESH_INTERVAL_MS: u64 = 5000;

/// 系统信息面板 — 主机 + Chimera 进程只读视图
///
/// 持有 `sysinfo::System` 实例,自管理刷新周期。
/// `#![forbid(unsafe_code)]` 由 crate 顶层属性保证(§4.1)。
pub struct SysinfoPanel {
    /// sysinfo 状态(持有 `System` 而非每次重建,避免重复加载开销)
    system: System,
    /// 上次进程信息刷新时间
    ///
    /// WHY Instant 而非 SystemTime:仅用于内部"距上次刷新多久"判断,
    /// 不会跨进程/跨设备传递,Instant 单调递增避免时钟回拨影响。
    last_refresh_at: Instant,
    /// 进程信息刷新间隔(毫秒)
    refresh_interval_ms: u64,
    /// Chimera 进程自身 PID(`std::process::id()`)
    ///
    /// WHY u32 而非 sysinfo::Pid:Pid 内部是 u32 的 NewType,但跨平台
    /// 存储字段用裸 u32 更便于序列化与测试比较,转换在调用处显式完成。
    pid: u32,
}

impl SysinfoPanel {
    /// 创建新的 SysinfoPanel(默认 5s 刷新间隔,立即采集主机+进程信息)
    ///
    /// # 设计
    /// - 调用 `System::new_all()` 立即加载 OS/CPU/内存/启动时间
    /// - 同步采集进程信息(PID/RSS),避免首次 `render()` 阻塞
    pub fn new() -> Self {
        Self::with_refresh_interval(DEFAULT_REFRESH_INTERVAL_MS)
    }

    /// 创建带自定义刷新间隔的 SysinfoPanel
    ///
    /// # 参数
    /// - `interval_ms`:刷新间隔(毫秒),下限 100ms(sysinfo 进程采集开销 ~10-50ms)
    ///
    /// # 跨平台注意
    /// `System::new_all()` 在 Windows/Linux/macOS 上语义一致,均会立即
    /// 同步采集全部 sysinfo 数据;首次调用约 10-50ms,后续 `refresh_*`
    /// 仅更新已采集的子集。
    pub fn with_refresh_interval(interval_ms: u64) -> Self {
        let interval_ms = interval_ms.max(MIN_REFRESH_INTERVAL_MS);
        let mut system = System::new_all();
        // 显式刷新进程 + 内存(0.32 API 变更,需用 refresh_specifics)
        // WHY refresh_specifics 而非 refresh_all:仅刷新所需子集,
        // 跳过 CPU/disks/networks,降低跨平台开销。
        // MemoryRefreshKind::everything() = ram + swap,涵盖 SysinfoPanel 展示所需。
        system.refresh_specifics(
            RefreshKind::new()
                .with_processes(ProcessRefreshKind::new().with_memory())
                .with_memory(MemoryRefreshKind::everything()),
        );
        Self {
            system,
            // 立即采集的时间戳即为"上次刷新"
            last_refresh_at: Instant::now(),
            refresh_interval_ms: interval_ms,
            pid: std::process::id(),
        }
    }

    /// 检查是否到达刷新间隔,若是则刷新进程信息
    ///
    /// `render()` 前调用,确保面板展示的是最新进程数据(主机信息无需刷新)。
    fn refresh_if_needed(&mut self) {
        let elapsed = self.last_refresh_at.elapsed();
        if elapsed >= Duration::from_millis(self.refresh_interval_ms) {
            self.system.refresh_specifics(
                RefreshKind::new().with_processes(ProcessRefreshKind::new().with_memory()),
            );
            self.last_refresh_at = Instant::now();
        }
    }

    /// 强制刷新(测试用 / 用户按 `r` 键手动触发)
    pub fn force_refresh(&mut self) {
        self.system.refresh_specifics(
            RefreshKind::new()
                .with_processes(ProcessRefreshKind::new().with_memory())
                .with_memory(MemoryRefreshKind::everything()),
        );
        self.last_refresh_at = Instant::now();
    }

    /// 返回 CPU 品牌字符串(测试用 + `cpu_model_displays_correctly` 断言)
    pub fn cpu_brand(&self) -> String {
        self.system
            .cpus()
            .first()
            .map(|cpu| {
                let brand = cpu.brand();
                if brand.is_empty() {
                    cpu.name().to_string()
                } else {
                    brand.to_string()
                }
            })
            .unwrap_or_default()
    }

    /// 返回逻辑核心数(物理/逻辑统一为 cpus().len(),与 `ResourceMonitorPanel` 一致)
    pub fn cpu_count(&self) -> usize {
        self.system.cpus().len()
    }

    /// 返回总物理内存(字节)
    pub fn total_memory(&self) -> u64 {
        self.system.total_memory()
    }

    /// 返回系统启动时间(Unix 秒)
    pub fn boot_time(&self) -> u64 {
        System::boot_time()
    }

    /// 返回 Chimera 进程 RSS(字节),进程不可用时返回 None
    pub fn chimera_memory(&self) -> Option<u64> {
        self.system
            .process(Pid::from_u32(self.pid))
            .map(|p| p.memory())
    }

    /// 返回 OS 长版本字符串
    pub fn os_version(&self) -> String {
        System::long_os_version()
            .or_else(System::kernel_version)
            .unwrap_or_else(|| "unknown".to_string())
    }

    /// 返回主机名
    pub fn host_name(&self) -> String {
        System::host_name().unwrap_or_else(|| "unknown".to_string())
    }

    /// 返回 CPU 架构
    pub fn cpu_arch(&self) -> String {
        System::cpu_arch().unwrap_or_else(|| "unknown".to_string())
    }

    /// 返回上次刷新时间(测试用)
    pub fn last_refresh_at(&self) -> Instant {
        self.last_refresh_at
    }

    /// 返回当前刷新间隔(毫秒)
    pub fn refresh_interval_ms(&self) -> u64 {
        self.refresh_interval_ms
    }

    // ============================================================
    // 渲染辅助
    // ============================================================

    /// 将字节数格式化为人类可读字符串(GB / MB)
    ///
    /// WHY 独立函数:与 render 解耦,便于单测覆盖格式化边界。
    /// 1024 进制与 `ResourceMonitorPanel::render_memory_section` 保持一致。
    fn format_bytes(bytes: u64) -> String {
        const GB: f64 = 1024.0 * 1024.0 * 1024.0;
        const MB: f64 = 1024.0 * 1024.0;
        let b = bytes as f64;
        if b >= GB {
            format!("{:.2} GB", b / GB)
        } else if b >= MB {
            format!("{:.1} MB", b / MB)
        } else {
            format!("{} B", bytes)
        }
    }

    /// 将 Unix 秒时间戳格式化为本地时间字符串
    fn format_boot_time(unix_secs: u64) -> String {
        DateTime::<Utc>::from_timestamp(unix_secs as i64, 0)
            .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
            .unwrap_or_else(|| "unknown".to_string())
    }
}

impl Default for SysinfoPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl Panel for SysinfoPanel {
    fn id(&self) -> PanelId {
        PanelId::Sysinfo
    }

    fn title(&self) -> Line<'static> {
        Line::from(" System Info ").style(
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
    }

    fn focus(&mut self, _focused: bool) {
        // SysinfoPanel 是只读视图,焦点变化不改变任何状态
    }

    fn handle_key(&mut self, key: KeyEvent, _state: &mut TuiState) -> Option<TuiCommand> {
        // 'r' 键手动触发刷新(spec 隐含约定,运维调试时常需要)
        if let crossterm::event::KeyCode::Char('r') = key.code {
            self.force_refresh();
        }
        None
    }

    fn handle_mouse(&mut self, _mouse: MouseEvent, _state: &mut TuiState) -> Option<TuiCommand> {
        // SysinfoPanel 不处理鼠标事件
        None
    }

    fn render(&mut self, _state: &TuiState, area: Rect, buf: &mut Buffer) {
        // 渲染前先按需刷新进程信息(主机信息仅构造时采集,无需刷新)
        self.refresh_if_needed();

        // 字段布局:每个字段独立一行,简单清晰,无需复杂 Layout
        let os_version = self.os_version();
        let host = self.host_name();
        let arch = self.cpu_arch();
        let cpu_brand = self.cpu_brand();
        let cpu_count = self.cpu_count();
        let total_mem = Self::format_bytes(self.total_memory());
        let boot = Self::format_boot_time(self.boot_time());
        let chimera_rss = self
            .chimera_memory()
            .map(Self::format_bytes)
            .unwrap_or_else(|| "n/a".to_string());

        // 行 1: OS 信息(hostname / os_version / arch)
        let line1 = format!("OS: {}  |  {}  |  arch: {}", host, os_version, arch);
        buf.set_line(
            area.x,
            area.y,
            &Line::from(Span::styled(line1, Style::default().fg(Color::White))),
            area.width,
        );

        // 行 2: CPU 型号 + 核心数
        if area.height > 1 {
            let line2 = format!("CPU: {}  |  cores: {}", cpu_brand, cpu_count);
            buf.set_line(
                area.x,
                area.y + 1,
                &Line::from(Span::styled(line2, Style::default().fg(Color::Green))),
                area.width,
            );
        }

        // 行 3: 内存
        if area.height > 2 {
            let line3 = format!("Memory Total: {}", total_mem);
            buf.set_line(
                area.x,
                area.y + 2,
                &Line::from(Span::styled(line3, Style::default().fg(Color::Yellow))),
                area.width,
            );
        }

        // 行 4: 启动时间
        if area.height > 3 {
            let line4 = format!("Boot: {}", boot);
            buf.set_line(
                area.x,
                area.y + 3,
                &Line::from(Span::styled(line4, Style::default().fg(Color::Blue))),
                area.width,
            );
        }

        // 行 5: Chimera 进程 PID + RSS
        if area.height > 4 {
            let line5 = format!("Chimera: PID {}  |  RSS: {}", self.pid, chimera_rss);
            buf.set_line(
                area.x,
                area.y + 4,
                &Line::from(Span::styled(
                    line5,
                    Style::default()
                        .fg(Color::Magenta)
                        .add_modifier(Modifier::BOLD),
                )),
                area.width,
            );
        }

        // 行 6: 操作提示
        if area.height > 5 {
            let line6 = "Press 'r' to refresh process info";
            buf.set_line(
                area.x,
                area.y + 5,
                &Line::from(Span::styled(line6, Style::default().fg(Color::DarkGray))),
                area.width,
            );
        }
    }

    fn shortcuts(&self) -> Vec<(&'static str, &'static str)> {
        vec![("R", "刷新")]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证默认构造不 panic,且初始 last_refresh_at 已被设置
    #[test]
    fn test_new_does_not_panic() {
        let panel = SysinfoPanel::new();
        // 默认 5s
        assert_eq!(panel.refresh_interval_ms(), 5000);
        // 立即构造 → last_refresh_at 与当前时间接近
        let now = Instant::now();
        let diff = now.duration_since(panel.last_refresh_at());
        assert!(
            diff < Duration::from_millis(100),
            "last_refresh_at should be set at construction, diff: {diff:?}"
        );
    }

    /// 验证 with_refresh_interval 接受自定义间隔
    #[test]
    fn test_with_refresh_interval() {
        let panel = SysinfoPanel::with_refresh_interval(1000);
        assert_eq!(panel.refresh_interval_ms(), 1000);

        // 下限 100ms:传入 50 应被钳制到 100
        let panel = SysinfoPanel::with_refresh_interval(50);
        assert_eq!(panel.refresh_interval_ms(), 100);
    }

    /// 验证 cpu_brand 返回非空
    #[test]
    fn test_cpu_brand_non_empty() {
        let panel = SysinfoPanel::new();
        // 任何真实平台都有 CPU brand 字符串
        let brand = panel.cpu_brand();
        assert!(!brand.is_empty(), "cpu_brand should be non-empty");
    }

    /// 验证 cpu_count 至少为 1
    #[test]
    fn test_cpu_count_at_least_one() {
        let panel = SysinfoPanel::new();
        assert!(panel.cpu_count() >= 1);
    }

    /// 验证 total_memory 大于 0
    #[test]
    fn test_total_memory_positive() {
        let panel = SysinfoPanel::new();
        // 任何真实机器至少有几十 MB 内存
        assert!(panel.total_memory() > 0);
    }

    /// 验证 format_bytes 各档位
    #[test]
    fn test_format_bytes() {
        // < 1MB 显示为 B
        assert_eq!(SysinfoPanel::format_bytes(512), "512 B");
        // 1024 B 仍 < 1MB,显示为 "1024 B"(无 KB 档位)
        assert_eq!(SysinfoPanel::format_bytes(1024), "1024 B");
        // 1 MB
        let s = SysinfoPanel::format_bytes(1024 * 1024);
        assert!(s.contains("MB"), "1MB should contain 'MB', got: {s}");
        assert!(
            s.starts_with("1.0"),
            "1MB should start with '1.0', got: {s}"
        );
        // 1 GB
        let s = SysinfoPanel::format_bytes(1024 * 1024 * 1024);
        assert!(s.contains("GB"), "1GB should contain 'GB', got: {s}");
    }

    /// 验证 refresh_if_needed 在间隔未到时不刷新,到时刷新
    #[test]
    fn test_refresh_if_needed_respects_interval() {
        let mut panel = SysinfoPanel::with_refresh_interval(100);
        let initial = panel.last_refresh_at();
        // 立即调用:不应刷新
        panel.refresh_if_needed();
        assert_eq!(panel.last_refresh_at(), initial);

        // 等待 150ms,超过 100ms 间隔
        std::thread::sleep(Duration::from_millis(150));
        panel.refresh_if_needed();
        assert!(panel.last_refresh_at() > initial);
    }

    /// 验证 force_refresh 立即刷新
    #[test]
    fn test_force_refresh_advances_timestamp() {
        let mut panel = SysinfoPanel::new();
        let initial = panel.last_refresh_at();
        std::thread::sleep(Duration::from_millis(5));
        panel.force_refresh();
        assert!(panel.last_refresh_at() > initial);
    }

    /// 验证 Panel trait 实现正确
    #[test]
    fn test_panel_trait_id() {
        let panel = SysinfoPanel::new();
        assert_eq!(panel.id(), PanelId::Sysinfo);
    }

    /// 验证 title() 返回非空文本
    #[test]
    fn test_panel_trait_title() {
        let panel = SysinfoPanel::new();
        let title = panel.title();
        assert!(!title.to_string().is_empty());
        assert!(title.to_string().contains("System"));
    }

    /// 验证 render 不 panic 且产生预期内容
    #[test]
    fn test_render_produces_content() {
        use ratatui::backend::TestBackend;
        use ratatui::Terminal;

        let mut panel = SysinfoPanel::new();
        let state = TuiState::new();
        let backend = TestBackend::new(100, 30);
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

        assert!(content.contains("OS"), "should contain OS line");
        assert!(content.contains("CPU"), "should contain CPU line");
        assert!(
            content.contains("Chimera"),
            "should contain Chimera PID line"
        );
    }
}
