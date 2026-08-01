//! 悖论风险实时监控仪表盘 — 推理悖论红线实时监控
//!
//! 对应架构层:L8 Parliament
//! 对应分析:三重悖论"推理悖论红线"——当协调成本超过推理增益时,
//! 多 Agent 审议反而引入系统性风险,需实时监控三信号融合预警。
//!
//! # 设计决策(WHY)
//!
//! ## 信号融合使用简单计数而非加权
//! 三信号(ratio/否决异常率/共识健康分)使用独立阈值判断后计数融合:
//! - 单信号超标 → Yellow(预警 + 降档封顶)
//! - 两信号超标 → Red(熔断)
//! - 全正常 → Green(恢复正常)
//! 避免权重调参复杂性(权重需要大量历史数据标定,当前阶段不可行)。
//!
//! ## 通过 StrategyCapGuard 实现封顶降级,不新增独立控制器
//! StrategyCapGuard 已提供滞后带状态机驱动的审议深度封顶,悖论仪表盘
//! 在紧急响应(Yellow/Red)时直接调用 `set_max_strategy` 绕过滞后带
//! 快速响应,比新增独立控制器更轻量(复用已有组件,减少耦合)。
//!
//! ## 风险事件通过 Event Bus 发布
//! 符合 §2.2 跨层通信规范(Normal 级别,非 Critical——告警由订阅者
//! 解释,同 CoordinationRatioReported 设计原则"事件是事实,告警是解释")。
//!
//! # 三信号阈值
//! - ratio > 1.5: 协调成本显著超过推理增益
//! - veto_anomaly_rate > 0.3: 否决异常率过高(接近 1.0 表示 Skeptic 频繁否决正常提案)
//! - health_score < 40: 共识质量严重退化(健康分 0-100)

use std::time::{SystemTime, UNIX_EPOCH};

use event_bus::{EventBus, EventMetadata, NexusEvent};
use serde::{Deserialize, Serialize};
use tracing::{error, info, warn};

use crate::strategy_cap::StrategyCapGuard;

// ============================================================
// 常量
// ============================================================

/// 恢复正常检测所需的连续正常更新次数
const NORMAL_RECOVERY_COUNT: usize = 5;

/// 预警历史最大保留条数
const DEFAULT_MAX_HISTORY: usize = 100;

/// 三信号阈值(与 struct 文档一致)
const RATIO_THRESHOLD: f64 = 1.5;
const VETO_ANOMALY_RATE_THRESHOLD: f32 = 0.3;
const HEALTH_SCORE_THRESHOLD: u8 = 40;

// ============================================================
// 核心类型
// ============================================================

/// 风险等级 — 悖论风险仪表盘的三态输出
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RiskLevel {
    /// 安全:所有信号正常
    Green,
    /// 警告:单信号超标
    Yellow,
    /// 危险:两信号超标
    Red,
}

/// 预警严重级别
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AlertSeverity {
    /// 风险已解除
    Clear,
    /// 警告
    Warning,
    /// 严重
    Critical,
}

/// 悖论风险预警事件 — 通过 Event Bus 或 tracing 发布
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParadoxRiskAlert {
    /// 预警严重级别
    pub severity: AlertSeverity,
    /// 触发预警的信号列表
    pub trigger_signals: Vec<String>,
    /// 预警时间戳(ms since epoch)
    pub timestamp: u64,
    /// 当前风险等级
    pub risk_level: RiskLevel,
}

/// 悖论风险总结报告
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParadoxRiskReport {
    /// 当前风险等级
    pub current_risk_level: RiskLevel,
    /// 当前 ratio
    pub current_ratio: f64,
    /// 当前否决异常率
    pub current_veto_anomaly_rate: f32,
    /// 当前健康分
    pub current_health_score: u8,
    /// 最近预警事件数
    pub recent_alert_count: usize,
    /// 报告生成时间戳
    pub timestamp: u64,
}

// ============================================================
// 仪表盘
// ============================================================

/// 悖论风险仪表盘 — 推理悖论红线实时监控
///
/// 维护三信号状态(ratio/否决异常率/共识健康分),
/// 单信号超标 → Yellow 预警 + 降档封顶,
/// 两信号超标 → Red 熔断。
///
/// # 并发安全
/// 外部调用方通过 `Mutex<ParadoxRiskDashboard>` 保护,
/// 锁在 `update`/`risk_level`/`generate_report` 方法返回时释放,
/// 不跨 `.await` 点(§4.4 反模式 #1)。
/// 毒锁降级使用 `unwrap_or_else(|e| e.into_inner())`。
///
/// # 与 StrategyCapGuard 的关系
/// 仪表盘是**上层指挥官**,StrategyCapGuard 是**执行者**:
/// 仪表盘监测三信号融合,在紧急响应时直接调用 `set_max_strategy`
/// 绕过滞后带快速降档/熔断;StrategyCapGuard 的滞后带状态机
/// 继续处理常规 ratio 反馈,两者互补不冲突。
pub struct ParadoxRiskDashboard {
    /// 当前协调成本/推理增益比值
    ratio: f64,
    /// 否决异常率 [0.0, 1.0]
    veto_anomaly_rate: f32,
    /// 共识健康分 [0, 100],来自 ConsensusQualityMetrics
    health_score: u8,
    /// 当前风险等级
    risk_level: RiskLevel,
    /// 连续正常计数(用于恢复正常状态检测)
    consecutive_normal: usize,
    /// 事件总线引用(用于发布预警事件)
    event_bus: Option<EventBus>,
    /// 策略封顶守卫引用(用于降档/熔断)
    strategy_cap: Option<std::sync::Arc<StrategyCapGuard>>,
    /// 最近风险事件历史(用于生成报告)
    alert_history: Vec<ParadoxRiskAlert>,
    /// 最大历史记录条数
    max_history: usize,
}

impl ParadoxRiskDashboard {
    /// 创建新的悖论风险仪表盘
    ///
    /// # 参数
    /// - `event_bus`:可选的事件总线,用于发布预警事件
    /// - `strategy_cap`:可选的策略封顶守卫,用于紧急降档/熔断
    ///
    /// WHY 均为 Option:仪表盘可独立运行在测试或无 EventBus 环境,
    /// 降档/熔断行为通过 `strategy_cap` 可选注入,不强制依赖。
    pub fn new(
        event_bus: Option<EventBus>,
        strategy_cap: Option<std::sync::Arc<StrategyCapGuard>>,
    ) -> Self {
        Self {
            ratio: 0.0,
            veto_anomaly_rate: 0.0,
            health_score: 50,
            risk_level: RiskLevel::Green,
            consecutive_normal: 0,
            event_bus,
            strategy_cap,
            alert_history: Vec::new(),
            max_history: DEFAULT_MAX_HISTORY,
        }
    }

    /// 更新三信号状态,重新计算风险等级,触发预警行为
    ///
    /// # 参数
    /// - `ratio`:协调成本/推理增益比值
    /// - `veto_anomaly_rate`:否决异常率 [0.0, 1.0]
    /// - `health_score`:共识健康分 [0, 100]
    ///
    /// # 预警行为
    /// 参见模块级文档中的"三信号阈值"与 struct 文档中的预警逻辑。
    ///
    /// # 恢复正常检测
    /// 连续 `NORMAL_RECOVERY_COUNT`(5)次更新所有信号正常 → 恢复 Green。
    /// WHY 5 次:单次更新可能是瞬时抖动,连续 5 次(约 5 个审议周期)
    /// 才认定系统已稳定恢复,避免频繁闪烁。
    pub fn update(&mut self, ratio: f64, veto_anomaly_rate: f32, health_score: u8) {
        // 更新信号值
        self.ratio = ratio;
        self.veto_anomaly_rate = veto_anomaly_rate.clamp(0.0, 1.0);
        self.health_score = health_score.min(100);

        let previous_risk_level = self.risk_level;
        let new_risk_level =
            Self::compute_risk_level(self.ratio, self.veto_anomaly_rate, self.health_score);

        // 恢复正常检测:连续 N 次全正常才恢复 Green
        if new_risk_level == RiskLevel::Green {
            self.consecutive_normal += 1;
            if self.consecutive_normal >= NORMAL_RECOVERY_COUNT {
                self.risk_level = RiskLevel::Green;
            }
        } else {
            // 任一新信号超标,立即重置连续计数并更新风险等级
            self.consecutive_normal = 0;
            self.risk_level = new_risk_level;
        }

        // 风险等级变化时触发预警行为
        if self.risk_level != previous_risk_level {
            self.trigger_alert(previous_risk_level);
        }
    }

    /// 获取当前风险等级
    pub fn risk_level(&self) -> RiskLevel {
        self.risk_level
    }

    /// 静态计算风险等级——单信号超标→Yellow,两信号超标→Red,全正常→Green
    ///
    /// # 阈值定义(与模块级文档一致)
    /// - ratio > 1.5: 协调成本显著超过推理增益
    /// - veto_anomaly_rate > 0.3: 否决异常率过高
    /// - health_score < 40: 共识质量严重退化
    ///
    /// WHY 静态方法:无需仪表盘实例即可计算风险等级,
    /// 便于测试与外部调用方(Dashboard 展示)独立使用。
    pub fn compute_risk_level(ratio: f64, veto_anomaly_rate: f32, health_score: u8) -> RiskLevel {
        let mut signal_count = 0u8;

        if ratio > RATIO_THRESHOLD {
            signal_count += 1;
        }
        if veto_anomaly_rate > VETO_ANOMALY_RATE_THRESHOLD {
            signal_count += 1;
        }
        if health_score < HEALTH_SCORE_THRESHOLD {
            signal_count += 1;
        }

        match signal_count {
            0 => RiskLevel::Green,
            1 => RiskLevel::Yellow,
            _ => RiskLevel::Red, // 2 or 3 signals
        }
    }

    /// 生成悖论风险总结报告
    ///
    /// 包含当前风险等级、三信号值、最近预警数及时间戳,
    /// 供外部(Dashboard 展示/监控)快照消费。
    pub fn generate_report(&self) -> ParadoxRiskReport {
        ParadoxRiskReport {
            current_risk_level: self.risk_level,
            current_ratio: self.ratio,
            current_veto_anomaly_rate: self.veto_anomaly_rate,
            current_health_score: self.health_score,
            recent_alert_count: self.alert_history.len(),
            timestamp: current_timestamp_ms(),
        }
    }

    // ============================================================
    // 内部方法
    // ============================================================

    /// 触发预警行为——根据当前风险等级与前一等级执行响应
    ///
    /// # 预警逻辑
    /// - Green → Yellow: warn + 降档 Simplifed + 发布 Warning 事件
    /// - Green/Yellow → Red: error + 熔断 FastPath + 发布 Critical 事件
    /// - Red/Yellow → Green: info + 恢复 Full + 发布 Clear 事件
    fn trigger_alert(&mut self, previous_risk_level: RiskLevel) {
        let now = current_timestamp_ms();
        let trigger_signals = self.collect_trigger_signals();

        match self.risk_level {
            RiskLevel::Yellow => {
                // 预警降档
                warn!(
                    previous_risk_level = ?previous_risk_level,
                    ratio = self.ratio,
                    veto_anomaly_rate = self.veto_anomaly_rate,
                    health_score = self.health_score,
                    trigger_signals = ?trigger_signals,
                    "推理悖论风险:协调成本/推理增益比值异常,降档封顶(Simplified)"
                );

                if let Some(ref cap) = self.strategy_cap {
                    use nexus_contracts::ActivationStrategy;
                    cap.set_max_strategy(ActivationStrategy::Simplified);
                }

                self.publish_alert(AlertSeverity::Warning, &trigger_signals, now);
            }
            RiskLevel::Red => {
                // 紧急熔断
                error!(
                    previous_risk_level = ?previous_risk_level,
                    ratio = self.ratio,
                    veto_anomaly_rate = self.veto_anomaly_rate,
                    health_score = self.health_score,
                    trigger_signals = ?trigger_signals,
                    "推理悖论风险:多信号超标,紧急熔断(FastPath)"
                );

                if let Some(ref cap) = self.strategy_cap {
                    use nexus_contracts::ActivationStrategy;
                    cap.set_max_strategy(ActivationStrategy::FastPath);
                }

                self.publish_alert(AlertSeverity::Critical, &trigger_signals, now);
            }
            RiskLevel::Green => {
                // 恢复正常
                info!(
                    previous_risk_level = ?previous_risk_level,
                    ratio = self.ratio,
                    veto_anomaly_rate = self.veto_anomaly_rate,
                    health_score = self.health_score,
                    "推理悖论风险已解除,恢复 Full 审议"
                );

                if let Some(ref cap) = self.strategy_cap {
                    use nexus_contracts::ActivationStrategy;
                    cap.set_max_strategy(ActivationStrategy::Full);
                }

                self.publish_alert(AlertSeverity::Clear, &trigger_signals, now);
            }
        }
    }

    /// 收集当前触发预警的信号列表
    fn collect_trigger_signals(&self) -> Vec<String> {
        let mut signals = Vec::new();
        if self.ratio > RATIO_THRESHOLD {
            signals.push(format!("ratio={:.2}>1.5", self.ratio));
        }
        if self.veto_anomaly_rate > VETO_ANOMALY_RATE_THRESHOLD {
            signals.push(format!("veto_anomaly={:.2}>0.3", self.veto_anomaly_rate));
        }
        if self.health_score < HEALTH_SCORE_THRESHOLD {
            signals.push(format!("health_score={}<40", self.health_score));
        }
        signals
    }

    /// 通过 Event Bus 或 tracing 发布预警事件
    ///
    /// WHY 优先用 Event Bus:符合 §2.2 跨层通信规范,
    /// 订阅者(TUI/监控)可实时消费。无 EventBus 时仅 tracing 记录。
    fn publish_alert(
        &mut self,
        severity: AlertSeverity,
        trigger_signals: &[String],
        timestamp: u64,
    ) {
        let alert = ParadoxRiskAlert {
            severity,
            trigger_signals: trigger_signals.to_vec(),
            timestamp,
            risk_level: self.risk_level,
        };

        // 记录到历史(上限裁剪)
        self.alert_history.push(alert.clone());
        if self.alert_history.len() > self.max_history {
            self.alert_history.remove(0);
        }

        // 通过 Event Bus 发布(使用 EfficiencyAlertTriggered 作为通用告警事件)
        if let Some(ref bus) = self.event_bus {
            let metric_name = match severity {
                AlertSeverity::Clear => "paradox_risk_cleared",
                AlertSeverity::Warning => "paradox_risk_warning",
                AlertSeverity::Critical => "paradox_risk_critical",
            };
            let _ = bus.publish_blocking(NexusEvent::EfficiencyAlertTriggered {
                metadata: EventMetadata::new("parliament:ParadoxRiskDashboard"),
                rule_id: "paradox_risk_dashboard".to_string(),
                metric_name: metric_name.to_string(),
                triggered_value: self.ratio,
                threshold: RATIO_THRESHOLD,
            });
        }
    }
}

// ============================================================
// 辅助函数
// ============================================================

/// 获取当前 Unix 时间戳(毫秒)
fn current_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // === 风险等级计算测试 ===

    #[test]
    fn test_compute_risk_level_green_all_normal() {
        // 全正常 → Green
        assert_eq!(
            ParadoxRiskDashboard::compute_risk_level(0.5, 0.1, 80),
            RiskLevel::Green
        );
    }

    #[test]
    fn test_compute_risk_level_yellow_ratio_exceeded() {
        // 仅 ratio 超标 → Yellow
        assert_eq!(
            ParadoxRiskDashboard::compute_risk_level(2.0, 0.1, 80),
            RiskLevel::Yellow
        );
    }

    #[test]
    fn test_compute_risk_level_yellow_veto_anomaly_exceeded() {
        // 仅否决异常率超标 → Yellow
        assert_eq!(
            ParadoxRiskDashboard::compute_risk_level(0.5, 0.5, 80),
            RiskLevel::Yellow
        );
    }

    #[test]
    fn test_compute_risk_level_yellow_health_score_low() {
        // 仅健康分过低 → Yellow
        assert_eq!(
            ParadoxRiskDashboard::compute_risk_level(0.5, 0.1, 30),
            RiskLevel::Yellow
        );
    }

    #[test]
    fn test_compute_risk_level_red_two_signals() {
        // 两信号超标 → Red
        assert_eq!(
            ParadoxRiskDashboard::compute_risk_level(2.0, 0.5, 80),
            RiskLevel::Red
        );
    }

    #[test]
    fn test_compute_risk_level_red_three_signals() {
        // 三信号全超标 → Red
        assert_eq!(
            ParadoxRiskDashboard::compute_risk_level(2.0, 0.5, 30),
            RiskLevel::Red
        );
    }

    // === 信号边界测试 ===

    #[test]
    fn test_compute_risk_level_boundary_ratio_exactly_threshold() {
        // ratio 恰好等于阈值(1.5) → 不超标 → Green
        assert_eq!(
            ParadoxRiskDashboard::compute_risk_level(1.5, 0.1, 80),
            RiskLevel::Green
        );
    }

    #[test]
    fn test_compute_risk_level_boundary_ratio_just_above_threshold() {
        // ratio 刚超过阈值(1.5 + ε) → Yellow
        assert_eq!(
            ParadoxRiskDashboard::compute_risk_level(1.5 + f64::EPSILON, 0.1, 80),
            RiskLevel::Yellow
        );
    }

    #[test]
    fn test_compute_risk_level_boundary_veto_anomaly_exactly_threshold() {
        // 否决异常率恰好等于阈值(0.3) → 不超标 → Green
        assert_eq!(
            ParadoxRiskDashboard::compute_risk_level(0.5, 0.3, 80),
            RiskLevel::Green
        );
    }

    #[test]
    fn test_compute_risk_level_boundary_health_score_exactly_threshold() {
        // 健康分恰好等于阈值(40) → 不超标 → Green
        assert_eq!(
            ParadoxRiskDashboard::compute_risk_level(0.5, 0.1, 40),
            RiskLevel::Green
        );
    }

    #[test]
    fn test_compute_risk_level_boundary_health_score_just_below_threshold() {
        // 健康分刚低于阈值(39) → Yellow
        assert_eq!(
            ParadoxRiskDashboard::compute_risk_level(0.5, 0.1, 39),
            RiskLevel::Yellow
        );
    }

    #[test]
    fn test_compute_risk_level_extreme_values() {
        // 极端值:ratio = INFINITY, veto_anomaly = 1.0, health_score = 0
        assert_eq!(
            ParadoxRiskDashboard::compute_risk_level(f64::INFINITY, 1.0, 0),
            RiskLevel::Red
        );
    }

    // === 更新与预警行为测试 ===

    #[test]
    fn test_update_initial_green() {
        // 初始创建为 Green
        let dashboard = ParadoxRiskDashboard::new(None, None);
        assert_eq!(dashboard.risk_level(), RiskLevel::Green);
    }

    #[test]
    fn test_update_yellow_on_ratio_exceeded() {
        // 更新一次:ratio 超标 → 应变为 Yellow
        let mut dashboard = ParadoxRiskDashboard::new(None, None);
        dashboard.update(2.0, 0.1, 80);
        assert_eq!(dashboard.risk_level(), RiskLevel::Yellow);
    }

    #[test]
    fn test_update_red_on_two_signals() {
        // 两信号超标 → Red
        let mut dashboard = ParadoxRiskDashboard::new(None, None);
        dashboard.update(2.0, 0.5, 80);
        assert_eq!(dashboard.risk_level(), RiskLevel::Red);
    }

    #[test]
    fn test_update_red_on_three_signals() {
        // 三信号全超标 → Red
        let mut dashboard = ParadoxRiskDashboard::new(None, None);
        dashboard.update(2.0, 0.5, 30);
        assert_eq!(dashboard.risk_level(), RiskLevel::Red);
    }

    #[test]
    fn test_update_green_to_yellow_to_red_to_green() {
        // 状态转换:Green → Yellow → Red → Green(需连续 5 次正常)
        let mut dashboard = ParadoxRiskDashboard::new(None, None);

        // Green → Yellow(ratio 超标)
        dashboard.update(2.0, 0.1, 80);
        assert_eq!(dashboard.risk_level(), RiskLevel::Yellow);

        // Yellow → Red(两信号超标)
        dashboard.update(2.0, 0.5, 80);
        assert_eq!(dashboard.risk_level(), RiskLevel::Red);

        // 连续 5 次全正常 → Green
        for _ in 0..4 {
            dashboard.update(0.5, 0.1, 80);
            // 前 4 次仍为 Red(连续正常计数未达到 5)
        }
        assert_eq!(
            dashboard.risk_level(),
            RiskLevel::Red,
            "4 次正常不应恢复 Green"
        );

        // 第 5 次 → 恢复 Green
        dashboard.update(0.5, 0.1, 80);
        assert_eq!(dashboard.risk_level(), RiskLevel::Green);
    }

    #[test]
    fn test_update_green_direct_to_red() {
        // 直接从 Green → Red(两信号同时超标)
        let mut dashboard = ParadoxRiskDashboard::new(None, None);
        dashboard.update(2.0, 0.5, 30);
        assert_eq!(dashboard.risk_level(), RiskLevel::Red);
    }

    #[test]
    fn test_consecutive_normal_reset_on_signal_trigger() {
        // 连续正常计数在信号超标时应重置
        let mut dashboard = ParadoxRiskDashboard::new(None, None);

        // 2 次正常
        dashboard.update(0.5, 0.1, 80);
        dashboard.update(0.5, 0.1, 80);
        assert_eq!(dashboard.consecutive_normal, 2);

        // 1 次超标 → 重置
        dashboard.update(2.0, 0.1, 80);
        assert_eq!(dashboard.consecutive_normal, 0);
        assert_eq!(dashboard.risk_level(), RiskLevel::Yellow);
    }

    // === 报告生成测试 ===

    #[test]
    fn test_generate_report_green() {
        let mut dashboard = ParadoxRiskDashboard::new(None, None);
        dashboard.update(0.5, 0.1, 80);

        let report = dashboard.generate_report();
        assert_eq!(report.current_risk_level, RiskLevel::Green);
        assert!((report.current_ratio - 0.5).abs() < 1e-9);
        assert!((report.current_veto_anomaly_rate - 0.1).abs() < 1e-6);
        assert_eq!(report.current_health_score, 80);
        assert_eq!(report.recent_alert_count, 0);
        assert!(report.timestamp > 0);
    }

    #[test]
    fn test_generate_report_yellow() {
        let mut dashboard = ParadoxRiskDashboard::new(None, None);
        dashboard.update(2.0, 0.1, 80);

        let report = dashboard.generate_report();
        assert_eq!(report.current_risk_level, RiskLevel::Yellow);
        assert!((report.current_ratio - 2.0).abs() < 1e-9);
    }

    #[test]
    fn test_generate_report_red() {
        let mut dashboard = ParadoxRiskDashboard::new(None, None);
        dashboard.update(2.0, 0.5, 30);

        let report = dashboard.generate_report();
        assert_eq!(report.current_risk_level, RiskLevel::Red);
        assert_eq!(report.recent_alert_count, 1); // 触发一次预警
    }

    #[test]
    fn test_generate_report_alert_history_count() {
        let mut dashboard = ParadoxRiskDashboard::new(None, None);
        // 多次触发预警
        dashboard.update(2.0, 0.1, 80); // Yellow
        dashboard.update(2.0, 0.5, 80); // Red
        dashboard.update(2.0, 0.5, 30); // Red(无变化,不触发)

        let report = dashboard.generate_report();
        // 2 次预警:Green→Yellow, Yellow→Red
        assert_eq!(report.recent_alert_count, 2);
    }

    // === 预警行为测试(带 StrategyCapGuard) ===

    #[test]
    fn test_yellow_alert_triggers_set_max_strategy_simplified() {
        use nexus_contracts::ActivationStrategy;
        let cap = std::sync::Arc::new(StrategyCapGuard::default());
        let mut dashboard = ParadoxRiskDashboard::new(None, Some(std::sync::Arc::clone(&cap)));

        assert_eq!(cap.current_cap(), ActivationStrategy::Full);

        // 触发 Yellow(ratio 超标)
        dashboard.update(2.0, 0.1, 80);

        // 封顶应降为 Simplified
        assert_eq!(
            cap.current_cap(),
            ActivationStrategy::Simplified,
            "Yellow 预警应将封顶降为 Simplified"
        );
    }

    #[test]
    fn test_red_alert_triggers_set_max_strategy_fastpath() {
        use nexus_contracts::ActivationStrategy;
        let cap = std::sync::Arc::new(StrategyCapGuard::default());
        let mut dashboard = ParadoxRiskDashboard::new(None, Some(std::sync::Arc::clone(&cap)));

        // 触发 Red(两信号超标)
        dashboard.update(2.0, 0.5, 80);

        // 封顶应降为 FastPath
        assert_eq!(
            cap.current_cap(),
            ActivationStrategy::FastPath,
            "Red 预警应将封顶降为 FastPath"
        );
    }

    #[test]
    fn test_green_alert_triggers_set_max_strategy_full() {
        use nexus_contracts::ActivationStrategy;
        let cap = std::sync::Arc::new(StrategyCapGuard::default());
        let mut dashboard = ParadoxRiskDashboard::new(None, Some(std::sync::Arc::clone(&cap)));

        // 先触发 Red
        dashboard.update(2.0, 0.5, 80);
        assert_eq!(cap.current_cap(), ActivationStrategy::FastPath);

        // 连续 5 次正常 → 恢复 Green
        for _ in 0..5 {
            dashboard.update(0.5, 0.1, 80);
        }

        // 封顶应恢复为 Full
        assert_eq!(
            cap.current_cap(),
            ActivationStrategy::Full,
            "Green 恢复应将封顶恢复为 Full"
        );
    }

    #[test]
    fn test_dashboard_no_strategy_cap_does_not_panic() {
        // 无 strategy_cap 时不应 panic
        let mut dashboard = ParadoxRiskDashboard::new(None, None);
        dashboard.update(2.0, 0.1, 80); // Yellow
        dashboard.update(2.0, 0.5, 80); // Red
        dashboard.update(0.5, 0.1, 80);
        dashboard.update(0.5, 0.1, 80);
        dashboard.update(0.5, 0.1, 80);
        dashboard.update(0.5, 0.1, 80);
        dashboard.update(0.5, 0.1, 80); // Green
        assert_eq!(dashboard.risk_level(), RiskLevel::Green);
    }

    // === 信号值钳位测试 ===

    #[test]
    fn test_veto_anomaly_rate_clamped() {
        let mut dashboard = ParadoxRiskDashboard::new(None, None);
        // 超过 1.0 的值应被钳位到 1.0
        dashboard.update(0.5, 1.5, 80);
        assert!((dashboard.veto_anomaly_rate - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_health_score_clamped() {
        let mut dashboard = ParadoxRiskDashboard::new(None, None);
        // 超过 100 的值应被钳位到 100
        dashboard.update(0.5, 0.1, 150);
        assert_eq!(dashboard.health_score, 100);
    }

    // === 历史记录上限测试 ===

    #[test]
    fn test_alert_history_max_size() {
        let mut dashboard = ParadoxRiskDashboard::new(None, None);
        // 设置小上限
        dashboard.max_history = 3;

        // 触发多次预警
        dashboard.update(2.0, 0.1, 80); // Yellow → 1 alert
        dashboard.update(2.0, 0.5, 80); // Red → 2 alerts
        dashboard.update(0.5, 0.1, 80);
        dashboard.update(0.5, 0.1, 80);
        dashboard.update(0.5, 0.1, 80);
        dashboard.update(0.5, 0.1, 80);
        dashboard.update(0.5, 0.1, 80); // Green → 3 alerts

        // 历史应不超过 3 条
        assert_eq!(dashboard.alert_history.len(), 3);
    }

    // === RiskLevel Display 测试 ===

    #[test]
    fn test_risk_level_serde_roundtrip() {
        let levels = [RiskLevel::Green, RiskLevel::Yellow, RiskLevel::Red];
        for level in &levels {
            let json = serde_json::to_string(level).unwrap();
            let restored: RiskLevel = serde_json::from_str(&json).unwrap();
            assert_eq!(*level, restored);
        }
    }

    #[test]
    fn test_alert_severity_serde_roundtrip() {
        let severities = [
            AlertSeverity::Clear,
            AlertSeverity::Warning,
            AlertSeverity::Critical,
        ];
        for severity in &severities {
            let json = serde_json::to_string(severity).unwrap();
            let restored: AlertSeverity = serde_json::from_str(&json).unwrap();
            assert_eq!(*severity, restored);
        }
    }
}
