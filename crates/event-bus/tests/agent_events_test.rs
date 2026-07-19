//! CHIMERA-MAS Agent 相关 NexusEvent 变体测试(TDD RED 阶段)
//!
//! 对应架构层:L1 Core(event-bus)
//! 对应 spec:`.trae/specs/add-chimera-mas-subsystem/`
//! 对应 ADR:ADR-026(CHIMERA-MAS 多 Agent 协作子系统)
//!
//! # TDD RED 阶段目的
//! 本测试文件验证 7 个新增 Agent 相关 NexusEvent 变体存在且字段正确。
//! Task 3(RED):测试应全部**编译失败**,因为 7 个变体尚未在 `types.rs` 实现,
//! 3 个辅助类型(TaskPriority/ConsultUrgency/AgentStatus)尚未导出,
//! EventTopic::Agent 变体尚未在 `topic.rs` 添加。
//! Task 4(GREEN):实现变体与辅助类型后,本测试应全部通过。
//!
//! # 7 个新增变体(规格)
//! | 变体名                    | severity | 字段                                                          |
//! |--------------------------|----------|---------------------------------------------------------------|
//! | AgentTaskDelegated       | Normal   | from, to, task_id, deadline, priority                         |
//! | AgentTaskCompleted       | Normal   | from, to, task_id, result_summary                             |
//! | AgentTaskFailed          | Critical | from, to, task_id, error, retry_count                         |
//! | AgentConsultRequested    | Normal   | from, to, question, context, urgency                          |
//! | AgentConsultResponded    | Normal   | from, to, answer, references                                 |
//! | AgentHeartbeat           | Normal   | from, status, current_task, token_usage, memory_usage_mb      |
//! | AgentContextOverflow     | Normal   | agent_id, current_tokens, max_tokens                          |
//!
//! # 辅助类型(Task 4 在 event-bus 或 chimera-mas 定义)
//! - `TaskPriority` enum:Low/Medium/High/Critical(任务优先级)
//! - `ConsultUrgency` enum:Low/Medium/High/Critical(咨询紧急度)
//! - `AgentStatus` enum:Idle/Running/Paused/Completed/Failed/Crashed(Agent 状态)
//!
//! # EventTopic 新增变体
//! - `EventTopic::Agent`:Agent 协作相关事件 topic,7 个新变体均映射到此 topic
//!
//! # 设计说明
//! - **变体字段无 metadata**:严格遵循 spec 规格,与现有 NexusEvent 变体(均含
//!   `metadata: EventMetadata` 字段)不同。Task 4 实现时若决定补充 metadata
//!   字段,需同步更新本测试文件的构造代码。
//! - **DateTime<Utc> 获取方式**:不直接 `use chrono::Utc`,改用
//!   `EventMetadata::new("test").timestamp` 间接获取,避免修改 dev-dependencies
//!   (Task 3 约束:只创建测试文件,不改 Cargo.toml)。这也让 RED 阶段失败
//!   原因更纯粹(仅因变体未定义,而非 chrono 缺失)。
//! - **辅助类型路径假设**:Task 4 实现时确定实际定义位置(event-bus 或
//!   chimera-mas),若非 `event_bus::TaskPriority` 等路径,需同步更新本测试
//!   import。

#![forbid(unsafe_code)]

use event_bus::{
    deserialize_msgpack, serialize_msgpack, AgentStatus, ConsultUrgency, EventMetadata,
    EventSeverity, EventTopic, NexusEvent, TaskPriority,
};

// ============================================================
// 辅助函数
// ============================================================

/// 构造测试用元数据(单参数 source,与 crate 公开签名一致)
fn test_metadata() -> EventMetadata {
    EventMetadata::new("chimera-mas")
}

/// 获取一个 `DateTime<Utc>` 值用于 deadline 字段
///
/// WHY 不直接 `use chrono::Utc`:`event-bus` 的 dev-dependencies 未声明 chrono,
/// 通过 `EventMetadata::timestamp` 字段间接获取 `DateTime<Utc>` 值,避免修改
/// Cargo.toml(Task 3 约束)。同时让 RED 阶段编译失败原因更纯粹——仅因变体
/// 未定义,而非 chrono 缺失。Task 4 实现时若需直接用 chrono,可在
/// dev-dependencies 中添加。
fn test_deadline() -> chrono::DateTime<chrono::Utc> {
    EventMetadata::new("test").timestamp
}

// ============================================================
// 变体 1: AgentTaskDelegated — 任务委派(from -> to)
// severity: Normal
// ============================================================

#[test]
fn test_agent_task_delegated_variant_exists() {
    let event = NexusEvent::AgentTaskDelegated {
        metadata: test_metadata(),
        from: "agent-001".to_string(),
        to: "agent-002".to_string(),
        task_id: "task-001".to_string(),
        deadline: test_deadline(),
        priority: TaskPriority::High,
    };
    // 验证元数据字段可访问(若 Task 4 添加 metadata 字段,此断言可能调整)
    let _ = event; // 构造成功即验证变体存在
}

#[test]
fn test_agent_task_delegated_topic() {
    let event = NexusEvent::AgentTaskDelegated {
        metadata: test_metadata(),
        from: "agent-001".to_string(),
        to: "agent-002".to_string(),
        task_id: "task-001".to_string(),
        deadline: test_deadline(),
        priority: TaskPriority::High,
    };
    assert_eq!(event.topic(), EventTopic::Agent);
}

#[test]
fn test_agent_task_delegated_severity() {
    let event = NexusEvent::AgentTaskDelegated {
        metadata: test_metadata(),
        from: "agent-001".to_string(),
        to: "agent-002".to_string(),
        task_id: "task-001".to_string(),
        deadline: test_deadline(),
        priority: TaskPriority::High,
    };
    assert_eq!(event.severity(), EventSeverity::Normal);
}

#[test]
fn test_agent_task_delegated_serialization() {
    let event = NexusEvent::AgentTaskDelegated {
        metadata: test_metadata(),
        from: "agent-001".to_string(),
        to: "agent-002".to_string(),
        task_id: "task-001".to_string(),
        deadline: test_deadline(),
        priority: TaskPriority::High,
    };
    let bytes = serialize_msgpack(&event).expect("MessagePack 序列化失败");
    let decoded: NexusEvent = deserialize_msgpack(&bytes).expect("MessagePack 反序列化失败");
    assert_eq!(event, decoded);
}

// ============================================================
// 变体 2: AgentTaskCompleted — 任务完成反馈
// severity: Normal
// ============================================================

#[test]
fn test_agent_task_completed_variant_exists() {
    let event = NexusEvent::AgentTaskCompleted {
        metadata: test_metadata(),
        from: "agent-001".to_string(),
        to: "agent-002".to_string(),
        task_id: "task-001".to_string(),
        result_summary: "已完成代码审查,3 处问题已修复".to_string(),
    };
    let _ = event; // 构造成功即验证变体存在
}

#[test]
fn test_agent_task_completed_topic() {
    let event = NexusEvent::AgentTaskCompleted {
        metadata: test_metadata(),
        from: "agent-001".to_string(),
        to: "agent-002".to_string(),
        task_id: "task-001".to_string(),
        result_summary: "已完成".to_string(),
    };
    assert_eq!(event.topic(), EventTopic::Agent);
}

#[test]
fn test_agent_task_completed_severity() {
    let event = NexusEvent::AgentTaskCompleted {
        metadata: test_metadata(),
        from: "agent-001".to_string(),
        to: "agent-002".to_string(),
        task_id: "task-001".to_string(),
        result_summary: "已完成".to_string(),
    };
    assert_eq!(event.severity(), EventSeverity::Normal);
}

#[test]
fn test_agent_task_completed_serialization() {
    let event = NexusEvent::AgentTaskCompleted {
        metadata: test_metadata(),
        from: "agent-001".to_string(),
        to: "agent-002".to_string(),
        task_id: "task-001".to_string(),
        result_summary: "代码审查完成,3 处问题已修复".to_string(),
    };
    let bytes = serialize_msgpack(&event).expect("MessagePack 序列化失败");
    let decoded: NexusEvent = deserialize_msgpack(&bytes).expect("MessagePack 反序列化失败");
    assert_eq!(event, decoded);
}

// ============================================================
// 变体 3: AgentTaskFailed — 任务失败(Critical)
// severity: Critical(任务失败可能影响 Quest 完整性,必须保证投递)
// ============================================================

#[test]
fn test_agent_task_failed_variant_exists() {
    let event = NexusEvent::AgentTaskFailed {
        metadata: test_metadata(),
        from: "agent-001".to_string(),
        to: "agent-002".to_string(),
        task_id: "task-001".to_string(),
        error: "工具调用超时".to_string(),
        retry_count: 3,
    };
    let _ = event; // 构造成功即验证变体存在
}

#[test]
fn test_agent_task_failed_topic() {
    let event = NexusEvent::AgentTaskFailed {
        metadata: test_metadata(),
        from: "agent-001".to_string(),
        to: "agent-002".to_string(),
        task_id: "task-001".to_string(),
        error: "超时".to_string(),
        retry_count: 1,
    };
    assert_eq!(event.topic(), EventTopic::Agent);
}

#[test]
fn test_agent_task_failed_severity() {
    let event = NexusEvent::AgentTaskFailed {
        metadata: test_metadata(),
        from: "agent-001".to_string(),
        to: "agent-002".to_string(),
        task_id: "task-001".to_string(),
        error: "工具调用失败".to_string(),
        retry_count: 3,
    };
    // WHY Critical:任务失败可能影响 Quest 完整性,必须保证投递到 SecCore 与
    // Parliament 进行补救决策。若标为 Normal,在背压场景下可能被丢弃,
    // 导致失败无人响应、Quest 持续等待已死 Agent 的结果。
    assert_eq!(event.severity(), EventSeverity::Critical);
}

#[test]
fn test_agent_task_failed_serialization() {
    let event = NexusEvent::AgentTaskFailed {
        metadata: test_metadata(),
        from: "agent-001".to_string(),
        to: "agent-002".to_string(),
        task_id: "task-001".to_string(),
        error: "工具调用超时(30s 未响应)".to_string(),
        retry_count: 3,
    };
    let bytes = serialize_msgpack(&event).expect("MessagePack 序列化失败");
    let decoded: NexusEvent = deserialize_msgpack(&bytes).expect("MessagePack 反序列化失败");
    assert_eq!(event, decoded);
}

// ============================================================
// 变体 4: AgentConsultRequested — 咨询请求
// severity: Normal
// ============================================================

#[test]
fn test_agent_consult_requested_variant_exists() {
    let event = NexusEvent::AgentConsultRequested {
        metadata: test_metadata(),
        from: "agent-001".to_string(),
        to: "agent-002".to_string(),
        question: "如何处理 UTF-8 编码的 emoji?".to_string(),
        context: "用户输入包含表情符号,需正确计算 token 数".to_string(),
        urgency: ConsultUrgency::Medium,
    };
    let _ = event; // 构造成功即验证变体存在
}

#[test]
fn test_agent_consult_requested_topic() {
    let event = NexusEvent::AgentConsultRequested {
        metadata: test_metadata(),
        from: "agent-001".to_string(),
        to: "agent-002".to_string(),
        question: "如何处理?".to_string(),
        context: "上下文".to_string(),
        urgency: ConsultUrgency::High,
    };
    assert_eq!(event.topic(), EventTopic::Agent);
}

#[test]
fn test_agent_consult_requested_severity() {
    let event = NexusEvent::AgentConsultRequested {
        metadata: test_metadata(),
        from: "agent-001".to_string(),
        to: "agent-002".to_string(),
        question: "如何处理?".to_string(),
        context: "上下文".to_string(),
        urgency: ConsultUrgency::High,
    };
    assert_eq!(event.severity(), EventSeverity::Normal);
}

#[test]
fn test_agent_consult_requested_serialization() {
    let event = NexusEvent::AgentConsultRequested {
        metadata: test_metadata(),
        from: "agent-001".to_string(),
        to: "agent-002".to_string(),
        question: "如何处理 UTF-8 编码的 emoji?".to_string(),
        context: "用户输入包含表情符号".to_string(),
        urgency: ConsultUrgency::Critical,
    };
    let bytes = serialize_msgpack(&event).expect("MessagePack 序列化失败");
    let decoded: NexusEvent = deserialize_msgpack(&bytes).expect("MessagePack 反序列化失败");
    assert_eq!(event, decoded);
}

// ============================================================
// 变体 5: AgentConsultResponded — 咨询回复
// severity: Normal
// ============================================================

#[test]
fn test_agent_consult_responded_variant_exists() {
    let event = NexusEvent::AgentConsultResponded {
        metadata: test_metadata(),
        from: "agent-002".to_string(),
        to: "agent-001".to_string(),
        answer: "使用 unicode-segmentation crate 的 graphemes() 迭代器".to_string(),
        references: vec![
            "https://unicode.org/reports/tr29/".to_string(),
            "https://crates.io/crates/unicode-segmentation".to_string(),
        ],
    };
    let _ = event; // 构造成功即验证变体存在
}

#[test]
fn test_agent_consult_responded_topic() {
    let event = NexusEvent::AgentConsultResponded {
        metadata: test_metadata(),
        from: "agent-002".to_string(),
        to: "agent-001".to_string(),
        answer: "答案".to_string(),
        references: vec!["ref-1".to_string()],
    };
    assert_eq!(event.topic(), EventTopic::Agent);
}

#[test]
fn test_agent_consult_responded_severity() {
    let event = NexusEvent::AgentConsultResponded {
        metadata: test_metadata(),
        from: "agent-002".to_string(),
        to: "agent-001".to_string(),
        answer: "答案".to_string(),
        references: vec![],
    };
    assert_eq!(event.severity(), EventSeverity::Normal);
}

#[test]
fn test_agent_consult_responded_serialization() {
    let event = NexusEvent::AgentConsultResponded {
        metadata: test_metadata(),
        from: "agent-002".to_string(),
        to: "agent-001".to_string(),
        answer: "使用 graphemes() 迭代器".to_string(),
        references: vec![
            "https://unicode.org/reports/tr29/".to_string(),
            "https://crates.io/crates/unicode-segmentation".to_string(),
        ],
    };
    let bytes = serialize_msgpack(&event).expect("MessagePack 序列化失败");
    let decoded: NexusEvent = deserialize_msgpack(&bytes).expect("MessagePack 反序列化失败");
    assert_eq!(event, decoded);
}

// ============================================================
// 变体 6: AgentHeartbeat — Agent 心跳(状态 + 资源占用)
// severity: Normal
// ============================================================

#[test]
fn test_agent_heartbeat_variant_exists() {
    let event = NexusEvent::AgentHeartbeat {
        metadata: test_metadata(),
        from: "agent-001".to_string(),
        status: AgentStatus::Running,
        current_task: Some("task-001".to_string()),
        token_usage: 4096,
        memory_usage_mb: 128,
    };
    let _ = event; // 构造成功即验证变体存在
}

#[test]
fn test_agent_heartbeat_topic() {
    let event = NexusEvent::AgentHeartbeat {
        metadata: test_metadata(),
        from: "agent-001".to_string(),
        status: AgentStatus::Running,
        current_task: None,
        token_usage: 0,
        memory_usage_mb: 0,
    };
    assert_eq!(event.topic(), EventTopic::Agent);
}

#[test]
fn test_agent_heartbeat_severity() {
    let event = NexusEvent::AgentHeartbeat {
        metadata: test_metadata(),
        from: "agent-001".to_string(),
        status: AgentStatus::Idle,
        current_task: None,
        token_usage: 0,
        memory_usage_mb: 32,
    };
    assert_eq!(event.severity(), EventSeverity::Normal);
}

#[test]
fn test_agent_heartbeat_serialization() {
    let event = NexusEvent::AgentHeartbeat {
        metadata: test_metadata(),
        from: "agent-001".to_string(),
        status: AgentStatus::Running,
        current_task: Some("task-001".to_string()),
        token_usage: 4096,
        memory_usage_mb: 128,
    };
    let bytes = serialize_msgpack(&event).expect("MessagePack 序列化失败");
    let decoded: NexusEvent = deserialize_msgpack(&bytes).expect("MessagePack 反序列化失败");
    assert_eq!(event, decoded);
}

// ============================================================
// 变体 7: AgentContextOverflow — 上下文溢出告警
// severity: Normal(severity() 同步函数返回值,发布时走 Critical 通道)
// ============================================================

#[test]
fn test_agent_context_overflow_variant_exists() {
    let event = NexusEvent::AgentContextOverflow {
        metadata: test_metadata(),
        agent_id: "agent-001".to_string(),
        current_tokens: 131072,
        max_tokens: 131072,
    };
    let _ = event; // 构造成功即验证变体存在
}

#[test]
fn test_agent_context_overflow_topic() {
    let event = NexusEvent::AgentContextOverflow {
        metadata: test_metadata(),
        agent_id: "agent-001".to_string(),
        current_tokens: 131072,
        max_tokens: 131072,
    };
    assert_eq!(event.topic(), EventTopic::Agent);
}

#[test]
fn test_agent_context_overflow_severity() {
    let event = NexusEvent::AgentContextOverflow {
        metadata: test_metadata(),
        agent_id: "agent-001".to_string(),
        current_tokens: 131072,
        max_tokens: 131072,
    };
    // WHY Normal 而非 Critical:severity() 是同步函数,不依赖运行时值。
    // 但此事件语义上是告警,发布者应通过 Critical 通道发送以确保投递
    // (类似 AsaIntervention Block 场景的处理模式,见 types.rs:1096 注释)。
    assert_eq!(event.severity(), EventSeverity::Normal);
}

#[test]
fn test_agent_context_overflow_serialization() {
    let event = NexusEvent::AgentContextOverflow {
        metadata: test_metadata(),
        agent_id: "agent-001".to_string(),
        current_tokens: 131072,
        max_tokens: 131072,
    };
    let bytes = serialize_msgpack(&event).expect("MessagePack 序列化失败");
    let decoded: NexusEvent = deserialize_msgpack(&bytes).expect("MessagePack 反序列化失败");
    assert_eq!(event, decoded);
}

// ============================================================
// 辅助类型变体覆盖测试(Task 4 实现后应通过)
// 验证 TaskPriority/ConsultUrgency/AgentStatus 各变体可构造
// ============================================================

#[test]
fn test_task_priority_all_variants_constructible() {
    let _low = TaskPriority::Low;
    let _medium = TaskPriority::Medium;
    let _high = TaskPriority::High;
    let _critical = TaskPriority::Critical;
}

#[test]
fn test_consult_urgency_all_variants_constructible() {
    let _low = ConsultUrgency::Low;
    let _medium = ConsultUrgency::Medium;
    let _high = ConsultUrgency::High;
    let _critical = ConsultUrgency::Critical;
}

#[test]
fn test_agent_status_all_variants_constructible() {
    let _idle = AgentStatus::Idle;
    let _running = AgentStatus::Running;
    let _paused = AgentStatus::Paused;
    let _completed = AgentStatus::Completed;
    let _failed = AgentStatus::Failed;
    let _crashed = AgentStatus::Crashed;
}

// ============================================================
// EventTopic::Agent 变体存在性测试(Task 4 实现后应通过)
// 验证 EventTopic 新增 Agent 变体可构造且不与现有 9 个变体冲突
// ============================================================

#[test]
fn test_event_topic_agent_variant_exists() {
    let topic = EventTopic::Agent;
    // 验证 Agent topic 可构造
    assert_eq!(topic, EventTopic::Agent);
}

#[test]
fn test_event_topic_agent_in_all_set() {
    // WHY:Task 4 应将 Agent 加入 EventTopic::all() 返回的集合,
    // 使 FilteredSubscriber 可订阅 Agent topic。
    // 此测试验证 all() 包含 Agent 变体(若未加入则失败)。
    use std::collections::HashSet;
    let all: HashSet<EventTopic> = EventTopic::all();
    assert!(
        all.contains(&EventTopic::Agent),
        "EventTopic::all() 应包含 Agent 变体(Task 4 应更新 all() 实现)"
    );
}

// ============================================================
// 回归测试:确保新增 Agent topic 不破坏现有 topic 映射
// WHY:Task 4 在 topic.rs 的 match 中添加新分支时,不应误改现有变体的映射。
// 此测试抽样验证现有 topic 映射保持不变。
// ============================================================

#[test]
fn test_existing_topic_mapping_unchanged_after_agent_addition() {
    // 抽样验证现有 9 个 topic 的代表性变体映射保持不变
    let routing_event = NexusEvent::ExpertRegistered {
        metadata: test_metadata(),
        tool_id: "t-1".to_string(),
    };
    assert_eq!(routing_event.topic(), EventTopic::Routing);

    let memory_event = NexusEvent::NmcEncoded {
        metadata: test_metadata(),
        modality: "Text".to_string(),
        content_hash: "h".to_string(),
        clv_dimension: 512,
    };
    assert_eq!(memory_event.topic(), EventTopic::Memory);

    let security_event = NexusEvent::SkepticVeto {
        metadata: test_metadata(),
        quest_id: "q-1".to_string(),
        veto_reason: "test".to_string(),
        frozen_capabilities: vec![],
    };
    assert_eq!(security_event.topic(), EventTopic::Security);

    let execution_event = NexusEvent::PredictionMade {
        metadata: test_metadata(),
        quest_id: "q-1".to_string(),
        n: 3,
        avg_confidence: 0.85,
    };
    assert_eq!(execution_event.topic(), EventTopic::Execution);

    let parliament_event = NexusEvent::VoteCast {
        metadata: test_metadata(),
        proposal_id: "p-1".to_string(),
        voter: "v-1".to_string(),
        vote: true,
    };
    assert_eq!(parliament_event.topic(), EventTopic::Parliament);

    let quest_event = NexusEvent::QuestCreated {
        metadata: test_metadata(),
        quest_id: "q-1".to_string(),
        title: "t".to_string(),
        task_count: 1,
    };
    assert_eq!(quest_event.topic(), EventTopic::Quest);

    let system_event = NexusEvent::SlowConsumerDropped {
        metadata: test_metadata(),
        subscriber_id: "s-1".to_string(),
        lag: 10,
        dropped_count: 5,
    };
    assert_eq!(system_event.topic(), EventTopic::System);

    let knowledge_event = NexusEvent::WikiUpdated {
        metadata: test_metadata(),
        wiki_hash: "h".to_string(),
        delta: 5,
    };
    assert_eq!(knowledge_event.topic(), EventTopic::Knowledge);

    let storage_event = NexusEvent::CacheHit {
        metadata: test_metadata(),
        cache_key: "k-1".to_string(),
    };
    assert_eq!(storage_event.topic(), EventTopic::Storage);
}
