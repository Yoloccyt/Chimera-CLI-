//! Task 12 (TDD RED) — RootOrchestrator 根协调器失败测试
//!
//! 对应 SubTask 12.1: 编写失败测试(RED 阶段,15+ 测试)
//! 对应 SubTask 12.2: RootOrchestrator::delegate(task) -> Result<Vec<AgentHandle>>
//! 对应 SubTask 12.3: RootOrchestrator::monitor() 订阅 NexusEvent::AgentHeartbeat
//! 对应 SubTask 12.4: 最大委托深度限制 max_agent_depth=5
//!
//! ## TDD 阶段
//!
//! - **RED**: 当前阶段,测试应失败(RootOrchestrator::delegate/monitor 尚未实现,
//!   new() 签名需演进为接收 EventBus)
//! - **GREEN**: SubTask 12.2-12.4 实现后,测试应全部通过
//!
//! ## 架构合规
//!
//! - §4.4 反模式 3:`bus.subscribe_filtered()` 必须在 `tokio::spawn()` 之前同步调用
//! - §6.2:AgentTaskDelegated 是 Normal 级,走 broadcast(非 mpsc)
//! - ADR-026 决策 1:最大委托深度 5(MAX_AGENT_DEPTH 常量)
//! - ADR-026 决策 2:复用 event-bus,不新建 AgentMessageBus
//! - §2.2 依赖铁律:chimera-mas (L9) → event-bus (L1) 向下依赖允许

#![forbid(unsafe_code)]

use chimera_mas::prelude::*;
use chimera_mas::MAX_AGENT_DEPTH;
use event_bus::{EventBus, EventMetadata, EventTopic, NexusEvent};
use nexus_core::{Task, TaskStatus};
use std::collections::HashSet;
use std::time::Duration;

// ============================================================
// 辅助函数 — 创建测试用 AgentTask
// ============================================================

/// 创建测试用 nexus_core::Task(Pending 状态,无依赖)
fn make_task(id: &str, desc: &str) -> Task {
    Task {
        task_id: id.into(),
        description: desc.into(),
        status: TaskStatus::Pending,
        dependencies: vec![],
    }
}

/// 创建测试用 AgentTask(指定复杂度,默认 delegation_depth=0)
fn make_agent_task(id: &str, complexity: TaskComplexity) -> AgentTask {
    AgentTask::new(
        make_task(id, &format!("任务-{id}")),
        complexity,
        1000,
        Duration::from_secs(60),
        QualityLevel::Standard,
    )
}

/// 创建带 delegation_depth 的 AgentTask
fn make_agent_task_with_depth(id: &str, complexity: TaskComplexity, depth: usize) -> AgentTask {
    make_agent_task(id, complexity).with_depth(depth)
}

// ============================================================
// SubTask 12.1: 基础结构与常量测试
// ============================================================

#[test]
fn test_max_agent_depth_constant_is_five() {
    // ADR-026 决策 1: 硬编码常量 MAX_AGENT_DEPTH=5
    assert_eq!(
        MAX_AGENT_DEPTH, 5,
        "MAX_AGENT_DEPTH 必须为 5(ADR-026 决策 1)"
    );
}

#[test]
fn test_root_orchestrator_new_default_max_depth() {
    let bus = EventBus::new();
    let orchestrator = RootOrchestrator::new(bus);
    assert_eq!(
        orchestrator.max_depth(),
        MAX_AGENT_DEPTH,
        "new() 默认 max_depth 必须为 MAX_AGENT_DEPTH(5)"
    );
}

#[test]
fn test_root_orchestrator_with_max_depth_custom() {
    let bus = EventBus::new();
    let orchestrator = RootOrchestrator::with_max_depth(bus, 3);
    assert_eq!(
        orchestrator.max_depth(),
        3,
        "with_max_depth(bus, 3) 应设置 max_depth=3"
    );
}

#[test]
fn test_root_orchestrator_with_max_depth_minimum_one() {
    // 传入 0 应被钳制为最小值 1,避免 max_depth=0 导致所有委托立即失败
    let bus = EventBus::new();
    let orchestrator = RootOrchestrator::with_max_depth(bus, 0);
    assert!(
        orchestrator.max_depth() >= 1,
        "with_max_depth(bus, 0) 应钳制为最小 1,实际 = {}",
        orchestrator.max_depth()
    );
}

#[test]
fn test_check_depth_within_limit_returns_ok() {
    let bus = EventBus::new();
    let orchestrator = RootOrchestrator::new(bus);
    // depth=3 < max_depth=5,应返回 Ok
    assert!(
        orchestrator.check_depth(3).is_ok(),
        "check_depth(3) 在 max_depth=5 时应返回 Ok"
    );
}

#[test]
fn test_check_depth_exceeded_returns_error() {
    let bus = EventBus::new();
    let orchestrator = RootOrchestrator::new(bus);
    // depth=5 >= max_depth=5,应返回 MaxDepthExceeded
    let result = orchestrator.check_depth(5);
    assert!(
        matches!(
            result,
            Err(MasError::MaxDepthExceeded {
                current_depth: 5,
                max_depth: 5
            })
        ),
        "check_depth(5) 在 max_depth=5 时应返回 MaxDepthExceeded {{5, 5}},实际 = {:?}",
        result
    );
}

#[test]
fn test_check_depth_boundary_four_ok() {
    let bus = EventBus::new();
    let orchestrator = RootOrchestrator::new(bus);
    // depth=4 < max_depth=5,边界值应返回 Ok(子任务 depth=5 仍可创建)
    assert!(
        orchestrator.check_depth(4).is_ok(),
        "check_depth(4) 在 max_depth=5 时应返回 Ok(边界值)"
    );
}

// ============================================================
// SubTask 12.2: delegate 任务分发测试
// ============================================================

#[tokio::test]
async fn test_delegate_simple_task_creates_one_agent() {
    let bus = EventBus::new();
    let orchestrator = RootOrchestrator::new(bus);
    let task = make_agent_task("t-simple", TaskComplexity::Simple);

    let handles = orchestrator
        .delegate(task)
        .await
        .expect("Simple 任务委托应成功");

    assert_eq!(handles.len(), 1, "Simple 任务应创建恰好 1 个子 Agent");
}

#[tokio::test]
async fn test_delegate_medium_task_creates_two_to_three_agents() {
    let bus = EventBus::new();
    let orchestrator = RootOrchestrator::new(bus);
    let task = make_agent_task("t-medium", TaskComplexity::Medium);

    let handles = orchestrator
        .delegate(task)
        .await
        .expect("Medium 任务委托应成功");

    assert!(
        (2..=3).contains(&handles.len()),
        "Medium 任务应创建 2-3 个子 Agent,实际 = {}",
        handles.len()
    );
}

#[tokio::test]
async fn test_delegate_complex_task_creates_three_to_five_agents() {
    let bus = EventBus::new();
    let orchestrator = RootOrchestrator::new(bus);
    let task = make_agent_task("t-complex", TaskComplexity::Complex);

    let handles = orchestrator
        .delegate(task)
        .await
        .expect("Complex 任务委托应成功");

    assert!(
        (3..=5).contains(&handles.len()),
        "Complex 任务应创建 3-5 个子 Agent,实际 = {}",
        handles.len()
    );
}

#[tokio::test]
async fn test_delegate_very_complex_task_creates_three_to_five_agents() {
    let bus = EventBus::new();
    let orchestrator = RootOrchestrator::new(bus);
    let task = make_agent_task("t-very-complex", TaskComplexity::VeryComplex);

    let handles = orchestrator
        .delegate(task)
        .await
        .expect("VeryComplex 任务委托应成功");

    assert!(
        (3..=5).contains(&handles.len()),
        "VeryComplex 任务应创建 3-5 个子 Agent,实际 = {}",
        handles.len()
    );
}

#[tokio::test]
async fn test_delegate_handles_have_correct_depth() {
    let bus = EventBus::new();
    let orchestrator = RootOrchestrator::new(bus);
    // delegation_depth=0 的任务,子 Agent depth 应为 1
    let task = make_agent_task_with_depth("t-depth", TaskComplexity::Simple, 0);

    let handles = orchestrator.delegate(task).await.expect("委托应成功");

    for handle in &handles {
        assert_eq!(
            handle.depth, 1,
            "delegation_depth=0 的子 Agent depth 必须为 1,实际 = {}",
            handle.depth
        );
    }
}

#[tokio::test]
async fn test_delegate_handles_have_agent_type() {
    let bus = EventBus::new();
    let orchestrator = RootOrchestrator::new(bus);
    let task = make_agent_task("t-type", TaskComplexity::Simple);

    let handles = orchestrator.delegate(task).await.expect("委托应成功");

    // 每个 handle 必须携带 agent_type(spec 要求"含 agent_id + agent_type")
    for handle in &handles {
        assert_eq!(
            handle.depth,
            handle.agent_type.depth(),
            "AgentHandle.depth 必须与 AgentHandle.agent_type.depth() 一致"
        );
    }
}

#[tokio::test]
async fn test_delegate_publishes_agent_task_delegated_events() {
    let bus = EventBus::new();
    // §4.4 反模式 3: subscribe 必须在 delegate(内部 spawn/create_agent)之前同步调用
    let mut rx = bus.subscribe_filtered(HashSet::from([EventTopic::Agent]));
    let orchestrator = RootOrchestrator::new(bus);
    let task = make_agent_task("t-event", TaskComplexity::Medium);

    let handles = orchestrator.delegate(task).await.expect("委托应成功");

    // delegate 应为每个子 Agent 发布 AgentTaskDelegated 事件
    // (AgentFactory::create_agent 内部已发布,delegate 调用 create_agent)
    let mut delegated_count = 0;
    for _ in 0..handles.len() {
        match tokio::time::timeout(Duration::from_millis(500), rx.recv()).await {
            Ok(Ok(NexusEvent::AgentTaskDelegated { .. })) => delegated_count += 1,
            _ => break,
        }
    }
    assert_eq!(
        delegated_count,
        handles.len(),
        "应发布 {} 个 AgentTaskDelegated 事件,实际收到 {}",
        handles.len(),
        delegated_count
    );
}

// ============================================================
// SubTask 12.4: 最大委托深度限制测试
// ============================================================

#[tokio::test]
async fn test_delegate_at_max_depth_returns_error() {
    let bus = EventBus::new();
    let orchestrator = RootOrchestrator::new(bus);
    // delegation_depth=5 >= max_depth=5,应返回 MaxDepthExceeded
    let task = make_agent_task_with_depth("t-max", TaskComplexity::Simple, 5);

    let result = orchestrator.delegate(task).await;
    assert!(
        matches!(
            result,
            Err(MasError::MaxDepthExceeded {
                current_depth: 5,
                max_depth: 5
            })
        ),
        "delegation_depth=5 应返回 MaxDepthExceeded {{5, 5}},实际 = {:?}",
        result
    );
}

#[tokio::test]
async fn test_delegate_depth_four_still_ok() {
    let bus = EventBus::new();
    let orchestrator = RootOrchestrator::new(bus);
    // delegation_depth=4 < max_depth=5,应仍可委托(子任务 depth=5,但未超限)
    let task = make_agent_task_with_depth("t-4", TaskComplexity::Simple, 4);

    let handles = orchestrator
        .delegate(task)
        .await
        .expect("delegation_depth=4 应仍可委托");

    // 子 Agent depth = delegation_depth + 1 = 5
    for handle in &handles {
        assert_eq!(
            handle.depth, 5,
            "delegation_depth=4 的子 Agent depth 必须为 5(临界值)"
        );
    }
}

#[tokio::test]
async fn test_delegate_child_depth_increments() {
    let bus = EventBus::new();
    let orchestrator = RootOrchestrator::new(bus);

    // 测试不同 delegation_depth 下子 Agent depth 递增
    for parent_depth in 0..4 {
        let task = make_agent_task_with_depth(
            &format!("t-inc-{parent_depth}"),
            TaskComplexity::Simple,
            parent_depth,
        );
        let handles = orchestrator
            .delegate(task)
            .await
            .unwrap_or_else(|_| panic!("delegation_depth={parent_depth} 委托应成功"));
        for handle in &handles {
            assert_eq!(
                handle.depth,
                parent_depth + 1,
                "delegation_depth={parent_depth} 的子 Agent depth 必须为 {}",
                parent_depth + 1
            );
        }
    }
}

#[tokio::test]
async fn test_delegate_custom_max_depth_rejects() {
    let bus = EventBus::new();
    // 自定义 max_depth=2,delegation_depth=2 应被拒绝
    let orchestrator = RootOrchestrator::with_max_depth(bus, 2);
    let task = make_agent_task_with_depth("t-custom", TaskComplexity::Simple, 2);

    let result = orchestrator.delegate(task).await;
    assert!(
        matches!(
            result,
            Err(MasError::MaxDepthExceeded {
                current_depth: 2,
                max_depth: 2
            })
        ),
        "delegation_depth=2 在 max_depth=2 时应返回 MaxDepthExceeded,实际 = {:?}",
        result
    );
}

// ============================================================
// SubTask 12.3: monitor 心跳订阅测试
// ============================================================

#[tokio::test]
async fn test_monitor_subscribes_agent_topic() {
    let bus = EventBus::new();
    let orchestrator = RootOrchestrator::new(bus.clone());

    // monitor 返回 JoinHandle,spawn 后台任务订阅 EventTopic::Agent
    let handle = orchestrator
        .monitor()
        .await
        .expect("monitor 应成功 spawn 心跳订阅任务");

    // 发布一个 AgentHeartbeat 事件,monitor 应能接收(不阻塞主线程)
    let heartbeat_event = NexusEvent::AgentHeartbeat {
        metadata: EventMetadata::new("test:orchestrator"),
        from: "agent-heartbeat-1".into(),
        status: event_bus::AgentStatus::Running,
        current_task: Some("t-heartbeat".into()),
        token_usage: 500,
        memory_usage_mb: 128,
    };
    bus.publish(heartbeat_event)
        .await
        .expect("发布心跳事件应成功");

    // 等待后台任务处理事件(短暂 sleep 确保事件被消费)
    tokio::time::sleep(Duration::from_millis(100)).await;

    // 验证 monitor 收到心跳并更新内部状态
    let count = orchestrator.heartbeat_count().await;
    assert!(
        count >= 1,
        "monitor 应收到至少 1 个心跳,heartbeat_count = {}",
        count
    );

    // 终止后台任务
    handle.abort();
}

#[tokio::test]
async fn test_monitor_collects_heartbeat_info() {
    let bus = EventBus::new();
    let orchestrator = RootOrchestrator::new(bus.clone());
    let handle = orchestrator.monitor().await.expect("monitor 应成功 spawn");

    // 发布心跳事件
    let agent_id = "agent-collect-1";
    let heartbeat_event = NexusEvent::AgentHeartbeat {
        metadata: EventMetadata::new("test:orchestrator"),
        from: agent_id.into(),
        status: event_bus::AgentStatus::Running,
        current_task: Some("t-collect".into()),
        token_usage: 1024,
        memory_usage_mb: 256,
    };
    bus.publish(heartbeat_event).await.unwrap();

    tokio::time::sleep(Duration::from_millis(150)).await;

    // 验证心跳信息被正确收集
    let info = orchestrator
        .get_heartbeat(agent_id)
        .await
        .expect("应能查询到 agent-collect-1 的心跳信息");
    assert_eq!(info.agent_id, agent_id);
    assert_eq!(info.status, event_bus::AgentStatus::Running);
    assert_eq!(info.current_task.as_deref(), Some("t-collect"));
    assert_eq!(info.token_usage, 1024);
    assert_eq!(info.memory_usage_mb, 256);

    handle.abort();
}

#[tokio::test]
async fn test_monitor_ignores_non_heartbeat_agent_events() {
    let bus = EventBus::new();
    let orchestrator = RootOrchestrator::new(bus.clone());
    let handle = orchestrator.monitor().await.expect("monitor 应成功 spawn");

    // 发布非 AgentHeartbeat 的 Agent 主题事件(如 AgentTaskCompleted)
    let completed_event = NexusEvent::AgentTaskCompleted {
        metadata: EventMetadata::new("test:orchestrator"),
        from: "agent-other".into(),
        to: "root".into(),
        task_id: "t-completed".into(),
        result_summary: "完成".into(),
    };
    bus.publish(completed_event).await.unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    // monitor 只收集 AgentHeartbeat,非心跳事件不应被记入 heartbeats
    let count = orchestrator.heartbeat_count().await;
    assert_eq!(
        count, 0,
        "monitor 应忽略非 AgentHeartbeat 事件,heartbeat_count = {count}"
    );

    handle.abort();
}

#[tokio::test]
async fn test_monitor_multiple_heartbeats_overwrite() {
    let bus = EventBus::new();
    let orchestrator = RootOrchestrator::new(bus.clone());
    let handle = orchestrator.monitor().await.expect("monitor 应成功 spawn");

    // 同一 Agent 发布两次心跳(状态变化),后一次应覆盖前一次
    let agent_id = "agent-overwrite";
    let h1 = NexusEvent::AgentHeartbeat {
        metadata: EventMetadata::new("test"),
        from: agent_id.into(),
        status: event_bus::AgentStatus::Idle,
        current_task: None,
        token_usage: 0,
        memory_usage_mb: 0,
    };
    bus.publish(h1).await.unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;

    let h2 = NexusEvent::AgentHeartbeat {
        metadata: EventMetadata::new("test"),
        from: agent_id.into(),
        status: event_bus::AgentStatus::Running,
        current_task: Some("t-new".into()),
        token_usage: 100,
        memory_usage_mb: 50,
    };
    bus.publish(h2).await.unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;

    // heartbeat_count 应为 1(同 agent_id 覆盖,非累加)
    let count = orchestrator.heartbeat_count().await;
    assert_eq!(count, 1, "同一 Agent 多次心跳应覆盖,非累加");

    let info = orchestrator.get_heartbeat(agent_id).await.unwrap();
    assert_eq!(
        info.status,
        event_bus::AgentStatus::Running,
        "应保留最新心跳状态"
    );
    assert_eq!(info.token_usage, 100, "应保留最新 token_usage");

    handle.abort();
}

// ============================================================
// ADR-027: delegate_quadrants 象限感知孙层编排测试 (§3.3 / INV-3 / INV-4)
// ============================================================

#[test]
fn test_delegate_quadrants_simple_activates_one() {
    let orch = RootOrchestrator::new(EventBus::new());
    // delegation_depth=2(子代理) → 孙代理 depth=3
    let task = make_agent_task_with_depth("q-simple", TaskComplexity::Simple, 2);
    let handles = orch
        .delegate_quadrants("sub-simple", &task)
        .expect("Simple 象限编排应成功");
    assert_eq!(handles.len(), 1, "Simple 仅激活 Q1");
    assert_eq!(handles[0].depth, 3, "delegation_depth=2 → 孙代理 depth=3");
}

#[test]
fn test_delegate_quadrants_very_complex_activates_four() {
    let orch = RootOrchestrator::new(EventBus::new());
    let task = make_agent_task_with_depth("q-vc", TaskComplexity::VeryComplex, 2);
    let handles = orch
        .delegate_quadrants("sub-vc", &task)
        .expect("VeryComplex 象限编排应成功");
    assert_eq!(handles.len(), 4, "VeryComplex 激活全部四象限");
}

#[test]
fn test_delegate_quadrants_never_exceeds_four() {
    // INV-3: 任何复杂度下孙层扇出 ≤ 4
    for (id, complexity) in [
        ("s", TaskComplexity::Simple),
        ("m", TaskComplexity::Medium),
        ("c", TaskComplexity::Complex),
        ("v", TaskComplexity::VeryComplex),
    ] {
        let orch = RootOrchestrator::new(EventBus::new());
        let task = make_agent_task_with_depth(&format!("q-{id}"), complexity, 2);
        let handles = orch
            .delegate_quadrants(&format!("sub-{id}"), &task)
            .expect("编排应成功");
        assert!(
            handles.len() <= 4,
            "INV-3: 孙层扇出 ≤ 4，实际 = {}",
            handles.len()
        );
    }
}

#[test]
fn test_delegate_quadrants_encodes_quadrant_in_scope() {
    let orch = RootOrchestrator::new(EventBus::new());
    let task = make_agent_task_with_depth("q-enc", TaskComplexity::VeryComplex, 2);
    let handles = orch
        .delegate_quadrants("sub-enc", &task)
        .expect("编排应成功");
    // 句柄顺序 Q1→Q2→Q3→Q4, task_scope 尾缀应依次编码
    let tags = ["#Q1", "#Q2", "#Q3", "#Q4"];
    for (handle, tag) in handles.iter().zip(tags.iter()) {
        match &handle.agent_type {
            AgentType::GrandAgent {
                task_scope,
                parent_id,
            } => {
                assert!(
                    task_scope.ends_with(tag),
                    "task_scope {task_scope} 应以 {tag} 结尾"
                );
                assert_eq!(parent_id, "sub-enc", "parent_id 应为发起子代理");
            }
            other => panic!("应为 GrandAgent, 实际 {other:?}"),
        }
        assert_eq!(handle.depth, 3, "孙代理 depth 应为 3");
    }
}

#[test]
fn test_delegate_quadrants_respects_max_depth() {
    let orch = RootOrchestrator::new(EventBus::new());
    // delegation_depth=5 >= max_depth=5 → MaxDepthExceeded(与 delegate 一致)
    let task = make_agent_task_with_depth("q-deep", TaskComplexity::Simple, 5);
    let result = orch.delegate_quadrants("sub-deep", &task);
    assert!(
        matches!(result, Err(MasError::MaxDepthExceeded { .. })),
        "发起方深度 5 应拒绝, 实际 = {result:?}"
    );
}

#[test]
fn test_delegate_quadrants_unique_ids_per_quadrant() {
    let orch = RootOrchestrator::new(EventBus::new());
    let task = make_agent_task_with_depth("q-ids", TaskComplexity::Complex, 2);
    let handles = orch
        .delegate_quadrants("sub-ids", &task)
        .expect("编排应成功");
    let ids: std::collections::HashSet<&str> =
        handles.iter().map(|h| h.agent_id.as_str()).collect();
    assert_eq!(ids.len(), handles.len(), "各象限孙代理 ID 应唯一(INV-4)");
}
