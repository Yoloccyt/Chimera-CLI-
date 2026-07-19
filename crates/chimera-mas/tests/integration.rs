//! Task 14 — chimera-mas 集成测试(端到端流程验证)
//!
//! 覆盖 4 个 SubTask 的端到端集成场景,验证 MAS 子系统从用户意图到结果聚集的
//! 完整数据流,以及核心架构红线(深度限制、上下文隔离、HCW 稀疏化)。
//!
//! ## 测试矩阵
//!
//! | SubTask | 测试函数 | 验证场景 |
//! |---------|---------|---------|
//! | 14.1 | test_end_to_end_orchestration_to_execution | delegate → execute_delegation → results 聚集 |
//! | 14.1 | test_end_to_end_publishes_delegated_and_completed_events | 事件发布验证 |
//! | 14.1 | test_end_to_end_very_complex_task_five_agents_parallel | VeryComplex 5 Agent 并行执行 |
//! | 14.2 | test_three_level_recursive_delegation_main_sub_grand | Main → Sub → Grand 三级递归 |
//! | 14.2 | test_delegation_rejected_at_max_depth_five | 深度=5 拒绝(MaxDepthExceeded) |
//! | 14.2 | test_delegation_allowed_at_depth_four_boundary | 深度=4 临界通过 |
//! | 14.3 | test_context_isolation_violation_on_cross_agent_access | 跨 Agent 访问违规 |
//! | 14.3 | test_context_isolation_safe_summary_excludes_sensitive_data | 安全摘要脱敏 |
//! | 14.4 | test_hcw_critical_block_never_compressed | Critical 块永不压缩 |
//! | 14.4 | test_hcw_optional_blocks_dropped_under_sparse_compression | Optional 块可被丢弃 |
//! | 14.4 | test_hcw_sparse_compression_reduces_token_load | 1M 上下文稀疏化后 token 减少 |
//!
//! ## 架构合规
//!
//! - §4.4 反模式 3:`bus.subscribe()` 在 `delegate` / `execute_delegation` 之前同步调用
//! - §6.2:Critical 安全事件(AgentTaskFailed)走 mpsc 双通道(本文件不测试失败场景)
//! - ADR-026 决策 1:最大委托深度 5(MAX_AGENT_DEPTH)
//! - ADR-026 决策 7:不自实现压缩,委托 hcw_window + osa_coordinator
//! - `#![forbid(unsafe_code)]` 保持(chimera-mas crate 级)

#![forbid(unsafe_code)]

use chimera_mas::prelude::*;
use chimera_mas::MAX_AGENT_DEPTH;
use event_bus::{EventBus, EventReceiver, NexusEvent};
use nexus_core::{Task, TaskStatus};
use std::time::Duration;

// ============================================================
// 辅助构造函数
// ============================================================

/// 构造测试用 nexus_core::Task(Pending 状态,无依赖)
fn make_task(task_id: &str, desc: &str) -> Task {
    Task {
        task_id: task_id.into(),
        description: desc.into(),
        status: TaskStatus::Pending,
        dependencies: vec![],
    }
}

/// 构造测试用 AgentTask(指定复杂度,默认 delegation_depth=0)
fn make_agent_task(task_id: &str, complexity: TaskComplexity) -> AgentTask {
    AgentTask::new(
        make_task(task_id, &format!("集成测试任务-{task_id}")),
        complexity,
        1000,
        Duration::from_secs(60),
        QualityLevel::Standard,
    )
}

/// 构造带 delegation_depth 的 AgentTask
fn make_agent_task_with_depth(
    task_id: &str,
    complexity: TaskComplexity,
    depth: usize,
) -> AgentTask {
    make_agent_task(task_id, complexity).with_depth(depth)
}

/// 排空 EventReceiver 缓冲区,返回所有已发布事件
///
/// WHY 同步排水:`delegate` 用 `publish_blocking`(sync)发布 AgentTaskDelegated,
/// `execute_delegation` 用 `publish().await`(async)发布 AgentTaskCompleted。
/// 两者均在调用返回时完成事件入队,`try_recv` 能非阻塞取出全部,
/// 避免 `recv().await` 在无事件时永久挂起。
fn drain_all_events(rx: &mut EventReceiver) -> Vec<NexusEvent> {
    let mut events = Vec::new();
    while let Ok(Some(event)) = rx.try_recv() {
        events.push(event);
    }
    events
}

// ============================================================
// SubTask 14.1: 端到端流程(用户意图 → MAS 编排 → Quest 执行 → 结果聚集)
// ============================================================

/// 验证完整端到端流程:RootOrchestrator.delegate → DelegationExecutor.execute_delegation → 结果聚集
///
/// 流程:
/// 1. 创建 RootOrchestrator
/// 2. 构造 AgentTask(complexity = Complex → 创建 3 个子 Agent)
/// 3. delegate(task) 返回 3 个 AgentHandle
/// 4. 为每个 handle 构造子任务,execute_delegation 并行执行
/// 5. 验证 Vec<TaskResult> 所有 success = true
/// 6. 验证发布 AgentTaskDelegated + AgentTaskCompleted 事件
#[tokio::test]
async fn test_end_to_end_orchestration_to_execution() {
    let bus = EventBus::new();
    // §4.4 反模式 3: subscribe 在 delegate/execute 之前同步调用
    let mut rx = bus.subscribe();

    // 1. 创建 RootOrchestrator
    let orchestrator = RootOrchestrator::new(bus.clone());

    // 2. 构造 Complex 任务(将创建 3 个子 Agent)
    let parent_task = make_agent_task("e2e-complex", TaskComplexity::Complex);

    // 3. delegate 创建子 Agent
    let handles = orchestrator
        .delegate(parent_task)
        .await
        .expect("delegate 应成功");
    assert_eq!(handles.len(), 3, "Complex 任务应创建 3 个子 Agent");

    // 4. 为每个子 Agent 构造子任务,并行执行
    let sub_tasks: Vec<AgentTask> = (0..handles.len())
        .map(|i| make_agent_task(&format!("e2e-sub-{i}"), TaskComplexity::Simple))
        .collect();

    let executor = DelegationExecutor::new(bus.clone(), Duration::from_secs(60));
    let results = executor
        .execute_delegation("root", sub_tasks)
        .await
        .expect("execute_delegation 应成功");

    // 5. 验证所有 success = true
    assert_eq!(results.len(), 3, "结果数应等于子任务数");
    assert!(
        results.iter().all(|r| r.success),
        "所有子任务应成功,实际: {:?}",
        results.iter().map(|r| r.success).collect::<Vec<_>>()
    );

    // 6. 验证事件发布
    let events = drain_all_events(&mut rx);
    let delegated_count = events
        .iter()
        .filter(|e| matches!(e, NexusEvent::AgentTaskDelegated { .. }))
        .count();
    let completed_count = events
        .iter()
        .filter(|e| matches!(e, NexusEvent::AgentTaskCompleted { .. }))
        .count();

    assert_eq!(
        delegated_count, 3,
        "应发布 3 个 AgentTaskDelegated 事件,实际 {delegated_count}"
    );
    assert_eq!(
        completed_count, 3,
        "应发布 3 个 AgentTaskCompleted 事件,实际 {completed_count}"
    );
}

/// 验证端到端流程的事件发布顺序与字段正确性
///
/// AgentTaskDelegated 在 delegate 阶段发布(create_agent 内部),
/// AgentTaskCompleted 在 execute_delegation 阶段发布。
/// 两者均应被订阅者接收,且 Delegated 先于 Completed。
#[tokio::test]
async fn test_end_to_end_publishes_delegated_and_completed_events() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();

    let orchestrator = RootOrchestrator::new(bus.clone());

    // Simple 任务 → 1 个子 Agent → 1 个 Delegated + 1 个 Completed
    let parent_task = make_agent_task("e2e-events", TaskComplexity::Simple);
    let handles = orchestrator
        .delegate(parent_task)
        .await
        .expect("delegate 应成功");
    assert_eq!(handles.len(), 1, "Simple 任务应创建 1 个子 Agent");

    // 执行子任务
    let sub_task = make_agent_task("e2e-events-sub", TaskComplexity::Simple);
    let executor = DelegationExecutor::new(bus.clone(), Duration::from_secs(60));
    let results = executor
        .execute_delegation("root", vec![sub_task])
        .await
        .expect("execute_delegation 应成功");
    assert_eq!(results.len(), 1);
    assert!(results[0].success);

    // 排空事件并验证顺序:Delegated 先于 Completed
    let events = drain_all_events(&mut rx);

    // 找到第一个 Delegated 和第一个 Completed 的索引
    let first_delegated_idx = events
        .iter()
        .position(|e| matches!(e, NexusEvent::AgentTaskDelegated { .. }))
        .expect("应有 AgentTaskDelegated 事件");
    let first_completed_idx = events
        .iter()
        .position(|e| matches!(e, NexusEvent::AgentTaskCompleted { .. }))
        .expect("应有 AgentTaskCompleted 事件");

    assert!(
        first_delegated_idx < first_completed_idx,
        "AgentTaskDelegated 应先于 AgentTaskCompleted 发布(委托先于执行)"
    );

    // 验证 AgentTaskCompleted 事件字段
    let completed_event = events
        .iter()
        .find(|e| matches!(e, NexusEvent::AgentTaskCompleted { .. }))
        .expect("应有 AgentTaskCompleted 事件");
    if let NexusEvent::AgentTaskCompleted {
        from,
        to,
        task_id,
        result_summary,
        ..
    } = completed_event
    {
        assert!(!from.is_empty(), "from 字段应非空(执行子任务的 agent_id)");
        assert_eq!(to, "root", "to 字段应为 parent_id(root)");
        assert_eq!(task_id, "e2e-events-sub", "task_id 应匹配子任务 ID");
        assert!(
            !result_summary.is_empty(),
            "result_summary 应非空(默认 runner 返回摘要)"
        );
    }
}

/// 验证 VeryComplex 任务创建 5 个子 Agent 并并行执行
///
/// VeryComplex 分发策略(spec SubTask 12.2)创建 5 个子 Agent,
/// 5 个子任务通过 FuturesUnordered 并行执行,总耗时应远小于串行累加。
#[tokio::test]
async fn test_end_to_end_very_complex_task_five_agents_parallel() {
    let bus = EventBus::new();
    let orchestrator = RootOrchestrator::new(bus.clone());

    // VeryComplex → 5 个子 Agent
    let parent_task = make_agent_task("e2e-vc", TaskComplexity::VeryComplex);
    let handles = orchestrator
        .delegate(parent_task)
        .await
        .expect("delegate 应成功");
    assert_eq!(handles.len(), 5, "VeryComplex 任务应创建 5 个子 Agent");

    // 验证所有 handle 的 depth = 1(delegation_depth=0 → child depth=1)
    for (i, handle) in handles.iter().enumerate() {
        assert_eq!(
            handle.depth, 1,
            "handle[{i}] depth 应为 1(RootOrchestrator 直接委托)"
        );
    }

    // 构造 5 个子任务并行执行
    let sub_tasks: Vec<AgentTask> = (0..5)
        .map(|i| make_agent_task(&format!("e2e-vc-sub-{i}"), TaskComplexity::Simple))
        .collect();

    let executor = DelegationExecutor::new(bus, Duration::from_secs(60));
    let start = std::time::Instant::now();
    let results = executor
        .execute_delegation("root", sub_tasks)
        .await
        .expect("execute_delegation 应成功");
    let elapsed = start.elapsed();

    // 验证 5 个子任务全部成功
    assert_eq!(results.len(), 5, "5 个子任务应全部返回结果");
    assert!(results.iter().all(|r| r.success), "所有 5 个子任务应成功");

    // 5 个并行任务(默认 runner 立即返回)应在 2s 内完成
    // WHY 2s 容忍度:tokio 调度抖动 + CI 环境性能波动
    assert!(
        elapsed < Duration::from_secs(2),
        "5 个并行任务不应长时间阻塞,实际耗时 {elapsed:?}"
    );
}

// ============================================================
// SubTask 14.2: 3 级递归委托(Main → Sub → Grand)+ 深度限制
// ============================================================

/// 验证 3 级递归委托:Root → Main(depth=1) → Sub(depth=2) → Grand(depth=3)
///
/// 每级委托通过增加 delegation_depth 模拟递归,验证子 Agent depth 递增正确。
/// RootOrchestrator 在每级委托中复用(实际场景中每级 Agent 独立持有 orchestrator 引用)。
#[tokio::test]
async fn test_three_level_recursive_delegation_main_sub_grand() {
    let bus = EventBus::new();
    let orchestrator = RootOrchestrator::new(bus);

    // Level 1: Root → Main(delegation_depth=0 → child depth=1)
    let main_task = make_agent_task_with_depth("l1-main", TaskComplexity::Simple, 0);
    let main_handles = orchestrator
        .delegate(main_task)
        .await
        .expect("L1 委托应成功");
    assert_eq!(main_handles.len(), 1, "Simple 任务应创建 1 个子 Agent");
    let main_handle = &main_handles[0];
    assert_eq!(main_handle.depth, 1, "Main Agent depth 应为 1");

    // Level 2: Main → Sub(delegation_depth=1 → child depth=2)
    let sub_task = make_agent_task_with_depth("l2-sub", TaskComplexity::Simple, 1)
        .with_parent(&main_handle.agent_id);
    let sub_handles = orchestrator
        .delegate(sub_task)
        .await
        .expect("L2 委托应成功");
    assert_eq!(sub_handles.len(), 1);
    let sub_handle = &sub_handles[0];
    assert_eq!(sub_handle.depth, 2, "Sub Agent depth 应为 2");

    // Level 3: Sub → Grand(delegation_depth=2 → child depth=3)
    let grand_task = make_agent_task_with_depth("l3-grand", TaskComplexity::Simple, 2)
        .with_parent(&sub_handle.agent_id);
    let grand_handles = orchestrator
        .delegate(grand_task)
        .await
        .expect("L3 委托应成功");
    assert_eq!(grand_handles.len(), 1);
    let grand_handle = &grand_handles[0];
    assert_eq!(grand_handle.depth, 3, "Grand Agent depth 应为 3");

    // 验证深度严格递增(Main < Sub < Grand)
    assert!(
        main_handle.depth < sub_handle.depth && sub_handle.depth < grand_handle.depth,
        "深度应严格递增: Main({}) < Sub({}) < Grand({})",
        main_handle.depth,
        sub_handle.depth,
        grand_handle.depth
    );
}

/// 验证 delegation_depth=5 时拒绝委托(MaxDepthExceeded)
///
/// ADR-026 决策 1:最大委托深度 5。delegation_depth >= max_depth 时返回错误。
#[tokio::test]
async fn test_delegation_rejected_at_max_depth_five() {
    let bus = EventBus::new();
    let orchestrator = RootOrchestrator::new(bus);

    // delegation_depth=5 >= max_depth=5 → MaxDepthExceeded
    let task = make_agent_task_with_depth("depth-max", TaskComplexity::Simple, 5);
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

    // 验证 MAX_AGENT_DEPTH 常量
    assert_eq!(MAX_AGENT_DEPTH, 5, "MAX_AGENT_DEPTH 常量应为 5");
}

/// 验证 delegation_depth=4 时允许委托(临界通过,子 depth=5)
///
/// depth=4 < max_depth=5,委托成功,子 Agent depth=5(最大允许深度)。
#[tokio::test]
async fn test_delegation_allowed_at_depth_four_boundary() {
    let bus = EventBus::new();
    let orchestrator = RootOrchestrator::new(bus);

    // delegation_depth=4 < max_depth=5 → 允许(子 depth=5,临界值)
    let task = make_agent_task_with_depth("depth-4", TaskComplexity::Simple, 4);
    let handles = orchestrator
        .delegate(task)
        .await
        .expect("delegation_depth=4 应允许委托(临界通过)");

    assert_eq!(handles.len(), 1, "Simple 任务应创建 1 个子 Agent");
    assert_eq!(
        handles[0].depth, 5,
        "子 Agent depth 应为 5(delegation_depth=4 + 1 = 5,临界值)"
    );
}

// ============================================================
// SubTask 14.3: 上下文隔离违规 + 安全摘要脱敏
// ============================================================

/// 验证跨 Agent 上下文访问触发 MasError::ContextIsolationViolation
///
/// Agent A 拥有独立 AgentContext,Agent B 试图通过 ContextIsolationGuard 访问
/// Agent A 的上下文时,应返回 ContextIsolationViolation(§6.2 红线)。
#[tokio::test]
async fn test_context_isolation_violation_on_cross_agent_access() {
    let bus = EventBus::new();

    // 创建 Agent A 和 Agent B 各自独立的上下文(1M Token 等效)
    let _ctx_a =
        AgentContext::new("agent-A", 1_048_576, bus.clone()).expect("Agent A 上下文创建应成功");
    let _ctx_b = AgentContext::new("agent-B", 1_048_576, bus).expect("Agent B 上下文创建应成功");

    // Agent A 的隔离守卫(owner = agent-A)
    let guard_a = ContextIsolationGuard::new("agent-A");

    // Agent B 试图访问 Agent A 的上下文 → 违规
    let result = guard_a.verify_access("agent-B");
    assert!(
        matches!(
            result,
            Err(MasError::ContextIsolationViolation {
                ref agent_id,
                ref context_id
            }) if agent_id == "agent-B" && context_id == "agent-A"
        ),
        "Agent B 访问 Agent A 的上下文应返回 ContextIsolationViolation,实际 = {:?}",
        result
    );

    // Agent A 访问自己的上下文 → 合法
    assert!(
        guard_a.verify_access("agent-A").is_ok(),
        "Agent A 访问自己的上下文应成功"
    );
}

/// 验证 create_safe_summary 提取的安全摘要不泄露完整对话
///
/// raw_conversation 块(含敏感信息)应被排除,status/decision 块可共享。
/// 每段内容截断至 200 字符,防止泄露过多上下文。
#[tokio::test]
async fn test_context_isolation_safe_summary_excludes_sensitive_data() {
    let bus = EventBus::new();
    let mut ctx = AgentContext::new("agent-summary", 1_048_576, bus).expect("上下文创建应成功");

    // 添加敏感原始对话块(不应出现在安全摘要中)
    ctx.add_block(ContextBlock::new(
        "raw_conversation",
        "SENSITIVE_SECRET_PASSWORD_12345 用户密码不应泄露给其他 Agent",
        100,
        ContextPriority::Normal,
    ))
    .expect("添加 raw_conversation 块应成功");

    // 添加可共享的任务状态块(name 含 "status")
    ctx.add_block(ContextBlock::new(
        "task_status",
        "任务进度 50%,已完成代码实现",
        50,
        ContextPriority::High,
    ))
    .expect("添加 task_status 块应成功");

    // 添加可共享的关键决策块(name 含 "decision")
    ctx.add_block(ContextBlock::new(
        "key_decision",
        "决定使用 Tokio 异步运行时",
        30,
        ContextPriority::High,
    ))
    .expect("添加 key_decision 块应成功");

    let guard = ContextIsolationGuard::new("agent-summary");
    let summary = guard
        .create_safe_summary(&ctx)
        .expect("安全摘要应成功(守卫 owner 与上下文所有者一致)");

    // 验证敏感数据不泄露
    assert!(
        !summary.contains("SENSITIVE_SECRET_PASSWORD_12345"),
        "安全摘要不应泄露敏感密码"
    );
    assert!(
        !summary.contains("raw_conversation"),
        "安全摘要不应包含原始对话块内容"
    );

    // 验证可共享内容包含(Markdown 格式)
    assert!(
        summary.contains("Task Status"),
        "安全摘要应包含 Task Status 章节"
    );
    assert!(
        summary.contains("Key Decision"),
        "安全摘要应包含 Key Decision 章节"
    );
    assert!(summary.contains("Tokio"), "安全摘要应包含决策内容(Tokio)");

    // 验证守卫 owner 不匹配时拒绝创建摘要
    let wrong_guard = ContextIsolationGuard::new("agent-other");
    let result = wrong_guard.create_safe_summary(&ctx);
    assert!(
        matches!(result, Err(MasError::ContextIsolationViolation { .. })),
        "守卫 owner 与上下文所有者不匹配时应返回 ContextIsolationViolation"
    );
}

// ============================================================
// SubTask 14.4: 1M Token 上下文经 HCW 稀疏化(128K 实际 + 8× 稀疏)
// ============================================================

/// 验证 Critical 块(system_prompt)永不被 HCW 稀疏化压缩
///
/// ADR-026 决策 7 红线:Critical 块 is_compressible=false,
/// build_prompt 强制将 Critical 块 name 加入 active_names,
/// 即使 OSA context_mask 未选中也保留。
///
/// 测试构造 Regular 档位(context_scope=10),20 个 Optional 块中只选 10 个,
/// 但 Critical 块必须保留。
#[tokio::test]
async fn test_hcw_critical_block_never_compressed() {
    let bus = EventBus::new();
    let mut ctx = AgentContext::new("agent-critical", 1_048_576, bus).expect("1M 上下文创建应成功");

    // 添加 Critical 块(system_prompt)— 永不压缩红线
    let critical_marker = "CRITICAL_SYSTEM_PROMPT_UNIQUE_MARKER_XYZ789";
    ctx.add_block(ContextBlock::new(
        "system_prompt",
        critical_marker,
        500,
        ContextPriority::Critical,
    ))
    .expect("添加 Critical 块应成功");

    // 添加 20 个 Optional 块(wiki_knowledge),总 token 触发 Regular 档位
    // total = 500 + 20*225 = 5000 ∈ [4096, 32768) → complexity=0.4 → Regular, K=10
    for i in 0..20 {
        ctx.add_block(ContextBlock::new(
            format!("wiki_knowledge_{i:02}"),
            format!("optional-wiki-content-{i:02}"),
            225,
            ContextPriority::Optional,
        ))
        .expect("添加 Optional 块应成功");
    }

    let prompt = ctx
        .build_prompt()
        .await
        .expect("build_prompt 应成功(HCW + OSA 稀疏化)");

    // 验证 Critical 块内容一定在 prompt 中(永不压缩红线)
    assert!(
        prompt.contains(critical_marker),
        "Critical 块内容必须出现在 prompt 中(ADR-026 红线:永不压缩)"
    );

    // 验证 prompt 非空
    assert!(!prompt.is_empty(), "稀疏化后 prompt 不应为空");
}

/// 验证 Optional 块(wiki 知识)可被 HCW 稀疏化丢弃
///
/// Regular 档位 context_scope=10,20 个 Optional 块中只保留 10 个,
/// 其余 10 个被丢弃(可完全丢弃,ADR-026 决策 7)。
#[tokio::test]
async fn test_hcw_optional_blocks_dropped_under_sparse_compression() {
    let bus = EventBus::new();
    let mut ctx = AgentContext::new("agent-optional", 1_048_576, bus).expect("1M 上下文创建应成功");

    // 添加 1 个 Critical 块(确保至少有内容保留)
    ctx.add_block(ContextBlock::new(
        "system_prompt",
        "system prompt baseline",
        500,
        ContextPriority::Critical,
    ))
    .expect("添加 Critical 块应成功");

    // 添加 20 个 Optional 块,每个带唯一标记
    // total = 500 + 20*225 = 5000 ∈ [4096, 32768) → Regular, K=10
    let mut markers = Vec::with_capacity(20);
    for i in 0..20 {
        let marker = format!("OPT_MARKER_{i:02}");
        markers.push(marker.clone());
        ctx.add_block(ContextBlock::new(
            format!("wiki_knowledge_{i:02}"),
            marker,
            225,
            ContextPriority::Optional,
        ))
        .expect("添加 Optional 块应成功");
    }

    let prompt = ctx.build_prompt().await.expect("build_prompt 应成功");

    // 统计保留的 Optional 块标记数
    let retained_markers: Vec<&String> = markers.iter().filter(|m| prompt.contains(*m)).collect();

    // 验证部分 Optional 块被丢弃(Regular 档位 K=10,20 个中只保留 10 个)
    assert!(
        retained_markers.len() < markers.len(),
        "应至少有部分 Optional 块被丢弃,保留 {}/{}",
        retained_markers.len(),
        markers.len()
    );
    assert!(
        retained_markers.len() <= 10,
        "Regular 档位 context_scope=10,保留 Optional 数应 <= 10,实际 {}",
        retained_markers.len()
    );
    assert!(
        !retained_markers.is_empty(),
        "应至少保留部分 Optional 块(OSA context_mask 应选中部分文件)"
    );
}

/// 验证 1M Token 上下文经 HCW 稀疏化后实际加载量 < 128K
///
/// 构造 UltraComplex 档位(total_tokens > 131_072) + 文件数 > 1000,
/// OSA context_mask 只选 1000 个文件(UltraComplex context_scope=1000),
/// 其余被稀疏化丢弃,实际加载 token 数 < 128K(1M / 8 稀疏压缩)。
#[tokio::test]
async fn test_hcw_sparse_compression_reduces_token_load() {
    let bus = EventBus::new();
    // 1M Token 等效上下文窗口(128K 实际 + 8× 稀疏压缩)
    let mut ctx =
        AgentContext::new("agent-1m-sparse", 1_048_576, bus).expect("1M 上下文创建应成功");

    // 添加 1050 个 Optional 块,总 token > 128K,触发 UltraComplex 档位
    // total = 1050 * 126 = 132_300 > 131_072 → complexity=0.9 → UltraComplex, K=1000
    // 文件数 1050 > 1000,OSA 只选 1000 个,丢弃 50 个
    let block_count = 1050usize;
    let tokens_per_block = 126usize;
    let total_tokens = block_count * tokens_per_block;
    assert!(
        total_tokens > 131_072,
        "测试前置:total_tokens({total_tokens})应 > 131_072(128K)以触发 UltraComplex"
    );

    let mut markers = Vec::with_capacity(block_count);
    for i in 0..block_count {
        let marker = format!("BLK{i:04}");
        markers.push(marker.clone());
        ctx.add_block(ContextBlock::new(
            format!("file_{i:04}"),
            marker,
            tokens_per_block,
            ContextPriority::Optional,
        ))
        .expect("添加块应成功");
    }

    let prompt = ctx
        .build_prompt()
        .await
        .expect("build_prompt 应成功(1M 上下文稀疏化)");

    // 统计保留的块标记数
    let retained_count = markers.iter().filter(|m| prompt.contains(*m)).count();

    // 验证部分块被稀疏化丢弃(UltraComplex K=1000,1050 中只保留 1000)
    assert!(
        retained_count < block_count,
        "应部分块被稀疏化丢弃,保留 {retained_count}/{block_count}"
    );
    assert!(
        retained_count <= 1000,
        "UltraComplex 档位 context_scope=1000,保留数应 <= 1000,实际 {retained_count}"
    );
    assert!(retained_count > 0, "应至少保留部分块(稀疏化不是全丢弃)");

    // 验证实际加载 token 数 < 128K(1M / 8 稀疏压缩,Ω-Compress)
    // 估算方式:保留块数 * tokens_per_block(每个块 token 数一致)
    let estimated_prompt_tokens = retained_count * tokens_per_block;
    assert!(
        estimated_prompt_tokens < 131_072,
        "稀疏化后实际加载 token 数应 < 128K (131_072),估算 = {estimated_prompt_tokens}(保留 {retained_count} 块 × {tokens_per_block} tokens/块)"
    );

    // 验证 max_tokens 为 1M(上下文窗口配置正确)
    assert_eq!(
        ctx.max_tokens, 1_048_576,
        "上下文窗口应为 1M Token 等效(128K 实际 + 8x 稀疏压缩)"
    );
}
