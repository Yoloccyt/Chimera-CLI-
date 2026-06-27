//! TTG 并发测试 — 验证多线程并发 select_mode 无 panic、无数据竞争
//!
//! 对应 SubTask 35.6
//!
//! # 测试策略
//! - 10 线程并发调用 select_mode,每个线程独立 Quest ID
//! - 验证无 panic、模式结果正确
//! - 共享 Quest ID 的并发调用验证 Mutex 互斥正确性

use std::sync::Arc;
use std::thread;

use decb_governor::BudgetTier;
use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};
use quest_engine::{TtgConfig, TtgGovernor};

/// 构造测试用 Quest
fn make_quest(quest_id: &str, task_count: usize) -> Quest {
    let tasks: Vec<Task> = (0..task_count)
        .map(|idx| Task {
            task_id: format!("task-{idx}"),
            description: format!("do task {idx}"),
            status: TaskStatus::Pending,
            dependencies: vec![],
        })
        .collect();
    Quest {
        quest_id: quest_id.to_string(),
        title: format!("quest {quest_id}"),
        tasks,
        thinking_mode: ThinkingMode::Standard,
        checkpoint_id: None,
    }
}

#[test]
fn test_concurrent_select_mode_independent_quests() {
    let governor = Arc::new(TtgGovernor::new(TtgConfig::default()));
    let mut handles = Vec::new();

    for i in 0..10 {
        let gov = Arc::clone(&governor);
        let handle = thread::spawn(move || {
            let quest = make_quest(&format!("q-concurrent-{i}"), 20);
            let (mode, _) = gov.select_mode(&quest, BudgetTier::HighTier);
            // 复杂 Quest + HighTier → Deep
            assert_eq!(mode, ThinkingMode::Deep);
            // current_mode 应记录 Deep
            assert_eq!(
                gov.current_mode(&format!("q-concurrent-{i}")),
                Some(ThinkingMode::Deep)
            );
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().expect("线程 panic");
    }
}

#[test]
fn test_concurrent_select_mode_shared_quest() {
    let governor = Arc::new(TtgGovernor::new(TtgConfig::default()));
    let quest = Arc::new(make_quest("q-shared", 20));
    let mut handles = Vec::new();

    // 10 线程并发操作同一个 Quest
    for _ in 0..10 {
        let gov = Arc::clone(&governor);
        let q = Arc::clone(&quest);
        let handle = thread::spawn(move || {
            let (mode, _) = gov.select_mode(&q, BudgetTier::HighTier);
            // 所有线程应得到相同结果:Deep
            assert_eq!(mode, ThinkingMode::Deep);
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().expect("线程 panic");
    }

    // 最终状态应为 Deep
    assert_eq!(governor.current_mode("q-shared"), Some(ThinkingMode::Deep));
}

#[test]
fn test_concurrent_override_mode() {
    let governor = Arc::new(TtgGovernor::new(TtgConfig::default()));
    let mut handles = Vec::new();

    for i in 0..10 {
        let gov = Arc::clone(&governor);
        let handle = thread::spawn(move || {
            let quest_id = format!("q-override-{i}");
            // HighTier 档位下覆盖为 Deep,应成功
            let result = gov.override_mode(&quest_id, ThinkingMode::Deep, BudgetTier::HighTier);
            assert!(result.is_ok());
            assert_eq!(gov.current_mode(&quest_id), Some(ThinkingMode::Deep));
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().expect("线程 panic");
    }
}

#[test]
fn test_concurrent_mixed_operations() {
    let governor = Arc::new(TtgGovernor::new(TtgConfig::default()));
    let quest = Arc::new(make_quest("q-mixed", 20));
    let mut handles = Vec::new();

    // 5 线程 select_mode
    for _ in 0..5 {
        let gov = Arc::clone(&governor);
        let q = Arc::clone(&quest);
        handles.push(thread::spawn(move || {
            let (mode, _) = gov.select_mode(&q, BudgetTier::HighTier);
            assert_eq!(mode, ThinkingMode::Deep);
        }));
    }

    // 5 线程 override_mode
    for _ in 0..5 {
        let gov = Arc::clone(&governor);
        handles.push(thread::spawn(move || {
            let _ = gov.override_mode("q-mixed", ThinkingMode::Standard, BudgetTier::HighTier);
        }));
    }

    for handle in handles {
        handle.join().expect("线程 panic");
    }

    // 最终模式应为 Deep 或 Standard(取决于线程交错),但不应 panic
    let final_mode = governor.current_mode("q-mixed");
    assert!(
        matches!(
            final_mode,
            Some(ThinkingMode::Deep) | Some(ThinkingMode::Standard)
        ),
        "final mode = {final_mode:?}"
    );
}

#[test]
fn test_concurrent_budget_adjusted() {
    let governor = Arc::new(TtgGovernor::new(TtgConfig::default()));
    let quest = Arc::new(make_quest("q-budget", 20));
    let mut handles = Vec::new();

    // 5 线程触发 HighTier → LowTier 联动切换
    for _ in 0..5 {
        let gov = Arc::clone(&governor);
        let q = Arc::clone(&quest);
        handles.push(thread::spawn(move || {
            // 首次会触发切换,后续被滞后机制抑制(返回 None)
            let _ =
                gov.on_budget_adjusted("q-budget", BudgetTier::HighTier, BudgetTier::LowTier, &q);
        }));
    }

    for handle in handles {
        handle.join().expect("线程 panic");
    }
    // 不应 panic,最终状态由首次成功的切换决定
    let final_mode = governor.current_mode("q-budget");
    assert!(final_mode.is_some(), "应有模式记录");
}
