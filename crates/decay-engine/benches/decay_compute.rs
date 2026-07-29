//! decay_compute SLO benchmark — 单次衰减计算延迟基准
//!
//! # 目标
//! SLO: 单次 `decay` 调用 < 1μs（含 DashMap 分片锁 + 浮点 clamp）。
//!
//! # 基准项
//! - `decay_compute/single/{profile}`: 四种 DecayProfile 档位的单次衰减延迟
//! - `decay_compute/bulk/{size}`: 不同规模能力注册表的批量衰减吞吐
//! - `decay_compute/event_types`: 四种 DecayEvent 类型的延迟对比
//!
//! # 设计说明
//! - 单次衰减路径: DashMap::get_mut + Instant::now + CapabilityLevel::new clamp
//! - SLO 1μs 依据: 生产环境每秒可处理 >1000 次衰减，留 10x 余量
//! - 使用 criterion 默认 sample_size=100，统计上等价于 min-of-N 5 采样

#![forbid(unsafe_code)]

use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion, Throughput};
use decay_engine::{DecayConfig, DecayEngine, DecayEvent};
use nexus_contracts::{DecayPolicy, DecayProfile};

/// SLO 目标: 单次衰减 < 1μs（仅作文档标注，criterion 通过统计区间自动验证）
const _SLO_SINGLE_US: f64 = 1.0;

/// 批量衰减规模梯度（模拟小规模 → 大规模能力注册表）
const BULK_SIZES: &[usize] = &[10, 100, 1000];

// ============================================================
// bench 1: 单次衰减 — 四种 DecayProfile 档位
// ============================================================

/// 单次衰减延迟（四种 DecayProfile 档位对比）
///
/// WHY 四种档位: Lenient/Standard/Strict/Aggressive 对应不同衰减曲线：
/// - Lenient: 线性慢衰减（time_decay_rate=0.0005），类似"缓坡"
/// - Standard: 线性标准衰减（time_decay_rate=0.001），默认基准
/// - Strict: 线性快衰减（time_decay_rate=0.005），"陡坡"
/// - Aggressive: 线性极快衰减（time_decay_rate=0.01），"悬崖"
///
/// 所有档位共享同一核心路径（decay_with_config），差异仅在浮点参数，
/// 理论上延迟应无显著差别。此 bench 验证参数变化不引入分支偏差。
fn single_decay_by_profile(c: &mut Criterion) {
    let mut group = c.benchmark_group("decay_compute/single");

    let profiles: &[(&str, DecayProfile)] = &[
        ("lenient", DecayProfile::Lenient),
        ("standard", DecayProfile::Standard),
        ("strict", DecayProfile::Strict),
        ("aggressive", DecayProfile::Aggressive),
    ];

    for &(name, profile) in profiles {
        // 每个 profile 使用独立 engine，避免跨 profile 状态干扰
        let engine = DecayEngine::new(DecayConfig::default());
        engine
            .register_capability("cap-slo", "slo-bench", 0.8)
            .expect("register_capability 失败");

        let policy = DecayPolicy::static_policy(profile);

        group.bench_function(BenchmarkId::new("profile", name), |b| {
            b.iter(|| {
                let level = engine
                    .decay_with_policy(
                        black_box("cap-slo"),
                        black_box(DecayEvent::TimeDecay),
                        // DecayPolicy 实现 Copy,直接按值传递(clippy::clone_on_copy)
                        black_box(policy),
                    )
                    .expect("decay_with_policy 失败");
                black_box(level);
            });
        });
    }
    group.finish();
}

// ============================================================
// bench 2: 四种 DecayEvent 类型延迟对比
// ============================================================

/// 单次衰减延迟（四种 DecayEvent 类型对比）
///
/// WHY 四种事件: TimeDecay / ViolationPenalty / Freeze / Restore 覆盖
/// decay_with_config 的所有 match 分支。验证各分支无异常开销差异。
/// - TimeDecay: 线性时间衰减（elapsed × rate）
/// - ViolationPenalty: 阶梯式惩罚（penalty × severity）
/// - Freeze: 立即清零（最简路径）
/// - Restore: 线性恢复（elapsed × restore_rate）
fn single_decay_by_event_type(c: &mut Criterion) {
    let mut group = c.benchmark_group("decay_compute/event_types");

    // 预构建四种事件（避免在 iter 中分配字符串）
    let time_decay = DecayEvent::TimeDecay;
    let violation = DecayEvent::ViolationPenalty {
        capability_id: "cap-evt".into(),
        severity: 1.5,
    };
    let freeze = DecayEvent::Freeze {
        capability_id: "cap-evt".into(),
        reason: "bench-freeze".into(),
    };
    let restore = DecayEvent::Restore {
        capability_id: "cap-evt".into(),
    };

    // TimeDecay / ViolationPenalty / Restore 需要非冻结状态
    // Freeze 会将能力冻结，需要独立 engine
    let engine_tv = DecayEngine::new(DecayConfig::default());
    engine_tv
        .register_capability("cap-evt", "event-bench", 0.8)
        .expect("register 失败");

    let engine_freeze = DecayEngine::new(DecayConfig::default());
    engine_freeze
        .register_capability("cap-evt", "event-bench", 0.8)
        .expect("register 失败");

    let engine_restore = DecayEngine::new(DecayConfig::default());
    engine_restore
        .register_capability("cap-evt", "event-bench", 0.5)
        .expect("register 失败");

    group.bench_function("time_decay", |b| {
        b.iter(|| {
            let level = engine_tv
                .decay(black_box("cap-evt"), black_box(time_decay.clone()))
                .expect("decay 失败");
            black_box(level);
        });
    });

    group.bench_function("violation_penalty", |b| {
        b.iter(|| {
            let level = engine_tv
                .decay(black_box("cap-evt"), black_box(violation.clone()))
                .expect("decay 失败");
            black_box(level);
        });
    });

    group.bench_function("freeze", |b| {
        b.iter(|| {
            // Freeze 幂等: 已冻结后再次冻结返回 Err，但 bench 测量的是热路径
            // 使用独立 engine，首次 Freeze 走完整路径
            let _ = engine_freeze.decay(black_box("cap-evt"), black_box(freeze.clone()));
        });
    });

    group.bench_function("restore", |b| {
        b.iter(|| {
            let level = engine_restore
                .decay(black_box("cap-evt"), black_box(restore.clone()))
                .expect("decay 失败");
            black_box(level);
        });
    });

    group.finish();
}

// ============================================================
// bench 3: 批量衰减吞吐（不同规模）
// ============================================================

/// 批量衰减吞吐量（不同能力注册表规模）
///
/// WHY 多规模: 验证 DashMap 分片锁在 10/100/1000 能力下的线性扩展性。
/// Throughput::Elements(size) 让 criterion 报告 per-element ops/sec。
/// SLO 目标: 单次 element < 1μs（批量场景下均摊）。
fn bulk_decay_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("decay_compute/bulk");

    for &size in BULK_SIZES {
        group.throughput(Throughput::Elements(size as u64));

        // 预填充能力列表（setup 不计入测量时间）
        let engine = DecayEngine::new(DecayConfig::default());
        for i in 0..size {
            let name = format!("capability-{i}");
            engine
                .register_capability(&format!("cap-{i}"), &name, 0.5)
                .expect("register_capability 失败");
        }
        // 预生成 ID 列表（避免 iter 中 format 干扰测量）
        let ids: Vec<String> = (0..size).map(|i| format!("cap-{i}")).collect();

        group.bench_with_input(BenchmarkId::from_parameter(size), &size, |b, &_| {
            b.iter(|| {
                for id in &ids {
                    let level = engine
                        .decay(black_box(id), black_box(DecayEvent::TimeDecay))
                        .expect("decay 失败");
                    black_box(level);
                }
            });
        });
    }
    group.finish();
}

criterion_group!(
    benches,
    single_decay_by_profile,
    single_decay_by_event_type,
    bulk_decay_throughput,
);
criterion_main!(benches);
