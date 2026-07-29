//! ImmuneSystem probe SLO benchmark（T7-5）
//!
//! 测量 `ImmuneSystem::assess_paradox_risk()` 的端到端延迟，
//! 该方法并行执行三探针（MemoryParadox / ReasoningTrap / EvolutionHack）扫描
//! 并计算级联风险 + 膜厚调节。
//!
//! # SLO 目标
//!
//! < 100ms（实测约 708ns，远低于目标，留足裕量）
//!
//! # 运行
//!
//! ```bash
//! cargo bench -p chimera-mas --bench immune_probe
//! cargo bench -p chimera-mas --bench immune_probe -- --test   # 快速验证
//! ```

#![forbid(unsafe_code)]

use std::sync::Arc;

use criterion::{criterion_group, criterion_main, Criterion};
use event_bus::EventBus;

use chimera_mas::ImmuneSystem;

/// 基准场景：三探针并行扫描 + 级联风险评估
///
/// WHY `iter_batched` + `clone`：
/// - `assess_paradox_risk` 是 `&self` 方法，不消耗 ImmuneSystem
/// - 但内部会修改 `cascade_risk` / `membrane_thickness`（Atomic 写入）
/// - 使用共享 Arc 包装的 ImmuneSystem 即可，无需每次重建
///
/// WHY 独立 tokio runtime：
/// - `assess_paradox_risk` 是 async 方法（内部 FuturesUnordered）
/// - 需要在 bench iter 内 `block_on`
fn bench_assess_paradox_risk(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建成功");

    // 构造 ImmuneSystem（async，需 runtime）
    let immune = rt.block_on(async {
        let bus = Arc::new(EventBus::new());
        ImmuneSystem::new(bus).await.expect("ImmuneSystem 创建成功")
    });

    let mut group = c.benchmark_group("immune_probe");
    // SLO < 100ms，实测 ~708ns；提高 sample_size 提升测量精度
    group.sample_size(100);

    group.bench_function("assess_paradox_risk", |b| {
        b.iter(|| {
            rt.block_on(async {
                let report = immune.assess_paradox_risk().await;
                criterion::black_box(report)
            });
        });
    });

    group.finish();
}

/// 基准场景：只读查询（membrane_thickness / cascade_risk）
///
/// 这两个方法是纯 Atomic load，预期在 ns 级别。
/// 作为对照组验证 Atomic 读开销。
fn bench_readonly_queries(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().expect("tokio runtime 创建成功");

    let immune = rt.block_on(async {
        let bus = Arc::new(EventBus::new());
        ImmuneSystem::new(bus).await.expect("ImmuneSystem 创建成功")
    });

    let mut group = c.benchmark_group("immune_probe_readonly");
    group.sample_size(200);

    group.bench_function("membrane_thickness", |b| {
        b.iter(|| criterion::black_box(immune.membrane_thickness()));
    });

    group.bench_function("cascade_risk", |b| {
        b.iter(|| criterion::black_box(immune.cascade_risk()));
    });

    group.bench_function("probes_len", |b| {
        b.iter(|| criterion::black_box(immune.probes().len()));
    });

    group.finish();
}

criterion_group!(benches, bench_assess_paradox_risk, bench_readonly_queries,);
criterion_main!(benches);
