//! W4 算子路由选择基准 — 聚合表 O(K) vs 全历史扫描对照（性能可证伪，ADR-084）
//!
//! 基准场景:
//! - `aggregate_select`: W4 聚合实现——预注入 N 条历史后,单次 Greedy 选择延迟
//!   （聚合查询 O(K),K=适用算子数 ≤4,与 N 无关）
//! - `full_scan_baseline`: 旧版全扫描语义参照——测试内从 N 条历史线性扫描
//!   计算成功均分 argmax（O(N) 随历史增长）
//!
//! 预期: N=4096(满窗口)时 aggregate 数量级优于 full_scan;
//! aggregate 在 N=256 与 N=4096 两档延迟基本持平（O(K) 不变性）。

use criterion::{criterion_group, criterion_main, Criterion};
use faae_router::{OperatorRouter, OperatorSelectionRecord};
use gsoe_evolution::OperatorContext;
use nexus_contracts::experience_card::{AtomicOperator, ExecutionStatus};
use nexus_contracts::OperatorSelectionStrategy;

/// 预注入 N 条历史记录（Draft/Improve 交替,分数 0.4~0.9）
fn router_with_history(n: usize) -> OperatorRouter {
    let mut router = OperatorRouter::new(OperatorSelectionStrategy::Greedy);
    for i in 0..n {
        let op = if i % 2 == 0 {
            AtomicOperator::Draft
        } else {
            AtomicOperator::Improve
        };
        let score = 0.4 + (i % 6) as f32 / 10.0;
        router.record_result("bench-task", op, score, ExecutionStatus::Success);
    }
    router
}

/// 默认上下文: Draft/Crossover 适用
fn bench_context() -> OperatorContext {
    OperatorContext {
        task_id: "bench".to_string(),
        task_type: "bench".to_string(),
        parent_card: None,
        error_signature: None,
        requirements: "bench".to_string(),
        code: None,
        card_query: None,
    }
}

/// 全扫描参照实现（旧版 Greedy 语义,O(N)）
fn full_scan_greedy(history: &[OperatorSelectionRecord], task: &str) -> AtomicOperator {
    let ops = [AtomicOperator::Draft, AtomicOperator::Crossover];
    let mut best = ops[0];
    let mut best_score = -1.0f32;
    for op in ops {
        let scores: Vec<f32> = history
            .iter()
            .filter(|r| {
                r.task_type == task
                    && r.selected_operator == op
                    && r.execution_status == ExecutionStatus::Success
            })
            .map(|r| r.result_score)
            .collect();
        let score = if scores.is_empty() {
            0.0
        } else {
            scores.iter().sum::<f32>() / scores.len() as f32
        };
        if score > best_score {
            best_score = score;
            best = op;
        }
    }
    best
}

fn bench_operator_select(c: &mut Criterion) {
    let ctx = bench_context();

    let mut group = c.benchmark_group("operator_select");
    for &n in &[256usize, 4096] {
        // 聚合实现: 预注入历史,测单次选择延迟
        let mut router = router_with_history(n);
        group.bench_function(format!("aggregate/n={n}"), |b| {
            b.iter(|| {
                let selected = router.select_operator("bench-task", &ctx);
                std::hint::black_box(selected);
            });
        });

        // 全扫描参照: 从导出历史线性扫描
        let history = router_with_history(n).export_history();
        group.bench_function(format!("full_scan/n={n}"), |b| {
            b.iter(|| std::hint::black_box(full_scan_greedy(&history, "bench-task")));
        });
    }
    group.finish();
}

criterion_group!(benches, bench_operator_select);
criterion_main!(benches);
