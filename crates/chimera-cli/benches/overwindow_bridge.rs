//! 超窗兜底路径性能基准（PROBE P5）
//!
//! # 基准场景
//!
//! - **set_corpus_600k**: 60 万 token 语料 chunk 化 + kvbsr 聚类构成量化
//!   （chunking O(字符数) + 每块 512 维 CLV 生成 + Union-Find 聚类）
//! - **provider_select_2344**: dense（CLV 余弦）+ sparse（关键词命中）+ RRF 融合全链路。
//!   ★ 名称里的 2344 是**历史标识**（按 256 token/块的估算），非实测规模：60 万 token 语料
//!   聚类后实为 **3684 块**（v3.5 M6 用 `block_count()` 自报，见 `ADR-196 §12` 结论 4）。
//!   重命名要同批改三处 bench 登记面，本批只改注释口径，改名待裁决。
//! - **provider_select_24k**: 规模化对照（P3 部分选择优化判据）；“24K 块”同样是用已被否证的
//!   256 token/块换算出来的**估算值【待验证】**，本组未自报块数
//! - **e2e_dense_ab**（v3.5 M6）: 同一语料上“走池 vs 纯 inline”的**端到端**配对对照，
//!   用于回答“dense 段隔离收益能否外推到全链路”（P-2 改阈判据的前置）
//!
//! # 约定
//!
//! 只输出分布不 panic（对齐 probe_select.rs 约定）。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code
use chimera_cli::overwindow_bridge::OverWindowBridge;
use criterion::{black_box, criterion_group, criterion_main, BenchmarkId, Criterion};

/// 构造指定规模语料（重复段填充；token 估算 = 字符数 / 4）
fn make_corpus(target_tokens: usize) -> String {
    let base = "模块A 处理请求路由与鉴权，模块B 负责缓存失效与回写，模块C 执行语义检索。";
    // 每段约 30 字符 ≈ 7.5 token → 段数 = target_tokens * 4 / 30
    let segments = (target_tokens * 4 / 30).max(1);
    let mut corpus = String::with_capacity(target_tokens * 4);
    for i in 0..segments {
        corpus.push_str(&format!("{base} 段{i} "));
    }
    corpus
}

fn bench_set_corpus_600k(c: &mut Criterion) {
    let bus = event_bus::EventBus::new();
    let bridge = OverWindowBridge::new(bus).unwrap();
    let corpus = make_corpus(600_000);

    c.bench_function("overwindow_set_corpus_600k_tokens", |b| {
        // set_corpus 返回 unit——black_box 包裹会触发 clippy::unit_arg（对齐 linucb_select 约定）
        b.iter(|| bridge.set_corpus(black_box(&corpus)))
    });
}

fn bench_provider_select_2344(c: &mut Criterion) {
    let bus = event_bus::EventBus::new();
    let bridge = OverWindowBridge::new(bus).unwrap();
    bridge.set_corpus(&make_corpus(600_000)); // 实测 3684 块；旧注释“≈2344 块”是按 256 token/块的估算，已被 v3.5 M6 否证
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("overwindow_provider_select_2344", |b| {
        b.iter(|| {
            // 超窗触发（1M 宣称 → 折减 600K < 70 万 token 语料）
            black_box(rt.block_on(bridge.run("语义检索", 700_000, 600_000)))
        })
    });
}

fn bench_provider_select_24k(c: &mut Criterion) {
    let bus = event_bus::EventBus::new();
    let bridge = OverWindowBridge::new(bus).unwrap();
    bridge.set_corpus(&make_corpus(6_000_000)); // 标称 24K 块 = 同一失真估算【待验证】，未用 block_count() 自报
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("overwindow_provider_select_24k", |b| {
        b.iter(|| black_box(rt.block_on(bridge.run("语义检索", 6_000_000, 600_000))))
    });
}

/// M6（v3.5 W6-1b）：端到端「走池 vs 纯 inline」**同 n 配对** A/B。
///
/// WHY 新增本组：`nexus-core` 的 `overwindow_dense_knee` 只量隔离的 dense 内核，
/// 而上方 `provider_select_*` 只量**当前生产档位下**的绝对延迟；两者之间恰好缺
/// “同语料、同查询、只差派发路径”的端到端对照——没有它，“段测 2–5×”能不能外推到
/// 全链路就只是主张，P-2 因此不能闭合（v3.5 W6-1b M6）。
///
/// 做法：在全局计算桥上把 `TaskKind::ClvSimilarity` 阈值在
/// `usize::MAX`（`route()` 恒判 Inline，即挂桥前的形状）、`1000`（生产档）与 `256`
/// 之间切换；chunk 固定为生产值 64，其余变量逐字相同 ⇒ 配对可比。
///
/// 诚实边界：本组只改本进程的全局表，测完立刻恢复；恢复时写回生产默认值
/// （`threshold=1000, chunk=64, OfflineMeasured{measured_at: 0}`，与 `dispatch.rs` 一致），
/// 不恢复会静默改掉同进程后续基准的派发条件。
fn bench_e2e_dense_ab(c: &mut Criterion) {
    use nexus_core::compute::{bridge as compute_bridge, TaskKind, ThresholdSource};

    /// 生产档阈值（`dispatch.rs` 的 `ClvSimilarity`）与分段大小。
    const PROD_THRESHOLD: usize = 1_000;
    const PROD_CHUNK: usize = 64;

    let mut group = c.benchmark_group("overwindow_e2e_dense_ab");
    let bus = event_bus::EventBus::new();
    let bridge = OverWindowBridge::new(bus).unwrap();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let hts = compute_bridge();

    let set_threshold = |threshold: usize, source: ThresholdSource| {
        hts.update_threshold(TaskKind::ClvSimilarity, threshold, PROD_CHUNK, source);
    };

    // 三套语料规模。★ 不在此假设块数：kvbsr 聚类后的实际块数由每臂开头的 `block_count()`
    // 自报，它才是 `dense_plan` 的真实输入（实测：180k→1100 块、600k→3684 块，
    // 均与旧注释里的估算值不相等）。112k 那一档专门落在 P-2 关心的
    // [513,1000) 开区：当前阈值 1000 下它不应走池，而下调到 256 后应走池——
    // 这正是“下调阈值到底赚多少”的唯一直接读数。
    // 第三臂 `pooled_if_thr_256` 回答“若阈值降到 256，本规模能拿多少”；
    // 第四臂 `inline_recheck_last` 是同路径同质控臂，给出本组结果的精度下限
    //（它两次的差就是“同路径不同测量时刻”的固有漂幅，不得把它当成收益）。
    for (size_label, tokens) in [
        ("corpus_112k", 112_000usize),
        ("corpus_180k", 180_000),
        ("corpus_600k", 600_000),
    ] {
        bridge.set_corpus(&make_corpus(tokens));
        // 与 provider_select_2344 同一比例（600k 语料 → declared 700k），保证确实走超窗路径。
        let declared = (tokens as u64) * 7 / 6;
        let arms: [(&str, usize); 4] = [
            ("inline", usize::MAX),               // 挂桥前的形状（先测）
            ("pooled_prod_1000", PROD_THRESHOLD), // 当前生产档
            ("pooled_if_thr_256", 256),           // P-2 候选下调值
            // ★ 同质控臂：与第一臂完全同路径（thr=MAX ⇒ Inline），但放在最后测。
            //   若它明显快于第一臂，则前几臂的差是“同一 bridge 上惰性状态逐步暖起来”的
            //   **位置效应**，而不是阈值/路径效应；只有两臂相当，才能把差归给阈值。
            //   没有这一臂，本组会把“测量顺序”当成“性能收益”，而后者正是要拿去改阈值的。
            ("inline_recheck_last", usize::MAX),
        ];
        for (mode, threshold) in arms {
            set_threshold(
                threshold,
                ThresholdSource::ConservativeDefault, // 本组是人为构造的对照条件，不宣称测定证据
            );
            // ★ 自报路由真值，且用的是**聚类后的实际块数**而非估算值：
            //   `dense_plan` 的输入就是 `table.len()`，而 kvbsr 聚类后的块数 ≠ chunk 数；
            //   拿估算值去猜路径会把“已越阈”读成“未越阈”（本组初版就因此得出过错误结论）。
            let actual_blocks = bridge.block_count();
            eprintln!(
                "[e2e-ab] mode={mode} thr={threshold} actual_blocks={actual_blocks} route(actual)={:?}",
                compute_bridge().route(TaskKind::ClvSimilarity, actual_blocks)
            );
            // 不计量的预热调用：使每个臂从同等“已温”状态起测，排除
            // “只有第一个臂承担新建语料表页缺页/分配器增长”的次序偏置。
            rt.block_on(bridge.run("语义检索", declared, tokens as u64))
                .ok();
            group.bench_function(BenchmarkId::new(size_label, mode), |b| {
                b.iter(|| {
                    black_box(rt.block_on(bridge.run(
                        black_box("语义检索"),
                        declared,
                        tokens as u64,
                    )))
                })
            });
        }
    }

    // 恢复生产档（包括它的真实源标签：预填占位、未复测）。
    set_threshold(
        PROD_THRESHOLD,
        ThresholdSource::OfflineMeasured { measured_at: 0 },
    );
    group.finish();
}

criterion_group!(
    benches,
    bench_set_corpus_600k,
    bench_provider_select_2344,
    bench_provider_select_24k,
    bench_e2e_dense_ab,
);
criterion_main!(benches);
