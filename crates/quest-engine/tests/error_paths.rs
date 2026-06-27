//! Quest Engine 错误路径测试(SubTask 37.7)
//!
//! 验证 QuestError 5 个错误路径的触发与处理:
//! TtgOverrideRejected / 复杂度评估异常 / 模式切换失败 / 配置错误 / 预算订阅失败。
//!
//! 对应架构层:L9 Quest
//! 对应创新点:TTG(Thinking Toggle Governance)

#![forbid(unsafe_code)]

use decb_governor::BudgetTier;
use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};
use quest_engine::{QuestError, TtgConfig, TtgGovernor};

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

// ============================================================
// Quest Engine 错误路径测试(5 个)
// ============================================================

/// 错误路径:TtgOverrideRejected — Degraded 档位下覆盖 Deep 被拒绝
///
/// WHY:Degraded 档位预算接近耗尽,Deep 模式会消耗更多 token,
/// 强制覆盖将导致预算溢出。此约束为架构红线(§6 架构红线:预算优先)。
#[test]
fn test_error_ttg_override_rejected_degraded() {
    let governor = TtgGovernor::new(TtgConfig::default());

    // 场景 1:Degraded 档位下尝试覆盖 Deep → 拒绝
    let result = governor.override_mode("quest-1", ThinkingMode::Deep, BudgetTier::Degraded);
    let err = result.expect_err("Degraded 档位覆盖 Deep 应被拒绝");

    match &err {
        QuestError::TtgOverrideRejected {
            quest_id,
            requested_mode,
            current_tier,
            reason,
        } => {
            assert_eq!(quest_id, "quest-1", "quest_id 应为 quest-1");
            assert!(
                requested_mode.contains("Deep"),
                "requested_mode 应包含 'Deep',实际: {requested_mode}"
            );
            assert!(
                current_tier.contains("degraded"),
                "current_tier 应包含 'degraded',实际: {current_tier}"
            );
            assert!(
                reason.contains("Degraded"),
                "reason 应解释 Degraded 约束,实际: {reason}"
            );
        }
        other => panic!("应返回 TtgOverrideRejected,实际: {other:?}"),
    }

    // 场景 2:验证错误消息 Display
    let msg = err.to_string();
    assert!(
        msg.contains("ttg override rejected"),
        "错误消息应包含 'ttg override rejected',实际: {msg}"
    );
    assert!(msg.contains("quest-1"), "错误消息应包含 quest_id");
    assert!(msg.contains("Deep"), "错误消息应包含 requested_mode");
    assert!(msg.contains("degraded"), "错误消息应包含 current_tier");

    // 场景 3:Degraded 档位下覆盖 Fast/Standard 应允许
    let result = governor.override_mode("quest-2", ThinkingMode::Fast, BudgetTier::Degraded);
    assert!(result.is_ok(), "Degraded 档位覆盖 Fast 应允许");
    let result = governor.override_mode("quest-3", ThinkingMode::Standard, BudgetTier::Degraded);
    assert!(result.is_ok(), "Degraded 档位覆盖 Standard 应允许");

    // 场景 4:非 Degraded 档位下覆盖 Deep 应允许
    let result = governor.override_mode("quest-4", ThinkingMode::Deep, BudgetTier::HighTier);
    assert!(result.is_ok(), "HighTier 档位覆盖 Deep 应允许");
    let result = governor.override_mode("quest-5", ThinkingMode::Deep, BudgetTier::LowTier);
    assert!(result.is_ok(), "LowTier 档位覆盖 Deep 应允许");
}

/// 错误路径:TTG 复杂度评估异常(边界条件)
///
/// WHY:复杂度评估应对边界条件(0 任务、超大任务数、空描述)容错,
/// 不 panic、不返回负值。此测试验证边界处理。
#[test]
fn test_error_ttg_invalid_complexity() {
    let governor = TtgGovernor::new(TtgConfig::default());

    // 场景 1:0 任务 Quest → 复杂度非负(标题长度贡献 description_length_factor)
    // WHY 不断言 == 0.0:title 长度 / normalizer 贡献 description_length_factor × 0.3,
    // 0 任务 Quest 仍有非零复杂度(来自标题),只需验证非负
    let empty_quest = make_quest("quest-empty", vec![]);
    let score = governor.evaluate_complexity(&empty_quest);
    assert!(
        score.value() >= 0.0,
        "0 任务 Quest 复杂度应 ≥ 0,实际: {}",
        score.value()
    );

    // 场景 2:超大任务数(1000 个)→ 复杂度非负,不 panic
    let large_quest = make_quest("quest-large", make_parallel_tasks(1000));
    let score = governor.evaluate_complexity(&large_quest);
    assert!(
        score.value() >= 0.0,
        "1000 任务 Quest 复杂度应 ≥ 0,实际: {}",
        score.value()
    );

    // 场景 3:空描述任务 → 复杂度非负
    let empty_desc_quest = make_quest(
        "quest-empty-desc",
        vec![Task {
            task_id: "t-1".into(),
            description: String::new(),
            status: TaskStatus::Pending,
            dependencies: vec![],
        }],
    );
    let score = governor.evaluate_complexity(&empty_desc_quest);
    assert!(
        score.value() >= 0.0,
        "空描述 Quest 复杂度应 ≥ 0,实际: {}",
        score.value()
    );

    // 场景 4:超长描述(10000 字符)→ 复杂度非负,description_length_factor clamp 到 1.0
    let long_desc = "x".repeat(10000);
    let long_desc_quest = make_quest(
        "quest-long-desc",
        vec![Task {
            task_id: "t-1".into(),
            description: long_desc,
            status: TaskStatus::Pending,
            dependencies: vec![],
        }],
    );
    let score = governor.evaluate_complexity(&long_desc_quest);
    assert!(
        score.value() >= 0.0,
        "超长描述 Quest 复杂度应 ≥ 0,实际: {}",
        score.value()
    );

    // 场景 5:复杂度评分不应为 NaN 或 Infinite
    assert!(!score.value().is_nan(), "复杂度评分不应为 NaN");
    assert!(!score.value().is_infinite(), "复杂度评分不应为 Infinite");
}

/// 错误路径:TTG 模式切换失败(滞后机制抑制)
///
/// WHY:档位切换滞后机制(10 秒)会抑制频繁切换,
/// 此测试验证 on_budget_adjusted 在滞后期内返回 None(切换被抑制)。
#[test]
fn test_error_ttg_mode_switch_failed() {
    let governor = TtgGovernor::new(TtgConfig::default());
    let quest = make_quest("quest-lag", make_parallel_tasks(5));

    // 场景 1:首次档位切换应成功
    let result = governor.on_budget_adjusted(
        "quest-lag",
        BudgetTier::HighTier,
        BudgetTier::LowTier,
        &quest,
    );
    assert!(result.is_some(), "首次档位切换应成功(无滞后抑制)");

    // 场景 2:滞后期内再次切换应被抑制(返回 None)
    let result = governor.on_budget_adjusted(
        "quest-lag",
        BudgetTier::LowTier,
        BudgetTier::HighTier,
        &quest,
    );
    assert!(result.is_none(), "滞后期内再次切换应被抑制(返回 None)");

    // 场景 3:相同档位(无变化)应返回 None
    let result = governor.on_budget_adjusted(
        "quest-lag",
        BudgetTier::HighTier,
        BudgetTier::HighTier,
        &quest,
    );
    assert!(result.is_none(), "相同档位(无变化)应返回 None");

    // 场景 4:Degraded 档位强制 Fast(优先级最高)
    let governor2 = TtgGovernor::new(TtgConfig::default());
    let result = governor2.on_budget_adjusted(
        "quest-degraded",
        BudgetTier::HighTier,
        BudgetTier::Degraded,
        &quest,
    );
    if let Some((mode, _reason)) = result {
        assert_eq!(
            mode,
            ThinkingMode::Fast,
            "Degraded 档位应强制 Fast,实际: {mode:?}"
        );
    } else {
        panic!("Degraded 档位切换应成功(首次切换无滞后)");
    }
}

/// 错误路径:TTG 配置错误(无效阈值)
///
/// WHY:TtgConfig 的阈值应满足 simple_task_threshold <= complex_task_threshold,
/// 此测试验证无效配置下的边界行为(不 panic,仍可运行)。
#[test]
fn test_error_ttg_config_invalid() {
    // 场景 1:simple_task_threshold > complex_task_threshold(逻辑倒挂)
    // WHY TtgConfig 无 validate 方法,构造时不校验,但 select_mode 仍可运行
    let inverted_config = TtgConfig::new(10, 3, 10000, 1000.0);
    let governor = TtgGovernor::new(inverted_config);

    let quest = make_quest("quest-inverted", make_parallel_tasks(5));
    let (mode, _reason) = governor.select_mode(&quest, BudgetTier::LowTier);

    // 验证不 panic(配置倒挂时仍返回有效模式)
    assert!(
        matches!(
            mode,
            ThinkingMode::Fast | ThinkingMode::Standard | ThinkingMode::Deep
        ),
        "配置倒挂时仍应返回有效模式,实际: {mode:?}"
    );

    // 场景 2:0 阈值配置
    let zero_config = TtgConfig::new(0, 0, 0, 0.0);
    let governor = TtgGovernor::new(zero_config);
    let quest = make_quest("quest-zero", make_parallel_tasks(1));
    let (mode, _reason) = governor.select_mode(&quest, BudgetTier::LowTier);

    // 验证不 panic
    assert!(
        matches!(
            mode,
            ThinkingMode::Fast | ThinkingMode::Standard | ThinkingMode::Deep
        ),
        "0 阈值配置时仍应返回有效模式,实际: {mode:?}"
    );

    // 场景 3:正常配置验证
    let normal_config = TtgConfig::default();
    assert_eq!(normal_config.simple_task_threshold, 3);
    assert_eq!(normal_config.complex_task_threshold, 10);
    assert_eq!(normal_config.lag_interval_ms, 10_000);
    assert!((normal_config.description_length_normalizer - 1000.0).abs() < 1e-6);

    // 场景 4:验证 TtgConfig::new 构造的字段一致性
    let custom_config = TtgConfig::new(5, 15, 5000, 2000.0);
    assert_eq!(custom_config.simple_task_threshold, 5);
    assert_eq!(custom_config.complex_task_threshold, 15);
    assert_eq!(custom_config.lag_interval_ms, 5000);
    assert!((custom_config.description_length_normalizer - 2000.0).abs() < 1e-6);
}

/// 错误路径:TTG 预算订阅失败(档位变化处理)
///
/// WHY:TTG 订阅 DECB 的 BudgetAdjusted 事件,联动切换思考模式。
/// 此测试验证 on_budget_adjusted 在各种边界条件下的容错处理。
#[test]
fn test_error_ttg_budget_subscription_failed() {
    let governor = TtgGovernor::new(TtgConfig::default());

    // 场景 1:不存在的 quest_id → 仍可处理(创建新条目)
    let quest = make_quest("quest-new", make_parallel_tasks(5));
    let result = governor.on_budget_adjusted(
        "quest-nonexistent",
        BudgetTier::HighTier,
        BudgetTier::LowTier,
        &quest,
    );
    assert!(result.is_some(), "不存在的 quest_id 应可处理(创建新条目)");

    // 场景 2:空 quest_id → 仍可处理
    let result = governor.on_budget_adjusted("", BudgetTier::HighTier, BudgetTier::LowTier, &quest);
    // 验证不 panic(空 quest_id 是边界情况)
    // 注意:结果可能是 Some 或 None,取决于滞后机制
    let _ = result;

    // 场景 3:0 任务 Quest 的档位切换
    let empty_quest = make_quest("quest-empty", vec![]);
    let result = governor.on_budget_adjusted(
        "quest-empty-budget",
        BudgetTier::HighTier,
        BudgetTier::LowTier,
        &empty_quest,
    );
    // 验证不 panic(0 任务是边界情况)
    if let Some((mode, _reason)) = result {
        // 0 任务 Quest 在 LowTier 下应选择 Fast(简单任务)
        assert_eq!(
            mode,
            ThinkingMode::Fast,
            "0 任务 Quest 在 LowTier 下应选择 Fast"
        );
    }

    // 场景 4:验证 current_mode 查询不存在的 quest_id 返回 None
    let mode = governor.current_mode("quest-never-seen");
    assert!(mode.is_none(), "未经过 TTG 决策的 Quest 应返回 None");

    // 场景 5:经过 select_mode 后 current_mode 应返回 Some
    let quest = make_quest("quest-after-select", make_parallel_tasks(5));
    let (mode, _) = governor.select_mode(&quest, BudgetTier::LowTier);
    let queried = governor.current_mode("quest-after-select");
    assert_eq!(
        queried,
        Some(mode),
        "select_mode 后 current_mode 应返回选择的模式"
    );
}
