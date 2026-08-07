//! 数据快照与指标类型 — TUI 各面板渲染所需数据的统一视图
//!
//! 包含 [`DataSnapshot`]、面板指标结构体([`BudgetMetrics`]、[`MemoryMetrics`]、
//! [`HealthMetrics`]、[`SecurityState`] 等)、数据源配置 [`DataSourceConfig`]、
//! 导出格式 [`ExportFormat`] 以及数据源 trait [`TuiDataSource`]。
//!
//! 对应架构层:L10 Interface

use chrono::{DateTime, Utc};
use event_bus::{ChatStatus, NexusEvent};
use nexus_core::{Quest, TaskStatus};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::Arc;

use crate::error::TuiError;
use crate::types::{ChatMessage, TickMode};

/// 数据快照 — TUI 各面板渲染所需数据的统一视图
///
/// WHY 独立结构体:面板渲染只依赖此快照,不依赖具体数据源实现,
/// 方便单元测试用内存桩替换 event-bus 订阅。
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct DataSnapshot {
    /// 快照生成号(单调递增,数据管道每次 tick 聚合后 +1)
    ///
    /// WHY 性能 P-1:事件循环轮询频率(100ms)高于数据 tick(250ms),
    /// 两 tick 之间快照内容不变。`TuiApp::update` 据此跳过无变化帧的
    /// 整包字段拷贝(latest_events / 历史曲线 / 指标),EventStream/Log
    /// 的过滤缓存也以它为失效键。
    #[serde(default)]
    pub revision: u64,

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
    /// 暂停状态的 Quest 的数量。这复用已有事件变体,不新增事件,符合 L10 只读
    /// EventBus 的约束。
    #[serde(default)]
    pub paused_quest_count: usize,

    /// 最近接收到的 NexusEvent,按时间顺序,旧在前
    ///
    /// WHY VecDeque:面板需要"最新 N 条"语义,从队尾追加、队首丢弃
    /// 为 O(1),避免频繁 `Vec::remove(0)`。
    /// WHY `Arc`:数据管道无新事件 tick 时直接共享上一快照的 Arc,
    /// 快照构建零拷贝;有事件 tick 写时复制后替换(评估报告 P0-2)。
    /// 读取方(update/面板)仅做引用计数递增,不修改内容。
    pub latest_events: Arc<VecDeque<NexusEvent>>,

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
    // === PROBE P0.4:HCW 召回读数(由 HcwRecallReported 事件同步) ===
    /// 多针召回率 needle_recall@8 ∈ [0,1]
    #[serde(default)]
    pub recall_needle_at_8: Option<f32>,
    /// 位置偏置比 ∈ [0,1]
    #[serde(default)]
    pub recall_position_bias: Option<f32>,
    /// 链路成功率 ∈ [0,1]
    #[serde(default)]
    pub recall_chain_success: Option<f32>,
    // === P8 ResourceMonitor 面板新增字段 ===
    /// 系统资源指标(由 SysMetricsCollector 采集)
    pub sys_metrics: crate::types::SystemMetrics,
    /// 系统资源指标历史(sparkline 数据)
    pub sys_metrics_history: Vec<u64>,
    /// 当前 tick 模式,状态栏展示用
    #[serde(default)]
    pub tick_mode: TickMode,
    // === M3b Chat 面板字段 ===
    /// 对话历史(由 ChatSync 拥有,同步到 TuiState)
    #[serde(default)]
    pub chat_messages: Vec<ChatMessage>,
    /// 对话会话状态(思考中/工具执行中/空闲)
    #[serde(default)]
    pub chat_status: ChatStatus,
    // === P0 交互链:Action 反馈字段 ===
    /// 最近一次 Action 终态反馈(消息, 是否错误);None = 尚无反馈
    #[serde(default)]
    pub action_feedback: Option<(String, bool)>,
    /// Action 反馈序号(单调递增,app 据此判定新反馈,避免每 tick 重复上屏)
    #[serde(default)]
    pub action_feedback_seq: u64,
    // === P1-W2.2 Critical 旁路通道丢弃计数 ===
    /// Critical 旁路通道(mpsc 4096)累计丢弃事件数(单调递增,0 = 无丢弃)
    ///
    /// 来源:efficiency-monitor 周期性采样 `EventBus::critical_dropped_count()`
    /// 并发布 `EfficiencyAlertTriggered` 事件(metric_name =
    /// `CRITICAL_DROPPED_METRIC_NAME`),由 `CriticalDroppedSync` 解析
    /// `triggered_value` 字段同步。EventStream 面板顶部据此显示告警。
    ///
    /// WHY 镜像累计值而非增量:TUI 仅显示当前累计丢弃总数,无需维护额外累加状态;
    /// efficiency-monitor 的 `publish_critical_dropped_alert` 传出的就是累计值。
    #[serde(default)]
    pub critical_event_dropped_count: u64,
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
    /// Eco 模式 tick 间隔(毫秒),高负载时降低 CPU 占用
    pub eco_tick_interval_ms: u64,
    /// 事件积压阈值,超过此值自动切换到 Eco 模式
    pub event_backlog_threshold: usize,
    /// Chat 对话历史保留的最大条数(FIFO,超出丢弃最旧)
    pub max_chat_messages: usize,
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
            // WHY 1000ms:大幅降低 CPU 占用,适合高负载场景
            eco_tick_interval_ms: 1000,
            // WHY 100:256 条事件上限的 ~40%,超过此值视为积压
            event_backlog_threshold: 100,
            // WHY 500:对话历史上限,足够一次会话的多轮交互,超出 FIFO 淘汰
            max_chat_messages: 500,
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

// ============================================================
// 数据导出 — CSV/JSON 格式导出 Quest 列表
// ============================================================

/// 导出格式
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportFormat {
    /// CSV 格式
    Csv,
    /// JSON 格式
    Json,
}

impl ExportFormat {
    /// 返回文件扩展名
    pub fn extension(&self) -> &'static str {
        match self {
            ExportFormat::Csv => "csv",
            ExportFormat::Json => "json",
        }
    }
}

/// CSV 字段转义:包含逗号/引号/换行的字段用双引号包裹
pub(crate) fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// 从 Quest 任务状态派生可读标签
pub(crate) fn quest_status_label(quest: &Quest) -> &'static str {
    if quest.tasks.is_empty() {
        return "Pending";
    }
    let has_running = quest.tasks.iter().any(|t| t.status == TaskStatus::Running);
    if has_running {
        return "Running";
    }
    let has_failed = quest.tasks.iter().any(|t| t.status == TaskStatus::Failed);
    if has_failed {
        return "Failed";
    }
    let all_completed = quest
        .tasks
        .iter()
        .all(|t| t.status == TaskStatus::Completed);
    if all_completed {
        return "Completed";
    }
    "Pending"
}

impl DataSnapshot {
    /// 将 Quest 列表导出为 CSV 或 JSON 文件
    ///
    /// WHY 仅导出 Quest:当前面板中最具导出价值的数据是 Quest 列表,
    /// 其他面板(metrics/history)的导出可在后续扩展。
    pub fn export_quests_to(
        &self,
        format: ExportFormat,
        path: &std::path::Path,
    ) -> Result<(), TuiError> {
        // 创建父目录
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }

        match format {
            ExportFormat::Csv => self.export_quests_csv(path),
            ExportFormat::Json => self.export_quests_json(path),
        }
    }

    fn export_quests_csv(&self, path: &std::path::Path) -> Result<(), TuiError> {
        let mut wtr = String::new();
        // CSV header
        wtr.push_str("quest_id,title,priority,task_count,status\n");
        for quest in &self.quest_list {
            let status = quest_status_label(quest);
            wtr.push_str(&format!(
                "{},{},{},{},{}\n",
                csv_escape(&quest.quest_id),
                csv_escape(&quest.title),
                quest.priority,
                quest.tasks.len(),
                csv_escape(status)
            ));
        }
        std::fs::write(path, wtr).map_err(|e| TuiError::ConfigError {
            detail: format!("CSV write failed: {e}"),
        })
    }

    fn export_quests_json(&self, path: &std::path::Path) -> Result<(), TuiError> {
        let json =
            serde_json::to_string_pretty(&self.quest_list).map_err(|e| TuiError::ConfigError {
                detail: format!("JSON serialize failed: {e}"),
            })?;
        std::fs::write(path, json).map_err(|e| TuiError::ConfigError {
            detail: format!("JSON write failed: {e}"),
        })
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
    ///
    /// WHY 返回 `Arc<DataSnapshot>`:快照在管道内为共享不可变结构
    /// (每 tick 原子 swap,复用 overwindow_bridge 已验证的 Arc 模式),
    /// 读取方仅做引用计数递增,避免每帧整包深拷贝。
    fn snapshot(&self) -> Result<Arc<DataSnapshot>, TuiError>;

    /// 返回数据源配置
    fn config(&self) -> &DataSourceConfig;
}
