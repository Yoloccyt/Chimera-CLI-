//! sparse_select_top_k 基准 — L0 热路径掩码选择的 O(n) 不变量证据链（ADR-217 D-36）
//!
//! # 为什么只有它有 bench 资格
//! `SparseMask::select_top_k` 是 L0 中**唯一有生产热路径调用**的函数：
//! chimera-mas 每请求构建提示词（`context/manager.rs` 调 `compute_all_masks`）→
//! osa-coordinator 五维度各调一次（`parallel.rs:211/220/235/259/272`）。
//! 其余三个"零覆盖"函数（`fuse_ranked_lists`/`sinkhorn_project`/`project_weights`）
//! 经实测在本仓**无任何生产调用方**（`recall_hybrid` 与 mcsm 两入口的调用点全在
//! `cfg(test)`），故不为其造 bench——见 ADR-217 D-36 的触发条件改写。
//!
//! # 本文件锁什么
//! 端到端形态已由 osa-coordinator 的 `compute_masks` / `parallel_vs_sequential`
//! bench 覆盖；这里补的是 **L0 单元级**基线，两条对照：
//! 1. `select_top_k`（实现走 `select_nth_unstable_by`，O(n) 平均 + Top-K 局部排序）；
//! 2. `full_sort` 对照组（`sort_by` 全排再截断，O(n log n)）。
//!
//! 若实现哪天回退成全排，(1) 的读数会贴到 (2)——红线 #8 的机器可见形态。
//!
//! # 为什么是 dev-only 而非 registered
//! 读数**不入 SLO 三表**：本仓纪律「无 clean-runner 不标定」（ADR-214 §6.3 /
//! v3.5 P-2·F-1），本机读数不得写成阈值。故仅在
//! `scripts/bench_inventory_freeze.txt` 以 `dev-only` 态登记（先例：
//! `chimera-cli|composition_root_bench`）。
#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench/example code idiom; E-5 targets production code
use criterion::{black_box, criterion_group, criterion_main, BatchSize, Criterion};
use nexus_contracts::{SparseMask, ToolId};

const N_SMALL: usize = 1_000;
const N_LARGE: usize = 10_000;

/// 确定性伪随机分数源（LGC），避免引入 `rand`（L0 零依赖铁律）并使 bench 可复现。
fn scores(len: usize) -> Vec<f32> {
    let mut seed: u64 = 0x9E37_79B9_7F4A_7C15;
    (0..len)
        .map(|_| {
            seed = seed
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            // 取高 24 bit 映射到 [0,1)，比整型转 f32 更稳定
            ((seed >> 40) as f32) / (1_u64 << 24) as f32
        })
        .collect()
}

fn ids_str(len: usize) -> Vec<String> {
    (0..len).map(|i| format!("item-{i}")).collect()
}

fn bench_select_top_k(c: &mut Criterion) {
    for n in [N_SMALL, N_LARGE] {
        let src_ids = ids_str(n);
        let src_scores = scores(n);

        for k in [10usize, n / 10] {
            // (1) 被测：select_nth_unstable_by 路线
            c.bench_function(&format!("select_top_k String n={n} k={k}"), |b| {
                b.iter(|| {
                    let mask = SparseMask::select_top_k(
                        black_box(&src_ids),
                        black_box(&src_scores),
                        black_box(k),
                    );
                    black_box(mask.active_ids.len());
                })
            });

            // (2) 对照组：全排序后截断（红线 #8 禁止的形态，此处仅作回退哨兵）
            c.bench_function(&format!("full_sort_control n={n} k={k}"), |b| {
                b.iter_batched(
                    || {
                        src_scores
                            .iter()
                            .copied()
                            .enumerate()
                            .collect::<Vec<(usize, f32)>>()
                    },
                    |mut indexed| {
                        indexed.sort_by(|a, b| {
                            b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
                        });
                        black_box(indexed.into_iter().take(k).count());
                    },
                    BatchSize::SmallInput,
                )
            });
        }
    }
}

fn bench_tool_id_instantiation(c: &mut Criterion) {
    // 泛型实例化形态（消费方真实类型）：ToolId newtype × n=10^4
    let n = N_LARGE;
    let src_ids: Vec<ToolId> = (0..n).map(|i| ToolId::new(format!("tool-{i}"))).collect();
    let src_scores = scores(n);
    let k = 100;
    c.bench_function(&format!("select_top_k ToolId n={n} k={k}"), |b| {
        b.iter(|| {
            let mask =
                SparseMask::select_top_k(black_box(&src_ids), black_box(&src_scores), black_box(k));
            black_box(mask.sparsity_ratio);
        })
    });
}

criterion_group!(benches, bench_select_top_k, bench_tool_id_instantiation);
criterion_main!(benches);
