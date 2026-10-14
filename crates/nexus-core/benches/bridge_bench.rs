//! ComputeBridge 桥接性能基准(route 三态派发 + reduce 双模式归约)
//!
//! 对应任务:P1-T8(手册 §8.3 L-f / §11.1 契约),bridge roundtrip 门禁基准
//! 对应架构层:L1 Core
//!
//! # 重建说明
//! ⚠️ 本文件于 **2026-08-28 磁盘 ENOSPC 事故中数据丢失**,现基于源码
//! (`compute/bridge.rs`) 公共 API 做功能重建。
//!
//! # 测量口径（ADR-198 D3）
//! 基准必须固定 `CHIMERA_COMPUTE_CALIBRATION=off`。原因：校准开启后会在被采样的
//! 1/64 调用上探测式改走 Inline 路径，属路径变更；开着校准测量会让同一机器
//! 同一代码得出不可复现的数字（本仓实测同机跨轮方差可达 ±40%，不该再叠一个可变开关）。
//!
//! # 基准场景
//! - `bridge_route`:全登记类（`TaskKind::ALL`）经 [`ComputeBridge::route`] 的三态派发延迟
//!   (纳秒级查表,ADR-127);
//! - `bridge_reduce`:经桥委托 [`reduce`](nexus_core::compute::reduce::reduce)
//!   的 Deterministic / Audit 双模式归约延迟。
//! - `bridge_spawn`:经 [`spawn_compute_batch`](ComputeBridge::spawn_compute_batch)
//!   的批量卸载往返延迟（ADR-197 新增 —— 该路径上多了一对按类型计数与
//!   一次饱和判定，旧 bench 完全测不到，故补此组作为现势基线）。
//! - `overwindow_dense_knee`:超窗簇 provider 稠密内核「串行 vs 池」n* 拐点曲线
//!   （WI-G6a；只产出数据不改默认，判读属静默窗口批次）。
//!
//! # 参考样板
//! 语法结构参照同目录完整保留的 `clv_bench.rs`。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use nexus_core::compute::reduce::{reduce, ReduceMode};
use nexus_core::compute::{bridge, ComputeBridge, TaskKind};

/// bench 1:全登记类任务经 `bridge().route(kind, n)` 的路由派发延迟
///
/// WHY 覆盖全登记类(遍历 `TaskKind::ALL` 单源):route 查表命中不同数组槽位,
/// 验证 arc-swap RCU 快照 + 常数数组索引在每类上保持纳秒级下限。
fn bridge_route(c: &mut Criterion) {
    let b: &ComputeBridge = bridge();
    let mut group = c.benchmark_group("bridge_route");
    for kind in TaskKind::ALL {
        group.bench_function(BenchmarkId::new("kind", format!("{kind:?}")), |be| {
            be.iter(|| {
                let plan = b.route(kind, 64);
                criterion::black_box(plan);
            });
        });
    }
    group.finish();
}

/// bench 2:经桥委托的 Deterministic / Audit 双模式归约
///
/// WHY 桥 `reduce(&vals, mode)` 委托 `reduce` 模块纯函数;两模式共享
/// 同输入,验证桥接层零额外开销(返回 f64)。
fn bridge_reduce(c: &mut Criterion) {
    let vals: Vec<f64> = (0..16_384).map(|i| (i as f64).sin()).collect();
    let b: &ComputeBridge = bridge();
    let mut group = c.benchmark_group("bridge_reduce");
    for mode in [ReduceMode::Deterministic, ReduceMode::Audit] {
        group.bench_function(BenchmarkId::new("mode", format!("{mode:?}")), |be| {
            be.iter(|| {
                let r = b.reduce(&vals, mode);
                criterion::black_box(r);
            });
        });
    }
    // 对照:直调 reduce 模块(基准桥委托的等价性依据)
    group.bench_function("direct_reduce_deterministic", |be| {
        be.iter(|| {
            let r = reduce(&vals, ReduceMode::Deterministic);
            criterion::black_box(r);
        });
    });
    group.finish();
}

/// bench 3:批量卸载往返延迟（ADR-197 新增）
///
/// WHY 必须单独一组：`bridge_route` 只测查表，`bridge_reduce` 只测归约，
/// 而饱和判定与计数都落在 `spawn_compute*` 路径上——若不测，本批改动的
/// 性能影响就是“无数据可辩”。本组不新建 SLO 阈值（避免未实测先登记，
/// ADR-190 反幽灵门），仅作为现势基线与后续 Phase B/D 的对照起点。
fn bridge_spawn(c: &mut Criterion) {
    let b: &ComputeBridge = bridge();
    let mut group = c.benchmark_group("bridge_spawn");
    for n in [1usize, 16, 128] {
        group.bench_function(BenchmarkId::new("batch", n), |be| {
            be.iter(|| {
                // 每次迭代重建闭包集:连同 Vec 分配一起计入（真实调用方的形状）
                let closures = (0..n).map(|i| move || criterion::black_box(i) as u64 + 1);
                let results = b.spawn_compute_batch(TaskKind::ClvSimilarity, closures);
                criterion::black_box(results.len());
            });
        });
    }
    group.finish();
}

/// bench 3b:饱和判定本身的开销上界（ADR-197）
///
/// WHY 单独测：batch 往返由 rayon scope + 槽位互斥量主导，若只报往返数字，
/// 就无法回答“你新加的判定/计数到底贵不贵”。本组只测
/// [`is_saturated`](ComputeBridge::is_saturated)（一次原子读 + 一次千分比
/// 运算 + 一次阈值读），即本次改动在每调用上的成本上界。
fn saturation_check(c: &mut Criterion) {
    let b: &ComputeBridge = bridge();
    let mut group = c.benchmark_group("bridge_saturation_check");
    group.bench_function("is_saturated", |be| {
        be.iter(|| criterion::black_box(b.is_saturated()));
    });
    group.finish();
}

/// bench 4:结果汇聚路径定量（第三轮 T2.1，先量后改）
///
/// WHY 单独一组：`bridge.rs:429` 以「8 线程 × 槽位锁开销可忽略」为据选用
/// 共享 `Mutex<Vec<Option<Result>>>` 逐任务写唯一槽位的汇聚方式（骨架阶段
/// 正确性优先），但该断言从未经 n 增长的实测背书。本组隔成两条可对比分母/分子
/// 的曲线（n 固定 100/1k/5k/10k，与 T3.3 静默窗口取数口径对齐）：
///   - `pool_trivial`：闭包几乎零工作 → 时间≈「汇聚机器」开销（pool scope +
///     每任务锁槽位 + 末尾 into_inner().collect），即被审计的那块开销；
///   - `pool_with_work`：每闭包固定 64 次整数加（模拟真实轻任务的稳定基线）。
///
/// 判读（静默窗口取数后人算占比）：overhead_share = pool_trivial / pool_with_work。
/// <5% ⇒ 维持 Mutex 方案、以实测替换 `bridge.rs:429` 注释后关闭；5-15% ⇒
/// [Directional] 登记不动；≥15% ⇒ 触发 T2.2b（per-slot 原子汇聚，ADR-125 方向，先立 spec）。
/// 不新建 SLO 阈值（避免未实测先登记，ADR-190 反幽灵门）；非静默窗口读数一律标
/// [Directional] 且不得据此触发改造（R5）。
fn batch_result_collect(c: &mut Criterion) {
    let b: &ComputeBridge = bridge();
    let mut group = c.benchmark_group("batch_result_collect");
    for n in [100usize, 1_000, 5_000, 10_000] {
        group.bench_function(BenchmarkId::new("pool_trivial", n), |be| {
            be.iter(|| {
                let closures = (0..n).map(|i| move || criterion::black_box(i) as u64);
                let r = b.spawn_compute_batch(TaskKind::ClvSimilarity, closures);
                criterion::black_box(r.len());
            });
        });
        group.bench_function(BenchmarkId::new("pool_with_work", n), |be| {
            be.iter(|| {
                let closures = (0..n).map(|i| {
                    move || {
                        let mut acc = criterion::black_box(i) as u64;
                        for _ in 0..64 {
                            acc = acc.wrapping_add(1);
                        }
                        acc
                    }
                });
                let r = b.spawn_compute_batch(TaskKind::ClvSimilarity, closures);
                criterion::black_box(r.len());
            });
        });
    }
    group.finish();
}

/// bench 5:overwindow 桥 dense 内核 n* 拐点曲线（WI-G6a，先量后建议）
///
/// WHY: `cpu-plane-c1c2-scale-bounding-2026-10-02.md` §3 未闭合项——provider
/// dense 段（块 CLV × 查询 CLV 余弦 + top-k 部分选择）生产可达 n ∈ [513, 3906]
/// 块，是否值得卸载取决于「串行 vs 池」交叉点 n* 是否低于真实 p50。本组按
/// 现状内核形状（CHUNK = 64 分段，段内保序）对拍两条曲线：
///   - `serial`：逐块余弦 + `select_nth_unstable_by` + sort（现状路径形状）；
///   - `offload`：CHUNK=64 → `spawn_compute_batch` → 段序拼接 + 同款 top-k。
///
/// 判读口径（R5）：offload < serial 的最小 n 即 n*；非静默窗口读数一律标
/// `[Directional]` 且不得据此触发改造。本组**不新建 SLO 阈值、不改任何默认**
/// （ADR-190 反幽灵门；apply 建议见 WI-G6a 报告）。
///
/// # v3.5 M3 方法学升级（两个组 + 多轮同号判据）
/// 历史单轮 7 点读数不足以定拐点（本轮已得反证：n=256 时 offload 快 1.295×，
/// 与给出 `n*≈287` 那一轮符号相反）。因此本组现同时产出：
///   - `overwindow_dense_knee`（正序，全 8 点）：同轮内 serial/offload 配对比值；
///   - `overwindow_dense_knee_rev`（反序，拐点候选 3 点）：**次序对照**，用于识别
///     “先跑者拿冷页/刚分配内存”的次序伪影。
///
/// 定拐点的硬判据：每点 ≥ 4/5 轮 median 同号，**且**两种注册次序下结论一致；
/// 不满足即记“n* 不可辨识（落在噪声带或采样点左侧）”，不得据此改阈值。
///
/// 内核复刻声明：`compute_provider_parts`（chimera-cli）为私有函数，本组按其
/// dense 段形状（cosine + select + sort，k = 128）在 bench 侧复刻——形状等价、
/// 非逐位对拍；数据只用于 n* 量级判读。
#[allow(clippy::type_complexity)] // 卸载闭包包型与生产点位同形（hcw/faae 同范式）
fn overwindow_dense_knee(c: &mut Criterion) {
    use std::sync::Arc;

    const K: usize = 128; // DEFAULT_TOP_K（chimera-cli/overwindow_bridge.rs）
    const CHUNK: usize = 64; // 生产点位 CHUNK 对齐

    /// 确定性块 CLV（SplitMix64 派生；512 维）
    fn make_clv(seed: u64) -> nexus_core::CLV {
        let mut s = seed
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(0x1234_5678);
        let v: Vec<f32> = (0..512)
            .map(|_| {
                s ^= s << 13;
                s ^= s >> 7;
                s ^= s << 17;
                ((s >> 40) as f32 / 16_777_216.0f32) - 0.5
            })
            .collect();
        nexus_core::CLV::from_vec(v).expect("512 维构造")
    }

    /// top-k 部分选择 + 排序（与 provider dense 段同款比较器）
    fn topk(scores: &mut Vec<(usize, f32)>, k: usize) {
        let n = scores.len();
        let nth = k.min(n);
        if nth > 0 && nth < n {
            scores.select_nth_unstable_by(nth - 1, |a, b| {
                b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
            });
            scores.truncate(nth);
        }
        scores.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    }

    let b: &ComputeBridge = bridge();
    let query = make_clv(0x00C0_FFEE);

    // 采样点集。v3.5 M3 新增 **384**：历史读数 `n*≈287` 是由 {256, 512} 两点
    // 线性插值得出的——点集**跨过 287 却不含任何落在 (256,512) 的点**，因此拐点
    // 位置在结构上不可证伪（本轮实测已得 n=256 时 offload 已快 1.295×，与
    // 给出 287 那一轮符号相反）。补 384 后 (256,512) 可被夹住。
    const SIZES: [usize; 8] = [256, 384, 512, 1024, 2048, 4096, 8192, 16_384];
    // 反向对照组只跑拐点候选区（控制成本；大 n 段两种次序已无歧义）。
    const REV_SIZES: [usize; 3] = [256, 384, 512];

    fn make_blocks(n: usize) -> Arc<Vec<nexus_core::CLV>> {
        // 必须先把迭代器 collect 成 Vec 再包 Arc：`Arc<Vec<T>>` 不能直接从
        // 迭代器 build（E0277，本轮实测踩到）。
        Arc::new(
            (0..n)
                .map(|i| make_clv(i as u64 + 1))
                .collect::<Vec<nexus_core::CLV>>(),
        )
    }

    // 两个测量形状写成宏而非闭包：`BenchmarkGroup<'_, M>` 带生命周期参数且
    // `ValueFormatter` 在 criterion 0.5 是 **trait**（把它当类型写会 E0782），
    // 闭包参数标注代价高于收益；宏不需要类型标注，两组间亦不重复内核代码。
    macro_rules! bench_serial {
        ($group:expr, $blocks:expr, $n:expr) => {
            $group.bench_function(BenchmarkId::new("serial", $n), |be| {
                be.iter(|| {
                    let mut scores: Vec<(usize, f32)> = $blocks
                        .iter()
                        .enumerate()
                        .map(|(i, blk)| (i, query.cosine_similarity(blk)))
                        .collect();
                    topk(&mut scores, K);
                    criterion::black_box(scores.len())
                });
            });
        };
    }

    macro_rules! bench_offload {
        ($group:expr, $blocks:expr, $n:expr) => {
            $group.bench_function(BenchmarkId::new("offload", $n), |be| {
                be.iter(|| {
                    let n_chunks = $blocks.len().div_ceil(CHUNK);
                    let tasks: Vec<Box<dyn FnOnce() -> Vec<(usize, f32)> + Send>> = (0..n_chunks)
                        .map(|ci| {
                            let all = Arc::clone($blocks);
                            let q = query.clone();
                            let start = ci * CHUNK;
                            let end = (start + CHUNK).min(all.len());
                            Box::new(move || {
                                (start..end)
                                    .map(|i| (i, q.cosine_similarity(&all[i])))
                                    .collect()
                            }) as Box<dyn FnOnce() -> Vec<(usize, f32)> + Send>
                        })
                        .collect();
                    let results = b.spawn_compute_batch(TaskKind::ClvSimilarity, tasks);
                    let mut scores: Vec<(usize, f32)> = Vec::with_capacity($n);
                    for r in results {
                        scores.extend(r.unwrap());
                    }
                    topk(&mut scores, K);
                    criterion::black_box(scores.len())
                });
            });
        };
    }

    // 主组：正序（serial → offload），覆盖全点集。
    let mut group = c.benchmark_group("overwindow_dense_knee");
    for n in SIZES {
        let blocks = make_blocks(n);
        bench_serial!(&mut group, &blocks, n);
        bench_offload!(&mut group, &blocks, n);
    }
    group.finish();

    // 对照组：反序（offload → serial），只跑拐点候选区。
    // WHY：主组里 serial 恒先跑，会系统性地拿到“先跑者拿冷页/刚分配内存”的次序
    // 偏置。拐点的判据是“每点 ≥ 4/5 轮 median 同号”，而**换序后仍同号**才能排除
    // 次序伪影；若换序使符号翻转，那说明所谓拐点落在噪声带内，不得据此改阈值。
    let mut rgroup = c.benchmark_group("overwindow_dense_knee_rev");
    for n in REV_SIZES {
        let blocks = make_blocks(n);
        bench_offload!(&mut rgroup, &blocks, n);
        bench_serial!(&mut rgroup, &blocks, n);
    }
    rgroup.finish();
}

criterion_group!(
    benches,
    bridge_route,
    bridge_reduce,
    bridge_spawn,
    saturation_check,
    batch_result_collect,
    overwindow_dense_knee
);
criterion_main!(benches);
