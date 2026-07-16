//! ResourceMonitorPanel 趋势图增强 — RED 阶段测试
//!
//! 对应 spec:enterprise-tui-monitoring-task-viz §二·系统监控增强
//! 对应创新点:实时趋势图(sparkline 滑动窗口) + 阈值告警颜色 + 中位数滤波
//!
//! # 测试目标
//! 1. `ResourceHistory` 滑动窗口 + 中位数滤波去抖动
//! 2. `ResourceMonitorPanel::with_trends(enabled=true)` 渲染 300 采样点 sparkline
//! 3. `ThresholdLevel` 三档阈值(Warning 70-90% / Critical > 90%)颜色映射
//! 4. 中位数滤波有效平滑锯齿(方差降低)
//!
//! # TDD 流程
//! - 当前 RED 阶段:测试使用尚未实现的 API,编译失败/断言失败即为预期结果
//! - 后续 GREEN 阶段:实现 `data::resource_history::ResourceHistory` 与
//!   `ResourceMonitorPanel::with_trends` 后,本测试应全部通过
//! - REFACTOR 阶段:提取通用 `TrendWindow` 组件(若需)

#![forbid(unsafe_code)]

use std::time::Duration;

use chimera_tui::data::resource_history::{ResourceHistory, ThresholdLevel};
use chimera_tui::panels::{Panel, ResourceMonitorPanel};
use chimera_tui::types::{
    CpuMetrics, DiskMetrics, MemMetrics, NetworkMetrics, PanelId, SystemMetrics, TuiState,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::TestBackend;
use ratatui::Terminal;

// ============================================================
// 1. 趋势图渲染 300 采样点测试
// ============================================================

#[test]
fn test_trend_chart_renders_300_samples() {
    // WHY 300 采样点:spec §二 默认 5 分钟滑动窗口 × 1s 采样间隔 = 300 样本
    let mut panel = ResourceMonitorPanel::with_trends(true);
    let mut state = TuiState::new();

    // 构造 300 个 CPU 历史点(锯齿状,模拟真实采样抖动)
    let mut history: Vec<u64> = Vec::with_capacity(300);
    for i in 0..300 {
        // 基础值 50% + 锯齿 ±20
        let v = 500 + ((i as i32 % 40) - 20) * 5;
        history.push(v as u64);
    }
    state.sys_metrics_history = history;

    state.sys_metrics = SystemMetrics {
        cpu: CpuMetrics {
            global_usage: 50.0,
            per_core_usage: vec![50.0, 50.0, 50.0, 50.0],
            core_count: 4,
        },
        memory: MemMetrics {
            total_bytes: 16 * 1024 * 1024 * 1024,
            used_bytes: 8 * 1024 * 1024 * 1024,
            available_bytes: 8 * 1024 * 1024 * 1024,
            usage_percent: 50.0,
            swap_total_bytes: 0,
            swap_used_bytes: 0,
        },
        disk: DiskMetrics::default(),
        network: NetworkMetrics::default(),
    };

    let backend = TestBackend::new(120, 30);
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
    // sparkline 至少应包含部分 spark 字符(▁▂▃▄▅▆▇█)
    let spark_chars = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let spark_count = spark_chars.iter().filter(|c| content.contains(**c)).count();
    assert!(
        spark_count >= 3,
        "趋势图应使用 sparkline 字符(▁-█),实际找到 {spark_count} 种"
    );
}

// ============================================================
// 2. 阈值告警颜色 — Critical (>= 90%) 测试
// ============================================================

#[test]
fn test_threshold_alert_color_critical_above_90() {
    // CPU 95% 触发 Critical — 红色
    let level = ThresholdLevel::classify(95.0);
    assert_eq!(level, ThresholdLevel::Critical);

    let mut panel = ResourceMonitorPanel::with_trends(true);
    let mut state = TuiState::new();
    state.sys_metrics = SystemMetrics {
        cpu: CpuMetrics {
            global_usage: 95.0,
            per_core_usage: vec![95.0, 93.0, 97.0],
            core_count: 3,
        },
        memory: MemMetrics::default(),
        disk: DiskMetrics::default(),
        network: NetworkMetrics::default(),
    };
    state.sys_metrics_history = vec![900; 64]; // 历史也是 90+

    let backend = TestBackend::new(80, 20);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            let area = f.area();
            panel.render(&state, area, f.buffer_mut());
        })
        .unwrap();

    // Critical 级别应生成 Color::Red 样式的文本
    let buffer = terminal.backend().buffer();
    let mut has_red = false;
    for cell in buffer.content().iter() {
        if cell.fg == ratatui::style::Color::Red
            && cell.symbol() != " "
            && !cell.symbol().is_empty()
        {
            has_red = true;
            break;
        }
    }
    assert!(has_red, "CPU > 90% 时,Critical 级别应使用 Color::Red 渲染");
}

// ============================================================
// 3. 阈值告警颜色 — Warning (70% - 90%) 测试
// ============================================================

#[test]
fn test_threshold_alert_color_warning_between_70_90() {
    // CPU 75% 触发 Warning — 黄色
    let level = ThresholdLevel::classify(75.0);
    assert_eq!(level, ThresholdLevel::Warning);

    let mut panel = ResourceMonitorPanel::with_trends(true);
    let mut state = TuiState::new();
    state.sys_metrics = SystemMetrics {
        cpu: CpuMetrics {
            global_usage: 75.0,
            per_core_usage: vec![75.0, 73.0, 77.0],
            core_count: 3,
        },
        memory: MemMetrics::default(),
        disk: DiskMetrics::default(),
        network: NetworkMetrics::default(),
    };
    state.sys_metrics_history = vec![750; 64];

    let backend = TestBackend::new(80, 20);
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw(|f| {
            let area = f.area();
            panel.render(&state, area, f.buffer_mut());
        })
        .unwrap();

    // Warning 级别应使用 Color::Yellow
    let buffer = terminal.backend().buffer();
    let mut has_yellow = false;
    for cell in buffer.content().iter() {
        if cell.fg == ratatui::style::Color::Yellow
            && cell.symbol() != " "
            && !cell.symbol().is_empty()
        {
            has_yellow = true;
            break;
        }
    }
    assert!(
        has_yellow,
        "CPU 70-90% 时,Warning 级别应使用 Color::Yellow 渲染"
    );
}

// ============================================================
// 4. 中位数滤波平滑锯齿测试
// ============================================================

#[test]
fn test_median_filter_smooths_jitter() {
    // 构造 30 个采样点:基础值 50% + 周期小抖动 + 偶发尖峰
    // 中位数滤波(窗口 5)应能显著抑制尖峰,降低方差
    let mut history = ResourceHistory::new(30, 5);
    let base_ts = 1_000_000u64;
    // 模式:50 ± 周期 3 的小抖动 + 每 7 个一个 +30 尖峰
    // 尖峰会被中位数窗口(5 邻居)淹没,大幅降低方差
    let raw_values: Vec<f32> = (0..30)
        .map(|i| {
            let base = 50.0_f32;
            let jitter = ((i % 3) as f32 - 1.0) * 5.0; // -5, 0, 5, -5, ...
            let spike = if i % 7 == 0 && i > 0 { 30.0 } else { 0.0 };
            base + jitter + spike
        })
        .collect();
    for (i, v) in raw_values.iter().enumerate() {
        history.push(base_ts + i as u64 * 1000, *v);
    }

    let filtered = history.filtered_values();
    assert_eq!(filtered.len(), 30, "滤波后样本数应与原始一致");

    // 计算原始值与滤波值的方差,滤波后方差应显著降低
    let raw_variance = compute_variance(&raw_values);
    let filtered_variance = compute_variance(&filtered);
    assert!(
        filtered_variance < raw_variance * 0.6,
        "中位数滤波后方差应降低 ≥ 40% (raw={:.2}, filtered={:.2})",
        raw_variance,
        filtered_variance
    );

    // samples_in_window 应返回窗口内样本数
    let samples = history.samples_in_window(Duration::from_secs(60));
    assert_eq!(samples, 30, "60s 窗口内应包含全部 30 样本");
}

/// 计算 f32 向量方差(总体方差,除以 n)
fn compute_variance(values: &[f32]) -> f32 {
    if values.is_empty() {
        return 0.0;
    }
    let mean: f32 = values.iter().sum::<f32>() / values.len() as f32;
    let variance: f32 =
        values.iter().map(|v| (v - mean).powi(2)).sum::<f32>() / values.len() as f32;
    variance
}

// ============================================================
// 5. 默认值向后兼容(默认 enable_trend_charts=false)
// ============================================================

#[test]
fn test_default_panel_backward_compatible() {
    // WHY:既有 16 面板 + 4 个集成测试依赖 ResourceMonitorPanel::new() 行为
    // 默认构造器必须保留瞬时 gauge 行为,enable_trend_charts=false
    let panel = ResourceMonitorPanel::new();
    assert_eq!(panel.id(), PanelId::ResourceMonitor);
    // 趋势图默认关闭
    assert!(!panel.trends_enabled(), "默认应禁用趋势图以保持向后兼容");
}

#[test]
fn test_with_trends_true_enables_charts() {
    let panel = ResourceMonitorPanel::with_trends(true);
    assert!(panel.trends_enabled(), "with_trends(true) 应启用趋势图");
    assert_eq!(panel.id(), PanelId::ResourceMonitor);
}

#[test]
fn test_resource_history_basic_push() {
    // 边界:基础 push 与 latest_value 语义
    let mut history = ResourceHistory::new(10, 3);
    assert!(history.is_empty());

    history.push(100, 50.0);
    history.push(200, 60.0);
    history.push(300, 70.0);

    assert_eq!(history.len(), 3);
    assert_eq!(history.latest_value(), Some(70.0));
    assert_eq!(history.latest_timestamp(), Some(300));
}

#[test]
fn test_resource_history_window_bounds() {
    // FIFO 容量控制:超过 window_size 丢弃最旧
    let mut history = ResourceHistory::new(5, 3);
    for i in 0..10 {
        history.push(i as u64, i as f32);
    }
    assert_eq!(history.len(), 5, "超过 window_size 应截断");
    // 保留最后 5 个:5, 6, 7, 8, 9
    assert_eq!(history.latest_value(), Some(9.0));
}

#[test]
fn test_threshold_classify_boundaries() {
    // 边界值测试:70/90 是 Warning/Critical 切换点
    assert_eq!(ThresholdLevel::classify(0.0), ThresholdLevel::Normal);
    assert_eq!(ThresholdLevel::classify(69.9), ThresholdLevel::Normal);
    assert_eq!(ThresholdLevel::classify(70.0), ThresholdLevel::Warning);
    assert_eq!(ThresholdLevel::classify(89.9), ThresholdLevel::Warning);
    assert_eq!(ThresholdLevel::classify(90.0), ThresholdLevel::Critical);
    assert_eq!(ThresholdLevel::classify(100.0), ThresholdLevel::Critical);
}
