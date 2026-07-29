//! 策略抖振检测器 — 监测 GSOE ↔ Quest 逻辑循环的策略震荡(P2-14)
//!
//! 对应架构层:L9 Quest
//!
//! # 背景(WHY)
//! 三重悖论"推理悖论红线"(见 `AI_Agent_三重悖论_x_Chimera_深度映射分析.md`)
//! 指出:10 层架构的跨层协调成本存在"推理悖论阈值"——当协调成本超过推理增益时,
//! 多 Agent 反而不如单 Agent。GSOE(L5)与 Quest(L9)通过 Event Bus 形成隐式循环:
//!
//! ```text
//! GSOE 发布 GsoePolicyUpdated ──→ Quest/Parliament 调整思考模式
//!   ↑                                        │
//!   └── Quest 发布 ThinkingModeSwitched ────┘
//! ```
//!
//! 当系统陷入"策略抖振"(Policy Oscillation)——短时间内反复切换策略,
//! 协调成本超过推理增益,系统效率下降而非提升。
//!
//! # 检测机制
//! - **滑动窗口**:跟踪最近 N 秒内的 `GsoePolicyUpdated` 和 `ThinkingModeSwitched` 事件
//! - **频率阈值**:时间窗口内事件数超过阈值视为高频切换
//! - **震荡模式检测**:统计 `(from_mode, to_mode)` 对的出现次数,
//!   某对出现 ≥3 次判定为震荡(如 Fast↔Deep 反复切换)
//! - **复合严重度**:基于震荡对数和切换频率计算 0.0-1.0 的严重度
//!
//! # 学术支撑
//! - 推理悖论阈值概念源自 `AI_Agent_三重悖论_x_Chimera_深度映射分析.md` §2
//! - 策略震荡(Policy Oscillation)在强化学习中称为"策略抖振"(Policy Churn),
//!   参考 Sutton & Barto *Reinforcement Learning* 2nd ed. §13.5
//! - 滑动窗口统计是流式异常检测的标准方法,参考 Aggarwal *Outlier Analysis* §4

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use crate::types::MetricSample;
use chrono::Utc;
use event_bus::NexusEvent;

/// 默认滑动窗口长度:60 秒
///
/// WHY 60s:平衡检测灵敏度与误报率。过短(如 10s)会因正常波动触发告警,
/// 过长(如 300s)会延迟抖振检测。60s 足以捕获 3+ 次完整震荡周期。
pub const DEFAULT_WINDOW_SECS: u64 = 60;

/// 默认震荡对阈值:时间窗口内相同 (from, to) 对出现 ≥3 次判定为震荡
///
/// WHY 3 次:2 次可能是正常切换(A→B→A 表示任务复杂度变化),
/// 3 次以上(A→B→A→B→A)几乎必然是策略抖振。
pub const DEFAULT_OSCILLATION_THRESHOLD: usize = 3;

/// 默认高频切换阈值:时间窗口内 TTG 切换次数 ≥5 视为高频
///
/// WHY 5 次:正常工作流中 60s 内 1-2 次切换是合理的(任务复杂度变化),
/// 5 次以上表明系统在反复调整,可能陷入抖振。
pub const DEFAULT_HIGH_FREQ_THRESHOLD: usize = 5;

/// 抖振严重度告警阈值:超过此值应触发 Warning 告警
pub const DEFAULT_SEVERITY_ALERT_THRESHOLD: f64 = 0.7;

/// 一条 `ThinkingModeSwitched` 事件的精简记录
///
/// 仅保留抖振检测所需字段,避免持有完整 NexusEvent 的内存开销。
#[derive(Debug, Clone, PartialEq, Eq)]
struct TtgSwitchRecord {
    /// 切换发生时刻(单调时钟,不受系统时钟调整影响)
    timestamp: Instant,
    /// 源思考模式(如 "Fast")
    from_mode: String,
    /// 目标思考模式(如 "Deep")
    to_mode: String,
}

/// 一条 `GsoePolicyUpdated` 事件的精简记录
///
/// WHY 保留 generation/improvement 字段:虽当前抖振检测仅基于事件频率,
/// 但未来可基于改进幅度(improvement 趋负 + 频率高 = 抖振信号更强)
/// 和世代数(generation 增长但 improvement 不增 = 进化停滞)做更精细
/// 的诊断。字段保留避免后续破坏性 schema 变更。
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct GsoeUpdateRecord {
    /// 更新发生时刻(单调时钟)
    timestamp: Instant,
    /// 进化世代数
    generation: u64,
    /// 相对上一代的改进幅度
    improvement: f32,
}

/// 策略抖振检测结果
///
/// 由 [`PolicyOscillationDetector::detect`] 返回,包含抖振检测的完整诊断信息。
#[derive(Debug, Clone, PartialEq)]
pub struct OscillationReport {
    /// 时间窗口内 GSOE 策略更新次数
    pub gsoe_updates_in_window: usize,
    /// 时间窗口内 TTG 切换次数
    pub ttg_switches_in_window: usize,
    /// 检测到的震荡对数(from_mode, to_mode)出现 ≥3 次的对数
    pub oscillation_pairs: usize,
    /// 检测到的所有震荡模式列表,如 [("Fast", "Deep", 3), ("Deep", "Fast", 4)]
    pub oscillation_patterns: Vec<(String, String, usize)>,
    /// 抖振严重度 ∈ [0.0, 1.0]
    ///
    /// 计算公式(复合指标):
    /// - 基础分 = min(oscillation_pairs / 3.0, 1.0) * 0.6
    /// - 频率分 = min(ttg_switches / high_freq_threshold, 1.0) * 0.4
    /// - severity = min(基础分 + 频率分, 1.0)
    pub severity: f64,
    /// 是否触发告警(severity > alert_threshold)
    pub should_alert: bool,
}

impl OscillationReport {
    /// 生成"无抖振"的默认报告(所有计数为 0,severity = 0.0)
    ///
    /// WHY 保留:面向未来的 API 扩展点,当需要返回"检测器刚初始化"的
    /// 默认状态时使用。当前 `detect()` 直接构造报告,但未来若添加
    /// 增量检测或快照恢复功能,此构造器可作为语义清晰的默认值。
    #[allow(dead_code)]
    fn no_oscillation() -> Self {
        Self {
            gsoe_updates_in_window: 0,
            ttg_switches_in_window: 0,
            oscillation_pairs: 0,
            oscillation_patterns: Vec::new(),
            severity: 0.0,
            should_alert: false,
        }
    }
}

/// 策略抖振检测器 — 监测 GSOE ↔ Quest 逻辑循环的策略震荡
///
/// # 设计决策(WHY)
/// - **`Instant` 单调时钟**:不受系统时钟调整影响,适合测量时间间隔
/// - **`VecDeque` 滑动窗口**:O(1) push/pop,过期清理在 `detect` 时惰性执行
/// - **Clone 共享状态**:基于 `Arc<Mutex<>>` 可在 `tokio::spawn` 后台任务中
///   与主线程共享(遵循 §4.4 反模式 #1:锁内不 await,`detect` 是同步方法)
/// - **不持久化**:抖振检测是运行时诊断,重启后历史无意义
///
/// # 线程安全
/// 内部状态使用 `std::sync::Mutex` 保护。所有公共方法都是同步的(非 async),
/// 适合在 `handle_broadcast_event` 中直接调用。遵循 §4.4 反模式 #1:
/// Mutex 写锁在方法返回时自动释放,不跨 `.await`。
#[derive(Debug)]
pub struct PolicyOscillationDetector {
    /// TTG 切换事件滑动窗口(按时间顺序排列)
    ttg_switches: std::sync::Mutex<VecDeque<TtgSwitchRecord>>,
    /// GSOE 策略更新事件滑动窗口
    gsoe_updates: std::sync::Mutex<VecDeque<GsoeUpdateRecord>>,
    /// 配置参数
    config: OscillationConfig,
}

/// 策略抖振检测器配置
#[derive(Debug, Clone)]
pub struct OscillationConfig {
    /// 滑动窗口长度
    pub window: Duration,
    /// 震荡对阈值:相同 (from, to) 对出现 ≥此值判定为震荡
    pub oscillation_threshold: usize,
    /// 高频切换阈值:时间窗口内 TTG 切换次数 ≥此值视为高频
    pub high_freq_threshold: usize,
    /// 严重度告警阈值:超过此值 should_alert = true
    pub severity_alert_threshold: f64,
}

impl Default for OscillationConfig {
    fn default() -> Self {
        Self {
            window: Duration::from_secs(DEFAULT_WINDOW_SECS),
            oscillation_threshold: DEFAULT_OSCILLATION_THRESHOLD,
            high_freq_threshold: DEFAULT_HIGH_FREQ_THRESHOLD,
            severity_alert_threshold: DEFAULT_SEVERITY_ALERT_THRESHOLD,
        }
    }
}

impl PolicyOscillationDetector {
    /// 创建策略抖振检测器,使用默认配置
    pub fn new() -> Self {
        Self::with_config(OscillationConfig::default())
    }

    /// 创建策略抖振检测器,使用自定义配置
    pub fn with_config(config: OscillationConfig) -> Self {
        Self {
            ttg_switches: std::sync::Mutex::new(VecDeque::new()),
            gsoe_updates: std::sync::Mutex::new(VecDeque::new()),
            config,
        }
    }

    /// 记录一个事件,如果是 TTG 切换或 GSOE 更新则加入滑动窗口
    ///
    /// 该方法是同步的,可在 `handle_broadcast_event` 中直接调用。
    /// 遵循 §4.4 反模式 #1:Mutex 写锁在方法返回时自动释放,不跨 `.await`。
    pub fn record_event(&self, event: &NexusEvent) {
        match event {
            NexusEvent::ThinkingModeSwitched {
                from_mode, to_mode, ..
            } => {
                let record = TtgSwitchRecord {
                    timestamp: Instant::now(),
                    from_mode: from_mode.clone(),
                    to_mode: to_mode.clone(),
                };
                // WHY unwrap_or_else:Mutex 中毒意味着另一线程 panic,
                // 此时抖振检测已无意义,降级取用中毒数据继续运行
                let mut guard = self.ttg_switches.lock().unwrap_or_else(|e| e.into_inner());
                guard.push_back(record);
            }
            NexusEvent::GsoePolicyUpdated {
                generation,
                improvement,
                ..
            } => {
                let record = GsoeUpdateRecord {
                    timestamp: Instant::now(),
                    generation: *generation,
                    improvement: *improvement,
                };
                let mut guard = self.gsoe_updates.lock().unwrap_or_else(|e| e.into_inner());
                guard.push_back(record);
            }
            _ => {
                // 其他事件不影响抖振检测,忽略
            }
        }
    }

    /// 执行抖振检测,返回当前时间窗口的检测报告
    ///
    /// 该方法会惰性清理滑动窗口中过期的事件。
    pub fn detect(&self) -> OscillationReport {
        let now = Instant::now();
        let window = self.config.window;

        // 清理并统计 TTG 切换窗口
        let ttg_patterns = {
            let mut guard = self.ttg_switches.lock().unwrap_or_else(|e| e.into_inner());
            // 惰性清理过期记录
            while let Some(front) = guard.front() {
                if now.duration_since(front.timestamp) > window {
                    guard.pop_front();
                } else {
                    break;
                }
            }
            // 统计 (from_mode, to_mode) 对的出现次数
            // WHY 使用 Vec 而非 HashMap:窗口内记录数通常 <50,
            // Vec 线性扫描足够快且无堆分配开销
            let mut patterns: Vec<((String, String), usize)> = Vec::new();
            for record in guard.iter() {
                let key = (record.from_mode.clone(), record.to_mode.clone());
                if let Some((_, count)) = patterns.iter_mut().find(|(k, _)| *k == key) {
                    *count += 1;
                } else {
                    patterns.push((key, 1));
                }
            }
            patterns
        };

        // 清理并统计 GSOE 更新窗口
        let gsoe_count = {
            let mut guard = self.gsoe_updates.lock().unwrap_or_else(|e| e.into_inner());
            while let Some(front) = guard.front() {
                if now.duration_since(front.timestamp) > window {
                    guard.pop_front();
                } else {
                    break;
                }
            }
            guard.len()
        };

        let ttg_count: usize = ttg_patterns.iter().map(|(_, c)| *c).sum();

        // 识别震荡对:出现次数 ≥ oscillation_threshold 的 (from, to) 对
        let oscillation_patterns: Vec<(String, String, usize)> = ttg_patterns
            .iter()
            .filter(|(_, count)| *count >= self.config.oscillation_threshold)
            .map(|((from, to), count)| (from.clone(), to.clone(), *count))
            .collect();
        let oscillation_pairs = oscillation_patterns.len();

        // 计算复合严重度(0.0-1.0)
        //
        // 基础分(60%):震荡对数越多越严重,3 对即满分
        let base_score = (oscillation_pairs as f64 / 3.0).min(1.0) * 0.6;
        // 频率分(40%):TTG 切换频率越高越严重
        let freq_score = (ttg_count as f64 / self.config.high_freq_threshold as f64).min(1.0) * 0.4;
        let severity = (base_score + freq_score).min(1.0);

        let should_alert = severity >= self.config.severity_alert_threshold;

        OscillationReport {
            gsoe_updates_in_window: gsoe_count,
            ttg_switches_in_window: ttg_count,
            oscillation_pairs,
            oscillation_patterns,
            severity,
            should_alert,
        }
    }

    /// 采集抖振检测指标,供 Prometheus /metrics 输出
    ///
    /// 返回的 `MetricSample` 列表包含:
    /// - `policy_oscillation_gsoe_updates_in_window`:窗口内 GSOE 更新次数
    /// - `policy_oscillation_ttg_switches_in_window`:窗口内 TTG 切换次数
    /// - `policy_oscillation_oscillation_pairs`:震荡对数
    /// - `policy_oscillation_severity`:抖振严重度
    pub fn collect_metrics(&self) -> Vec<MetricSample> {
        let report = self.detect();
        let now = Utc::now();

        vec![
            MetricSample {
                name: "policy_oscillation_gsoe_updates_in_window".to_string(),
                value: report.gsoe_updates_in_window as f64,
                labels: vec![],
                timestamp: now,
            },
            MetricSample {
                name: "policy_oscillation_ttg_switches_in_window".to_string(),
                value: report.ttg_switches_in_window as f64,
                labels: vec![],
                timestamp: now,
            },
            MetricSample {
                name: "policy_oscillation_oscillation_pairs".to_string(),
                value: report.oscillation_pairs as f64,
                labels: vec![],
                timestamp: now,
            },
            MetricSample {
                name: "policy_oscillation_severity".to_string(),
                value: report.severity,
                labels: vec![],
                timestamp: now,
            },
        ]
    }

    /// 获取配置引用
    pub fn config(&self) -> &OscillationConfig {
        &self.config
    }

    /// 清空滑动窗口(主要用于测试)
    #[cfg(test)]
    pub fn clear(&self) {
        self.ttg_switches
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
        self.gsoe_updates
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clear();
    }
}

impl Default for PolicyOscillationDetector {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use event_bus::EventMetadata;
    use std::thread;

    fn make_ttg_switch(from: &str, to: &str) -> NexusEvent {
        NexusEvent::ThinkingModeSwitched {
            metadata: EventMetadata::new("test"),
            quest_id: "q-test".into(),
            from_mode: from.into(),
            to_mode: to.into(),
            reason: "test".into(),
        }
    }

    fn make_gsoe_update(gen: u64, imp: f32) -> NexusEvent {
        NexusEvent::GsoePolicyUpdated {
            metadata: EventMetadata::new("test"),
            generation: gen,
            improvement: imp,
            new_mutation_rate: 0.1,
            new_selection_pressure: 0.5,
        }
    }

    fn make_cache_hit() -> NexusEvent {
        NexusEvent::CacheHit {
            metadata: EventMetadata::new("test"),
            cache_key: "k-1".into(),
        }
    }

    #[test]
    fn test_new_detector_has_empty_windows() {
        let detector = PolicyOscillationDetector::new();
        let report = detector.detect();
        assert_eq!(report.gsoe_updates_in_window, 0);
        assert_eq!(report.ttg_switches_in_window, 0);
        assert_eq!(report.oscillation_pairs, 0);
        assert_eq!(report.severity, 0.0);
        assert!(!report.should_alert);
    }

    #[test]
    fn test_record_ttg_switch_increments_count() {
        let detector = PolicyOscillationDetector::new();
        detector.record_event(&make_ttg_switch("Fast", "Deep"));
        detector.record_event(&make_ttg_switch("Deep", "Standard"));

        let report = detector.detect();
        assert_eq!(report.ttg_switches_in_window, 2);
        assert_eq!(report.gsoe_updates_in_window, 0);
    }

    #[test]
    fn test_record_gsoe_update_increments_count() {
        let detector = PolicyOscillationDetector::new();
        detector.record_event(&make_gsoe_update(1, 0.05));
        detector.record_event(&make_gsoe_update(2, 0.03));
        detector.record_event(&make_gsoe_update(3, 0.01));

        let report = detector.detect();
        assert_eq!(report.gsoe_updates_in_window, 3);
        assert_eq!(report.ttg_switches_in_window, 0);
    }

    #[test]
    fn test_other_events_ignored() {
        let detector = PolicyOscillationDetector::new();
        detector.record_event(&make_cache_hit());
        detector.record_event(&make_cache_hit());

        let report = detector.detect();
        assert_eq!(report.ttg_switches_in_window, 0);
        assert_eq!(report.gsoe_updates_in_window, 0);
    }

    #[test]
    fn test_oscillation_pair_detected() {
        // 3 次 Fast→Deep + 2 次 Deep→Fast + 1 次 Standard→Deep
        // Fast→Deep 出现 3 次 ≥ threshold(3),应检测为震荡对
        let detector = PolicyOscillationDetector::new();
        detector.record_event(&make_ttg_switch("Fast", "Deep"));
        detector.record_event(&make_ttg_switch("Deep", "Fast"));
        detector.record_event(&make_ttg_switch("Fast", "Deep"));
        detector.record_event(&make_ttg_switch("Deep", "Fast"));
        detector.record_event(&make_ttg_switch("Fast", "Deep"));
        detector.record_event(&make_ttg_switch("Standard", "Deep"));

        let report = detector.detect();
        // Fast→Deep 出现 3 次(≥3,震荡对)
        // Deep→Fast 出现 2 次(<3,非震荡对)
        // Standard→Deep 出现 1 次(<3,非震荡对)
        assert_eq!(report.ttg_switches_in_window, 6);
        assert_eq!(report.oscillation_pairs, 1);
        assert!(report
            .oscillation_patterns
            .iter()
            .any(|(from, to, count)| from == "Fast" && to == "Deep" && *count == 3));
    }

    #[test]
    fn test_multiple_oscillation_pairs_detected() {
        // 3 次 Fast↔Deep + 3 次 Standard↔Deep = 2 个震荡对
        let detector = PolicyOscillationDetector::new();
        for _ in 0..3 {
            detector.record_event(&make_ttg_switch("Fast", "Deep"));
            detector.record_event(&make_ttg_switch("Deep", "Fast"));
        }
        for _ in 0..3 {
            detector.record_event(&make_ttg_switch("Standard", "Deep"));
            detector.record_event(&make_ttg_switch("Deep", "Standard"));
        }

        let report = detector.detect();
        assert_eq!(report.ttg_switches_in_window, 12);
        // Fast→Deep(3), Deep→Fast(3), Standard→Deep(3), Deep→Standard(3)
        // 4 个 (from,to) 对各出现 3 次 ≥ threshold,故 4 个震荡对
        assert_eq!(report.oscillation_pairs, 4);
    }

    #[test]
    fn test_severity_increases_with_oscillation() {
        let detector = PolicyOscillationDetector::new();

        // 无抖振:severity = 0.0
        let r1 = detector.detect();
        assert_eq!(r1.severity, 0.0);

        // 1 个震荡对 + 3 次切换
        for _ in 0..3 {
            detector.record_event(&make_ttg_switch("Fast", "Deep"));
        }
        let r2 = detector.detect();
        // 基础分 = min(1/3, 1.0) * 0.6 = 0.2
        // 频率分 = min(3/5, 1.0) * 0.4 = 0.24
        // severity = 0.44
        assert!(r2.severity > 0.0);
        assert!(r2.severity < 1.0);

        // 清空后大量震荡:severity 接近 1.0
        detector.clear();
        for _ in 0..10 {
            detector.record_event(&make_ttg_switch("Fast", "Deep"));
            detector.record_event(&make_ttg_switch("Deep", "Fast"));
        }
        let r3 = detector.detect();
        // Fast→Deep(10), Deep→Fast(10) = 2 个震荡对
        // 基础分 = min(2/3, 1.0) * 0.6 = 0.4
        // 频率分 = min(20/5, 1.0) * 0.4 = 0.4
        // severity = 0.8
        assert!(r3.severity > r2.severity);
        assert!(r3.should_alert); // 0.8 > 0.7 阈值
    }

    #[test]
    fn test_should_alert_triggers_at_threshold() {
        let detector = PolicyOscillationDetector::new();

        // 严重度 0.0,不应告警
        let r = detector.detect();
        assert!(!r.should_alert);

        // 制造足够多震荡触发告警
        // 3 个震荡对 + 20 次切换
        for _ in 0..7 {
            detector.record_event(&make_ttg_switch("Fast", "Deep"));
        }
        for _ in 0..7 {
            detector.record_event(&make_ttg_switch("Deep", "Standard"));
        }
        for _ in 0..7 {
            detector.record_event(&make_ttg_switch("Standard", "Fast"));
        }
        let r = detector.detect();
        // 3 个震荡对(Fast→Deep, Deep→Standard, Standard→Fast 各 7 次)
        // 基础分 = min(3/3, 1.0) * 0.6 = 0.6
        // 频率分 = min(21/5, 1.0) * 0.4 = 0.4
        // severity = 1.0
        assert_eq!(r.oscillation_pairs, 3);
        assert!((r.severity - 1.0).abs() < 1e-6);
        assert!(r.should_alert);
    }

    #[test]
    fn test_window_expiration() {
        // 使用极短窗口(10ms)测试过期清理
        let config = OscillationConfig {
            window: Duration::from_millis(10),
            ..OscillationConfig::default()
        };
        let detector = PolicyOscillationDetector::with_config(config);

        detector.record_event(&make_ttg_switch("Fast", "Deep"));
        assert_eq!(detector.detect().ttg_switches_in_window, 1);

        // 等待窗口过期
        thread::sleep(Duration::from_millis(20));

        // 检测时应清理过期记录
        let report = detector.detect();
        assert_eq!(report.ttg_switches_in_window, 0);
    }

    #[test]
    fn test_collect_metrics_returns_all_samples() {
        let detector = PolicyOscillationDetector::new();
        detector.record_event(&make_ttg_switch("Fast", "Deep"));
        detector.record_event(&make_gsoe_update(1, 0.05));

        let samples = detector.collect_metrics();
        assert_eq!(samples.len(), 4);
        assert!(samples
            .iter()
            .any(|s| s.name == "policy_oscillation_gsoe_updates_in_window"));
        assert!(samples
            .iter()
            .any(|s| s.name == "policy_oscillation_ttg_switches_in_window"));
        assert!(samples
            .iter()
            .any(|s| s.name == "policy_oscillation_oscillation_pairs"));
        assert!(samples
            .iter()
            .any(|s| s.name == "policy_oscillation_severity"));
    }

    #[test]
    fn test_clone_config_independent() {
        // 验证配置 Clone 后修改不影响原配置
        let config1 = OscillationConfig::default();
        let mut config2 = config1.clone();
        config2.high_freq_threshold = 10;

        assert_eq!(config1.high_freq_threshold, DEFAULT_HIGH_FREQ_THRESHOLD);
        assert_eq!(config2.high_freq_threshold, 10);
    }

    #[test]
    fn test_concurrent_access_safe() {
        // 验证多线程并发访问不会 panic
        // WHY: 抖振检测器可能被后台订阅任务和主线程同时访问
        use std::sync::Arc;
        let detector = Arc::new(PolicyOscillationDetector::new());
        let detector_clone = Arc::clone(&detector);

        let handle = thread::spawn(move || {
            for _ in 0..10 {
                detector_clone.record_event(&make_ttg_switch("Fast", "Deep"));
            }
        });

        for _ in 0..10 {
            detector.record_event(&make_ttg_switch("Deep", "Fast"));
        }

        handle.join().expect("子线程不应 panic");

        let report = detector.detect();
        assert_eq!(report.ttg_switches_in_window, 20);
    }

    #[test]
    fn test_no_oscillation_below_threshold() {
        // 2 次相同切换(< threshold=3),不应判定为震荡对
        let detector = PolicyOscillationDetector::new();
        detector.record_event(&make_ttg_switch("Fast", "Deep"));
        detector.record_event(&make_ttg_switch("Fast", "Deep"));

        let report = detector.detect();
        assert_eq!(report.ttg_switches_in_window, 2);
        assert_eq!(report.oscillation_pairs, 0);
        // 频率分 = min(2/5, 1.0) * 0.4 = 0.16
        assert!(report.severity < 0.7);
        assert!(!report.should_alert);
    }
}
