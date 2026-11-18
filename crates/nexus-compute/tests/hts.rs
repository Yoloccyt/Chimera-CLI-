//! HTS-CPU 三层阈值来源的公开契约 — 阈值表查表 / 来源标注 / 序贯检验三态
//!
//! 对应 ADR-103 三重来源:① 离线测定（`OfflineMeasured`）② 序贯检验
//! （`SequentialTest` 状态机）③ cgroup 校正（由 `src/hts/mod.rs` 内联单测覆盖解析面）。
//! 本文件从 crate 外验证**阈值表可查/可更新/来源可溯源**与序贯检验的
//! 接受(Promote)/拒绝(Reject)/继续(Continue)三态边界。
//!
//! 确定性纪律:序贯检验无随机源,喂入固定样本序列即得固定决策序列（Ω₂）。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code

use nexus_compute::hts::sequential_test::SequentialTestConfig;
use nexus_compute::{
    DispatchPlan, HtsTable, SequentialTest, TaskKind, TestDecision, ThresholdSource,
};

#[test]
fn hts_table_lookup_is_total_and_monotone_free_of_zero_entries() {
    // 契约:十类登记全部可查,阈值/chunk 为正,chunk 为 2 的幂（分块树前提）
    let table = HtsTable::default();
    for kind in TaskKind::ALL {
        let entry = table.get(kind);
        assert!(
            entry.threshold > 0,
            "{kind:?} 阈值必须为正（0 会让任意 n 都进池）"
        );
        assert!(entry.chunk > 0, "{kind:?} chunk 必须为正");
        assert!(
            entry.chunk.is_power_of_two(),
            "{kind:?} chunk={} 必须是 2 的幂（DetReduce 块界前提）",
            entry.chunk
        );
    }
}

#[test]
fn hts_table_update_is_isolated_per_kind() {
    // 契约:表项按 TaskKind 独立 —— 校准一个负载不得连带改变另一个（ADR-196 解耦回归锁）
    let mut table = HtsTable::default();
    let before_other = table.get(TaskKind::DecayBatch);
    table.update(
        TaskKind::ExpertScore,
        777,
        16,
        ThresholdSource::SequentialTest {
            promoted_at: 1_700_000_000,
            samples: 64,
        },
    );
    let updated = table.get(TaskKind::ExpertScore);
    assert_eq!(updated.threshold, 777);
    assert_eq!(updated.chunk, 16);
    assert_eq!(
        updated.source,
        ThresholdSource::SequentialTest {
            promoted_at: 1_700_000_000,
            samples: 64,
        }
    );
    assert_eq!(
        table.get(TaskKind::DecayBatch),
        before_other,
        "未更新表项必须逐字段不变"
    );
}

#[test]
fn threshold_source_distinguishes_prefill_calibration_and_fallback() {
    // 契约:三种来源可区分（诚实数据红线:预填 ≠ 已校准 ≠ 已测定）
    let prefill = ThresholdSource::OfflineMeasured { measured_at: 0 };
    let measured = ThresholdSource::OfflineMeasured {
        measured_at: 1_700_000_000,
    };
    let calibrated = ThresholdSource::SequentialTest {
        promoted_at: 1_700_000_000,
        samples: 30,
    };
    let fallback = ThresholdSource::ConservativeDefault;
    assert_ne!(prefill, measured, "预填占位与真实测定时间戳必须可区分");
    assert_ne!(prefill, calibrated);
    assert_ne!(calibrated, fallback);
    assert_eq!(fallback, ThresholdSource::ConservativeDefault, "Eq 语义");
}

#[test]
fn sequential_test_continues_while_below_min_samples() {
    // 契约 1:小样本不判定（即便效果极显著）—— 防早停假阳性
    let mut t = SequentialTest::new(SequentialTestConfig::default());
    for i in 0..29 {
        assert_eq!(
            t.record(100.0, 1.0 + (i % 3) as f64),
            TestDecision::Continue,
            "第 {} 条样本(n < min_samples)必须 Continue",
            i + 1
        );
    }
    assert_eq!(t.sample_count(), 29);
    assert_eq!(t.spent_alpha(), 0.0, "未终局前不得消耗 alpha 预算");
    assert_eq!(t.decision(), None, "未终局前决策必须为空");
}

#[test]
fn sequential_test_promotes_when_effect_and_lrt_reach_upper_bound() {
    // 契约 2:SPRT 跨上界 + 效果量达标 → Promote（回传注入的候选阈值）
    let mut t = SequentialTest::new(SequentialTestConfig::default());
    t.set_candidate_threshold(256);
    let mut decisions = Vec::new();
    for i in 0..35 {
        decisions.push(t.record(100.0 + (i % 3) as f64, 10.0 + (i % 3) as f64));
    }
    // 首个判定点 = min_samples = 30（下标 29）;终局后 record 一律 Reject,故此处断言该点
    assert_eq!(
        decisions[29],
        TestDecision::Promote { threshold: 256 },
        "强效果数据跨上界后必须 Promote"
    );
    assert_eq!(
        decisions[30..],
        [TestDecision::Reject; 5],
        "终局后重复喂样本必须一律 Reject"
    );
    assert_eq!(
        t.decision(),
        Some(TestDecision::Promote { threshold: 256 }),
        "终局决策必须可经 decision() 查询"
    );
    assert_eq!(t.spent_alpha(), 0.05, "终局即耗尽一次性 alpha 预算");
}

#[test]
fn sequential_test_rejects_when_effect_size_not_reached() {
    // 契约 3:跨上界但效果量未达标 → Reject（Promote 只在效果量达标时发出）
    let mut t = SequentialTest::new(SequentialTestConfig::default());
    t.set_candidate_threshold(128);
    let mut last = TestDecision::Continue;
    for i in 0..40 {
        // 仅快 10%,低于 effect_size = 20%
        last = t.record(100.0 + (i % 2) as f64, 90.0 + (i % 2) as f64);
    }
    assert_eq!(last, TestDecision::Reject, "效果量未达标必须 Reject");
    assert_eq!(t.decision(), Some(TestDecision::Reject));
}

#[test]
fn sequential_test_freezes_after_terminal_decision() {
    // 契约 6:终局后实例关闭 —— 后续样本一律 Reject,样本量不再增长（防 p-hacking 预算墙）
    let mut t = SequentialTest::new(SequentialTestConfig::default());
    for i in 0..30 {
        let _ = t.record(100.0 + (i % 3) as f64, 10.0 + (i % 3) as f64);
    }
    assert!(t.decision().is_some(), "30 条强效果样本后应已终局");
    let frozen = t.sample_count();
    for _ in 0..8 {
        assert_eq!(
            t.record(100.0, 1.0),
            TestDecision::Reject,
            "终局后 record 必须强制 Reject"
        );
    }
    assert_eq!(t.sample_count(), frozen, "终局后不得再积累样本");
    assert_eq!(t.spent_alpha(), 0.05);
}

#[test]
fn sequential_test_config_validation_rejects_illegal_parameters() {
    // 契约:非法配置是编程错误 —— try_new 返回 Err 而非静默降级
    let mut cfg = SequentialTestConfig::default();
    assert!(SequentialTest::try_new(cfg).is_ok(), "默认配置必须合法");

    cfg.alpha = 1.0;
    assert!(SequentialTest::try_new(cfg).is_err(), "alpha 越界必须拒绝");

    cfg = SequentialTestConfig::default();
    cfg.power = 0.0;
    assert!(SequentialTest::try_new(cfg).is_err(), "power 越界必须拒绝");

    cfg = SequentialTestConfig::default();
    cfg.min_samples = 600;
    assert!(
        SequentialTest::try_new(cfg).is_err(),
        "min > max 样本窗口倒挂必须拒绝"
    );

    cfg = SequentialTestConfig::default();
    cfg.effect_size = 0.0;
    assert!(
        SequentialTest::try_new(cfg).is_err(),
        "effect_size 为 0 必须拒绝"
    );
}

#[test]
fn hts_table_stays_consistent_with_dispatch_route_surface() {
    // 契约:动态表默认值 = 路由静态参照（同一登记点的两条读取路径不得漂移）
    let table = HtsTable::default();
    let bridge = nexus_compute::bridge();
    for kind in TaskKind::ALL {
        let threshold = table.get(kind).threshold;
        assert_eq!(
            bridge.threshold_of(kind),
            threshold,
            "{kind:?} 全局桥表项偏离 HtsTable::default()"
        );
        // 阈值即干净切割点:n−1 → Inline,n → Rayon（io=false 的登记事实）
        assert_eq!(
            bridge.route(kind, threshold.saturating_sub(1)),
            DispatchPlan::Inline
        );
        assert_eq!(bridge.route(kind, threshold), DispatchPlan::Rayon);
    }
}
