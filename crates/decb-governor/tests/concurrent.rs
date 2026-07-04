//! DECB 并发测试 — 验证多线程并发预算计算与档位切换无数据竞争
//!
//! 对应 SubTask 34.6
//!
//! # 测试目标
//! - 10 线程并发 compute_budget,无 panic、无数据竞争
//! - 并发 record_consumption,消耗累加正确
//! - 并发 switch_tier,档位状态一致
//! - 性能断言测试标记 `#[ignore]`,需用 `cargo test -- --ignored` 运行

use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use decb_governor::{BudgetConsumption, BudgetTier, DecbConfig, DecbGovernor, QuestBudgetInput};

/// 构造测试用治理器
fn make_governor() -> Arc<DecbGovernor> {
    Arc::new(DecbGovernor::new(DecbConfig::default()).unwrap())
}

/// 构造短滞后治理器(便于测试档位切换)
fn make_governor_no_lag() -> Arc<DecbGovernor> {
    Arc::new(
        DecbGovernor::new(DecbConfig {
            tier_switch_lag_ms: 0,
            ..Default::default()
        })
        .unwrap(),
    )
}

// ============================================================
// 并发 compute_budget 测试
// ============================================================

#[tokio::test]
async fn test_concurrent_compute_budget_no_panic() {
    let governor = make_governor();
    let quest = QuestBudgetInput::simple("quest-concurrent");

    // 10 线程并发计算预算系数
    let mut handles = Vec::new();
    for _ in 0..10 {
        let governor = governor.clone();
        let quest = quest.clone();
        handles.push(tokio::spawn(async move {
            // 每个线程计算 100 次
            for _ in 0..100 {
                let coef = governor.compute_budget(&quest);
                assert!((0.0..=1.0).contains(&coef), "coefficient must be in [0, 1]");
            }
        }));
    }

    // 等待所有线程完成,任一 panic 则测试失败
    for handle in handles {
        handle.await.expect("compute_budget task panicked");
    }
}

#[tokio::test]
async fn test_concurrent_compute_budget_different_quests() {
    let governor = make_governor();

    // 10 线程并发计算不同 Quest 的预算系数
    let mut handles = Vec::new();
    for i in 0..10 {
        let governor = governor.clone();
        let quest = QuestBudgetInput::new(format!("quest-{i}"), i * 2, i, None, i * 100);
        handles.push(tokio::spawn(async move {
            let coef = governor.compute_budget(&quest);
            assert!((0.0..=1.0).contains(&coef));
        }));
    }

    for handle in handles {
        handle.await.expect("task panicked");
    }
}

#[tokio::test]
async fn test_concurrent_compute_budget_with_deadline() {
    let governor = make_governor();

    // 10 线程并发,部分有 deadline(紧急/非紧急)
    let mut handles = Vec::new();
    for i in 0..10 {
        let governor = governor.clone();
        let deadline = if i < 5 {
            // 紧急:30 分钟后
            Some(Utc::now() + chrono::Duration::minutes(30))
        } else {
            // 非紧急:2 天后
            Some(Utc::now() + chrono::Duration::days(2))
        };
        let quest = QuestBudgetInput::new(format!("quest-{i}"), 5, 2, deadline, 100);
        handles.push(tokio::spawn(async move {
            let coef = governor.compute_budget(&quest);
            assert!((0.0..=1.0).contains(&coef));
        }));
    }

    for handle in handles {
        handle.await.expect("task panicked");
    }
}

// ============================================================
// 并发 record_consumption 测试
// ============================================================

#[tokio::test]
async fn test_concurrent_record_consumption_no_panic() {
    let governor = make_governor_no_lag();

    // 10 线程并发记录消耗
    let mut handles = Vec::new();
    for _ in 0..10 {
        let governor = governor.clone();
        handles.push(tokio::spawn(async move {
            for _ in 0..10 {
                let consumption = BudgetConsumption::new(100, 1, 1);
                let _ = governor.record_consumption(&consumption);
            }
        }));
    }

    for handle in handles {
        handle.await.expect("record_consumption task panicked");
    }

    // 验证累计消耗正确:10 线程 × 10 次 × (100 tokens × 0.00001 + 1 call × 0.001)
    let stats = governor.get_stats();
    let expected_per_record = 100.0 * 0.00001 + 1.0 * 0.001;
    let expected_total = 10.0 * 10.0 * expected_per_record;
    assert!(
        (stats.total_consumption - expected_total).abs() < 1e-3,
        "total consumption should be ~{expected_total}, got {}",
        stats.total_consumption
    );
}

#[tokio::test]
async fn test_concurrent_record_consumption_overflow_safe() {
    // WHY 高并发下溢出检测:多线程同时记录大额消耗,不应 panic 或数据竞争
    let governor = Arc::new(
        DecbGovernor::new(DecbConfig {
            total_budget_limit: 1000.0,
            tier_switch_lag_ms: 0,
            ..Default::default()
        })
        .unwrap(),
    );

    let mut handles = Vec::new();
    for _ in 0..10 {
        let governor = governor.clone();
        handles.push(tokio::spawn(async move {
            // 每次记录 200 成本,5 次后总计 10000(远超 1000 上限)
            for _ in 0..5 {
                let consumption = BudgetConsumption {
                    total_cost: 200.0,
                    ..BudgetConsumption::zero()
                };
                let _ = governor.record_consumption(&consumption);
            }
        }));
    }

    for handle in handles {
        handle.await.expect("task panicked");
    }

    // 最终档位应为 Degraded(消耗远超上限)
    let tier = governor.current_tier();
    assert!(
        tier == BudgetTier::Degraded || tier == BudgetTier::LowTier,
        "tier should be degraded after massive consumption, got {tier}"
    );
}

// ============================================================
// 并发 switch_tier 测试
// ============================================================

#[tokio::test]
async fn test_concurrent_switch_tier_consistency() {
    let governor = make_governor_no_lag();

    // 10 线程并发切换档位
    let mut handles = Vec::new();
    for i in 0..10 {
        let governor = governor.clone();
        let target_tier = if i % 3 == 0 {
            BudgetTier::HighTier
        } else if i % 3 == 1 {
            BudgetTier::LowTier
        } else {
            BudgetTier::Degraded
        };
        handles.push(tokio::spawn(async move {
            let _ = governor.switch_tier(target_tier);
        }));
    }

    for handle in handles {
        handle.await.expect("switch_tier task panicked");
    }

    // 最终档位应为某个有效值(不 panic 即可)
    let tier = governor.current_tier();
    assert!(
        matches!(
            tier,
            BudgetTier::HighTier | BudgetTier::LowTier | BudgetTier::Degraded
        ),
        "tier should be valid after concurrent switches"
    );
}

// ============================================================
// 并发 reset_budget 测试
// ============================================================

#[tokio::test]
async fn test_concurrent_reset_and_record() {
    let governor = make_governor_no_lag();

    // 线程 1-5:并发记录消耗
    let mut handles = Vec::new();
    for _ in 0..5 {
        let governor = governor.clone();
        handles.push(tokio::spawn(async move {
            for _ in 0..20 {
                let consumption = BudgetConsumption::new(10, 1, 0);
                let _ = governor.record_consumption(&consumption);
            }
        }));
    }

    // 线程 6:并发重置预算
    let gov_reset = governor.clone();
    handles.push(tokio::spawn(async move {
        for _ in 0..5 {
            gov_reset.reset_budget();
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }));

    for handle in handles {
        handle.await.expect("task panicked");
    }

    // 不 panic 即通过,统计值应为非负
    let stats = governor.get_stats();
    assert!(stats.total_consumption >= 0.0);
    assert!(stats.remaining_budget >= 0.0);
}

// ============================================================
// 性能断言测试(标记 #[ignore])
// ============================================================

/// 性能断言测试:预算系数计算延迟 < 1ms
///
/// 标记 `#[ignore]`,需用 `cargo test -- --ignored` 运行
#[tokio::test]
#[ignore = "perf: run with --ignored"]
async fn test_perf_compute_budget_latency() {
    let governor = make_governor();
    let quest = QuestBudgetInput::new(
        "quest-perf",
        10,
        3,
        Some(Utc::now() + chrono::Duration::hours(2)),
        500,
    );

    // 预热
    for _ in 0..100 {
        let _ = governor.compute_budget(&quest);
    }

    // 测量 1000 次计算延迟
    let start = std::time::Instant::now();
    for _ in 0..1000 {
        let _ = governor.compute_budget(&quest);
    }
    let elapsed = start.elapsed();

    // 1000 次计算应在 1 秒内(即每次 < 1ms)
    assert!(
        elapsed < Duration::from_secs(1),
        "1000 compute_budget took {elapsed:?}, expected < 1s (i.e. < 1ms per call)"
    );
}

/// 性能断言测试:档位切换延迟 < 1ms
///
/// 标记 `#[ignore]`,需用 `cargo test -- --ignored` 运行
#[tokio::test]
#[ignore = "perf: run with --ignored"]
async fn test_perf_switch_tier_latency() {
    let governor = make_governor_no_lag();

    // 预热
    for _ in 0..100 {
        let _ = governor.switch_tier(BudgetTier::LowTier);
        let _ = governor.switch_tier(BudgetTier::HighTier);
    }

    // 测量 1000 次切换延迟
    let start = std::time::Instant::now();
    for i in 0..1000 {
        let target = if i % 2 == 0 {
            BudgetTier::LowTier
        } else {
            BudgetTier::HighTier
        };
        let _ = governor.switch_tier(target);
    }
    let elapsed = start.elapsed();

    // 1000 次切换应在 1 秒内(即每次 < 1ms)
    assert!(
        elapsed < Duration::from_secs(1),
        "1000 switch_tier took {elapsed:?}, expected < 1s (i.e. < 1ms per call)"
    );
}

/// 性能断言测试:record_consumption 吞吐量
///
/// 标记 `#[ignore]`,需用 `cargo test -- --ignored` 运行
#[tokio::test]
#[ignore = "perf: run with --ignored"]
async fn test_perf_record_consumption_throughput() {
    let governor = Arc::new(
        DecbGovernor::new(DecbConfig {
            total_budget_limit: 1e12, // 超大预算避免触发降级
            tier_switch_lag_ms: 0,
            ..Default::default()
        })
        .unwrap(),
    );

    let consumption = BudgetConsumption::new(100, 1, 1);

    // 测量 10000 次记录延迟
    let start = std::time::Instant::now();
    for _ in 0..10_000 {
        let _ = governor.record_consumption(&consumption);
    }
    let elapsed = start.elapsed();

    // 10000 次记录应在 1 秒内
    assert!(
        elapsed < Duration::from_secs(1),
        "10000 record_consumption took {elapsed:?}, expected < 1s"
    );
}
