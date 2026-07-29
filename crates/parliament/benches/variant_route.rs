//! 变体池路由性能基准(closure Stage C-12)
//!
//! 对应架构层: L8 Parliament(parliament)
//! 对应 ADR: ADR-051 决策 2(规则式三级路由)+ ADR-049 决策 6(性能可证伪)
//! 对应验收门禁: **满池(64 变体)三级路由 < 1µs**
//! (路由在任务分派热路径上,Vec 线性扫描的设计前提是池上限 64 时
//! 开销可忽略——本 bench 将该前提固化为可回归证据)
//!
//! # 运行
//!
//! ```powershell
//! cargo bench -p parliament --bench variant_route
//! ```

use criterion::{black_box, criterion_group, criterion_main, Criterion};
use nexus_contracts::{VariantContract, VariantId};
use parliament::VariantPool;

/// 构建满池(16 任务类型 × 每类型 4 变体 = 64,触达 ADR-051 决策 4 双上限)
fn full_pool() -> VariantPool {
    let mut pool = VariantPool::new();
    for t in 0..16 {
        for v in 0..4 {
            pool.register(VariantContract::new(
                VariantId::new(format!("spec-{t}-{v}"), v + 1),
                vec![format!("task-type-{t}")],
                0.5 + (v as f32) * 0.1,
                0.2,
            ));
        }
    }
    pool
}

/// 精确匹配路由(第一级:命中 task_types 且取 expected_performance 最高)
fn route_exact_hit(c: &mut Criterion) {
    let pool = full_pool();
    c.bench_function("variant_pool/route_exact_64", |b| {
        b.iter(|| {
            // 轮询不同任务类型,避免分支预测器过拟合单一键
            for t in 0..16 {
                let hit = pool.route(black_box(&format!("task-type-{t}")));
                black_box(hit);
            }
        });
    });
}

/// 未命中路由(第三级:无精确匹配且无通用兜底 → None,全扫路径)
fn route_miss(c: &mut Criterion) {
    let pool = full_pool();
    c.bench_function("variant_pool/route_miss_64", |b| {
        b.iter(|| {
            let hit = pool.route(black_box("nonexistent-task-type"));
            black_box(hit);
        });
    });
}

/// 兜底路由(第二级:含通用变体的池,未知类型命中兜底)
fn route_fallback(c: &mut Criterion) {
    let mut pool = full_pool();
    // 通用变体(task_types 为空 = 兜底,ADR-051 决策 2)
    pool.register(VariantContract::new(
        VariantId::new("generic-spec", 1),
        Vec::new(),
        0.6,
        0.2,
    ));
    c.bench_function("variant_pool/route_fallback_65", |b| {
        b.iter(|| {
            let hit = pool.route(black_box("unknown-task-type"));
            black_box(hit);
        });
    });
}

criterion_group!(benches, route_exact_hit, route_miss, route_fallback);
criterion_main!(benches);
