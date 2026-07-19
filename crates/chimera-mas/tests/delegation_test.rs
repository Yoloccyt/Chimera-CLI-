//! DelegationExecutor 集成测试 — Task 13
//!
//! 覆盖并行委托、超时治理、事件发布、结果聚集等核心行为。
//! 遵循 §4.4 async 反模式红线:
//! - 不持锁跨 `.await`(本模块无锁,天然满足)
//! - `broadcast` 先 `subscribe` 再 `spawn`(每个测试在调用 execute_delegation 前订阅)
//! - Critical 事件(`AgentTaskFailed`)走 mpsc 双通道(`publish_critical`)
//!
//! 测试通过 `DelegationExecutor::with_runner` 注入不同 TaskRunner,
//! 控制子任务的成功/失败/超时行为,验证聚集与事件发布逻辑。

use chimera_mas::delegation::{DelegationExecutor, TaskRunner};
use chimera_mas::prelude::*;
use event_bus::{EventBus, EventSeverity, NexusEvent};
use nexus_core::{Task, TaskStatus};
use std::sync::Arc;
use std::time::Duration;

// ============================================================
// 辅助构造函数
// ============================================================

/// 构造测试用 AgentTask,可指定 task_id 与 acceptable_latency
fn make_task(task_id: &str, latency: Duration) -> AgentTask {
    let inner = Task {
        task_id: task_id.into(),
        description: format!("test task {task_id}"),
        status: TaskStatus::Pending,
        dependencies: vec![],
    };
    AgentTask::new(
        inner,
        TaskComplexity::Simple,
        100,
        latency,
        QualityLevel::Standard,
    )
}

/// 成功 runner — 立即返回 Ok(摘要),模拟快速完成任务
fn success_runner() -> TaskRunner {
    Arc::new(|task: AgentTask| Box::pin(async move { Ok(format!("done-{}", task.inner.task_id)) }))
}

/// 失败 runner — 立即返回 Err,模拟任务执行失败
fn failure_runner() -> TaskRunner {
    Arc::new(|task: AgentTask| {
        Box::pin(async move { Err(format!("exec-fail-{}", task.inner.task_id)) })
    })
}

/// 慢 runner — sleep 5 秒,用于验证超时 cancel(零孤儿调用,§6.1 红线)
fn slow_runner() -> TaskRunner {
    Arc::new(|_task: AgentTask| {
        Box::pin(async move {
            tokio::time::sleep(Duration::from_secs(5)).await;
            Ok("should-not-reach".into())
        })
    })
}

/// 收集 broadcast 接收器中所有 `AgentTaskCompleted` / `AgentTaskFailed` 事件
///
/// WHY 同步遍历:execute_delegation 返回时所有子任务已完成,事件已入 broadcast 通道,
/// `try_recv` 能非阻塞取出全部。避免 `recv().await` 在无事件时永久挂起。
fn drain_agent_task_events(rx: &mut event_bus::EventReceiver) -> Vec<NexusEvent> {
    let mut events = Vec::new();
    while let Ok(Some(event)) = rx.try_recv() {
        if matches!(
            event,
            NexusEvent::AgentTaskCompleted { .. } | NexusEvent::AgentTaskFailed { .. }
        ) {
            events.push(event);
        }
    }
    events
}

// ============================================================
// 测试用例
// ============================================================

#[tokio::test]
async fn test_delegation_executor_new() {
    let bus = EventBus::new();
    let timeout = Duration::from_secs(60);
    let executor = DelegationExecutor::new(bus.clone(), timeout);
    assert_eq!(executor.default_timeout(), timeout);
}

#[tokio::test]
async fn test_execute_delegation_empty_returns_empty() {
    let bus = EventBus::new();
    let executor = DelegationExecutor::new(bus, Duration::from_secs(10));
    let results = executor
        .execute_delegation("parent-1", vec![])
        .await
        .expect("空任务列表应返回空 Vec");
    assert!(results.is_empty(), "空委托应返回空结果列表");
}

#[tokio::test]
async fn test_execute_delegation_single_success() {
    let bus = EventBus::new();
    let executor = DelegationExecutor::with_runner(bus, Duration::from_secs(10), success_runner());
    let task = make_task("t-single", Duration::from_secs(10));
    let results = executor
        .execute_delegation("parent-1", vec![task])
        .await
        .expect("单个成功任务不应返回错误");
    assert_eq!(results.len(), 1);
    assert!(results[0].success, "任务应成功");
    assert_eq!(results[0].task_id, "t-single");
}

#[tokio::test]
async fn test_execute_delegation_parallel_all_success() {
    let bus = EventBus::new();
    let executor = DelegationExecutor::with_runner(bus, Duration::from_secs(10), success_runner());
    let tasks: Vec<AgentTask> = (0..5)
        .map(|i| make_task(&format!("t-{i}"), Duration::from_secs(10)))
        .collect();
    let results = executor
        .execute_delegation("parent-1", tasks)
        .await
        .expect("并行 5 个任务应全部成功");
    assert_eq!(results.len(), 5);
    assert!(results.iter().all(|r| r.success), "所有任务应成功");
}

#[tokio::test]
async fn test_execute_delegation_results_count_matches() {
    let bus = EventBus::new();
    let executor = DelegationExecutor::with_runner(bus, Duration::from_secs(10), success_runner());
    let tasks: Vec<AgentTask> = (0..7)
        .map(|i| make_task(&format!("t-{i}"), Duration::from_secs(10)))
        .collect();
    let expected = tasks.len();
    let results = executor
        .execute_delegation("parent-1", tasks)
        .await
        .expect("委托应成功");
    assert_eq!(results.len(), expected, "结果数应等于子任务数");
}

#[tokio::test]
async fn test_execute_delegation_timeout_produces_failure_result() {
    // 慢 runner sleep 5s,default_timeout 100ms → 超时
    let bus = EventBus::new();
    let executor = DelegationExecutor::with_runner(bus, Duration::from_millis(100), slow_runner());
    let task = make_task("t-slow", Duration::from_secs(10));
    let results = executor
        .execute_delegation("parent-1", vec![task])
        .await
        .expect("超时不应导致整体 Err,而是返回失败 TaskResult");
    assert_eq!(results.len(), 1);
    assert!(!results[0].success, "超时任务应标记为失败");
    assert_eq!(results[0].task_id, "t-slow");
}

#[tokio::test]
async fn test_execute_delegation_timeout_does_not_block_others() {
    // 快 runner 立即成功,验证整体不会长时间阻塞
    let bus = EventBus::new();
    let executor =
        DelegationExecutor::with_runner(bus, Duration::from_millis(100), success_runner());
    let mut tasks: Vec<AgentTask> = Vec::new();
    for i in 0..3 {
        tasks.push(make_task(&format!("t-fast-{i}"), Duration::from_secs(10)));
    }
    let start = std::time::Instant::now();
    let results = executor
        .execute_delegation("parent-1", tasks)
        .await
        .expect("应成功");
    let elapsed = start.elapsed();
    assert_eq!(results.len(), 3);
    assert!(
        elapsed < Duration::from_secs(2),
        "快任务不应长时间阻塞,实际耗时 {elapsed:?}"
    );
}

#[tokio::test]
async fn test_execute_delegation_timeout_with_slow_runner_no_block() {
    // 真正的超时不阻塞测试:慢 runner + 短超时,整体在超时窗口内完成
    let bus = EventBus::new();
    let executor = DelegationExecutor::with_runner(bus, Duration::from_millis(100), slow_runner());
    let mut tasks: Vec<AgentTask> = Vec::new();
    for i in 0..3 {
        tasks.push(make_task(&format!("t-slow-{i}"), Duration::from_secs(10)));
    }
    let start = std::time::Instant::now();
    let results = executor
        .execute_delegation("parent-1", tasks)
        .await
        .expect("应成功");
    let elapsed = start.elapsed();
    assert_eq!(results.len(), 3);
    assert!(results.iter().all(|r| !r.success), "所有慢任务应超时失败");
    // 3 个并行任务,每个超时 100ms,并行执行总耗时应 < 2s(容忍调度抖动)
    assert!(
        elapsed < Duration::from_secs(2),
        "并行超时不应串行等待,实际耗时 {elapsed:?}"
    );
}

#[tokio::test]
async fn test_execute_delegation_publishes_completed_event() {
    let bus = EventBus::new();
    // §4.4 反模式 3:subscribe 必须在 execute_delegation(spawn)之前同步调用
    let mut rx = bus.subscribe();
    let executor = DelegationExecutor::with_runner(bus, Duration::from_secs(10), success_runner());
    let task = make_task("t-evt-ok", Duration::from_secs(10));
    executor
        .execute_delegation("parent-1", vec![task])
        .await
        .expect("应成功");

    let events = drain_agent_task_events(&mut rx);
    let completed_count = events
        .iter()
        .filter(|e| matches!(e, NexusEvent::AgentTaskCompleted { .. }))
        .count();
    assert_eq!(
        completed_count, 1,
        "成功任务应发布 1 个 AgentTaskCompleted 事件"
    );
}

#[tokio::test]
async fn test_execute_delegation_publishes_failed_event_broadcast() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let executor = DelegationExecutor::with_runner(bus, Duration::from_secs(10), failure_runner());
    let task = make_task("t-evt-fail", Duration::from_secs(10));
    executor
        .execute_delegation("parent-1", vec![task])
        .await
        .expect("应成功");

    let events = drain_agent_task_events(&mut rx);
    let failed_count = events
        .iter()
        .filter(|e| matches!(e, NexusEvent::AgentTaskFailed { .. }))
        .count();
    assert_eq!(
        failed_count, 1,
        "失败任务应发布 1 个 AgentTaskFailed 事件(broadcast 通道)"
    );
}

#[tokio::test]
async fn test_execute_delegation_failed_event_critical_severity() {
    // §6.2 红线:AgentTaskFailed 是 Critical 级事件
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let executor = DelegationExecutor::with_runner(bus, Duration::from_secs(10), failure_runner());
    let task = make_task("t-crit", Duration::from_secs(10));
    executor
        .execute_delegation("parent-1", vec![task])
        .await
        .expect("应成功");

    let events = drain_agent_task_events(&mut rx);
    let failed_event = events
        .iter()
        .find(|e| matches!(e, NexusEvent::AgentTaskFailed { .. }))
        .expect("应有 AgentTaskFailed 事件");
    assert_eq!(
        failed_event.severity(),
        EventSeverity::Critical,
        "AgentTaskFailed severity 必须为 Critical(§6.2 红线)"
    );
}

#[tokio::test]
async fn test_execute_delegation_failed_event_via_mpsc() {
    // §6.2 红线:Critical 事件用 mpsc 双通道,subscribe_critical_events 应收到
    let bus = EventBus::new();
    // §4.4 反模式 3:subscribe_critical_events 在 execute_delegation 之前同步调用
    let mut critical_rx = bus.subscribe_critical_events();
    let executor = DelegationExecutor::with_runner(bus, Duration::from_secs(10), failure_runner());
    let task = make_task("t-mpsc", Duration::from_secs(10));
    executor
        .execute_delegation("parent-1", vec![task])
        .await
        .expect("应成功");

    // mpsc 旁路通道应收到 AgentTaskFailed(双通道投递)
    let event = tokio::time::timeout(Duration::from_secs(1), critical_rx.recv())
        .await
        .expect("mpsc 通道应在 1s 内收到 Critical 事件")
        .expect("mpsc receiver 不应关闭");
    assert!(
        matches!(event, NexusEvent::AgentTaskFailed { .. }),
        "mpsc 旁路通道应投递 AgentTaskFailed"
    );
}

#[tokio::test]
async fn test_execute_delegation_mixed_success_failure() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    // 用 failure_runner,所有任务失败,验证失败事件数量
    let executor = DelegationExecutor::with_runner(bus, Duration::from_secs(10), failure_runner());
    let mut tasks: Vec<AgentTask> = Vec::new();
    for i in 0..4 {
        tasks.push(make_task(&format!("t-mix-{i}"), Duration::from_secs(10)));
    }
    let results = executor
        .execute_delegation("parent-1", tasks)
        .await
        .expect("应成功");

    assert_eq!(results.len(), 4);
    assert!(
        results.iter().all(|r| !r.success),
        "failure_runner 下所有任务应失败"
    );

    let events = drain_agent_task_events(&mut rx);
    let failed_count = events
        .iter()
        .filter(|e| matches!(e, NexusEvent::AgentTaskFailed { .. }))
        .count();
    assert_eq!(failed_count, 4, "4 个失败任务应发布 4 个 AgentTaskFailed");
}

#[tokio::test]
async fn test_execute_delegation_uses_acceptable_latency() {
    // 子任务 acceptable_latency = 50ms(短),default_timeout = 10s(长)
    // slow_runner sleep 5s → 50ms 超时(证明用 acceptable_latency 而非 default_timeout)
    let bus = EventBus::new();
    let executor = DelegationExecutor::with_runner(bus, Duration::from_secs(10), slow_runner());
    let task = make_task("t-latency", Duration::from_millis(50));
    let start = std::time::Instant::now();
    let results = executor
        .execute_delegation("parent-1", vec![task])
        .await
        .expect("应成功");
    let elapsed = start.elapsed();

    assert_eq!(results.len(), 1);
    assert!(
        !results[0].success,
        "应因 acceptable_latency=50ms 超时而失败"
    );
    assert!(
        elapsed < Duration::from_secs(2),
        "应在 acceptable_latency(50ms)附近超时,而非等 default_timeout(10s),实际 {elapsed:?}"
    );
}

#[tokio::test]
async fn test_execute_delegation_default_timeout_fallback() {
    // acceptable_latency = 0(未设置),应回退到 default_timeout
    // slow_runner sleep 5s,default_timeout 100ms → 100ms 超时
    let bus = EventBus::new();
    let executor = DelegationExecutor::with_runner(bus, Duration::from_millis(100), slow_runner());
    let task = make_task("t-fallback", Duration::ZERO);
    let start = std::time::Instant::now();
    let results = executor
        .execute_delegation("parent-1", vec![task])
        .await
        .expect("应成功");
    let elapsed = start.elapsed();

    assert_eq!(results.len(), 1);
    assert!(!results[0].success, "应因 default_timeout=100ms 超时而失败");
    assert!(
        elapsed < Duration::from_secs(2),
        "应在 default_timeout(100ms)附近超时,实际 {elapsed:?}"
    );
}

#[tokio::test]
async fn test_execute_delegation_task_result_fields_populated() {
    let bus = EventBus::new();
    let executor = DelegationExecutor::with_runner(bus, Duration::from_secs(10), success_runner());
    let task = make_task("t-fields", Duration::from_secs(10));
    let results = executor
        .execute_delegation("parent-1", vec![task])
        .await
        .expect("应成功");

    assert_eq!(results.len(), 1);
    let r = &results[0];
    assert!(r.success);
    assert_eq!(r.task_id, "t-fields");
    // agent_id 应非空(由 DelegationExecutor 派生,关联 parent_id)
    assert!(!r.agent_id.is_empty(), "agent_id 应被填充");
    // summary 应包含 runner 返回的内容
    assert!(
        r.summary.contains("done-t-fields"),
        "summary 应包含 runner 输出,实际: {}",
        r.summary
    );
    // duration 应被填充(执行有耗时,即便极短,>= 0)
    assert!(r.duration >= Duration::ZERO, "duration 应被填充(>= 0)");
}
