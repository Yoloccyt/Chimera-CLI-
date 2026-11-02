//! Quest Engine proptest — TTG 不变量属性测试(SubTask 37.7)
//!
//! 验证复杂度评分非负与模式选择一致性。
//!
//! 对应架构层:L9 Quest
//! 对应创新点:TTG(Thinking Toggle Governance)

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use nexus_contracts::BudgetTier;
use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};
use proptest::prelude::*;
use quest_engine::dag::{sort_tasks_topologically, topological_order, validate_dag};
use quest_engine::{QuestError, TtgConfig, TtgGovernor};

/// 构造测试用 Quest
fn make_quest(quest_id: &str, tasks: Vec<Task>) -> Quest {
    Quest {
        quest_id: quest_id.to_string(),
        title: format!("quest {quest_id}"),
        tasks,
        thinking_mode: ThinkingMode::Standard,
        checkpoint_id: None,
        priority: 128,
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

// ============================================================
// 公开 DAG API 契约 —— 黑盒属性测试（P1 接线批）
//
// 与 src/dag.rs 内部 proptest 的分工：内部测试证"实现正确"，本文件证
// "对外承诺的契约成立"（从 crate 外只能看到 pub 面）。两者不是重复：
// 日后重构 `kahn_core` 时，判据正是本文件的契约不得被破。
// ============================================================

/// 构造性无环任务图：节点 i 只依赖下标更小的节点（故必无环）
fn arb_public_acyclic_tasks() -> impl Strategy<Value = Vec<Task>> {
    proptest::collection::vec(any::<u16>(), 1..16).prop_map(|masks| {
        masks
            .iter()
            .enumerate()
            .map(|(i, &mask)| {
                let dependencies = (0..i)
                    .filter(|&j| mask & (1 << j) != 0)
                    .map(|j| format!("task-{j}"))
                    .collect();
                Task {
                    task_id: format!("task-{i}"),
                    description: format!("do task {i}"),
                    status: TaskStatus::Pending,
                    dependencies,
                }
            })
            .collect()
    })
}

/// 循环右移 k 位：制造与依赖序不一致的输入序
fn rotate(tasks: &[Task], k: usize) -> Vec<Task> {
    let n = tasks.len();
    if n == 0 {
        return Vec::new();
    }
    let k = k % n;
    tasks[n - k..]
        .iter()
        .chain(tasks[..n - k].iter())
        .cloned()
        .collect()
}

/// 公开定序 API 的四重契约：排列守恒 + 依赖先行 + 与 id 序同根 + 幂等
#[test]
fn proptest_public_sort_contract_permutation_dep_first_and_idempotent() {
    proptest!(|(tasks in arb_public_acyclic_tasks(), k in 0usize..24usize)| {
        let shuffled = rotate(&tasks, k);
        let sorted = sort_tasks_topologically(shuffled.clone())?;

        // 1. 排列守恒：不增不减
        prop_assert_eq!(sorted.len(), shuffled.len());
        let mut got: Vec<String> = sorted.iter().map(|t| t.task_id.clone()).collect();
        let mut want: Vec<String> = shuffled.iter().map(|t| t.task_id.clone()).collect();
        got.sort();
        want.sort();
        prop_assert_eq!(got, want, "定序不得增删任务");

        // 2. 依赖先行
        let pos: std::collections::HashMap<&str, usize> = sorted
            .iter()
            .enumerate()
            .map(|(i, t)| (t.task_id.as_str(), i))
            .collect();
        for t in &sorted {
            for d in &t.dependencies {
                prop_assert!(
                    pos[d.as_str()] < pos[t.task_id.as_str()],
                    "依赖 {} 必须先于 {}",
                    d,
                    t.task_id
                );
            }
        }

        // 3. 与公开取序 API 同根：不得养出第二套定序语义
        let ids: Vec<String> = sorted.iter().map(|t| t.task_id.clone()).collect();
        prop_assert_eq!(
            ids,
            topological_order(&shuffled)?,
            "实体序必须逐项等于 topological_order 的 id 序"
        );

        // 4. 幂等
        prop_assert_eq!(
            sorted.clone(),
            sort_tasks_topologically(sorted)?,
            "定序必须幂等"
        );
    });
}

/// 三个公开守门 API 对成环输入结论一致：定序不得绕过 `validate_dag` 分支
///
/// WHY 黑盒断三方一致：`create_quest` 同时走 `validate_dag` 与
/// `sort_tasks_topologically`。若日后有人把定序改成"静默忽略环"，
/// 单看定序自己的测试仍会绿（它只会不返 Err），但本测试会因
/// 三方不一致而红——这才能真正钉住"校验未被绕过"。
#[test]
fn proptest_public_api_all_guards_agree_on_cycle() {
    proptest!(|(tasks in arb_public_acyclic_tasks())| {
        prop_assume!(tasks.len() >= 2);
        let mut cyclic = tasks;
        let last_id = cyclic.last().unwrap().task_id.clone();
        let first_id = cyclic[0].task_id.clone();
        // 双向往返边→必成环
        cyclic[0].dependencies.push(last_id);
        let n = cyclic.len() - 1;
        cyclic[n].dependencies.push(first_id);

        let v = validate_dag(&cyclic);
        let t = topological_order(&cyclic);
        let s = sort_tasks_topologically(cyclic.clone());

        prop_assert!(matches!(v, Err(QuestError::CyclicDependency)), "validate_dag 应判环: {v:?}");
        prop_assert!(matches!(t, Err(QuestError::CyclicDependency)), "topological_order 应判环: {t:?}");
        prop_assert!(
            matches!(s, Err(QuestError::CyclicDependency)),
            "sort_tasks_topologically 应判环而非静默返回乱序表: {s:?}"
        );
    });
}
