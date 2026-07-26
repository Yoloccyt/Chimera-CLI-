//! R1 召回配额离线 RL 集成测试 — 端到端流程验证（P4-W16.2.2 步骤 7）
//!
//! 对应 ADR: **ADR-042**（R2 冻结）+ **ADR-043**（R1 影子模式）+ **ADR-037**（CapabilityToken 四态）
//! 对应设计源: `NEXUS-OMEGA_v5.0_系统性完整设计文档.md` §7.5
//!
//! # 测试覆盖
//!
//! 1. **CQL/IQL 端到端**: 回放池填充 → 训练 → 推理 → 策略输出
//! 2. **影子模式生命周期**: Provisional → 14 天观察 → 解冻就绪 / 回滚
//! 3. **回滚触发**: 连续退化 / AsaIntervention / EWMA 崩塌 / 召回率下降
//! 4. **序列化兼容**: ShadowComparisonReport serde 往返
//! 5. **解冻条件评估**: 部分条件不满足时拒绝解冻

use chrono::Utc;
use nexus_contracts::{RecallQuota, RecallQuotaPolicy};
use omega_learner::r1_recall_quota::{
    R1Context, RecallQuotaLearner, RecallQuotaTransition,
};
use omega_learner::replay_pool::ReplayPool;
use omega_learner::s2_memory::TaskPhase;
use omega_learner::shadow_mode::{
    ComparisonResult, PromotionReadiness, ShadowComparisonReport, ShadowModeTracker, StrategyMetrics,
    DEFAULT_OBSERVATION_DAYS, EWMA_PROMOTION_THRESHOLD,
};
use rand::thread_rng;

// ============================================================
// 辅助函数
// ============================================================

/// 构造默认 R1 上下文（LongRun 阶段，中等复杂度与内存压力）
fn make_ctx() -> R1Context {
    R1Context::new(TaskPhase::LongRun, 0.7, 0.4).unwrap()
}

/// 构造下一状态上下文（轻微状态变化）
fn make_next_ctx() -> R1Context {
    R1Context::new(TaskPhase::LongRun, 0.8, 0.5).unwrap()
}

/// 填充回放池至指定容量（使用统一奖励避免训练发散）
fn fill_pool(pool: &ReplayPool<RecallQuotaTransition>, n: usize) {
    let ctx = make_ctx();
    let next_ctx = make_next_ctx();
    for _ in 0..n {
        pool.push(
            RecallQuotaTransition::new(&ctx, RecallQuota::K20, 0.75, &next_ctx, false, "q-1").unwrap(),
        );
    }
}

/// 构造策略指标快照
fn make_metrics(recall: f32, false_block: f32, latency: f32, count: u64) -> StrategyMetrics {
    StrategyMetrics::new(recall, false_block, latency, count).unwrap()
}

/// 构造对比报告（R1 综合得分比 L3 高 diff）
fn make_report(diff: f32, remaining: u16) -> ShadowComparisonReport {
    let r1 = make_metrics(0.9, 0.05, 0.1, 100);
    // 保持 l3.recall_rate 与 r1 相同，避免触发 RecallRateDrop 信号
    let l3 = StrategyMetrics {
        recall_rate: r1.recall_rate,
        false_block_rate: r1.false_block_rate,
        latency_penalty: r1.latency_penalty,
        composite_score: r1.composite_score - diff,
        sample_count: 100,
    };
    ShadowComparisonReport::new(Utc::now(), r1, l3, remaining)
}

// ============================================================
// 1. CQL/IQL 端到端测试
// ============================================================

#[test]
fn test_r1_cql_learner_end_to_end() {
    // 1. 填充回放池（>= min_pool_size）
    let pool: ReplayPool<RecallQuotaTransition> = ReplayPool::new();
    fill_pool(&pool, 300);

    // 2. 创建 CQL 学习器并训练
    let mut learner = RecallQuotaLearner::default_cql().unwrap();
    let mut rng = thread_rng();
    learner.train(&pool, &mut rng).unwrap();

    // 3. 验证训练步数 > 0
    assert!(learner.train_steps() > 0, "CQL 训练步数必须 > 0");

    // 4. 推理：选择召回配额
    let ctx = make_ctx();
    let quota = learner.select_quota(&ctx).unwrap();

    // 5. 验证输出的 quota 是有效的 5 档之一
    assert!(matches!(
        quota,
        RecallQuota::K5 | RecallQuota::K10 | RecallQuota::K20 | RecallQuota::K50 | RecallQuota::K100
    ));

    // 6. 验证策略输出为 Learned
    let policy = learner.current_policy(1, &ctx);
    assert!(policy.is_learned(), "训练后策略必须为 Learned");
}

#[test]
fn test_r1_iql_learner_end_to_end() {
    // 1. 填充回放池
    let pool: ReplayPool<RecallQuotaTransition> = ReplayPool::new();
    fill_pool(&pool, 300);

    // 2. 创建 IQL 学习器并训练
    let mut learner = RecallQuotaLearner::default_iql().unwrap();
    let mut rng = thread_rng();
    learner.train(&pool, &mut rng).unwrap();

    // 3. 验证训练步数 > 0
    assert!(learner.train_steps() > 0, "IQL 训练步数必须 > 0");

    // 4. 推理
    let ctx = make_ctx();
    let quota = learner.select_quota(&ctx).unwrap();
    assert!(matches!(
        quota,
        RecallQuota::K5 | RecallQuota::K10 | RecallQuota::K20 | RecallQuota::K50 | RecallQuota::K100
    ));

    // 5. 验证策略输出
    let policy = learner.current_policy(1, &ctx);
    assert!(policy.is_learned(), "IQL 训练后策略必须为 Learned");
}

#[test]
fn test_r1_cql_train_multiple_iterations_stable() {
    // 多次训练验证稳定性（不 panic、不发散）
    let pool: ReplayPool<RecallQuotaTransition> = ReplayPool::new();
    fill_pool(&pool, 500);

    let mut learner = RecallQuotaLearner::default_cql().unwrap();
    let mut rng = thread_rng();

    // 连续训练 3 轮
    for _ in 0..3 {
        learner.train(&pool, &mut rng).unwrap();
    }

    // 验证训练步数累计
    assert!(learner.train_steps() >= 3, "多轮训练后步数应累计");

    // 推理仍然有效
    let ctx = make_ctx();
    let _ = learner.select_quota(&ctx).unwrap();
}

// ============================================================
// 2. 影子模式生命周期测试
// ============================================================

#[test]
fn test_r1_shadow_mode_initial_state_no_reports() {
    // 初始状态：无对比报告，胜率 0，观察期未满
    let tracker = ShadowModeTracker::new(0);

    assert_eq!(tracker.current_win_rate(), 0.0);
    assert!(!tracker.observation_period_complete(DEFAULT_OBSERVATION_DAYS as i64 * 86400));
}

#[test]
fn test_r1_shadow_mode_promotion_to_authorized() {
    // 模拟 14 天全胜场景 → 解冻就绪
    let start_time: i64 = 0;
    let day_seconds: i64 = 86400;
    let mut tracker = ShadowModeTracker::new(start_time);

    // 14 天 R1 显著优于 L3
    for day in 0..DEFAULT_OBSERVATION_DAYS {
        let now = start_time + (day as i64 + 1) * day_seconds;
        let report = make_report(0.15, DEFAULT_OBSERVATION_DAYS - 1 - day);
        let rollback = tracker.record_daily_report(report);
        assert!(rollback.is_none(), "全胜期间不应触发回滚");
    }

    // 评估解冻就绪（EWMA=0.8 ≥ 0.7，无 ASA）
    let now = start_time + DEFAULT_OBSERVATION_DAYS as i64 * day_seconds;
    let readiness = tracker.evaluate_promotion_readiness(now, 0.8);

    assert!(readiness.ewma达标, "EWMA=0.8 应达标");
    assert!(readiness.win_rate_达标, "14 天全胜应达标");
    assert!(readiness.observation_complete, "14 天观察期应满");
    assert!(readiness.no_asa_intervention, "无 ASA 应达标");
    assert!(readiness.is_ready(), "全条件满足应可解冻");
}

#[test]
fn test_r1_shadow_mode_asa_intervention_resets() {
    // ASA 触发后，no_asa_intervention 条件不满足
    let start_time: i64 = 0;
    let day_seconds: i64 = 86400;
    let mut tracker = ShadowModeTracker::new(start_time);

    // 14 天全胜
    for day in 0..DEFAULT_OBSERVATION_DAYS {
        let now = start_time + (day as i64 + 1) * day_seconds;
        let report = make_report(0.15, DEFAULT_OBSERVATION_DAYS - 1 - day);
        let _ = tracker.record_daily_report(report);
    }

    // 模拟 ASA 触发（调用 record_asa_intervention 增加内部 asa_count）
    tracker.record_asa_intervention(Utc::now());
    let now = start_time + DEFAULT_OBSERVATION_DAYS as i64 * day_seconds;
    let readiness = tracker.evaluate_promotion_readiness(now, 0.8);

    assert!(!readiness.no_asa_intervention, "ASA 触发后应不达标");
    assert!(!readiness.is_ready(), "ASA 触发后不应解冻");
}

// ============================================================
// 3. 回滚触发测试
// ============================================================

#[test]
fn test_r1_shadow_mode_rollback_on_consecutive_regression() {
    // 连续 3 天 R1 显著差于 L3 → 触发回滚
    let mut tracker = ShadowModeTracker::new(0);

    // 前 2 天正常
    for _ in 0..2 {
        let report = make_report(0.1, 12);
        assert!(tracker.record_daily_report(report).is_none());
    }

    // 连续 3 天显著退化（diff = -0.15）
    let mut rollback_triggered = false;
    for _ in 0..3 {
        let report = make_report(-0.15, 9);
        if tracker.record_daily_report(report).is_some() {
            rollback_triggered = true;
        }
    }

    assert!(rollback_triggered, "连续 3 天显著退化应触发回滚");
}

#[test]
fn test_r1_shadow_mode_rollback_on_recall_rate_drop() {
    // R1 召回率较 L3 下降 ≥ 5% → 触发回滚
    let mut tracker = ShadowModeTracker::new(0);

    // 构造召回率下降报告（R1 recall=0.80, L3 recall=0.90 → drop=0.10 ≥ 0.05）
    let r1 = make_metrics(0.80, 0.05, 0.1, 100);
    let l3 = make_metrics(0.90, 0.05, 0.1, 100);
    let report = ShadowComparisonReport::new(Utc::now(), r1, l3, 10);

    let rollback = tracker.record_daily_report(report);
    assert!(rollback.is_some(), "召回率下降 ≥ 5% 应触发回滚");
}

#[test]
fn test_r1_shadow_mode_no_rollback_on_tied() {
    // 持平场景不应触发回滚
    let mut tracker = ShadowModeTracker::new(0);

    // diff = 0.0 → Tied
    let report = make_report(0.0, 10);
    let rollback = tracker.record_daily_report(report);
    assert!(rollback.is_none(), "持平场景不应触发回滚");
}

// ============================================================
// 4. 序列化兼容测试
// ============================================================

#[test]
fn test_r1_comparison_report_serialization() {
    // ShadowComparisonReport serde 往返
    let r1 = make_metrics(0.92, 0.03, 0.05, 150);
    let l3 = make_metrics(0.85, 0.05, 0.08, 150);
    let report = ShadowComparisonReport::new(Utc::now(), r1, l3, 7);

    // 序列化
    let json = serde_json::to_string(&report).unwrap();
    assert!(!json.is_empty());

    // 反序列化
    let deserialized: ShadowComparisonReport = serde_json::from_str(&json).unwrap();
    assert_eq!(report, deserialized);

    // 验证对比结论
    assert_eq!(deserialized.comparison, ComparisonResult::R1SlightlyBetter);
}

#[test]
fn test_r1_strategy_metrics_serialization() {
    let metrics = make_metrics(0.95, 0.02, 0.03, 200);

    let json = serde_json::to_string(&metrics).unwrap();
    let deserialized: StrategyMetrics = serde_json::from_str(&json).unwrap();
    assert_eq!(metrics, deserialized);
}

// ============================================================
// 5. 解冻条件评估测试
// ============================================================

#[test]
fn test_r1_promotion_readiness_partial_conditions() {
    // 仅满足部分条件时不应解冻
    let start_time: i64 = 0;
    let day_seconds: i64 = 86400;
    let mut tracker = ShadowModeTracker::new(start_time);

    // 仅 7 天数据（观察期未满 14 天）
    for day in 0..7u16 {
        let now = start_time + (day as i64 + 1) * day_seconds;
        let report = make_report(0.15, DEFAULT_OBSERVATION_DAYS - 1 - day);
        let _ = tracker.record_daily_report(report);
    }

    let now = start_time + 7 * day_seconds;
    let readiness = tracker.evaluate_promotion_readiness(now, 0.8);

    // 观察期未满
    assert!(!readiness.observation_complete, "7 天观察期未满 14 天");
    assert!(!readiness.is_ready(), "观察期未满不应解冻");

    // 应列出未满足的条件
    let unmet = readiness.unmet_conditions();
    assert!(!unmet.is_empty(), "应有未满足的条件");
}

#[test]
fn test_r1_promotion_readiness_low_ewma() {
    // EWMA < 0.7 时不应解冻
    let start_time: i64 = 0;
    let day_seconds: i64 = 86400;
    let mut tracker = ShadowModeTracker::new(start_time);

    // 14 天数据但 EWMA 低
    for day in 0..DEFAULT_OBSERVATION_DAYS {
        let now = start_time + (day as i64 + 1) * day_seconds;
        let report = make_report(0.15, DEFAULT_OBSERVATION_DAYS - 1 - day);
        let _ = tracker.record_daily_report(report);
    }

    let now = start_time + DEFAULT_OBSERVATION_DAYS as i64 * day_seconds;
    // EWMA = 0.5 < 0.7 阈值
    let readiness = tracker.evaluate_promotion_readiness(now, 0.5);

    assert!(!readiness.ewma达标, "EWMA=0.5 < 0.7 应不达标");
    assert!(!readiness.is_ready(), "EWMA 低不应解冻");
}

#[test]
fn test_r1_recall_quota_policy_static_fallback() {
    // Static 策略不依赖 CapabilityToken 授权（C4 合规第一层）
    let policy = RecallQuotaPolicy::Static(RecallQuota::K10);
    assert!(!policy.is_learned());
    assert!(matches!(policy, RecallQuotaPolicy::Static(_)));
}

#[test]
fn test_r1_recall_quota_policy_learned_requires_version() {
    // Learned 策略携带版本号（与 CapabilityToken::bound_policy_version 对齐）
    let policy = RecallQuotaPolicy::Learned {
        version: 42,
        quota: RecallQuota::K20,
    };
    assert!(policy.is_learned());
    assert!(matches!(policy, RecallQuotaPolicy::Learned { .. }));
}

// ============================================================
// 6. EWMA 阈值常量验证
// ============================================================

#[test]
fn test_r1_ewma_promotion_threshold_constant() {
    // 验证 EWMA 解冻阈值常量（ADR-043 决策 3 条件 1）
    assert_eq!(EWMA_PROMOTION_THRESHOLD, 0.7);
    // 与 CapabilityToken::ACTIVATION_THRESHOLD (0.3) 区分
    assert!(EWMA_PROMOTION_THRESHOLD > 0.3, "解冻阈值应严于激活阈值");
}
