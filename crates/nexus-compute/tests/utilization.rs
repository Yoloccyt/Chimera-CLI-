//! 利用率测量基座的公开契约 — 探针字段合法性、聚合单调性、tokio 缺失时的降级口径
//!
//! 对应 v4.0 §7.5.4 / WI-34:双探针（tokio 稳定子集 + rayon 池活跃率）。
//! 本文件只用**手工构造的快照**做聚合断言（不依赖真实池占用,避免慢机抖动）,
//! 另有两条探针自身的事实断言（无 runtime 必 None / 有 runtime 必 Some）。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code

use std::time::Instant;

use nexus_compute::{RayonProbe, TokioProbe, UtilizationSampler, UtilizationSnapshot};

/// 手工快照构造 — 把"采样时刻"的唯一不确定性隔离在此处
fn snapshot(
    tokio: Option<TokioProbe>,
    pool_threads: usize,
    active_tasks: usize,
) -> UtilizationSnapshot {
    UtilizationSnapshot {
        sampled_at: Instant::now(),
        tokio,
        rayon: RayonProbe {
            pool_threads,
            active_tasks,
        },
    }
}

#[test]
fn rayon_probe_ratio_is_zero_safe_and_allows_backlog_above_one() {
    // 契约:分母为零防除零;active > threads 合法（积压语义,不 clamp 掩盖过载）
    assert_eq!(
        RayonProbe {
            pool_threads: 0,
            active_tasks: 5,
        }
        .active_ratio(),
        0.0,
        "零线程必须返回 0.0 而非 NaN/Inf"
    );
    assert_eq!(
        RayonProbe {
            pool_threads: 8,
            active_tasks: 4,
        }
        .active_ratio(),
        0.5
    );
    assert_eq!(
        RayonProbe {
            pool_threads: 8,
            active_tasks: 16,
        }
        .active_ratio(),
        2.0,
        "积压率可 > 1.0（不得被 clamp）"
    );
}

#[test]
fn combined_utilization_falls_back_to_rayon_when_tokio_absent() {
    // 契约:tokio 不可用时降级为 rayon 单侧（诚实数据:降级而非补 0）
    let snap = snapshot(None, 4, 1);
    assert!(
        (snap.combined_utilization() - 0.25).abs() < 1e-9,
        "降级口径必须等于 rayon active_ratio"
    );
    // tokio 在场时两运行时各占一半权重
    let with_tokio = snapshot(
        Some(TokioProbe {
            num_workers: 2,
            num_alive_tasks: 2,
            global_queue_depth: 0,
            busy_us: 0,
        }),
        4,
        1,
    );
    assert!(
        (with_tokio.combined_utilization() - 0.625).abs() < 1e-9,
        "0.5×1.0(tokio) + 0.5×0.25(rayon) = 0.625"
    );
}

#[test]
fn tokio_probe_approx_ratio_is_capped_at_one() {
    // 契约:近似口径上限 1.0（存活任务可能远超 worker 数,评分不得越界）
    let probe = TokioProbe {
        num_workers: 4,
        num_alive_tasks: 40,
        global_queue_depth: 0,
        busy_us: 0,
    };
    assert_eq!(probe.utilization_approx(), 1.0);
    let idle = TokioProbe {
        num_workers: 4,
        num_alive_tasks: 0,
        global_queue_depth: 0,
        busy_us: 0,
    };
    assert_eq!(idle.utilization_approx(), 0.0);
    let no_workers = TokioProbe {
        num_workers: 0,
        num_alive_tasks: 9,
        global_queue_depth: 0,
        busy_us: 0,
    };
    assert_eq!(no_workers.utilization_approx(), 0.0, "零 worker 防除零");
}

#[test]
fn sampler_aggregation_is_monotone_and_bounded() {
    // 契约:count 单调递增;peak 单调不减;mean/peak ∈ [0,1] 且 peak ≥ mean
    let sampler = UtilizationSampler::new();
    assert_eq!(sampler.count(), 0);
    assert_eq!(sampler.mean(), 0.0);
    assert_eq!(sampler.peak(), 0.0);

    let samples = [
        snapshot(None, 4, 0), // 0.0
        snapshot(None, 4, 2), // 0.5
        snapshot(None, 4, 1), // 0.25（回落不得拉低峰值）
    ];
    let mut last_peak = 0.0_f64;
    for (i, snap) in samples.iter().enumerate() {
        sampler.record(snap);
        assert_eq!(sampler.count(), i + 1, "采样计数必须逐次递增");
        let peak = sampler.peak();
        assert!(
            peak >= last_peak - 1e-12,
            "第 {i} 次采样后峰值回落: {peak} < {last_peak}"
        );
        assert!((0.0..=1.0).contains(&peak), "峰值越界: {peak}");
        last_peak = peak;
    }
    assert!((sampler.peak() - 0.5).abs() < 1e-9, "峰值应为 0.5");
    assert!(
        (sampler.mean() - 0.25).abs() < 1e-3,
        "均值应为 (0+0.5+0.25)/3 = 0.25,实际 {}",
        sampler.mean()
    );
    assert!(sampler.peak() >= sampler.mean(), "峰值不得低于均值");
}

#[test]
fn sampler_can_be_shared_across_threads_without_losing_samples() {
    // 契约:跨线程 record 安全 —— 总计数精确等于调用次数（聚合值允许量化误差）
    let sampler = std::sync::Arc::new(UtilizationSampler::new());
    let handles: Vec<_> = (0..4)
        .map(|_| {
            let s = std::sync::Arc::clone(&sampler);
            std::thread::spawn(move || {
                for _ in 0..50 {
                    s.record(&UtilizationSnapshot {
                        sampled_at: Instant::now(),
                        tokio: None,
                        rayon: RayonProbe {
                            pool_threads: 4,
                            active_tasks: 1,
                        },
                    });
                }
            })
        })
        .collect();
    for h in handles {
        h.join().expect("采样线程应正常退出");
    }
    assert_eq!(sampler.count(), 200, "并发采样不得丢计数");
}

#[test]
fn tokio_probe_is_none_without_a_runtime_context() {
    // 契约:无 runtime 上下文返回 None（纯观测降级,绝不 panic）
    // 本测试跑在普通 #[test] 线程上,必然没有 runtime 上下文
    assert!(
        TokioProbe::sample().is_none(),
        "无 tokio runtime 时必须返回 None"
    );
}

#[tokio::test]
async fn tokio_probe_reports_worker_count_inside_a_runtime() {
    // 契约:runtime 内必须可采样,且 worker 数 > 0、近似利用率 ∈ [0,1]
    let probe = TokioProbe::sample().expect("runtime 内必须可采样");
    assert!(probe.num_workers > 0, "runtime 内 worker 数必须为正");
    let ratio = probe.utilization_approx();
    assert!(
        (0.0..=1.0).contains(&ratio),
        "近似利用率必须落在 [0,1]: {ratio}"
    );
}

#[test]
fn live_bridge_probe_exposes_positive_pool_threads() {
    // 契约:全局桥的 rayon 探针字段合法（线程数下限 2,活跃数非负）
    let probe = nexus_compute::bridge().rayon_probe();
    assert!(probe.pool_threads >= 2, "池线程数下限为 2（MIN_THREADS）");
    assert!(
        probe.active_ratio() >= 0.0,
        "活跃率必须非负: {}",
        probe.active_ratio()
    );
}
