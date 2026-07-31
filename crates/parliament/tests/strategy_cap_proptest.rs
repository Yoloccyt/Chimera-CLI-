//! StrategyCapGuard 属性测试与审议集成测试 — 推理悖论红线风控(M3)
//!
//! # 覆盖性质
//! 1. **封顶档位单调有界**:任意 ratio 序列下,封顶恒在
//!    {FastPath, Simplified, Full} 内,且每次变更恰好一档
//! 2. **min 上界语义**:`apply(s)` 结果既不高于输入策略也不高于当前封顶
//! 3. **Skeptic 不变量**:封顶降到任何档位,恶意提案仍被 Vetoed
//!    (红队防线不可被风控降级绕过)
//! 4. **封顶生效**:Full 策略在 Simplified 封顶下实际走 3 角色辩论

use event_bus::{EventBus, NexusEvent};
use nexus_contracts::{ActivationStrategy, ParliamentPolicy};
use nexus_core::{Quest, Task, TaskStatus, ThinkingMode};
use parliament::{Parliament, ParliamentConfig, Proposal, StrategyCapConfig, StrategyCapGuard};
use proptest::prelude::*;
use std::time::Duration;

// ============================================================
// 辅助构造
// ============================================================

/// 无驻留时间限制的守卫配置(便于状态机快速转换)
fn fast_cap_config() -> StrategyCapConfig {
    StrategyCapConfig {
        enter_consecutive: 3,
        exit_consecutive: 5,
        exit_ratio_factor: 0.8,
        min_dwell_ms: 0,
    }
}

fn make_quest() -> Quest {
    Quest {
        quest_id: "q-cap".into(),
        title: "封顶测试 Quest".into(),
        tasks: vec![Task {
            task_id: "t-0".into(),
            description: "任务".into(),
            status: TaskStatus::Pending,
            dependencies: vec![],
        }],
        thinking_mode: ThinkingMode::Fast,
        checkpoint_id: None,
        priority: 128,
    }
}

/// 驱动守卫降档 N 次(每次需 enter_consecutive 个越阈报告)
fn lower_cap_times(guard: &StrategyCapGuard, times: usize) {
    for _ in 0..times {
        for _ in 0..guard.config().enter_consecutive {
            guard.observe(2.0, 1.0);
        }
    }
}

// ============================================================
// 属性测试
// ============================================================

proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]

    /// 性质 1:任意 ratio 序列下封顶档位单调有界
    ///
    /// - 封顶恒为三档之一(枚举天然保证,断言判别值域 [1,3])
    /// - 每次 CapChange 的档位差恰为 1(不跳档)
    /// - 降档方向 new < old,升档方向 new > old
    #[test]
    fn prop_cap_transitions_are_single_step_and_bounded(
        ratios in prop::collection::vec(0.0f64..3.0, 1..200)
    ) {
        let guard = StrategyCapGuard::new(fast_cap_config());
        for ratio in ratios {
            if let Some(change) = guard.observe(ratio, 1.0) {
                let old = change.old_cap as u8;
                let new = change.new_cap as u8;
                prop_assert!((1..=3).contains(&old) && (1..=3).contains(&new));
                prop_assert_eq!(
                    old.abs_diff(new), 1,
                    "封顶变更必须恰好一档: {} -> {}", old, new
                );
            }
            let cap = guard.current_cap() as u8;
            prop_assert!((1..=3).contains(&cap), "封顶应恒在三档内");
        }
    }

    /// 性质 2:apply 的 min 上界语义 — 结果不高于输入策略且不高于封顶
    #[test]
    fn prop_apply_never_exceeds_strategy_or_cap(
        ratios in prop::collection::vec(0.0f64..3.0, 0..50),
        strategy_idx in 0usize..3
    ) {
        let guard = StrategyCapGuard::new(fast_cap_config());
        for ratio in ratios {
            guard.observe(ratio, 1.0);
        }
        let strategy = ActivationStrategy::ALL[strategy_idx];
        let applied = guard.apply(strategy);
        prop_assert!(
            (applied as u8) <= (strategy as u8),
            "生效策略不得高于请求策略"
        );
        prop_assert!(
            (applied as u8) <= (guard.current_cap() as u8),
            "生效策略不得高于封顶"
        );
    }

    /// 性质 3(M3-T3.1):滞后带内序列永不触发封顶变更
    ///
    /// ratio 恒在 (exit_factor×t, t] 区间(带内)时,双计数器被持续清零,
    /// 封顶应始终保持初始 Full(防抖振设计的核心保证)。
    #[test]
    fn prop_hysteresis_band_never_changes_cap(
        // 严格带内:(0.8, 1.0](阈值 t=1.0,exit_factor=0.8)
        ratios in prop::collection::vec(0.800001f64..=1.0, 1..100)
    ) {
        let guard = StrategyCapGuard::new(fast_cap_config());
        for ratio in ratios {
            prop_assert!(
                guard.observe(ratio, 1.0).is_none(),
                "滞后带内不得触发封顶变更"
            );
        }
        prop_assert_eq!(guard.current_cap(), ActivationStrategy::Full);
    }

    /// 性质 4(M3-T3.1):持续越阈序列收敛到 FastPath 后饱和不再变更
    ///
    /// 模拟推理悖论持续恶化场景:封顶单调下降至最低档后保持稳定
    /// (不会环绕/回升),且总变更次数恰为 2(Full→Simplified→FastPath)。
    #[test]
    fn prop_sustained_over_threshold_converges_to_fastpath(
        ratios in prop::collection::vec(1.000001f64..5.0, 10..100)
    ) {
        let guard = StrategyCapGuard::new(fast_cap_config());
        let mut changes = 0usize;
        for ratio in ratios {
            if guard.observe(ratio, 1.0).is_some() {
                changes += 1;
            }
        }
        // ≥10 次连续越阈(enter=3)必然完成两次降档后饱和
        prop_assert_eq!(changes, 2, "持续越阈应恰降两档后饱和");
        prop_assert_eq!(guard.current_cap(), ActivationStrategy::FastPath);
    }
}

// ============================================================
// 审议集成测试(Skeptic 不变量 + 封顶生效)
// ============================================================

/// Skeptic 不变量:封顶降到最低档(FastPath),恶意提案仍被否决
///
/// WHY 关键:推理悖论风控降低的是审议深度,不是安全防线。
/// 若封顶能绕过 Skeptic,风控本身就成了攻击面(策略性利用红线)。
#[tokio::test]
async fn test_skeptic_veto_survives_fastpath_cap() {
    let config = ParliamentConfig {
        strategy_cap: fast_cap_config(),
        ..Default::default()
    };
    let parliament = Parliament::new(config, EventBus::new());

    // 封顶降到 FastPath(两轮降档)
    lower_cap_times(parliament.strategy_cap(), 2);
    assert_eq!(
        parliament.strategy_cap().current_cap(),
        ActivationStrategy::FastPath
    );

    // 恶意提案在 FastPath 封顶下仍应被 Skeptic 否决
    let quest = make_quest();
    let proposal = Proposal::new("p-mal", "q-cap", "sudo rm -rf /", 0.9);
    let consensus = parliament.deliberate(&quest, &proposal).await.unwrap();
    assert!(
        consensus.is_vetoed(),
        "FastPath 封顶不得绕过 Skeptic 否决,实际: {consensus:?}"
    );
}

/// 封顶生效:Full 策略在 Simplified 封顶下实际走 3 角色辩论
///
/// 通过 DebateStarted 事件的 participant_count 验证实际审议深度。
#[tokio::test]
async fn test_cap_lowers_effective_strategy_to_simplified() {
    let config = ParliamentConfig {
        strategy_cap: fast_cap_config(),
        ..Default::default()
    };
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let parliament = Parliament::new(config, bus);

    // 封顶降一档:Full → Simplified
    lower_cap_times(parliament.strategy_cap(), 1);

    // 显式请求 Full 策略,应被封顶压到 Simplified(3 参与者)
    let quest = make_quest();
    let proposal = Proposal::new("p-cap", "q-cap", "常规提案", 0.2);
    let policy = ParliamentPolicy::static_policy(ActivationStrategy::Full);
    parliament
        .deliberate_with_policy(&quest, &proposal, &policy)
        .await
        .unwrap();

    // DebateStarted.participant_count 应为 3(Simplified),而非 5(Full)
    let mut participant_count = None;
    for _ in 0..30 {
        match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
            Ok(Ok(NexusEvent::DebateStarted {
                participant_count: count,
                ..
            })) => {
                participant_count = Some(count);
                break;
            }
            Ok(_) => continue,
            Err(_) => break,
        }
    }
    assert_eq!(
        participant_count,
        Some(3),
        "Full 策略在 Simplified 封顶下应实际走 3 角色辩论"
    );
}

/// 封顶未触发时行为不变(向后兼容):默认封顶 Full,Full 策略走 5 角色
#[tokio::test]
async fn test_default_cap_preserves_full_debate_behavior() {
    let bus = EventBus::new();
    let mut rx = bus.subscribe();
    let parliament = Parliament::new(ParliamentConfig::default(), bus);

    let quest = make_quest();
    let proposal = Proposal::new("p-default", "q-cap", "常规提案", 0.2);
    parliament.deliberate(&quest, &proposal).await.unwrap();

    let mut participant_count = None;
    for _ in 0..30 {
        match tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
            Ok(Ok(NexusEvent::DebateStarted {
                participant_count: count,
                ..
            })) => {
                participant_count = Some(count);
                break;
            }
            Ok(_) => continue,
            Err(_) => break,
        }
    }
    assert_eq!(
        participant_count,
        Some(5),
        "默认封顶(Full)下行为应与既有 5 角色辩论完全一致"
    );
}
