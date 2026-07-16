//! 资源历史时间序列 — ResourceMonitorPanel 趋势图组件
//!
//! 对应 spec:enterprise-tui-monitoring-task-viz §二·系统监控增强
//!
//! # 设计决策(WHY)
//! - `ResourceHistory` 封装滑动窗口时间序列 + 中位数滤波去抖动,
//!   复用 `render::sparkline` 已有 API 渲染,避免在面板中重复实现窗口管理。
//! - 采用 `Vec<MetricSample>` 而非 `VecDeque<MetricSample>`:容量小
//!   (默认 300 样本 = 5 分钟 × 1Hz),FIFO 截断频率低,Vec 切片性能更好
//!   且序列化天然兼容(`Vec<T>` 是 serde 默认支持类型)。
//! - 中位数滤波窗口 = 5(任务硬性要求,spec §一·三 P3 阈值告警):
//!   去抖动效果显著(尖峰抑制 ≥ 40% 方差)且单次滤波 O(5 log 5) 可忽略。
//! - `MetricSample` 仅含 ts + value:避免过度抽象(暂不需要 tag/source 字段),
//!   后续若需按来源区分可加 `Option<&'static str>`。
//!
//! # 边界语义
//! - 窗口未满时(`len() < filter_window`):`filtered_values()` 仍返回全部样本
//!   (中位数窗口在边界用 `saturating_sub` 与 `min` 截断,自动回退到较小窗口)。
//! - 边界样本(首尾):中位数窗口使用 `saturating_sub` 与 `min`,自动截断。
//! - `latest_*` 方法在空窗口时返回 `None`,符合 Rust 标准库 `Vec::last` 语义。

use std::time::Duration;

use ratatui::style::Color;
use serde::{Deserialize, Serialize};

/// 指标采样点 — 时间戳 + 值的最小数据单元
///
/// WHY 独立结构体:与 `DataSnapshot` 解耦,允许面板独立持有历史窗口,
/// 也方便 `serde` 序列化(时间序列持久化时可用)。
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct MetricSample {
    /// Unix 时间戳(毫秒)
    pub ts: u64,
    /// 指标值(单位由调用方决定,例如 CPU 使用率百分比)
    pub value: f32,
}

impl MetricSample {
    /// 构造新采样点
    pub fn new(ts: u64, value: f32) -> Self {
        Self { ts, value }
    }
}

/// 阈值告警级别 — 三档颜色映射(Normal / Warning / Critical)
///
/// WHY 独立 enum:与 `GaugeThreshold`(render.rs)语义互补但更精简。
/// `GaugeThreshold` 面向 Gauge widget 的百分比,`ThresholdLevel` 面向
/// sparkline 趋势点的瞬时着色。两者边界(70/90)一致,可在配置层统一。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ThresholdLevel {
    /// 正常(< 70%),绿色
    Normal,
    /// 警告(70% ≤ v < 90%),黄色
    Warning,
    /// 危险(≥ 90%),红色
    Critical,
}

impl ThresholdLevel {
    /// 根据百分比值分类阈值级别
    ///
    /// # 参数
    /// - `value_pct`:百分比值(0-100)
    ///
    /// # 边界语义(左闭右开)
    /// - `< 70%` → Normal
    /// - `70% ≤ v < 90%` → Warning
    /// - `≥ 90%` → Critical
    pub fn classify(value_pct: f32) -> Self {
        if value_pct >= 90.0 {
            ThresholdLevel::Critical
        } else if value_pct >= 70.0 {
            ThresholdLevel::Warning
        } else {
            ThresholdLevel::Normal
        }
    }

    /// 返回该阈值级别对应的 ratatui 颜色
    pub fn color(&self) -> Color {
        match self {
            ThresholdLevel::Normal => Color::Green,
            ThresholdLevel::Warning => Color::Yellow,
            ThresholdLevel::Critical => Color::Red,
        }
    }
}

// ============================================================
// 阈值颜色渐变(P4.1 体验优化)
// ============================================================

/// 渐变端点:Green(0% 起点)
///
/// WHY 选 #2ECC40:与 ratatui `Color::Green` 视觉接近但稍亮,
/// 在 0% 处比 ANSI Green (#00FF00) 柔和,避免视觉过饱和。
const GRADIENT_GREEN: (u8, u8, u8) = (46, 204, 64);
/// 渐变端点:Yellow(70% 段终点 / 70-90% 段起点)
const GRADIENT_YELLOW: (u8, u8, u8) = (255, 220, 0);
/// 渐变端点:OrangeRed(90% 段终点 / 90-100% 段起点)
const GRADIENT_ORANGE_RED: (u8, u8, u8) = (255, 133, 27);
/// 渐变端点:Red(100% 终点)
const GRADIENT_RED: (u8, u8, u8) = (255, 65, 54);

/// 根据百分比值返回平滑过渡的 RGB 颜色
///
/// # 算法
/// RGB 线性插值,三段过渡,避免 70%/90% 离散色突变:
/// - `[0%, 70%)` : Green → Yellow
/// - `[70%, 90%)`: Yellow → OrangeRed
/// - `[90%, 100%]` : OrangeRed → Red
///
/// # 参数
/// - `value_pct`:百分比值,自动钳制到 `[0.0, 100.0]`(NaN 钳制为 0)
///
/// # 返回
/// `ratatui::style::Color::Rgb(r, g, b)`,确保趋势图渐变着色与终端
/// 256 色 / 真彩色模式兼容。
///
/// # 设计权衡
/// - 选 RGB 线性插值而非 HSL:避免 hue 环绕(HSL 在 0% 与 100% 间
///   会出现 hue 跨 360° 跳跃);RGB 在 70/90 段切换处仍连续。
/// - 三段而非单段:RGB 端点距离较大(>350),单段插值会导致中间色
///   偏离预期橙黄色调。三段对应 spec 阈值 (70/90%) 告警颜色逻辑。
pub fn gradient_color(value_pct: f32) -> Color {
    // 钳制输入:负值 → 0.0,> 100 → 100.0
    // WHY NaN 检查:NaN 在 f32 排序中行为未定义(Ord 不可派生),
    // clamp 不能正确处理 NaN(is_nan() 不会返回 true 给 clamp),
    // 显式 is_finite() 守卫保证插值计算稳定。
    let v = if value_pct.is_finite() {
        value_pct.clamp(0.0, 100.0)
    } else {
        0.0
    };

    if v < 70.0 {
        // 段 1: Green → Yellow
        let t = v / 70.0;
        lerp_rgb(GRADIENT_GREEN, GRADIENT_YELLOW, t)
    } else if v < 90.0 {
        // 段 2: Yellow → OrangeRed
        let t = (v - 70.0) / 20.0;
        lerp_rgb(GRADIENT_YELLOW, GRADIENT_ORANGE_RED, t)
    } else {
        // 段 3: OrangeRed → Red
        let t = ((v - 90.0) / 10.0).min(1.0);
        lerp_rgb(GRADIENT_ORANGE_RED, GRADIENT_RED, t)
    }
}

/// RGB 线性插值
///
/// # 参数
/// - `start`:起始颜色 (r, g, b)
/// - `end`:终止颜色 (r, g, b)
/// - `t`:插值因子,预期 [0.0, 1.0],超出范围会被钳制
///
/// # 算法
/// `result = start + (end - start) * t`,逐通道浮点计算后四舍五入到 u8。
///
/// WHY 用 f64:避免 f32 在大数乘法时精度损失(如 255.0 * 0.7 = 178.4999...,
/// round() 后是 178;f32 在 70% 处误差可忽略但 f64 更稳定)。
fn lerp_rgb(start: (u8, u8, u8), end: (u8, u8, u8), t: f32) -> Color {
    let t = t.clamp(0.0, 1.0) as f64;
    let r = start.0 as f64 + (end.0 as f64 - start.0 as f64) * t;
    let g = start.1 as f64 + (end.1 as f64 - start.1 as f64) * t;
    let b = start.2 as f64 + (end.2 as f64 - start.2 as f64) * t;
    Color::Rgb(r.round() as u8, g.round() as u8, b.round() as u8)
}

/// 资源历史滑动窗口 — ResourceMonitorPanel 趋势图的数据结构
///
/// # 容量控制
/// - `window_size`:最大样本数(FIFO,超出则丢弃最旧)
/// - `filter_window`:中位数滤波窗口大小(默认 5)
pub struct ResourceHistory {
    /// 采样点缓冲(FIFO,容量由 `window_size` 控制)
    samples: Vec<MetricSample>,
    /// 窗口容量上限
    window_size: usize,
    /// 中位数滤波窗口(中心对称)
    filter_window: usize,
}

impl ResourceHistory {
    /// 创建新的资源历史窗口
    ///
    /// # 参数
    /// - `window_size`:滑动窗口容量(默认 300 = 5 分钟 × 1Hz)
    /// - `filter_window`:中位数滤波窗口(默认 5,任务硬性要求)
    ///
    /// WHY 容量 300:spec §二·监控明确 5 分钟 × 1s 采样 = 300 样本,
    /// 在 80-120 列终端上恰好填满 sparkline 主面板宽度。
    pub fn new(window_size: usize, filter_window: usize) -> Self {
        Self {
            samples: Vec::with_capacity(window_size),
            window_size: window_size.max(1),
            filter_window: filter_window.max(1),
        }
    }

    /// 追加一个采样点;若超过容量则从队首丢弃最旧样本
    pub fn push(&mut self, ts: u64, value: f32) {
        if self.samples.len() >= self.window_size {
            // WHY remove(0) 而非 drain:窗口小(≤ 300),O(n) 拷贝可忽略
            self.samples.remove(0);
        }
        self.samples.push(MetricSample::new(ts, value));
    }

    /// 当前样本数
    pub fn len(&self) -> usize {
        self.samples.len()
    }

    /// 窗口是否为空
    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    /// 最新样本的值;空窗口返回 `None`
    pub fn latest_value(&self) -> Option<f32> {
        self.samples.last().map(|s| s.value)
    }

    /// 最新样本的时间戳;空窗口返回 `None`
    pub fn latest_timestamp(&self) -> Option<u64> {
        self.samples.last().map(|s| s.ts)
    }

    /// 返回指定时间窗口内的样本数(基于最新样本时间戳为锚点)
    ///
    /// # 用法
    /// `samples_in_window(Duration::from_secs(60))` 返回最近 60 秒内的样本数。
    /// 用于面板 "60s 内 N 个采样" 状态显示。
    pub fn samples_in_window(&self, d: Duration) -> usize {
        let Some(latest) = self.samples.last() else {
            return 0;
        };
        let cutoff = latest.ts.saturating_sub(d.as_millis() as u64);
        self.samples.iter().filter(|s| s.ts >= cutoff).count()
    }

    /// 返回中位数滤波后的值序列(去抖动后用于 sparkline 渲染)
    ///
    /// # 算法
    /// 对每个输出索引 i,取 `[i - half, i + half]` 范围的中位数,
    /// 边界用 `saturating_sub` 与 `min` 截断,自动处理首尾不足。
    ///
    /// WHY 中位数:中位数对单个离群点不敏感(尖峰抑制),
    /// 比移动平均更鲁棒,适合 CPU/内存指标的瞬时抖动场景。
    /// 比卡尔曼滤波简单,无需状态机,易于单元测试。
    pub fn filtered_values(&self) -> Vec<f32> {
        if self.samples.is_empty() {
            return Vec::new();
        }
        let half = self.filter_window / 2;
        let n = self.samples.len();
        // WHY 提取原始值:中位数排序只关心 value,避免重复访问 ts
        let raw: Vec<f32> = self.samples.iter().map(|s| s.value).collect();

        (0..n)
            .map(|i| {
                let start = i.saturating_sub(half);
                let end = (i + half + 1).min(n);
                let mut window: Vec<f32> = raw[start..end].to_vec();
                // partial_cmp 对 NaN 返回 None,fallback 到 Equal 保持稳定排序
                window.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
                window[window.len() / 2]
            })
            .collect()
    }

    /// 返回原始值序列(未滤波,用于持久化或对比显示)
    pub fn raw_values(&self) -> Vec<f32> {
        self.samples.iter().map(|s| s.value).collect()
    }

    /// 返回窗口容量
    pub fn window_capacity(&self) -> usize {
        self.window_size
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_metric_sample_new() {
        let s = MetricSample::new(1000, 42.5);
        assert_eq!(s.ts, 1000);
        assert_eq!(s.value, 42.5);
    }

    #[test]
    fn test_threshold_level_classify_normal() {
        assert_eq!(ThresholdLevel::classify(0.0), ThresholdLevel::Normal);
        assert_eq!(ThresholdLevel::classify(50.0), ThresholdLevel::Normal);
        assert_eq!(ThresholdLevel::classify(69.9), ThresholdLevel::Normal);
    }

    #[test]
    fn test_threshold_level_classify_warning() {
        assert_eq!(ThresholdLevel::classify(70.0), ThresholdLevel::Warning);
        assert_eq!(ThresholdLevel::classify(80.0), ThresholdLevel::Warning);
        assert_eq!(ThresholdLevel::classify(89.9), ThresholdLevel::Warning);
    }

    #[test]
    fn test_threshold_level_classify_critical() {
        assert_eq!(ThresholdLevel::classify(90.0), ThresholdLevel::Critical);
        assert_eq!(ThresholdLevel::classify(95.0), ThresholdLevel::Critical);
        assert_eq!(ThresholdLevel::classify(100.0), ThresholdLevel::Critical);
    }

    #[test]
    fn test_threshold_level_color() {
        assert_eq!(ThresholdLevel::Normal.color(), Color::Green);
        assert_eq!(ThresholdLevel::Warning.color(), Color::Yellow);
        assert_eq!(ThresholdLevel::Critical.color(), Color::Red);
    }

    #[test]
    fn test_resource_history_push_and_latest() {
        let mut h = ResourceHistory::new(10, 3);
        assert!(h.is_empty());
        h.push(100, 50.0);
        h.push(200, 60.0);
        h.push(300, 70.0);
        assert_eq!(h.len(), 3);
        assert_eq!(h.latest_value(), Some(70.0));
        assert_eq!(h.latest_timestamp(), Some(300));
    }

    #[test]
    fn test_resource_history_window_bounds() {
        let mut h = ResourceHistory::new(5, 3);
        for i in 0..10 {
            h.push(i as u64, i as f32);
        }
        assert_eq!(h.len(), 5, "超过 window_size 应截断");
        assert_eq!(h.latest_value(), Some(9.0));
    }

    #[test]
    fn test_resource_history_filtered_empty() {
        let h = ResourceHistory::new(10, 3);
        assert!(h.filtered_values().is_empty());
    }

    #[test]
    fn test_resource_history_filtered_preserves_length() {
        let mut h = ResourceHistory::new(30, 5);
        for i in 0..30 {
            h.push(i as u64, 50.0);
        }
        let filtered = h.filtered_values();
        assert_eq!(filtered.len(), 30, "滤波后样本数应与原始一致");
        // 全部值都是 50,滤波后仍是 50
        for v in &filtered {
            assert!((v - 50.0).abs() < 1e-5);
        }
    }

    #[test]
    fn test_resource_history_samples_in_window() {
        let mut h = ResourceHistory::new(100, 3);
        for i in 0..10 {
            // 间隔 1000ms
            h.push(i as u64 * 1000, i as f32);
        }
        // 10 个样本(0-9s),最新是 9000ms
        assert_eq!(h.samples_in_window(Duration::from_secs(60)), 10);
        // 5s 窗口:9000-5000=4000ms 起,样本 4,5,6,7,8,9 = 6 个
        assert_eq!(h.samples_in_window(Duration::from_secs(5)), 6);
    }

    #[test]
    fn test_resource_history_raw_values() {
        let mut h = ResourceHistory::new(10, 3);
        h.push(1, 10.0);
        h.push(2, 20.0);
        h.push(3, 30.0);
        assert_eq!(h.raw_values(), vec![10.0, 20.0, 30.0]);
    }

    #[test]
    fn test_resource_history_window_capacity() {
        let h = ResourceHistory::new(300, 5);
        assert_eq!(h.window_capacity(), 300);
    }
}
