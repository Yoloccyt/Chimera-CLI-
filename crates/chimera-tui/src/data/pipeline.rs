//! 数据管道与数据源实现 — 后台事件聚合、系统资源采集与测试桩
//!
//! 包含 [`DataPipeline`](后台事件聚合管道)、[`SysMetricsCollector`](系统资源采集器)、
//! [`StubDataSource`](测试桩数据源)以及辅助函数。
//!
//! 对应架构层:L10 Interface

use chrono::Utc;
use event_bus::{EventMetadata, NexusEvent};
use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};
use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tokio::task::JoinHandle;
use tokio::time::{self, Duration};

use super::snapshot::{
    AsaInterventionSummary, BudgetMetrics, DataSnapshot, DataSourceConfig, HealthMetrics,
    MemoryMetrics, RedTeamAuditSummary, SecurityState, SkepticVetoSummary, TuiDataSource,
};
use super::sync::*;
use crate::error::TuiError;
use crate::subscriber::EventSubscriber;
use crate::types::{CpuMetrics, DiskMetrics, MemMetrics, NetworkMetrics, SystemMetrics, TickMode};

// ============================================================
// P8 系统资源指标采集器 — SysMetricsCollector
// ============================================================

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
        let mut system = sysinfo::System::new_all();
        system.refresh_all();
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
            priority: 128,
        });

        snapshot.budget_metrics = BudgetMetrics {
            total_consumption: 350.0,
            remaining_budget: 650.0,
            utilization_rate: 0.35,
            current_tier: "High".into(),
            coefficient: 1.0,
            is_exceeded: false,
            alert: None,
        };

        snapshot.memory_metrics = MemoryMetrics {
            hit_rate_percent: 87.5,
            evictions: 12,
            context_window_size: 4096,
            compressed_ratio: 0.72,
            cache_hits: 120,
            cache_misses: 18,
            tier: "L1".into(),
        };

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

        snapshot.health_metrics = HealthMetrics {
            events_per_second: 42.0,
            slow_consumer_count: 1,
            average_latency_ms: 15.5,
            health_score: HealthMetrics::compute_health_score(1),
        };

        snapshot.budget_history = vec![30, 32, 35, 33, 36, 38, 35];
        snapshot.memory_history = vec![80, 82, 85, 83, 86, 88, 87];
        snapshot.event_rate_history = vec![30, 35, 40, 38, 42, 45, 42];

        snapshot.decay_metrics = crate::types::DecayMetrics {
            coefficient: 0.85,
            recent_events: vec!["capability_frozen:cap-1".into()],
            cycle_start: Some(Utc::now()),
            // P2-11: stub 数据源默认 fallback_count_delta = 0(无 learner 异常)
            fallback_count_delta: 0,
        };
        snapshot.decay_history = vec![1000, 980, 950, 920, 880, 860, 850];

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
    task: Mutex<Option<JoinHandle<()>>>,
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
        let eco_tick_ms = config.eco_tick_interval_ms;
        let event_backlog_threshold = config.event_backlog_threshold;
        let max_event_history = config.max_event_history;
        let max_chat_messages = config.max_chat_messages;
        let max_quest_list_size = config.max_quest_list_size;
        let max_history_len = config.max_history_len;
        let max_security_summaries = config.max_security_summaries;
        let max_frozen_capabilities = config.max_frozen_capabilities;
        let snapshot_interval_s = config.snapshot_interval_s;
        let max_snapshots = config.max_snapshots;

        let task = tokio::spawn(async move {
            let mut current_tick_ms = tick_ms;
            let mut eco_countdown: u32 = 0;
            let mut tick_mode = TickMode::Normal;
            let mut quest_sync = QuestSync::new();
            let mut budget_sync = BudgetSync::new();
            let mut memory_sync = MemorySync::new();
            let mut security_sync = SecuritySync::new();
            let mut health_sync = HealthSync::new(max_history_len);
            let mut decay_sync = DecaySync::new();
            let mut router_sync = RouterSync::new();
            let mut mcp_nodes_sync = McpNodesSync::new();
            let mut chtc_sync = ChtcSync::new();
            let mut osa_sync = OsaSync::new();
            let mut clv_sync = ClvSync::new();
            let mut chat_sync = ChatSync::new(max_chat_messages);
            let mut action_feedback_sync = ActionFeedbackSync::new();
            let mut critical_dropped_sync = CriticalDroppedSync::new();
            let mut sys_collector: Option<SysMetricsCollector> = None;
            let mut latest_events: VecDeque<NexusEvent> = VecDeque::new();

            let mut budget_history: Vec<u64> = Vec::with_capacity(max_history_len);
            let mut memory_history: Vec<u64> = Vec::with_capacity(max_history_len);
            let mut event_rate_history: Vec<u64> = Vec::with_capacity(max_history_len);
            let mut decay_history: Vec<u64> = Vec::with_capacity(max_history_len);
            let mut sys_metrics_history: Vec<u64> = Vec::with_capacity(max_history_len);

            let mut timeline_snapshots: Vec<crate::types::TimelineSnapshot> =
                Vec::with_capacity(max_snapshots);
            let mut total_event_count: u64 = 0;
            let mut events_since_last_snapshot: u64 = 0;
            let mut last_timeline_snapshot: chrono::DateTime<chrono::Utc> = chrono::Utc::now();

            loop {
                time::sleep(Duration::from_millis(current_tick_ms)).await;

                let mut guard = subscriber_clone.lock().unwrap_or_else(|poisoned| {
                    tracing::warn!(
                        "TUI data pipeline subscriber mutex was poisoned; recovering state"
                    );
                    poisoned.into_inner()
                });
                let Some(sub) = guard.as_mut() else {
                    break;
                };

                let mut events = Vec::new();
                while let Some(event) = sub.try_recv() {
                    events.push(event);
                }
                drop(guard);

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
                    decay_sync.apply_event(event);
                    router_sync.apply_event(event);
                    mcp_nodes_sync.apply_event(event);
                    chtc_sync.apply_event(event);
                    osa_sync.apply_event(event);
                    clv_sync.apply_event(event);
                    chat_sync.apply_event(event);
                    action_feedback_sync.apply_event(event);
                    critical_dropped_sync.apply_event(event);
                    latest_events.push_back(event.clone());
                }

                while latest_events.len() > max_event_history {
                    latest_events.pop_front();
                }

                let events_this_tick = events.len();
                let eps = health_sync.compute_events_per_second(events_this_tick, tick_ms);
                let budget = budget_sync.metrics();
                let memory = memory_sync.metrics();
                let decay = decay_sync.metrics();

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
                push_history(
                    &mut decay_history,
                    (decay.coefficient * 1000.0) as u64,
                    max_history_len,
                );

                let sys_collector = sys_collector.get_or_insert_with(SysMetricsCollector::new);
                let sys_metrics = sys_collector.refresh_and_snapshot();
                push_history(
                    &mut sys_metrics_history,
                    (sys_metrics.cpu.global_usage * 10.0) as u64,
                    max_history_len,
                );

                let quest_list = truncate_quests(quest_sync.quests(), max_quest_list_size);
                let paused_quest_count = quest_sync.paused_quest_count();
                let health_sync_metrics = health_sync.metrics();
                let health = HealthMetrics {
                    events_per_second: eps,
                    health_score: HealthMetrics::compute_health_score_with_backlog(
                        health_sync_metrics.slow_consumer_count,
                        quest_list.len(),
                    ),
                    ..health_sync_metrics
                };

                let now = chrono::Utc::now();
                let elapsed_secs = now
                    .signed_duration_since(last_timeline_snapshot)
                    .num_seconds();
                if elapsed_secs >= snapshot_interval_s as i64 {
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
                    decay_metrics: decay,
                    router_metrics: router_sync.metrics(),
                    mcp_nodes: mcp_nodes_sync.nodes(),
                    chtc_state: chtc_sync.state(),
                    decay_history: decay_history.clone(),
                    timeline_snapshots: timeline_snapshots.clone(),
                    osa_sparsity: osa_sync.sparsity(),
                    osa_context_mask: osa_sync.context_mask(),
                    osa_sparsity_history: osa_sync.sparsity_history(),
                    clv_summary: clv_sync.summary(),
                    sys_metrics,
                    sys_metrics_history: sys_metrics_history.clone(),
                    tick_mode,
                    chat_messages: chat_sync.messages(),
                    chat_status: chat_sync.status(),
                    action_feedback: action_feedback_sync.latest(),
                    action_feedback_seq: action_feedback_sync.seq(),
                    critical_event_dropped_count: critical_dropped_sync.count(),
                };
                let mut guard = snapshot_clone.lock().unwrap_or_else(|poisoned| {
                    tracing::warn!(
                        "TUI data pipeline snapshot mutex was poisoned; recovering state"
                    );
                    poisoned.into_inner()
                });
                *guard = snap;

                let backlog = latest_events.len();
                match tick_mode {
                    TickMode::Normal => {
                        if backlog >= event_backlog_threshold {
                            tick_mode = TickMode::Eco;
                            current_tick_ms = eco_tick_ms;
                            eco_countdown = 5;
                            tracing::info!(
                                "DataPipeline tick mode switched to Eco (backlog={backlog} >= threshold={event_backlog_threshold})"
                            );
                        }
                    }
                    TickMode::Eco => {
                        if backlog >= event_backlog_threshold {
                            eco_countdown = 5;
                        } else {
                            eco_countdown = eco_countdown.saturating_sub(1);
                            if eco_countdown == 0 {
                                tick_mode = TickMode::Normal;
                                current_tick_ms = tick_ms;
                                tracing::info!(
                                    "DataPipeline tick mode switched back to Normal (backlog={backlog})"
                                );
                            }
                        }
                    }
                }
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
pub(super) fn push_history(history: &mut Vec<u64>, value: u64, max: usize) {
    if history.len() >= max {
        history.remove(0);
    }
    history.push(value);
}
