//! Quest Engine proptest — TTG 不变量属性测试(SubTask 37.7)
//!
//! 验证复杂度评分非负与模式选择一致性。
//!
//! 对应架构层:L9 Quest
//! 对应创新点:TTG(Thinking Toggle Governance)

#![forbid(unsafe_code)]

use decb_governor::BudgetTier;
use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};
use proptest::prelude::*;
use quest_engine::{TtgConfig, TtgGovernor};

/// 构造测试用 Quest
fn make_quest(quest_id: &str, tasks: Vec<Task>) -> Quest {
    Quest {
        quest_id: quest_id.to_string(),
        title: format!("quest {quest_id}"),
        tasks,
        thinking_mode: ThinkingMode::Standard,
        checkpoint_id: None,
    }
}

/// 构造扁平并行任务(无依赖)
fn make_parallel_tasks(count: usize) -> Vec<Task> {
    (0..count)
        .map(|idx| Task {
            task_id: format!("task-{idx}"),
            description: format!("do task {idx}"),
            status: TaskStatus::Pending,
            dependencies: vec![],
        })
        .collect()
}

// 不变量:复杂度评分 ≥ 0
//
// 生成随机 task_count、dependency_depth、description_length,
// 计算复杂度评分,验证结果始终非负。
//
// WHY 此不变量:复杂度评分是 TTG 模式选择的输入,
// 负值会导致模式选择逻辑异常(§6 架构红线:竞态/异常防护)
#[test]
fn proptest_complexity_score_non_negative() {
    proptest!(|(task_count in 0u32..=100, description_length in 0u32..=10000)| {
        let governor = TtgGovernor::new(TtgConfig::default());

        // 构造随机长度的描述
        let description = "x".repeat(description_length as usize);
        let mut tasks = Vec::with_capacity(task_count as usize);
        for i in 0..task_count {
            tasks.push(Task {
                task_id: format!("task-{i}"),
                description: description.clone(),
                status: TaskStatus::Pending,
                dependencies: vec![],
            });
        }
        let quest = make_quest("q-prop", tasks);

        let score = governor.evaluate_complexity(&quest);

        prop_assert!(
            score.value() >= 0.0,
            "复杂度评分 {} 应 ≥ 0(task_count={}, desc_len={})",
            score.value(),
            task_count,
            description_length
        );
    });
}

// 不变量:模式选择与档位一致性
//
// 验证 TTG 模式选择规则:
// - Degraded → Fast(降级模式强制快速,预算优先)
// - 简单任务(≤ simple_task_threshold)且非 HighTier → Fast
// - 复杂任务(> complex_task_threshold)或 HighTier → Deep
// - 其他 → Standard
//
// WHY 此不变量:确保 TTG 决策可预测,
// 防止预算耗尽时仍选择 Deep 模式导致溢出(§6 架构红线:预算优先)
#[test]
fn proptest_mode_selection_consistency() {
    proptest!(|(task_count in 0u32..=30, tier_idx in 0u8..=2)| {
        let governor = TtgGovernor::new(TtgConfig::default());
        let quest = make_quest("q-mode", make_parallel_tasks(task_count as usize));

        let budget_tier = match tier_idx {
            0 => BudgetTier::Degraded,
            1 => BudgetTier::LowTier,
            _ => BudgetTier::HighTier,
        };

        let (mode, _reason) = governor.select_mode(&quest, budget_tier);

        let config = TtgConfig::default();
        let task_count = task_count as usize;

        // 验证模式选择与规则一致
        match budget_tier {
            BudgetTier::Degraded => {
                prop_assert_eq!(
                    mode,
                    ThinkingMode::Fast,
                    "Degraded 档位应强制 Fast,实际: {:?}",
                    mode
                );
            }
            BudgetTier::LowTier => {
                if task_count <= config.simple_task_threshold {
                    prop_assert_eq!(mode, ThinkingMode::Fast, "LowTier + 简单任务应 Fast");
                } else {
                    prop_assert_eq!(mode, ThinkingMode::Standard, "LowTier + 中等任务应 Standard");
                }
            }
            BudgetTier::HighTier => {
                if task_count > config.complex_task_threshold {
                    prop_assert_eq!(mode, ThinkingMode::Deep, "HighTier + 复杂任务应 Deep");
                } else {
                    prop_assert_eq!(mode, ThinkingMode::Standard, "HighTier + 中等任务应 Standard");
                }
            }
        }
    });
}

// 不变量:Degraded 档位下手动覆盖 Deep 被拒绝
//
// WHY 此不变量:Degraded 档位预算接近耗尽,
// Deep 模式会消耗更多 token 导致溢出(§6 架构红线:预算优先)
#[test]
fn proptest_degraded_rejects_deep_override() {
    proptest!(|(quest_id_suffix in 0u32..=100)| {
        let governor = TtgGovernor::new(TtgConfig::default());
        let quest_id = format!("q-deep-{quest_id_suffix}");

        let result = governor.override_mode(&quest_id, ThinkingMode::Deep, BudgetTier::Degraded);

        prop_assert!(
            result.is_err(),
            "Degraded 档位下覆盖 Deep 应被拒绝(quest_id={})",
            quest_id
        );
    });
}

// 不变量:非 Degraded 档位下手动覆盖 Deep 被允许
#[test]
fn proptest_non_degraded_allows_deep_override() {
    proptest!(|(quest_id_suffix in 0u32..=100)| {
        let governor = TtgGovernor::new(TtgConfig::default());
        let quest_id = format!("q-deep-ok-{quest_id_suffix}");

        let result = governor.override_mode(&quest_id, ThinkingMode::Deep, BudgetTier::HighTier);

        prop_assert!(
            result.is_ok(),
            "HighTier 档位下覆盖 Deep 应被允许(quest_id={})",
            quest_id
        );
    });
}
