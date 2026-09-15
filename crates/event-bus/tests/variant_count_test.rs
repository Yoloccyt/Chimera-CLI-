//! NexusEvent 变体计数锁定测试 — M1 架构重构方向 3 任务 4
//!
//! 对应任务: severity() fail-closed 改造的配套锁定测试
//! 架构层: L1 Core (event-bus)
//!
//! # WHY(防未来静默漂移的三层锁)
//! Rust 无枚举反射,运行时"变体总数"只能来自人工登记的权威清单。本文件提供:
//! 1. **编译期穷举锁** `variant_ordinals()` — match 无通配符,新增/改名/删除
//!    变体即编译错误。即使未来 severity() 被误加回通配符(本 M1 修复的回归),
//!    这道锁依然拦住未登记的变体。
//! 2. **计数锁** `test_variant_count_lock` — 样本清单实际构造的变体数必须
//!    等于 `VARIANT_COUNT_LOCK`(145);样本与序数表交叉断言一一对应
//!    (清单漏登/重登即失败)。
//! 3. **分级分布锁** `test_severity_distribution_lock` — severity() 的
//!    Critical/Info/Normal 分布必须等于 17/11/117;并与 bus.rs
//!    `LANE_FORBIDDEN_SHARD` 的 17 个 Critical 名单双向互锁。
//!
//! # 维护规约(新增变体时必须同步修改)
//! - enum 新增变体 → 编译器强制修改 severity()/type_name()/topic(),
//!   同时把新变体登记进 `all_variant_samples()` 与 `variant_ordinals()`,
//!   并把 `VARIANT_COUNT_LOCK` 及分布锁 +1;
//! - 新增 Critical 变体 → 还须同步 bus.rs `is_critical_mpsc_event`
//!   (双清单同步红线)与 `LANE_FORBIDDEN_SHARD`。

#![forbid(unsafe_code)]

use std::collections::HashSet;

use chrono::Utc;
use event_bus::payloads::{RollbackDiagnosticContext, RollbackTriggerType};
use event_bus::topic::EventTopic;
use event_bus::{
    ActionSource, AgentStatus, BudgetMetricsPayload, ChatStatus, ClvSummary, CompatLevel,
    ConsultUrgency, EventMetadata, EventSeverity, NexusEvent, QuestStatus, RouterStatsPayload,
    TaskPriority, VoteValue, LANE_FORBIDDEN_SHARD,
};
use nexus_contracts::behavior_contract::ContractContext;
use nexus_contracts::reward::RewardSignal;

/// 变体总数锁定值(2026-09-12 实测 enum = 145,含 WIP 新增 TuiChatHistoryReplaced)
const VARIANT_COUNT_LOCK: usize = 145;
/// severity 分布锁定值(Critical,与 bus.rs CRITICAL_TOTAL = 17 一致)
const CRITICAL_LOCK: usize = 17;
/// severity 分布锁定值(Info)
const INFO_LOCK: usize = 11;
/// severity 分布锁定值(Normal)
const NORMAL_LOCK: usize = 117;

/// 构造全部 145 个 NexusEvent 变体的最小实例清单(enum 声明序)
///
/// WHY 每变体一个构造器: 把"变体清单"变成编译期契约 —— 变体改名/删除立即
/// 编译错误;新增变体未登记时,下方计数锁与序数表交叉断言失败。
/// 字段值一律取最小合法值(空集合/零值/None),与业务语义无关。
fn all_variant_samples() -> Vec<NexusEvent> {
    vec![
        NexusEvent::UserIntentEncoded {
            metadata: EventMetadata::new("test"),
            intent_id: "s".into(),
            raw_text: "s".into(),
            risk_level: 1,
        },
        NexusEvent::NexusStateChanged {
            metadata: EventMetadata::new("test"),
            state_hash: "s".into(),
            prev_hash: "s".into(),
        },
        NexusEvent::ModelRouteSelected {
            metadata: EventMetadata::new("test"),
            quest_id: "s".into(),
            model_id: "s".into(),
            route_reason: "s".into(),
        },
        NexusEvent::QuestCreated {
            metadata: EventMetadata::new("test"),
            quest_id: "s".into(),
            title: "s".into(),
            task_count: 1,
        },
        NexusEvent::QuestProgressUpdated {
            metadata: EventMetadata::new("test"),
            quest_id: "s".into(),
            completed: 1,
            total: 1,
        },
        NexusEvent::QuestListUpdated {
            metadata: EventMetadata::new("test"),
            quests: vec![],
            source: "s".into(),
        },
        NexusEvent::QuestCompleted {
            metadata: EventMetadata::new("test"),
            quest_id: "s".into(),
            status: QuestStatus::Completed,
        },
        NexusEvent::ThinkingModeSwitched {
            metadata: EventMetadata::new("test"),
            quest_id: "s".into(),
            from_mode: "s".into(),
            to_mode: "s".into(),
            reason: "s".into(),
        },
        NexusEvent::CheckpointSaved {
            metadata: EventMetadata::new("test"),
            quest_id: "s".into(),
            checkpoint_id: "s".into(),
            memory_snapshot_hash: "s".into(),
        },
        NexusEvent::CheckpointLoaded {
            metadata: EventMetadata::new("test"),
            quest_id: "s".into(),
            checkpoint_id: "s".into(),
        },
        NexusEvent::ConsensusReached {
            metadata: EventMetadata::new("test"),
            quest_id: "s".into(),
            decision_hash: "s".into(),
            dpo_pair_id: None,
        },
        NexusEvent::VoteCast {
            metadata: EventMetadata::new("test"),
            proposal_id: "s".into(),
            voter: "s".into(),
            vote: true,
        },
        NexusEvent::CapabilityFrozen {
            metadata: EventMetadata::new("test"),
            capability_id: "s".into(),
            reason: "s".into(),
        },
        NexusEvent::ShadowBreakerTripped {
            metadata: EventMetadata::new("test"),
            reason: "s".into(),
        },
        NexusEvent::BudgetExceeded {
            metadata: EventMetadata::new("test"),
            budget_type: "s".into(),
            current: 1,
            limit: 1,
        },
        NexusEvent::SandboxViolation {
            metadata: EventMetadata::new("test"),
            violation_type: "s".into(),
            detail: "s".into(),
        },
        NexusEvent::OperationProduced {
            metadata: EventMetadata::new("test"),
            op_id: "s".into(),
            content_hash: "s".into(),
        },
        NexusEvent::PredictionVerified {
            metadata: EventMetadata::new("test"),
            op_id: "s".into(),
            score: 0.5,
        },
        NexusEvent::OmniSparseMasksComputed {
            metadata: EventMetadata::new("test"),
            mask_hash: "s".into(),
            sparsity: 0.5,
            context_mask: vec![],
        },
        NexusEvent::ToolsRouted {
            metadata: EventMetadata::new("test"),
            routed_count: 1,
            top_tool: "s".into(),
            routed_tools: vec![],
        },
        NexusEvent::ExecutionCompleted {
            metadata: EventMetadata::new("test"),
            quest_id: "s".into(),
            result_hash: "s".into(),
        },
        NexusEvent::MemoryMetricsReported {
            metadata: EventMetadata::new("test"),
            hit_rate: 0.5,
            evictions: 1,
        },
        NexusEvent::MemoryTiered {
            metadata: EventMetadata::new("test"),
            tier: "s".into(),
            item_count: 1,
            memory_id: None,
        },
        NexusEvent::CacheHit {
            metadata: EventMetadata::new("test"),
            cache_key: "s".into(),
        },
        NexusEvent::CacheMiss {
            metadata: EventMetadata::new("test"),
            cache_key: "s".into(),
        },
        NexusEvent::WikiUpdated {
            metadata: EventMetadata::new("test"),
            wiki_hash: "s".into(),
            delta: 1,
        },
        NexusEvent::EvolutionTriggered {
            metadata: EventMetadata::new("test"),
            generation: 1,
            fitness: 0.5,
        },
        NexusEvent::DpoPairGenerated {
            metadata: EventMetadata::new("test"),
            pair_id: "s".into(),
            chosen: "s".into(),
            rejected: "s".into(),
        },
        NexusEvent::AuditLogged {
            metadata: EventMetadata::new("test"),
            audit_hash: "s".into(),
            severity: "s".into(),
        },
        NexusEvent::McpMessageReceived {
            metadata: EventMetadata::new("test"),
            source_node: "s".into(),
            msg_type: "s".into(),
        },
        NexusEvent::SlowConsumerDropped {
            metadata: EventMetadata::new("test"),
            subscriber_id: "s".into(),
            lag: 1,
            dropped_count: 1,
        },
        NexusEvent::ContextWindowSwitched {
            metadata: EventMetadata::new("test"),
            from_tier: "s".into(),
            to_tier: "s".into(),
            reason: "s".into(),
        },
        NexusEvent::ContextCompressed {
            metadata: EventMetadata::new("test"),
            original_size: 1,
            compressed_size: 1,
            ratio: 0.5,
        },
        NexusEvent::CapabilityTiered {
            metadata: EventMetadata::new("test"),
            capability_id: "s".into(),
            from_tier: "s".into(),
            to_tier: "s".into(),
            reason: "s".into(),
        },
        NexusEvent::CapabilityTierStatsReported {
            metadata: EventMetadata::new("test"),
            hot: 1,
            warm: 1,
            cold: 1,
            ice: 1,
        },
        NexusEvent::BlocksRebalanced {
            metadata: EventMetadata::new("test"),
            old_block_count: 1,
            new_block_count: 1,
        },
        NexusEvent::ExpertActivated {
            metadata: EventMetadata::new("test"),
            activated_experts: vec![],
            suppressed_experts: vec![],
            top_gate_value: 0.5,
        },
        NexusEvent::ActivationThresholdAdjusted {
            metadata: EventMetadata::new("test"),
            old_threshold: 0.5,
            new_threshold: 0.5,
            load_factor: 0.5,
        },
        NexusEvent::ActivationCacheStats {
            metadata: EventMetadata::new("test"),
            hit_rate: 0.5,
            entry_count: 1,
        },
        NexusEvent::GatherCompleted {
            metadata: EventMetadata::new("test"),
            total: 1,
            succeeded: 1,
            failed: 1,
            latency_ms: 0.5,
        },
        NexusEvent::OperationTimedOut {
            metadata: EventMetadata::new("test"),
            operation_id: "s".into(),
            timeout_ms: 1,
        },
        NexusEvent::GatherTimedOut {
            metadata: EventMetadata::new("test"),
            deadline_ms: 1,
            elapsed_ms: 1,
            total: 1,
            abandoned: 1,
        },
        NexusEvent::OrphanCallDetected {
            metadata: EventMetadata::new("test"),
            operation_id: "s".into(),
            spawn_location: "s".into(),
        },
        NexusEvent::ProducerStrategyAdjusted {
            metadata: EventMetadata::new("test"),
            adjustment_reason: "s".into(),
            new_strategy: "s".into(),
        },
        NexusEvent::PredictionMade {
            metadata: EventMetadata::new("test"),
            quest_id: "s".into(),
            n: 1,
            avg_confidence: 0.5,
        },
        NexusEvent::PredictionStatsReported {
            metadata: EventMetadata::new("test"),
            success_rate_by_n: Default::default(),
        },
        NexusEvent::PredictionRolledBack {
            metadata: EventMetadata::new("test"),
            failed_step: 1,
            rollback_to: 1,
        },
        NexusEvent::CachePrefetched {
            metadata: EventMetadata::new("test"),
            prefetched_ids: vec![],
        },
        NexusEvent::CacheStatsReported {
            metadata: EventMetadata::new("test"),
            hit_rate: 0.5,
            eviction_count: 1,
        },
        NexusEvent::ExpertRouted {
            metadata: EventMetadata::new("test"),
            routed_tool: "s".into(),
            confidence: 0.5,
        },
        NexusEvent::EntropyBalanced {
            metadata: EventMetadata::new("test"),
            old_entropy: 0.5,
            new_entropy: 0.5,
            redistributed_count: 1,
        },
        NexusEvent::ExpertRegistered {
            metadata: EventMetadata::new("test"),
            tool_id: "s".into(),
        },
        NexusEvent::ExpertUnregistered {
            metadata: EventMetadata::new("test"),
            tool_id: "s".into(),
        },
        NexusEvent::DebateStarted {
            metadata: EventMetadata::new("test"),
            quest_id: "s".into(),
            proposal_id: "s".into(),
            participant_count: 1,
        },
        NexusEvent::SkepticVeto {
            metadata: EventMetadata::new("test"),
            quest_id: "s".into(),
            veto_reason: "s".into(),
            frozen_capabilities: vec![],
        },
        NexusEvent::VetoOverridden {
            metadata: EventMetadata::new("test"),
            quest_id: "s".into(),
            proposal_id: "s".into(),
            veto_reason: "s".into(),
            override_reason: "s".into(),
            override_by: "s".into(),
        },
        NexusEvent::RedTeamAudit {
            metadata: EventMetadata::new("test"),
            vulnerability_type: "s".into(),
            failed_probes: 1,
            total_probes: 1,
            detection_rate: 0.5,
            remediation_suggestion: "s".into(),
        },
        NexusEvent::BudgetAdjusted {
            metadata: EventMetadata::new("test"),
            quest_id: "s".into(),
            old_tier: "s".into(),
            new_tier: "s".into(),
            coefficient: 0.5,
            reason: "s".into(),
        },
        NexusEvent::AsaIntervention {
            metadata: EventMetadata::new("test"),
            operation_id: "s".into(),
            action: "s".into(),
            safety_score: 0.5,
            block_reason: None,
            alternative_suggestion: None,
        },
        NexusEvent::AhirtProbeCompleted {
            metadata: EventMetadata::new("test"),
            probe_type: "s".into(),
            total: 1,
            passed: 1,
            failed: 1,
            detection_rate: 0.5,
        },
        NexusEvent::RoleRegistered {
            metadata: EventMetadata::new("test"),
            role_id: "s".into(),
            role_name: "s".into(),
            voting_weight: 0.5,
        },
        NexusEvent::BudgetStatsReported {
            metadata: EventMetadata::new("test"),
            total_consumption: 0.5,
            remaining_budget: 0.5,
            utilization_rate: 0.5,
        },
        NexusEvent::BudgetMetricsUpdated {
            metadata: EventMetadata::new("test"),
            metrics: BudgetMetricsPayload {
                total_consumption: 0.0,
                remaining_budget: 0.0,
                utilization_rate: 0.0,
                current_tier: "t".into(),
                coefficient: 1.0,
                is_exceeded: false,
                alert: None,
            },
        },
        NexusEvent::NmcEncoded {
            metadata: EventMetadata::new("test"),
            modality: "s".into(),
            content_hash: "s".into(),
            clv_dimension: 1,
        },
        NexusEvent::ChtcToolCallReceived {
            metadata: EventMetadata::new("test"),
            call_id: "s".into(),
            tool_id: "s".into(),
            ide_source: "s".into(),
            parameters_hash: "s".into(),
        },
        NexusEvent::SsraFusionCompleted {
            metadata: EventMetadata::new("test"),
            quest_id: "s".into(),
            fused_template_id: "s".into(),
            latency_ms: 1,
            confidence: 0.5,
        },
        NexusEvent::GsoePolicyUpdated {
            metadata: EventMetadata::new("test"),
            generation: 1,
            improvement: 0.5,
            new_mutation_rate: 0.5,
            new_selection_pressure: 0.5,
        },
        NexusEvent::LsctTierSwitched {
            metadata: EventMetadata::new("test"),
            capability_id: "s".into(),
            from_tier: "s".into(),
            to_tier: "s".into(),
            reason: "s".into(),
        },
        NexusEvent::McpMeshTransactionCompleted {
            metadata: EventMetadata::new("test"),
            transaction_id: "s".into(),
            participant_count: 1,
            latency_ms: 1,
            success: true,
            capability_id: None,
        },
        NexusEvent::CsnSubstitutionTriggered {
            metadata: EventMetadata::new("test"),
            original_capability_id: "s".into(),
            substitute_id: "s".into(),
            similarity_score: 0.5,
            degradation_level: 1,
        },
        NexusEvent::SesaActivationCompleted {
            metadata: EventMetadata::new("test"),
            total_experts: 1,
            active_experts: 1,
            sparsity_ratio: 0.5,
            latency_us: 1,
        },
        NexusEvent::EfficiencyAlertTriggered {
            metadata: EventMetadata::new("test"),
            rule_id: "s".into(),
            metric_name: "s".into(),
            triggered_value: 0.5,
            threshold: 0.5,
        },
        NexusEvent::QuestPauseRequested {
            metadata: EventMetadata::new("test"),
            quest_id: "s".into(),
            requested_by: "s".into(),
        },
        NexusEvent::QuestResumeRequested {
            metadata: EventMetadata::new("test"),
            quest_id: "s".into(),
            requested_by: "s".into(),
        },
        NexusEvent::VoteCastRequested {
            metadata: EventMetadata::new("test"),
            proposal_id: "s".into(),
            voter: "s".into(),
            vote: VoteValue::Yes,
        },
        NexusEvent::RefreshStateRequested {
            metadata: EventMetadata::new("test"),
            requested_by: "s".into(),
        },
        NexusEvent::QuestPaused {
            metadata: EventMetadata::new("test"),
            quest_id: "s".into(),
            requested_by: "s".into(),
        },
        NexusEvent::QuestResumed {
            metadata: EventMetadata::new("test"),
            quest_id: "s".into(),
            requested_by: "s".into(),
        },
        NexusEvent::QuestCancelRequested {
            metadata: EventMetadata::new("test"),
            quest_id: "s".into(),
            requested_by: "s".into(),
        },
        NexusEvent::QuestCancelled {
            metadata: EventMetadata::new("test"),
            quest_id: "s".into(),
            requested_by: "s".into(),
        },
        NexusEvent::QuestPriorityChanged {
            metadata: EventMetadata::new("test"),
            quest_id: "s".into(),
            new_priority: 1,
            requested_by: "s".into(),
        },
        NexusEvent::QuestPriorityAdjusted {
            metadata: EventMetadata::new("test"),
            quest_id: "s".into(),
            new_priority: 1,
            requested_by: "s".into(),
        },
        NexusEvent::DecayMetricsReported {
            metadata: EventMetadata::new("test"),
            coefficient: 0.5,
            recent_events: vec![],
            cycle_start: Utc::now(),
            fallback_count_delta: 1,
        },
        NexusEvent::RouterStatsReported {
            metadata: EventMetadata::new("test"),
            kvbsr_stats: RouterStatsPayload {
                hit_rate: 0.0,
                p50_latency_us: 0,
                p95_latency_us: 0,
                p99_latency_us: 0,
                hot_capabilities: vec![],
            },
            sesa_stats: RouterStatsPayload {
                hit_rate: 0.0,
                p50_latency_us: 0,
                p95_latency_us: 0,
                p99_latency_us: 0,
                hot_capabilities: vec![],
            },
            faae_stats: RouterStatsPayload {
                hit_rate: 0.0,
                p50_latency_us: 0,
                p95_latency_us: 0,
                p99_latency_us: 0,
                hot_capabilities: vec![],
            },
        },
        NexusEvent::McpNodeHeartbeat {
            metadata: EventMetadata::new("test"),
            node_id: "s".into(),
            status: "s".into(),
            throughput: 1,
            last_seen: Utc::now(),
        },
        NexusEvent::ChtcAdapterStatus {
            metadata: EventMetadata::new("test"),
            adapter_id: "s".into(),
            adapter_type: "s".into(),
            compatibility_score: 1,
            recent_requests: vec![],
            is_online: true,
        },
        NexusEvent::ClvSnapshotReported {
            metadata: EventMetadata::new("test"),
            modality: "s".into(),
            content_hash: "s".into(),
            clv_summary: ClvSummary {
                block_means: vec![],
                l2_norm: 0.0,
                top_dims: vec![],
            },
        },
        NexusEvent::AgentTaskDelegated {
            metadata: EventMetadata::new("test"),
            from: "s".into(),
            to: "s".into(),
            task_id: "s".into(),
            deadline: Utc::now(),
            priority: TaskPriority::Medium,
        },
        NexusEvent::AgentTaskCompleted {
            metadata: EventMetadata::new("test"),
            from: "s".into(),
            to: "s".into(),
            task_id: "s".into(),
            result_summary: "s".into(),
        },
        NexusEvent::AgentTaskFailed {
            metadata: EventMetadata::new("test"),
            from: "s".into(),
            to: "s".into(),
            task_id: "s".into(),
            error: "s".into(),
            retry_count: 1,
        },
        NexusEvent::AgentConsultRequested {
            metadata: EventMetadata::new("test"),
            from: "s".into(),
            to: "s".into(),
            question: "s".into(),
            context: "s".into(),
            urgency: ConsultUrgency::Medium,
        },
        NexusEvent::AgentConsultResponded {
            metadata: EventMetadata::new("test"),
            from: "s".into(),
            to: "s".into(),
            answer: "s".into(),
            references: vec![],
        },
        NexusEvent::AgentHeartbeat {
            metadata: EventMetadata::new("test"),
            from: "s".into(),
            status: AgentStatus::Idle,
            current_task: None,
            token_usage: 1,
            memory_usage_mb: 1,
        },
        NexusEvent::AgentContextOverflow {
            metadata: EventMetadata::new("test"),
            agent_id: "s".into(),
            current_tokens: 1,
            max_tokens: 1,
        },
        NexusEvent::TuiActionRequested {
            metadata: EventMetadata::new("test"),
            request_id: "s".into(),
            action_id: "s".into(),
            payload: "s".into(),
            source: ActionSource::Palette,
        },
        NexusEvent::TuiActionProgressed {
            metadata: EventMetadata::new("test"),
            action_id: "s".into(),
            delta: "s".into(),
        },
        NexusEvent::TuiActionCompleted {
            metadata: EventMetadata::new("test"),
            request_id: "s".into(),
            action_id: "s".into(),
            result: "s".into(),
        },
        NexusEvent::TuiActionFailed {
            metadata: EventMetadata::new("test"),
            request_id: "s".into(),
            action_id: "s".into(),
            error: "s".into(),
        },
        NexusEvent::TuiChatSubmitted {
            metadata: EventMetadata::new("test"),
            session_id: "s".into(),
            query: "s".into(),
            slash_command: None,
        },
        NexusEvent::TuiChatResponseChunk {
            metadata: EventMetadata::new("test"),
            session_id: "s".into(),
            delta: "s".into(),
            cursor_hint: 1,
        },
        NexusEvent::TuiChatCompleted {
            metadata: EventMetadata::new("test"),
            session_id: "s".into(),
            tool_use: None,
        },
        NexusEvent::TuiChatStatusChanged {
            metadata: EventMetadata::new("test"),
            session_id: "s".into(),
            status: ChatStatus::Idle,
        },
        NexusEvent::TuiChatHistoryReplaced {
            metadata: EventMetadata::new("test"),
            session_id: "s".into(),
            messages: vec![],
        },
        NexusEvent::TuiHello {
            metadata: EventMetadata::new("test"),
            proto: "s".into(),
            tui_version: "s".into(),
            caps: vec![],
        },
        NexusEvent::TuiHelloAck {
            metadata: EventMetadata::new("test"),
            proto: "s".into(),
            compat: CompatLevel::Full,
            server_version: "s".into(),
        },
        NexusEvent::R1ShadowRegressionDetected {
            metadata: EventMetadata::new("test"),
            report_date: Utc::now(),
            regression_streak: 1,
        },
        NexusEvent::R1ShadowPromotionReady {
            metadata: EventMetadata::new("test"),
            report_date: Utc::now(),
            win_rate: 0.5,
            ewma_level: 0.5,
        },
        NexusEvent::R1ShadowRollbackFailed {
            metadata: EventMetadata::new("test"),
            reason: "s".into(),
            trigger_type: RollbackTriggerType::Unknown,
            triggered_at: None,
            details: "s".into(),
            diagnostic: RollbackDiagnosticContext::default(),
        },
        NexusEvent::SpecRegistered {
            metadata: EventMetadata::new("test"),
            spec_name: "s".into(),
            spec_version: 1,
            parent_version: None,
            source: "s".into(),
        },
        NexusEvent::R2FreezeViolation {
            metadata: EventMetadata::new("test"),
            violation_type: "s".into(),
            evidence: "s".into(),
        },
        NexusEvent::R2FreezeRollbackFailed {
            metadata: EventMetadata::new("test"),
            reason: "s".into(),
        },
        NexusEvent::CoordinationRatioReported {
            metadata: EventMetadata::new("test"),
            coordination_cost_ms: 0.5,
            inference_gain: 0.5,
            cost_index: 0.5,
            gain_index: 0.5,
            ratio: 0.5,
            is_paradox_risk: true,
            threshold: 0.5,
            sample_count: 1,
        },
        NexusEvent::AuditFindingRaised {
            metadata: EventMetadata::new("test"),
            finding_severity: "s".into(),
            category: "s".into(),
            message: "s".into(),
            evidence_kind: "s".into(),
            fix_hint: "s".into(),
        },
        NexusEvent::HarnessReportGenerated {
            metadata: EventMetadata::new("test"),
            task_comprehension: 0.5,
            controllable_execution: 0.5,
            change_verification: 0.5,
            reliable_delivery: 0.5,
            experience_accumulation: 0.5,
            findings_count: 1,
        },
        NexusEvent::DebateCompleted {
            metadata: EventMetadata::new("test"),
            quest_id: "s".into(),
            proposal_id: "s".into(),
            debate_latency_ms: 0.5,
            strategy: "s".into(),
            weighted_approval_rate: None,
            participation_rate: None,
            divergence: None,
            abstention_rate: None,
            consensus_margin: None,
            outcome: "s".into(),
        },
        NexusEvent::DelegationCompleted {
            metadata: EventMetadata::new("test"),
            parent_id: "s".into(),
            quest_id: None,
            total_overhead_ms: 0.5,
            sub_task_count: 1,
            success_count: 1,
        },
        NexusEvent::ParliamentStrategyCapChanged {
            metadata: EventMetadata::new("test"),
            old_cap: "s".into(),
            new_cap: "s".into(),
            ratio: 0.5,
            threshold: 0.5,
        },
        NexusEvent::ModelAffinitySelected {
            metadata: EventMetadata::new("test"),
            intent_id: "s".into(),
            route_key: "s".into(),
            dialect: "s".into(),
            cost_estimate_micro: 1,
            peak_factor_percent: 1,
        },
        NexusEvent::CrossVendorNegotiation {
            metadata: EventMetadata::new("test"),
            session_id: "s".into(),
            quest_id: "s".into(),
            producer_provider: "s".into(),
            verifier_provider: "s".into(),
            skeptic_provider: "s".into(),
            cross_vendor_enforced: true,
            decorrelation_status: "s".into(),
        },
        NexusEvent::ProviderDegraded {
            metadata: EventMetadata::new("test"),
            route_key: "s".into(),
            reason: "s".into(),
            health_score: 1,
        },
        NexusEvent::AffinityCapabilityNegotiated {
            metadata: EventMetadata::new("test"),
            route_key: "s".into(),
            fidelity: "s".into(),
            degraded_capabilities: vec![],
        },
        NexusEvent::AffinityQuotaExhausted {
            metadata: EventMetadata::new("test"),
            route_key: "s".into(),
            reason: "s".into(),
        },
        NexusEvent::AffinityUnknownField {
            metadata: EventMetadata::new("test"),
            route_key: "s".into(),
            dialect: "s".into(),
            raw_excerpt: "s".into(),
        },
        NexusEvent::StreamSessionCompleted {
            metadata: EventMetadata::new("test"),
            intent_id: "s".into(),
            route_key: "s".into(),
            input_tokens: 1,
            output_tokens: 1,
            cache_hit_tokens: 1,
            cost_actual_micro: 1,
            ttft_ms: 1,
            semantic_cache_hit: true,
            trimmed_before_tokens: None,
            trimmed_after_tokens: None,
            compressed_ratio: None,
            early_stop_reason: None,
            coalesced: true,
        },
        NexusEvent::WindowAffinityApplied {
            metadata: EventMetadata::new("test"),
            route_key: "s".into(),
            folded: true,
            needs_chunking: true,
            tier: "s".into(),
        },
        NexusEvent::CacheAffinityApplied {
            metadata: EventMetadata::new("test"),
            route_key: "s".into(),
            strategy: "s".into(),
            cache_control_injected: true,
            breakpoint_count: 1,
        },
        NexusEvent::ContextBudgetAllocated {
            metadata: EventMetadata::new("test"),
            budget_tokens: 1,
            tier: "s".into(),
            sparsity: 0.5,
        },
        NexusEvent::SemanticCacheHit {
            metadata: EventMetadata::new("test"),
            namespace: "s".into(),
            similarity: 0.5,
        },
        NexusEvent::GhostMemoryDetected {
            metadata: EventMetadata::new("test"),
            ghost_rate: 0.5,
            ghost_count: 1,
            total_recalls: 1,
            current_strategy: "s".into(),
        },
        NexusEvent::MemConStrategyAdjusted {
            metadata: EventMetadata::new("test"),
            from_strategy: "s".into(),
            to_strategy: "s".into(),
            reason: "s".into(),
            ghost_rate: None,
        },
        NexusEvent::BenchmarkMetricsCollected {
            metadata: EventMetadata::new("test"),
            equivalent_input_cost_micro: 1,
            vendor_cache_hit_rate_percent: 1,
            semantic_cache_hit_rate_percent: 1,
            ttft_p95_ms: 1,
            total_output_tokens: 1,
            task_success_rate_percent: 1,
            per_vendor_snapshot_json: "s".into(),
        },
        NexusEvent::HcwRecallReported {
            metadata: EventMetadata::new("test"),
            tier: "s".into(),
            needle_recall_at_8: 0.5,
            position_bias: 0.5,
            chain_success_rate: 0.5,
            selected_count: 1,
        },
        NexusEvent::HcwRecallDegraded {
            metadata: EventMetadata::new("test"),
            tier: "s".into(),
            recall_rate: 0.5,
            baseline_recall: 0.5,
            reason: "s".into(),
        },
        NexusEvent::OverWindowFallbackTriggered {
            metadata: EventMetadata::new("test"),
            corpus_tokens: 1,
            effective_window: 1,
            candidate_count: 1,
            loaded_count: 1,
        },
        NexusEvent::ResourceRecovered {
            metadata: EventMetadata::new("test"),
            resource_type: "s".into(),
        },
        NexusEvent::FormalViolation {
            metadata: EventMetadata::new("test"),
            contract_id: "s".into(),
            target_type: "s".into(),
            violations: vec![],
            context: ContractContext::Runtime,
        },
        NexusEvent::RewardSignalReported {
            metadata: EventMetadata::new("test"),
            signal: RewardSignal {
                spec_id: "spec".into(),
                raw_reward: 0.0,
                weighted_reward: 0.0,
                timestamp_ms: 0,
                is_security_observation: false,
            },
        },
        NexusEvent::StopRulingIssued {
            metadata: EventMetadata::new("test"),
            quest_id: "s".into(),
            reason: "s".into(),
            preserve_best: true,
        },
        NexusEvent::VariantApproved {
            metadata: EventMetadata::new("test"),
            variant_id: "s".into(),
            score: 0.5,
        },
        NexusEvent::ParentSelected {
            metadata: EventMetadata::new("test"),
            task_id: "s".into(),
            parent_node_id: "s".into(),
            quality: 0.5,
            progress: 0.5,
            novelty: 0.5,
        },
        NexusEvent::ErrorSignatureMatched {
            metadata: EventMetadata::new("test"),
            error_hash: "s".into(),
            matched_card_ids: vec![],
        },
        NexusEvent::TokenLedgerRecorded {
            metadata: EventMetadata::new("test"),
            evidence_id: "s".into(),
            token_usage: 1,
        },
        NexusEvent::AssessmentUpdated {
            metadata: EventMetadata::new("test"),
            overall_score: 0.5,
            dimensions: vec![],
        },
        NexusEvent::BusThroughputReported {
            metadata: EventMetadata::new("test"),
            published_total: 1,
            events_per_sec: 0.5,
            window_secs: 1,
        },
        NexusEvent::SecurityInterceptionReported {
            metadata: EventMetadata::new("test"),
            total_requests: 1,
            blocked_requests: 1,
            interception_rate: 0.5,
        },
    ]
}

/// 变体序数表(enum 声明序,1..=145) — 编译期穷举锁
///
/// WHY 无通配符: 新增变体不显式登记即编译错误(第一道锁,先于一切运行时断言)。
#[allow(dead_code)]
fn variant_ordinals(event: &NexusEvent) -> u32 {
    match event {
        NexusEvent::UserIntentEncoded { .. } => 1,
        NexusEvent::NexusStateChanged { .. } => 2,
        NexusEvent::ModelRouteSelected { .. } => 3,
        NexusEvent::QuestCreated { .. } => 4,
        NexusEvent::QuestProgressUpdated { .. } => 5,
        NexusEvent::QuestListUpdated { .. } => 6,
        NexusEvent::QuestCompleted { .. } => 7,
        NexusEvent::ThinkingModeSwitched { .. } => 8,
        NexusEvent::CheckpointSaved { .. } => 9,
        NexusEvent::CheckpointLoaded { .. } => 10,
        NexusEvent::ConsensusReached { .. } => 11,
        NexusEvent::VoteCast { .. } => 12,
        NexusEvent::CapabilityFrozen { .. } => 13,
        NexusEvent::ShadowBreakerTripped { .. } => 14,
        NexusEvent::BudgetExceeded { .. } => 15,
        NexusEvent::SandboxViolation { .. } => 16,
        NexusEvent::OperationProduced { .. } => 17,
        NexusEvent::PredictionVerified { .. } => 18,
        NexusEvent::OmniSparseMasksComputed { .. } => 19,
        NexusEvent::ToolsRouted { .. } => 20,
        NexusEvent::ExecutionCompleted { .. } => 21,
        NexusEvent::MemoryMetricsReported { .. } => 22,
        NexusEvent::MemoryTiered { .. } => 23,
        NexusEvent::CacheHit { .. } => 24,
        NexusEvent::CacheMiss { .. } => 25,
        NexusEvent::WikiUpdated { .. } => 26,
        NexusEvent::EvolutionTriggered { .. } => 27,
        NexusEvent::DpoPairGenerated { .. } => 28,
        NexusEvent::AuditLogged { .. } => 29,
        NexusEvent::McpMessageReceived { .. } => 30,
        NexusEvent::SlowConsumerDropped { .. } => 31,
        NexusEvent::ContextWindowSwitched { .. } => 32,
        NexusEvent::ContextCompressed { .. } => 33,
        NexusEvent::CapabilityTiered { .. } => 34,
        NexusEvent::CapabilityTierStatsReported { .. } => 35,
        NexusEvent::BlocksRebalanced { .. } => 36,
        NexusEvent::ExpertActivated { .. } => 37,
        NexusEvent::ActivationThresholdAdjusted { .. } => 38,
        NexusEvent::ActivationCacheStats { .. } => 39,
        NexusEvent::GatherCompleted { .. } => 40,
        NexusEvent::OperationTimedOut { .. } => 41,
        NexusEvent::GatherTimedOut { .. } => 42,
        NexusEvent::OrphanCallDetected { .. } => 43,
        NexusEvent::ProducerStrategyAdjusted { .. } => 44,
        NexusEvent::PredictionMade { .. } => 45,
        NexusEvent::PredictionStatsReported { .. } => 46,
        NexusEvent::PredictionRolledBack { .. } => 47,
        NexusEvent::CachePrefetched { .. } => 48,
        NexusEvent::CacheStatsReported { .. } => 49,
        NexusEvent::ExpertRouted { .. } => 50,
        NexusEvent::EntropyBalanced { .. } => 51,
        NexusEvent::ExpertRegistered { .. } => 52,
        NexusEvent::ExpertUnregistered { .. } => 53,
        NexusEvent::DebateStarted { .. } => 54,
        NexusEvent::SkepticVeto { .. } => 55,
        NexusEvent::VetoOverridden { .. } => 56,
        NexusEvent::RedTeamAudit { .. } => 57,
        NexusEvent::BudgetAdjusted { .. } => 58,
        NexusEvent::AsaIntervention { .. } => 59,
        NexusEvent::AhirtProbeCompleted { .. } => 60,
        NexusEvent::RoleRegistered { .. } => 61,
        NexusEvent::BudgetStatsReported { .. } => 62,
        NexusEvent::BudgetMetricsUpdated { .. } => 63,
        NexusEvent::NmcEncoded { .. } => 64,
        NexusEvent::ChtcToolCallReceived { .. } => 65,
        NexusEvent::SsraFusionCompleted { .. } => 66,
        NexusEvent::GsoePolicyUpdated { .. } => 67,
        NexusEvent::LsctTierSwitched { .. } => 68,
        NexusEvent::McpMeshTransactionCompleted { .. } => 69,
        NexusEvent::CsnSubstitutionTriggered { .. } => 70,
        NexusEvent::SesaActivationCompleted { .. } => 71,
        NexusEvent::EfficiencyAlertTriggered { .. } => 72,
        NexusEvent::QuestPauseRequested { .. } => 73,
        NexusEvent::QuestResumeRequested { .. } => 74,
        NexusEvent::VoteCastRequested { .. } => 75,
        NexusEvent::RefreshStateRequested { .. } => 76,
        NexusEvent::QuestPaused { .. } => 77,
        NexusEvent::QuestResumed { .. } => 78,
        NexusEvent::QuestCancelRequested { .. } => 79,
        NexusEvent::QuestCancelled { .. } => 80,
        NexusEvent::QuestPriorityChanged { .. } => 81,
        NexusEvent::QuestPriorityAdjusted { .. } => 82,
        NexusEvent::DecayMetricsReported { .. } => 83,
        NexusEvent::RouterStatsReported { .. } => 84,
        NexusEvent::McpNodeHeartbeat { .. } => 85,
        NexusEvent::ChtcAdapterStatus { .. } => 86,
        NexusEvent::ClvSnapshotReported { .. } => 87,
        NexusEvent::AgentTaskDelegated { .. } => 88,
        NexusEvent::AgentTaskCompleted { .. } => 89,
        NexusEvent::AgentTaskFailed { .. } => 90,
        NexusEvent::AgentConsultRequested { .. } => 91,
        NexusEvent::AgentConsultResponded { .. } => 92,
        NexusEvent::AgentHeartbeat { .. } => 93,
        NexusEvent::AgentContextOverflow { .. } => 94,
        NexusEvent::TuiActionRequested { .. } => 95,
        NexusEvent::TuiActionProgressed { .. } => 96,
        NexusEvent::TuiActionCompleted { .. } => 97,
        NexusEvent::TuiActionFailed { .. } => 98,
        NexusEvent::TuiChatSubmitted { .. } => 99,
        NexusEvent::TuiChatResponseChunk { .. } => 100,
        NexusEvent::TuiChatCompleted { .. } => 101,
        NexusEvent::TuiChatStatusChanged { .. } => 102,
        NexusEvent::TuiChatHistoryReplaced { .. } => 103,
        NexusEvent::TuiHello { .. } => 104,
        NexusEvent::TuiHelloAck { .. } => 105,
        NexusEvent::R1ShadowRegressionDetected { .. } => 106,
        NexusEvent::R1ShadowPromotionReady { .. } => 107,
        NexusEvent::R1ShadowRollbackFailed { .. } => 108,
        NexusEvent::SpecRegistered { .. } => 109,
        NexusEvent::R2FreezeViolation { .. } => 110,
        NexusEvent::R2FreezeRollbackFailed { .. } => 111,
        NexusEvent::CoordinationRatioReported { .. } => 112,
        NexusEvent::AuditFindingRaised { .. } => 113,
        NexusEvent::HarnessReportGenerated { .. } => 114,
        NexusEvent::DebateCompleted { .. } => 115,
        NexusEvent::DelegationCompleted { .. } => 116,
        NexusEvent::ParliamentStrategyCapChanged { .. } => 117,
        NexusEvent::ModelAffinitySelected { .. } => 118,
        NexusEvent::CrossVendorNegotiation { .. } => 119,
        NexusEvent::ProviderDegraded { .. } => 120,
        NexusEvent::AffinityCapabilityNegotiated { .. } => 121,
        NexusEvent::AffinityQuotaExhausted { .. } => 122,
        NexusEvent::AffinityUnknownField { .. } => 123,
        NexusEvent::StreamSessionCompleted { .. } => 124,
        NexusEvent::WindowAffinityApplied { .. } => 125,
        NexusEvent::CacheAffinityApplied { .. } => 126,
        NexusEvent::ContextBudgetAllocated { .. } => 127,
        NexusEvent::SemanticCacheHit { .. } => 128,
        NexusEvent::GhostMemoryDetected { .. } => 129,
        NexusEvent::MemConStrategyAdjusted { .. } => 130,
        NexusEvent::BenchmarkMetricsCollected { .. } => 131,
        NexusEvent::HcwRecallReported { .. } => 132,
        NexusEvent::HcwRecallDegraded { .. } => 133,
        NexusEvent::OverWindowFallbackTriggered { .. } => 134,
        NexusEvent::ResourceRecovered { .. } => 135,
        NexusEvent::FormalViolation { .. } => 136,
        NexusEvent::RewardSignalReported { .. } => 137,
        NexusEvent::StopRulingIssued { .. } => 138,
        NexusEvent::VariantApproved { .. } => 139,
        NexusEvent::ParentSelected { .. } => 140,
        NexusEvent::ErrorSignatureMatched { .. } => 141,
        NexusEvent::TokenLedgerRecorded { .. } => 142,
        NexusEvent::AssessmentUpdated { .. } => 143,
        NexusEvent::BusThroughputReported { .. } => 144,
        NexusEvent::SecurityInterceptionReported { .. } => 145,
    }
}

#[test]
fn test_variant_count_lock() {
    let all = all_variant_samples();
    assert_eq!(
        all.len(),
        VARIANT_COUNT_LOCK,
        "NexusEvent 变体实际数量与锁定值不符: 清单需同步登记 enum 新变体"
    );
    // 序数表与样本清单交叉锁: 145 个样本必须映射到 145 个不同序数
    // (清单漏登变体 → 样本数不足;序数表漏登 → 编译错误;双重登记 → 序数重复)
    let ordinals: HashSet<u32> = all.iter().map(variant_ordinals).collect();
    assert_eq!(
        ordinals.len(),
        VARIANT_COUNT_LOCK,
        "variant_ordinals 与 all_variant_samples 必须一一对应"
    );
    // type_name 唯一性: 每个样本的类型名互不重复(防构造器复制粘贴错位)
    let names: HashSet<&str> = all.iter().map(|e| e.type_name()).collect();
    assert_eq!(
        names.len(),
        VARIANT_COUNT_LOCK,
        "type_name() 出现重复,样本构造器与变体未一一对应"
    );
}

#[test]
fn test_severity_distribution_lock() {
    let all = all_variant_samples();
    let critical = all
        .iter()
        .filter(|e| e.severity() == EventSeverity::Critical)
        .count();
    let info = all
        .iter()
        .filter(|e| e.severity() == EventSeverity::Info)
        .count();
    let normal = all
        .iter()
        .filter(|e| e.severity() == EventSeverity::Normal)
        .count();
    assert_eq!(critical, CRITICAL_LOCK, "Critical 变体数漂移");
    assert_eq!(info, INFO_LOCK, "Info 变体数漂移");
    assert_eq!(normal, NORMAL_LOCK, "Normal 变体数漂移");
    assert_eq!(
        critical + info + normal,
        VARIANT_COUNT_LOCK,
        "severity() 必须覆盖全部变体且每变体恰好一级"
    );
}

#[test]
fn test_critical_set_matches_lane_forbidden_shard() {
    // 与 bus.rs LANE_FORBIDDEN_SHARD(17 个 Critical 变体名)双向互锁:
    // severity() 的 Critical 集合 == 分片禁区名单,任何一侧漂移即失败
    let critical_names: HashSet<&str> = all_variant_samples()
        .iter()
        .filter(|e| e.severity() == EventSeverity::Critical)
        .map(|e| e.type_name())
        .collect();
    let forbidden: HashSet<&str> = LANE_FORBIDDEN_SHARD.iter().copied().collect();
    assert_eq!(
        critical_names, forbidden,
        "severity() Critical 清单必须与 bus.rs LANE_FORBIDDEN_SHARD 一一对应"
    );
}

#[test]
fn test_topic_defined_for_all_variants() {
    let all_topics = EventTopic::all();
    for event in all_variant_samples() {
        assert!(
            all_topics.contains(&event.topic()),
            "{} 的 topic() 不在 EventTopic::all() 内",
            event.type_name()
        );
    }
}
