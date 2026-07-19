//! Task 8.1 (TDD RED) — Agent 工厂与生命周期失败测试
//!
//! 对应 SubTask 8.1: 编写失败测试覆盖 create_agent + 生命周期状态机 + 事件发布
//! 对应 SubTask 8.2: AgentFactory::create_agent(agent_type, agent_id) -> Result<Agent>
//! 对应 SubTask 8.3: AgentLifecycle 状态机(Idle→Running→Paused→Completed/Failed/Terminated)
//! 对应 SubTask 8.4: create_agent 后发布 NexusEvent::AgentTaskDelegated
//!
//! ## TDD 阶段
//!
//! - **RED**: 当前阶段,测试应全部失败(Agent/AgentLifecycle/LifecycleState 尚未实现)
//! - **GREEN**: SubTask 8.2-8.4 实现后,测试应全部通过
//!
//! ## 架构合规
//!
//! - §4.4 反模式 3:`bus.subscribe()` 在 create_agent 之前同步调用(红线)
//! - §4.4 反模式 8:sync 方法 create_agent 用 `publish_blocking` 发布事件
//! - §6.2:AgentTaskDelegated 是 Normal 级,走 broadcast(非 mpsc)
//! - Agent 持有 AgentMeta + AgentContext + AgentLifecycle 三组件

#![forbid(unsafe_code)]

use chimera_mas::prelude::*;
use chimera_mas::LifecycleState;
use event_bus::{EventBus, NexusEvent, TaskPriority};

// ============================================================
// SubTask 8.2: AgentFactory::create_agent 基础测试
// ============================================================

#[test]
fn test_create_root_orchestrator_agent() {
    let bus = EventBus::new();
    let factory = AgentFactory::new(bus);
    let agent = factory
        .create_agent(AgentType::RootOrchestrator, "root-1")
        .expect("创建 RootOrchestrator Agent 应成功");

    assert_eq!(agent.meta().agent_id, "root-1");
    assert_eq!(agent.meta().agent_type, AgentType::RootOrchestrator);
    assert_eq!(agent.meta().depth, 0, "RootOrchestrator depth 必须为 0");
    assert_eq!(
        agent.meta().context_window,
        1_048_576,
        "上下文窗口应为 1M 等效"
    );
}

#[test]
fn test_create_main_agent() {
    let bus = EventBus::new();
    let factory = AgentFactory::new(bus);
    let agent_type = AgentType::MainAgent {
        domain: "frontend".into(),
    };
    let agent = factory
        .create_agent(agent_type.clone(), "main-1")
        .expect("创建 MainAgent 应成功");

    assert_eq!(agent.meta().agent_id, "main-1");
    assert_eq!(agent.meta().agent_type, agent_type);
    assert_eq!(agent.meta().depth, 1);
}

#[test]
fn test_create_sub_agent() {
    let bus = EventBus::new();
    let factory = AgentFactory::new(bus);
    let agent_type = AgentType::SubAgent {
        parent_id: "main-1".into(),
        task_scope: "implement-api".into(),
    };
    let agent = factory
        .create_agent(agent_type.clone(), "sub-1")
        .expect("创建 SubAgent 应成功");

    assert_eq!(agent.meta().agent_id, "sub-1");
    assert_eq!(agent.meta().agent_type, agent_type);
    assert_eq!(agent.meta().depth, 2);
    assert_eq!(agent.meta().parent_id.as_deref(), Some("main-1"));
}

#[test]
fn test_create_grand_agent() {
    let bus = EventBus::new();
    let factory = AgentFactory::new(bus);
    let agent_type = AgentType::GrandAgent {
        parent_id: "sub-1".into(),
        task_scope: "refactor-module".into(),
    };
    let agent = factory
        .create_agent(agent_type.clone(), "grand-1")
        .expect("创建 GrandAgent 应成功");

    assert_eq!(agent.meta().agent_id, "grand-1");
    assert_eq!(agent.meta().agent_type, agent_type);
    assert_eq!(agent.meta().depth, 3);
}

#[test]
fn test_create_expert_agent() {
    let bus = EventBus::new();
    let factory = AgentFactory::new(bus);
    let agent_type = AgentType::ExpertAgent {
        specialty: vec!["security".into(), "cryptography".into()],
    };
    let agent = factory
        .create_agent(agent_type.clone(), "expert-1")
        .expect("创建 ExpertAgent 应成功");

    assert_eq!(agent.meta().agent_id, "expert-1");
    assert_eq!(agent.meta().agent_type, agent_type);
    assert_eq!(agent.meta().depth, 0, "ExpertAgent depth 必须为 0");
    assert!(agent.meta().parent_id.is_none());
}

#[test]
fn test_create_agent_duplicate_id_returns_error() {
    let bus = EventBus::new();
    let factory = AgentFactory::new(bus);
    let _first = factory
        .create_agent(AgentType::RootOrchestrator, "dup-1")
        .expect("首次创建应成功");

    let second = factory.create_agent(AgentType::RootOrchestrator, "dup-1");
    assert!(
        matches!(second, Err(MasError::AgentAlreadyExists { ref agent_id }) if agent_id == "dup-1"),
        "重复 agent_id 应返回 AgentAlreadyExists,实际: {second:?}"
    );
}

// ============================================================
// SubTask 8.2: Agent 组件持有验证(AgentMeta + AgentContext + AgentLifecycle)
// ============================================================

#[test]
fn test_new_agent_initial_state_is_idle() {
    let bus = EventBus::new();
    let factory = AgentFactory::new(bus);
    let agent = factory
        .create_agent(AgentType::RootOrchestrator, "idle-1")
        .expect("创建 Agent 应成功");

    assert_eq!(
        agent.state(),
        LifecycleState::Idle,
        "新创建 Agent 状态必须为 Idle"
    );
    assert_eq!(
        agent.meta().status,
        AgentStatus::Idle,
        "AgentMeta.status 必须与 lifecycle 同步为 Idle"
    );
}

#[test]
fn test_agent_contains_meta_context_and_lifecycle() {
    let bus = EventBus::new();
    let factory = AgentFactory::new(bus.clone());
    let agent = factory
        .create_agent(AgentType::RootOrchestrator, "comp-1")
        .expect("创建 Agent 应成功");

    // 验证 Agent 持有 AgentMeta
    assert_eq!(agent.meta().agent_id, "comp-1");
    // 验证 Agent 持有 AgentContext(agent_id 一致)
    assert_eq!(agent.context().agent_id, "comp-1");
    assert_eq!(agent.context().max_tokens, 1_048_576);
    // 验证 Agent 持有 AgentLifecycle
    assert_eq!(agent.lifecycle().current_state(), LifecycleState::Idle);
}

#[test]
fn test_agent_into_parts_decomposes() {
    let bus = EventBus::new();
    let factory = AgentFactory::new(bus);
    let agent = factory
        .create_agent(AgentType::RootOrchestrator, "parts-1")
        .expect("创建 Agent 应成功");

    let (meta, context, lifecycle) = agent.into_parts();
    assert_eq!(meta.agent_id, "parts-1");
    assert_eq!(context.agent_id, "parts-1");
    assert_eq!(lifecycle.current_state(), LifecycleState::Idle);
}

// ============================================================
// SubTask 8.3: AgentLifecycle 状态机合法转换
// ============================================================

#[test]
fn test_lifecycle_start_idle_to_running() {
    let bus = EventBus::new();
    let factory = AgentFactory::new(bus);
    let mut agent = factory
        .create_agent(AgentType::RootOrchestrator, "lc-start-1")
        .expect("创建 Agent 应成功");

    agent.start().expect("Idle → Running 应成功");
    assert_eq!(agent.state(), LifecycleState::Running);
    assert_eq!(agent.meta().status, AgentStatus::Running);
}

#[test]
fn test_lifecycle_pause_running_to_paused() {
    let bus = EventBus::new();
    let factory = AgentFactory::new(bus);
    let mut agent = factory
        .create_agent(AgentType::RootOrchestrator, "lc-pause-1")
        .expect("创建 Agent 应成功");
    agent.start().expect("Idle → Running");

    agent.pause().expect("Running → Paused 应成功");
    assert_eq!(agent.state(), LifecycleState::Paused);
    assert_eq!(agent.meta().status, AgentStatus::Paused);
}

#[test]
fn test_lifecycle_resume_paused_to_running() {
    let bus = EventBus::new();
    let factory = AgentFactory::new(bus);
    let mut agent = factory
        .create_agent(AgentType::RootOrchestrator, "lc-resume-1")
        .expect("创建 Agent 应成功");
    agent.start().expect("Idle → Running");
    agent.pause().expect("Running → Paused");

    agent.resume().expect("Paused → Running 应成功");
    assert_eq!(agent.state(), LifecycleState::Running);
    assert_eq!(agent.meta().status, AgentStatus::Running);
}

#[test]
fn test_lifecycle_complete_running_to_completed() {
    let bus = EventBus::new();
    let factory = AgentFactory::new(bus);
    let mut agent = factory
        .create_agent(AgentType::RootOrchestrator, "lc-complete-1")
        .expect("创建 Agent 应成功");
    agent.start().expect("Idle → Running");

    agent.complete().expect("Running → Completed 应成功");
    assert_eq!(agent.state(), LifecycleState::Completed);
    assert_eq!(agent.meta().status, AgentStatus::Completed);
}

#[test]
fn test_lifecycle_fail_running_to_failed() {
    let bus = EventBus::new();
    let factory = AgentFactory::new(bus);
    let mut agent = factory
        .create_agent(AgentType::RootOrchestrator, "lc-fail-1")
        .expect("创建 Agent 应成功");
    agent.start().expect("Idle → Running");

    agent.fail().expect("Running → Failed 应成功");
    assert_eq!(agent.state(), LifecycleState::Failed);
    assert_eq!(agent.meta().status, AgentStatus::Failed);
}

#[test]
fn test_lifecycle_fail_from_failed_is_idempotent() {
    let bus = EventBus::new();
    let factory = AgentFactory::new(bus);
    let mut agent = factory
        .create_agent(AgentType::RootOrchestrator, "lc-fail-idem-1")
        .expect("创建 Agent 应成功");
    agent.start().expect("Idle → Running");
    agent.fail().expect("Running → Failed");

    // Failed → Failed 幂等(任务描述: Running/Failed → Failed)
    agent.fail().expect("Failed → Failed 应幂等成功");
    assert_eq!(agent.state(), LifecycleState::Failed);
}

#[test]
fn test_lifecycle_destroy_from_running_to_terminated() {
    let bus = EventBus::new();
    let factory = AgentFactory::new(bus);
    let mut agent = factory
        .create_agent(AgentType::RootOrchestrator, "lc-destroy-1")
        .expect("创建 Agent 应成功");
    agent.start().expect("Idle → Running");

    agent.destroy().expect("Running → Terminated 应成功");
    assert_eq!(agent.state(), LifecycleState::Terminated);
    // Terminated 映射为 AgentStatus::Crashed(AgentStatus 无 Terminated 变体)
    assert_eq!(agent.meta().status, AgentStatus::Crashed);
}

#[test]
fn test_lifecycle_destroy_from_idle_to_terminated() {
    let bus = EventBus::new();
    let factory = AgentFactory::new(bus);
    let mut agent = factory
        .create_agent(AgentType::RootOrchestrator, "lc-destroy-idle-1")
        .expect("创建 Agent 应成功");

    agent
        .destroy()
        .expect("Idle → Terminated 应成功(任意状态可 destroy)");
    assert_eq!(agent.state(), LifecycleState::Terminated);
}

#[test]
fn test_lifecycle_destroy_is_idempotent() {
    let bus = EventBus::new();
    let factory = AgentFactory::new(bus);
    let mut agent = factory
        .create_agent(AgentType::RootOrchestrator, "lc-destroy-idem-1")
        .expect("创建 Agent 应成功");
    agent.destroy().expect("首次 destroy");

    // 已 Terminated 再次 destroy 幂等成功
    agent.destroy().expect("Terminated → Terminated 应幂等成功");
    assert_eq!(agent.state(), LifecycleState::Terminated);
}

#[test]
fn test_lifecycle_crash_from_running_to_failed() {
    let bus = EventBus::new();
    let factory = AgentFactory::new(bus);
    let mut agent = factory
        .create_agent(AgentType::RootOrchestrator, "lc-crash-1")
        .expect("创建 Agent 应成功");
    agent.start().expect("Idle → Running");

    agent.crash(); // crash 无错误返回(任意非终态 → Failed)
    assert_eq!(agent.state(), LifecycleState::Failed);
    assert_eq!(agent.meta().status, AgentStatus::Failed);
}

#[test]
fn test_lifecycle_crash_from_terminal_is_noop() {
    let bus = EventBus::new();
    let factory = AgentFactory::new(bus);
    let mut agent = factory
        .create_agent(AgentType::RootOrchestrator, "lc-crash-term-1")
        .expect("创建 Agent 应成功");
    agent.start().expect("Idle → Running");
    agent.complete().expect("Running → Completed");

    agent.crash(); // 已终态,crash 静默忽略
    assert_eq!(
        agent.state(),
        LifecycleState::Completed,
        "Completed 状态 crash 应为 no-op"
    );
}

// ============================================================
// SubTask 8.3: restart 生命周期方法
// ============================================================

#[test]
fn test_lifecycle_restart_from_completed_to_idle() {
    let bus = EventBus::new();
    let factory = AgentFactory::new(bus);
    let mut agent = factory
        .create_agent(AgentType::RootOrchestrator, "lc-restart-1")
        .expect("创建 Agent 应成功");
    agent.start().expect("Idle → Running");
    agent.complete().expect("Running → Completed");

    agent.restart().expect("Completed → Idle 应成功");
    assert_eq!(agent.state(), LifecycleState::Idle);
    assert_eq!(agent.meta().status, AgentStatus::Idle);

    // restart 后可再次 start
    agent.start().expect("restart 后 Idle → Running 应成功");
    assert_eq!(agent.state(), LifecycleState::Running);
}

#[test]
fn test_lifecycle_restart_from_terminated_to_idle() {
    let bus = EventBus::new();
    let factory = AgentFactory::new(bus);
    let mut agent = factory
        .create_agent(AgentType::RootOrchestrator, "lc-restart-term-1")
        .expect("创建 Agent 应成功");
    agent.destroy().expect("Idle → Terminated");

    agent.restart().expect("Terminated → Idle 应成功");
    assert_eq!(agent.state(), LifecycleState::Idle);
}

#[test]
fn test_lifecycle_restart_from_failed_to_idle() {
    let bus = EventBus::new();
    let factory = AgentFactory::new(bus);
    let mut agent = factory
        .create_agent(AgentType::RootOrchestrator, "lc-restart-fail-1")
        .expect("创建 Agent 应成功");
    agent.start().expect("Idle → Running");
    agent.fail().expect("Running → Failed");

    agent.restart().expect("Failed → Idle 应成功");
    assert_eq!(agent.state(), LifecycleState::Idle);
}

// ============================================================
// SubTask 8.3: AgentLifecycle 状态机非法转换(返回 InvalidAgentState)
// ============================================================

#[test]
fn test_invalid_transition_start_from_running() {
    let bus = EventBus::new();
    let factory = AgentFactory::new(bus);
    let mut agent = factory
        .create_agent(AgentType::RootOrchestrator, "inv-start-1")
        .expect("创建 Agent 应成功");
    agent.start().expect("Idle → Running");

    let err = agent.start().expect_err("Running → start 应失败");
    assert!(
        matches!(err, MasError::InvalidAgentState { ref agent_id, .. } if agent_id == "inv-start-1"),
        "非法转换应返回 InvalidAgentState,实际: {err:?}"
    );
}

#[test]
fn test_invalid_transition_pause_from_idle() {
    let bus = EventBus::new();
    let factory = AgentFactory::new(bus);
    let mut agent = factory
        .create_agent(AgentType::RootOrchestrator, "inv-pause-1")
        .expect("创建 Agent 应成功");

    let err = agent.pause().expect_err("Idle → pause 应失败");
    assert!(
        matches!(err, MasError::InvalidAgentState { ref agent_id, .. } if agent_id == "inv-pause-1"),
        "非法转换应返回 InvalidAgentState,实际: {err:?}"
    );
}

#[test]
fn test_invalid_transition_resume_from_running() {
    let bus = EventBus::new();
    let factory = AgentFactory::new(bus);
    let mut agent = factory
        .create_agent(AgentType::RootOrchestrator, "inv-resume-1")
        .expect("创建 Agent 应成功");
    agent.start().expect("Idle → Running");

    let err = agent.resume().expect_err("Running → resume 应失败");
    assert!(
        matches!(err, MasError::InvalidAgentState { .. }),
        "非法转换应返回 InvalidAgentState,实际: {err:?}"
    );
}

#[test]
fn test_invalid_transition_complete_from_idle() {
    let bus = EventBus::new();
    let factory = AgentFactory::new(bus);
    let mut agent = factory
        .create_agent(AgentType::RootOrchestrator, "inv-complete-1")
        .expect("创建 Agent 应成功");

    let err = agent.complete().expect_err("Idle → complete 应失败");
    assert!(
        matches!(err, MasError::InvalidAgentState { .. }),
        "非法转换应返回 InvalidAgentState,实际: {err:?}"
    );
}

#[test]
fn test_invalid_transition_restart_from_running() {
    let bus = EventBus::new();
    let factory = AgentFactory::new(bus);
    let mut agent = factory
        .create_agent(AgentType::RootOrchestrator, "inv-restart-1")
        .expect("创建 Agent 应成功");
    agent.start().expect("Idle → Running");

    let err = agent
        .restart()
        .expect_err("Running → restart 应失败(仅终态可 restart)");
    assert!(
        matches!(err, MasError::InvalidAgentState { .. }),
        "非法转换应返回 InvalidAgentState,实际: {err:?}"
    );
}

#[test]
fn test_invalid_transition_fail_from_completed() {
    let bus = EventBus::new();
    let factory = AgentFactory::new(bus);
    let mut agent = factory
        .create_agent(AgentType::RootOrchestrator, "inv-fail-1")
        .expect("创建 Agent 应成功");
    agent.start().expect("Idle → Running");
    agent.complete().expect("Running → Completed");

    let err = agent
        .fail()
        .expect_err("Completed → fail 应失败(仅 Running/Failed 可 fail)");
    assert!(
        matches!(err, MasError::InvalidAgentState { .. }),
        "非法转换应返回 InvalidAgentState,实际: {err:?}"
    );
}

// ============================================================
// SubTask 8.3: LifecycleState 与 AgentStatus 映射
// ============================================================

#[test]
fn test_lifecycle_state_as_agent_status_mapping() {
    assert_eq!(LifecycleState::Idle.as_agent_status(), AgentStatus::Idle);
    assert_eq!(
        LifecycleState::Running.as_agent_status(),
        AgentStatus::Running
    );
    assert_eq!(
        LifecycleState::Paused.as_agent_status(),
        AgentStatus::Paused
    );
    assert_eq!(
        LifecycleState::Completed.as_agent_status(),
        AgentStatus::Completed
    );
    assert_eq!(
        LifecycleState::Failed.as_agent_status(),
        AgentStatus::Failed
    );
    // Terminated 映射为 Crashed(AgentStatus 无 Terminated 变体,语义为"已终止不可恢复")
    assert_eq!(
        LifecycleState::Terminated.as_agent_status(),
        AgentStatus::Crashed
    );
}

#[test]
fn test_lifecycle_state_is_terminal() {
    assert!(!LifecycleState::Idle.is_terminal());
    assert!(!LifecycleState::Running.is_terminal());
    assert!(!LifecycleState::Paused.is_terminal());
    assert!(LifecycleState::Completed.is_terminal());
    assert!(LifecycleState::Failed.is_terminal());
    assert!(LifecycleState::Terminated.is_terminal());
}

#[test]
fn test_lifecycle_transition_count_increments() {
    let bus = EventBus::new();
    let factory = AgentFactory::new(bus);
    let mut agent = factory
        .create_agent(AgentType::RootOrchestrator, "lc-count-1")
        .expect("创建 Agent 应成功");

    assert_eq!(agent.lifecycle().transition_count(), 0, "初始转换次数为 0");
    agent.start().expect("Idle → Running");
    assert_eq!(agent.lifecycle().transition_count(), 1);
    agent.pause().expect("Running → Paused");
    assert_eq!(agent.lifecycle().transition_count(), 2);
    agent.resume().expect("Paused → Running");
    assert_eq!(agent.lifecycle().transition_count(), 3);
}

// ============================================================
// SubTask 8.4: create_agent 发布 NexusEvent::AgentTaskDelegated
// ============================================================

#[test]
fn test_create_agent_publishes_agent_task_delegated_event() {
    // §4.4 反模式 3 红线:bus.subscribe() 必须在 create_agent 之前同步调用
    let bus = EventBus::new();
    let mut rx = bus.subscribe(); // 先订阅,确保不错过事件
    let factory = AgentFactory::new(bus);

    let agent = factory
        .create_agent(AgentType::RootOrchestrator, "evt-1")
        .expect("创建 Agent 应成功");

    // 验证事件已发布(create_agent 用 publish_blocking 同步发布)
    let event = rx
        .try_recv()
        .expect("应能接收到事件")
        .expect("事件不应为 None");

    match event {
        NexusEvent::AgentTaskDelegated {
            to,
            task_id,
            priority,
            ..
        } => {
            assert_eq!(to, "evt-1", "事件 to 字段应为新 Agent ID");
            assert!(
                task_id.contains("evt-1"),
                "task_id 应包含 agent_id,实际: {task_id}"
            );
            assert_eq!(priority, TaskPriority::Medium, "默认优先级应为 Medium");
        }
        other => panic!("期望 AgentTaskDelegated 事件,实际收到: {other:?}"),
    }

    // 验证 Agent 已创建
    assert_eq!(agent.meta().agent_id, "evt-1");
}

#[test]
fn test_create_agent_event_contains_from_and_deadline() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let factory = AgentFactory::new(bus);

    let _agent = factory
        .create_agent(
            AgentType::MainAgent {
                domain: "backend".into(),
            },
            "evt-2",
        )
        .expect("创建 Agent 应成功");

    let event = rx.try_recv().unwrap().unwrap();
    if let NexusEvent::AgentTaskDelegated {
        from,
        to,
        deadline,
        metadata,
        ..
    } = event
    {
        assert!(!from.is_empty(), "from 字段不应为空");
        assert_eq!(to, "evt-2");
        // deadline 应在未来(创建时设置为 now + 1h)
        assert!(
            deadline > chrono::Utc::now(),
            "deadline 应在未来,实际: {deadline}"
        );
        // metadata.source 应标识来源
        assert!(
            metadata.source.contains("chimera-mas") || metadata.source.contains("AgentFactory"),
            "metadata.source 应标识来源,实际: {}",
            metadata.source
        );
    } else {
        panic!("期望 AgentTaskDelegated 事件");
    }
}

#[test]
fn test_create_agent_without_subscriber_does_not_error() {
    // 无订阅者时 publish_blocking 应返回 Ok(Normal 级事件静默丢弃,非错误)
    let bus = EventBus::new();
    let factory = AgentFactory::new(bus);

    let result = factory.create_agent(AgentType::RootOrchestrator, "no-sub-1");
    assert!(
        result.is_ok(),
        "无订阅者时 create_agent 仍应成功,实际: {:?}",
        result.err()
    );
}

// ============================================================
// SubTask 8.2: AgentFactory 辅助方法
// ============================================================

#[test]
fn test_agent_factory_event_bus_accessor() {
    let bus = EventBus::new();
    let factory = AgentFactory::new(bus.clone());
    // 验证 factory 持有 EventBus 且可访问
    assert_eq!(factory.event_bus().subscriber_count(), 0);
}

#[test]
fn test_agent_factory_registry_prevents_duplicate_across_types() {
    let bus = EventBus::new();
    let factory = AgentFactory::new(bus);
    // 用 RootOrchestrator 创建
    let _first = factory
        .create_agent(AgentType::RootOrchestrator, "cross-1")
        .expect("首次创建应成功");
    // 用不同 AgentType 相同 agent_id 创建应失败
    let err = factory.create_agent(
        AgentType::ExpertAgent {
            specialty: vec!["test".into()],
        },
        "cross-1",
    );
    assert!(
        matches!(err, Err(MasError::AgentAlreadyExists { .. })),
        "跨 AgentType 重复 agent_id 也应返回 AgentAlreadyExists"
    );
}
