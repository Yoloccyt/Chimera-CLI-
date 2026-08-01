//! 效率监控器核心实现 — `EfficiencyMonitor` struct 及其全部方法
//!
//! 本模块包含:
//! - [`EfficiencyMonitor`] 结构体定义与构造方法
//! - 同步事件记录 (`record_event`) 与告警检查 (`check_alerts`)
//! - Prometheus `/metrics` 渲染 (`render_metrics`)
//! - 后台事件订阅循环 (`start_event_subscriber`)
//! - Critical 旁路通道丢弃事件数采样 (`sample_critical_dropped_count`)
//! - 后台辅助函数 (`handle_broadcast_event`, `handle_critical_event` 等)

use crate::affinity_metrics::AffinityMetrics;
use crate::alerts::AlertRuleEngine;
use crate::collectors::{EventMetricCollector, MetricCollector};
use crate::config::MonitorConfig;
use crate::error::MonitorError;
use crate::oscillation_detector::PolicyOscillationDetector;
use crate::types::{AlertEvent, AlertRule, AlertSeverity};
use event_bus::{EventBus, EventMetadata, NexusEvent};
use std::time::Duration;
use tracing::warn;

/// efficiency-monitor 的 source 标识（用于 EventMetadata）
const MONITOR_SOURCE: &str = "efficiency-monitor";

/// Critical 旁路通道丢弃事件数指标名（P1-W2.2 新增）
///
/// 该指标在 /metrics 输出中作为 counter 暴露，同时作为
/// `EfficiencyAlertTriggered` 事件的 `metric_name` 字段值，
/// 供 TUI 的 `CriticalDroppedSync` 同步器识别并更新告警显示。
///
/// WHY 常量定义在此处而非 event-bus：event-bus 仅提供
/// `CriticalEventDropped` 载荷类型与 `critical_dropped_count()` API，
/// 指标命名与 Prometheus 暴露是 efficiency-monitor（L9）的职责。
/// TUI（L10）在 `CriticalDroppedSync` 中硬编码同一字符串识别事件。
pub const CRITICAL_DROPPED_METRIC_NAME: &str = "nexus_critical_event_dropped_total";

/// 判断事件是否为 Critical 告警事件（必须立即告警）
///
/// 注意：这与 `NexusEvent::severity()` 部分重叠但语义不同。
/// - `NexusEvent::severity()` 是事件总线的背压级别：SkepticVeto/RedTeamAudit/
///   BudgetExceeded 为 Critical（F-001 修复后），AsaIntervention 仍为 Normal
/// - `is_critical_alert_event` 是 efficiency-monitor 的告警级别（4 个事件均为 Critical）
///
/// WHY 单独定义：AsaIntervention 在 event-bus 中返回 Normal
/// （因为 severity() 是同步函数不依赖运行时值），但在 efficiency-monitor 中
/// 代表安全红线，必须立即告警。F-001 修复后 BudgetExceeded 在两层都是 Critical，
/// 此处保留匹配是出于对称性与稳定性——即使未来 event-bus 的 severity 分类变化，
/// efficiency-monitor 的告警语义也不受影响。
fn is_critical_alert_event(event: &NexusEvent) -> bool {
    matches!(
        event,
        NexusEvent::SkepticVeto { .. }
            | NexusEvent::RedTeamAudit { .. }
            | NexusEvent::AsaIntervention { .. }
            | NexusEvent::BudgetExceeded { .. }
    )
}

/// 效率监控器 — 整合采集器、告警引擎与事件总线
///
/// 持有五个核心组件：
/// - `config`：监控配置（采集间隔、cooldown、Critical 立即告警开关）
/// - `collectors`：事件指标采集器（按 type_name 统计发布次数）
/// - `alert_engine`：告警规则引擎（配置化阈值检测 + cooldown 防抖）
/// - `oscillation_detector`：策略抖振检测器（P2-14,GSOE↔Quest 逻辑循环监测）
/// - `affinity_metrics`：MCA 亲和指标采集器（A5 体验对等不变量，ADR-065/066）
/// - `event_bus`：可选事件总线（订阅 NexusEvent + 发布 EfficiencyAlertTriggered）
pub struct EfficiencyMonitor {
    /// 监控配置
    config: MonitorConfig,
    /// 事件指标采集器（Clone 廉价，基于 `Arc<DashMap>`）
    collectors: EventMetricCollector,
    /// 告警规则引擎（Clone 廉价，基于 `Arc<DashMap>`）
    alert_engine: AlertRuleEngine,
    /// 策略抖振检测器（P2-14,监测 GSOE ↔ Quest 逻辑循环的策略震荡）
    ///
    /// WHY Arc:后台订阅任务与主线程需共享同一检测器实例。
    /// `PolicyOscillationDetector` 内部用 `Mutex<VecDeque>`,Clone 需 Arc 包装。
    oscillation_detector: std::sync::Arc<PolicyOscillationDetector>,
    /// MCA 亲和指标采集器 — 每通道体验度量(A5)
    ///
    /// 消费 StreamSessionCompleted/AffinityCapabilityNegotiated/ProviderDegraded
    /// 事件,产出 TTFT p50/p95、缓存命中率、特性启用率等指标。
    /// Clone 廉价(Arc 共享),后台订阅任务与主线程共享同一实例。
    affinity_metrics: AffinityMetrics,
    /// 可选事件总线（订阅事件 + 发布告警）
    event_bus: Option<EventBus>,
}

impl EfficiencyMonitor {
    /// 创建效率监控器（无 EventBus，不订阅事件也不发布告警）
    ///
    /// 适用场景：单元测试、仅需要同步记录事件与渲染 /metrics 的场景。
    pub fn new(config: MonitorConfig) -> Self {
        Self {
            config,
            collectors: EventMetricCollector::new(),
            alert_engine: AlertRuleEngine::new(),
            oscillation_detector: std::sync::Arc::new(PolicyOscillationDetector::new()),
            affinity_metrics: AffinityMetrics::new(),
            event_bus: None,
        }
    }

    /// 创建效率监控器并绑定 EventBus
    ///
    /// 绑定后：
    /// - `record_event` 中触发的 Critical 告警会发布 `EfficiencyAlertTriggered` 事件
    /// - `check_alerts` 触发的规则告警会发布 `EfficiencyAlertTriggered` 事件
    /// - `start_event_subscriber` 可启动后台订阅循环
    pub fn with_event_bus(config: MonitorConfig, bus: EventBus) -> Self {
        Self {
            config,
            collectors: EventMetricCollector::new(),
            alert_engine: AlertRuleEngine::new(),
            oscillation_detector: std::sync::Arc::new(PolicyOscillationDetector::new()),
            affinity_metrics: AffinityMetrics::new(),
            event_bus: Some(bus),
        }
    }

    /// 同步记录一个事件，更新指标计数器
    ///
    /// 若配置启用 `critical_instant_alert` 且事件为 Critical 告警事件
    /// （SkepticVeto/RedTeamAudit/AsaIntervention/BudgetExceeded），
    /// 将立即记录 Critical 告警计数并发布 `EfficiencyAlertTriggered` 事件。
    ///
    /// P2-14 新增:同时将 `ThinkingModeSwitched` 和 `GsoePolicyUpdated` 事件
    /// 喂入策略抖振检测器,用于监测 GSOE ↔ Quest 逻辑循环的策略震荡。
    ///
    /// P2-1 后续增强:检测 `CoordinationRatioReported` 事件,当 `is_paradox_risk`
    /// 为 true 时记录推理悖论风险告警并发布 `EfficiencyAlertTriggered`。
    ///
    /// 该方法是同步的，适合在不便 await 的场景调用。
    ///
    /// MCA 亲和事件(StreamSessionCompleted/AffinityCapabilityNegotiated/
    /// ProviderDegraded)同时喂入 affinity_metrics 采集器,更新每通道体验度量。
    pub fn record_event(&self, event: &NexusEvent) {
        // 记录事件指标
        self.collectors.record_event(event);

        // P2-14: 将事件喂入策略抖振检测器
        // WHY 在 record_event 中调用:这是所有事件的统一入口点,确保不遗漏
        self.oscillation_detector.record_event(event);

        // MCA 亲和事件:喂入 affinity_metrics 采集器
        self.affinity_metrics.handle_mca_event(event);

        // Critical 事件立即告警（绕过规则引擎，直接触发）
        if self.config.critical_instant_alert && is_critical_alert_event(event) {
            self.collectors
                .record_alert(AlertSeverity::Critical.as_str());
            self.publish_critical_alert(event);
        }

        // P2-1 后续增强:推理悖论风险告警
        // WHY 在 record_event 中处理:CoordinationRatioReported 由 quest-engine
        // 通过 publish_blocking 发布,efficiency-monitor 作为订阅者在此入口消费。
        // 当 ratio > threshold(is_paradox_risk=true) 时触发 Warning 级告警,
        // 供 TUI 事件流面板展示(三重悖论推理悖论红线度量闭环)。
        if let NexusEvent::CoordinationRatioReported {
            is_paradox_risk,
            ratio,
            threshold,
            ..
        } = event
        {
            if *is_paradox_risk {
                self.collectors
                    .record_alert(AlertSeverity::Warning.as_str());
                self.publish_paradox_risk_alert(*ratio, *threshold);
            }
        }
    }

    /// 添加一条告警规则
    ///
    /// 添加后，`check_alerts` 会按规则阈值检查指标样本。
    pub fn add_alert_rule(&self, rule: AlertRule) {
        self.alert_engine.add_rule(rule);
    }

    /// 检查所有告警规则，返回触发的告警事件
    ///
    /// # 流程
    /// 1. 从采集器收集当前指标快照
    /// 2. 用告警规则引擎检查快照（考虑 cooldown）
    /// 3. 对每个触发的告警，记录告警计数并发布 `EfficiencyAlertTriggered` 事件
    ///
    /// 返回的 `Vec<AlertEvent>` 不包含 Critical 立即告警（那些在 `record_event` 中处理）。
    pub fn check_alerts(&self) -> Vec<AlertEvent> {
        let samples = self.collectors.collect();
        let alerts = self.alert_engine.check(&samples);

        // 对每个触发的告警，记录计数并发布事件
        for alert in &alerts {
            if let Some((metric_name, severity)) = self.alert_engine.get_rule_info(&alert.rule_id) {
                self.collectors.record_alert(severity.as_str());
                self.publish_rule_alert(
                    &alert.rule_id,
                    &metric_name,
                    alert.triggered_value,
                    alert.threshold,
                );
            }
        }

        alerts
    }

    /// 渲染 Prometheus 文本格式的 /metrics 输出
    ///
    /// 输出格式遵循 Prometheus exposition format，包含：
    /// - `nexus_event_total`：按事件类型分桶的发布次数
    /// - `nexus_critical_event_total`：按事件类型分桶的 Critical 事件次数
    /// - `nexus_alert_triggered_total`：按严重级别分桶的告警触发次数
    /// - `mca_*`：MCA 亲和指标（TTFT p50/p95、缓存命中率、特性启用率等）
    pub fn render_metrics(&self) -> String {
        let mut event_metrics = crate::dashboard::render_metrics(&self.collectors);
        let affinity_metrics = crate::dashboard::render_metrics(&self.affinity_metrics);
        // 合并输出:事件指标在前,亲和指标在后
        // 若亲和指标非空,追加到事件指标后
        if !affinity_metrics.is_empty() {
            event_metrics.push_str(&affinity_metrics);
        }
        event_metrics
    }

    /// 启动后台事件订阅循环
    ///
    /// 在 `tokio::spawn` 之前同步调用 `bus.subscribe()` 与
    /// `bus.subscribe_critical_events()`，确保不会错过后续发布的事件
    /// （Week 6 教训：broadcast 时序；§4.4 反模式 3）。
    ///
    /// # 双通道消费（§6.2 红线，2026-06-29）
    /// 后台任务通过 `tokio::select!` 同时消费两个通道，职责互斥避免 double-count：
    /// - **broadcast 主通道**：接收全部事件，**仅记录事件指标**（不触发告警）
    /// - **critical mpsc 旁路**：接收 4 类 Critical 安全告警事件
    ///   （SkepticVeto/RedTeamAudit/AsaIntervention/BudgetExceeded），
    ///   **仅触发 Critical 告警**（不记录事件指标）。
    ///   mpsc Unbounded 不会 Lagged，broadcast 丢弃时仍确保告警触发。
    ///
    /// WHY 职责拆分：同一 Critical 事件会双投递到两个通道（broadcast + mpsc），
    /// 若两条通道都触发告警会导致 double-count。拆分后 broadcast 负责指标、
    /// mpsc 负责告警，即使 broadcast Lagged 仅损失指标，告警必达（§6.2 红线）。
    ///
    /// # 错误
    /// 返回 `MonitorError::Config` 若未绑定 EventBus。
    ///
    /// # 注意
    /// 调用方必须在 tokio runtime 上下文中调用此方法。
    pub fn start_event_subscriber(&self) -> Result<(), MonitorError> {
        let bus = self.event_bus.clone().ok_or_else(|| MonitorError::Config {
            reason: "未绑定 EventBus，无法启动事件订阅".into(),
        })?;

        let collectors = self.collectors.clone();
        let bus_for_alerts = bus.clone();
        let critical_enabled = self.config.critical_instant_alert;
        // P1-W2.2：周期性采样 Critical 旁路通道丢弃事件数的间隔
        // 复用 collect_interval_ms（默认 1s），平衡监控实时性与系统开销
        let collect_interval_ms = self.config.collect_interval_ms;

        // P2-14: 共享抖振检测器引用,在后台订阅中记录 TTG/GSOE 事件
        // WHY Arc::clone:PolicyOscillationDetector 内部用 Mutex<VecDeque>,
        // 通过 Arc 共享同一实例,后台任务与主线程记录到同一滑动窗口。
        let oscillation_detector = std::sync::Arc::clone(&self.oscillation_detector);

        // 共享亲和指标采集器,后台订阅中消费 MCA 事件
        let affinity_metrics = self.affinity_metrics.clone();

        // 在 spawn 之前同步订阅两个通道，确保不会错过后续发布的事件
        // WHY: tokio::broadcast 仅投递给发布时已存在的 receiver；
        // 若在 spawn 的 async block 内 subscribe，后台任务调度时机不确定，
        // 可能晚于 publish 导致事件静默丢失（broadcast 不缓存历史消息给新订阅者）
        // WHY mpsc 旁路同步订阅：§4.4 反模式 3，与 broadcast 同理；
        // Critical 安全告警事件必须确保投递，不能因 spawn 时序丢失
        let mut rx = bus.subscribe();
        let mut critical_rx = bus.subscribe_critical_events();

        // WHY fire-and-forget（B-Min-2 评估）：事件订阅器为应用生命周期任务，
        // 随进程退出自动终止。panic 时 tokio 运行时回收资源，不影响监控数据完整性
        // （collectors 为 Arc 共享，下一轮订阅周期会重新记录）。
        tokio::spawn(async move {
            // P1-W2.2：周期性采样 Critical 旁路通道丢弃事件数
            // WHY tokio::time::interval：与事件接收并行，不阻塞 broadcast/mpsc 消费。
            // interval 第一次 tick 立即返回（采样初始值，通常为 0，无害），后续按
            // collect_interval_ms 周期触发。select! 随机选择就绪分支，即使错过
            // 一次 tick，下次仍会采样到正确的累计值（单调递增，非增量）。
            let mut sample_interval =
                tokio::time::interval(Duration::from_millis(collect_interval_ms));
            // 上次采样的累计丢弃数，用于检测是否有新增丢弃
            let mut last_dropped_count: u64 = 0;

            // 双通道消费循环：broadcast 主流 + mpsc 旁路兜底 + 周期采样
            // WHY tokio::select!：同时 await broadcast recv、mpsc recv 与 interval tick，
            // 哪个先就绪就处理哪个。select! 是 tokio 标准模式，无需额外依赖。
            // WHY mpsc 旁路处理：即使在 broadcast Lagged 场景下，Critical 事件
            // 仍需触发告警记录与 EfficiencyAlertTriggered 发布，确保运维感知
            loop {
                tokio::select! {
                    // mpsc 旁路：Critical 安全告警事件（broadcast Lagged 兜底）
                    Some(critical_event) = critical_rx.recv() => {
                        handle_critical_event(
                            &collectors,
                            &bus_for_alerts,
                            critical_enabled,
                            &critical_event,
                        );
                    }
                    // broadcast 主流：全部事件（含 Critical，向后兼容）
                    result = rx.recv() => {
                        match result {
                            Ok(event) => {
                                handle_broadcast_event(
                                    &collectors,
                                    &bus_for_alerts,
                                    critical_enabled,
                                    &oscillation_detector,
                                    &affinity_metrics,
                                    &event,
                                );
                            }
                            Err(e) => {
                                // SlowConsumerDropped/RecvTimeout：继续循环等新事件
                                // ChannelClosed：所有 Sender 已 drop，退出循环
                                if matches!(e, event_bus::EventBusError::ChannelClosed) {
                                    break;
                                }
                            }
                        }
                    }
                    // P1-W2.2：周期性采样 Critical 旁路通道丢弃事件数
                    // WHY 独立分支：采样不依赖事件到达，即使无事件也需周期刷新指标，
                    // 确保 /metrics 端点与 TUI 告警显示反映最新的丢弃累计值。
                    _ = sample_interval.tick() => {
                        let current = bus_for_alerts.critical_dropped_count();
                        collectors.record_critical_dropped(current);
                        // 仅当累计丢弃数增加时发布告警事件，避免每 tick 重复发布
                        // WHY 比较 > 而非 !=：EventBus 计数单调递增，> 涵盖所有增长场景
                        if current > last_dropped_count {
                            publish_critical_dropped_alert(&bus_for_alerts, current);
                            last_dropped_count = current;
                        }
                    }
                }
            }
        });

        Ok(())
    }

    /// 获取配置引用
    pub fn config(&self) -> &MonitorConfig {
        &self.config
    }

    /// 获取采集器引用（用于直接查询计数）
    pub fn collectors(&self) -> &EventMetricCollector {
        &self.collectors
    }

    /// 获取告警引擎引用（用于直接管理规则）
    pub fn alert_engine(&self) -> &AlertRuleEngine {
        &self.alert_engine
    }

    /// 获取策略抖振检测器引用(P2-14)
    ///
    /// 通过此引用可调用 `detect()` 获取当前抖振检测报告,
    /// 或调用 `collect_metrics()` 获取抖振指标样本。
    pub fn oscillation_detector(&self) -> &PolicyOscillationDetector {
        &self.oscillation_detector
    }

    /// 获取策略抖振检测器的 Arc 克隆(P2-14)
    ///
    /// 用于需要在后台任务或其他位置共享检测器的场景。
    pub fn oscillation_detector_arc(&self) -> std::sync::Arc<PolicyOscillationDetector> {
        std::sync::Arc::clone(&self.oscillation_detector)
    }

    /// 获取 MCA 亲和指标采集器引用
    ///
    /// 通过此引用可查询每通道的 TTFT 百分位、缓存命中率、特性启用率等体验度量。
    pub fn affinity_metrics(&self) -> &AffinityMetrics {
        &self.affinity_metrics
    }

    /// 执行策略抖振检测,返回当前时间窗口的检测报告(P2-14)
    ///
    /// 这是访问抖振检测结果的便捷方法,等价于
    /// `monitor.oscillation_detector().detect()`。
    ///
    /// # 返回
    /// 返回 [`OscillationReport`],包含:
    /// - GSOE 更新次数 / TTG 切换次数(时间窗口内)
    /// - 震荡对数与震荡模式列表
    /// - 抖振严重度 ∈ [0.0, 1.0]
    /// - 是否触发告警(severity > 0.7)
    pub fn detect_oscillation(&self) -> crate::oscillation_detector::OscillationReport {
        self.oscillation_detector.detect()
    }

    /// 采样 Critical 旁路通道累计丢弃事件数（P1-W2.2 新增）
    ///
    /// 从绑定的 EventBus 拉取 `critical_dropped_count()` 当前累计值，
    /// 更新采集器的 `nexus_critical_event_dropped_total` 指标快照，
    /// 返回最新采样值。
    ///
    /// # 设计决策（WHY 选项 1：直接调用 bus API）
    /// efficiency-monitor 持有 `Option<EventBus>` 引用（EventBus 内部为 Arc，
    /// Clone 廉价），可直接调用 `bus.critical_dropped_count()` 采样。
    /// 这是最简单的方案，无需新增事件变体或共享 AtomicU64。
    ///
    /// # 调用时机
    /// - **自动**：`start_event_subscriber` 在 select! 循环中加入周期性 tick
    ///   分支（间隔 = `config.collect_interval_ms`，默认 1s），自动调用此方法
    /// - **手动**：调用方可在任意时刻调用此方法强制刷新快照（用于测试或
    ///   未启用 `start_event_subscriber` 的场景）
    ///
    /// # 返回
    /// - `Some(count)`：采样成功，返回当前累计丢弃数
    /// - `None`：未绑定 EventBus，无法采样
    pub fn sample_critical_dropped_count(&self) -> Option<u64> {
        let bus = self.event_bus.as_ref()?;
        let current = bus.critical_dropped_count();
        self.collectors.record_critical_dropped(current);
        Some(current)
    }

    /// 发布 Critical 事件立即告警（同步，使用 publish_blocking）
    ///
    /// 构造 `EfficiencyAlertTriggered` 事件并通过 `publish_blocking` 发布。
    /// 发布失败仅记录日志，不阻塞调用方。
    fn publish_critical_alert(&self, event: &NexusEvent) {
        let Some(bus) = &self.event_bus else {
            return;
        };

        let type_name = event.type_name();
        let alert_event = NexusEvent::EfficiencyAlertTriggered {
            metadata: EventMetadata::new(MONITOR_SOURCE),
            rule_id: format!("critical-{type_name}"),
            metric_name: type_name.to_string(),
            triggered_value: 1.0,
            threshold: 1.0,
        };

        if let Err(e) = bus.publish_blocking(alert_event) {
            warn!(error = %e, event_type = type_name, "发布 Critical 告警事件失败");
        }
    }

    /// 发布规则触发的告警（同步，使用 publish_blocking）
    ///
    /// 构造 `EfficiencyAlertTriggered` 事件并通过 `publish_blocking` 发布。
    /// 发布失败仅记录日志，不阻塞调用方。
    fn publish_rule_alert(
        &self,
        rule_id: &str,
        metric_name: &str,
        triggered_value: f64,
        threshold: f64,
    ) {
        let Some(bus) = &self.event_bus else {
            return;
        };

        let alert_event = NexusEvent::EfficiencyAlertTriggered {
            metadata: EventMetadata::new(MONITOR_SOURCE),
            rule_id: rule_id.to_string(),
            metric_name: metric_name.to_string(),
            triggered_value,
            threshold,
        };

        if let Err(e) = bus.publish_blocking(alert_event) {
            warn!(error = %e, rule_id = rule_id, "发布规则告警事件失败");
        }
    }

    /// P2-1: 发布推理悖论风险告警(同步,使用 publish_blocking)
    ///
    /// 当 `CoordinationRatioReported` 事件的 `is_paradox_risk = true` 时调用,
    /// 构造 `EfficiencyAlertTriggered` 事件(WARNING 级)并通过 `publish_blocking` 发布。
    /// 发布失败仅记录日志,不阻塞调用方。
    ///
    /// # 参数
    /// - `ratio`: 当前协调成本/推理增益比值
    /// - `threshold`: 推理悖论告警阈值
    fn publish_paradox_risk_alert(&self, ratio: f64, threshold: f64) {
        let Some(bus) = &self.event_bus else {
            return;
        };

        let alert_event = NexusEvent::EfficiencyAlertTriggered {
            metadata: EventMetadata::new(MONITOR_SOURCE),
            rule_id: "paradox-risk-coordination-ratio".into(),
            metric_name: "coordination_to_gain_ratio".into(),
            triggered_value: ratio,
            threshold,
        };

        if let Err(e) = bus.publish_blocking(alert_event) {
            warn!(error = %e, ratio, threshold, "发布推理悖论风险告警事件失败");
        }
    }
}

impl Default for EfficiencyMonitor {
    fn default() -> Self {
        Self::new(MonitorConfig::default())
    }
}

/// 在后台任务中处理 broadcast 主流事件（全部事件）
///
/// 记录事件指标（所有事件，含 Critical）。**不触发告警** — Critical 告警
/// 逻辑委托给 [`handle_critical_event`] 通过 mpsc 旁路处理，避免同一
/// Critical 事件被 double-count（broadcast + mpsc 旁路双投递）。
///
/// P2-14 新增:同时将事件喂入策略抖振检测器,记录 TTG/GSOE 事件到滑动窗口。
///
/// MCA 亲和事件(StreamSessionCompleted/AffinityCapabilityNegotiated/
/// ProviderDegraded)同时喂入 affinity_metrics 采集器,更新每通道体验度量。
///
/// WHY 拆分职责：broadcast 主流负责事件指标记录（所有事件），
/// mpsc 旁路负责 Critical 告警触发（仅 4 类事件）。两条通道职责互斥，
/// 即使 broadcast Lagged 导致事件指标缺失，Critical 告警仍由 mpsc 旁路
/// 确保触发（§6.2 红线：Critical 安全事件必须确保送达）。
fn handle_broadcast_event(
    collectors: &EventMetricCollector,
    _bus: &EventBus,
    _critical_enabled: bool,
    oscillation_detector: &PolicyOscillationDetector,
    affinity_metrics: &AffinityMetrics,
    event: &NexusEvent,
) {
    // 仅记录事件指标，告警逻辑委托给 handle_critical_event（mpsc 旁路）
    collectors.record_event(event);
    // P2-14: 将事件喂入抖振检测器
    // WHY 在 broadcast 路径记录:与 event 指标记录同路径,确保不遗漏
    oscillation_detector.record_event(event);
    // MCA 亲和事件:喂入 affinity_metrics 采集器
    // WHY 在 broadcast 路径记录:与 event 指标记录同路径,确保不遗漏
    affinity_metrics.handle_mca_event(event);
}

/// 在后台任务中处理 mpsc 旁路 Critical 事件（Critical 告警触发）
///
/// §6.2 红线：Critical 安全告警事件通过 mpsc 旁路确保投递。此函数是
/// Critical 告警的**唯一触发点** — broadcast 主流不再触发告警，
/// 避免同一事件被 double-count。mpsc 旁路 Unbounded 不会 Lagged，
/// 确保 broadcast 丢弃时 Critical 告警仍能触发。
///
/// WHY 不调用 record_event：event 指标已由 broadcast 主流的
/// `handle_broadcast_event` 记录（若未 Lagged）；若 broadcast Lagged，
/// event 指标缺失但告警仍触发（可接受取舍：告警优先于指标）。
fn handle_critical_event(
    collectors: &EventMetricCollector,
    bus: &EventBus,
    critical_enabled: bool,
    event: &NexusEvent,
) {
    // mpsc 旁路仅投递 4 类 Critical 安全告警事件，无需再判 is_critical_alert_event
    // WHY 不调用 record_event：避免与 broadcast 主流 double-count event 指标
    if critical_enabled {
        collectors.record_alert(AlertSeverity::Critical.as_str());
        publish_critical_alert_blocking(bus, event);
    }
}

/// 在后台任务中发布 Critical 事件立即告警（异步上下文，使用 publish_blocking）
///
/// WHY 使用 publish_blocking：后台订阅循环中不便 await（会影响后续事件接收），
/// publish_blocking 是同步发送，不会阻塞事件循环。
fn publish_critical_alert_blocking(bus: &EventBus, event: &NexusEvent) {
    let type_name = event.type_name();
    let alert_event = NexusEvent::EfficiencyAlertTriggered {
        metadata: EventMetadata::new(MONITOR_SOURCE),
        rule_id: format!("critical-{type_name}"),
        metric_name: type_name.to_string(),
        triggered_value: 1.0,
        threshold: 1.0,
    };

    if let Err(e) = bus.publish_blocking(alert_event) {
        warn!(error = %e, event_type = type_name, "后台任务发布 Critical 告警事件失败");
    }
}

/// 发布 Critical 旁路通道丢弃事件数告警（P1-W2.2 新增）
///
/// 当 `EventBus::critical_dropped_count()` 检测到累计丢弃数增加时，构造
/// `EfficiencyAlertTriggered` 事件并通过 `publish_blocking` 发布。
/// TUI 的 `CriticalDroppedSync` 同步器通过 `metric_name ==
/// CRITICAL_DROPPED_METRIC_NAME` 识别此事件，更新告警显示。
///
/// WHY 使用 publish_blocking：与 `publish_critical_alert_blocking` 一致，
/// 后台订阅循环中不便 await，publish_blocking 是同步发送，不阻塞事件循环。
///
/// WHY triggered_value 为累计值而非增量：TUI 需要显示累计丢弃总数，
/// 传递累计值避免 TUI 端维护额外的累加状态。`threshold` 设为 0 表示
/// 任何丢弃都应告警（> 0 即触发）。
fn publish_critical_dropped_alert(bus: &EventBus, dropped_count: u64) {
    let alert_event = NexusEvent::EfficiencyAlertTriggered {
        metadata: EventMetadata::new(MONITOR_SOURCE),
        rule_id: "critical-event-dropped".into(),
        metric_name: CRITICAL_DROPPED_METRIC_NAME.to_string(),
        triggered_value: dropped_count as f64,
        threshold: 0.0,
    };

    if let Err(e) = bus.publish_blocking(alert_event) {
        warn!(
            error = %e,
            dropped_count,
            "后台任务发布 Critical 丢弃告警事件失败"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Comparison;
    use event_bus::EventMetadata;

    fn make_skeptic_veto() -> NexusEvent {
        NexusEvent::SkepticVeto {
            metadata: EventMetadata::new("parliament"),
            quest_id: "q-1".into(),
            veto_reason: "test".into(),
            frozen_capabilities: vec![],
        }
    }

    fn make_red_team_audit() -> NexusEvent {
        NexusEvent::RedTeamAudit {
            metadata: EventMetadata::new("parliament"),
            vulnerability_type: "test".into(),
            failed_probes: 1,
            total_probes: 10,
            detection_rate: 0.1,
            remediation_suggestion: "fix".into(),
        }
    }

    fn make_asa_intervention() -> NexusEvent {
        NexusEvent::AsaIntervention {
            metadata: EventMetadata::new("seccore"),
            operation_id: "op-1".into(),
            action: "Block".into(),
            safety_score: 0.2,
            block_reason: Some("unsafe".into()),
            alternative_suggestion: None,
        }
    }

    fn make_budget_exceeded() -> NexusEvent {
        NexusEvent::BudgetExceeded {
            metadata: EventMetadata::new("decb-governor"),
            budget_type: "token".into(),
            current: 100,
            limit: 50,
        }
    }

    fn make_cache_hit() -> NexusEvent {
        NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k-1".into(),
        }
    }

    // P2-14 测试辅助:构造 TTG 切换事件
    fn make_ttg_switch(from: &str, to: &str) -> NexusEvent {
        NexusEvent::ThinkingModeSwitched {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: "q-test".into(),
            from_mode: from.into(),
            to_mode: to.into(),
            reason: "test".into(),
        }
    }

    // P2-14 测试辅助:构造 GSOE 策略更新事件
    fn make_gsoe_update(gen: u64, imp: f32) -> NexusEvent {
        NexusEvent::GsoePolicyUpdated {
            metadata: EventMetadata::new("gsoe-evolution"),
            generation: gen,
            improvement: imp,
            new_mutation_rate: 0.1,
            new_selection_pressure: 0.5,
        }
    }

    // MCA 测试辅助:构造 StreamSessionCompleted 事件
    fn make_stream_session(route_key: &str, ttft_ms: u64) -> NexusEvent {
        NexusEvent::StreamSessionCompleted {
            metadata: EventMetadata::new("mca-gateway"),
            intent_id: "i-1".into(),
            route_key: route_key.into(),
            input_tokens: 100,
            output_tokens: 50,
            cache_hit_tokens: 30,
            cost_actual_micro: 500,
            ttft_ms,
        }
    }

    // MCA 测试辅助:构造 AffinityCapabilityNegotiated 事件
    fn make_affinity_negotiation(route_key: &str, fidelity: &str) -> NexusEvent {
        NexusEvent::AffinityCapabilityNegotiated {
            metadata: EventMetadata::new("mca-gateway"),
            route_key: route_key.into(),
            fidelity: fidelity.into(),
            degraded_capabilities: vec![],
        }
    }

    // MCA 测试辅助:构造 ProviderDegraded 事件
    fn make_provider_degraded(route_key: &str) -> NexusEvent {
        NexusEvent::ProviderDegraded {
            metadata: EventMetadata::new("mca-gateway"),
            route_key: route_key.into(),
            reason: "timeout".into(),
            health_score: 30,
        }
    }

    #[test]
    fn test_is_critical_alert_event_skeptic_veto() {
        assert!(is_critical_alert_event(&make_skeptic_veto()));
    }

    #[test]
    fn test_is_critical_alert_event_red_team_audit() {
        assert!(is_critical_alert_event(&make_red_team_audit()));
    }

    #[test]
    fn test_is_critical_alert_event_asa_intervention() {
        assert!(is_critical_alert_event(&make_asa_intervention()));
    }

    #[test]
    fn test_is_critical_alert_event_budget_exceeded() {
        assert!(is_critical_alert_event(&make_budget_exceeded()));
    }

    #[test]
    fn test_is_critical_alert_event_normal_event() {
        assert!(!is_critical_alert_event(&make_cache_hit()));
    }

    #[test]
    fn test_new_without_event_bus() {
        let monitor = EfficiencyMonitor::new(MonitorConfig::default());
        assert!(monitor.event_bus.is_none());
    }

    #[test]
    fn test_with_event_bus_binds_bus() {
        let bus = EventBus::new();
        let monitor = EfficiencyMonitor::with_event_bus(MonitorConfig::default(), bus);
        assert!(monitor.event_bus.is_some());
    }

    #[test]
    fn test_record_event_updates_collectors() {
        let monitor = EfficiencyMonitor::new(MonitorConfig::default());
        monitor.record_event(&make_cache_hit());
        monitor.record_event(&make_cache_hit());

        assert_eq!(monitor.collectors().event_count("CacheHit"), 2);
    }

    #[test]
    fn test_record_critical_event_records_alert_count() {
        let monitor = EfficiencyMonitor::new(MonitorConfig::default());
        monitor.record_event(&make_skeptic_veto());

        // Critical 事件应记录 critical 告警计数
        assert_eq!(monitor.collectors().alert_count("critical"), 1);
    }

    #[test]
    fn test_record_normal_event_does_not_record_alert() {
        let monitor = EfficiencyMonitor::new(MonitorConfig::default());
        monitor.record_event(&make_cache_hit());

        // Normal 事件不应记录告警
        assert_eq!(monitor.collectors().alert_count("critical"), 0);
        assert_eq!(monitor.collectors().alert_count("warning"), 0);
    }

    #[test]
    fn test_record_critical_event_with_disabled_instant_alert() {
        let config = MonitorConfig {
            critical_instant_alert: false,
            ..MonitorConfig::default()
        };
        let monitor = EfficiencyMonitor::new(config);
        monitor.record_event(&make_skeptic_veto());

        // 禁用立即告警后，Critical 事件不应记录告警计数
        assert_eq!(monitor.collectors().alert_count("critical"), 0);
        // 但事件计数仍应记录
        assert_eq!(monitor.collectors().event_count("SkepticVeto"), 1);
    }

    #[test]
    fn test_add_alert_rule() {
        let monitor = EfficiencyMonitor::new(MonitorConfig::default());
        monitor.add_alert_rule(AlertRule::new(
            "r-1",
            "nexus_event_total",
            10.0,
            Comparison::GreaterThan,
            AlertSeverity::Warning,
        ));
        assert_eq!(monitor.alert_engine().rule_count(), 1);
    }

    #[test]
    fn test_check_alerts_returns_triggered() {
        let monitor = EfficiencyMonitor::new(MonitorConfig::default());
        monitor.add_alert_rule(AlertRule::new(
            "r-1",
            "nexus_event_total",
            5.0,
            Comparison::GreaterOrEqual,
            AlertSeverity::Warning,
        ));

        // 记录 6 次 CacheHit 事件
        for _ in 0..6 {
            monitor.record_event(&make_cache_hit());
        }

        let alerts = monitor.check_alerts();
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].rule_id, "r-1");
    }

    #[test]
    fn test_check_alerts_records_alert_count() {
        let monitor = EfficiencyMonitor::new(MonitorConfig::default());
        monitor.add_alert_rule(AlertRule::new(
            "r-1",
            "nexus_event_total",
            5.0,
            Comparison::GreaterOrEqual,
            AlertSeverity::Warning,
        ));

        for _ in 0..6 {
            monitor.record_event(&make_cache_hit());
        }

        let _ = monitor.check_alerts();
        // 应记录 warning 告警计数
        assert_eq!(monitor.collectors().alert_count("warning"), 1);
    }

    #[test]
    fn test_render_metrics_contains_event_total() {
        let monitor = EfficiencyMonitor::new(MonitorConfig::default());
        monitor.record_event(&make_cache_hit());

        let output = monitor.render_metrics();
        assert!(output.contains("nexus_event_total"));
        assert!(output.contains(r#"type="CacheHit""#));
    }

    #[test]
    fn test_render_metrics_contains_alert_triggered() {
        let monitor = EfficiencyMonitor::new(MonitorConfig::default());
        monitor.record_event(&make_skeptic_veto()); // 触发 Critical 告警

        let output = monitor.render_metrics();
        assert!(output.contains("nexus_alert_triggered_total"));
        assert!(output.contains(r#"severity="critical""#));
    }

    #[test]
    fn test_start_event_subscriber_without_bus_returns_error() {
        let monitor = EfficiencyMonitor::new(MonitorConfig::default());
        let result = monitor.start_event_subscriber();
        assert!(matches!(result, Err(MonitorError::Config { .. })));
    }

    #[tokio::test]
    async fn test_record_critical_event_publishes_alert_event() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let monitor = EfficiencyMonitor::with_event_bus(MonitorConfig::default(), bus);

        // 记录 Critical 事件，应发布 EfficiencyAlertTriggered
        monitor.record_event(&make_skeptic_veto());

        // 接收并验证事件
        let event = rx.recv().await.expect("应收到事件");
        match event {
            NexusEvent::EfficiencyAlertTriggered {
                rule_id,
                metric_name,
                triggered_value,
                threshold,
                ..
            } => {
                assert!(rule_id.contains("critical"));
                assert_eq!(metric_name, "SkepticVeto");
                assert!((triggered_value - 1.0).abs() < f64::EPSILON);
                assert!((threshold - 1.0).abs() < f64::EPSILON);
            }
            _ => panic!("期望 EfficiencyAlertTriggered 事件，收到 {:?}", event),
        }
    }

    #[tokio::test]
    async fn test_check_alerts_publishes_alert_event() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let monitor = EfficiencyMonitor::with_event_bus(MonitorConfig::default(), bus);

        monitor.add_alert_rule(AlertRule::new(
            "r-1",
            "nexus_event_total",
            5.0,
            Comparison::GreaterOrEqual,
            AlertSeverity::Warning,
        ));

        for _ in 0..6 {
            monitor.record_event(&make_cache_hit());
        }

        let alerts = monitor.check_alerts();
        assert_eq!(alerts.len(), 1);

        // 接收告警事件
        let event = rx.recv().await.expect("应收到告警事件");
        match event {
            NexusEvent::EfficiencyAlertTriggered { rule_id, .. } => {
                assert_eq!(rule_id, "r-1");
            }
            _ => panic!("期望 EfficiencyAlertTriggered 事件"),
        }
    }

    #[tokio::test]
    async fn test_start_event_subscriber_receives_events() {
        let bus = EventBus::new();
        let monitor = EfficiencyMonitor::with_event_bus(MonitorConfig::default(), bus.clone());

        // 启动后台订阅
        monitor.start_event_subscriber().expect("启动订阅失败");

        // 给后台任务时间启动
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // 发布事件
        bus.publish(make_cache_hit()).await.expect("发布失败");

        // 给后台任务时间处理
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // 验证事件被记录
        assert_eq!(monitor.collectors().event_count("CacheHit"), 1);
    }

    #[tokio::test]
    async fn test_start_event_subscriber_critical_alert() {
        let bus = EventBus::new();
        let monitor = EfficiencyMonitor::with_event_bus(MonitorConfig::default(), bus.clone());

        // 启动后台订阅
        monitor.start_event_subscriber().expect("启动订阅失败");

        // 给后台任务时间启动
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // 发布 Critical 事件
        bus.publish(make_skeptic_veto()).await.expect("发布失败");

        // 给后台任务时间处理
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // 验证事件被记录
        assert_eq!(monitor.collectors().event_count("SkepticVeto"), 1);
        // 验证 Critical 告警计数
        assert_eq!(monitor.collectors().alert_count("critical"), 1);
    }

    #[test]
    fn test_default_uses_default_config() {
        let monitor = EfficiencyMonitor::default();
        assert_eq!(monitor.config().collect_interval_ms, 1000);
        assert!(monitor.config().critical_instant_alert);
    }

    // ============================================================
    // P1-W2.2 新增：Critical 旁路通道丢弃事件数采样与告警
    // ============================================================

    #[test]
    fn test_sample_critical_dropped_count_without_bus_returns_none() {
        // 未绑定 EventBus 时，sample_critical_dropped_count 应返回 None
        let monitor = EfficiencyMonitor::new(MonitorConfig::default());
        assert_eq!(monitor.sample_critical_dropped_count(), None);
    }

    #[test]
    fn test_sample_critical_dropped_count_with_bus_returns_zero_initially() {
        // 绑定 EventBus 后，初始 dropped_count 应为 0
        let bus = EventBus::new();
        let monitor = EfficiencyMonitor::with_event_bus(MonitorConfig::default(), bus);
        assert_eq!(monitor.sample_critical_dropped_count(), Some(0));
        // 采集器应反映采样值
        assert_eq!(monitor.collectors().critical_dropped_count(), 0);
    }

    #[tokio::test]
    async fn test_sample_critical_dropped_count_updates_collector() {
        // 触发 Critical 通道丢弃后，采样应反映新的累计值
        let bus = EventBus::new();
        // 订阅但不消费，模拟慢消费者填满 4096 容量
        let _stale_rx = bus.subscribe_critical_events();

        let capacity = bus.critical_channel_capacity();
        let publish_count: u64 = (capacity + 100) as u64;

        for i in 0..publish_count {
            // publish_critical 显式走 mpsc 旁路，容量满时 try_send 失败丢弃并递增计数
            let _ = bus
                .publish_critical(NexusEvent::CacheHit {
                    metadata: event_bus::EventMetadata::new("test"),
                    cache_key: format!("k-{i}"),
                })
                .await;
        }

        let monitor = EfficiencyMonitor::with_event_bus(MonitorConfig::default(), bus);
        let sampled = monitor
            .sample_critical_dropped_count()
            .expect("应返回采样值");
        // 应有丢弃发生（具体数量取决于 try_send 时序，但应 > 0）
        assert!(
            sampled > 0,
            "发布 {publish_count} 个事件到容量 {capacity} 的通道，应有丢弃发生"
        );
        assert_eq!(monitor.collectors().critical_dropped_count(), sampled);
    }

    #[test]
    fn test_render_metrics_contains_critical_dropped_total() {
        // /metrics 输出应包含 nexus_critical_event_dropped_total 指标
        let bus = EventBus::new();
        let monitor = EfficiencyMonitor::with_event_bus(MonitorConfig::default(), bus);
        monitor.sample_critical_dropped_count();

        let output = monitor.render_metrics();
        assert!(output.contains("# HELP nexus_critical_event_dropped_total"));
        assert!(output.contains("# TYPE nexus_critical_event_dropped_total counter"));
        assert!(output.contains("nexus_critical_event_dropped_total"));
    }

    #[tokio::test]
    async fn test_start_event_subscriber_publishes_dropped_alert() {
        // 后台订阅器应周期性采样 dropped_count 并在增加时发布告警事件
        //
        // WHY 大容量 broadcast（16384）：publish_critical 会向 broadcast + mpsc 双通道
        // 投递 4146 个 CacheHit 事件；后台任务的 critical_rx 消费后会对每个事件调用
        // handle_critical_event -> publish_critical_alert_blocking，再向 broadcast 投递
        // 4146 个 EfficiencyAlertTriggered 事件。总计 ~8293 事件远超默认容量 1024，
        // 会导致 test rx Lagged -> rx.recv() 返回 Err -> while 循环提前退出。
        // 16384 容量可吸收全部事件，确保 test rx 不丢 dropped alert。
        let bus = EventBus::with_capacity(16384);
        let mut rx = bus.subscribe();

        // 使用极短采样间隔加速测试（10ms）
        let config = MonitorConfig {
            collect_interval_ms: 10,
            ..MonitorConfig::default()
        };
        let monitor = EfficiencyMonitor::with_event_bus(config, bus.clone());
        monitor.start_event_subscriber().expect("启动订阅失败");

        // 给后台任务时间启动
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // 触发 Critical 通道丢弃：订阅但不消费，填满 4096 容量
        let _stale_rx = bus.subscribe_critical_events();
        let capacity = bus.critical_channel_capacity();
        for i in 0..(capacity + 50) as u64 {
            let _ = bus
                .publish_critical(NexusEvent::CacheHit {
                    metadata: event_bus::EventMetadata::new("test"),
                    cache_key: format!("k-{i}"),
                })
                .await;
        }

        // 等待至少一个采样周期（10ms x 2 = 20ms，加余量）
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // 应收到至少一个 EfficiencyAlertTriggered 事件（metric_name = nexus_critical_event_dropped_total）
        let mut found_dropped_alert = false;
        // 排空已有事件（可能包含 mpsc 旁路的 Critical 告警）
        // WHY 处理 Err（Lagged）：即使 16384 容量，极端时序下仍可能 Lagged，
        // 此时 continue 继续排空而非退出循环，确保不遗漏后续的 dropped alert 事件。
        while let Ok(recv_result) =
            tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await
        {
            match recv_result {
                Ok(event) => {
                    if let NexusEvent::EfficiencyAlertTriggered { metric_name, .. } = &event {
                        if metric_name == CRITICAL_DROPPED_METRIC_NAME {
                            found_dropped_alert = true;
                            break;
                        }
                    }
                }
                // Lagged/SlowConsumerDropped：继续排空，不退出循环
                Err(_) => continue,
            }
        }
        assert!(
            found_dropped_alert,
            "应发布 metric_name = {CRITICAL_DROPPED_METRIC_NAME} 的告警事件"
        );

        // 采集器应反映丢弃计数
        assert!(
            monitor.collectors().critical_dropped_count() > 0,
            "采集器应记录丢弃计数"
        );
    }

    // ============================================================
    // P2-14 新增:策略抖振检测器集成测试
    // ============================================================

    #[test]
    fn test_record_event_feeds_oscillation_detector() {
        // EfficiencyMonitor::record_event 应将 TTG 切换事件喂入抖振检测器
        let monitor = EfficiencyMonitor::new(MonitorConfig::default());

        // 记录 3 次 Fast->Deep 切换(达到震荡阈值)
        for _ in 0..3 {
            monitor.record_event(&make_ttg_switch("Fast", "Deep"));
        }

        let report = monitor.detect_oscillation();
        assert_eq!(report.ttg_switches_in_window, 3);
        assert_eq!(report.oscillation_pairs, 1); // Fast->Deep 出现 3 次
    }

    #[test]
    fn test_record_event_feeds_gsoe_updates() {
        // GSOE 策略更新事件应被抖振检测器记录
        let monitor = EfficiencyMonitor::new(MonitorConfig::default());

        monitor.record_event(&make_gsoe_update(1, 0.05));
        monitor.record_event(&make_gsoe_update(2, 0.03));

        let report = monitor.detect_oscillation();
        assert_eq!(report.gsoe_updates_in_window, 2);
    }

    #[test]
    fn test_oscillation_severity_alert_threshold() {
        // 当抖振严重度超过阈值(0.7)时,should_alert 应为 true
        let monitor = EfficiencyMonitor::new(MonitorConfig::default());

        // 制造足够多震荡:3 个震荡对 + 21 次切换 -> severity = 1.0
        for _ in 0..7 {
            monitor.record_event(&make_ttg_switch("Fast", "Deep"));
        }
        for _ in 0..7 {
            monitor.record_event(&make_ttg_switch("Deep", "Standard"));
        }
        for _ in 0..7 {
            monitor.record_event(&make_ttg_switch("Standard", "Fast"));
        }

        let report = monitor.detect_oscillation();
        assert!(report.should_alert, "严重度 {} 应触发告警", report.severity);
        assert!((report.severity - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_no_oscillation_for_normal_workflow() {
        // 正常工作流(1-2 次切换)不应触发抖振告警
        let monitor = EfficiencyMonitor::new(MonitorConfig::default());

        monitor.record_event(&make_ttg_switch("Fast", "Standard"));
        monitor.record_event(&make_ttg_switch("Standard", "Deep"));

        let report = monitor.detect_oscillation();
        assert_eq!(report.ttg_switches_in_window, 2);
        assert_eq!(report.oscillation_pairs, 0);
        assert!(!report.should_alert);
    }

    #[test]
    fn test_oscillation_detector_accessible_via_monitor() {
        // oscillation_detector() 访问器应返回有效引用
        let monitor = EfficiencyMonitor::new(MonitorConfig::default());
        let detector = monitor.oscillation_detector();

        // 通过引用调用 detect() 应返回默认报告
        let report = detector.detect();
        assert_eq!(report.ttg_switches_in_window, 0);
    }

    #[test]
    fn test_oscillation_detector_arc_shareable() {
        // oscillation_detector_arc() 应返回可共享的 Arc 克隆
        let monitor = EfficiencyMonitor::new(MonitorConfig::default());
        let detector_arc = monitor.oscillation_detector_arc();

        // 通过 Arc 记录事件,原 monitor 应可见
        detector_arc.record_event(&make_ttg_switch("Fast", "Deep"));

        let report = monitor.detect_oscillation();
        assert_eq!(report.ttg_switches_in_window, 1);
    }

    #[tokio::test]
    async fn test_background_subscriber_feeds_oscillation_detector() {
        // 后台订阅路径应将事件喂入抖振检测器
        let bus = EventBus::new();
        let monitor = EfficiencyMonitor::with_event_bus(MonitorConfig::default(), bus.clone());

        // 启动后台订阅
        monitor.start_event_subscriber().expect("启动订阅失败");

        // 给后台任务时间启动
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // 发布 3 次 TTG 切换事件
        for _ in 0..3 {
            bus.publish(make_ttg_switch("Fast", "Deep"))
                .await
                .expect("发布失败");
        }

        // 给后台任务时间处理
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // 验证抖振检测器收到事件
        let report = monitor.detect_oscillation();
        assert_eq!(
            report.ttg_switches_in_window, 3,
            "后台订阅应将 TTG 事件喂入抖振检测器"
        );
        assert_eq!(report.oscillation_pairs, 1);
    }

    #[test]
    fn test_collect_metrics_includes_oscillation_samples() {
        // 抖振检测器的 collect_metrics() 应返回 4 个指标样本
        let monitor = EfficiencyMonitor::new(MonitorConfig::default());
        monitor.record_event(&make_ttg_switch("Fast", "Deep"));

        let detector = monitor.oscillation_detector();
        let samples = detector.collect_metrics();

        assert_eq!(samples.len(), 4);
        assert!(samples
            .iter()
            .any(|s| s.name == "policy_oscillation_severity"));
        assert!(samples
            .iter()
            .any(|s| s.name == "policy_oscillation_ttg_switches_in_window"));
    }

    // ============================================================
    // MCA 亲和指标集成测试
    // ============================================================

    #[test]
    fn test_new_initializes_affinity_metrics() {
        // EfficiencyMonitor::new() 应初始化 affinity_metrics
        let monitor = EfficiencyMonitor::new(MonitorConfig::default());
        let am = monitor.affinity_metrics();
        // 查询未记录的通道应返回 None
        assert_eq!(am.ttft_percentile("test/t-model", 0.50), None);
    }

    #[test]
    fn test_record_event_feeds_affinity_metrics() {
        // record_event 应将 MCA 事件喂入 affinity_metrics 采集器
        let monitor = EfficiencyMonitor::new(MonitorConfig::default());

        // 记录 StreamSessionCompleted 事件
        monitor.record_event(&make_stream_session("zhipu/glm-5.2", 150));

        // 验证 TTFT 被记录
        let am = monitor.affinity_metrics();
        assert_eq!(am.ttft_percentile("zhipu/glm-5.2", 0.50), Some(150));
        assert!((am.cache_hit_rate("zhipu/glm-5.2").unwrap() - 0.3).abs() < 1e-6);
    }

    #[test]
    fn test_record_event_feeds_affinity_negotiation() {
        // record_event 应将 AffinityCapabilityNegotiated 事件喂入采集器
        let monitor = EfficiencyMonitor::new(MonitorConfig::default());

        monitor.record_event(&make_affinity_negotiation("zhipu/glm-5.2", "full_fidelity"));

        let am = monitor.affinity_metrics();
        let rate = am.feature_enablement_rate("zhipu/glm-5.2").unwrap();
        assert!((rate - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_record_event_feeds_affinity_degraded() {
        // record_event 应将 ProviderDegraded 事件喂入采集器
        let monitor = EfficiencyMonitor::new(MonitorConfig::default());

        monitor.record_event(&make_provider_degraded("zhipu/glm-5.2"));

        // 验证降级计数
        let samples = monitor.affinity_metrics().collect();
        let degraded = samples
            .iter()
            .find(|s| s.name == "mca_provider_degraded_total")
            .expect("应有降级计数样本");
        assert!((degraded.value - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_render_metrics_contains_mca_metrics() {
        // render_metrics 输出应包含 mca_* 亲和指标
        let monitor = EfficiencyMonitor::new(MonitorConfig::default());
        monitor.record_event(&make_stream_session("zhipu/glm-5.2", 150));

        let output = monitor.render_metrics();
        assert!(
            output.contains("mca_ttft_p50_ms"),
            "应包含 mca_ttft_p50_ms 指标"
        );
        assert!(
            output.contains("mca_ttft_p95_ms"),
            "应包含 mca_ttft_p95_ms 指标"
        );
        assert!(
            output.contains("mca_cache_hit_rate"),
            "应包含 mca_cache_hit_rate 指标"
        );
        assert!(
            output.contains("mca_feature_enablement_rate"),
            "应包含 mca_feature_enablement_rate 指标"
        );
        // 验证标签含 route 维度
        assert!(
            output.contains(r#"route="zhipu/glm-5.2""#),
            "指标标签应含 route 维度"
        );
    }

    #[test]
    fn test_affinity_metrics_accessible_via_monitor() {
        // affinity_metrics() 访问器应返回有效引用
        let monitor = EfficiencyMonitor::new(MonitorConfig::default());
        let am = monitor.affinity_metrics();

        // 通过引用记录事件
        am.record_session("test/t-model", 100, 0, 0, 0);
        assert_eq!(am.ttft_percentile("test/t-model", 0.50), Some(100));
    }

    #[tokio::test]
    async fn test_background_subscriber_feeds_affinity_metrics() {
        // 后台订阅路径应将 MCA 事件喂入 affinity_metrics 采集器
        let bus = EventBus::new();
        let monitor = EfficiencyMonitor::with_event_bus(MonitorConfig::default(), bus.clone());

        // 启动后台订阅
        monitor.start_event_subscriber().expect("启动订阅失败");

        // 给后台任务时间启动
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // 发布 MCA 事件
        bus.publish(make_stream_session("zhipu/glm-5.2", 200))
            .await
            .expect("发布失败");
        bus.publish(make_affinity_negotiation("zhipu/glm-5.2", "full_fidelity"))
            .await
            .expect("发布失败");

        // 给后台任务时间处理
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        // 验证亲和度量被记录
        let am = monitor.affinity_metrics();
        assert_eq!(
            am.ttft_percentile("zhipu/glm-5.2", 0.50),
            Some(200),
            "后台订阅应将 StreamSessionCompleted 事件喂入 affinity_metrics"
        );
        let rate = am.feature_enablement_rate("zhipu/glm-5.2").unwrap();
        assert!(
            (rate - 1.0).abs() < 1e-6,
            "后台订阅应将 AffinityCapabilityNegotiated 事件喂入 affinity_metrics"
        );
    }
}
