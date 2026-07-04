//! SubTask 2.12: WindowSelector 单元测试
//!
//! 验证 4 个复杂度档位选择正确窗口层级,决策耗时 < 1ms。
//! 测试通过 `hcw_window` crate 的公共 API 进行(集成测试)。

use hcw_window::{WindowSelector, WindowTier};

/// 验证 4 个复杂度档位选择正确的窗口层级
#[test]
fn test_four_complexity_tiers_select_correct_window() {
    // L0:简单任务(complexity < 0.25)
    assert_eq!(
        WindowSelector::select(0.1),
        WindowTier::L0,
        "简单任务应选 L0"
    );

    // L1:常规任务(0.25 <= complexity < 0.5)
    assert_eq!(
        WindowSelector::select(0.3),
        WindowTier::L1,
        "常规任务应选 L1"
    );

    // L2:复杂任务(0.5 <= complexity < 0.75)
    assert_eq!(
        WindowSelector::select(0.6),
        WindowTier::L2,
        "复杂任务应选 L2"
    );

    // L3:超复杂任务(complexity >= 0.75)
    assert_eq!(
        WindowSelector::select(0.8),
        WindowTier::L3,
        "超复杂任务应选 L3"
    );
}

/// 验证边界值:0.25/0.5/0.75 归为更高层级(左闭右开)
#[test]
fn test_boundary_thresholds() {
    assert_eq!(
        WindowSelector::select(0.25),
        WindowTier::L1,
        "0.25 边界归 L1"
    );
    assert_eq!(WindowSelector::select(0.5), WindowTier::L2, "0.5 边界归 L2");
    assert_eq!(
        WindowSelector::select(0.75),
        WindowTier::L3,
        "0.75 边界归 L3"
    );
}

/// 验证极端值:0.0 选 L0,1.0 选 L3
#[test]
fn test_extreme_values() {
    assert_eq!(WindowSelector::select(0.0), WindowTier::L0, "0.0 选 L0");
    assert_eq!(WindowSelector::select(1.0), WindowTier::L3, "1.0 选 L3");
}

/// 验证超出 [0, 1] 范围的复杂度:负值归 L0,超过 1 归 L3
#[test]
fn test_out_of_range_complexity() {
    assert_eq!(WindowSelector::select(-0.1), WindowTier::L0, "负值归 L0");
    assert_eq!(WindowSelector::select(-1.0), WindowTier::L0, "负值归 L0");
    assert_eq!(WindowSelector::select(1.5), WindowTier::L3, "超过 1 归 L3");
    assert_eq!(
        WindowSelector::select(100.0),
        WindowTier::L3,
        "超过 1 归 L3"
    );
}

/// 验证 NaN 处理:NaN 归为 L0(最简单,避免不必要的资源消耗)
#[test]
fn test_nan_returns_l0() {
    assert_eq!(
        WindowSelector::select(f32::NAN),
        WindowTier::L0,
        "NaN 归 L0"
    );
    assert_eq!(
        WindowSelector::select(f32::INFINITY),
        WindowTier::L3,
        "Infinity 归 L3"
    );
    assert_eq!(
        WindowSelector::select(f32::NEG_INFINITY),
        WindowTier::L0,
        "NegInfinity 归 L0"
    );
}

/// 性能基准:1000 次决策 P50 < 10ms, P99 < 20ms(平均每次 < 10us,远低于 1ms 要求)
/// SubTask 11.2:添加 warmup(10 次)+ P50/P99 统计(100 次测量)
#[test]
#[ignore = "perf: run with --ignored"]
fn test_performance_under_1ms() {
    // Warmup(10 次,触发缓存预热与分支预测器稳定)
    for _ in 0..10 {
        for i in 0..1000 {
            let _ = WindowSelector::select(i as f32 / 1000.0);
        }
    }

    // 正式测量(100 次,每次 1000 次决策,收集延迟分布)
    let mut latencies = Vec::with_capacity(100);
    for _ in 0..100 {
        let start = std::time::Instant::now();
        for i in 0..1000 {
            let _ = WindowSelector::select(i as f32 / 1000.0);
        }
        latencies.push(start.elapsed().as_nanos() as f64);
    }
    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = latencies[50];
    let p99 = latencies[99];

    // P50 < 10ms(10_000_000ns), P99 < 20ms(原阈值 × 2)
    let threshold_ns = 10_000_000.0_f64;
    assert!(
        p50 < threshold_ns,
        "P50 延迟 {}ns 超过 {}ns",
        p50,
        threshold_ns
    );
    assert!(
        p99 < threshold_ns * 2.0,
        "P99 延迟 {}ns 超过 {}ns",
        p99,
        threshold_ns * 2.0
    );
}

/// 性能基准:单次决策 P50 < 1ms, P99 < 2ms(任务要求)
/// SubTask 11.2:添加 warmup(10 次)+ P50/P99 统计(100 次测量)
#[test]
#[ignore = "perf: run with --ignored"]
fn test_single_decision_under_1ms() {
    // Warmup(10 次,触发缓存预热与分支预测器稳定)
    for _ in 0..10 {
        let _ = WindowSelector::select(0.5);
    }

    // 正式测量(100 次,收集延迟分布)
    let mut latencies = Vec::with_capacity(100);
    for _ in 0..100 {
        let start = std::time::Instant::now();
        let _ = WindowSelector::select(0.5);
        latencies.push(start.elapsed().as_nanos() as f64);
    }
    latencies.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p50 = latencies[50];
    let p99 = latencies[99];

    // P50 < 1ms(1_000_000ns), P99 < 2ms(原阈值 × 2)
    let threshold_ns = 1_000_000.0_f64;
    assert!(
        p50 < threshold_ns,
        "P50 延迟 {}ns 超过 {}ns",
        p50,
        threshold_ns
    );
    assert!(
        p99 < threshold_ns * 2.0,
        "P99 延迟 {}ns 超过 {}ns",
        p99,
        threshold_ns * 2.0
    );
}

/// 验证选择器的纯函数性质:相同输入总是产生相同输出
#[test]
fn test_selector_is_pure_function() {
    for complexity in [0.1, 0.3, 0.5, 0.7, 0.9] {
        let first = WindowSelector::select(complexity);
        let second = WindowSelector::select(complexity);
        assert_eq!(first, second, "相同复杂度 {complexity} 应产生相同结果");
    }
}

/// 验证层级递进:复杂度递增时,窗口层级单调不减
#[test]
fn test_monotonic_non_decreasing() {
    let mut prev_tier = WindowSelector::select(0.0);
    for i in 1..=100 {
        let complexity = i as f32 / 100.0;
        let curr_tier = WindowSelector::select(complexity);
        assert!(
            curr_tier >= prev_tier,
            "复杂度 {complexity} 的层级 {curr_tier:?} 不应低于前一层级 {prev_tier:?}"
        );
        prev_tier = curr_tier;
    }
}
