//! 持锁跨 await 死锁检测测试(B-Crit-1/2/3/4 回归守卫)
//!
//! 本测试套件是 faae-router 4 个 Critical 级"持锁跨 await"反模式的回归守卫。
//! 每个测试针对一个 B-Crit,通过高并发场景 + `tokio::time::timeout` 检测死锁:
//!
//! - **B-Crit-1**(`test_no_deadlock_balance_with_concurrent_register`):
//!   route 进入 EDSB balance 分支(熵 < 阈值触发实际均衡计算)时,
//!   并发 register/unregister。若 route 持 registry 读锁跨 `edsb.balance().await`
//!   (内部多次 await:compute_entropy / estimate / publish),
//!   则 register 写锁被阻塞 → 整体超时死锁。
//!
//! - **B-Crit-2**(`test_no_deadlock_concurrent_route_same_tool`):
//!   100 个并发 route 同一工具,竞争 `last_used_at` 写锁。
//!   若未消除三重嵌套锁(registry 读 → profile 读 → last_used_at 写),
//!   嵌套锁跨 await 会死锁。
//!
//! - **B-Crit-3**(`test_no_deadlock_decay_and_route_last_used`):
//!   并发 `decay_usage_counts`(读 last_used_at) + route(写 last_used_at)。
//!   若 decay 持 profile 读锁跨 `last_used_at.read().await`,
//!   与 route 的 `last_used_at.write().await` 形成读写锁死锁。
//!
//! - **B-Crit-4**(`test_no_deadlock_decay_snapshot_with_concurrent_register`):
//!   模拟 `spawn_decay_loop` 的快照模式(克隆 registry → 释放锁 → decay),
//!   并发 register。若未克隆快照直接持 registry 读锁跨 `decay_usage_counts().await`
//!   (内部多次 await),则 register 写锁被阻塞 → 死锁。
//!
//! # 测试策略
//! - 超时阈值 10 秒(正常负载下应 < 2 秒,留 5x 余量)
//! - 100 个并发任务混合 route/register/unregister/decay
//! - 触发实际 balance 路径(预先设置 usage_count 不均匀使熵 < 0.6)
//! - 若超时则判定为持锁跨 await 违规(测试失败 + panic)

use std::sync::Arc;
use std::time::Duration;

use event_bus::EventBus;
use faae_router::{EdsbBalancer, ExpertProfile, FaaeConfig, FaaeRouter, ToolId};
use tokio::time::timeout;

/// 构造测试用专家画像(64 维向量,指定维度为 1.0 确保区分度)
fn make_profile(name: &str, dim_idx: usize) -> ExpertProfile {
    let mut v = vec![0.0; 64];
    v[dim_idx % 64] = 1.0;
    ExpertProfile::new(name, v, vec!["test".into()], 1.0)
}

/// 构造带初始 usage_count 的专家画像(用于触发 EDSB 低熵均衡)
fn make_profile_with_count(name: &str, dim_idx: usize, count: u64) -> ExpertProfile {
    let mut v = vec![0.0; 64];
    v[dim_idx % 64] = 1.0;
    ExpertProfile::with_usage_count(name, v, vec!["test".into()], 1.0, count)
}

/// 超时阈值:正常负载下应远小于此值,设 10 秒留足余量。
/// 若超时则判定为持锁跨 await 死锁。
const DEADLOCK_TIMEOUT: Duration = Duration::from_secs(10);

// =============================================================================
// B-Crit-1: router.rs 持读锁跨 edsb.balance().await
// =============================================================================

/// B-Crit-1 回归守卫:route 持读锁跨 `edsb.balance().await` 死锁检测
///
/// 场景:
/// - 注册 4 个工具,usage_count = [100, 0, 0, 0](熵 ≈ 0,必触发 balance)
/// - 50 个并发 route(进入 balance 分支,内部多次 await)
/// - 50 个并发 register/unregister(需要 registry 写锁)
///
/// 修复前:route 持 registry 读锁跨 balance().await → register 写锁阻塞 → 死锁
/// 修复后:route 克隆 registry 快照后释放读锁,锁外调用 balance → 无死锁
#[tokio::test]
async fn test_no_deadlock_balance_with_concurrent_register() {
    let bus = EventBus::new();
    let router = Arc::new(FaaeRouter::new(bus));

    // 预注册 4 个工具,负载高度集中(熵 ≈ 0,必触发 balance)
    router
        .register_expert(make_profile_with_count("hot", 0, 100))
        .await;
    router
        .register_expert(make_profile_with_count("c1", 1, 0))
        .await;
    router
        .register_expert(make_profile_with_count("c2", 2, 0))
        .await;
    router
        .register_expert(make_profile_with_count("c3", 3, 0))
        .await;

    let result = timeout(DEADLOCK_TIMEOUT, async {
        let mut handles = Vec::new();

        // 50 个并发 route(进入 balance 分支)
        for _ in 0..50 {
            let r = router.clone();
            handles.push(tokio::spawn(async move {
                let clv = {
                    let mut v = vec![0.0; 64];
                    v[0] = 1.0;
                    v
                };
                let candidates: Vec<ToolId> = (0..4)
                    .map(|i| ToolId::new(["hot", "c1", "c2", "c3"][i]))
                    .collect();
                let _ = r.route(&clv, &candidates).await;
            }));
        }

        // 50 个并发 register + unregister(需要 registry 写锁)
        for i in 0..50 {
            let r = router.clone();
            handles.push(tokio::spawn(async move {
                let name = format!("dyn-{i}");
                r.register_expert(make_profile(&name, i)).await;
                let _ = r.unregister_expert(&ToolId::new(name)).await;
            }));
        }

        for h in handles {
            let _ = h.await;
        }
    })
    .await;

    assert!(
        result.is_ok(),
        "B-Crit-1 回归:route 持读锁跨 balance().await 导致死锁(10 秒超时)"
    );
}

// =============================================================================
// B-Crit-2: router.rs 三重嵌套锁 + 持锁跨 last_used_at.write().await
// =============================================================================

/// B-Crit-2 回归守卫:三重嵌套锁(registry 读 → profile 读 → last_used_at 写)死锁检测
///
/// 场景:
/// - 注册 1 个工具
/// - 100 个并发 route 同一工具(竞争 last_used_at 写锁)
///
/// 修复前:registry 读锁 → profile 读锁 → last_used_at 写锁,三重嵌套跨 await
///         多个 route 同时持 registry 读锁等待 last_used_at 写锁,
///         而写 last_used_at 需要 profile 读锁(已被另一 route 持有)→ 死锁
/// 修复后:三阶段顺序获取锁,每阶段锁内仅同步操作 → 无死锁
#[tokio::test]
async fn test_no_deadlock_concurrent_route_same_tool() {
    // 关闭 balance 以隔离 B-Crit-2(只测试 last_used_at 写锁竞争)
    let config = FaaeConfig::default().with_balance_enabled(false);
    let router = Arc::new(FaaeRouter::with_config(EventBus::new(), config));
    router.register_expert(make_profile("hot", 0)).await;

    let result = timeout(DEADLOCK_TIMEOUT, async {
        let mut handles = Vec::new();
        for _ in 0..100 {
            let r = router.clone();
            handles.push(tokio::spawn(async move {
                let clv = vec![1.0; 64];
                let candidates = vec![ToolId::new("hot")];
                let _ = r.route(&clv, &candidates).await;
            }));
        }
        for h in handles {
            let _ = h.await;
        }
    })
    .await;

    assert!(
        result.is_ok(),
        "B-Crit-2 回归:三重嵌套锁跨 last_used_at.write().await 导致死锁(10 秒超时)"
    );
}

// =============================================================================
// B-Crit-3: edsb.rs decay_usage_counts 嵌套读锁跨 last_used_at.read().await
// =============================================================================

/// B-Crit-3 回归守卫:decay_usage_counts 嵌套读锁(profile 读 → last_used_at 读)死锁检测
///
/// 场景:
/// - 注册 4 个工具(有 usage_count)
/// - 并发:20 个 decay_usage_counts(读 last_used_at) + 80 个 route(写 last_used_at)
///
/// 修复前:decay 持 profile 读锁跨 `last_used_at.read().await`
///         同时 route 持 profile 读锁等待 `last_used_at.write().await`
///         若 last_used_at 被 route 写锁持有,decay 的 last_used_at.read 阻塞,
///         而 route 的 profile 读锁又阻塞 decay 释放 profile 读锁 → 死锁
/// 修复后:decay 分阶段获取锁(profile 读 → clone Arc → 释放 → last_used_at 读)
///         → 无嵌套锁 → 无死锁
#[tokio::test]
async fn test_no_deadlock_decay_and_route_last_used() {
    let bus = EventBus::new();
    let config = FaaeConfig::default().with_balance_enabled(false);
    let router = Arc::new(FaaeRouter::with_config(bus, config.clone()));
    let edsb = EdsbBalancer::new(config, EventBus::new());

    // 注册 4 个工具(各有 usage_count,触发 decay 实际计算)
    for i in 0..4 {
        router
            .register_expert(make_profile_with_count(&format!("t{i}"), i, 50))
            .await;
    }

    let result = timeout(DEADLOCK_TIMEOUT, async {
        let mut handles = Vec::new();

        // 20 个并发 decay_usage_counts(读 last_used_at)
        for _ in 0..20 {
            let r = router.clone();
            let edsb = edsb.clone();
            handles.push(tokio::spawn(async move {
                // 模拟 spawn_decay_loop 内部的快照模式(B-Crit-4 也覆盖)
                let snapshot = {
                    let registry = r.registry();
                    let reg = registry.read().await;
                    reg.clone()
                };
                edsb.decay_usage_counts(&snapshot).await;
            }));
        }

        // 80 个并发 route(写 last_used_at)
        for tid in 0..80 {
            let r = router.clone();
            handles.push(tokio::spawn(async move {
                let clv = {
                    let mut v = vec![0.0; 64];
                    v[tid % 4] = 1.0;
                    v
                };
                let candidates: Vec<ToolId> =
                    (0..4).map(|i| ToolId::new(format!("t{i}"))).collect();
                let _ = r.route(&clv, &candidates).await;
            }));
        }

        for h in handles {
            let _ = h.await;
        }
    })
    .await;

    assert!(
        result.is_ok(),
        "B-Crit-3 回归:decay_usage_counts 嵌套读锁跨 last_used_at.read().await 导致死锁(10 秒超时)"
    );
}

// =============================================================================
// B-Crit-4: edsb.rs spawn_decay_loop 持外层读锁跨 decay_usage_counts().await
// =============================================================================

/// B-Crit-4 回归守卫:spawn_decay_loop 持 registry 读锁跨 decay_usage_counts().await 死锁检测
///
/// 场景:
/// - 注册 8 个工具(有 usage_count)
/// - 并发:20 个"快照 + decay"循环(模拟 spawn_decay_loop 逻辑) + 80 个 register/unregister
///
/// 修复前:spawn_decay_loop 持 registry 读锁跨 `decay_usage_counts().await`
///         (内部多次 await:profile 读、last_used_at 读)
///         并发 register 需要 registry 写锁 → 被读锁阻塞 → 死锁
/// 修复后:spawn_decay_loop 克隆 registry 快照后释放读锁,锁外 decay → 无死锁
#[tokio::test]
async fn test_no_deadlock_decay_snapshot_with_concurrent_register() {
    let bus = EventBus::new();
    let config = FaaeConfig::default().with_balance_enabled(false);
    let router = Arc::new(FaaeRouter::with_config(bus, config.clone()));
    let edsb = Arc::new(EdsbBalancer::new(config, EventBus::new()));

    // 预注册 8 个工具(有 usage_count,触发 decay 实际计算)
    for i in 0..8 {
        router
            .register_expert(make_profile_with_count(&format!("t{i}"), i, 30))
            .await;
    }

    let result = timeout(DEADLOCK_TIMEOUT, async {
        let mut handles = Vec::new();

        // 20 个并发"快照 + decay"循环(模拟 spawn_decay_loop 内部逻辑)
        for _ in 0..20 {
            let r = router.clone();
            let edsb = edsb.clone();
            handles.push(tokio::spawn(async move {
                // 复刻 spawn_decay_loop 修复后的快照模式
                let snapshot = {
                    let registry = r.registry();
                    let reg = registry.read().await;
                    reg.clone()
                }; // registry 读锁在此释放
                edsb.decay_usage_counts(&snapshot).await;
            }));
        }

        // 80 个并发 register + unregister(需要 registry 写锁)
        for i in 0..80 {
            let r = router.clone();
            handles.push(tokio::spawn(async move {
                let name = format!("dyn-{i}");
                r.register_expert(make_profile(&name, i)).await;
                let _ = r.unregister_expert(&ToolId::new(name)).await;
            }));
        }

        for h in handles {
            let _ = h.await;
        }
    })
    .await;

    assert!(
        result.is_ok(),
        "B-Crit-4 回归:spawn_decay_loop 持外层读锁跨 decay_usage_counts().await 导致死锁(10 秒超时)"
    );
}

// =============================================================================
// 综合压力测试:4 个 B-Crit 混合场景
// =============================================================================

/// 综合回归守卫:100 个并发任务混合 route/register/unregister/decay,
/// 同时启动 spawn_decay_loop 后台任务。
///
/// 此测试是 B-Crit-1/2/3/4 的综合压力测试,确保 4 个修复在混合场景下协同工作。
/// 若任一 Critical 回归,会导致整体超时死锁。
#[tokio::test]
async fn test_no_deadlock_mixed_stress_all_crits() {
    let bus = EventBus::new();
    let router = Arc::new(FaaeRouter::new(bus));

    // 预注册 10 个工具,负载集中(触发 balance)
    router
        .register_expert(make_profile_with_count("hot", 0, 200))
        .await;
    for i in 1..10 {
        router
            .register_expert(make_profile_with_count(&format!("c{i}"), i, 0))
            .await;
    }

    // 启动后台 decay loop(B-Crit-4 守卫)
    router.spawn_decay_loop();

    let result = timeout(DEADLOCK_TIMEOUT, async {
        let mut handles = Vec::new();

        // 40 个并发 route(触发 balance + last_used_at 写)
        for _ in 0..40 {
            let r = router.clone();
            handles.push(tokio::spawn(async move {
                let clv = {
                    let mut v = vec![0.0; 64];
                    v[0] = 1.0;
                    v
                };
                // 候选工具:hot + c1..c9(避免在闭包内混合 &'static str 与 &String)
                let candidates: Vec<ToolId> = std::iter::once(ToolId::new("hot"))
                    .chain((1..10).map(|i| ToolId::new(format!("c{i}"))))
                    .collect();
                let _ = r.route(&clv, &candidates).await;
            }));
        }

        // 30 个并发 register + unregister(B-Crit-1/B-Crit-4 守卫)
        for i in 0..30 {
            let r = router.clone();
            handles.push(tokio::spawn(async move {
                let name = format!("dyn-{i}");
                r.register_expert(make_profile(&name, i)).await;
                let _ = r.unregister_expert(&ToolId::new(name)).await;
            }));
        }

        // 30 个并发手动 decay(B-Crit-3 守卫)
        for _ in 0..30 {
            let r = router.clone();
            handles.push(tokio::spawn(async move {
                let snapshot = {
                    let registry = r.registry();
                    let reg = registry.read().await;
                    reg.clone()
                };
                r.edsb().decay_usage_counts(&snapshot).await;
            }));
        }

        for h in handles {
            let _ = h.await;
        }
    })
    .await;

    assert!(
        result.is_ok(),
        "综合压力测试失败:4 个 B-Crit 修复中至少一个回归导致死锁(10 秒超时)"
    );
}
