//! DECB 预算系数计算性能基准 — criterion 基准测试
//!
//! 对应 SubTask 34.6
//!
//! # 基准配置
//! - warmup: 500ms
//! - measurement: 100 次采样
//! - 测量预算系数计算延迟(目标 < 1ms)

use chrono::Utc;
use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use decb_governor::{DecbConfig, DecbGovernor, QuestBudgetInput};

/// 基准:简单 Quest 预算系数计算(单任务,无依赖,无 deadline)
fn bench_compute_budget_simple(c: &mut Criterion) {
    let governor = DecbGovernor::new(DecbConfig::default()).unwrap();
    let quest = QuestBudgetInput::simple("quest-bench-simple");

    c.bench_with_input(
        BenchmarkId::new("compute_budget", "simple"),
        &(&governor, &quest),
        |b, &(gov, q)| {
            b.iter(|| gov.compute_budget(q));
        },
    );
}

/// 基准:复杂 Quest 预算系数计算(20 任务,深度 5,无 deadline)
fn bench_compute_budget_complex(c: &mut Criterion) {
    let governor = DecbGovernor::new(DecbConfig::default()).unwrap();
    let quest = QuestBudgetInput::new("quest-bench-complex", 20, 5, None, 1000);

    c.bench_with_input(
        BenchmarkId::new("compute_budget", "complex"),
        &(&governor, &quest),
        |b, &(gov, q)| {
            b.iter(|| gov.compute_budget(q));
        },
    );
}

/// 基准:紧急 Quest 预算系数计算(有 deadline,30 分钟内)
fn bench_compute_budget_urgent(c: &mut Criterion) {
    let governor = DecbGovernor::new(DecbConfig::default()).unwrap();
    let deadline = Utc::now() + chrono::Duration::minutes(30);
    let quest = QuestBudgetInput::new("quest-bench-urgent", 10, 3, Some(deadline), 500);

    c.bench_with_input(
        BenchmarkId::new("compute_budget", "urgent"),
        &(&governor, &quest),
        |b, &(gov, q)| {
            b.iter(|| gov.compute_budget(q));
        },
    );
}

/// 基准:档位判定延迟
fn bench_determine_tier(c: &mut Criterion) {
    let governor = DecbGovernor::new(DecbConfig::default()).unwrap();

    c.bench_function("determine_tier", |b| {
        b.iter(|| {
            // 遍历不同系数,覆盖三档
            for coef in [0.1_f32, 0.4, 0.8] {
                let _ = governor.determine_tier(coef);
            }
        });
    });
}

/// 基准:消耗记录延迟(无溢出)
fn bench_record_consumption(c: &mut Criterion) {
    let governor = DecbGovernor::new(DecbConfig {
        total_budget_limit: 1e12, // 超大预算避免触发降级
        ..DecbConfig::default()
    })
    .unwrap();
    let consumption = decb_governor::BudgetConsumption::new(100, 1, 1);

    c.bench_function("record_consumption", |b| {
        b.iter(|| {
            let _ = governor.record_consumption(&consumption);
        });
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(100)
        .warm_up_time(std::time::Duration::from_millis(500));
    targets = bench_compute_budget_simple,
              bench_compute_budget_complex,
              bench_compute_budget_urgent,
              bench_determine_tier,
              bench_record_consumption
}

criterion_main!(benches);
