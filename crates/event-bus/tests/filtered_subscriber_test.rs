//! Phase IV Task C1: EventTopic + FilteredSubscriber 集成测试
//!
//! 验证 FilteredSubscriber 仅接收订阅 topic 的事件，
//! 既有 subscribe() 保持全量广播向后兼容。
//!
//! 对应架构层：L1 Core（event-bus）
//! 设计决策（2026-07-09）：9 类 EventTopic 分类，架构纯净度优先

use event_bus::{EventBus, EventMetadata, EventTopic, NexusEvent};
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
// 测试 5：遍历全部 66 个 NexusEvent 变体，验证 topic() 返回有效 EventTopic
//
// WHY 此测试：Rust match 的穷尽性保证编译期覆盖所有变体，但运行时仍需
// 验证每个变体实例化后调用 topic() 不 panic 且返回 all() 集合内的值。
// 此测试同时是"66 变体映射完整性"的守护测试。
// ============================================================

#[test]
fn test_topic_mapping_covers_all_variants() {
    let all_topics = EventTopic::all();

    // 构造全部 66 个变体的实例，逐个验证 topic() 返回有效值
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
    ];

    // 守护：变体总数必须等于 66（与 NexusEvent 当前定义一致）
    assert_eq!(
        variants.len(),
        66,
        "测试构造的变体数应为 66（与 NexusEvent 当前定义一致）"
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
