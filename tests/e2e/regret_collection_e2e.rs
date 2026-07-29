//! 后悔率采集管线端到端测试 — R2 解冻阶段③ 前置 1(Phase 前置1→前置3 信号链)
//!
//! 对应架构层: L6 omega-learner(采集) × L4 decay-engine(熔断器)
//! 对应 ADR: ADR-052 待办 1(后悔率采集)+ 待办 3(熔断)+ ADR-042(R2 冻结)
//! 对应计划: R2 解冻阶段③ 前置 1
//!
//! # 闭环验证:前置1 采集 → 前置3 熔断(用户要求 1 功能闭环)
//!
//! 本 E2E 验证完整信号链:`RegretCollector` 采集后悔率观测 → `assess_trend()`
//! 产出真实 `VerificationResult` → `ShadowModeCircuitBreaker.observe()` → 门控裁决。
//! 这是前置 1 与前置 3 的真实组件协同(非手构造验证结果)。
//!
//! # 三路径覆盖(用户要求 1)
//!
//! - **正常路径**:后悔率收敛(趋势非增)→ 熔断器许可
//! - **边界条件**:样本不足(趋势 Skipped 证据不足)→ 熔断器拒绝但不跳闸;
//!   步数单调性边界
//! - **异常场景**:后悔率发散(趋势 Violated)→ 熔断器永久跳闸

use decay_engine::shadow_breaker::ShadowModeCircuitBreaker;
use omega_learner::regret_pipeline::RegretCollector;

// ============================================================
// 正常路径:后悔率收敛 → assess_trend Satisfied → 熔断器许可
// ============================================================

#[test]
fn test_normal_converging_regret_permits_rl() {
    let mut collector = RegretCollector::new(128, 2, 0.05);
    // 模拟学习收敛:后悔率逐步下降(窗口均值 0.8 → 0.5 → 0.2)
    for (step, r) in [0.9, 0.7, 0.6, 0.4, 0.3, 0.1].iter().enumerate() {
        collector.record_regret(step as u64 + 1, *r);
    }

    // 前置1 产出真实 VerificationResult
    let trend = collector.assess_trend();
    assert!(trend.is_satisfied(), "收敛后悔率应趋势 Satisfied");

    // 前置3 消费:熔断器许可
    let mut cb = ShadowModeCircuitBreaker::new();
    let verdict = cb.observe(&[trend, collector.assess_step_monotonicity()]);
    assert!(verdict.is_permitted(), "收敛趋势 + 单调步数应许可 RL 更新");
    assert!(!cb.is_tripped());
}

// ============================================================
// 异常场景:后悔率发散 → assess_trend Violated → 熔断器永久跳闸
// ============================================================

#[test]
fn test_exception_diverging_regret_trips_breaker() {
    let mut collector = RegretCollector::new(128, 2, 0.05);
    // 模拟学习发散:后悔率上升(窗口均值 0.2 → 0.8)
    for (step, r) in [0.2, 0.2, 0.8, 0.8].iter().enumerate() {
        collector.record_regret(step as u64 + 1, *r);
    }

    let trend = collector.assess_trend();
    assert!(trend.is_violated(), "发散后悔率应趋势 Violated");

    // 熔断器消费发散信号 → 永久跳闸
    let mut cb = ShadowModeCircuitBreaker::new();
    let verdict = cb.observe(&[trend]);
    assert!(!verdict.is_permitted(), "发散趋势应拒绝 RL 更新");
    assert!(cb.is_tripped(), "后悔率发散应触发熔断器永久跳闸");
    assert!(cb.trip_cause().unwrap().contains("后悔率"));
}

/// 异常:步数回退(快照乱序)→ assess_step_monotonicity Violated → 跳闸
#[test]
fn test_exception_step_regression_trips_breaker() {
    let mut collector = RegretCollector::new(128, 2, 0.05);
    collector.record_regret(5, 0.3);
    collector.record_regret(3, 0.2); // 步数回退(持久化/恢复缺陷)
    collector.record_regret(7, 0.1);

    let step_check = collector.assess_step_monotonicity();
    assert!(step_check.is_violated(), "步数回退应 Violated");

    let mut cb = ShadowModeCircuitBreaker::new();
    cb.observe(&[step_check]);
    assert!(cb.is_tripped(), "步数乱序应触发跳闸");
}

// ============================================================
// 边界条件
// ============================================================

/// 边界:样本不足 → assess_trend Skipped → 熔断器证据不足拒绝(不跳闸)
#[test]
fn test_boundary_insufficient_samples_denies_no_trip() {
    let mut collector = RegretCollector::new(128, 2, 0.05);
    collector.record_regret(1, 0.5); // 仅 1 条,完整窗口 < 2

    let trend = collector.assess_trend();
    assert!(trend.is_skipped(), "样本不足应 Skipped");

    let mut cb = ShadowModeCircuitBreaker::new();
    let verdict = cb.observe(&[trend]);
    assert!(!verdict.is_permitted(), "证据不足 fail-closed 拒绝");
    assert!(!cb.is_tripped(), "Skipped 非违规,不应跳闸");
}

/// 边界:空采集管线 → 空序列 → Skipped
#[test]
fn test_boundary_empty_collector_skipped() {
    let collector = RegretCollector::default();
    assert!(collector.is_empty());
    assert!(collector.assess_trend().is_skipped());
}

/// 边界:探索抖动在容差内(窗口均值 0.5 → 0.52,容差 0.05)→ Satisfied → 许可
#[test]
fn test_boundary_exploration_jitter_within_tolerance_permits() {
    let mut collector = RegretCollector::new(128, 2, 0.05);
    for (step, r) in [0.5, 0.5, 0.52, 0.52].iter().enumerate() {
        collector.record_regret(step as u64 + 1, *r);
    }
    let trend = collector.assess_trend();
    assert!(trend.is_satisfied(), "容差内抖动应 Satisfied");

    let mut cb = ShadowModeCircuitBreaker::new();
    assert!(cb.observe(&[trend]).is_permitted());
}

// ============================================================
// 影子模式多周期:滑动窗口 + 持续评估
// ============================================================

/// 影子模式:连续采集 + 每周期评估,窗口淘汰旧样本,持续收敛保持许可
#[test]
fn test_shadow_mode_sliding_window_multi_cycle() {
    // 容量 6:窗口滚动
    let mut collector = RegretCollector::new(6, 2, 0.05);
    let mut cb = ShadowModeCircuitBreaker::new();

    // 20 步持续收敛的后悔率(每步略降),窗口滚动保留最近 6 条
    for step in 0..20u64 {
        let regret = 1.0 - (step as f64) * 0.04; // 1.0 → 0.24 线性下降
        collector.record_regret(step + 1, regret.max(0.0));

        if collector.len() >= 4 {
            let verdict = cb.observe(&[collector.assess_trend()]);
            assert!(verdict.is_permitted(), "持续收敛应始终许可(step {step})");
        }
    }
    // 窗口容量上限生效
    assert_eq!(collector.len(), 6);
    assert!(!cb.is_tripped());
}
