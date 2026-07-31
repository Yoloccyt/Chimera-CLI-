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
//! - `snapshot`:数据快照、指标类型、数据源配置与 trait 定义
//! - `sync`:事件同步器(从 NexusEvent 流维护各面板本地状态)
//! - `pipeline`:后台数据管道、系统资源采集与测试桩
//! - `metrics_history`:ResourceMonitorPanel 趋势图所需的滑动窗口时间序列
//!   与中位数滤波组件(见 enterprise-tui-monitoring-task-viz §二)。
//! - `resource_history`:资源历史数据管理
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

pub(crate) mod pipeline;
pub(crate) mod snapshot;
pub(crate) mod sync;

// Re-export all public types to maintain the existing API surface
pub use pipeline::{DataPipeline, StubDataSource, SysMetricsCollector};
pub use snapshot::{
    AsaInterventionSummary, BudgetMetrics, DataSnapshot, DataSourceConfig, ExportFormat,
    HealthMetrics, MemoryMetrics, RedTeamAuditSummary, SecurityState, SkepticVetoSummary,
    TuiDataSource,
};
pub use sync::{
    ActionFeedbackSync, BudgetSync, ChatSync, ChtcSync, CriticalDroppedSync, DecaySync, HealthSync,
    McpNodesSync, MemorySync, OsaSync, QuestSync, RouterSync, SecuritySync,
};

#[cfg(test)]
mod tests {
    use super::snapshot::{csv_escape, quest_status_label};
    use super::sync::{ClvSync, CRITICAL_DROPPED_METRIC_NAME};
    use super::*;
    use crate::types::{ChatRole, TickMode};
    use chrono::Utc;
    use event_bus::{BudgetMetricsPayload, ChatStatus, EventMetadata, NexusEvent, QuestStatus};
    use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};

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
            capability_id: None,
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
        assert_eq!(snap.tick_mode, TickMode::Normal);
    }

    #[test]
    fn test_data_snapshot_default_tick_mode() {
        let snap = DataSnapshot::default();
        assert_eq!(snap.tick_mode, TickMode::Normal);
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
        assert_eq!(HealthMetrics::compute_health_score_with_backlog(0, 0), 100);
        assert_eq!(HealthMetrics::compute_health_score_with_backlog(0, 10), 100);
        assert_eq!(HealthMetrics::compute_health_score_with_backlog(0, 11), 90);
        assert_eq!(HealthMetrics::compute_health_score_with_backlog(0, 15), 90);
        assert_eq!(HealthMetrics::compute_health_score_with_backlog(1, 15), 80);
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
        assert_eq!(cfg.eco_tick_interval_ms, 1000);
        assert_eq!(cfg.event_backlog_threshold, 100);
    }

    #[test]
    fn test_datasource_config_default_eco_fields() {
        let cfg = DataSourceConfig::default();
        assert_eq!(cfg.eco_tick_interval_ms, 1000);
        assert_eq!(cfg.event_backlog_threshold, 100);
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
    // QuestSync 暂停状态跟踪测试
    // ============================================================

    fn quest_paused_event(quest_id: &str) -> NexusEvent {
        NexusEvent::QuestPaused {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: quest_id.into(),
            requested_by: "tui".into(),
        }
    }

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
        assert_eq!(sync.paused_quest_count(), 0);
        sync.apply_event(&quest_paused_event("q1"));
        assert_eq!(sync.paused_quest_count(), 1);
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
        sync.apply_event(&quest_resumed_event("q1"));
        assert_eq!(sync.paused_quest_count(), 1);
    }

    #[test]
    fn test_quest_sync_paused_id_not_in_quest_list_ignored() {
        let mut sync = QuestSync::new();
        sync.apply_event(&quest_list_event(vec![quest("q1", "first")]));
        sync.apply_event(&quest_paused_event("q-unknown"));
        assert_eq!(sync.paused_quest_count(), 0);
    }

    #[test]
    fn test_quest_sync_quest_list_updated_preserves_paused_ids() {
        let mut sync = QuestSync::new();
        sync.apply_event(&quest_list_event(vec![
            quest("q1", "first"),
            quest("q2", "second"),
        ]));
        sync.apply_event(&quest_paused_event("q1"));
        sync.apply_event(&quest_list_event(vec![quest("q1", "first")]));
        assert_eq!(sync.paused_quest_count(), 1);
    }

    #[test]
    fn test_quest_sync_quest_completed_removes_from_paused() {
        let mut sync = QuestSync::new();
        sync.apply_event(&quest_list_event(vec![
            quest("q1", "first"),
            quest("q2", "second"),
        ]));
        sync.apply_event(&quest_paused_event("q1"));
        assert_eq!(sync.paused_quest_count(), 1);
        sync.apply_event(&quest_completed_event("q1", QuestStatus::Completed));
        assert_eq!(sync.paused_quest_count(), 0);
    }

    // ============================================================
    // QuestSync 取消与优先级调整测试
    // ============================================================

    fn quest_cancelled_event(quest_id: &str) -> NexusEvent {
        NexusEvent::QuestCancelled {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: quest_id.into(),
            requested_by: "test".into(),
        }
    }

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
        let mut sync = QuestSync::new();
        sync.apply_event(&quest_list_event(vec![
            quest("q1", "first"),
            quest("q2", "second"),
        ]));
        sync.apply_event(&quest_paused_event("q1"));
        assert_eq!(sync.paused_quest_count(), 1);
        sync.apply_event(&quest_cancelled_event("q1"));
        assert_eq!(sync.paused_quest_count(), 0);
    }

    #[test]
    fn test_quest_sync_cancelled_unknown_id_no_change() {
        let mut sync = QuestSync::new();
        let q1 = quest("q1", "first");
        sync.apply_event(&quest_list_event(vec![q1.clone()]));
        let updated = sync.apply_event(&quest_cancelled_event("nonexistent"));
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
        assert_eq!(sync.quests()[0].priority, 200);
    }

    #[test]
    fn test_quest_sync_priority_adjusted_unknown_id_ignored() {
        let mut sync = QuestSync::new();
        let q1 = quest("q1", "first");
        sync.apply_event(&quest_list_event(vec![q1.clone()]));
        let updated = sync.apply_event(&quest_priority_adjusted_event("nonexistent", 200));
        assert!(updated.is_none());
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
        use super::pipeline::push_history;
        let mut history = Vec::new();
        for i in 0..70 {
            push_history(&mut history, i, 64);
        }
        assert_eq!(history.len(), 64);
        assert_eq!(history[0], 6);
        assert_eq!(history[63], 69);
    }

    // ============================================================
    // P2 新增同步器测试
    // ============================================================

    fn decay_event(coefficient: f32, recent: Vec<&str>) -> NexusEvent {
        NexusEvent::DecayMetricsReported {
            metadata: EventMetadata::new("decay-engine"),
            coefficient,
            recent_events: recent.into_iter().map(String::from).collect(),
            cycle_start: Utc::now(),
            // P2-11: 测试辅助函数默认 fallback_count_delta = 0
            fallback_count_delta: 0,
        }
    }

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

    fn mcp_heartbeat_event(node_id: &str, status: &str, throughput: u64) -> NexusEvent {
        NexusEvent::McpNodeHeartbeat {
            metadata: EventMetadata::new("mcp-mesh"),
            node_id: node_id.into(),
            status: status.into(),
            throughput,
            last_seen: Utc::now(),
        }
    }

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
        sync.apply_event(&mcp_heartbeat_event("node-1", "online", 100));
        assert_eq!(sync.nodes().len(), 1);
        assert_eq!(sync.nodes()[0].node_id, "node-1");
        assert_eq!(sync.nodes()[0].status, crate::types::NodeStatus::Online);
        assert_eq!(sync.nodes()[0].throughput, 100);
        sync.apply_event(&mcp_heartbeat_event("node-1", "degraded", 50));
        assert_eq!(sync.nodes().len(), 1);
        assert_eq!(sync.nodes()[0].status, crate::types::NodeStatus::Degraded);
        assert_eq!(sync.nodes()[0].throughput, 50);
        sync.apply_event(&mcp_heartbeat_event("node-2", "offline", 0));
        assert_eq!(sync.nodes().len(), 2);
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
        assert_eq!(nodes[3].status, crate::types::NodeStatus::Offline);
    }

    #[test]
    fn test_chtc_sync_upsert() {
        let mut sync = ChtcSync::new();
        assert!(sync.state().adapters.is_empty());
        sync.apply_event(&chtc_adapter_event("vscode", "vscode", 95, true));
        assert_eq!(sync.state().adapters.len(), 1);
        assert_eq!(sync.state().adapters[0].adapter_id, "vscode");
        assert_eq!(sync.state().adapters[0].compatibility_score, 95);
        assert!(sync.state().adapters[0].is_online);
        sync.apply_event(&chtc_adapter_event("vscode", "vscode", 80, false));
        assert_eq!(sync.state().adapters.len(), 1);
        assert_eq!(sync.state().adapters[0].compatibility_score, 80);
        assert!(!sync.state().adapters[0].is_online);
        sync.apply_event(&chtc_adapter_event("jetbrains", "jetbrains", 90, true));
        assert_eq!(sync.state().adapters.len(), 2);
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
        let snap = DataSnapshot::default();
        assert_eq!(snap.decay_metrics.coefficient, 1.0);
        assert!(snap.decay_metrics.cycle_start.is_none());
        assert_eq!(snap.router_metrics.kvbsr_stats.hit_rate, 0.0);
        assert!(snap.mcp_nodes.is_empty());
        assert!(snap.chtc_state.adapters.is_empty());
        assert!(snap.decay_history.is_empty());
    }

    // ============================================================
    // P7 新增同步器测试
    // ============================================================

    fn osa_event(sparsity: f32, context_mask: Vec<&str>) -> NexusEvent {
        NexusEvent::OmniSparseMasksComputed {
            metadata: EventMetadata::new("osa-coordinator"),
            mask_hash: format!("mask-{sparsity}"),
            sparsity,
            context_mask: context_mask.into_iter().map(String::from).collect(),
        }
    }

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
        assert!(sync.sparsity().is_none());
        assert!(sync.context_mask().is_empty());
        assert!(sync.sparsity_history().is_empty());
        let updated = sync.apply_event(&osa_event(0.45, vec!["file1.rs", "file2.rs"]));
        assert!(updated.is_some());
        assert_eq!(sync.sparsity(), Some(0.45));
        assert_eq!(sync.context_mask().len(), 2);
        assert_eq!(sync.sparsity_history().len(), 1);
        assert_eq!(sync.sparsity_history()[0], 450);
    }

    #[test]
    fn test_osa_sync_history_fifo() {
        let mut sync = OsaSync::new();
        for i in 0..300 {
            sync.apply_event(&osa_event(i as f32 / 1000.0, vec![]));
        }
        assert_eq!(sync.sparsity_history().len(), 256);
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
    fn test_clv_sync_snapshot_reported() -> Result<(), Box<dyn std::error::Error>> {
        let mut sync = ClvSync::new();
        assert!(sync.summary().is_none());
        let updated = sync.apply_event(&clv_event(2.5, 8));
        assert!(updated.is_some());
        let s = sync.summary().ok_or("expected summary")?;
        assert_eq!(s.block_means.len(), 8);
        assert!((s.l2_norm - 2.5).abs() < 1e-5);
        Ok(())
    }

    #[test]
    fn test_clv_sync_overwrites_previous() -> Result<(), Box<dyn std::error::Error>> {
        let mut sync = ClvSync::new();
        sync.apply_event(&clv_event(1.0, 8));
        sync.apply_event(&clv_event(2.0, 8));
        let s = sync.summary().ok_or("expected summary")?;
        assert!((s.l2_norm - 2.0).abs() < 1e-5);
        Ok(())
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
        let snap = DataSnapshot::default();
        assert!(snap.timeline_snapshots.is_empty());
        assert!(snap.osa_sparsity.is_none());
        assert!(snap.osa_context_mask.is_empty());
        assert!(snap.osa_sparsity_history.is_empty());
        assert!(snap.clv_summary.is_none());
        assert_eq!(snap.tick_mode, TickMode::Normal);
    }

    #[test]
    fn test_data_source_config_p7_fields_default() {
        let cfg = DataSourceConfig::default();
        assert_eq!(cfg.snapshot_interval_s, 30);
        assert_eq!(cfg.max_snapshots, 100);
    }

    // ============================================================
    // 导出测试
    // ============================================================

    fn make_test_quest(id: &str, title: &str, priority: u8) -> Quest {
        Quest {
            quest_id: id.to_string(),
            title: title.to_string(),
            priority,
            tasks: vec![Task {
                task_id: format!("{id}-t1"),
                description: "test task".to_string(),
                status: TaskStatus::Running,
                dependencies: vec![],
            }],
            thinking_mode: ThinkingMode::Standard,
            checkpoint_id: None,
        }
    }

    #[test]
    fn test_csv_export_format() -> Result<(), Box<dyn std::error::Error>> {
        let snapshot = DataSnapshot {
            quest_list: vec![
                make_test_quest("q1", "Build API", 5),
                make_test_quest("q2", "Test, with comma", 3),
            ],
            ..Default::default()
        };
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("test.csv");
        snapshot.export_quests_to(ExportFormat::Csv, &path)?;
        let content = std::fs::read_to_string(&path)?;
        assert!(content.starts_with("quest_id,title,priority,task_count,status"));
        assert!(content.contains("q1,Build API,5,1,Running"));
        assert!(content.contains("\"Test, with comma\""));
        Ok(())
    }

    #[test]
    fn test_json_export_format() -> Result<(), Box<dyn std::error::Error>> {
        let snapshot = DataSnapshot {
            quest_list: vec![make_test_quest("q1", "Build API", 5)],
            ..Default::default()
        };
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("test.json");
        snapshot.export_quests_to(ExportFormat::Json, &path)?;
        let content = std::fs::read_to_string(&path)?;
        let parsed: serde_json::Value = serde_json::from_str(&content)?;
        assert!(parsed.is_array());
        assert_eq!(parsed[0]["quest_id"], "q1");
        Ok(())
    }

    #[test]
    fn test_csv_escape_quotes() {
        let escaped = csv_escape("say \"hello\"");
        assert_eq!(escaped, "\"say \"\"hello\"\"\"");
    }

    #[test]
    fn test_csv_escape_no_special_chars() {
        let escaped = csv_escape("simple_field");
        assert_eq!(escaped, "simple_field");
    }

    #[test]
    fn test_quest_status_label_all_states() {
        let empty = Quest {
            quest_id: "e1".into(),
            title: "empty".into(),
            tasks: vec![],
            thinking_mode: ThinkingMode::Standard,
            checkpoint_id: None,
            priority: 128,
        };
        assert_eq!(quest_status_label(&empty), "Pending");

        let running = Quest {
            quest_id: "r1".into(),
            title: "running".into(),
            tasks: vec![Task {
                task_id: "t1".into(),
                description: "".into(),
                status: TaskStatus::Running,
                dependencies: vec![],
            }],
            thinking_mode: ThinkingMode::Standard,
            checkpoint_id: None,
            priority: 128,
        };
        assert_eq!(quest_status_label(&running), "Running");

        let completed = Quest {
            quest_id: "c1".into(),
            title: "completed".into(),
            tasks: vec![Task {
                task_id: "t1".into(),
                description: "".into(),
                status: TaskStatus::Completed,
                dependencies: vec![],
            }],
            thinking_mode: ThinkingMode::Standard,
            checkpoint_id: None,
            priority: 128,
        };
        assert_eq!(quest_status_label(&completed), "Completed");

        let failed = Quest {
            quest_id: "f1".into(),
            title: "failed".into(),
            tasks: vec![Task {
                task_id: "t1".into(),
                description: "".into(),
                status: TaskStatus::Failed,
                dependencies: vec![],
            }],
            thinking_mode: ThinkingMode::Standard,
            checkpoint_id: None,
            priority: 128,
        };
        assert_eq!(quest_status_label(&failed), "Failed");
    }

    #[test]
    fn test_export_format_extension() {
        assert_eq!(ExportFormat::Csv.extension(), "csv");
        assert_eq!(ExportFormat::Json.extension(), "json");
    }

    // ===== M3b ChatSync 单测 =====

    fn chat_submitted(query: &str) -> NexusEvent {
        NexusEvent::TuiChatSubmitted {
            metadata: EventMetadata::new("chimera-tui"),
            session_id: "s1".into(),
            query: query.into(),
            slash_command: None,
        }
    }

    fn chat_chunk(delta: &str) -> NexusEvent {
        NexusEvent::TuiChatResponseChunk {
            metadata: EventMetadata::new("orchestrator"),
            session_id: "s1".into(),
            delta: delta.into(),
            cursor_hint: 0,
        }
    }

    #[test]
    fn chat_sync_submit_appends_user_message() {
        let mut sync = ChatSync::new(500);
        sync.apply_event(&chat_submitted("hi"));
        let msgs = sync.messages();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].role, ChatRole::User);
        assert_eq!(msgs[0].content, "hi");
    }

    #[test]
    fn chat_sync_chunks_accumulate_into_one_assistant() {
        let mut sync = ChatSync::new(500);
        sync.apply_event(&chat_submitted("hi"));
        sync.apply_event(&chat_chunk("Hel"));
        sync.apply_event(&chat_chunk("lo"));
        let msgs = sync.messages();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[1].role, ChatRole::Assistant);
        assert_eq!(msgs[1].content, "Hello");
    }

    #[test]
    fn chat_sync_completed_starts_new_assistant_for_next_chunk() {
        let mut sync = ChatSync::new(500);
        sync.apply_event(&chat_submitted("hi"));
        sync.apply_event(&chat_chunk("a"));
        sync.apply_event(&NexusEvent::TuiChatCompleted {
            metadata: EventMetadata::new("orchestrator"),
            session_id: "s1".into(),
            tool_use: None,
        });
        sync.apply_event(&chat_chunk("b"));
        let msgs = sync.messages();
        assert_eq!(msgs.len(), 3);
        assert_eq!(msgs[1].content, "a");
        assert_eq!(msgs[2].content, "b");
    }

    #[test]
    fn chat_sync_status_changed_updates_status() {
        let mut sync = ChatSync::new(500);
        assert_eq!(sync.status(), ChatStatus::Idle);
        sync.apply_event(&NexusEvent::TuiChatStatusChanged {
            metadata: EventMetadata::new("orchestrator"),
            session_id: "s1".into(),
            status: ChatStatus::Thinking,
        });
        assert_eq!(sync.status(), ChatStatus::Thinking);
    }

    #[test]
    fn chat_sync_submit_does_not_touch_status() {
        let mut sync = ChatSync::new(500);
        sync.apply_event(&chat_submitted("hi"));
        assert_eq!(sync.status(), ChatStatus::Idle);
    }

    #[test]
    fn chat_sync_enforces_max_messages_fifo() {
        let mut sync = ChatSync::new(2);
        sync.apply_event(&chat_submitted("a"));
        sync.apply_event(&chat_submitted("b"));
        sync.apply_event(&chat_submitted("c"));
        let msgs = sync.messages();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].content, "b");
        assert_eq!(msgs[1].content, "c");
    }

    // ===== P0 交互链 ActionFeedbackSync 单测 =====

    #[test]
    fn action_feedback_sync_completed_records_success() {
        let mut sync = ActionFeedbackSync::new();
        assert_eq!(sync.seq(), 0);
        assert_eq!(sync.latest(), None);
        sync.apply_event(&NexusEvent::TuiActionCompleted {
            metadata: EventMetadata::new("chimera-cli"),
            action_id: "quest.pause".into(),
            result: "已暂停 Quest q-1".into(),
        });
        assert_eq!(sync.seq(), 1);
        assert_eq!(sync.latest(), Some(("已暂停 Quest q-1".to_string(), false)));
    }

    #[test]
    fn action_feedback_sync_failed_marks_error() {
        let mut sync = ActionFeedbackSync::new();
        sync.apply_event(&NexusEvent::TuiActionFailed {
            metadata: EventMetadata::new("chimera-cli"),
            action_id: "task.pause".into(),
            error: "尚未实现".into(),
        });
        assert_eq!(sync.seq(), 1);
        assert_eq!(sync.latest(), Some(("尚未实现".to_string(), true)));
    }

    #[test]
    fn action_feedback_sync_seq_monotonic_and_ignores_non_action() {
        let mut sync = ActionFeedbackSync::new();
        for i in 0..3 {
            sync.apply_event(&NexusEvent::TuiActionCompleted {
                metadata: EventMetadata::new("chimera-cli"),
                action_id: "quest.resume".into(),
                result: format!("r{i}"),
            });
        }
        assert_eq!(sync.seq(), 3);
        sync.apply_event(&chat_submitted("hi"));
        assert_eq!(sync.seq(), 3);
    }

    // === P1-W2.2 CriticalDroppedSync 测试 ===

    fn critical_dropped_alert(count: u64) -> NexusEvent {
        NexusEvent::EfficiencyAlertTriggered {
            metadata: EventMetadata::new("efficiency-monitor"),
            rule_id: "critical-event-dropped".into(),
            metric_name: CRITICAL_DROPPED_METRIC_NAME.to_string(),
            triggered_value: count as f64,
            threshold: 0.0,
        }
    }

    fn unrelated_alert() -> NexusEvent {
        NexusEvent::EfficiencyAlertTriggered {
            metadata: EventMetadata::new("efficiency-monitor"),
            rule_id: "critical-CacheHit".into(),
            metric_name: "CacheHit".to_string(),
            triggered_value: 1.0,
            threshold: 1.0,
        }
    }

    #[test]
    fn critical_dropped_sync_default_count_zero() {
        let sync = CriticalDroppedSync::new();
        assert_eq!(sync.count(), 0);
    }

    #[test]
    fn critical_dropped_sync_applies_matching_event() {
        let mut sync = CriticalDroppedSync::new();
        sync.apply_event(&critical_dropped_alert(42));
        assert_eq!(sync.count(), 42);
    }

    #[test]
    fn critical_dropped_sync_ignores_unrelated_alert() {
        let mut sync = CriticalDroppedSync::new();
        sync.apply_event(&critical_dropped_alert(10));
        sync.apply_event(&unrelated_alert());
        assert_eq!(sync.count(), 10);
    }

    #[test]
    fn critical_dropped_sync_ignores_non_alert_event() {
        let mut sync = CriticalDroppedSync::new();
        sync.apply_event(&critical_dropped_alert(5));
        sync.apply_event(&NexusEvent::CacheHit {
            metadata: EventMetadata::new("scc-cache"),
            cache_key: "k1".into(),
        });
        assert_eq!(sync.count(), 5);
    }

    #[test]
    fn critical_dropped_sync_count_monotonic_mirror() {
        let mut sync = CriticalDroppedSync::new();
        sync.apply_event(&critical_dropped_alert(10));
        assert_eq!(sync.count(), 10);
        sync.apply_event(&critical_dropped_alert(15));
        assert_eq!(sync.count(), 15);
        sync.apply_event(&critical_dropped_alert(15));
        assert_eq!(sync.count(), 15);
    }
}
