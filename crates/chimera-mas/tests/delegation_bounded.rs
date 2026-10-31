//! Delegation concurrency bound + JoinError isolation (Phase 5).

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom
#![forbid(unsafe_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chimera_mas::{AgentTask, DelegationExecutor, QualityLevel, TaskComplexity, TaskRunner};
use event_bus::EventBus;
use nexus_core::{Task, TaskStatus};

fn tasks(count: usize) -> Vec<AgentTask> {
    (0..count)
        .map(|index| {
            AgentTask::new(
                Task {
                    task_id: format!("task-{index}"),
                    description: format!("task {index}"),
                    status: TaskStatus::Pending,
                    dependencies: vec![],
                },
                TaskComplexity::Simple,
                100,
                Duration::from_secs(5),
                QualityLevel::Standard,
            )
        })
        .collect()
}

fn counting_runner(current: Arc<AtomicUsize>, peak: Arc<AtomicUsize>) -> TaskRunner {
    Arc::new(move |_task| {
        let current = Arc::clone(&current);
        let peak = Arc::clone(&peak);
        Box::pin(async move {
            let in_flight = current.fetch_add(1, Ordering::SeqCst) + 1;
            peak.fetch_max(in_flight, Ordering::SeqCst);
            tokio::time::sleep(Duration::from_millis(15)).await;
            current.fetch_sub(1, Ordering::SeqCst);
            Ok("ok".to_string())
        })
    })
}

#[tokio::test]
async fn delegation_peak_inflight_is_bounded() {
    let current = Arc::new(AtomicUsize::new(0));
    let peak = Arc::new(AtomicUsize::new(0));
    let executor = DelegationExecutor::with_runner(
        EventBus::new(),
        Duration::from_secs(5),
        counting_runner(Arc::clone(&current), Arc::clone(&peak)),
    )
    .with_max_concurrency(4);

    let results = executor
        .execute_delegation("parent", tasks(16))
        .await
        .expect("delegation");
    assert_eq!(results.len(), 16);
    assert!(results.iter().all(|result| result.success));
    let observed = peak.load(Ordering::SeqCst);
    assert!(observed <= 4, "peak in-flight {observed} must be <= 4");
}

#[tokio::test]
async fn delegation_join_error_is_isolated() {
    let runner: TaskRunner = Arc::new(|task| {
        Box::pin(async move {
            if task.inner.task_id == "task-3" {
                panic!("injected panic for JoinError coverage");
            }
            Ok("ok".to_string())
        })
    });
    let executor = DelegationExecutor::with_runner(EventBus::new(), Duration::from_secs(5), runner)
        .with_max_concurrency(4);

    let results = executor
        .execute_delegation("parent", tasks(8))
        .await
        .expect("delegation must not fail the whole batch");
    assert_eq!(
        results.len(),
        8,
        "JoinError must be synthesized as a result"
    );
    assert_eq!(
        results.iter().filter(|result| !result.success).count(),
        1,
        "only the panicked task should fail"
    );
}
