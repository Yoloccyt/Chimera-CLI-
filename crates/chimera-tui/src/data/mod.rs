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
//! # 子模块
//! - `resource_history`:ResourceMonitorPanel 趋势图所需的滑动窗口时间序列
//!   与中位数滤波组件(见 enterprise-tui-monitoring-task-viz §二)。
//!
//! # 消费的事件变体
//!
//! `DataPipeline` 直接消费 `event-bus` 中已有的以下 `NexusEvent` 变体:
//! - `QuestListUpdated` / `QuestCompleted` / `QuestCancelled` /
//!   `QuestPriorityAdjusted`:维护 Quest 列表(含移除与优先级更新)。
//! - `BudgetMetricsUpdated`:更新 Budget 面板指标。
//! - `MemoryMetricsReported` / `ContextWindowSwitched` / `ContextCompressed` /
//!   `CacheStatsReported` / `CacheHit` / `CacheMiss`:更新 Memory 面板指标。
//! - `SkepticVeto` / `RedTeamAudit` / `AsaIntervention` / `CapabilityFrozen`:
//!   更新 Security 面板状态。
//! - `SlowConsumerDropped` / `McpMeshTransactionCompleted`:更新 Health 面板指标。
//! - 其余事件进入 `latest_events` 日志流,供 Log 面板展示。

pub mod metrics_history;
pub mod resource_history;
use crate::error::TuiError;
use crate::subscriber::EventSubscriber;
use crate::types::{CpuMetrics, DiskMetrics, MemMetrics, NetworkMetrics, SystemMetrics};
use chrono::{DateTime, Utc};
use event_bus::{EventMetadata, NexusEvent};
use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};
use serde::{Deserialize, Serialize};
use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex};
use std::time::Instant;
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

    /// 暂停 Quest 数(从 `QuestPaused`/`QuestResumed` 事件派生)
    ///
    /// WHY 派生字段:`Quest` 本身无 paused 字段(nexus-core 领域类型稳定性约束,
    /// §3.3.1 变更需 ADR),因此 `QuestSync` 订阅已有的 `QuestPaused`/`QuestResumed`
    /// 事件维护 `paused_quest_ids` 集合,生成快照时计算 quest_list 中同时处于
    /// 暂停状态的 Quest 数量。这复用已有事件变体,不新增事件,符合 L10 只读
    /// EventBus 的约束。
    #[serde(default)]
    pub paused_quest_count: usize,

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

    // === P2 TUI v1.7-omega 新增字段(与 TuiState 对齐) ===
    /// 衰减指标(数据驱动 Decay 面板)
    pub decay_metrics: crate::types::DecayMetrics,
    /// 路由器指标(数据驱动 Router 面板)
    pub router_metrics: crate::types::RouterMetrics,
    /// MCP 节点状态列表(数据驱动 McpNodes 面板)
    pub mcp_nodes: Vec<crate::types::McpNodeStatus>,
    /// CHTC 适配器状态(数据驱动 Chtc 面板)
    pub chtc_state: crate::types::ChtcState,
    /// 衰减历史 sparkline 数据点
    pub decay_history: Vec<u64>,

    // === P7 TUI v1.8-omega 新增字段(OsaSparse/ClvVector/Timeline 面板数据接入) ===
    /// Timeline 面板的历史快照列表(按 snapshot_interval_s 周期生成,FIFO max_snapshots 容量)
    pub timeline_snapshots: Vec<crate::types::TimelineSnapshot>,
    /// OSA 平均稀疏度 [0.0, 1.0](None = 未收到事件)
    pub osa_sparsity: Option<f32>,
    /// OSA context 维度活跃文件 ID 列表
    pub osa_context_mask: Vec<String>,
    /// OSA 稀疏度历史(容量 256,FIFO,存 sparsity * 1000 为 u64)
    pub osa_sparsity_history: Vec<u64>,
    /// CLV 摘要(None = 未收到事件)
    pub clv_summary: Option<event_bus::ClvSummary>,
    // === P8 ResourceMonitor 面板新增字段 ===
    /// 系统资源指标(由 SysMetricsCollector 采集)
    pub sys_metrics: crate::types::SystemMetrics,
    /// 系统资源指标历史(sparkline 数据)
    pub sys_metrics_history: Vec<u64>,
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

    /// 根据慢消费者数量与活跃 Quest 数计算健康评分(含积压因子)
    ///
    /// 公式:起始 100,每个慢消费者扣 10 分;活跃 Quest > 10 时额外扣 10 分
    /// (积压因子),最低 0 分。
    ///
    /// WHY 新增方法而非修改 `compute_health_score`:原方法有 5 个单元测试与
    /// 1 个集成测试断言其语义,修改签名会破坏向后兼容(§3.3.1 SemVer 友好)。
    /// 新方法扩展积压因子,由 `DataPipeline` 在生成快照时调用,将活跃 Quest
    /// 积压对系统健康的影响纳入评分。
    ///
    /// # 参数
    /// - `slow_consumer_count`:慢消费者数量(每个扣 10 分)
    /// - `active_quest_count`:活跃 Quest 数(> 10 时扣 10 分积压因子)
    pub fn compute_health_score_with_backlog(
        slow_consumer_count: u64,
        active_quest_count: usize,
    ) -> u8 {
        let mut score = 100i64 - 10 * slow_consumer_count as i64;
        // 积压因子:活跃 Quest 超过 10 个时扣 10 分
        if active_quest_count > 10 {
            score -= 10;
        }
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
    /// Timeline 快照间隔(秒),控制 TimelineSnapshot 生成频率(P7 历史回放)
    ///
    /// WHY 从 TuiConfig 桥接:DataPipeline 需按此周期生成 TimelineSnapshot,
    /// 供 Timeline 面板回放历史系统状态。
    pub snapshot_interval_s: u16,
    /// Timeline 快照最大保留数(FIFO,超出则丢弃最旧)
    pub max_snapshots: usize,
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
            // WHY 与 TuiConfig 默认值对齐:30s × 100 = 50 分钟历史回放窗口
            snapshot_interval_s: 30,
            max_snapshots: 100,
        }
    }
}

impl DataSourceConfig {
    /// 从 `TuiConfig` 构建数据源配置(P4.3 可调 tick 暴露)
    ///
    /// WHY 单一桥接:`TuiConfig.tick_interval_ms` 是面向用户的 tick 配置,
    /// 而 `DataPipeline` 消费的是独立的 `DataSourceConfig`。此前 CLI 固定使用
    /// `DataSourceConfig::default()`,导致 TuiConfig 的 tick 形同虚设——修改
    /// `TuiConfig` 不会改变管道实际 tick。本桥接让 `TuiConfig` 成为 tick 的
    /// 唯一真实来源(single source of truth)。
    ///
    /// 当前仅映射 `tick_interval_ms`(本任务范围);其余字段沿用
    /// `DataSourceConfig` 默认值,后续若 `TuiConfig` 需控制更多数据源行为
    /// 可在此扩展映射,避免调用点散落字段拼接。
    pub fn from_tui_config(tui: &crate::config::TuiConfig) -> Self {
        Self {
            tick_interval_ms: u64::from(tui.tick_interval_ms),
            // P7:桥接 Timeline 回放配置,让 TuiConfig 成为唯一真实来源
            snapshot_interval_s: tui.snapshot_interval_s,
            max_snapshots: tui.max_snapshots,
            ..Self::default()
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

/// Quest 同步器 — 从 NexusEvent 维护本地 Quest 列表与暂停状态
///
/// WHY 独立结构体:将事件→状态的转换逻辑隔离,`DataPipeline`(P1.3)
/// 可组合多个同步器生成统一快照,同时方便单元测试直接喂事件。
///
/// # 暂停状态跟踪
/// `Quest` 本身无 paused 字段(nexus-core 领域类型稳定性约束),因此
/// `QuestSync` 订阅已有的 `QuestPaused`/`QuestResumed` 事件维护
/// `paused_quest_ids` 集合。只跟踪 quest_list 中存在的 Quest ID,
/// 避免计数不在活动列表中的暂停 Quest。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct QuestSync {
    quests: Vec<Quest>,
    /// 暂停 Quest ID 集合(从 QuestPaused/QuestResumed 事件派生)
    paused_quest_ids: HashSet<String>,
}

impl QuestSync {
    /// 创建空的 Quest 同步器
    pub fn new() -> Self {
        Self::default()
    }

    /// 应用单个 NexusEvent,若事件影响 Quest 列表则返回更新后的列表副本
    ///
    /// - `QuestListUpdated`:替换整个列表(冷启动/lag 后对齐)。暂停集合保留,
    ///   因为新列表中仍存在的暂停 Quest 应继续被计数。
    /// - `QuestCompleted`:按 quest_id 从活动列表移除,并从暂停集合清理。
    /// - `QuestCancelled`:按 quest_id 从活动列表移除,并从暂停集合清理。
    ///   与 `QuestCompleted` 对称,确保取消的 Quest 不残留暂停状态(内存泄漏防护)。
    /// - `QuestPriorityAdjusted`:按 quest_id 原地更新 priority 字段。
    ///   不影响其他状态(暂停集合、任务列表等),仅刷新优先级。
    /// - `QuestPaused`:若 quest_id 在活动列表中,加入暂停集合。
    /// - `QuestResumed`:从暂停集合移除。
    /// - 其他事件:返回 `None`,状态不变。
    pub fn apply_event(&mut self, event: &NexusEvent) -> Option<Vec<Quest>> {
        match event {
            NexusEvent::QuestListUpdated { quests, .. } => {
                self.quests = quests.clone();
                Some(self.quests.clone())
            }
            NexusEvent::QuestCompleted { quest_id, .. } => {
                self.quests.retain(|q| q.quest_id != *quest_id);
                // Quest 完成后从暂停集合清理,防止内存泄漏
                self.paused_quest_ids.remove(quest_id);
                Some(self.quests.clone())
            }
            NexusEvent::QuestCancelled { quest_id, .. } => {
                self.quests.retain(|q| q.quest_id != *quest_id);
                // Quest 取消后从暂停集合清理,与 QuestCompleted 对称,防止内存泄漏
                self.paused_quest_ids.remove(quest_id);
                Some(self.quests.clone())
            }
            NexusEvent::QuestPriorityAdjusted {
                quest_id,
                new_priority,
                ..
            } => {
                // 静默更新:找不到 quest_id 时不操作,避免为未知 ID 引入 panic 路径
                if let Some(quest) = self.quests.iter_mut().find(|q| q.quest_id == *quest_id) {
                    quest.priority = *new_priority;
                    Some(self.quests.clone())
                } else {
                    None
                }
            }
            NexusEvent::QuestPaused { quest_id, .. } => {
                // 只跟踪活动列表中存在的 Quest,避免计数无效暂停
                if self.quests.iter().any(|q| q.quest_id == *quest_id) {
                    self.paused_quest_ids.insert(quest_id.clone());
                }
                None
            }
            NexusEvent::QuestResumed { quest_id, .. } => {
                self.paused_quest_ids.remove(quest_id);
                None
            }
            _ => None,
        }
    }

    /// 获取当前活动 Quest 列表副本
    pub fn quests(&self) -> Vec<Quest> {
        self.quests.clone()
    }

    /// 获取当前暂停 Quest 数(quest_list 中同时处于暂停状态的 Quest 数量)
    ///
    /// WHY 交叉过滤:只统计 quest_list 中存在的暂停 Quest,确保暂停 Quest 数
    /// 不会因 quest_list 更新(如 QuestCompleted 移除)而虚高。
    pub fn paused_quest_count(&self) -> usize {
        self.quests
            .iter()
            .filter(|q| self.paused_quest_ids.contains(&q.quest_id))
            .count()
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

// ============================================================
// P2 TUI v1.7-omega 新增同步器 — 4 个监控面板的数据接入
// ============================================================
//
// WHY 独立结构体:与 QuestSync/BudgetSync 等保持对称,将事件→状态
// 转换逻辑隔离。每个同步器只处理一个 NexusEvent 变体,职责单一,
// 便于单元测试直接喂事件验证状态变化。

/// 衰减同步器 — 从 `DecayMetricsReported` 事件维护本地 DecayMetrics
///
/// 发布者:L4 decay-engine。消费:L10 TUI Decay 面板。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct DecaySync {
    metrics: crate::types::DecayMetrics,
}

impl DecaySync {
    /// 创建空的衰减同步器
    pub fn new() -> Self {
        Self::default()
    }

    /// 应用单个 NexusEvent,若事件影响衰减指标则返回更新后的指标副本
    ///
    /// - `DecayMetricsReported`:替换本地衰减指标,并返回新系数用于历史追加。
    /// - 其他事件:返回 `None`,状态不变。
    pub fn apply_event(&mut self, event: &NexusEvent) -> Option<crate::types::DecayMetrics> {
        match event {
            NexusEvent::DecayMetricsReported {
                coefficient,
                recent_events,
                cycle_start,
                ..
            } => {
                self.metrics.coefficient = *coefficient;
                self.metrics.recent_events = recent_events.clone();
                self.metrics.cycle_start = Some(*cycle_start);
                Some(self.metrics.clone())
            }
            _ => None,
        }
    }

    /// 获取当前衰减指标副本
    pub fn metrics(&self) -> crate::types::DecayMetrics {
        self.metrics.clone()
    }
}

/// 路由器统计同步器 — 从 `RouterStatsReported` 事件维护本地 RouterMetrics
///
/// 发布者:L9 efficiency-monitor(聚合 L6 三路由器)。消费:L10 TUI Router 面板。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct RouterSync {
    metrics: crate::types::RouterMetrics,
}

impl RouterSync {
    /// 创建空的路由器统计同步器
    pub fn new() -> Self {
        Self::default()
    }

    /// 应用单个 NexusEvent,若事件影响路由器指标则返回更新后的指标副本
    ///
    /// - `RouterStatsReported`:替换三路由器统计。
    /// - 其他事件:返回 `None`,状态不变。
    pub fn apply_event(&mut self, event: &NexusEvent) -> Option<crate::types::RouterMetrics> {
        match event {
            NexusEvent::RouterStatsReported {
                kvbsr_stats,
                sesa_stats,
                faae_stats,
                ..
            } => {
                self.metrics.kvbsr_stats = convert_router_payload(kvbsr_stats);
                self.metrics.sesa_stats = convert_router_payload(sesa_stats);
                self.metrics.faae_stats = convert_router_payload(faae_stats);
                Some(self.metrics.clone())
            }
            _ => None,
        }
    }

    /// 获取当前路由器指标副本
    pub fn metrics(&self) -> crate::types::RouterMetrics {
        self.metrics.clone()
    }
}

/// 将 event-bus 的 RouterStatsPayload 转换为 TUI 内部的 RouterStatsInfo
///
/// WHY 单独函数:DecaySync/RouterSync/McpNodesSync/ChtcSync 均需做类似
/// 载荷→本地类型的转换,提取为函数避免重复代码。同时隔离类型映射,
/// 未来若 TUI 内部类型字段变化,只需修改此函数。
fn convert_router_payload(
    payload: &event_bus::RouterStatsPayload,
) -> crate::types::RouterStatsInfo {
    crate::types::RouterStatsInfo {
        hit_rate: payload.hit_rate,
        p50_latency_us: payload.p50_latency_us,
        p95_latency_us: payload.p95_latency_us,
        p99_latency_us: payload.p99_latency_us,
        hot_capabilities: payload.hot_capabilities.clone(),
    }
}

/// MCP 节点同步器 — 从 `McpNodeHeartbeat` 事件维护本地节点列表
///
/// 发布者:L10 mcp-mesh。消费:L10 TUI McpNodes 面板。
/// 采用 upsert 语义:相同 node_id 更新,新 node_id 追加。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct McpNodesSync {
    nodes: Vec<crate::types::McpNodeStatus>,
}

impl McpNodesSync {
    /// 创建空的 MCP 节点同步器
    pub fn new() -> Self {
        Self::default()
    }

    /// 应用单个 NexusEvent,若事件为节点心跳则 upsert 节点状态
    ///
    /// - `McpNodeHeartbeat`:按 node_id upsert。状态字符串映射到 NodeStatus 枚举:
    ///   - "online" → Online
    ///   - "degraded" → Degraded
    ///   - 其他(含 "offline")→ Offline
    /// - 其他事件:返回 `None`,状态不变。
    pub fn apply_event(&mut self, event: &NexusEvent) -> Option<Vec<crate::types::McpNodeStatus>> {
        match event {
            NexusEvent::McpNodeHeartbeat {
                node_id,
                status,
                throughput,
                last_seen,
                ..
            } => {
                let node_status = match status.as_str() {
                    "online" => crate::types::NodeStatus::Online,
                    "degraded" => crate::types::NodeStatus::Degraded,
                    _ => crate::types::NodeStatus::Offline,
                };
                let new_status = crate::types::McpNodeStatus {
                    node_id: node_id.clone(),
                    status: node_status,
                    throughput: *throughput,
                    last_seen: Some(*last_seen),
                };
                // upsert:已有则替换,无则追加
                if let Some(existing) = self.nodes.iter_mut().find(|n| n.node_id == *node_id) {
                    *existing = new_status;
                } else {
                    self.nodes.push(new_status);
                }
                Some(self.nodes.clone())
            }
            _ => None,
        }
    }

    /// 获取当前节点列表副本
    pub fn nodes(&self) -> Vec<crate::types::McpNodeStatus> {
        self.nodes.clone()
    }
}

/// CHTC 适配器同步器 — 从 `ChtcAdapterStatus` 事件维护本地适配器列表
///
/// 发布者:L10 chtc-bridge。消费:L10 TUI Chtc 面板。
/// 采用 upsert 语义:相同 adapter_id 更新,新 adapter_id 追加。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ChtcSync {
    state: crate::types::ChtcState,
}

impl ChtcSync {
    /// 创建空的 CHTC 适配器同步器
    pub fn new() -> Self {
        Self::default()
    }

    /// 应用单个 NexusEvent,若事件为适配器状态则 upsert 适配器信息
    ///
    /// - `ChtcAdapterStatus`:按 adapter_id upsert。
    /// - 其他事件:返回 `None`,状态不变。
    pub fn apply_event(&mut self, event: &NexusEvent) -> Option<crate::types::ChtcState> {
        match event {
            NexusEvent::ChtcAdapterStatus {
                adapter_id,
                adapter_type,
                compatibility_score,
                recent_requests,
                is_online,
                ..
            } => {
                let new_info = crate::types::ChtcAdapterInfo {
                    adapter_id: adapter_id.clone(),
                    adapter_type: adapter_type.clone(),
                    compatibility_score: *compatibility_score,
                    recent_requests: recent_requests.clone(),
                    is_online: *is_online,
                };
                // upsert:已有则替换,无则追加
                if let Some(existing) = self
                    .state
                    .adapters
                    .iter_mut()
                    .find(|a| a.adapter_id == *adapter_id)
                {
                    *existing = new_info;
                } else {
                    self.state.adapters.push(new_info);
                }
                Some(self.state.clone())
            }
            _ => None,
        }
    }

    /// 获取当前 CHTC 状态副本
    pub fn state(&self) -> crate::types::ChtcState {
        self.state.clone()
    }
}

// ============================================================
// P7 TUI v1.8-omega 新增同步器 — OsaSparse / ClvVector 面板数据接入
// ============================================================
//
// WHY 独立同步器:与 DecaySync/RouterSync 等保持对称,将事件→状态
// 转换逻辑隔离。每个同步器只处理一个 NexusEvent 变体,职责单一,
// 便于单元测试直接喂事件验证状态变化。

/// OSA 稀疏度同步器 — 从 `OmniSparseMasksComputed` 事件维护本地 OSA 状态
///
/// 发布者:L6 osa-coordinator。消费:L10 TUI OsaSparse 面板。
///
/// WHY 独立同步器: OSA 事件的消费逻辑与预算/健康同步器解耦,
/// 便于独立测试和未来扩展(如五维独立稀疏度展示)。
#[derive(Debug, Clone, PartialEq)]
pub struct OsaSync {
    /// 平均稀疏度 [0.0, 1.0](None = 未收到事件)
    sparsity: Option<f32>,
    /// context 维度活跃文件 ID 列表
    context_mask: Vec<String>,
    /// 稀疏度历史(容量 256,FIFO,存 sparsity * 1000 为 u64)
    sparsity_history: Vec<u64>,
    /// 稀疏度历史容量(FIFO)
    max_history: usize,
}

impl Default for OsaSync {
    fn default() -> Self {
        Self {
            sparsity: None,
            context_mask: Vec::new(),
            sparsity_history: Vec::new(),
            // WHY 256:与 OSA 稀疏度 sparkline 展示需求匹配,
            // 256 个点在 80 列终端上足够平滑,同时内存占用可忽略。
            max_history: 256,
        }
    }
}

impl OsaSync {
    /// 创建 OSA 稀疏度同步器,默认历史容量 256
    pub fn new() -> Self {
        Self::default()
    }

    /// 应用单个 NexusEvent,若事件为 OSA 稀疏度计算则更新本地状态
    ///
    /// - `OmniSparseMasksComputed`:更新 sparsity / context_mask,并追加历史点。
    ///   历史存储为 `sparsity * 1000` 的 u64 值,避免 f32 序列化精度问题。
    /// - 其他事件:返回 `None`,状态不变。
    pub fn apply_event(&mut self, event: &NexusEvent) -> Option<()> {
        match event {
            NexusEvent::OmniSparseMasksComputed {
                sparsity,
                context_mask,
                ..
            } => {
                self.sparsity = Some(*sparsity);
                self.context_mask = context_mask.clone();
                // 追加历史点(sparsity * 1000 存为 u64,FIFO 容量控制)
                let history_value = (*sparsity * 1000.0) as u64;
                self.sparsity_history.push(history_value);
                while self.sparsity_history.len() > self.max_history {
                    self.sparsity_history.remove(0);
                }
                Some(())
            }
            _ => None,
        }
    }

    /// 获取当前平均稀疏度
    pub fn sparsity(&self) -> Option<f32> {
        self.sparsity
    }

    /// 获取当前 context 维度活跃文件 ID 列表副本
    pub fn context_mask(&self) -> Vec<String> {
        self.context_mask.clone()
    }

    /// 获取稀疏度历史副本
    pub fn sparsity_history(&self) -> Vec<u64> {
        self.sparsity_history.clone()
    }
}

/// CLV 摘要同步器 — 从 `ClvSnapshotReported` 事件维护本地 CLV 摘要
///
/// 发布者:L2 nmc-encoder。消费:L10 TUI ClvVector 面板。
///
/// WHY 独立同步器: CLV 摘要的更新逻辑简单(直接覆盖),
/// 但独立同步器保持与其他同步器的一致性,便于统一管理。
#[derive(Debug, Default, Clone, PartialEq)]
pub struct ClvSync {
    /// CLV 摘要(None = 未收到事件)
    summary: Option<event_bus::ClvSummary>,
}

impl ClvSync {
    /// 创建 CLV 摘要同步器
    pub fn new() -> Self {
        Self::default()
    }

    /// 应用单个 NexusEvent,若事件为 CLV 快照报告则更新本地摘要
    ///
    /// - `ClvSnapshotReported`:直接覆盖本地 CLV 摘要(最新覆盖旧值)。
    /// - 其他事件:返回 `None`,状态不变。
    pub fn apply_event(&mut self, event: &NexusEvent) -> Option<()> {
        match event {
            NexusEvent::ClvSnapshotReported { clv_summary, .. } => {
                self.summary = Some(clv_summary.clone());
                Some(())
            }
            _ => None,
        }
    }

    /// 获取当前 CLV 摘要副本
    pub fn summary(&self) -> Option<event_bus::ClvSummary> {
        self.summary.clone()
    }
}

// ============================================================
// P8 系统资源指标采集器 — SysMetricsCollector
// ============================================================
//
// 通过 sysinfo 采集 OS 级 CPU/内存/磁盘/网络指标。
// 发布者:DataPipeline 每个 tick 调用 refresh_and_snapshot()。
// 消费:L10 TUI ResourceMonitor / Health 面板。

/// 系统资源指标采集器 — 通过 sysinfo 采集 OS 级 CPU/内存/磁盘/网络指标
///
/// 发布者:DataPipeline 每个 tick 调用 refresh_and_snapshot()。
/// 消费:L10 TUI ResourceMonitor / Health 面板。
///
/// # 实现说明
/// - CPU 使用率基于两次 refresh 之间的差值计算(sysinfo 的 CPU usage 需要
///   至少两次采样才能计算差值)
/// - 首次调用返回全零(无历史基准),后续调用返回实际差值
/// - 网络速率基于两次采样的累计值差值与时间间隔计算
/// - 磁盘 I/O 速率在 sysinfo 0.32 中不可用(Disk 无 usage() 方法),设为 0
pub struct SysMetricsCollector {
    /// sysinfo 系统实例(持有以复用内部缓存)
    system: sysinfo::System,
    /// 上次采样时间(用于计算速率)
    last_sample_time: Instant,
    /// 上次累计接收字节(网络)
    last_rx_bytes: u64,
    /// 上次累计发送字节(网络)
    last_tx_bytes: u64,
}

impl SysMetricsCollector {
    /// 创建新的系统资源采集器
    pub fn new() -> Self {
        // WHY 先 refresh_all 再采样:sysinfo 需要初始基准值才能计算 CPU 差值
        let mut system = sysinfo::System::new_all();
        system.refresh_all();
        // 在初始刷新后获取网络累计值作为基准
        let networks = sysinfo::Networks::new_with_refreshed_list();
        let total_rx: u64 = networks.values().map(|n| n.total_received()).sum();
        let total_tx: u64 = networks.values().map(|n| n.total_transmitted()).sum();

        Self {
            system,
            last_sample_time: Instant::now(),
            last_rx_bytes: total_rx,
            last_tx_bytes: total_tx,
        }
    }

    /// 刷新系统指标并返回快照
    ///
    /// 每个 DataPipeline tick 调用一次。首次调用后即可获得非零 CPU 值
    /// (构造时已做初始刷新)。
    pub fn refresh_and_snapshot(&mut self) -> SystemMetrics {
        // 刷新 CPU 和内存
        self.system.refresh_cpu_all();
        self.system.refresh_memory();

        let now = Instant::now();
        let elapsed = now.duration_since(self.last_sample_time);
        let elapsed_secs = elapsed.as_secs_f64();
        self.last_sample_time = now;

        // --- CPU ---
        let cpu_count = self.system.cpus().len();
        let per_core: Vec<f32> = self
            .system
            .cpus()
            .iter()
            .map(|cpu| cpu.cpu_usage())
            .collect();
        let global_usage = if !per_core.is_empty() {
            per_core.iter().sum::<f32>() / per_core.len() as f32
        } else {
            0.0
        };
        let cpu = CpuMetrics {
            global_usage,
            per_core_usage: per_core,
            core_count: cpu_count,
        };

        // --- 内存 ---
        let total_mem = self.system.total_memory();
        let used_mem = self.system.used_memory();
        let available_mem = self.system.available_memory();
        let usage_percent = if total_mem > 0 {
            (used_mem as f32 / total_mem as f32) * 100.0
        } else {
            0.0
        };
        let swap_total = self.system.total_swap();
        let swap_used = self.system.used_swap();
        let memory = MemMetrics {
            total_bytes: total_mem,
            used_bytes: used_mem,
            available_bytes: available_mem,
            usage_percent,
            swap_total_bytes: swap_total,
            swap_used_bytes: swap_used,
        };

        // --- 磁盘 ---
        // NOTE: sysinfo 0.32 Disk 无 usage() 方法,磁盘 I/O 速率不可用。
        // 当升级到 sysinfo >= 0.34 后可通过 Disk::usage() 采集读写字节。
        let disk = DiskMetrics::default();

        // --- 网络 ---
        let networks = sysinfo::Networks::new_with_refreshed_list();
        let current_rx: u64 = networks.values().map(|n| n.total_received()).sum();
        let current_tx: u64 = networks.values().map(|n| n.total_transmitted()).sum();
        let (rx_rate, tx_rate) = if elapsed_secs > 0.0 {
            let rx = ((current_rx.saturating_sub(self.last_rx_bytes)) as f64 / elapsed_secs) as u64;
            let tx = ((current_tx.saturating_sub(self.last_tx_bytes)) as f64 / elapsed_secs) as u64;
            (rx, tx)
        } else {
            (0, 0)
        };
        self.last_rx_bytes = current_rx;
        self.last_tx_bytes = current_tx;
        let network = NetworkMetrics {
            rx_bytes_per_sec: rx_rate,
            tx_bytes_per_sec: tx_rate,
            total_rx_bytes: current_rx,
            total_tx_bytes: current_tx,
        };

        SystemMetrics {
            cpu,
            memory,
            disk,
            network,
        }
    }
}

impl Default for SysMetricsCollector {
    fn default() -> Self {
        Self::new()
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

    /// 使用指定数据源配置创建示例桩数据源(P4.3)
    ///
    /// WHY:让默认 `TuiApp::new` 路径同样尊重 `TuiConfig.tick_interval_ms`,
    /// 保证 "TuiConfig 驱动数据源 tick" 在桩模式下与生产管道行为一致,
    /// 而非仅在 CLI 实时管道生效。
    pub fn with_config(config: DataSourceConfig) -> Self {
        Self { config }
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
            // 与 Quest::default() 保持一致:默认优先级 128
            priority: 128,
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

        // P2 新增:提供示例衰减指标与历史,让 Decay 面板不空载。
        snapshot.decay_metrics = crate::types::DecayMetrics {
            coefficient: 0.85,
            recent_events: vec!["capability_frozen:cap-1".into()],
            cycle_start: Some(Utc::now()),
        };
        snapshot.decay_history = vec![1000, 980, 950, 920, 880, 860, 850];

        // P2 新增:提供示例路由器指标,让 Router 面板不空载。
        snapshot.router_metrics = crate::types::RouterMetrics {
            kvbsr_stats: crate::types::RouterStatsInfo {
                hit_rate: 0.87,
                p50_latency_us: 120,
                p95_latency_us: 480,
                p99_latency_us: 950,
                hot_capabilities: vec![("search".into(), 42), ("read_file".into(), 28)],
            },
            sesa_stats: crate::types::RouterStatsInfo {
                hit_rate: 0.72,
                p50_latency_us: 200,
                p95_latency_us: 800,
                p99_latency_us: 1500,
                hot_capabilities: vec![("activate".into(), 15)],
            },
            faae_stats: crate::types::RouterStatsInfo {
                hit_rate: 0.91,
                p50_latency_us: 60,
                p95_latency_us: 280,
                p99_latency_us: 650,
                hot_capabilities: vec![("tool_call".into(), 88)],
            },
        };

        // P2 新增:提供示例 MCP 节点状态,让 McpNodes 面板不空载。
        snapshot.mcp_nodes = vec![
            crate::types::McpNodeStatus {
                node_id: "mcp-node-1".into(),
                status: crate::types::NodeStatus::Online,
                throughput: 120,
                last_seen: Some(Utc::now()),
            },
            crate::types::McpNodeStatus {
                node_id: "mcp-node-2".into(),
                status: crate::types::NodeStatus::Degraded,
                throughput: 45,
                last_seen: Some(Utc::now()),
            },
        ];

        // P2 新增:提供示例 CHTC 适配器状态,让 Chtc 面板不空载。
        snapshot.chtc_state = crate::types::ChtcState {
            adapters: vec![
                crate::types::ChtcAdapterInfo {
                    adapter_id: "vscode-ext".into(),
                    adapter_type: "vscode".into(),
                    compatibility_score: 95,
                    recent_requests: vec![("tool_call".into(), 42)],
                    is_online: true,
                },
                crate::types::ChtcAdapterInfo {
                    adapter_id: "jetbrains-plugin".into(),
                    adapter_type: "jetbrains".into(),
                    compatibility_score: 88,
                    recent_requests: vec![("tool_call".into(), 18)],
                    is_online: true,
                },
            ],
        };

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
        // P7:Timeline 快照配置
        let snapshot_interval_s = config.snapshot_interval_s;
        let max_snapshots = config.max_snapshots;

        let task = tokio::spawn(async move {
            // WHY interval 而非 sleep:interval 会自动追钟，避免任务处理耗时导致 tick 漂移。
            let mut interval = time::interval(Duration::from_millis(tick_ms));
            let mut quest_sync = QuestSync::new();
            let mut budget_sync = BudgetSync::new();
            let mut memory_sync = MemorySync::new();
            let mut security_sync = SecuritySync::new();
            let mut health_sync = HealthSync::new(max_history_len);
            // P2 新增同步器
            let mut decay_sync = DecaySync::new();
            let mut router_sync = RouterSync::new();
            let mut mcp_nodes_sync = McpNodesSync::new();
            let mut chtc_sync = ChtcSync::new();
            // P7 新增同步器:OsaSparse / ClvVector 面板数据接入
            let mut osa_sync = OsaSync::new();
            let mut clv_sync = ClvSync::new();
            // P8:系统资源采集器,延迟到第一次 tick 时初始化
            // WHY 延迟初始化: SysMetricsCollector::new() 调用
            // sysinfo::System::new_all() + refresh_all() 是同步阻塞操作
            // (Windows 上可能耗时 100ms-500ms)。在 spawn 任务启动时同步执行会阻塞
            // current_thread runtime(如 #[tokio::test] 默认),导致 EventSubscriber
            // 无法及时将事件转发到 buffer。延迟到第一次 tick 之后,让 interval.tick()
            // 的 await 点先让出控制权给 EventSubscriber 运行,减少启动阻塞。
            let mut sys_collector: Option<SysMetricsCollector> = None;
            let mut latest_events: VecDeque<NexusEvent> = VecDeque::new();

            // Sparkline 历史缓存
            let mut budget_history: Vec<u64> = Vec::with_capacity(max_history_len);
            let mut memory_history: Vec<u64> = Vec::with_capacity(max_history_len);
            let mut event_rate_history: Vec<u64> = Vec::with_capacity(max_history_len);
            let mut decay_history: Vec<u64> = Vec::with_capacity(max_history_len);
            // P8:系统资源指标历史(sparkline 数据,CPU 使用率 × 10 的 u64 表示)
            let mut sys_metrics_history: Vec<u64> = Vec::with_capacity(max_history_len);

            // P7:Timeline 快照状态(周期生成,非事件驱动)
            // - timeline_snapshots:历史快照列表(FIFO max_snapshots 容量)
            // - total_event_count:累计事件总数(用于 TimelineSnapshot.event_count)
            // - events_since_last_snapshot:自上次快照以来的事件数(用于计算 event_rate)
            // - last_timeline_snapshot:上次快照时间(用于判断是否到达 snapshot_interval_s)
            let mut timeline_snapshots: Vec<crate::types::TimelineSnapshot> =
                Vec::with_capacity(max_snapshots);
            let mut total_event_count: u64 = 0;
            let mut events_since_last_snapshot: u64 = 0;
            let mut last_timeline_snapshot: chrono::DateTime<chrono::Utc> = chrono::Utc::now();

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
                    // P2 新增同步器:全部事件均尝试应用,不匹配的变体返回 None
                    decay_sync.apply_event(event);
                    router_sync.apply_event(event);
                    mcp_nodes_sync.apply_event(event);
                    chtc_sync.apply_event(event);
                    // P7 新增同步器:OSA 稀疏度 / CLV 摘要
                    osa_sync.apply_event(event);
                    clv_sync.apply_event(event);
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
                let decay = decay_sync.metrics();

                // P7:累计事件总数,供 TimelineSnapshot.event_count 使用
                total_event_count += events_this_tick as u64;
                events_since_last_snapshot += events_this_tick as u64;

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
                // 衰减系数 × 1000 作为 sparkline 数据点,保留 3 位小数精度
                push_history(
                    &mut decay_history,
                    (decay.coefficient * 1000.0) as u64,
                    max_history_len,
                );

                // P8:延迟初始化系统资源采集器(首次 tick 时)
                let sys_collector = sys_collector.get_or_insert_with(SysMetricsCollector::new);
                // P8:采集系统资源指标并维护 sparkline 历史
                let sys_metrics = sys_collector.refresh_and_snapshot();
                push_history(
                    &mut sys_metrics_history,
                    (sys_metrics.cpu.global_usage * 10.0) as u64,
                    max_history_len,
                );

                // 提前 truncate quest_list,供健康评分积压因子与快照共用
                let quest_list = truncate_quests(quest_sync.quests(), max_quest_list_size);
                let paused_quest_count = quest_sync.paused_quest_count();
                let health_sync_metrics = health_sync.metrics();
                // 健康评分纳入积压因子:活跃 Quest > 10 时扣 10 分
                let health = HealthMetrics {
                    events_per_second: eps,
                    health_score: HealthMetrics::compute_health_score_with_backlog(
                        health_sync_metrics.slow_consumer_count,
                        quest_list.len(),
                    ),
                    ..health_sync_metrics
                };

                // P7:按 snapshot_interval_s 周期生成 TimelineSnapshot(非事件驱动)
                //
                // WHY 在构造 DataSnapshot 前生成:Timeline 快照需要引用本 tick 的
                // budget/health/decay 指标,生成后追加到 timeline_snapshots,
                // 再随 DataSnapshot 一起写入共享状态。
                let now = chrono::Utc::now();
                let elapsed_secs = now
                    .signed_duration_since(last_timeline_snapshot)
                    .num_seconds();
                if elapsed_secs >= snapshot_interval_s as i64 {
                    // 计算自上次快照以来的事件速率(每秒事件数)
                    let rate = if elapsed_secs > 0 {
                        events_since_last_snapshot / elapsed_secs as u64
                    } else {
                        0
                    };
                    let timeline_entry = crate::types::TimelineSnapshot {
                        timestamp: now,
                        event_count: total_event_count,
                        event_rate: rate,
                        budget_utilization: budget.utilization_rate,
                        health_score: health.health_score,
                        decay_coefficient: decay.coefficient,
                    };
                    timeline_snapshots.push(timeline_entry);
                    // FIFO 容量控制:超出 max_snapshots 则丢弃最旧
                    while timeline_snapshots.len() > max_snapshots {
                        timeline_snapshots.remove(0);
                    }
                    last_timeline_snapshot = now;
                    events_since_last_snapshot = 0;
                }

                let snap = DataSnapshot {
                    quest_list,
                    paused_quest_count,
                    latest_events: latest_events.clone(),
                    budget_metrics: budget,
                    memory_metrics: memory,
                    security_state: security_sync.state(),
                    health_metrics: health,
                    budget_history: budget_history.clone(),
                    memory_history: memory_history.clone(),
                    event_rate_history: event_rate_history.clone(),
                    // P2 新增字段
                    decay_metrics: decay,
                    router_metrics: router_sync.metrics(),
                    mcp_nodes: mcp_nodes_sync.nodes(),
                    chtc_state: chtc_sync.state(),
                    decay_history: decay_history.clone(),
                    // P7 新增字段:OsaSparse / ClvVector / Timeline 面板数据
                    timeline_snapshots: timeline_snapshots.clone(),
                    osa_sparsity: osa_sync.sparsity(),
                    osa_context_mask: osa_sync.context_mask(),
                    osa_sparsity_history: osa_sync.sparsity_history(),
                    clv_summary: clv_sync.summary(),
                    // P8:系统资源指标(由 SysMetricsCollector 实时采集)
                    sys_metrics,
                    sys_metrics_history: sys_metrics_history.clone(),
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
            priority: 128,
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
    fn test_health_score_with_backlog_formula() {
        // 积压因子:活跃 Quest > 10 时额外扣 10 分,最低 0
        // 无慢消费者 + 无积压
        assert_eq!(HealthMetrics::compute_health_score_with_backlog(0, 0), 100);
        // 无慢消费者 + 恰好 10 个 Quest(阈值边界,不扣分)
        assert_eq!(HealthMetrics::compute_health_score_with_backlog(0, 10), 100);
        // 无慢消费者 + 11 个 Quest(超阈值,扣 10 分)
        assert_eq!(HealthMetrics::compute_health_score_with_backlog(0, 11), 90);
        // 无慢消费者 + 15 个 Quest
        assert_eq!(HealthMetrics::compute_health_score_with_backlog(0, 15), 90);
        // 1 个慢消费者 + 15 个 Quest(100 - 10 - 10 = 80)
        assert_eq!(HealthMetrics::compute_health_score_with_backlog(1, 15), 80);
        // 10 个慢消费者 + 15 个 Quest(clamp 到 0)
        assert_eq!(HealthMetrics::compute_health_score_with_backlog(10, 15), 0);
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

    // ============================================================
    // QuestSync 暂停状态跟踪测试 — 从 QuestPaused/QuestResumed 事件派生
    // ============================================================
    //
    // WHY 独立测试组:Quest 本身无 paused 字段(nexus-core 领域类型稳定
    // 性约束),QuestSync 通过订阅已有的 QuestPaused/QuestResumed 事件
    // 维护 paused_quest_ids 集合,生成快照时计算 paused_quest_count。
    // 这复用已有事件变体,不新增事件,符合"从 quest_list 派生"约束。

    /// 构造 QuestPaused 事件
    fn quest_paused_event(quest_id: &str) -> NexusEvent {
        NexusEvent::QuestPaused {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: quest_id.into(),
            requested_by: "tui".into(),
        }
    }

    /// 构造 QuestResumed 事件
    fn quest_resumed_event(quest_id: &str) -> NexusEvent {
        NexusEvent::QuestResumed {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: quest_id.into(),
            requested_by: "tui".into(),
        }
    }

    #[test]
    fn test_quest_sync_paused_tracking() {
        let mut sync = QuestSync::new();
        sync.apply_event(&quest_list_event(vec![
            quest("q1", "first"),
            quest("q2", "second"),
        ]));

        // 初始无暂停
        assert_eq!(sync.paused_quest_count(), 0);

        // 暂停 q1
        sync.apply_event(&quest_paused_event("q1"));
        assert_eq!(sync.paused_quest_count(), 1);

        // 暂停 q2
        sync.apply_event(&quest_paused_event("q2"));
        assert_eq!(sync.paused_quest_count(), 2);
    }

    #[test]
    fn test_quest_sync_resumed_clears_paused() {
        let mut sync = QuestSync::new();
        sync.apply_event(&quest_list_event(vec![
            quest("q1", "first"),
            quest("q2", "second"),
        ]));
        sync.apply_event(&quest_paused_event("q1"));
        sync.apply_event(&quest_paused_event("q2"));
        assert_eq!(sync.paused_quest_count(), 2);

        // 恢复 q1
        sync.apply_event(&quest_resumed_event("q1"));
        assert_eq!(sync.paused_quest_count(), 1);
    }

    #[test]
    fn test_quest_sync_paused_id_not_in_quest_list_ignored() {
        // 暂停一个不在 quest_list 中的 Quest ID,不应计入 paused_quest_count
        let mut sync = QuestSync::new();
        sync.apply_event(&quest_list_event(vec![quest("q1", "first")]));
        sync.apply_event(&quest_paused_event("q-unknown"));
        assert_eq!(
            sync.paused_quest_count(),
            0,
            "paused Quest not in quest_list should not be counted"
        );
    }

    #[test]
    fn test_quest_sync_quest_list_updated_preserves_paused_ids() {
        // QuestListUpdated 替换整个列表时,paused_quest_ids 应保留
        // (新列表中仍存在的暂停 Quest 继续被计数)
        let mut sync = QuestSync::new();
        sync.apply_event(&quest_list_event(vec![
            quest("q1", "first"),
            quest("q2", "second"),
        ]));
        sync.apply_event(&quest_paused_event("q1"));

        // 列表更新(移除 q2,保留 q1)
        sync.apply_event(&quest_list_event(vec![quest("q1", "first")]));
        assert_eq!(
            sync.paused_quest_count(),
            1,
            "paused Quest q1 should still be counted after list update"
        );
    }

    #[test]
    fn test_quest_sync_quest_completed_removes_from_paused() {
        // QuestCompleted 移除 Quest 时,若该 Quest 在 paused 集合中,应一并清理
        let mut sync = QuestSync::new();
        sync.apply_event(&quest_list_event(vec![
            quest("q1", "first"),
            quest("q2", "second"),
        ]));
        sync.apply_event(&quest_paused_event("q1"));
        assert_eq!(sync.paused_quest_count(), 1);

        // q1 完成,从列表移除,paused 集合也应清理 q1
        sync.apply_event(&quest_completed_event("q1", QuestStatus::Completed));
        assert_eq!(
            sync.paused_quest_count(),
            0,
            "completed Quest should be removed from paused set"
        );
    }

    // ============================================================
    // QuestSync 取消与优先级调整测试 — Task M4 扩展
    // ============================================================
    //
    // WHY 独立测试组:QuestCancelled 与 QuestCompleted 行为对称(移除+清理暂停),
    // QuestPriorityAdjusted 仅更新 priority 字段。内联单元测试直接验证 QuestSync
    // 状态,不依赖 DataPipeline 的 tokio tick 时序,覆盖更精准。

    /// 构造 QuestCancelled 事件
    fn quest_cancelled_event(quest_id: &str) -> NexusEvent {
        NexusEvent::QuestCancelled {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: quest_id.into(),
            requested_by: "test".into(),
        }
    }

    /// 构造 QuestPriorityAdjusted 事件
    fn quest_priority_adjusted_event(quest_id: &str, new_priority: u8) -> NexusEvent {
        NexusEvent::QuestPriorityAdjusted {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: quest_id.into(),
            new_priority,
            requested_by: "test".into(),
        }
    }

    #[test]
    fn test_quest_sync_cancelled_removes_quest() {
        let mut sync = QuestSync::new();
        let q1 = quest("q1", "first");
        let q2 = quest("q2", "second");
        sync.apply_event(&quest_list_event(vec![q1.clone(), q2.clone()]));

        let updated = sync.apply_event(&quest_cancelled_event("q1"));
        assert_eq!(updated, Some(vec![q2.clone()]));
        assert_eq!(sync.quests(), vec![q2]);
    }

    #[test]
    fn test_quest_sync_cancelled_removes_from_paused() {
        // QuestCancelled 移除 Quest 时,若该 Quest 在 paused 集合中,应一并清理
        // 与 QuestCompleted 对称,防止内存泄漏
        let mut sync = QuestSync::new();
        sync.apply_event(&quest_list_event(vec![
            quest("q1", "first"),
            quest("q2", "second"),
        ]));
        sync.apply_event(&quest_paused_event("q1"));
        assert_eq!(sync.paused_quest_count(), 1);

        // q1 取消,从列表移除,paused 集合也应清理 q1
        sync.apply_event(&quest_cancelled_event("q1"));
        assert_eq!(
            sync.paused_quest_count(),
            0,
            "cancelled Quest should be removed from paused set"
        );
    }

    #[test]
    fn test_quest_sync_cancelled_unknown_id_no_change() {
        // 取消不存在的 quest_id 不应 panic,也不应改变列表
        let mut sync = QuestSync::new();
        let q1 = quest("q1", "first");
        sync.apply_event(&quest_list_event(vec![q1.clone()]));

        let updated = sync.apply_event(&quest_cancelled_event("nonexistent"));
        // retain 对不存在的 ID 是 no-op,返回 Some(列表副本) 表示状态已"处理"
        assert!(updated.is_some());
        assert_eq!(sync.quests(), vec![q1]);
    }

    #[test]
    fn test_quest_sync_priority_adjusted_updates_field() {
        let mut sync = QuestSync::new();
        let q1 = quest("q1", "first");
        sync.apply_event(&quest_list_event(vec![q1]));

        let updated = sync.apply_event(&quest_priority_adjusted_event("q1", 200));
        assert!(updated.is_some());
        let quests = sync.quests();
        assert_eq!(quests.len(), 1);
        assert_eq!(quests[0].priority, 200);
    }

    #[test]
    fn test_quest_sync_priority_adjusted_unknown_id_ignored() {
        // 调整不存在的 quest_id 应静默返回 None,不 panic,不改变列表
        let mut sync = QuestSync::new();
        let q1 = quest("q1", "first");
        sync.apply_event(&quest_list_event(vec![q1.clone()]));

        let updated = sync.apply_event(&quest_priority_adjusted_event("nonexistent", 200));
        assert!(updated.is_none(), "unknown quest_id should return None");
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

    // ============================================================
    // P2 新增同步器测试 — DecaySync / RouterSync / McpNodesSync / ChtcSync
    // ============================================================

    /// 构造 DecayMetricsReported 事件
    fn decay_event(coefficient: f32, recent: Vec<&str>) -> NexusEvent {
        NexusEvent::DecayMetricsReported {
            metadata: EventMetadata::new("decay-engine"),
            coefficient,
            recent_events: recent.into_iter().map(String::from).collect(),
            cycle_start: Utc::now(),
        }
    }

    /// 构造 RouterStatsReported 事件
    fn router_stats_event(kvbsr_hit: f32, sesa_hit: f32, faae_hit: f32) -> NexusEvent {
        NexusEvent::RouterStatsReported {
            metadata: EventMetadata::new("efficiency-monitor"),
            kvbsr_stats: event_bus::RouterStatsPayload {
                hit_rate: kvbsr_hit,
                p50_latency_us: 100,
                p95_latency_us: 500,
                p99_latency_us: 1000,
                hot_capabilities: vec![("cap-1".into(), 42)],
            },
            sesa_stats: event_bus::RouterStatsPayload {
                hit_rate: sesa_hit,
                p50_latency_us: 200,
                p95_latency_us: 800,
                p99_latency_us: 1500,
                hot_capabilities: vec![],
            },
            faae_stats: event_bus::RouterStatsPayload {
                hit_rate: faae_hit,
                p50_latency_us: 50,
                p95_latency_us: 300,
                p99_latency_us: 700,
                hot_capabilities: vec![],
            },
        }
    }

    /// 构造 McpNodeHeartbeat 事件
    fn mcp_heartbeat_event(node_id: &str, status: &str, throughput: u64) -> NexusEvent {
        NexusEvent::McpNodeHeartbeat {
            metadata: EventMetadata::new("mcp-mesh"),
            node_id: node_id.into(),
            status: status.into(),
            throughput,
            last_seen: Utc::now(),
        }
    }

    /// 构造 ChtcAdapterStatus 事件
    fn chtc_adapter_event(
        adapter_id: &str,
        adapter_type: &str,
        score: u8,
        online: bool,
    ) -> NexusEvent {
        NexusEvent::ChtcAdapterStatus {
            metadata: EventMetadata::new("chtc-bridge"),
            adapter_id: adapter_id.into(),
            adapter_type: adapter_type.into(),
            compatibility_score: score,
            recent_requests: vec![("req-1".into(), 5)],
            is_online: online,
        }
    }

    #[test]
    fn test_decay_sync_metrics_reported() {
        let mut sync = DecaySync::new();
        // 默认状态:系数 1.0,无事件
        assert_eq!(sync.metrics().coefficient, 1.0);
        assert!(sync.metrics().cycle_start.is_none());

        let updated = sync.apply_event(&decay_event(0.7, vec!["ev1", "ev2"]));
        assert!(updated.is_some());
        let metrics = sync.metrics();
        assert_eq!(metrics.coefficient, 0.7);
        assert_eq!(
            metrics.recent_events,
            vec!["ev1".to_string(), "ev2".to_string()]
        );
        assert!(metrics.cycle_start.is_some());
    }

    #[test]
    fn test_decay_sync_unrelated_event_unchanged() {
        let mut sync = DecaySync::new();
        sync.apply_event(&decay_event(0.5, vec!["ev1"]));

        let unrelated = NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k1".into(),
        };
        let result = sync.apply_event(&unrelated);
        assert!(result.is_none());
        assert_eq!(sync.metrics().coefficient, 0.5);
    }

    #[test]
    fn test_router_sync_stats_reported() {
        let mut sync = RouterSync::new();
        let updated = sync.apply_event(&router_stats_event(0.85, 0.72, 0.91));
        assert!(updated.is_some());
        let metrics = sync.metrics();
        assert_eq!(metrics.kvbsr_stats.hit_rate, 0.85);
        assert_eq!(metrics.sesa_stats.hit_rate, 0.72);
        assert_eq!(metrics.faae_stats.hit_rate, 0.91);
        assert_eq!(metrics.kvbsr_stats.p99_latency_us, 1000);
        assert_eq!(metrics.kvbsr_stats.hot_capabilities.len(), 1);
    }

    #[test]
    fn test_router_sync_unrelated_event_unchanged() {
        let mut sync = RouterSync::new();
        sync.apply_event(&router_stats_event(0.85, 0.72, 0.91));

        let unrelated = NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k1".into(),
        };
        let result = sync.apply_event(&unrelated);
        assert!(result.is_none());
        assert_eq!(sync.metrics().kvbsr_stats.hit_rate, 0.85);
    }

    #[test]
    fn test_mcp_nodes_sync_upsert() {
        let mut sync = McpNodesSync::new();
        assert!(sync.nodes().is_empty());

        // 首次心跳:新增节点
        sync.apply_event(&mcp_heartbeat_event("node-1", "online", 100));
        assert_eq!(sync.nodes().len(), 1);
        assert_eq!(sync.nodes()[0].node_id, "node-1");
        assert_eq!(sync.nodes()[0].status, crate::types::NodeStatus::Online);
        assert_eq!(sync.nodes()[0].throughput, 100);

        // 同 node_id 再次心跳:upsert 更新状态
        sync.apply_event(&mcp_heartbeat_event("node-1", "degraded", 50));
        assert_eq!(sync.nodes().len(), 1, "upsert should not duplicate");
        assert_eq!(sync.nodes()[0].status, crate::types::NodeStatus::Degraded);
        assert_eq!(sync.nodes()[0].throughput, 50);

        // 新 node_id:追加
        sync.apply_event(&mcp_heartbeat_event("node-2", "offline", 0));
        assert_eq!(sync.nodes().len(), 2);
        assert_eq!(sync.nodes()[1].node_id, "node-2");
        assert_eq!(sync.nodes()[1].status, crate::types::NodeStatus::Offline);
    }

    #[test]
    fn test_mcp_nodes_sync_status_string_mapping() {
        let mut sync = McpNodesSync::new();
        sync.apply_event(&mcp_heartbeat_event("n1", "online", 10));
        sync.apply_event(&mcp_heartbeat_event("n2", "degraded", 5));
        sync.apply_event(&mcp_heartbeat_event("n3", "offline", 0));
        sync.apply_event(&mcp_heartbeat_event("n4", "unknown_status", 0));

        let nodes = sync.nodes();
        assert_eq!(nodes[0].status, crate::types::NodeStatus::Online);
        assert_eq!(nodes[1].status, crate::types::NodeStatus::Degraded);
        assert_eq!(nodes[2].status, crate::types::NodeStatus::Offline);
        // 未知状态字符串映射到 Offline
        assert_eq!(nodes[3].status, crate::types::NodeStatus::Offline);
    }

    #[test]
    fn test_chtc_sync_upsert() {
        let mut sync = ChtcSync::new();
        assert!(sync.state().adapters.is_empty());

        // 首次状态:新增适配器
        sync.apply_event(&chtc_adapter_event("vscode", "vscode", 95, true));
        assert_eq!(sync.state().adapters.len(), 1);
        assert_eq!(sync.state().adapters[0].adapter_id, "vscode");
        assert_eq!(sync.state().adapters[0].compatibility_score, 95);
        assert!(sync.state().adapters[0].is_online);

        // 同 adapter_id 再次状态:upsert 更新
        sync.apply_event(&chtc_adapter_event("vscode", "vscode", 80, false));
        assert_eq!(
            sync.state().adapters.len(),
            1,
            "upsert should not duplicate"
        );
        assert_eq!(sync.state().adapters[0].compatibility_score, 80);
        assert!(!sync.state().adapters[0].is_online);

        // 新 adapter_id:追加
        sync.apply_event(&chtc_adapter_event("jetbrains", "jetbrains", 90, true));
        assert_eq!(sync.state().adapters.len(), 2);
        assert_eq!(sync.state().adapters[1].adapter_id, "jetbrains");
    }

    #[test]
    fn test_chtc_sync_unrelated_event_unchanged() {
        let mut sync = ChtcSync::new();
        sync.apply_event(&chtc_adapter_event("vscode", "vscode", 95, true));

        let unrelated = NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k1".into(),
        };
        let result = sync.apply_event(&unrelated);
        assert!(result.is_none());
        assert_eq!(sync.state().adapters.len(), 1);
    }

    #[test]
    fn test_data_snapshot_p2_fields_default() {
        // P2 新增字段在 Default 实现中应正确初始化
        let snap = DataSnapshot::default();
        assert_eq!(snap.decay_metrics.coefficient, 1.0);
        assert!(snap.decay_metrics.cycle_start.is_none());
        assert_eq!(snap.router_metrics.kvbsr_stats.hit_rate, 0.0);
        assert!(snap.mcp_nodes.is_empty());
        assert!(snap.chtc_state.adapters.is_empty());
        assert!(snap.decay_history.is_empty());
    }

    // ============================================================
    // P7 新增同步器测试 — OsaSync / ClvSync / DataSnapshot 新字段
    // ============================================================

    /// 构造 OmniSparseMasksComputed 事件
    fn osa_event(sparsity: f32, context_mask: Vec<&str>) -> NexusEvent {
        NexusEvent::OmniSparseMasksComputed {
            metadata: EventMetadata::new("osa-coordinator"),
            mask_hash: format!("mask-{sparsity}"),
            sparsity,
            context_mask: context_mask.into_iter().map(String::from).collect(),
        }
    }

    /// 构造 ClvSnapshotReported 事件
    fn clv_event(l2_norm: f32, block_count: usize) -> NexusEvent {
        NexusEvent::ClvSnapshotReported {
            metadata: EventMetadata::new("nmc-encoder"),
            modality: "Text".into(),
            content_hash: format!("hash-{l2_norm}"),
            clv_summary: event_bus::ClvSummary {
                block_means: vec![0.1; block_count],
                l2_norm,
                top_dims: vec![(0, 0.8)],
            },
        }
    }

    #[test]
    fn test_osa_sync_omni_sparse_masks_computed() {
        let mut sync = OsaSync::new();
        // 默认状态:无稀疏度数据
        assert!(sync.sparsity().is_none());
        assert!(sync.context_mask().is_empty());
        assert!(sync.sparsity_history().is_empty());

        let updated = sync.apply_event(&osa_event(0.45, vec!["file1.rs", "file2.rs"]));
        assert!(updated.is_some());
        assert_eq!(sync.sparsity(), Some(0.45));
        assert_eq!(sync.context_mask().len(), 2);
        assert_eq!(sync.sparsity_history().len(), 1);
        // sparsity * 1000 = 450
        assert_eq!(sync.sparsity_history()[0], 450);
    }

    #[test]
    fn test_osa_sync_history_fifo() {
        let mut sync = OsaSync::new();
        // 填充超过 256 个,验证 FIFO 容量控制
        for i in 0..300 {
            sync.apply_event(&osa_event(i as f32 / 1000.0, vec![]));
        }
        // FIFO 容量控制:应只保留最后 256 个
        assert_eq!(sync.sparsity_history().len(), 256);
        // 最后一个值应为 299 * 1000 / 1000 = 299 → 存为 299
        assert_eq!(sync.sparsity_history()[255], 299);
    }

    #[test]
    fn test_osa_sync_unrelated_event_unchanged() {
        let mut sync = OsaSync::new();
        sync.apply_event(&osa_event(0.45, vec!["file1.rs"]));

        let unrelated = NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k1".into(),
        };
        let result = sync.apply_event(&unrelated);
        assert!(result.is_none());
        assert_eq!(sync.sparsity(), Some(0.45));
    }

    #[test]
    fn test_clv_sync_snapshot_reported() {
        let mut sync = ClvSync::new();
        assert!(sync.summary().is_none());

        let updated = sync.apply_event(&clv_event(2.5, 8));
        assert!(updated.is_some());
        let s = sync.summary().unwrap();
        assert_eq!(s.block_means.len(), 8);
        assert!((s.l2_norm - 2.5).abs() < 1e-5);
    }

    #[test]
    fn test_clv_sync_overwrites_previous() {
        let mut sync = ClvSync::new();
        // 第一次更新
        sync.apply_event(&clv_event(1.0, 8));
        // 第二次更新(覆盖)
        sync.apply_event(&clv_event(2.0, 8));
        let s = sync.summary().unwrap();
        // 应为第二次的值
        assert!((s.l2_norm - 2.0).abs() < 1e-5);
    }

    #[test]
    fn test_clv_sync_unrelated_event_unchanged() {
        let mut sync = ClvSync::new();
        sync.apply_event(&clv_event(2.5, 8));

        let unrelated = NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k1".into(),
        };
        let result = sync.apply_event(&unrelated);
        assert!(result.is_none());
        assert!(sync.summary().is_some());
    }

    #[test]
    fn test_data_snapshot_p7_fields_default() {
        // P7 新增字段在 Default 实现中应正确初始化
        let snap = DataSnapshot::default();
        assert!(snap.timeline_snapshots.is_empty());
        assert!(snap.osa_sparsity.is_none());
        assert!(snap.osa_context_mask.is_empty());
        assert!(snap.osa_sparsity_history.is_empty());
        assert!(snap.clv_summary.is_none());
    }

    #[test]
    fn test_data_source_config_p7_fields_default() {
        // P7 新增配置字段默认值与 TuiConfig 对齐
        let cfg = DataSourceConfig::default();
        assert_eq!(cfg.snapshot_interval_s, 30);
        assert_eq!(cfg.max_snapshots, 100);
    }
}
