//! WSJF 优先级调度集成测试 (§8 / ADR-027 决策 4)
//!
//! 覆盖:
//! - WSJF 评分公式与输入归一(clamp)
//! - 评分 → 优先级阈值映射(默认 + 自定义)
//! - 优先级秩往返与排序
//! - 饥饿老化线性提权(纯函数)
//! - Critical 抢占 Low 规则
//! - `PriorityScheduler` 入队/出队排序、WSJF 次序、动态重排

use chimera_mas::delegation::{AgentTask, QualityLevel, TaskComplexity};
use chimera_mas::scheduler::{
    aged_priority_rank, priority_from_rank, priority_rank, score_to_priority, should_preempt,
    wsjf_score, PriorityScheduler, PriorityThresholds, WsjfInput, WsjfWeights,
};
use event_bus::TaskPriority;
use nexus_core::{Task, TaskStatus};
use std::time::Duration;

/// 构造指定优先级的 AgentTask(默认 Medium 复杂度)。
fn make_task(id: &str, priority: TaskPriority) -> AgentTask {
    let task = Task {
        task_id: id.into(),
        description: format!("task {id}"),
        status: TaskStatus::Pending,
        dependencies: vec![],
    };
    AgentTask::new(
        task,
        TaskComplexity::Medium,
        1000,
        Duration::from_secs(60),
        QualityLevel::Standard,
    )
    .with_priority(priority)
}

// ============================================================
// WSJF 评分公式与归一
// ============================================================

#[test]
fn test_wsjf_score_equal_weights() {
    // (2+3+4+1)/2 = 5.0
    let input = WsjfInput::new(2.0, 3.0, 4.0, 1.0, 2.0);
    let score = wsjf_score(&input, &WsjfWeights::default());
    assert!((score - 5.0).abs() < 1e-9, "score = {score}");
}

#[test]
fn test_wsjf_score_custom_weights() {
    // w=(2,1,1,1), 各项=1, size=1 → (2+1+1+1)/1 = 5
    let input = WsjfInput::new(1.0, 1.0, 1.0, 1.0, 1.0);
    let score = wsjf_score(&input, &WsjfWeights::new(2.0, 1.0, 1.0, 1.0));
    assert!((score - 5.0).abs() < 1e-9, "score = {score}");
}

#[test]
fn test_wsjf_input_clamps_to_1_10() {
    // 超界输入被钳制: 20→10, 0→1; job_size 0→1
    let input = WsjfInput::new(20.0, 0.0, 5.0, 5.0, 0.0);
    assert!((input.business_value - 10.0).abs() < f64::EPSILON);
    assert!((input.time_criticality - 1.0).abs() < f64::EPSILON);
    assert!((input.job_size - 1.0).abs() < f64::EPSILON);
}

#[test]
fn test_wsjf_higher_value_higher_score() {
    let low = WsjfInput::new(2.0, 2.0, 2.0, 2.0, 5.0);
    let high = WsjfInput::new(9.0, 9.0, 9.0, 9.0, 5.0);
    let w = WsjfWeights::default();
    assert!(wsjf_score(&high, &w) > wsjf_score(&low, &w));
}

#[test]
fn test_wsjf_larger_job_size_lower_score() {
    let small = WsjfInput::new(8.0, 8.0, 8.0, 8.0, 2.0);
    let large = WsjfInput::new(8.0, 8.0, 8.0, 8.0, 10.0);
    let w = WsjfWeights::default();
    assert!(wsjf_score(&small, &w) > wsjf_score(&large, &w));
}

// ============================================================
// 阈值映射
// ============================================================

#[test]
fn test_score_to_priority_default_boundaries() {
    let t = PriorityThresholds::default();
    // 边界值(>=)
    assert_eq!(score_to_priority(8.0, &t), TaskPriority::Critical);
    assert_eq!(score_to_priority(7.99, &t), TaskPriority::High);
    assert_eq!(score_to_priority(5.0, &t), TaskPriority::High);
    assert_eq!(score_to_priority(4.99, &t), TaskPriority::Medium);
    assert_eq!(score_to_priority(2.5, &t), TaskPriority::Medium);
    assert_eq!(score_to_priority(2.49, &t), TaskPriority::Low);
}

#[test]
fn test_score_to_priority_custom_thresholds() {
    let t = PriorityThresholds {
        t1_critical: 100.0,
        t2_high: 50.0,
        t3_medium: 10.0,
    };
    assert_eq!(score_to_priority(120.0, &t), TaskPriority::Critical);
    assert_eq!(score_to_priority(60.0, &t), TaskPriority::High);
    assert_eq!(score_to_priority(20.0, &t), TaskPriority::Medium);
    assert_eq!(score_to_priority(5.0, &t), TaskPriority::Low);
}

// ============================================================
// 优先级秩
// ============================================================

#[test]
fn test_priority_rank_strict_ordering() {
    assert!(priority_rank(TaskPriority::Critical) > priority_rank(TaskPriority::High));
    assert!(priority_rank(TaskPriority::High) > priority_rank(TaskPriority::Medium));
    assert!(priority_rank(TaskPriority::Medium) > priority_rank(TaskPriority::Low));
}

#[test]
fn test_priority_rank_roundtrip() {
    for p in [
        TaskPriority::Low,
        TaskPriority::Medium,
        TaskPriority::High,
        TaskPriority::Critical,
    ] {
        assert_eq!(priority_from_rank(priority_rank(p)), p);
    }
}

// ============================================================
// 饥饿老化(纯函数)
// ============================================================

#[test]
fn test_aged_rank_linear_and_capped() {
    let th = Duration::from_secs(60);
    assert_eq!(aged_priority_rank(TaskPriority::Low, Duration::ZERO, th), 0);
    assert_eq!(
        aged_priority_rank(TaskPriority::Low, Duration::from_secs(120), th),
        2
    );
    // 封顶 Critical(3), 不溢出
    assert_eq!(
        aged_priority_rank(TaskPriority::High, Duration::from_secs(6000), th),
        3
    );
}

#[test]
fn test_aged_rank_zero_threshold_no_aging() {
    assert_eq!(
        aged_priority_rank(
            TaskPriority::Low,
            Duration::from_secs(10000),
            Duration::ZERO
        ),
        0
    );
}

// ============================================================
// 抢占
// ============================================================

#[test]
fn test_should_preempt_matrix() {
    assert!(should_preempt(TaskPriority::Low, TaskPriority::Critical));
    assert!(!should_preempt(TaskPriority::Low, TaskPriority::High));
    assert!(!should_preempt(
        TaskPriority::Medium,
        TaskPriority::Critical
    ));
    assert!(!should_preempt(TaskPriority::High, TaskPriority::Critical));
    assert!(!should_preempt(
        TaskPriority::Critical,
        TaskPriority::Critical
    ));
}

// ============================================================
// PriorityScheduler
// ============================================================

#[test]
fn test_scheduler_empty() {
    let mut s = PriorityScheduler::new();
    assert!(s.is_empty());
    assert_eq!(s.len(), 0);
    assert!(s.dequeue().is_none());
    assert!(s.peek_effective_priority().is_none());
}

#[test]
fn test_scheduler_len_after_enqueue() {
    let mut s = PriorityScheduler::new();
    let mid = WsjfInput::new(5.0, 5.0, 5.0, 5.0, 5.0);
    s.enqueue(make_task("a", TaskPriority::Medium), &mid);
    s.enqueue(make_task("b", TaskPriority::Low), &mid);
    assert_eq!(s.len(), 2);
    assert!(!s.is_empty());
}

#[test]
fn test_scheduler_dequeues_by_priority_first() {
    let mut s = PriorityScheduler::new();
    let mid = WsjfInput::new(5.0, 5.0, 5.0, 5.0, 5.0);
    // 故意乱序入队
    s.enqueue(make_task("low", TaskPriority::Low), &mid);
    s.enqueue(make_task("crit", TaskPriority::Critical), &mid);
    s.enqueue(make_task("med", TaskPriority::Medium), &mid);
    s.enqueue(make_task("high", TaskPriority::High), &mid);

    // 出队顺序: Critical → High → Medium → Low
    assert_eq!(s.dequeue().unwrap().inner.task_id, "crit");
    assert_eq!(s.dequeue().unwrap().inner.task_id, "high");
    assert_eq!(s.dequeue().unwrap().inner.task_id, "med");
    assert_eq!(s.dequeue().unwrap().inner.task_id, "low");
    assert!(s.dequeue().is_none());
}

#[test]
fn test_scheduler_critical_never_after_low() {
    // 不变量: 只要 Critical 与 Low 同时在队, Critical 先出
    let mut s = PriorityScheduler::new();
    let mid = WsjfInput::new(5.0, 5.0, 5.0, 5.0, 5.0);
    s.enqueue(make_task("low", TaskPriority::Low), &mid);
    s.enqueue(make_task("crit", TaskPriority::Critical), &mid);
    assert_eq!(s.dequeue().unwrap().inner.task_id, "crit");
}

#[test]
fn test_scheduler_wsjf_breaks_ties_within_same_priority() {
    let mut s = PriorityScheduler::new();
    // 同为 Medium, WSJF 不同
    let low_wsjf = WsjfInput::new(2.0, 2.0, 2.0, 2.0, 10.0); // 分低
    let high_wsjf = WsjfInput::new(9.0, 9.0, 9.0, 9.0, 1.0); // 分高
    s.enqueue(make_task("weak", TaskPriority::Medium), &low_wsjf);
    s.enqueue(make_task("strong", TaskPriority::Medium), &high_wsjf);
    // WSJF 高者先出
    assert_eq!(s.dequeue().unwrap().inner.task_id, "strong");
    assert_eq!(s.dequeue().unwrap().inner.task_id, "weak");
}

#[test]
fn test_scheduler_recompute_from_wsjf_overrides_priority() {
    let mut s = PriorityScheduler::new();
    // 初始 Low, 但 WSJF 极高(全 10, size 1 → 40 >= t1=8 → Critical)
    let high_wsjf = WsjfInput::new(10.0, 10.0, 10.0, 10.0, 1.0);
    s.enqueue(make_task("sleeper", TaskPriority::Low), &high_wsjf);

    // 重排前有效优先级 = Low
    assert_eq!(s.peek_effective_priority(), Some(TaskPriority::Low));

    // 依据 WSJF 动态重排
    s.recompute_from_wsjf();
    let task = s.dequeue().unwrap();
    assert_eq!(task.priority, TaskPriority::Critical);
}

#[test]
fn test_scheduler_peek_reflects_top_priority() {
    let mut s = PriorityScheduler::new();
    let mid = WsjfInput::new(5.0, 5.0, 5.0, 5.0, 5.0);
    s.enqueue(make_task("m", TaskPriority::Medium), &mid);
    s.enqueue(make_task("c", TaskPriority::Critical), &mid);
    assert_eq!(s.peek_effective_priority(), Some(TaskPriority::Critical));
    // peek 不移除
    assert_eq!(s.len(), 2);
}
