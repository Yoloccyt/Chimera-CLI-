//! TUI 数据源抽象 — 为 L10 Interface 提供统一数据访问契约
//!
//! 设计约束(WHY):
//! - `chimera-tui` 位于 L10,按 §2.2 依赖铁律禁止直接依赖 L9 的
//!   `quest-engine`/`efficiency-monitor`。因此本模块只依赖 L1 的
//!   `event-bus` 与 `nexus-core`(共享领域类型),所有数据通过
//!   `NexusEvent` 事件流推导。
//! - `TuiDataSource` trait 将事件总线细节与面板渲染解耦:面板只读
//!   `DataSnapshot`,不关心数据是实时事件、本地缓存还是测试桩。
//! - `DataSnapshot` 使用本地 `BudgetMetrics` 而非直接暴露 L9 指标类型,
//!   避免跨层泄漏。
//!
//! # 消费的事件变体
//!
//! `DataPipeline` 直接消费 `event-bus` 中已有的以下 `NexusEvent` 变体:
//! - `QuestListUpdated` / `QuestCompleted`:维护 Quest 列表。
//! - `BudgetMetricsUpdated`:更新 Budget 面板指标。
//! - `MemoryMetricsReported` / `ContextWindowSwitched` / `ContextCompressed` /
//!   `CacheStatsReported` / `CacheHit` / `CacheMiss`:更新 Memory 面板指标。
//! - `SkepticVeto` / `RedTeamAudit` / `AsaIntervention` / `CapabilityFrozen`:
//!   更新 Security 面板状态。
//! - `SlowConsumerDropped` / `McpMeshTransactionCompleted`:更新 Health 面板指标。
//! - 其余事件进入 `latest_events` 日志流,供 Log 面板展示。

use crate::error::TuiError;
use crate::subscriber::EventSubscriber;
use chrono::{DateTime, Utc};
use event_bus::{EventMetadata, NexusEvent};
use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use tokio::task::JoinHandle;
use tokio::time::{self, Duration};

/// 数据快照 — TUI 各面板渲染所需数据的统一视图
///
/// WHY 独立结构体:面板渲染只依赖此快照,不依赖具体数据源实现,
/// 方便单元测试用内存桩替换 event-bus 订阅。
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataSnapshot {
    /// 当前活动 Quest 列表
    ///
    /// 来源:聚合 `QuestListUpdated`(替换整个列表)与 `QuestCompleted`
    /// (按 quest_id 移除)事件。使用 `nexus_core::Quest` 保证与 L1 领域模型一致。
    pub quest_list: Vec<Quest>,

    /// 最近接收到的 NexusEvent,按时间顺序,旧在前
    ///
    /// WHY VecDeque:面板需要"最新 N 条"语义,从队尾追加、队首丢弃
    /// 为 O(1),避免频繁 `Vec::remove(0)`。
    pub latest_events: VecDeque<NexusEvent>,

    /// 当前预算指标
    pub budget_metrics: BudgetMetrics,

    /// 当前记忆指标
    pub memory_metrics: MemoryMetrics,

    /// 当前安全状态
    pub security_state: SecurityState,

    /// 当前健康指标
    pub health_metrics: HealthMetrics,

    /// 预算利用率历史(百分比,0-100),用于 Budget 面板 Sparkline
    pub budget_history: Vec<u64>,

    /// 缓存命中率历史(百分比,0-100),用于 Memory 面板 Sparkline
    pub memory_history: Vec<u64>,

    /// 事件速率历史(每秒事件数),用于 Health 面板 Sparkline
    pub event_rate_history: Vec<u64>,
}

/// 预算指标 — TUI Budget 面板的轻量级本地视图
///
/// WHY 不直接复用 `efficiency-monitor` 类型:该 crate 位于 L9,
/// L10 不能直接依赖。本结构体只保留面板展示必需字段,
/// 由 `BudgetMetricsUpdated` 事件直接填充而来。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BudgetMetrics {
    /// 总消耗量(单位由预算类型决定)
    pub total_consumption: f64,
    /// 剩余预算
    pub remaining_budget: f64,
    /// 利用率 [0.0, 1.0]
    pub utilization_rate: f32,
    /// 当前预算档位(如 "High"/"Medium"/"Low")
    pub current_tier: String,
    /// 档位系数,1.0 为基准
    pub coefficient: f32,
    /// 是否已触发预算超限
    pub is_exceeded: bool,
    /// 最新告警信息(无告警为 None)
    pub alert: Option<String>,
}

impl Default for BudgetMetrics {
    fn default() -> Self {
        Self {
            total_consumption: 0.0,
            remaining_budget: 0.0,
            utilization_rate: 0.0,
            current_tier: "High".into(),
            coefficient: 1.0,
            is_exceeded: false,
            alert: None,
        }
    }
}

/// 记忆指标 — TUI Memory 面板的轻量级本地视图
///
/// WHY 不直接复用 `mlc-engine`/`hcw-window` 类型:这些 crate 位于 L2,
/// L10 不能直接依赖。本结构体只保留面板展示必需字段,
/// 由 L1/L2 事件直接填充而来。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MemoryMetrics {
    /// 缓存命中率百分比 [0.0, 100.0]
    pub hit_rate_percent: f32,
    /// 周期内驱逐数
    pub evictions: u64,
    /// 当前上下文窗口大小(字节)
    pub context_window_size: u64,
    /// 压缩率 [0.0, 1.0],compressed_size / original_size
    pub compressed_ratio: f32,
    /// 累计缓存命中次数
    pub cache_hits: u64,
    /// 累计缓存未命中次数
    pub cache_misses: u64,
    /// 当前窗口/缓存层级(如 "L0"/"Hot"/"Warm"/"Cold"/"Ice")
    pub tier: String,
}

impl Default for MemoryMetrics {
    fn default() -> Self {
        Self {
            hit_rate_percent: 0.0,
            evictions: 0,
            context_window_size: 0,
            compressed_ratio: 1.0,
            cache_hits: 0,
            cache_misses: 0,
            tier: "L0".into(),
        }
    }
}

/// Skeptic 否决摘要 — Security 面板展示用
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SkepticVetoSummary {
    /// Quest ID
    pub quest_id: String,
    /// 否决原因
    pub veto_reason: String,
    /// 被冻结的能力 ID 列表
    pub frozen_capabilities: Vec<String>,
    /// 事件发生时间
    pub timestamp: DateTime<Utc>,
}

/// 红队审计摘要 — Security 面板展示用
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RedTeamAuditSummary {
    /// 漏洞类型
    pub vulnerability_type: String,
    /// 失败探测数
    pub failed_probes: u32,
    /// 总探测数
    pub total_probes: u32,
    /// 检测率 [0.0, 1.0]
    pub detection_rate: f32,
    /// 补救建议
    pub remediation_suggestion: String,
    /// 事件发生时间
    pub timestamp: DateTime<Utc>,
}

/// ASA 安全干预摘要 — Security 面板展示用
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AsaInterventionSummary {
    /// 被干预的操作 ID
    pub operation_id: String,
    /// 干预动作(Allow/Warn/Block)
    pub action: String,
    /// 安全评分 [0.0, 1.0]
    pub safety_score: f32,
    /// Block 时的阻断原因
    pub block_reason: Option<String>,
    /// 事件发生时间
    pub timestamp: DateTime<Utc>,
}

/// 安全状态 — TUI Security 面板的轻量级本地视图
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct SecurityState {
    /// 最近 Skeptic 否决事件
    pub active_vetoes: Vec<SkepticVetoSummary>,
    /// 最近红队审计结果
    pub recent_audits: Vec<RedTeamAuditSummary>,
    /// 最近 ASA 安全干预
    pub recent_interventions: Vec<AsaInterventionSummary>,
    /// 当前被冻结的能力 ID 列表
    pub frozen_capabilities: Vec<String>,
}

/// 健康指标 — TUI Health 面板的轻量级本地视图
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct HealthMetrics {
    /// 每秒事件数
    pub events_per_second: f64,
    /// 慢消费者被丢弃次数
    pub slow_consumer_count: u64,
    /// 平均 MCP Mesh 事务延迟(毫秒)
    pub average_latency_ms: f64,
    /// 健康评分 [0, 100]
    pub health_score: u8,
}

impl Default for HealthMetrics {
    fn default() -> Self {
        Self {
            events_per_second: 0.0,
            slow_consumer_count: 0,
            average_latency_ms: 0.0,
            health_score: 100,
        }
    }
}

impl HealthMetrics {
    /// 根据慢消费者数量计算健康评分
    ///
    /// M2 公式:起始 100,每个慢消费者扣 10 分,最低 0 分。
    pub fn compute_health_score(slow_consumer_count: u64) -> u8 {
        let score = 100i64 - 10 * slow_consumer_count as i64;
        score.clamp(0, 100) as u8
    }
}

/// TUI 数据源配置 — 控制缓存大小与行为
///
/// WHY 提前定义配置:后续 `DataPipeline` 需要容量上限,
/// 避免事件流无限增长导致内存膨胀。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataSourceConfig {
    /// 事件流保留的最大条数
    pub max_event_history: usize,
    /// Quest 列表保留的最大条数
    pub max_quest_list_size: usize,
    /// 预算指标无更新时的过期时间(毫秒),当前占位
    // TODO(M2): wire up budget metrics TTL/expiry when the panel needs staleness handling.
    pub budget_metrics_ttl_ms: u64,
    /// tick 间隔(毫秒),控制快照生成频率
    pub tick_interval_ms: u64,
    /// Sparkline 历史最大长度
    pub max_history_len: usize,
    /// 安全摘要列表最大长度
    pub max_security_summaries: usize,
    /// 冻结能力列表最大长度
    pub max_frozen_capabilities: usize,
}

impl Default for DataSourceConfig {
    fn default() -> Self {
        Self {
            // WHY 256:平衡调试可见性与内存占用;按每条 NexusEvent 约 500 字节估算,
            // 约 128KB,远低于 HCW 128K 窗口约束。
            max_event_history: 256,
            max_quest_list_size: 64,
            budget_metrics_ttl_ms: 5000,
            tick_interval_ms: 250,
            // WHY 64:Sparkline 在 80 列终端上约占用 60-70 列,64 个点刚好填满
            // 主面板宽度,同时保持较低内存占用。
            max_history_len: 64,
            max_security_summaries: 10,
            max_frozen_capabilities: 20,
        }
    }
}

/// TUI 数据源 trait — 抽象事件总线订阅、测试桩或缓存
///
/// 设计目标:
/// - 面板渲染只读 `DataSnapshot`,与事件订阅解耦。
/// - 返回 `TuiError` 统一错误处理(§4.1:库层用 thiserror)。
pub trait TuiDataSource {
    /// 获取当前数据快照
    ///
    /// 实现者应返回最近一次聚合结果;若尚未收到任何事件,
    /// 返回默认空快照而非错误,保证面板始终可渲染。
    fn snapshot(&self) -> Result<DataSnapshot, TuiError>;

    /// 返回数据源配置
    fn config(&self) -> &DataSourceConfig;
}

/// Quest 同步器 — 从 NexusEvent 维护本地 Quest 列表
///
/// WHY 独立结构体:将事件→状态的转换逻辑隔离,`DataPipeline`(P1.3)
/// 可组合多个同步器生成统一快照,同时方便单元测试直接喂事件。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct QuestSync {
    quests: Vec<Quest>,
}

impl QuestSync {
    /// 创建空的 Quest 同步器
    pub fn new() -> Self {
        Self::default()
    }

    /// 应用单个 NexusEvent,若事件影响 Quest 列表则返回更新后的列表副本
    ///
    /// - `QuestListUpdated`:替换整个列表(冷启动/lag 后对齐)。
    /// - `QuestCompleted`:按 quest_id 从活动列表移除。
    /// - 其他事件:返回 `None`,状态不变。
    pub fn apply_event(&mut self, event: &NexusEvent) -> Option<Vec<Quest>> {
        match event {
            NexusEvent::QuestListUpdated { quests, .. } => {
                self.quests = quests.clone();
                Some(self.quests.clone())
            }
            NexusEvent::QuestCompleted { quest_id, .. } => {
                self.quests.retain(|q| q.quest_id != *quest_id);
                Some(self.quests.clone())
            }
            _ => None,
        }
    }

    /// 获取当前活动 Quest 列表副本
    pub fn quests(&self) -> Vec<Quest> {
        self.quests.clone()
    }
}

/// Budget 同步器 — 从 NexusEvent 维护本地 BudgetMetrics
///
/// WHY 独立结构体:与 `QuestSync` 对称,将事件→指标的转换隔离,
/// 由 `BudgetMetricsUpdated` 直接填充面板视图,无需拼合多个事件。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct BudgetSync {
    metrics: BudgetMetrics,
}

impl BudgetSync {
    /// 创建空的 Budget 同步器
    pub fn new() -> Self {
        Self::default()
    }

    /// 应用单个 NexusEvent,若事件影响预算指标则返回更新后的指标副本
    ///
    /// - `BudgetMetricsUpdated`:直接替换本地指标。
    /// - 其他事件:返回 `None`,状态不变。
    pub fn apply_event(&mut self, event: &NexusEvent) -> Option<BudgetMetrics> {
        match event {
            NexusEvent::BudgetMetricsUpdated { metrics, .. } => {
                self.metrics = BudgetMetrics {
                    total_consumption: metrics.total_consumption,
                    remaining_budget: metrics.remaining_budget,
                    utilization_rate: metrics.utilization_rate,
                    current_tier: metrics.current_tier.clone(),
                    coefficient: metrics.coefficient,
                    is_exceeded: metrics.is_exceeded,
                    alert: metrics.alert.clone(),
                };
                Some(self.metrics.clone())
            }
            _ => None,
        }
    }

    /// 获取当前预算指标副本
    pub fn metrics(&self) -> BudgetMetrics {
        self.metrics.clone()
    }
}

/// 记忆同步器 — 从 NexusEvent 维护本地 MemoryMetrics
///
/// WHY 独立结构体:与 `BudgetSync` 对称,将 L2/L3 事件→面板指标的转换隔离。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct MemorySync {
    metrics: MemoryMetrics,
}

impl MemorySync {
    /// 创建空的 Memory 同步器
    pub fn new() -> Self {
        Self::default()
    }

    /// 应用单个 NexusEvent,若事件影响记忆指标则返回更新后的指标副本
    ///
    /// 处理的事件:
    /// - `MemoryMetricsReported`:命中率、驱逐数。
    /// - `ContextWindowSwitched`:当前层级(`to_tier`)。
    /// - `ContextCompressed`:上下文窗口大小与压缩率。
    /// - `CacheStatsReported`:命中率与驱逐数(备选来源)。
    /// - `CacheHit` / `CacheMiss`:累计命中/未命中计数。
    pub fn apply_event(&mut self, event: &NexusEvent) -> Option<MemoryMetrics> {
        let changed = match event {
            NexusEvent::MemoryMetricsReported {
                hit_rate,
                evictions,
                ..
            } => {
                self.metrics.hit_rate_percent = hit_rate * 100.0;
                self.metrics.evictions = *evictions;
                true
            }
            NexusEvent::ContextWindowSwitched { to_tier, .. } => {
                self.metrics.tier = to_tier.clone();
                true
            }
            NexusEvent::ContextCompressed {
                original_size,
                ratio,
                ..
            } => {
                self.metrics.context_window_size = *original_size;
                self.metrics.compressed_ratio = *ratio;
                true
            }
            NexusEvent::CacheStatsReported {
                hit_rate,
                eviction_count,
                ..
            } => {
                self.metrics.hit_rate_percent = hit_rate * 100.0;
                self.metrics.evictions = *eviction_count;
                true
            }
            NexusEvent::CacheHit { .. } => {
                self.metrics.cache_hits += 1;
                true
            }
            NexusEvent::CacheMiss { .. } => {
                self.metrics.cache_misses += 1;
                true
            }
            _ => false,
        };

        if changed {
            Some(self.metrics.clone())
        } else {
            None
        }
    }

    /// 获取当前记忆指标副本
    pub fn metrics(&self) -> MemoryMetrics {
        self.metrics.clone()
    }
}

/// 安全同步器 — 从 NexusEvent 维护本地 SecurityState
///
/// WHY 独立结构体:将 L4/L8 安全事件→面板状态的转换隔离,
/// 面板侧无需理解 NexusEvent 的完整结构。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct SecuritySync {
    state: SecurityState,
}

impl SecuritySync {
    /// 创建空的 Security 同步器
    pub fn new() -> Self {
        Self::default()
    }

    /// 应用单个 NexusEvent,若事件影响安全状态则返回更新后的状态副本
    ///
    /// 处理的事件:
    /// - `SkepticVeto`:追加到 `active_vetoes` 并合并冻结能力。
    /// - `RedTeamAudit`:追加到 `recent_audits`。
    /// - `AsaIntervention`:追加到 `recent_interventions`。
    /// - `CapabilityFrozen`:追加到 `frozen_capabilities`。
    /// - `SandboxViolation`:不直接修改状态,仍进入日志流供 Log 面板展示。
    pub fn apply_event(
        &mut self,
        event: &NexusEvent,
        max_summaries: usize,
        max_frozen: usize,
    ) -> Option<SecurityState> {
        let changed = match event {
            NexusEvent::SkepticVeto {
                quest_id,
                veto_reason,
                frozen_capabilities,
                metadata,
            } => {
                self.state.active_vetoes.push(SkepticVetoSummary {
                    quest_id: quest_id.clone(),
                    veto_reason: veto_reason.clone(),
                    frozen_capabilities: frozen_capabilities.clone(),
                    timestamp: metadata.timestamp,
                });
                for cap in frozen_capabilities {
                    if !self.state.frozen_capabilities.contains(cap) {
                        self.state.frozen_capabilities.push(cap.clone());
                    }
                }
                true
            }
            NexusEvent::RedTeamAudit {
                vulnerability_type,
                failed_probes,
                total_probes,
                detection_rate,
                remediation_suggestion,
                metadata,
            } => {
                self.state.recent_audits.push(RedTeamAuditSummary {
                    vulnerability_type: vulnerability_type.clone(),
                    failed_probes: *failed_probes,
                    total_probes: *total_probes,
                    detection_rate: *detection_rate,
                    remediation_suggestion: remediation_suggestion.clone(),
                    timestamp: metadata.timestamp,
                });
                true
            }
            NexusEvent::AsaIntervention {
                operation_id,
                action,
                safety_score,
                block_reason,
                metadata,
                ..
            } => {
                self.state
                    .recent_interventions
                    .push(AsaInterventionSummary {
                        operation_id: operation_id.clone(),
                        action: action.clone(),
                        safety_score: *safety_score,
                        block_reason: block_reason.clone(),
                        timestamp: metadata.timestamp,
                    });
                true
            }
            NexusEvent::CapabilityFrozen {
                capability_id,
                reason,
                ..
            } => {
                if !self.state.frozen_capabilities.contains(capability_id) {
                    self.state.frozen_capabilities.push(capability_id.clone());
                }
                // 记录冻结原因到状态,方便面板展示。
                let _ = reason;
                true
            }
            NexusEvent::SandboxViolation { .. } => false,
            _ => false,
        };

        // 限制列表长度,避免内存无限增长。
        while self.state.active_vetoes.len() > max_summaries {
            self.state.active_vetoes.remove(0);
        }
        while self.state.recent_audits.len() > max_summaries {
            self.state.recent_audits.remove(0);
        }
        while self.state.recent_interventions.len() > max_summaries {
            self.state.recent_interventions.remove(0);
        }
        while self.state.frozen_capabilities.len() > max_frozen {
            self.state.frozen_capabilities.remove(0);
        }

        if changed {
            Some(self.state.clone())
        } else {
            None
        }
    }

    /// 获取当前安全状态副本
    pub fn state(&self) -> SecurityState {
        self.state.clone()
    }
}

/// 健康同步器 — 从 NexusEvent 维护本地 HealthMetrics
///
/// WHY 独立结构体:将系统健康事件→面板指标的转换隔离。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct HealthSync {
    metrics: HealthMetrics,
    /// 最近 MCP Mesh 事务延迟样本,用于计算平均延迟
    latency_samples: Vec<u64>,
    /// 最大延迟样本数
    max_latency_samples: usize,
}

impl HealthSync {
    /// 创建空的 Health 同步器
    pub fn new(max_latency_samples: usize) -> Self {
        Self {
            max_latency_samples,
            ..Default::default()
        }
    }

    /// 应用单个 NexusEvent,若事件影响健康指标则返回更新后的指标副本
    ///
    /// 处理的事件:
    /// - `SlowConsumerDropped`:增加慢消费者计数。
    /// - `McpMeshTransactionCompleted`:记录延迟样本并更新平均延迟。
    pub fn apply_event(&mut self, event: &NexusEvent) -> Option<HealthMetrics> {
        let changed = match event {
            NexusEvent::SlowConsumerDropped { .. } => {
                self.metrics.slow_consumer_count += 1;
                true
            }
            NexusEvent::McpMeshTransactionCompleted { latency_ms, .. } => {
                self.latency_samples.push(*latency_ms);
                while self.latency_samples.len() > self.max_latency_samples {
                    self.latency_samples.remove(0);
                }
                self.metrics.average_latency_ms = if self.latency_samples.is_empty() {
                    0.0
                } else {
                    self.latency_samples.iter().sum::<u64>() as f64
                        / self.latency_samples.len() as f64
                };
                true
            }
            _ => false,
        };

        if changed {
            self.metrics.health_score =
                HealthMetrics::compute_health_score(self.metrics.slow_consumer_count);
            Some(self.metrics.clone())
        } else {
            None
        }
    }

    /// 获取当前健康指标副本
    pub fn metrics(&self) -> HealthMetrics {
        self.metrics.clone()
    }

    /// 根据本 tick 新增事件数计算每秒事件数
    ///
    /// `tick_interval_ms` 为 DataPipeline 的 tick 间隔。
    pub fn compute_events_per_second(&self, events_this_tick: usize, tick_interval_ms: u64) -> f64 {
        if tick_interval_ms == 0 {
            return 0.0;
        }
        events_this_tick as f64 / (tick_interval_ms as f64 / 1000.0)
    }
}

/// 内存桩数据源 — 返回包含示例 Quest 与 Budget 数据的快照
///
/// WHY: TUI 默认启动时不强制要求真实 event-bus 连接；提供一个无依赖的
/// 桩实现，使 `TuiApp::new` 保持向后兼容，同时让 demo/stub 模式也能展示
/// 有意义的数据，而不是空面板。
#[derive(Debug, Default, Clone)]
pub struct StubDataSource {
    config: DataSourceConfig,
}

impl StubDataSource {
    /// 创建新的示例桩数据源
    pub fn new() -> Self {
        Self::default()
    }
}

impl TuiDataSource for StubDataSource {
    fn snapshot(&self) -> Result<DataSnapshot, TuiError> {
        let mut snapshot = DataSnapshot::default();

        // 提供一条示例 Quest，让默认启动的 Quest 面板不空载。
        snapshot.quest_list.push(Quest {
            quest_id: "stub-q1".into(),
            title: "Demo Quest".into(),
            tasks: vec![
                Task {
                    task_id: "stub-t1".into(),
                    description: "completed demo task".into(),
                    status: TaskStatus::Completed,
                    dependencies: vec![],
                },
                Task {
                    task_id: "stub-t2".into(),
                    description: "pending demo task".into(),
                    status: TaskStatus::Pending,
                    dependencies: vec![],
                },
            ],
            thinking_mode: ThinkingMode::Standard,
            checkpoint_id: None,
        });

        // 提供非零预算指标，让 Budget 面板展示进度条与状态。
        snapshot.budget_metrics = BudgetMetrics {
            total_consumption: 350.0,
            remaining_budget: 650.0,
            utilization_rate: 0.35,
            current_tier: "High".into(),
            coefficient: 1.0,
            is_exceeded: false,
            alert: None,
        };

        // 提供示例记忆指标，让 Memory 面板展示 Gauge 与 Sparkline。
        snapshot.memory_metrics = MemoryMetrics {
            hit_rate_percent: 87.5,
            evictions: 12,
            context_window_size: 4096,
            compressed_ratio: 0.72,
            cache_hits: 120,
            cache_misses: 18,
            tier: "L1".into(),
        };

        // 提供示例安全状态，让 Security 面板展示列表。
        snapshot.security_state = SecurityState {
            active_vetoes: vec![SkepticVetoSummary {
                quest_id: "stub-q1".into(),
                veto_reason: "demo veto".into(),
                frozen_capabilities: vec!["demo-cap".into()],
                timestamp: Utc::now(),
            }],
            recent_audits: vec![RedTeamAuditSummary {
                vulnerability_type: "prompt_injection".into(),
                failed_probes: 1,
                total_probes: 10,
                detection_rate: 0.1,
                remediation_suggestion: "add input validation".into(),
                timestamp: Utc::now(),
            }],
            recent_interventions: vec![AsaInterventionSummary {
                operation_id: "stub-op".into(),
                action: "Warn".into(),
                safety_score: 0.6,
                block_reason: None,
                timestamp: Utc::now(),
            }],
            frozen_capabilities: vec!["demo-cap".into()],
        };

        // 提供示例健康指标，让 Health 面板展示 Gauge 与 Sparkline。
        snapshot.health_metrics = HealthMetrics {
            events_per_second: 42.0,
            slow_consumer_count: 1,
            average_latency_ms: 15.5,
            health_score: HealthMetrics::compute_health_score(1),
        };

        // 提供示例历史曲线，让 Sparkline 不空载。
        snapshot.budget_history = vec![30, 32, 35, 33, 36, 38, 35];
        snapshot.memory_history = vec![80, 82, 85, 83, 86, 88, 87];
        snapshot.event_rate_history = vec![30, 35, 40, 38, 42, 45, 42];

        // 提供一条示例事件，让 Log / Parliament 面板不空载。
        snapshot.latest_events.push_back(NexusEvent::CacheHit {
            metadata: EventMetadata::new("stub"),
            cache_key: "demo".into(),
        });

        Ok(snapshot)
    }

    fn config(&self) -> &DataSourceConfig {
        &self.config
    }
}

/// 数据管道 — 后台聚合事件并生成统一快照
///
/// WHY:将事件订阅、去重、状态同步与快照生成封装为独立生命周期，
/// 让 TUI 主循环只读 `DataSnapshot`，不直接处理 event-bus 细节。
#[derive(Debug)]
pub struct DataPipeline {
    config: DataSourceConfig,
    // WHY Mutex<Option<JoinHandle>>: 支持 `shutdown(&self)` 被外部 Arc 持有方调用,
    // 无需 TuiApp 归还所有权即可清理后台任务。
    task: Mutex<Option<JoinHandle<()>>>,
    // WHY 用 Arc<Mutex<Option<EventSubscriber>>>: 后台任务需要可变访问
    // `try_recv`,而 `DataPipeline::shutdown` 需要持有 subscriber 调用其
    // `shutdown`,因此共享所有权并由 `shutdown` 通过 `take()` 取出。
    subscriber: Arc<Mutex<Option<EventSubscriber>>>,
    snapshot: Arc<Mutex<DataSnapshot>>,
}

impl DataPipeline {
    /// 创建数据管道并启动后台聚合任务
    ///
    /// # 参数
    /// - `subscriber`: 已订阅 event-bus 的事件订阅者
    /// - `config`: 数据源配置，包含 tick 间隔与容量限制
    pub fn new(subscriber: EventSubscriber, config: DataSourceConfig) -> Self {
        let snapshot = Arc::new(Mutex::new(DataSnapshot::default()));
        let snapshot_clone = Arc::clone(&snapshot);
        let subscriber = Arc::new(Mutex::new(Some(subscriber)));
        let subscriber_clone = Arc::clone(&subscriber);
        let tick_ms = config.tick_interval_ms;
        let max_event_history = config.max_event_history;
        let max_quest_list_size = config.max_quest_list_size;
        let max_history_len = config.max_history_len;
        let max_security_summaries = config.max_security_summaries;
        let max_frozen_capabilities = config.max_frozen_capabilities;

        let task = tokio::spawn(async move {
            // WHY interval 而非 sleep:interval 会自动追钟，避免任务处理耗时导致 tick 漂移。
            let mut interval = time::interval(Duration::from_millis(tick_ms));
            let mut quest_sync = QuestSync::new();
            let mut budget_sync = BudgetSync::new();
            let mut memory_sync = MemorySync::new();
            let mut security_sync = SecuritySync::new();
            let mut health_sync = HealthSync::new(max_history_len);
            let mut latest_events: VecDeque<NexusEvent> = VecDeque::new();

            // Sparkline 历史缓存
            let mut budget_history: Vec<u64> = Vec::with_capacity(max_history_len);
            let mut memory_history: Vec<u64> = Vec::with_capacity(max_history_len);
            let mut event_rate_history: Vec<u64> = Vec::with_capacity(max_history_len);

            loop {
                interval.tick().await;

                // 取出订阅者引用;若已被 shutdown 取走,则退出循环。
                let mut guard = subscriber_clone.lock().unwrap_or_else(|poisoned| {
                    tracing::warn!(
                        "TUI data pipeline subscriber mutex was poisoned; recovering state"
                    );
                    poisoned.into_inner()
                });
                let Some(sub) = guard.as_mut() else {
                    break;
                };

                // 批量取出当前缓冲区中的所有事件，一次性消费避免多次加锁。
                let mut events = Vec::new();
                while let Some(event) = sub.try_recv() {
                    events.push(event);
                }
                // 锁只在取订阅者和 drain 缓冲区时持有,状态更新与快照写入不跨 await。
                drop(guard);

                // 先定位同一 tick 内最后一个 QuestListUpdated 与 BudgetMetricsUpdated
                // 的索引。仅这两个高频状态事件需要在状态更新层去重，日志流仍保留全部。
                let mut last_quest_idx = None::<usize>;
                let mut last_budget_idx = None::<usize>;
                for (idx, event) in events.iter().enumerate() {
                    match event {
                        NexusEvent::QuestListUpdated { .. } => last_quest_idx = Some(idx),
                        NexusEvent::BudgetMetricsUpdated { .. } => last_budget_idx = Some(idx),
                        _ => {}
                    }
                }

                for (idx, event) in events.iter().enumerate() {
                    let is_deduped_quest = matches!(event, NexusEvent::QuestListUpdated { .. })
                        && Some(idx) != last_quest_idx;
                    let is_deduped_budget =
                        matches!(event, NexusEvent::BudgetMetricsUpdated { .. })
                            && Some(idx) != last_budget_idx;

                    // 非去重状态事件才应用同步器；被去重的事件仍进入日志流。
                    if !is_deduped_quest && !is_deduped_budget {
                        quest_sync.apply_event(event);
                        budget_sync.apply_event(event);
                    }
                    memory_sync.apply_event(event);
                    security_sync.apply_event(
                        event,
                        max_security_summaries,
                        max_frozen_capabilities,
                    );
                    health_sync.apply_event(event);
                    latest_events.push_back(event.clone());
                }

                // 限制事件日志长度，防止内存无限增长。
                while latest_events.len() > max_event_history {
                    latest_events.pop_front();
                }

                // 计算本 tick 事件速率并更新历史曲线。
                let events_this_tick = events.len();
                let eps = health_sync.compute_events_per_second(events_this_tick, tick_ms);
                let budget = budget_sync.metrics();
                let memory = memory_sync.metrics();

                push_history(
                    &mut budget_history,
                    (budget.utilization_rate * 100.0) as u64,
                    max_history_len,
                );
                push_history(
                    &mut memory_history,
                    memory.hit_rate_percent as u64,
                    max_history_len,
                );
                push_history(&mut event_rate_history, eps as u64, max_history_len);

                let health = HealthMetrics {
                    events_per_second: eps,
                    ..health_sync.metrics()
                };

                let snap = DataSnapshot {
                    quest_list: truncate_quests(quest_sync.quests(), max_quest_list_size),
                    latest_events: latest_events.clone(),
                    budget_metrics: budget,
                    memory_metrics: memory,
                    security_state: security_sync.state(),
                    health_metrics: health,
                    budget_history: budget_history.clone(),
                    memory_history: memory_history.clone(),
                    event_rate_history: event_rate_history.clone(),
                };
                let mut guard = snapshot_clone.lock().unwrap_or_else(|poisoned| {
                    tracing::warn!(
                        "TUI data pipeline snapshot mutex was poisoned; recovering state"
                    );
                    poisoned.into_inner()
                });
                *guard = snap;
            }
        });

        Self {
            config,
            task: Mutex::new(Some(task)),
            subscriber,
            snapshot,
        }
    }

    /// 非阻塞读取当前快照
    pub fn snapshot(&self) -> DataSnapshot {
        let guard = self.snapshot.lock().unwrap_or_else(|poisoned| {
            tracing::warn!("TUI data pipeline snapshot mutex was poisoned; recovering state");
            poisoned.into_inner()
        });
        guard.clone()
    }

    /// 返回数据源配置
    pub fn config(&self) -> &DataSourceConfig {
        &self.config
    }

    /// 关闭数据管道，中止并等待后台任务结束
    ///
    /// 取 `&self` 使外部 Arc 持有方可在不回收所有权的情况下清理后台任务。
    /// 先关闭 `EventSubscriber` 的转发任务，再中止数据聚合任务，避免 orphan task。
    pub async fn shutdown(&self) {
        // 先优雅停止 EventSubscriber,避免其后台转发任务被 detach。
        let sub = self
            .subscriber
            .lock()
            .unwrap_or_else(|poisoned| {
                tracing::warn!("TUI data pipeline subscriber mutex was poisoned; recovering state");
                poisoned.into_inner()
            })
            .take();
        if let Some(mut sub) = sub {
            sub.shutdown().await;
        }

        // 取出 JoinHandle 所有权后再 abort + await，避免 `&self` 无法消费 handle。
        let Some(handle) = self
            .task
            .lock()
            .unwrap_or_else(|poisoned| {
                tracing::warn!("TUI data pipeline task mutex was poisoned; recovering state");
                poisoned.into_inner()
            })
            .take()
        else {
            return;
        };
        // abort 唤醒可能正在 interval.tick() 上等待的任务，再 await 确保资源释放，
        // 避免 orphan task(§4.4 反模式 #7)。
        handle.abort();
        let _ = handle.await;
    }
}

// WHY 实现 Drop:调用者若忘记 `shutdown()` 或提前 drop DataPipeline,
// 仍必须中止后台任务,避免 tokio::task::JoinHandle 被 drop 后任务继续运行
// 成为 orphan task(§4.4 反模式 #7)。
// Drop 仅作为兜底;正常路径仍应显式调用 `shutdown().await` 以优雅关闭 subscriber。
impl Drop for DataPipeline {
    fn drop(&mut self) {
        // 取出 JoinHandle 所有权后显式 abort;若直接 drop handle,tokio 会 detach
        // 任务,导致其在后台继续运行。
        if let Some(handle) = self
            .task
            .lock()
            .unwrap_or_else(|poisoned| {
                tracing::warn!(
                    "TUI data pipeline task mutex was poisoned during drop; recovering state"
                );
                poisoned.into_inner()
            })
            .take()
        {
            handle.abort();
        }

        // subscriber 字段会在 DataPipeline drop 后按声明顺序释放,
        // EventSubscriber 自己的 Drop 会 abort 其转发任务;此处无需额外处理。
    }
}

impl TuiDataSource for DataPipeline {
    fn snapshot(&self) -> Result<DataSnapshot, TuiError> {
        Ok(DataPipeline::snapshot(self))
    }

    fn config(&self) -> &DataSourceConfig {
        DataPipeline::config(self)
    }
}

// WHY Arc<DataPipeline>: CLI 需要保留 `pipeline` 变量以便在 TUI 退出后调用
// `pipeline.shutdown().await`，同时把数据源的共享引用交给 `TuiApp`。
impl TuiDataSource for Arc<DataPipeline> {
    fn snapshot(&self) -> Result<DataSnapshot, TuiError> {
        Ok(DataPipeline::snapshot(self))
    }

    fn config(&self) -> &DataSourceConfig {
        DataPipeline::config(self)
    }
}

/// 辅助函数：截断 quest 列表至配置上限
///
/// WHY 单独函数:DataSnapshot 只保留面板展示所需前 N 个 quest，
/// 同时让 QuestSync 保持完整语义，便于未来按优先级排序后截断。
fn truncate_quests(quests: Vec<Quest>, max: usize) -> Vec<Quest> {
    let mut quests = quests;
    quests.truncate(max);
    quests
}

/// 辅助函数：向历史曲线追加一个点，超过容量时从队首丢弃
fn push_history(history: &mut Vec<u64>, value: u64, max: usize) {
    if history.len() >= max {
        history.remove(0);
    }
    history.push(value);
}

#[cfg(test)]
mod tests {
    use super::*;
    use event_bus::{BudgetMetricsPayload, EventMetadata, QuestStatus};
    use nexus_core::{Task, TaskStatus};

    /// 构造测试用 Quest
    fn quest(id: &str, title: &str) -> Quest {
        Quest {
            quest_id: id.into(),
            title: title.into(),
            tasks: vec![Task {
                task_id: format!("{id}-t1"),
                description: "test task".into(),
                status: TaskStatus::Pending,
                dependencies: vec![],
            }],
            thinking_mode: ThinkingMode::Standard,
            checkpoint_id: None,
        }
    }

    /// 构造 QuestListUpdated 事件
    fn quest_list_event(quests: Vec<Quest>) -> NexusEvent {
        NexusEvent::QuestListUpdated {
            metadata: EventMetadata::new("quest-engine"),
            quests,
            source: "quest-engine".into(),
        }
    }

    /// 构造 QuestCompleted 事件
    fn quest_completed_event(quest_id: &str, status: QuestStatus) -> NexusEvent {
        NexusEvent::QuestCompleted {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: quest_id.into(),
            status,
        }
    }

    /// 构造 BudgetMetricsUpdated 事件
    fn budget_metrics_event(metrics: BudgetMetrics) -> NexusEvent {
        NexusEvent::BudgetMetricsUpdated {
            metadata: EventMetadata::new("efficiency-monitor"),
            metrics: BudgetMetricsPayload {
                total_consumption: metrics.total_consumption,
                remaining_budget: metrics.remaining_budget,
                utilization_rate: metrics.utilization_rate,
                current_tier: metrics.current_tier,
                coefficient: metrics.coefficient,
                is_exceeded: metrics.is_exceeded,
                alert: metrics.alert,
            },
        }
    }

    /// 构造 MemoryMetricsReported 事件
    fn memory_metrics_event(hit_rate: f32, evictions: u64) -> NexusEvent {
        NexusEvent::MemoryMetricsReported {
            metadata: EventMetadata::new("mlc-engine"),
            hit_rate,
            evictions,
        }
    }

    /// 构造 ContextWindowSwitched 事件
    fn context_window_switched_event(to_tier: &str) -> NexusEvent {
        NexusEvent::ContextWindowSwitched {
            metadata: EventMetadata::new("hcw-window"),
            from_tier: "L0".into(),
            to_tier: to_tier.into(),
            reason: "capacity exceeded".into(),
        }
    }

    /// 构造 ContextCompressed 事件
    fn context_compressed_event(original_size: u64, ratio: f32) -> NexusEvent {
        NexusEvent::ContextCompressed {
            metadata: EventMetadata::new("hcw-window"),
            original_size,
            compressed_size: (original_size as f32 * ratio) as u64,
            ratio,
        }
    }

    /// 构造 CacheStatsReported 事件
    fn cache_stats_event(hit_rate: f32, eviction_count: u64) -> NexusEvent {
        NexusEvent::CacheStatsReported {
            metadata: EventMetadata::new("scc-cache"),
            hit_rate,
            eviction_count,
        }
    }

    /// 构造 SkepticVeto 事件
    fn skeptic_veto_event(quest_id: &str, reason: &str, caps: Vec<&str>) -> NexusEvent {
        NexusEvent::SkepticVeto {
            metadata: EventMetadata::new("parliament"),
            quest_id: quest_id.into(),
            veto_reason: reason.into(),
            frozen_capabilities: caps.into_iter().map(String::from).collect(),
        }
    }

    /// 构造 RedTeamAudit 事件
    fn red_team_audit_event(detection_rate: f32) -> NexusEvent {
        NexusEvent::RedTeamAudit {
            metadata: EventMetadata::new("parliament"),
            vulnerability_type: "prompt_injection".into(),
            failed_probes: 2,
            total_probes: 10,
            detection_rate,
            remediation_suggestion: "sanitize input".into(),
        }
    }

    /// 构造 AsaIntervention 事件
    fn asa_intervention_event(action: &str, score: f32) -> NexusEvent {
        NexusEvent::AsaIntervention {
            metadata: EventMetadata::new("seccore"),
            operation_id: "op-1".into(),
            action: action.into(),
            safety_score: score,
            block_reason: None,
            alternative_suggestion: None,
        }
    }

    /// 构造 CapabilityFrozen 事件
    fn capability_frozen_event(capability_id: &str) -> NexusEvent {
        NexusEvent::CapabilityFrozen {
            metadata: EventMetadata::new("decay-engine"),
            capability_id: capability_id.into(),
            reason: "security policy".into(),
        }
    }

    /// 构造 SlowConsumerDropped 事件
    fn slow_consumer_event() -> NexusEvent {
        NexusEvent::SlowConsumerDropped {
            metadata: EventMetadata::new("event-bus"),
            subscriber_id: "sub-1".into(),
            lag: 100,
            dropped_count: 5,
        }
    }

    /// 构造 McpMeshTransactionCompleted 事件
    fn mcp_mesh_event(latency_ms: u64) -> NexusEvent {
        NexusEvent::McpMeshTransactionCompleted {
            metadata: EventMetadata::new("mcp-mesh"),
            transaction_id: "tx-1".into(),
            participant_count: 3,
            latency_ms,
            success: true,
        }
    }

    #[test]
    fn test_data_snapshot_default_empty() {
        let snap = DataSnapshot::default();
        assert!(snap.quest_list.is_empty());
        assert!(snap.latest_events.is_empty());
        assert_eq!(snap.budget_metrics.utilization_rate, 0.0);
        assert_eq!(snap.memory_metrics.hit_rate_percent, 0.0);
        assert!(snap.security_state.active_vetoes.is_empty());
        assert_eq!(snap.health_metrics.health_score, 100);
    }

    #[test]
    fn test_budget_metrics_default() {
        let bm = BudgetMetrics::default();
        assert!(!bm.is_exceeded);
        assert_eq!(bm.current_tier, "High");
        assert_eq!(bm.coefficient, 1.0);
    }

    #[test]
    fn test_memory_metrics_default() {
        let mm = MemoryMetrics::default();
        assert_eq!(mm.tier, "L0");
        assert_eq!(mm.compressed_ratio, 1.0);
    }

    #[test]
    fn test_health_metrics_default() {
        let hm = HealthMetrics::default();
        assert_eq!(hm.health_score, 100);
    }

    #[test]
    fn test_health_score_formula() {
        assert_eq!(HealthMetrics::compute_health_score(0), 100);
        assert_eq!(HealthMetrics::compute_health_score(1), 90);
        assert_eq!(HealthMetrics::compute_health_score(5), 50);
        assert_eq!(HealthMetrics::compute_health_score(10), 0);
        assert_eq!(HealthMetrics::compute_health_score(100), 0);
    }

    #[test]
    fn test_data_source_config_default() {
        let cfg = DataSourceConfig::default();
        assert_eq!(cfg.max_event_history, 256);
        assert_eq!(cfg.max_quest_list_size, 64);
        assert_eq!(cfg.budget_metrics_ttl_ms, 5000);
        assert_eq!(cfg.tick_interval_ms, 250);
        assert_eq!(cfg.max_history_len, 64);
    }

    #[test]
    fn test_quest_sync_list_updated_replaces_list() {
        let mut sync = QuestSync::new();
        let q1 = quest("q1", "first");
        let q2 = quest("q2", "second");

        let updated = sync.apply_event(&quest_list_event(vec![q1.clone(), q2.clone()]));
        assert_eq!(updated, Some(vec![q1.clone(), q2.clone()]));
        assert_eq!(sync.quests(), vec![q1, q2]);
    }

    #[test]
    fn test_quest_sync_completed_removes_quest() {
        let mut sync = QuestSync::new();
        let q1 = quest("q1", "first");
        let q2 = quest("q2", "second");
        sync.apply_event(&quest_list_event(vec![q1.clone(), q2.clone()]));

        let updated = sync.apply_event(&quest_completed_event("q1", QuestStatus::Completed));
        assert_eq!(updated, Some(vec![q2.clone()]));
        assert_eq!(sync.quests(), vec![q2]);
    }

    #[test]
    fn test_quest_sync_unrelated_event_unchanged() {
        let mut sync = QuestSync::new();
        let q1 = quest("q1", "first");
        sync.apply_event(&quest_list_event(vec![q1.clone()]));

        let unrelated = NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k1".into(),
        };
        let result = sync.apply_event(&unrelated);
        assert!(result.is_none());
        assert_eq!(sync.quests(), vec![q1]);
    }

    #[test]
    fn test_budget_sync_metrics_updated() {
        let mut sync = BudgetSync::new();
        let metrics = BudgetMetrics {
            total_consumption: 8000.0,
            remaining_budget: 2000.0,
            utilization_rate: 0.8,
            current_tier: "Medium".into(),
            coefficient: 0.8,
            is_exceeded: false,
            alert: Some("approaching limit".into()),
        };

        let updated = sync.apply_event(&budget_metrics_event(metrics.clone()));
        assert_eq!(updated, Some(metrics.clone()));
        assert_eq!(sync.metrics(), metrics);
    }

    #[test]
    fn test_budget_sync_unrelated_event_unchanged() {
        let mut sync = BudgetSync::new();
        let metrics = BudgetMetrics {
            total_consumption: 5000.0,
            remaining_budget: 5000.0,
            utilization_rate: 0.5,
            current_tier: "High".into(),
            coefficient: 1.0,
            is_exceeded: false,
            alert: None,
        };
        sync.apply_event(&budget_metrics_event(metrics.clone()));

        let unrelated = NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k1".into(),
        };
        let result = sync.apply_event(&unrelated);
        assert!(result.is_none());
        assert_eq!(sync.metrics(), metrics);
    }

    #[test]
    fn test_memory_sync_metrics_reported() {
        let mut sync = MemorySync::new();
        let updated = sync.apply_event(&memory_metrics_event(0.85, 7));
        assert!(updated.is_some());
        let metrics = sync.metrics();
        assert_eq!(metrics.hit_rate_percent, 85.0);
        assert_eq!(metrics.evictions, 7);
    }

    #[test]
    fn test_memory_sync_context_window_switched() {
        let mut sync = MemorySync::new();
        sync.apply_event(&context_window_switched_event("L2"));
        assert_eq!(sync.metrics().tier, "L2");
    }

    #[test]
    fn test_memory_sync_context_compressed() {
        let mut sync = MemorySync::new();
        sync.apply_event(&context_compressed_event(8192, 0.5));
        let metrics = sync.metrics();
        assert_eq!(metrics.context_window_size, 8192);
        assert_eq!(metrics.compressed_ratio, 0.5);
    }

    #[test]
    fn test_memory_sync_cache_hit_miss_counters() {
        let mut sync = MemorySync::new();
        for _ in 0..3 {
            sync.apply_event(&NexusEvent::CacheHit {
                metadata: EventMetadata::new("scc-cache"),
                cache_key: "k1".into(),
            });
        }
        for _ in 0..2 {
            sync.apply_event(&NexusEvent::CacheMiss {
                metadata: EventMetadata::new("scc-cache"),
                cache_key: "k2".into(),
            });
        }
        let metrics = sync.metrics();
        assert_eq!(metrics.cache_hits, 3);
        assert_eq!(metrics.cache_misses, 2);
    }

    #[test]
    fn test_memory_sync_cache_stats_reported() {
        let mut sync = MemorySync::new();
        sync.apply_event(&cache_stats_event(0.78, 4));
        let metrics = sync.metrics();
        assert_eq!(metrics.hit_rate_percent, 78.0);
        assert_eq!(metrics.evictions, 4);
    }

    #[test]
    fn test_security_sync_veto_and_frozen_caps() {
        let mut sync = SecuritySync::new();
        sync.apply_event(
            &skeptic_veto_event("q1", "unsafe", vec!["cap1", "cap2"]),
            10,
            20,
        );
        let state = sync.state();
        assert_eq!(state.active_vetoes.len(), 1);
        assert_eq!(state.active_vetoes[0].quest_id, "q1");
        assert_eq!(state.frozen_capabilities, vec!["cap1", "cap2"]);
    }

    #[test]
    fn test_security_sync_red_team_audit() {
        let mut sync = SecuritySync::new();
        sync.apply_event(&red_team_audit_event(0.25), 10, 20);
        let state = sync.state();
        assert_eq!(state.recent_audits.len(), 1);
        assert_eq!(state.recent_audits[0].detection_rate, 0.25);
    }

    #[test]
    fn test_security_sync_asa_intervention() {
        let mut sync = SecuritySync::new();
        sync.apply_event(&asa_intervention_event("Block", 0.2), 10, 20);
        let state = sync.state();
        assert_eq!(state.recent_interventions.len(), 1);
        assert_eq!(state.recent_interventions[0].action, "Block");
    }

    #[test]
    fn test_security_sync_capability_frozen() {
        let mut sync = SecuritySync::new();
        sync.apply_event(&capability_frozen_event("cap-x"), 10, 20);
        let state = sync.state();
        assert_eq!(state.frozen_capabilities, vec!["cap-x"]);
    }

    #[test]
    fn test_security_sync_bounds_lists() {
        let mut sync = SecuritySync::new();
        for i in 0..15 {
            sync.apply_event(
                &skeptic_veto_event(&format!("q{i}"), "reason", vec![]),
                5,
                20,
            );
        }
        assert_eq!(sync.state().active_vetoes.len(), 5);
    }

    #[test]
    fn test_health_sync_slow_consumer() {
        let mut sync = HealthSync::new(64);
        sync.apply_event(&slow_consumer_event());
        let metrics = sync.metrics();
        assert_eq!(metrics.slow_consumer_count, 1);
        assert_eq!(metrics.health_score, 90);
    }

    #[test]
    fn test_health_sync_mcp_mesh_latency() {
        let mut sync = HealthSync::new(64);
        sync.apply_event(&mcp_mesh_event(10));
        sync.apply_event(&mcp_mesh_event(20));
        let metrics = sync.metrics();
        assert_eq!(metrics.average_latency_ms, 15.0);
    }

    #[test]
    fn test_health_sync_events_per_second() {
        let sync = HealthSync::new(64);
        assert_eq!(sync.compute_events_per_second(10, 250), 40.0);
        assert_eq!(sync.compute_events_per_second(0, 250), 0.0);
        assert_eq!(sync.compute_events_per_second(10, 0), 0.0);
    }

    #[test]
    fn test_push_history_bounds() {
        let mut history = Vec::new();
        for i in 0..70 {
            push_history(&mut history, i, 64);
        }
        assert_eq!(history.len(), 64);
        assert_eq!(history[0], 6);
        assert_eq!(history[63], 69);
    }
}
