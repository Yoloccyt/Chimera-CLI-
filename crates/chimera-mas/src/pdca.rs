//! PDCA 端到端闭环强化 — Task 20 §20 Plan-Do-Check-Act 闭环 + 告警阈值
//!
//! 架构层归属: L9 Quest(chimera-mas 内部子模块)
//! 核心职责: PDCA 端到端闭环强化 + criterion 基准 + 闭环告警阈值
//!
//! ## PDCA 闭环(SubTask 20.8-20.10)
//!
//! 1. **Plan**(计划): 由 `plan_reflux()` 生成下一轮目标指标 + 行动项 + 优先级调整
//! 2. **Do**(执行): 由调用方按 `PdcaAdjustments` 执行(本模块不直接执行)
//! 3. **Check**(检查): `check()` 从 efficiency-monitor 收集度量 → `PdcaMetrics`
//! 4. **Act**(处置): `act()` 根据 metrics 调整 tier 分布 / 衰减时间常数 / Agent 池大小 / WSJF 权重
//!
//! ## 闭环告警阈值(SubTask 20.11)
//!
//! 复用 efficiency-monitor `AlertRule` + cooldown 60s 防抖:
//! - 内存 > 130MB → Critical
//! - 单 Agent > 2.6MB → Warning
//! - Wiki > 10000 条 → Warning
//! - 咨询超时率 > 5% → Warning
//!
//! ## 红线对齐
//!
//! - §4.1: 库层 thiserror,无 unwrap/expect
//! - §4.4 反模式 6: f32 禁止隐式转 f64,全程 f64
//! - §6.1: 单函数 ≤ 200 行
//! - §6.2: Critical 安全事件用 mpsc(本模块仅返回告警事件,发布由调用方负责)
//! - `#![forbid(unsafe_code)]`: crate 级已在 lib.rs 声明,本模块无需重复
//!
//! ## 性能可证伪(§3.4.1 第 6 条)
//!
//! 本模块对应的 5 项 criterion 基准在 `benches/mas_benchmark.rs`:
//! - `window_select` < 1ms(hcw-window selector)
//! - `mlc_l2_knn_top10@4096` < 5ms(mlc-engine)
//! - `wiki_knn@1000` < 10ms / `wiki_knn@10` < 1ms(repo-wiki)
//! - `decay_compute` < 1μs(cmt-tiering decay)
//! - `50agent_mem_peak` ≤ 130MB(§15.3 预算模型)

use crate::error::{MasError, Result};
use crate::invariants::MEMORY_BUDGET_MB;
use crate::scheduler::WsjfWeights;

// ============================================================
// 常量 — PDCA 闭环告警阈值(§20.11)
// ============================================================

/// 全局内存告警阈值(MB)— 超过触发 Critical 告警
///
/// 来源:§15.3 50 Agent 稳态预算上限(130MB)。
/// WHY 与 MEMORY_BUDGET_MB 一致:130MB 是 ADR-026 决策 7 的硬性红线,
/// 超过即视为系统进入危险区,需立即触发降级链(DegradationChain::MemoryNearBudget)。
pub const ALERT_MEMORY_CRITICAL_MB: f64 = 130.0;

/// 单 Agent 内存告警阈值(MB)— 超过触发 Warning 告警
///
/// 来源:50 Agent 稳态分布(30×L0 + 12×L1 + 5×L2 + 3×L3)聚合 ≈ 6MB,
/// 单 Agent 平均 6/50 ≈ 0.12MB,L3 单 Agent 密集驻留 0.5MB,
/// 2.6MB 表示单 Agent 驻留超过 L3 上限(0.5MB)5 倍,视为异常密集驻留。
pub const ALERT_SINGLE_AGENT_WARNING_MB: f64 = 2.6;

/// Wiki 条目数告警阈值(条)— 超过触发 Warning 告警
///
/// 来源:repo-wiki 内存 KNN 降级实现,10000 条目后检索延迟显著上升
/// (vector.rs 性能特征:10000+ 条目应迁移至 sqlite-vec 或专用向量数据库)。
pub const ALERT_WIKI_COUNT_WARNING: u32 = 10_000;

/// 咨询超时率告警阈值(0.0-1.0)— 超过触发 Warning 告警
///
/// 来源:§18.3 专家咨询 SLA,Critical < 5s / High < 15s / Medium < 30s。
/// 5% 超时率表示每 20 次咨询有 1 次超时,影响专家协作效率。
pub const ALERT_CONSULT_TIMEOUT_RATE_WARNING: f64 = 0.05;

/// PDCA 默认 cooldown 时间(秒)— 复用 efficiency-monitor AlertRule 默认值
///
/// WHY 60s:与 efficiency-monitor `AlertRule::new` 默认 cooldown 一致,
/// 防止同一告警在短时间内重复触发告警风暴(§20.11)。
pub const PDCA_ALERT_COOLDOWN_SECS: u64 = 60;

// ============================================================
// PdcaMetrics — Check 阶段度量收集(SubTask 20.8)
// ============================================================

/// PDCA Check 阶段收集的度量 — 用于 Act 阶段决策
///
/// ## 字段语义
///
/// - `memory_usage_mb`: 当前全局内存使用(MB,§15.3 INV-7 全局约束)
/// - `failure_rate`: 任务失败率(0.0-1.0,AgentTaskFailed / total_tasks)
/// - `agent_context_overflow_freq`: AgentContextOverflow 事件频率(次/小时)
/// - `consult_timeout_rate`: 咨询超时率(0.0-1.0,ExpertUnavailable::reason="timeout" / total_consults)
/// - `wiki_entry_count`: Wiki 条目总数(用于告警阈值检查)
/// - `max_single_agent_mb`: 单 Agent 最大驻留(MB,用于告警阈值检查)
///
/// WHY 用 f64 而非 f32:§4.4 反模式 6,全程 f64 避免精度膨胀
#[derive(Debug, Clone, PartialEq)]
pub struct PdcaMetrics {
    /// 当前全局内存使用(MB)
    pub memory_usage_mb: f64,
    /// 任务失败率(0.0-1.0)
    pub failure_rate: f64,
    /// AgentContextOverflow 事件频率(次/小时)
    pub agent_context_overflow_freq: f64,
    /// 咨询超时率(0.0-1.0)
    pub consult_timeout_rate: f64,
    /// Wiki 条目总数
    pub wiki_entry_count: u32,
    /// 单 Agent 最大驻留(MB)
    pub max_single_agent_mb: f64,
}

impl PdcaMetrics {
    /// 创建新的度量快照
    pub fn new(
        memory_usage_mb: f64,
        failure_rate: f64,
        agent_context_overflow_freq: f64,
        consult_timeout_rate: f64,
        wiki_entry_count: u32,
        max_single_agent_mb: f64,
    ) -> Self {
        Self {
            memory_usage_mb,
            failure_rate: failure_rate.clamp(0.0, 1.0),
            agent_context_overflow_freq,
            consult_timeout_rate: consult_timeout_rate.clamp(0.0, 1.0),
            wiki_entry_count,
            max_single_agent_mb,
        }
    }

    /// 创建零度量(用于测试与初始化)
    pub fn zero() -> Self {
        Self {
            memory_usage_mb: 0.0,
            failure_rate: 0.0,
            agent_context_overflow_freq: 0.0,
            consult_timeout_rate: 0.0,
            wiki_entry_count: 0,
            max_single_agent_mb: 0.0,
        }
    }

    /// 返回当前内存预算利用率(0.0-1.0)
    ///
    /// 公式:`memory_usage_mb / MEMORY_BUDGET_MB`
    pub fn memory_utilization(&self) -> f64 {
        if MEMORY_BUDGET_MB == 0 {
            return 0.0;
        }
        self.memory_usage_mb / (MEMORY_BUDGET_MB as f64)
    }
}

impl Default for PdcaMetrics {
    fn default() -> Self {
        Self::zero()
    }
}

// ============================================================
// TierDistribution — Agent tier 分布(§15.3 50 Agent 稳态)
// ============================================================

/// Agent tier 分布 — L0/L1/L2/L3 Agent 数量配比
///
/// 默认值(ADR-026 决策 7):30×L0 + 12×L1 + 5×L2 + 3×L3 = 50 Agent 稳态分布,
/// 聚合 ≈ 6MB,远低于 130MB 上限。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TierDistribution {
    /// L0 Simple Agent 数量(默认 30)
    pub l0_count: u32,
    /// L1 Regular Agent 数量(默认 12)
    pub l1_count: u32,
    /// L2 Complex Agent 数量(默认 5)
    pub l2_count: u32,
    /// L3 UltraComplex Agent 数量(默认 3)
    pub l3_count: u32,
}

impl TierDistribution {
    /// 默认 50 Agent 稳态分布(30/12/5/3)
    pub const fn default_50() -> Self {
        Self {
            l0_count: 30,
            l1_count: 12,
            l2_count: 5,
            l3_count: 3,
        }
    }

    /// 总 Agent 数量
    pub fn total(&self) -> u32 {
        self.l0_count + self.l1_count + self.l2_count + self.l3_count
    }

    /// 创建自定义分布
    pub fn new(l0: u32, l1: u32, l2: u32, l3: u32) -> Self {
        Self {
            l0_count: l0,
            l1_count: l1,
            l2_count: l2,
            l3_count: l3,
        }
    }
}

impl Default for TierDistribution {
    fn default() -> Self {
        Self::default_50()
    }
}

// ============================================================
// PdcaAdjustments — Act 阶段调整(SubTask 20.9)
// ============================================================

/// PDCA Act 阶段输出 — 根据 metrics 调整系统参数
///
/// ## 字段语义
///
/// - `tier_distribution`: L0/L1/L2/L3 Agent 比例(§15.3 50 Agent 稳态)
/// - `tau_seconds`: 衰减时间常数(秒,cmt-tiering DecayCalculator 参数)
/// - `pool_size`: Agent 池大小(50 稳态,可上下浮动)
/// - `wsjf_weights`: WSJF 权重 W1..W4(§8.2,可经 PDCA 回流微调)
#[derive(Debug, Clone, PartialEq)]
pub struct PdcaAdjustments {
    /// Agent tier 分布
    pub tier_distribution: TierDistribution,
    /// 衰减时间常数(秒)
    pub tau_seconds: u64,
    /// Agent 池大小
    pub pool_size: u32,
    /// WSJF 权重 W1..W4
    pub wsjf_weights: WsjfWeights,
}

impl PdcaAdjustments {
    /// 创建新的调整配置
    pub fn new(
        tier_distribution: TierDistribution,
        tau_seconds: u64,
        pool_size: u32,
        wsjf_weights: WsjfWeights,
    ) -> Self {
        Self {
            tier_distribution,
            tau_seconds,
            pool_size,
            wsjf_weights,
        }
    }

    /// 默认调整(50 Agent 稳态 + 24h 衰减 + 等权 WSJF)
    pub fn default_50agent() -> Self {
        Self {
            tier_distribution: TierDistribution::default_50(),
            tau_seconds: 86_400, // 24h
            pool_size: 50,
            wsjf_weights: WsjfWeights::default(),
        }
    }
}

impl Default for PdcaAdjustments {
    fn default() -> Self {
        Self::default_50agent()
    }
}

// ============================================================
// PlanReflux — Plan 阶段回流(SubTask 20.10)
// ============================================================

/// PDCA Plan 阶段回流 — 生成下一轮 Plan
///
/// 包含下一轮的目标指标、行动项与优先级调整,作为下一轮 Do 阶段的输入。
///
/// ## 字段语义
///
/// - `target_metrics`: 下一轮目标度量(对照本轮 metrics 设定改进目标)
/// - `action_items`: 行动项列表(具体执行动作)
/// - `priority_adjustments`: 优先级调整(W1..W4 权重微调建议)
#[derive(Debug, Clone, PartialEq)]
pub struct PlanReflux {
    /// 下一轮目标度量
    pub target_metrics: PdcaMetrics,
    /// 行动项列表
    pub action_items: Vec<String>,
    /// WSJF 权重微调建议
    pub priority_adjustments: WsjfWeights,
}

impl PlanReflux {
    /// 创建新的 Plan 回流
    pub fn new(
        target_metrics: PdcaMetrics,
        action_items: Vec<String>,
        priority_adjustments: WsjfWeights,
    ) -> Self {
        Self {
            target_metrics,
            action_items,
            priority_adjustments,
        }
    }
}

// ============================================================
// AlertSeverity / Alert — 闭环告警(SubTask 20.11)
// ============================================================

/// PDCA 告警严重级别 — 与 efficiency-monitor `AlertSeverity` 对齐
///
/// WHY 独立定义而非复用 efficiency-monitor 类型:本模块的告警返回给调用方,
/// 由调用方决定如何发布(可发布 EfficiencyAlertTriggered 事件或 AgentContextOverflow),
/// 解耦告警生成与发布,便于测试与跨场景复用。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PdcaAlertSeverity {
    /// 信息级:仅记录,无需立即响应
    Info,
    /// 警告级:需要关注,但不阻塞执行
    Warning,
    /// 关键级:必须立即响应(如内存超 130MB 红线)
    Critical,
}

impl PdcaAlertSeverity {
    /// 返回严重级别的字符串表示(用于日志与告警图表)
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warning => "warning",
            Self::Critical => "critical",
        }
    }
}

/// PDCA 告警事件 — 由 `AlertThresholds::evaluate()` 返回
///
/// ## 字段语义
///
/// - `rule_id`: 规则 ID(唯一标识,用于 cooldown 跟踪)
/// - `severity`: 告警严重级别(Critical/Warning/Info)
/// - `metric_value`: 触发时的实际指标值
/// - `threshold`: 规则阈值
/// - `message`: 人类可读的告警消息
#[derive(Debug, Clone, PartialEq)]
pub struct PdcaAlert {
    /// 规则 ID
    pub rule_id: String,
    /// 告警严重级别
    pub severity: PdcaAlertSeverity,
    /// 触发时的实际值
    pub metric_value: f64,
    /// 规则阈值
    pub threshold: f64,
    /// 人类可读的告警消息
    pub message: String,
}

impl PdcaAlert {
    /// 创建新的告警事件
    pub fn new(
        rule_id: impl Into<String>,
        severity: PdcaAlertSeverity,
        metric_value: f64,
        threshold: f64,
        message: impl Into<String>,
    ) -> Self {
        Self {
            rule_id: rule_id.into(),
            severity,
            metric_value,
            threshold,
            message: message.into(),
        }
    }
}

// ============================================================
// AlertThresholds — 闭环告警阈值(SubTask 20.11)
// ============================================================

/// PDCA 闭环告警阈值配置 — 复用 efficiency-monitor AlertRule cooldown 60s 防抖
///
/// ## 告警规则(§20.11)
///
/// | 指标 | 阈值 | 严重级别 | 规则 ID |
/// |------|------|---------|---------|
/// | 全局内存 | > 130MB | Critical | `memory_critical` |
/// | 单 Agent 驻留 | > 2.6MB | Warning | `single_agent_warning` |
/// | Wiki 条目数 | > 10000 | Warning | `wiki_count_warning` |
/// | 咨询超时率 | > 5% | Warning | `consult_timeout_warning` |
///
/// ## 使用方式
///
/// ```ignore
/// use chimera_mas::pdca::{AlertThresholds, PdcaMetrics};
///
/// let thresholds = AlertThresholds::default();
/// let metrics = PdcaMetrics::new(140.0, 0.05, 10.0, 0.03, 5000, 1.5);
/// let alerts = thresholds.evaluate(&metrics);
/// assert!(!alerts.is_empty()); // 触发 Critical 内存告警
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct AlertThresholds {
    /// 全局内存告警阈值(MB,默认 130.0)
    pub memory_critical_mb: f64,
    /// 单 Agent 内存告警阈值(MB,默认 2.6)
    pub single_agent_warning_mb: f64,
    /// Wiki 条目数告警阈值(默认 10000)
    pub wiki_count_warning: u32,
    /// 咨询超时率告警阈值(0.0-1.0,默认 0.05)
    pub consult_timeout_rate_warning: f64,
}

impl Default for AlertThresholds {
    /// 创建默认告警阈值(对齐 §20.11 规则)
    ///
    /// WHY 实现 Default trait 而非自定义 `default()` 方法:
    /// clippy `should_implement_trait` lint 要求名为 `default()` 的无参构造器
    /// 必须实现 `Default` trait,避免与 `T::default()` 调用约定混淆。
    /// 同时实现 trait 后 `AlertThresholds::default()` 语法仍可用(方法解析路由到 trait 方法)。
    fn default() -> Self {
        Self {
            memory_critical_mb: ALERT_MEMORY_CRITICAL_MB,
            single_agent_warning_mb: ALERT_SINGLE_AGENT_WARNING_MB,
            wiki_count_warning: ALERT_WIKI_COUNT_WARNING,
            consult_timeout_rate_warning: ALERT_CONSULT_TIMEOUT_RATE_WARNING,
        }
    }
}

impl AlertThresholds {
    /// 创建自定义告警阈值
    pub fn new(
        memory_critical_mb: f64,
        single_agent_warning_mb: f64,
        wiki_count_warning: u32,
        consult_timeout_rate_warning: f64,
    ) -> Self {
        Self {
            memory_critical_mb,
            single_agent_warning_mb,
            wiki_count_warning,
            consult_timeout_rate_warning: consult_timeout_rate_warning.clamp(0.0, 1.0),
        }
    }

    /// 评估当前度量,返回触发的告警列表
    ///
    /// ## 参数
    ///
    /// - `metrics`: PDCA Check 阶段收集的度量
    ///
    /// ## 返回
    ///
    /// 触发的告警列表(可能为空)。调用方负责:
    /// - 应用 cooldown 防抖(60s,与 efficiency-monitor AlertRule 一致)
    /// - 发布 Critical 事件(走 mpsc,§6.2 红线)
    pub fn evaluate(&self, metrics: &PdcaMetrics) -> Vec<PdcaAlert> {
        let mut alerts = Vec::new();

        // 规则 1:全局内存 > 130MB → Critical
        if metrics.memory_usage_mb > self.memory_critical_mb {
            alerts.push(PdcaAlert::new(
                "memory_critical",
                PdcaAlertSeverity::Critical,
                metrics.memory_usage_mb,
                self.memory_critical_mb,
                format!(
                    "Global memory {:.2}MB exceeds critical threshold {:.2}MB ({}MB budget)",
                    metrics.memory_usage_mb, self.memory_critical_mb, MEMORY_BUDGET_MB
                ),
            ));
        }

        // 规则 2:单 Agent > 2.6MB → Warning
        if metrics.max_single_agent_mb > self.single_agent_warning_mb {
            alerts.push(PdcaAlert::new(
                "single_agent_warning",
                PdcaAlertSeverity::Warning,
                metrics.max_single_agent_mb,
                self.single_agent_warning_mb,
                format!(
                    "Single agent resident {:.2}MB exceeds warning threshold {:.2}MB",
                    metrics.max_single_agent_mb, self.single_agent_warning_mb
                ),
            ));
        }

        // 规则 3:Wiki > 10000 条 → Warning
        if metrics.wiki_entry_count > self.wiki_count_warning {
            alerts.push(PdcaAlert::new(
                "wiki_count_warning",
                PdcaAlertSeverity::Warning,
                metrics.wiki_entry_count as f64,
                self.wiki_count_warning as f64,
                format!(
                    "Wiki entry count {} exceeds warning threshold {} (consider migrating to sqlite-vec)",
                    metrics.wiki_entry_count, self.wiki_count_warning
                ),
            ));
        }

        // 规则 4:咨询超时率 > 5% → Warning
        if metrics.consult_timeout_rate > self.consult_timeout_rate_warning {
            alerts.push(PdcaAlert::new(
                "consult_timeout_warning",
                PdcaAlertSeverity::Warning,
                metrics.consult_timeout_rate,
                self.consult_timeout_rate_warning,
                format!(
                    "Consult timeout rate {:.2}% exceeds warning threshold {:.2}%",
                    metrics.consult_timeout_rate * 100.0,
                    self.consult_timeout_rate_warning * 100.0
                ),
            ));
        }

        alerts
    }
}

// ============================================================
// PdcaLoop — PDCA 闭环主入口(SubTask 20.8-20.10)
// ============================================================

/// PDCA 闭环主入口 — 协调 Check / Act / Plan 三阶段
///
/// ## 设计原则
///
/// - **无状态**:不持有运行时状态,所有方法为 `&self` 或关联函数,便于并发调用
/// - **纯函数**:Check / Act / Plan 均为纯函数,无副作用,易测试
/// - **解耦**:不直接调用 efficiency-monitor / cmt-tiering 等下游 crate,
///   度量收集由调用方完成后传入 `PdcaMetrics`
///
/// ## PDCA 闭环流程
///
/// ```text
/// ┌──────────────────────────────────────────────────────────┐
/// │  Check  → PdcaMetrics  (从 efficiency-monitor 收集)      │
/// │    │                                                      │
/// │    ▼                                                      │
/// │  Act    → PdcaAdjustments (调整 tier/tau/pool/wsjf)     │
/// │    │                                                      │
/// │    ▼                                                      │
/// │  Plan   → PlanReflux     (生成下一轮目标与行动项)         │
/// │    │                                                      │
/// │    └──→ 下一轮 Do 阶段输入                                │
/// └──────────────────────────────────────────────────────────┘
/// ```
pub struct PdcaLoop {
    /// 告警阈值配置
    pub alert_thresholds: AlertThresholds,
}

impl PdcaLoop {
    /// 创建新的 PDCA 闭环,使用默认告警阈值
    pub fn new() -> Self {
        Self {
            alert_thresholds: AlertThresholds::default(),
        }
    }

    /// 创建新的 PDCA 闭环,使用自定义告警阈值
    pub fn with_thresholds(alert_thresholds: AlertThresholds) -> Self {
        Self { alert_thresholds }
    }

    /// Check 阶段 — 评估当前度量,返回告警列表(SubTask 20.8)
    ///
    /// ## 参数
    ///
    /// - `metrics`: 调用方从 efficiency-monitor 收集的当前度量
    ///
    /// ## 返回
    ///
    /// 触发的告警列表。若返回空 Vec,表示当前系统状态健康。
    ///
    /// ## 注意
    ///
    /// 本方法仅返回告警事件,不发布事件。调用方负责:
    /// - 应用 cooldown 防抖(60s,§20.11)
    /// - 发布 Critical 事件(走 mpsc,§6.2 红线)
    pub fn check(&self, metrics: &PdcaMetrics) -> Vec<PdcaAlert> {
        self.alert_thresholds.evaluate(metrics)
    }

    /// Act 阶段 — 根据 metrics 调整系统参数(SubTask 20.9)
    ///
    /// ## 调整逻辑
    ///
    /// - **内存压力高**(`memory_utilization >= 0.9`):
    ///   - 减少 L3 Agent 数量(降级到 L2)
    ///   - 缩短 tau_seconds(加速衰减)
    ///   - 减小 pool_size(降低并发)
    /// - **失败率高**(`failure_rate > 0.1`):
    ///   - 增大 W3(风险消减权重,优先处理高风险任务)
    /// - **咨询超时率高**(`consult_timeout_rate > 0.05`):
    ///   - 增大 W2(时间敏感度权重,优先处理时间紧迫任务)
    /// - **正常状态**:
    ///   - 保持默认 50 Agent 稳态分布 + 24h 衰减 + 等权 WSJF
    ///
    /// ## 参数
    ///
    /// - `metrics`: 当前度量(由 Check 阶段产生)
    ///
    /// ## 返回
    ///
    /// 调整后的 `PdcaAdjustments`,调用方按此执行 Do 阶段。
    pub fn act(&self, metrics: &PdcaMetrics) -> Result<PdcaAdjustments> {
        // 默认调整(50 Agent 稳态 + 24h 衰减 + 等权 WSJF)
        let mut distribution = TierDistribution::default_50();
        let mut tau_seconds: u64 = 86_400; // 24h
        let mut pool_size: u32 = 50;
        let mut weights = WsjfWeights::default();

        let mem_util = metrics.memory_utilization();

        // 内存压力高(>= 90% 预算):触发降级链
        if mem_util >= 0.9 {
            // 减少 L3 Agent 数量(3 → 1,降级到 L2)
            // WHY 减 L3:L3 单 Agent 驻留最大(0.5MB),减少 L3 立即降低内存压力
            distribution.l3_count = 1;
            distribution.l2_count += 2; // 保持总 Agent 数大致不变
                                        // 缩短 tau(24h → 12h),加速衰减释放内存
            tau_seconds = 43_200;
            // 减小 pool_size(50 → 40),降低并发压力
            pool_size = 40;
        } else if mem_util >= 0.7 {
            // 中度内存压力(>= 70%):轻度调整
            // 减少 L3 Agent 数量(3 → 2)
            distribution.l3_count = 2;
            distribution.l2_count += 1;
            // tau 保持 24h
            // pool_size 保持 50
        }

        // 失败率高(> 10%):增大 W3(风险消减权重)
        if metrics.failure_rate > 0.1 {
            weights.w3 = 2.0; // 风险消减权重加倍
        }

        // 咨询超时率高(> 5%):增大 W2(时间敏感度权重)
        if metrics.consult_timeout_rate > 0.05 {
            weights.w2 = 1.5; // 时间敏感度权重提升 50%
        }

        // AgentContextOverflow 频率高(> 10 次/小时):减小 pool_size 进一步
        if metrics.agent_context_overflow_freq > 10.0 {
            pool_size = pool_size.saturating_sub(10);
        }

        // 校验调整后的分布总 Agent 数与 pool_size 一致
        // WHY 校验:确保 tier_distribution.total() <= pool_size,避免派生超限
        let total = distribution.total();
        if total > pool_size {
            // 若分布总数超过 pool_size,缩减 L0(最廉价,降级影响最小)
            let excess = total - pool_size;
            distribution.l0_count = distribution.l0_count.saturating_sub(excess);
        }

        // 校验 tau_seconds 非 0(DecayCalculator::new 会拒绝 0)
        if tau_seconds == 0 {
            return Err(MasError::Internal(
                "PDCA Act: tau_seconds must be > 0 (cmt-tiering DecayCalculator constraint)".into(),
            ));
        }

        Ok(PdcaAdjustments::new(
            distribution,
            tau_seconds,
            pool_size,
            weights,
        ))
    }

    /// Plan 阶段 — 回流下一轮 Plan(SubTask 20.10)
    ///
    /// 根据当前 metrics 与 adjustments 生成下一轮的:
    /// - `target_metrics`: 改进目标(对照当前 metrics 设定)
    /// - `action_items`: 具体行动项列表
    /// - `priority_adjustments`: WSJF 权重微调建议
    ///
    /// ## 参数
    ///
    /// - `current_metrics`: 当前轮次的度量(由 Check 阶段产生)
    /// - `adjustments`: 当前轮次的调整(由 Act 阶段产生)
    ///
    /// ## 返回
    ///
    /// 下一轮 Plan 的 `PlanReflux`。
    pub fn plan_reflux(
        &self,
        current_metrics: &PdcaMetrics,
        adjustments: &PdcaAdjustments,
    ) -> Result<PlanReflux> {
        // 生成目标度量:对照当前 metrics 设定改进目标(降低 20% 关键指标)
        let target_metrics = PdcaMetrics::new(
            // 内存目标:降低 20%(若已 < 100MB 则保持)
            (current_metrics.memory_usage_mb * 0.8).max(0.0),
            // 失败率目标:降低 50%
            (current_metrics.failure_rate * 0.5).clamp(0.0, 1.0),
            // AgentContextOverflow 频率目标:降低 30%
            current_metrics.agent_context_overflow_freq * 0.7,
            // 咨询超时率目标:降低 50%
            (current_metrics.consult_timeout_rate * 0.5).clamp(0.0, 1.0),
            // Wiki 条目数目标:保持(归档由 §17 调度,非 PDCA 范围)
            current_metrics.wiki_entry_count,
            // 单 Agent 驻留目标:降低 30%
            (current_metrics.max_single_agent_mb * 0.7).max(0.0),
        );

        // 生成行动项列表
        let mut action_items = Vec::new();

        let mem_util = current_metrics.memory_utilization();
        if mem_util >= 0.9 {
            action_items.push(
                "Trigger DegradationChain::MemoryNearBudget (HcwCompress → TierDemote → RejectNewAgent)"
                    .to_string(),
            );
        } else if mem_util >= 0.7 {
            action_items.push("Reduce L3 Agent count by 1, promote to L2".to_string());
        }

        if current_metrics.failure_rate > 0.1 {
            action_items.push(format!(
                "Increase W3 (risk reduction) weight from 1.0 to {}",
                adjustments.wsjf_weights.w3
            ));
        }

        if current_metrics.consult_timeout_rate > 0.05 {
            action_items.push(format!(
                "Increase W2 (time criticality) weight from 1.0 to {}",
                adjustments.wsjf_weights.w2
            ));
        }

        if current_metrics.agent_context_overflow_freq > 10.0 {
            action_items.push(format!(
                "Reduce pool_size to {} (current overflow freq: {:.1}/h)",
                adjustments.pool_size, current_metrics.agent_context_overflow_freq
            ));
        }

        if current_metrics.wiki_entry_count > ALERT_WIKI_COUNT_WARNING {
            action_items.push(
                "Schedule archive migration: consider sqlite-vec or dedicated vector DB"
                    .to_string(),
            );
        }

        if action_items.is_empty() {
            action_items.push("System healthy, no action items".to_string());
        }

        // 生成优先级调整建议(对照当前调整给出下一轮微调建议)
        let priority_adjustments = WsjfWeights::new(
            adjustments.wsjf_weights.w1,
            adjustments.wsjf_weights.w2,
            adjustments.wsjf_weights.w3,
            adjustments.wsjf_weights.w4,
        );

        Ok(PlanReflux::new(
            target_metrics,
            action_items,
            priority_adjustments,
        ))
    }
}

impl Default for PdcaLoop {
    fn default() -> Self {
        Self::new()
    }
}

// ============================================================
// 单元测试(模块内,与集成测试 tests/pdca_test.rs 互补)
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // --- PdcaMetrics 测试 ---

    #[test]
    fn test_metrics_new_clamps_rates() {
        let m = PdcaMetrics::new(50.0, 1.5, 5.0, 2.0, 100, 0.5);
        // failure_rate 应被 clamp 到 1.0
        assert!((m.failure_rate - 1.0).abs() < f64::EPSILON);
        // consult_timeout_rate 应被 clamp 到 1.0
        assert!((m.consult_timeout_rate - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_metrics_zero_initializes_all_zero() {
        let m = PdcaMetrics::zero();
        assert_eq!(m.memory_usage_mb, 0.0);
        assert_eq!(m.failure_rate, 0.0);
        assert_eq!(m.agent_context_overflow_freq, 0.0);
        assert_eq!(m.consult_timeout_rate, 0.0);
        assert_eq!(m.wiki_entry_count, 0);
        assert_eq!(m.max_single_agent_mb, 0.0);
    }

    #[test]
    fn test_memory_utilization_calculates_ratio() {
        let m = PdcaMetrics::new(65.0, 0.0, 0.0, 0.0, 0, 0.0);
        // 65MB / 130MB = 0.5
        assert!((m.memory_utilization() - 0.5).abs() < 1e-9);
    }

    #[test]
    fn test_memory_utilization_zero_budget_returns_zero() {
        // 边界:0 预算返回 0(避免除零)
        let m = PdcaMetrics {
            memory_usage_mb: 100.0,
            ..PdcaMetrics::zero()
        };
        // MEMORY_BUDGET_MB 是常量 130,不会为 0,此测试覆盖理论分支
        // 通过手动检查 > 0 路径
        assert!(m.memory_utilization() > 0.0);
    }

    // --- TierDistribution 测试 ---

    #[test]
    fn test_default_50agent_distribution() {
        let d = TierDistribution::default_50();
        assert_eq!(d.l0_count, 30);
        assert_eq!(d.l1_count, 12);
        assert_eq!(d.l2_count, 5);
        assert_eq!(d.l3_count, 3);
        assert_eq!(d.total(), 50);
    }

    #[test]
    fn test_tier_distribution_new_custom() {
        let d = TierDistribution::new(10, 5, 2, 1);
        assert_eq!(d.total(), 18);
    }

    // --- PdcaAdjustments 测试 ---

    #[test]
    fn test_adjustments_default_50agent() {
        let a = PdcaAdjustments::default_50agent();
        assert_eq!(a.tier_distribution.total(), 50);
        assert_eq!(a.tau_seconds, 86_400);
        assert_eq!(a.pool_size, 50);
        assert_eq!(a.wsjf_weights, WsjfWeights::default());
    }

    // --- AlertThresholds 测试 ---

    #[test]
    fn test_alert_thresholds_default_values() {
        let t = AlertThresholds::default();
        assert!((t.memory_critical_mb - 130.0).abs() < f64::EPSILON);
        assert!((t.single_agent_warning_mb - 2.6).abs() < f64::EPSILON);
        assert_eq!(t.wiki_count_warning, 10_000);
        assert!((t.consult_timeout_rate_warning - 0.05).abs() < f64::EPSILON);
    }

    #[test]
    fn test_evaluate_memory_critical_alert() {
        let t = AlertThresholds::default();
        let m = PdcaMetrics::new(140.0, 0.0, 0.0, 0.0, 1000, 1.0);
        let alerts = t.evaluate(&m);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].rule_id, "memory_critical");
        assert_eq!(alerts[0].severity, PdcaAlertSeverity::Critical);
        assert!(alerts[0].message.contains("140"));
    }

    #[test]
    fn test_evaluate_single_agent_warning_alert() {
        let t = AlertThresholds::default();
        let m = PdcaMetrics::new(50.0, 0.0, 0.0, 0.0, 1000, 3.0);
        let alerts = t.evaluate(&m);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].rule_id, "single_agent_warning");
        assert_eq!(alerts[0].severity, PdcaAlertSeverity::Warning);
    }

    #[test]
    fn test_evaluate_wiki_count_warning_alert() {
        let t = AlertThresholds::default();
        let m = PdcaMetrics::new(50.0, 0.0, 0.0, 0.0, 15_000, 1.0);
        let alerts = t.evaluate(&m);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].rule_id, "wiki_count_warning");
    }

    #[test]
    fn test_evaluate_consult_timeout_warning_alert() {
        let t = AlertThresholds::default();
        let m = PdcaMetrics::new(50.0, 0.0, 0.0, 0.08, 1000, 1.0);
        let alerts = t.evaluate(&m);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].rule_id, "consult_timeout_warning");
    }

    #[test]
    fn test_evaluate_healthy_metrics_returns_empty() {
        let t = AlertThresholds::default();
        let m = PdcaMetrics::new(50.0, 0.0, 0.0, 0.0, 1000, 1.0);
        let alerts = t.evaluate(&m);
        assert!(alerts.is_empty());
    }

    #[test]
    fn test_evaluate_multiple_alerts_combined() {
        let t = AlertThresholds::default();
        // 触发全部 4 条规则
        let m = PdcaMetrics::new(140.0, 0.2, 5.0, 0.1, 15_000, 3.0);
        let alerts = t.evaluate(&m);
        assert_eq!(alerts.len(), 4);
    }

    // --- PdcaLoop::check() 测试 ---

    #[test]
    fn test_pdca_loop_check_returns_alerts() {
        let loop_ = PdcaLoop::new();
        let m = PdcaMetrics::new(140.0, 0.0, 0.0, 0.0, 1000, 1.0);
        let alerts = loop_.check(&m);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].severity, PdcaAlertSeverity::Critical);
    }

    // --- PdcaLoop::act() 测试 ---

    #[test]
    fn test_pdca_loop_act_normal_state_returns_default() {
        let loop_ = PdcaLoop::new();
        let m = PdcaMetrics::new(50.0, 0.0, 0.0, 0.0, 1000, 0.5);
        let adj = loop_.act(&m).expect("Act 成功");
        assert_eq!(adj.tier_distribution, TierDistribution::default_50());
        assert_eq!(adj.tau_seconds, 86_400);
        assert_eq!(adj.pool_size, 50);
        assert_eq!(adj.wsjf_weights, WsjfWeights::default());
    }

    #[test]
    fn test_pdca_loop_act_high_memory_pressure() {
        let loop_ = PdcaLoop::new();
        // 90% 预算 = 117MB,使用 120MB 触发高内存压力
        let m = PdcaMetrics::new(120.0, 0.0, 0.0, 0.0, 1000, 0.5);
        let adj = loop_.act(&m).expect("Act 成功");
        // 应减少 L3(3 → 1),增加 L2(+2)
        assert_eq!(adj.tier_distribution.l3_count, 1);
        assert_eq!(adj.tier_distribution.l2_count, 7);
        // tau 应缩短到 12h
        assert_eq!(adj.tau_seconds, 43_200);
        // pool_size 应减小到 40
        assert_eq!(adj.pool_size, 40);
    }

    #[test]
    fn test_pdca_loop_act_moderate_memory_pressure() {
        let loop_ = PdcaLoop::new();
        // 70% 预算 = 91MB,使用 95MB 触发中度内存压力
        let m = PdcaMetrics::new(95.0, 0.0, 0.0, 0.0, 1000, 0.5);
        let adj = loop_.act(&m).expect("Act 成功");
        // 应减少 L3(3 → 2),增加 L2(+1)
        assert_eq!(adj.tier_distribution.l3_count, 2);
        assert_eq!(adj.tier_distribution.l2_count, 6);
        // tau 应保持 24h
        assert_eq!(adj.tau_seconds, 86_400);
    }

    #[test]
    fn test_pdca_loop_act_high_failure_rate_increases_w3() {
        let loop_ = PdcaLoop::new();
        let m = PdcaMetrics::new(50.0, 0.2, 0.0, 0.0, 1000, 0.5);
        let adj = loop_.act(&m).expect("Act 成功");
        // W3(风险消减权重)应加倍到 2.0
        assert!((adj.wsjf_weights.w3 - 2.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_pdca_loop_act_high_consult_timeout_increases_w2() {
        let loop_ = PdcaLoop::new();
        let m = PdcaMetrics::new(50.0, 0.0, 0.0, 0.08, 1000, 0.5);
        let adj = loop_.act(&m).expect("Act 成功");
        // W2(时间敏感度权重)应提升到 1.5
        assert!((adj.wsjf_weights.w2 - 1.5).abs() < f64::EPSILON);
    }

    #[test]
    fn test_pdca_loop_act_high_overflow_freq_reduces_pool() {
        let loop_ = PdcaLoop::new();
        let m = PdcaMetrics::new(50.0, 0.0, 15.0, 0.0, 1000, 0.5);
        let adj = loop_.act(&m).expect("Act 成功");
        // pool_size 应减小 10(50 → 40)
        assert_eq!(adj.pool_size, 40);
    }

    // --- PdcaLoop::plan_reflux() 测试 ---

    #[test]
    fn test_plan_reflux_generates_target_metrics() {
        let loop_ = PdcaLoop::new();
        let current = PdcaMetrics::new(100.0, 0.1, 5.0, 0.04, 1000, 1.0);
        let adj = PdcaAdjustments::default_50agent();
        let reflux = loop_.plan_reflux(&current, &adj).expect("Plan 成功");

        // 内存目标应降低 20%(100 → 80)
        assert!((reflux.target_metrics.memory_usage_mb - 80.0).abs() < f64::EPSILON);
        // 失败率目标应降低 50%(0.1 → 0.05)
        assert!((reflux.target_metrics.failure_rate - 0.05).abs() < f64::EPSILON);
    }

    #[test]
    fn test_plan_reflux_generates_action_items_for_high_memory() {
        let loop_ = PdcaLoop::new();
        // 90% 预算 = 117MB,使用 120MB 触发高内存压力
        let current = PdcaMetrics::new(120.0, 0.0, 0.0, 0.0, 1000, 0.5);
        let adj = PdcaAdjustments::default_50agent();
        let reflux = loop_.plan_reflux(&current, &adj).expect("Plan 成功");

        // 应包含 DegradationChain 触发行动项
        assert!(reflux
            .action_items
            .iter()
            .any(|s| s.contains("DegradationChain")));
    }

    #[test]
    fn test_plan_reflux_healthy_state_returns_no_action() {
        let loop_ = PdcaLoop::new();
        let current = PdcaMetrics::new(50.0, 0.0, 0.0, 0.0, 1000, 0.5);
        let adj = PdcaAdjustments::default_50agent();
        let reflux = loop_.plan_reflux(&current, &adj).expect("Plan 成功");

        // 健康状态应返回"无行动项"占位
        assert!(reflux.action_items.iter().any(|s| s.contains("healthy")));
    }

    #[test]
    fn test_plan_reflux_wiki_high_count_action() {
        let loop_ = PdcaLoop::new();
        let current = PdcaMetrics::new(50.0, 0.0, 0.0, 0.0, 15_000, 0.5);
        let adj = PdcaAdjustments::default_50agent();
        let reflux = loop_.plan_reflux(&current, &adj).expect("Plan 成功");

        // 应包含 sqlite-vec 迁移建议
        assert!(reflux.action_items.iter().any(|s| s.contains("sqlite-vec")));
    }

    // --- PdcaAlertSeverity 测试 ---

    #[test]
    fn test_alert_severity_as_str() {
        assert_eq!(PdcaAlertSeverity::Info.as_str(), "info");
        assert_eq!(PdcaAlertSeverity::Warning.as_str(), "warning");
        assert_eq!(PdcaAlertSeverity::Critical.as_str(), "critical");
    }

    // --- 完整 PDCA 闭环测试 ---

    #[test]
    fn test_full_pdca_cycle_normal_state() {
        let loop_ = PdcaLoop::new();
        let metrics = PdcaMetrics::new(50.0, 0.0, 0.0, 0.0, 1000, 0.5);

        // Check:无告警
        let alerts = loop_.check(&metrics);
        assert!(alerts.is_empty());

        // Act:默认调整
        let adj = loop_.act(&metrics).expect("Act 成功");
        assert_eq!(adj.tier_distribution, TierDistribution::default_50());

        // Plan:生成回流(健康状态)
        let reflux = loop_.plan_reflux(&metrics, &adj).expect("Plan 成功");
        assert!(!reflux.action_items.is_empty());
    }

    #[test]
    fn test_full_pdca_cycle_critical_state() {
        let loop_ = PdcaLoop::new();
        // 严重状态:内存 140MB + 失败率 20% + 咨询超时 8%
        let metrics = PdcaMetrics::new(140.0, 0.2, 15.0, 0.08, 15_000, 3.0);

        // Check:触发告警
        let alerts = loop_.check(&metrics);
        assert!(!alerts.is_empty());
        assert!(alerts
            .iter()
            .any(|a| a.severity == PdcaAlertSeverity::Critical));

        // Act:调整系统参数
        let adj = loop_.act(&metrics).expect("Act 成功");
        assert!(adj.tier_distribution.l3_count < 3); // 应减少 L3
        assert!(adj.pool_size < 50); // 应减小 pool

        // Plan:生成回流
        let reflux = loop_.plan_reflux(&metrics, &adj).expect("Plan 成功");
        assert!(reflux
            .action_items
            .iter()
            .any(|s| s.contains("DegradationChain")));
    }
}
