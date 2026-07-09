//! TTG EventBus 集成收尾测试 — 验证模式切换事件发布完整覆盖
//!
//! 对应 Phase V Task V-5 [N18]
//!
//! # 测试目标
//! 这是清理 ttg.rs 中重复 `tracing::info!` 前建立的特征化测试(characterization tests)。
//! 事件发布行为已由 `select_mode_and_publish` / `on_budget_adjusted_and_publish` /
//! `override_mode_and_publish` 三个异步入口实现,本测试确保清理重复日志后
//! 所有模式切换路径仍正确发布 `ThinkingModeSwitched` 事件,行为不回退。
//!
//! # 覆盖矩阵
//! | 路径 | 入口 | 期望事件 |
//! |------|------|---------|
//! | select_mode 规则1 (Degraded 强制 Fast) | select_mode_and_publish | Fast |
//! | select_mode 规则2 (简单任务+LowTier) | select_mode_and_publish | Fast |
//! | select_mode 规则3 (中等任务+LowTier) | select_mode_and_publish | Standard |
//! | select_mode 规则4 (复杂任务+HighTier) | select_mode_and_publish | Deep |
//! | on_budget_adjusted (LowTier→HighTier) | on_budget_adjusted_and_publish | Deep |
//! | override_mode (手动覆盖) | override_mode_and_publish | Deep |
//! | reset_override (清除覆盖) | reset_override | 无事件(向后兼容) |

use std::time::Duration;

use decb_governor::BudgetTier;
use event_bus::{EventBus, EventReceiver, NexusEvent};
use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};
use quest_engine::{TtgConfig, TtgGovernor};

// ============================================================
// 测试辅助函数
// ============================================================

/// 构造测试用 Quest(扁平并行任务,无依赖)
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

/// 从订阅器接收一个事件,断言其为 `ThinkingModeSwitched` 并返回关键字段
///
/// WHY 抽取辅助:四个测试用例都有 "recv → match → 断言变体" 的相同结构,
/// 提取后避免重复,且失败信息统一。返回 `(quest_id, to_mode, reason)` 三元组。
async fn assert_mode_switch(rx: &mut EventReceiver) -> (String, String, String) {
    let event = rx.recv().await.expect("应收到 ThinkingModeSwitched 事件");
    match event {
        NexusEvent::ThinkingModeSwitched {
            quest_id,
            to_mode,
            reason,
            ..
        } => (quest_id, to_mode, reason),
        other => panic!("期望 ThinkingModeSwitched,实际收到 {other:?}"),
    }
}

// ============================================================
// 测试 1:select_mode 四条规则各自发布事件
// ============================================================

/// 验证 select_mode 的四条决策规则通过 `select_mode_and_publish` 入口
/// 都能正确发布 `ThinkingModeSwitched` 事件。
///
/// 每条规则用独立 Quest(首次选择从 None 触发发布),共发布 4 个事件,
/// 按 publish 顺序依次接收并断言目标模式。
#[tokio::test]
async fn test_select_mode_publishes_event_for_each_rule() {
    let bus = EventBus::new();
    // WHY subscribe 必须在 with_event_bus(move bus) 之前同步调用,
    // 否则 broadcast 会静默丢失事件(§4.4 async 反模式 #3)
    let mut rx = bus.subscribe();
    let governor = TtgGovernor::with_event_bus(TtgConfig::default(), bus);

    // 规则 1:Degraded 档位强制 Fast(复杂 Quest 也走 Fast,预算优先)
    let q1 = make_quest("rule-1-degraded", 20);
    let r1 = governor
        .select_mode_and_publish("rule-1-degraded", &q1, BudgetTier::Degraded)
        .await
        .expect("规则1 发布不应失败");
    assert!(r1.is_some(), "规则1 首次选择应触发事件");

    // 规则 2:简单任务(≤ simple_task_threshold) + 非高预算档位 → Fast
    let q2 = make_quest("rule-2-simple", 1);
    let r2 = governor
        .select_mode_and_publish("rule-2-simple", &q2, BudgetTier::LowTier)
        .await
        .expect("规则2 发布不应失败");
    assert!(r2.is_some(), "规则2 首次选择应触发事件");

    // 规则 3:中等任务(≤ complex_task_threshold)或 LowTier → Standard
    let q3 = make_quest("rule-3-medium", 5);
    let r3 = governor
        .select_mode_and_publish("rule-3-medium", &q3, BudgetTier::LowTier)
        .await
        .expect("规则3 发布不应失败");
    assert!(r3.is_some(), "规则3 首次选择应触发事件");

    // 规则 4:复杂任务(> complex_task_threshold)或 HighTier → Deep
    let q4 = make_quest("rule-4-complex", 20);
    let r4 = governor
        .select_mode_and_publish("rule-4-complex", &q4, BudgetTier::HighTier)
        .await
        .expect("规则4 发布不应失败");
    assert!(r4.is_some(), "规则4 首次选择应触发事件");

    // 按发布顺序依次断言四个事件的目标模式
    let expected_modes = ["Fast", "Fast", "Standard", "Deep"];
    for expected in expected_modes {
        let (_, to_mode, _) = assert_mode_switch(&mut rx).await;
        assert_eq!(to_mode, expected, "规则事件的目标模式应匹配");
    }
}

// ============================================================
// 测试 2:on_budget_adjusted 切换模式时发布事件
// ============================================================

/// 验证 `on_budget_adjusted_and_publish` 在档位变化触发模式切换时发布事件,
/// 且 `reason` 字段携带 `budget_linkage` 标识,供下游(Parliament)追溯联动源。
#[tokio::test]
async fn test_on_budget_adjusted_publishes_event() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let governor = TtgGovernor::with_event_bus(TtgConfig::default(), bus);

    // 复杂 Quest,LowTier → HighTier 触发 Deep(规则4 联动)
    let quest = make_quest("budget-link-1", 20);
    let result = governor
        .on_budget_adjusted_and_publish(
            "budget-link-1",
            BudgetTier::LowTier,
            BudgetTier::HighTier,
            &quest,
        )
        .await
        .expect("预算联动发布不应失败");
    assert!(result.is_some(), "档位变化应触发模式切换");

    let (quest_id, to_mode, reason) = assert_mode_switch(&mut rx).await;
    assert_eq!(quest_id, "budget-link-1");
    assert_eq!(to_mode, "Deep");
    assert!(
        reason.contains("budget_linkage"),
        "reason 应标识预算联动: {reason}"
    );
}

// ============================================================
// 测试 3:override_mode 发布事件
// ============================================================

/// 验证 `override_mode_and_publish` 在手动覆盖时发布事件,
/// 且 `reason` 字段携带 `manual_override` 标识。
#[tokio::test]
async fn test_override_mode_publishes_event() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let governor = TtgGovernor::with_event_bus(TtgConfig::default(), bus);

    let mode = governor
        .override_mode_and_publish("override-evt-1", ThinkingMode::Deep, BudgetTier::HighTier)
        .await
        .expect("手动覆盖发布不应失败");
    assert_eq!(mode, ThinkingMode::Deep);

    let (quest_id, to_mode, reason) = assert_mode_switch(&mut rx).await;
    assert_eq!(quest_id, "override-evt-1");
    assert_eq!(to_mode, "Deep");
    assert!(
        reason.contains("manual_override"),
        "reason 应标识手动覆盖: {reason}"
    );
}

// ============================================================
// 测试 4:reset_override 不发布事件(向后兼容)
// ============================================================

/// 验证 `reset_override` 不发布 `ThinkingModeSwitched` 事件。
///
/// WHY 向后兼容:reset 是清除手动覆盖标记、恢复自动决策语义,并非模式切换。
/// `ThinkingModeReset` 事件变体延后至 v1.2.0,当前 reset 保持静默(仅记录诊断日志)。
/// 本测试作回归保护:防止将来误给 reset_override 增加事件发布而破坏契约。
#[tokio::test]
async fn test_reset_override_no_event() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let governor = TtgGovernor::with_event_bus(TtgConfig::default(), bus);

    // 先手动覆盖,产生一个 ThinkingModeSwitched 事件
    governor
        .override_mode_and_publish("reset-evt-1", ThinkingMode::Deep, BudgetTier::HighTier)
        .await
        .expect("覆盖发布不应失败");
    assert!(governor.is_overridden("reset-evt-1"));
    // 收掉覆盖事件,清空队列
    let _ = assert_mode_switch(&mut rx).await;

    // 执行 reset_override(同步方法,不触碰 EventBus)
    governor.reset_override("reset-evt-1");
    assert!(
        !governor.is_overridden("reset-evt-1"),
        "reset 后覆盖标记应清除"
    );

    // 验证短期内无新事件到达:reset 不应发布 ThinkingModeSwitched
    let result = tokio::time::timeout(Duration::from_millis(150), rx.recv()).await;
    assert!(
        result.is_err(),
        "reset_override 不应发布 ThinkingModeSwitched 事件"
    );
}
