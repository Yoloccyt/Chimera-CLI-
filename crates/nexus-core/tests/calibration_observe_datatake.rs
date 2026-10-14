//! WI-G6c 阶段 3 静默窗口取数驱动 — `CHIMERA_COMPUTE_CALIBRATION=observe` 采样实测
//!
//! # 定位（报告型 WI;本文件 = 取数工具,非 CI 测试）
//! 以**被显式 `#[ignore]`** 的手动驱动,在 observe 模式下驱动 overwindow 形状的
//! ClvSimilarity 批量卸载若干千次,令 ADR-198 校准器按 1/64 采样 + 奇偶探测分流
//! 攒出成对 (inline_us, pool_us) 样本,然后导出 `calibration_snapshot()` 证据行。
//!
//! # 硬约束（取数纪律）
//! - **observe 模式不触 apply**:驱动内显式断言 `flush()` 写表数 == 0、
//!   `applied == false`、全部 `TaskKind::ALL` 阈值前后逐项相等（HtsTable 无默认值变更）;
//! - 非静默窗口读数一律 `[Directional]`（R5）:本驱动不做任何结论断言
//!   （`Decided` 仅回显,不断言 Promote/Reject）;
//! - 运行方式（不进 CI）:
//!   `$env:CHIMERA_COMPUTE_CALIBRATION='observe'; cargo test -p nexus-core --test calibration_observe_datatake -- --ignored --nocapture`

#![allow(clippy::unwrap_used, clippy::expect_used)] // test/bench code idiom; E-5 targets production code

use std::sync::Arc;

use nexus_core::compute::calibration::{flush, mode, CalibrationMode};
use nexus_core::compute::{bridge, calibration_snapshot, TaskKind};
use nexus_core::CLV;

const K: usize = 128; // DEFAULT_TOP_K（chimera-cli/overwindow_bridge.rs）
const CHUNK: usize = 64; // 生产点位 CHUNK 对齐
const N_BLOCKS: usize = 2048; // overwindow 生产带 [513,3906] 内取点
const CALLS: usize = 5_000; // ≥ 2×(30 min_samples)×64 采样周期（保证两桶各 ≥30）

/// 确定性块 CLV（SplitMix64 派生;与 WI-G6a bench 同源）
fn make_clv(seed: u64) -> CLV {
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
    CLV::from_vec(v).expect("512 维构造")
}

/// top-k 部分选择 + 排序（dense 段同款比较器）
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

/// 一次被观测驱动的卸载调用（CHUNK=64 分段 → spawn_compute_batch → 拼接 + top-k）
#[allow(clippy::type_complexity)] // 卸载闭包包型与生产点位同形（hcw/faae 同范式）
fn drive_one(b: &nexus_core::compute::ComputeBridge, blocks: &Arc<Vec<CLV>>, query: &CLV) {
    let n_chunks = blocks.len().div_ceil(CHUNK);
    let tasks: Vec<Box<dyn FnOnce() -> Vec<(usize, f32)> + Send>> = (0..n_chunks)
        .map(|ci| {
            let all = Arc::clone(blocks);
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
    let mut scores: Vec<(usize, f32)> = Vec::with_capacity(blocks.len());
    for r in results {
        scores.extend(r.unwrap());
    }
    topk(&mut scores, K);
    std::hint::black_box(scores.len());
}

#[test]
#[ignore = "manual stage-3 data-taking; run with CHIMERA_COMPUTE_CALIBRATION=observe --ignored --nocapture"]
fn calibration_observe_datatake() {
    // ---- 前置:必须处于 observe 模式（误跑即快速失败,不产出脏数据）----
    assert_eq!(
        mode(),
        CalibrationMode::Observe,
        "必须设置 CHIMERA_COMPUTE_CALIBRATION=observe 后运行（本驱动不产出 off/apply 数据）"
    );

    let b = bridge();
    // ---- 阈值基线（前后逐项相等 = HtsTable 无默认值变更）----
    let before: Vec<(TaskKind, usize)> = TaskKind::ALL
        .iter()
        .map(|k| (*k, b.threshold_of(*k)))
        .collect();

    // ---- 驱动：5000 次卸载调用（1/64 采样 ⇒ 各桶 ~39 样本 ≥ min_samples=30）----
    let query = make_clv(0x00C0_FFEE);
    let blocks: Arc<Vec<CLV>> = Arc::new((0..N_BLOCKS).map(|i| make_clv(i as u64 + 1)).collect());
    let t0 = std::time::Instant::now();
    for _ in 0..CALLS {
        drive_one(b, &blocks, &query);
    }
    let drive_elapsed = t0.elapsed();

    // ---- observe 模式不触 apply：flush 写表数必须为 0 ----
    let written = flush(b);
    assert_eq!(written, 0, "observe 模式 flush 不得写表");

    // ---- 证据导出（供报告抓取;非静默窗口读数一律 [Directional]）----
    println!("== WI-G6c calibration observe datatake ==");
    println!(
        "mode={:?} calls={CALLS} n_blocks={N_BLOCKS} chunk={CHUNK} drive_elapsed={:?}",
        mode(),
        drive_elapsed
    );
    for stat in calibration_snapshot() {
        if stat.inline_samples == 0 && stat.pool_samples == 0 {
            continue;
        }
        println!(
            "DATATAKE kind={:?} inline_samples={} pool_samples={} decided={:?} candidate_threshold={} applied={}",
            stat.kind, stat.inline_samples, stat.pool_samples, stat.decided, stat.candidate_threshold, stat.applied
        );
        assert!(
            !stat.applied,
            "observe 模式不得出现 applied=true（{kind:?}）",
            kind = stat.kind
        );
        assert!(
            stat.inline_samples >= 30 && stat.pool_samples >= 30,
            "两桶样本量应 ≥ min_samples=30（取数充足性:{:?} inline={} pool={}）",
            stat.kind,
            stat.inline_samples,
            stat.pool_samples
        );
    }

    // ---- HtsTable 无默认值变更（逐项对拍）----
    for (kind, t_before) in &before {
        let t_after = b.threshold_of(*kind);
        assert_eq!(
            *t_before, t_after,
            "observe 模式不得改表（{kind:?}: {t_before} → {t_after}）"
        );
        println!("THRESHOLD-UNCHANGED kind={kind:?} value={t_after}");
    }

    // 回显决定（不成就结论——非静默窗口,方向性数据）
    let snap = calibration_snapshot();
    let clv = snap
        .iter()
        .find(|s| s.kind == TaskKind::ClvSimilarity)
        .expect("ClvSimilarity 快照存在");
    println!(
        "[Directional] clv_similarity decided={:?} candidate={} current={} (decided 仅回显,不断言)",
        clv.decided,
        clv.candidate_threshold,
        b.threshold_of(TaskKind::ClvSimilarity)
    );
}
