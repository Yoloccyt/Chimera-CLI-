//! Phase IV Task C1: EventTopic + FilteredSubscriber 集成测试
//!
//! 验证 FilteredSubscriber 仅接收订阅 topic 的事件，
//! 既有 subscribe() 保持全量广播向后兼容。
//!
//! 对应架构层：L1 Core（event-bus）
//! 设计决策（2026-07-09）：9 类 EventTopic 分类，架构纯净度优先
//!
//! Task 5 扩展(CHIMERA-MAS,ADR-026):新增 EventTopic::Agent 主题测试,
//! 覆盖 7 个 Agent 协作变体 + recv_matching 谓词选择性订阅验证。

#![forbid(unsafe_code)]

use event_bus::{
    AgentStatus, ConsultUrgency, EventBus, EventMetadata, EventSeverity, EventTopic, NexusEvent,
    TaskPriority,
};
use std::collections::HashSet;

// ============================================================
// 辅助函数：构造各类 topic 的代表性事件
// ============================================================

/// 构造一个 Routing topic 事件（ExpertRegistered）
fn routing_event() -> NexusEvent {
    NexusEvent::ExpertRegistered {
        metadata: EventMetadata::new("test-source"),
        tool_id: "tool-1".into(),
    }
}

/// 构造一个 Memory topic 事件（NmcEncoded）
fn memory_event() -> NexusEvent {
    NexusEvent::NmcEncoded {
        metadata: EventMetadata::new("test-source"),
        modality: "Text".into(),
        content_hash: "abc123".into(),
        clv_dimension: 512,
    }
}

/// 构造一个 Security topic 事件（SkepticVeto）
fn security_event() -> NexusEvent {
    NexusEvent::SkepticVeto {
        metadata: EventMetadata::new("test-source"),
        quest_id: "q-1".into(),
        veto_reason: "test".into(),
        frozen_capabilities: vec![],
    }
}

/// 构造一个 Parliament topic 事件（VoteCast）
fn parliament_event() -> NexusEvent {
    NexusEvent::VoteCast {
        metadata: EventMetadata::new("test-source"),
        proposal_id: "p-1".into(),
        voter: "v-1".into(),
        vote: true,
    }
}

/// 构造一个 Execution topic 事件（PredictionMade）
fn execution_event() -> NexusEvent {
    NexusEvent::PredictionMade {
        metadata: EventMetadata::new("test-source"),
        quest_id: "q-1".into(),
        n: 3,
        avg_confidence: 0.85,
    }
}

/// 获取一个 `DateTime<Utc>` 值用于 AgentTaskDelegated 的 deadline 字段
///
/// WHY 不直接 `use chrono::Utc`：`event-bus` 的 dev-dependencies 未显式声明 chrono,
/// 通过 `EventMetadata::timestamp` 间接获取,与 agent_events_test.rs 保持一致。
fn test_deadline() -> chrono::DateTime<chrono::Utc> {
    EventMetadata::new("test").timestamp
}

// ============================================================
// 测试 1：FilteredSubscriber 订阅单 topic，仅接收匹配事件
// ============================================================

#[tokio::test]
async fn test_filtered_subscriber_only_receives_topic_events() {
    let bus = EventBus::new();

    // 订阅 Routing topic（单 topic 集合）
    let topics: HashSet<EventTopic> = [EventTopic::Routing].into_iter().collect();
    let mut rx = bus.subscribe_filtered(topics);

    // 发布一个 Routing 事件 + 一个 Memory 事件
    let routing = routing_event();
    let memory = memory_event();
    bus.publish(routing.clone()).await.unwrap();
    bus.publish(memory.clone()).await.unwrap();

    // 应只收到 Routing 事件（Memory 事件被消费丢弃）
    let received = rx.recv().await.unwrap();
    assert_eq!(received, routing);

    // 再 recv 应阻塞（无更多匹配事件），用 try_recv 验证缓冲区为空
    assert!(rx.try_recv().unwrap().is_none());
}

// ============================================================
// 测试 2：FilteredSubscriber 订阅多 topic，仅接收匹配集合事件
// ============================================================

#[tokio::test]
async fn test_filtered_subscriber_multiple_topics() {
    let bus = EventBus::new();

    // 订阅 {Security, Parliament} 两个 topic
    let topics: HashSet<EventTopic> = [EventTopic::Security, EventTopic::Parliament]
        .into_iter()
        .collect();
    let mut rx = bus.subscribe_filtered(topics);

    // 发布 4 类事件：Security、Parliament、Routing、Memory
    let security = security_event();
    let parliament = parliament_event();
    let routing = routing_event();
    let memory = memory_event();

    bus.publish(routing.clone()).await.unwrap(); // 不匹配，被消费丢弃
    bus.publish(security.clone()).await.unwrap(); // 匹配
    bus.publish(memory.clone()).await.unwrap(); // 不匹配，被消费丢弃
    bus.publish(parliament.clone()).await.unwrap(); // 匹配

    // 按发布顺序接收（broadcast FIFO），跳过不匹配事件
    let first = rx.recv().await.unwrap();
    let second = rx.recv().await.unwrap();

    assert_eq!(first, security);
    assert_eq!(second, parliament);

    // 缓冲区应已空
    assert!(rx.try_recv().unwrap().is_none());
}

// ============================================================
// 测试 3：FilteredSubscriber 订阅全部 9 个 topic，收到所有事件
// ============================================================

#[tokio::test]
async fn test_filtered_subscriber_all_topics_receives_everything() {
    let bus = EventBus::new();

    // 订阅全部 9 个 topic
    let mut rx = bus.subscribe_filtered(EventTopic::all());

    // 发布三类事件
    let routing = routing_event();
    let security = security_event();
    let execution = execution_event();

    bus.publish(routing.clone()).await.unwrap();
    bus.publish(security.clone()).await.unwrap();
    bus.publish(execution.clone()).await.unwrap();

    // 全部应收到（无过滤）
    let r1 = rx.recv().await.unwrap();
    let r2 = rx.recv().await.unwrap();
    let r3 = rx.recv().await.unwrap();

    assert_eq!(r1, routing);
    assert_eq!(r2, security);
    assert_eq!(r3, execution);
}

// ============================================================
// 测试 4：既有 subscribe() 保持全量广播向后兼容
// ============================================================

#[tokio::test]
async fn test_subscribe_remains_backward_compatible() {
    let bus = EventBus::new();

    // 既有 subscribe() 返回 EventReceiver（全量广播）
    let mut rx = bus.subscribe();

    // 发布 Routing + Memory 事件
    let routing = routing_event();
    let memory = memory_event();
    bus.publish(routing.clone()).await.unwrap();
    bus.publish(memory.clone()).await.unwrap();

    // 应都收到（无过滤）
    let r1 = rx.recv().await.unwrap();
    let r2 = rx.recv().await.unwrap();

    assert_eq!(r1, routing);
    assert_eq!(r2, memory);
}

// ============================================================
// 测试 5：遍历全部 74 个 NexusEvent 变体，验证 topic() 返回有效 EventTopic
//
// WHY 此测试：Rust match 的穷尽性保证编译期覆盖所有变体，但运行时仍需
// 验证每个变体实例化后调用 topic() 不 panic 且返回 all() 集合内的值。
// 此测试同时是"74 变体映射完整性"的守护测试。
//
// Task 5 更新：原 67 变体 + 7 个 Agent 变体(ADR-026) = 74 变体。
// ============================================================

#[test]
fn test_topic_mapping_covers_all_variants() {
    let all_topics = EventTopic::all();

    // 构造全部 74 个变体的实例，逐个验证 topic() 返回有效值
    let variants: Vec<NexusEvent> = vec![
        // === Quest (7) ===
        NexusEvent::UserIntentEncoded {
            metadata: EventMetadata::new("test-source"),
            intent_id: "i-1".into(),
            raw_text: "test".into(),
            risk_level: 10,
        },
        NexusEvent::QuestCreated {
            metadata: EventMetadata::new("test-source"),
            quest_id: "q-1".into(),
            title: "t".into(),
            task_count: 1,
        },
        NexusEvent::QuestProgressUpdated {
            metadata: EventMetadata::new("test-source"),
            quest_id: "q-1".into(),
            completed: 1,
            total: 3,
        },
        NexusEvent::ThinkingModeSwitched {
            metadata: EventMetadata::new("test-source"),
            quest_id: "q-1".into(),
            from_mode: "fast".into(),
            to_mode: "deep".into(),
            reason: "test".into(),
        },
        NexusEvent::CheckpointSaved {
            metadata: EventMetadata::new("test-source"),
            quest_id: "q-1".into(),
            checkpoint_id: "c-1".into(),
            memory_snapshot_hash: "h".into(),
        },
        NexusEvent::CheckpointLoaded {
            metadata: EventMetadata::new("test-source"),
            quest_id: "q-1".into(),
            checkpoint_id: "c-1".into(),
        },
        NexusEvent::ModelRouteSelected {
            metadata: EventMetadata::new("test-source"),
            quest_id: "q-1".into(),
            model_id: "m-1".into(),
            route_reason: "test".into(),
        },
        // === Memory (7) ===
        NexusEvent::NexusStateChanged {
            metadata: EventMetadata::new("test-source"),
            state_hash: "h".into(),
            prev_hash: "p".into(),
        },
        NexusEvent::MemoryMetricsReported {
            metadata: EventMetadata::new("test-source"),
            hit_rate: 0.5,
            evictions: 1,
        },
        NexusEvent::MemoryTiered {
            metadata: EventMetadata::new("test-source"),
            tier: "Hot".into(),
            item_count: 1,
            memory_id: None,
        },
        NexusEvent::ContextWindowSwitched {
            metadata: EventMetadata::new("test-source"),
            from_tier: "L0".into(),
            to_tier: "L1".into(),
            reason: "test".into(),
        },
        NexusEvent::ContextCompressed {
            metadata: EventMetadata::new("test-source"),
            original_size: 100,
            compressed_size: 50,
            ratio: 0.5,
        },
        NexusEvent::CapabilityTiered {
            metadata: EventMetadata::new("test-source"),
            capability_id: "c-1".into(),
            from_tier: "Hot".into(),
            to_tier: "Warm".into(),
            reason: "test".into(),
        },
        NexusEvent::NmcEncoded {
            metadata: EventMetadata::new("test-source"),
            modality: "Text".into(),
            content_hash: "h".into(),
            clv_dimension: 512,
        },
        // === Security (8) ===
        NexusEvent::CapabilityFrozen {
            metadata: EventMetadata::new("test-source"),
            capability_id: "c-1".into(),
            reason: "test".into(),
        },
        NexusEvent::SandboxViolation {
            metadata: EventMetadata::new("test-source"),
            violation_type: "test".into(),
            detail: "test".into(),
        },
        NexusEvent::AuditLogged {
            metadata: EventMetadata::new("test-source"),
            audit_hash: "h".into(),
            severity: "Normal".into(),
        },
        NexusEvent::SkepticVeto {
            metadata: EventMetadata::new("test-source"),
            quest_id: "q-1".into(),
            veto_reason: "test".into(),
            frozen_capabilities: vec![],
        },
        NexusEvent::VetoOverridden {
            metadata: EventMetadata::new("test-source"),
            quest_id: "q-1".into(),
            proposal_id: "p-1".into(),
            veto_reason: "test".into(),
            override_reason: "test".into(),
            override_by: "admin".into(),
        },
        NexusEvent::RedTeamAudit {
            metadata: EventMetadata::new("test-source"),
            vulnerability_type: "test".into(),
            failed_probes: 1,
            total_probes: 10,
            detection_rate: 0.1,
            remediation_suggestion: "test".into(),
        },
        NexusEvent::AsaIntervention {
            metadata: EventMetadata::new("test-source"),
            operation_id: "o-1".into(),
            action: "Allow".into(),
            safety_score: 1.0,
            block_reason: None,
            alternative_suggestion: None,
        },
        NexusEvent::AhirtProbeCompleted {
            metadata: EventMetadata::new("test-source"),
            probe_type: "test".into(),
            total: 10,
            passed: 8,
            failed: 2,
            detection_rate: 0.2,
        },
        // === Execution (11) ===
        NexusEvent::OperationProduced {
            metadata: EventMetadata::new("test-source"),
            op_id: "o-1".into(),
            content_hash: "h".into(),
        },
        NexusEvent::PredictionVerified {
            metadata: EventMetadata::new("test-source"),
            op_id: "o-1".into(),
            score: 0.9,
        },
        NexusEvent::ExecutionCompleted {
            metadata: EventMetadata::new("test-source"),
            quest_id: "q-1".into(),
            result_hash: "h".into(),
        },
        NexusEvent::GatherCompleted {
            metadata: EventMetadata::new("test-source"),
            total: 10,
            succeeded: 8,
            failed: 2,
            latency_ms: 50.0,
        },
        NexusEvent::OperationTimedOut {
            metadata: EventMetadata::new("test-source"),
            operation_id: "o-1".into(),
            timeout_ms: 1000,
        },
        NexusEvent::GatherTimedOut {
            metadata: EventMetadata::new("test-source"),
            deadline_ms: 5000,
            elapsed_ms: 5012,
            total: 10,
            abandoned: 3,
        },
        NexusEvent::OrphanCallDetected {
            metadata: EventMetadata::new("test-source"),
            operation_id: "o-1".into(),
            spawn_location: "test.rs:1".into(),
        },
        NexusEvent::ProducerStrategyAdjusted {
            metadata: EventMetadata::new("test-source"),
            adjustment_reason: "test".into(),
            new_strategy: "s".into(),
        },
        NexusEvent::PredictionMade {
            metadata: EventMetadata::new("test-source"),
            quest_id: "q-1".into(),
            n: 3,
            avg_confidence: 0.85,
        },
        NexusEvent::PredictionStatsReported {
            metadata: EventMetadata::new("test-source"),
            success_rate_by_n: std::collections::HashMap::new(),
        },
        NexusEvent::PredictionRolledBack {
            metadata: EventMetadata::new("test-source"),
            failed_step: 2,
            rollback_to: 1,
        },
        NexusEvent::SsraFusionCompleted {
            metadata: EventMetadata::new("test-source"),
            quest_id: "q-1".into(),
            fused_template_id: "t-1".into(),
            latency_ms: 100,
            confidence: 0.9,
        },
        // === Parliament (7) ===
        NexusEvent::ConsensusReached {
            metadata: EventMetadata::new("test-source"),
            quest_id: "q-1".into(),
            decision_hash: "h".into(),
            dpo_pair_id: None,
        },
        NexusEvent::VoteCast {
            metadata: EventMetadata::new("test-source"),
            proposal_id: "p-1".into(),
            voter: "v-1".into(),
            vote: true,
        },
        NexusEvent::DebateStarted {
            metadata: EventMetadata::new("test-source"),
            quest_id: "q-1".into(),
            proposal_id: "p-1".into(),
            participant_count: 5,
        },
        NexusEvent::RoleRegistered {
            metadata: EventMetadata::new("test-source"),
            role_id: "r-1".into(),
            role_name: "Visionary".into(),
            voting_weight: 0.4,
        },
        NexusEvent::BudgetAdjusted {
            metadata: EventMetadata::new("test-source"),
            quest_id: "q-1".into(),
            old_tier: "High".into(),
            new_tier: "Medium".into(),
            coefficient: 0.5,
            reason: "test".into(),
        },
        NexusEvent::BudgetStatsReported {
            metadata: EventMetadata::new("test-source"),
            total_consumption: 5000.0,
            remaining_budget: 5000.0,
            utilization_rate: 0.5,
        },
        NexusEvent::BudgetExceeded {
            metadata: EventMetadata::new("test-source"),
            budget_type: "token".into(),
            current: 10000,
            limit: 8000,
        },
        // === Routing (11) ===
        NexusEvent::OmniSparseMasksComputed {
            metadata: EventMetadata::new("test-source"),
            mask_hash: "h".into(),
            sparsity: 0.6,
            context_mask: vec![],
        },
        NexusEvent::ToolsRouted {
            metadata: EventMetadata::new("test-source"),
            routed_count: 5,
            top_tool: "t-1".into(),
            routed_tools: vec![],
        },
        NexusEvent::BlocksRebalanced {
            metadata: EventMetadata::new("test-source"),
            old_block_count: 10,
            new_block_count: 12,
        },
        NexusEvent::ExpertActivated {
            metadata: EventMetadata::new("test-source"),
            activated_experts: vec![],
            suppressed_experts: vec![],
            top_gate_value: 0.85,
        },
        NexusEvent::ActivationThresholdAdjusted {
            metadata: EventMetadata::new("test-source"),
            old_threshold: 0.5,
            new_threshold: 0.6,
            load_factor: 0.7,
        },
        NexusEvent::ActivationCacheStats {
            metadata: EventMetadata::new("test-source"),
            hit_rate: 0.8,
            entry_count: 100,
        },
        NexusEvent::ExpertRouted {
            metadata: EventMetadata::new("test-source"),
            routed_tool: "t-1".into(),
            confidence: 0.9,
        },
        NexusEvent::ExpertRegistered {
            metadata: EventMetadata::new("test-source"),
            tool_id: "t-1".into(),
        },
        NexusEvent::ExpertUnregistered {
            metadata: EventMetadata::new("test-source"),
            tool_id: "t-1".into(),
        },
        NexusEvent::EntropyBalanced {
            metadata: EventMetadata::new("test-source"),
            old_entropy: 0.5,
            new_entropy: 0.6,
            redistributed_count: 3,
        },
        NexusEvent::SesaActivationCompleted {
            metadata: EventMetadata::new("test-source"),
            total_experts: 100,
            active_experts: 40,
            sparsity_ratio: 0.4,
            latency_us: 500,
        },
        // === System (6) ===
        NexusEvent::McpMessageReceived {
            metadata: EventMetadata::new("test-source"),
            source_node: "n-1".into(),
            msg_type: "test".into(),
        },
        NexusEvent::SlowConsumerDropped {
            metadata: EventMetadata::new("test-source"),
            subscriber_id: "s-1".into(),
            lag: 10,
            dropped_count: 5,
        },
        NexusEvent::ChtcToolCallReceived {
            metadata: EventMetadata::new("test-source"),
            call_id: "c-1".into(),
            tool_id: "t-1".into(),
            ide_source: "VSCode".into(),
            parameters_hash: "h".into(),
        },
        NexusEvent::McpMeshTransactionCompleted {
            metadata: EventMetadata::new("test-source"),
            transaction_id: "t-1".into(),
            participant_count: 3,
            latency_ms: 100,
            success: true,
        },
        NexusEvent::CsnSubstitutionTriggered {
            metadata: EventMetadata::new("test-source"),
            original_capability_id: "c-1".into(),
            substitute_id: "s-1".into(),
            similarity_score: 0.9,
            degradation_level: 0,
        },
        NexusEvent::EfficiencyAlertTriggered {
            metadata: EventMetadata::new("test-source"),
            rule_id: "r-1".into(),
            metric_name: "m".into(),
            triggered_value: 100.0,
            threshold: 80.0,
        },
        // === Knowledge (4) ===
        NexusEvent::WikiUpdated {
            metadata: EventMetadata::new("test-source"),
            wiki_hash: "h".into(),
            delta: 5,
        },
        NexusEvent::EvolutionTriggered {
            metadata: EventMetadata::new("test-source"),
            generation: 1,
            fitness: 0.5,
        },
        NexusEvent::DpoPairGenerated {
            metadata: EventMetadata::new("test-source"),
            pair_id: "p-1".into(),
            chosen: "c".into(),
            rejected: "r".into(),
        },
        NexusEvent::GsoePolicyUpdated {
            metadata: EventMetadata::new("test-source"),
            generation: 1,
            improvement: 0.05,
            new_mutation_rate: 0.1,
            new_selection_pressure: 0.5,
        },
        // === Storage (5) ===
        NexusEvent::CacheHit {
            metadata: EventMetadata::new("test-source"),
            cache_key: "k-1".into(),
        },
        NexusEvent::CacheMiss {
            metadata: EventMetadata::new("test-source"),
            cache_key: "k-1".into(),
        },
        NexusEvent::CachePrefetched {
            metadata: EventMetadata::new("test-source"),
            prefetched_ids: vec![],
        },
        NexusEvent::CacheStatsReported {
            metadata: EventMetadata::new("test-source"),
            hit_rate: 0.8,
            eviction_count: 5,
        },
        NexusEvent::LsctTierSwitched {
            metadata: EventMetadata::new("test-source"),
            capability_id: "c-1".into(),
            from_tier: "Warm".into(),
            to_tier: "Hot".into(),
            reason: "test".into(),
        },
        // === Agent (7) === Task 4 CHIMERA-MAS 多 Agent 协作(ADR-026)
        // 7 个变体均映射到 EventTopic::Agent,severity 仅 AgentTaskFailed 为 Critical
        NexusEvent::AgentTaskDelegated {
            metadata: EventMetadata::new("test-source"),
            from: "agent-1".into(),
            to: "agent-2".into(),
            task_id: "t-1".into(),
            deadline: test_deadline(),
            priority: TaskPriority::High,
        },
        NexusEvent::AgentTaskCompleted {
            metadata: EventMetadata::new("test-source"),
            from: "agent-1".into(),
            to: "agent-2".into(),
            task_id: "t-1".into(),
            result_summary: "done".into(),
        },
        NexusEvent::AgentTaskFailed {
            metadata: EventMetadata::new("test-source"),
            from: "agent-1".into(),
            to: "agent-2".into(),
            task_id: "t-1".into(),
            error: "timeout".into(),
            retry_count: 3,
        },
        NexusEvent::AgentConsultRequested {
            metadata: EventMetadata::new("test-source"),
            from: "agent-1".into(),
            to: "agent-2".into(),
            question: "how?".into(),
            context: "ctx".into(),
            urgency: ConsultUrgency::Medium,
        },
        NexusEvent::AgentConsultResponded {
            metadata: EventMetadata::new("test-source"),
            from: "agent-2".into(),
            to: "agent-1".into(),
            answer: "answer".into(),
            references: vec![],
        },
        NexusEvent::AgentHeartbeat {
            metadata: EventMetadata::new("test-source"),
            from: "agent-1".into(),
            status: AgentStatus::Running,
            current_task: Some("t-1".into()),
            token_usage: 4096,
            memory_usage_mb: 128,
        },
        NexusEvent::AgentContextOverflow {
            metadata: EventMetadata::new("test-source"),
            agent_id: "agent-1".into(),
            current_tokens: 131072,
            max_tokens: 131072,
        },
    ];

    // 守护：变体总数必须等于 74（67 原有 + 7 个 Agent 变体,与 NexusEvent 当前定义一致）
    assert_eq!(
        variants.len(),
        74,
        "测试构造的变体数应为 74（67 原有 + 7 个 Agent 变体,与 NexusEvent 当前定义一致）"
    );

    // 遍历每个变体，验证 topic() 返回值在 all_topics 集合内（无 panic）
    for event in &variants {
        let topic = event.topic();
        assert!(
            all_topics.contains(&topic),
            "变体 {:?} 的 topic() 返回 {:?} 不在 all() 集合内",
            event.type_name(),
            topic
        );
    }
}

// ============================================================
// 测试 6：订阅 EventTopic::Agent,验证接收全部 7 个 Agent 变体
//
// WHY 此测试:FilteredSubscriber 基于 topic 集合订阅,验证 7 个 Agent 变体
// 均映射到 EventTopic::Agent,且通过 subscribe_filtered 可全部接收。
// ============================================================

#[tokio::test]
async fn test_subscribe_agent_topic_receives_all_7_variants() {
    let bus = EventBus::new();

    // 订阅 EventTopic::Agent(单 topic 集合)
    let topics: HashSet<EventTopic> = [EventTopic::Agent].into_iter().collect();
    let mut rx = bus.subscribe_filtered(topics);

    // 构造 7 个 Agent 变体
    let delegated = NexusEvent::AgentTaskDelegated {
        metadata: EventMetadata::new("chimera-mas"),
        from: "agent-001".into(),
        to: "agent-002".into(),
        task_id: "task-001".into(),
        deadline: test_deadline(),
        priority: TaskPriority::High,
    };
    let completed = NexusEvent::AgentTaskCompleted {
        metadata: EventMetadata::new("chimera-mas"),
        from: "agent-001".into(),
        to: "agent-002".into(),
        task_id: "task-001".into(),
        result_summary: "已完成代码审查".into(),
    };
    let failed = NexusEvent::AgentTaskFailed {
        metadata: EventMetadata::new("chimera-mas"),
        from: "agent-001".into(),
        to: "agent-002".into(),
        task_id: "task-001".into(),
        error: "工具调用超时".into(),
        retry_count: 3,
    };
    let consult_req = NexusEvent::AgentConsultRequested {
        metadata: EventMetadata::new("chimera-mas"),
        from: "agent-001".into(),
        to: "agent-002".into(),
        question: "如何处理 UTF-8?".into(),
        context: "用户输入含 emoji".into(),
        urgency: ConsultUrgency::Medium,
    };
    let consult_resp = NexusEvent::AgentConsultResponded {
        metadata: EventMetadata::new("chimera-mas"),
        from: "agent-002".into(),
        to: "agent-001".into(),
        answer: "使用 graphemes()".into(),
        references: vec!["https://unicode.org/reports/tr29/".into()],
    };
    let heartbeat = NexusEvent::AgentHeartbeat {
        metadata: EventMetadata::new("chimera-mas"),
        from: "agent-001".into(),
        status: AgentStatus::Running,
        current_task: Some("task-001".into()),
        token_usage: 4096,
        memory_usage_mb: 128,
    };
    let overflow = NexusEvent::AgentContextOverflow {
        metadata: EventMetadata::new("chimera-mas"),
        agent_id: "agent-001".into(),
        current_tokens: 131072,
        max_tokens: 131072,
    };

    // 按顺序发布 7 个 Agent 变体
    bus.publish(delegated.clone()).await.unwrap();
    bus.publish(completed.clone()).await.unwrap();
    bus.publish(failed.clone()).await.unwrap();
    bus.publish(consult_req.clone()).await.unwrap();
    bus.publish(consult_resp.clone()).await.unwrap();
    bus.publish(heartbeat.clone()).await.unwrap();
    bus.publish(overflow.clone()).await.unwrap();

    // 全部应收到(broadcast FIFO 顺序)
    let r1 = rx.recv().await.unwrap();
    let r2 = rx.recv().await.unwrap();
    let r3 = rx.recv().await.unwrap();
    let r4 = rx.recv().await.unwrap();
    let r5 = rx.recv().await.unwrap();
    let r6 = rx.recv().await.unwrap();
    let r7 = rx.recv().await.unwrap();

    assert_eq!(r1, delegated);
    assert_eq!(r2, completed);
    assert_eq!(r3, failed);
    assert_eq!(r4, consult_req);
    assert_eq!(r5, consult_resp);
    assert_eq!(r6, heartbeat);
    assert_eq!(r7, overflow);

    // 缓冲区应已空
    assert!(rx.try_recv().unwrap().is_none());
}

// ============================================================
// 测试 7：AgentTaskFailed severity 必须为 Critical
//
// WHY 此测试:任务失败可能影响 Quest 完整性,必须保证投递到 SecCore 与
// Parliament 进行补救决策。severity() 显式分支返回 Critical,此测试
// 守护该分支防止未来重构时被通配符误判为 Normal。
// ============================================================

#[test]
fn test_agent_task_failed_has_critical_severity() {
    let event = NexusEvent::AgentTaskFailed {
        metadata: EventMetadata::new("chimera-mas"),
        from: "agent-001".into(),
        to: "agent-002".into(),
        task_id: "task-001".into(),
        error: "工具调用超时(30s 未响应)".into(),
        retry_count: 3,
    };
    assert_eq!(
        event.severity(),
        EventSeverity::Critical,
        "AgentTaskFailed 必须为 Critical(任务失败可能影响 Quest 完整性)"
    );
    assert_eq!(event.type_name(), "AgentTaskFailed");
}

// ============================================================
// 测试 8：订阅 EventTopic::Agent,发布混合事件(Agent + 非 Agent),验证只接收 Agent 事件
//
// WHY 此测试:验证 FilteredSubscriber 的 topic 过滤正确性 — 非 Agent 事件
// 应被消费丢弃,不占用 Agent 订阅者缓冲区。与测试 2(多 topic 过滤)互补,
// 此测试专注 Agent topic 的过滤行为。
// ============================================================

#[tokio::test]
async fn test_agent_events_filtered_by_topic() {
    let bus = EventBus::new();

    // 订阅 EventTopic::Agent
    let topics: HashSet<EventTopic> = [EventTopic::Agent].into_iter().collect();
    let mut rx = bus.subscribe_filtered(topics);

    // 发布混合事件:Agent + 非 Agent(Routing/Memory/Security)
    let routing = routing_event(); // 非 Agent — 应被消费丢弃
    let heartbeat = NexusEvent::AgentHeartbeat {
        metadata: EventMetadata::new("chimera-mas"),
        from: "agent-001".into(),
        status: AgentStatus::Running,
        current_task: None,
        token_usage: 0,
        memory_usage_mb: 32,
    };
    let memory = memory_event(); // 非 Agent — 应被消费丢弃
    let delegated = NexusEvent::AgentTaskDelegated {
        metadata: EventMetadata::new("chimera-mas"),
        from: "agent-001".into(),
        to: "agent-002".into(),
        task_id: "task-001".into(),
        deadline: test_deadline(),
        priority: TaskPriority::High,
    };
    let security = security_event(); // 非 Agent — 应被消费丢弃

    bus.publish(routing.clone()).await.unwrap(); // 不匹配,被消费
    bus.publish(heartbeat.clone()).await.unwrap(); // 匹配
    bus.publish(memory.clone()).await.unwrap(); // 不匹配,被消费
    bus.publish(delegated.clone()).await.unwrap(); // 匹配
    bus.publish(security.clone()).await.unwrap(); // 不匹配,被消费

    // 只收到 Agent 事件(按发布顺序)
    let first = rx.recv().await.unwrap();
    let second = rx.recv().await.unwrap();

    assert_eq!(first, heartbeat);
    assert_eq!(second, delegated);

    // 缓冲区应已空(3 个非 Agent 事件已被消费丢弃)
    assert!(rx.try_recv().unwrap().is_none());
}

// ============================================================
// 测试 9：recv_matching 谓词精确匹配 AgentTaskDelegated 变体
//
// WHY 此测试:验证 EventReceiver::recv_matching 谓词过滤能正确匹配
// AgentTaskDelegated 变体。与 FilteredSubscriber(topic 级过滤)互补,
// recv_matching 基于谓词的临时过滤,适合一次性精确匹配场景。
// ============================================================

#[tokio::test]
async fn test_recv_matching_agent_task_delegated() {
    let bus = EventBus::new();
    // 使用 subscribe()(非 subscribe_filtered)获取 EventReceiver,
    // 因为 recv_matching 是 EventReceiver 的方法,FilteredSubscriber 不直接暴露。
    let mut rx = bus.subscribe();

    // 发布混合 Agent 事件:heartbeat + delegated + completed
    let heartbeat = NexusEvent::AgentHeartbeat {
        metadata: EventMetadata::new("chimera-mas"),
        from: "agent-001".into(),
        status: AgentStatus::Idle,
        current_task: None,
        token_usage: 0,
        memory_usage_mb: 0,
    };
    let delegated = NexusEvent::AgentTaskDelegated {
        metadata: EventMetadata::new("chimera-mas"),
        from: "agent-001".into(),
        to: "agent-002".into(),
        task_id: "task-001".into(),
        deadline: test_deadline(),
        priority: TaskPriority::High,
    };
    let completed = NexusEvent::AgentTaskCompleted {
        metadata: EventMetadata::new("chimera-mas"),
        from: "agent-001".into(),
        to: "agent-002".into(),
        task_id: "task-001".into(),
        result_summary: "done".into(),
    };

    bus.publish(heartbeat.clone()).await.unwrap(); // 不匹配谓词,被消费
    bus.publish(delegated.clone()).await.unwrap(); // 匹配谓词
    bus.publish(completed.clone()).await.unwrap(); // 不匹配谓词,留在缓冲区

    // recv_matching 只接收 AgentTaskDelegated 变体
    let received = rx
        .recv_matching(|e| matches!(e, NexusEvent::AgentTaskDelegated { .. }))
        .await
        .unwrap();
    assert_eq!(received, delegated);

    // 后续 recv 应拿到 completed(heartbeat 已被 recv_matching 消费)
    let remaining = rx.recv().await.unwrap();
    assert_eq!(remaining, completed);
}

// ============================================================
// 测试 10：AgentHeartbeat 的 topic() 返回 EventTopic::Agent
//
// WHY 此测试:验证 AgentHeartbeat 变体正确映射到 EventTopic::Agent 主题。
// 此测试是 topic() 映射的单元级守护,与测试 5(全变体覆盖)互补 —
// 测试 5 验证全部变体 topic() 返回值在 all() 集合内,此测试显式断言
// AgentHeartbeat 返回 EventTopic::Agent(而非其他 topic)。
// ============================================================

#[test]
fn test_agent_heartbeat_topic_is_agent() {
    let event = NexusEvent::AgentHeartbeat {
        metadata: EventMetadata::new("chimera-mas"),
        from: "agent-001".into(),
        status: AgentStatus::Running,
        current_task: Some("task-001".into()),
        token_usage: 4096,
        memory_usage_mb: 128,
    };
    assert_eq!(
        event.topic(),
        EventTopic::Agent,
        "AgentHeartbeat 必须映射到 EventTopic::Agent"
    );
}
