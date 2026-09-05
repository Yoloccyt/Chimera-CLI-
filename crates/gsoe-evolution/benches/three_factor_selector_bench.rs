//! 三因子父本选择器 criterion 基准 — v3.4.0 §10.2 OpenMLE 核心算法
//!
//! 对应架构层: L5 Knowledge(gsoe-evolution)
//! 对应设计源: `Chimera_CLI_v3.4.0_omega_..._权威版.md` §10.2
//! 对应 ADR: ADR-049 决策 1 + ADR-094(三因子父本选择: UCB+Softmax+冷却)
//!
//! # 基准清单
//!
//! ## 1. three_factor_select_8_candidates — 典型候选池选择
//!
//! 覆盖 8 个候选的 select() 完整路径:三因子归一化 + UCB bonus + 冷却因子 +
//! Softmax 温度采样。代表父本池的常规规模,预期延迟数百 ns 级。
//!
//! ## 2. three_factor_select_64_candidates — 大候选池选择
//!
//! 覆盖 64 个候选的选择路径,验证归一化/Softmax 随候选数扩展的开销,
//! 用于确认三因子选择不成为父本选择的关键路径瓶颈。
//!
//! # 基准配置
//!
//! - warmup: 500ms(与 channel_b_benchmark / evolution_benchmark 一致)
//! - sample_size: 20(小样本快速验证)
//!
//! # 对齐声明
//!
//! 基准覆盖 §10.2 全文公式,为"三因子选择性能"声明提供数据证据(§0 性能证据铁律)。

use chrono::Utc;
use criterion::{black_box, criterion_group, criterion_main, Criterion};
use gsoe_evolution::ThreeFactorSelector;
use nexus_contracts::experience_card::{AtomicOperator, CardMetadata, ExecutionStatus};
use nexus_contracts::{ExperienceCard, ThreeFactorScore};
use std::time::Duration;

/// 构造带三因子评分的候选 experience card
fn make_card(node: &str, quality: f32, progress: f32, novelty: f32) -> ExperienceCard {
    ExperienceCard {
        card_id: format!("card-{node}").into(),
        task_id: "task-bench".into(),
        node_id: node.into(),
        parent_id: None,
        created_at: Utc::now(),
        operator: AtomicOperator::Draft,
        score: quality,
        delta_vs_parent: progress,
        method_family: "bench".into(),
        error_signature: None,
        three_factor: ThreeFactorScore {
            quality,
            progress,
            novelty,
        },
        execution_status: ExecutionStatus::Success,
        token_evidence_ids: Vec::new(),
        segment_id: None,
        metadata: CardMetadata::default(),
    }
}

/// 构造 n 个候选:三因子分数随索引小幅波动,避免退化为纯 UCB/纯利用基准
fn make_bench_candidates(n: usize) -> Vec<ExperienceCard> {
    (0..n)
        .map(|i| {
            let t = (i as f32) / n.max(1) as f32;
            make_card(
                &format!("n{i}"),
                0.1 + 0.8 * t,
                0.05 * (i % 5) as f32,
                0.2 + 0.7 * ((i * 7) % 13) as f32 / 13.0,
            )
        })
        .collect()
}

/// 基准:8 候选池 select()
fn bench_three_factor_select_8(c: &mut Criterion) {
    let candidates = make_bench_candidates(8);
    c.bench_function("three_factor_select_8_candidates", |b| {
        b.iter(|| {
            let mut selector = ThreeFactorSelector::new(1.414, 0.005, 0.5);
            black_box(selector.select(&candidates)).expect("选择成功")
        });
    });
}

/// 基准:64 候选池 select()
fn bench_three_factor_select_64(c: &mut Criterion) {
    let candidates = make_bench_candidates(64);
    c.bench_function("three_factor_select_64_candidates", |b| {
        b.iter(|| {
            let mut selector = ThreeFactorSelector::new(1.414, 0.005, 0.5);
            black_box(selector.select(&candidates)).expect("选择成功")
        });
    });
}

criterion_group! {
    name = benches;
    config = Criterion::default()
        .sample_size(20)
        .warm_up_time(Duration::from_millis(500));
    targets = bench_three_factor_select_8,
        bench_three_factor_select_64
}

criterion_main!(benches);
