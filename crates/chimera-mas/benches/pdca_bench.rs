//! PDCA 闭环 + 专家反馈热路径性能基准(专家 Agent 优化 2026-08-11)
//!
//! # 基准覆盖
//! - `pdca_act`: Act 阶段纯函数热路径(全局参数调整)
//! - `pdca_act_with_feedback`: 带专家反馈的 Act(专家级建议生成)
//! - `pdca_plan_reflux`: Plan 阶段回流(目标指标 + 行动项)
//! - `feedback_record_outcome`: 专家反馈上报热路径(DashMap 写入)
//! - `feedback_priority_adjustments`: 专家级建议生成(全量扫描 + 排序)
//!
//! # 性能可证伪(§3.4.1 第 6 条)
//! 上述路径均为毫秒级以下纯计算,基准提供优化前后量化对比依据。

use chimera_mas::feedback::ExpertFeedbackRegistry;
use chimera_mas::pdca::{PdcaLoop, PdcaMetrics};
use criterion::{criterion_group, criterion_main, Criterion};

fn bench_pdca_act(c: &mut Criterion) {
    let loop_ = PdcaLoop::new();
    let healthy = PdcaMetrics::new(50.0, 0.0, 0.0, 0.0, 1000, 0.5);
    let stressed = PdcaMetrics::new(120.0, 0.2, 15.0, 0.08, 15_000, 3.0);

    c.bench_function("pdca_act/healthy", |b| {
        b.iter(|| criterion::black_box(loop_.act(&healthy).expect("act ok")));
    });
    c.bench_function("pdca_act/stressed", |b| {
        b.iter(|| criterion::black_box(loop_.act(&stressed).expect("act ok")));
    });
}

fn bench_pdca_act_with_feedback(c: &mut Criterion) {
    let loop_ = PdcaLoop::new();
    let metrics = PdcaMetrics::new(50.0, 0.0, 0.0, 0.0, 1000, 0.5);

    // 8 专家反馈池(模拟 E01-E08 全部上报 10+ 次)
    let feedback = ExpertFeedbackRegistry::new();
    for i in 0..8u32 {
        for j in 0..12u32 {
            let success = (i + j) % 3 != 0; // 约 2/3 成功率
            feedback.record_outcome(&format!("E0{}", i + 1), success, 5.0 + j as f32);
        }
    }

    c.bench_function("pdca_act_with_feedback/8_experts", |b| {
        b.iter(|| {
            criterion::black_box(
                loop_
                    .act_with_feedback(&metrics, &feedback)
                    .expect("act ok"),
            )
        });
    });
}

fn bench_pdca_plan_reflux(c: &mut Criterion) {
    let loop_ = PdcaLoop::new();
    let metrics = PdcaMetrics::new(100.0, 0.1, 5.0, 0.04, 1000, 1.0);
    let adjustments = loop_.act(&metrics).expect("act ok");

    c.bench_function("pdca_plan_reflux", |b| {
        b.iter(|| {
            criterion::black_box(loop_.plan_reflux(&metrics, &adjustments).expect("plan ok"))
        });
    });
}

fn bench_feedback_record_outcome(c: &mut Criterion) {
    let feedback = ExpertFeedbackRegistry::new();

    c.bench_function("feedback_record_outcome", |b| {
        let mut i = 0u64;
        b.iter(|| {
            feedback.record_outcome(&format!("E{}", i % 8), true, 5.0);
            i += 1;
        });
    });
}

fn bench_feedback_priority_adjustments(c: &mut Criterion) {
    let feedback = ExpertFeedbackRegistry::new();
    for i in 0..32u32 {
        for j in 0..12u32 {
            let success = (i + j) % 2 == 0;
            feedback.record_outcome(&format!("exp-{i}"), success, 5.0);
        }
    }

    c.bench_function("feedback_priority_adjustments/32_experts", |b| {
        b.iter(|| criterion::black_box(feedback.priority_adjustments(10)));
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(100)
        .warm_up_time(std::time::Duration::from_millis(500));
    targets = bench_pdca_act, bench_pdca_act_with_feedback, bench_pdca_plan_reflux, bench_feedback_record_outcome, bench_feedback_priority_adjustments
}

criterion_main!(benches);
