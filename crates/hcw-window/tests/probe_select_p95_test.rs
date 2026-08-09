//! PROBE P1.7 双性能红线 — 探针装窗 p95<10ms + 打分吞吐 ≥10K 块/秒
//!
//! 对应任务: PROBE P1 实施计划 T7（P1.7 双性能红线）
//! 对应预算: 设计文档 §4.2.4（延迟预算 10ms，吞吐 10K 块/秒）
//!
//! # 红线（两红线并存，对象不同）
//! - **新红线**: 探针装窗 p95 < 10ms（快照 + 打分 + Top-K + 三区组装全链路）
//! - **新红线**: 打分吞吐 ≥ 10K 块/秒（CLV SIMD cosine 28ns@512d → 理论 ~35M/秒）
//! - **旧红线**: window_select <1ms 不放宽（tests/selector.rs 既有守护，O(1) 档位决策）
//!
//! # 范式（复刻 coarse_recall_p95_test.rs）
//! - `#[ignore]` + `--release` 显式运行（debug 模式开销污染 p95 测量，R8 前科）
//! - warmup 10 次（fine_recall_p95_test.rs L191-201 先例）
//! - 阈值常量 + eprintln 输出分布
//!
//! # 运行
//! ```bash
//! cargo test -p hcw-window --release --test probe_select_p95_test -- --ignored --nocapture
//! ```

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::time::Instant;

use hcw_window::recall::types::BlockId;
use hcw_window::{probe_health, ProbeHealth, ScoreCache};
use nexus_core::CLV;

/// 探针装窗 p95 红线（毫秒，设计文档 §4.2.4 延迟预算，默认 10ms）
///
/// # WHY 参数化（P1-5）
/// 硬编码时序阈值在 CI 慢机/高负载下余量不足会偶发失败（项目 memory 实证：
/// debug 开销 + 并行竞争污染延迟测量）。`HCW_PROBE_WINDOW_P95_MS` 使 CI
/// 可在不改代码的情况下覆盖阈值；失败安全：未设置/解析失败回退 10ms。
fn probe_window_p95_ms() -> u128 {
    std::env::var("HCW_PROBE_WINDOW_P95_MS")
        .ok()
        .and_then(|s| s.parse::<u128>().ok())
        .unwrap_or(10)
}

/// 打分吞吐红线（块/秒，设计文档 §6.1，默认 10_000）
///
/// # WHY 参数化（P1-5）
/// 吞吐为时序派生断言（块数/总耗时），同 p95 属负载敏感类，CI 慢机下
/// 余量不足会偶发失败；`HCW_PROBE_THROUGHPUT_MIN` 使 CI 可覆盖。
/// 失败安全：未设置/解析失败回退 10_000 块/秒。
fn probe_throughput_min() -> f64 {
    std::env::var("HCW_PROBE_THROUGHPUT_MIN")
        .ok()
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(10_000.0)
}

/// 窗口档块数（128K 档 = 256 块 × 512 token）
const BLOCK_COUNT: usize = 256;

/// warmup 次数（对齐 fine_recall_p95_test 范式）
const WARMUP_ITERS: usize = 10;

/// 测量样本数
const SAMPLE_COUNT: usize = 100;

/// 构造确定性 CLV（SplitMix64 强混合，与 recall/eval 同模式）
fn make_clv(seed: u64) -> CLV {
    let v: Vec<f32> = (0..512)
        .map(|j| {
            let mut z = seed.wrapping_add((j as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            ((z >> 11) as f32) / (1u64 << 53) as f32 * 2.0 - 1.0
        })
        .collect();
    CLV::from_vec(v).expect("512 dims")
}

/// 构造评测语料（256 块 + 主题探针；块 CLV 与探针同主题 60% 混合模拟真实判别）
fn build_corpus() -> (CLV, Vec<(BlockId, CLV)>) {
    let topic = make_clv(0x5EED_CAFE);
    let blocks: Vec<(BlockId, CLV)> = (0..BLOCK_COUNT)
        .map(|i| {
            let noise = make_clv(1000 + i as u64);
            let t = topic.as_slice();
            let n = noise.as_slice();
            let v: Vec<f32> = (0..512).map(|j| 0.6 * t[j] + 0.4 * n[j]).collect();
            (format!("b{i:03}"), CLV::from_vec(v).expect("512 dims"))
        })
        .collect();
    (topic, blocks)
}

/// 探针装窗全链路（快照 → 健康检测 → 全量打分 → Top-K → 组装）
///
/// 模拟 P1.6 快照通路 + repr_clv 缓存消费 + select_nth Top-K + 三区组装：
/// 返回耗时与选中块数（block 数即吞吐计量的"块"）
fn probe_window_pass(
    probe: &CLV,
    blocks: &[(BlockId, CLV)],
    cache: &mut ScoreCache,
    version: u64,
    top_k: usize,
) -> (u128, usize) {
    let start = Instant::now();

    // 1. 探针健康检测（单趟，NaN/零向量率）
    let clvs: Vec<CLV> = blocks.iter().map(|(_, c)| c.clone()).collect();
    assert_ne!(probe_health(probe, &clvs), ProbeHealth::NotFinite);

    // 2. 增量重打分：双因子命中复用，未命中全量重算
    let hash = ScoreCache::probe_fingerprint(probe);
    let scores: HashMap<BlockId, f32> = match cache.try_hit(hash, version) {
        Some(cached) => cached.clone(),
        None => {
            let s: HashMap<BlockId, f32> = blocks
                .iter()
                .map(|(id, clv)| (id.clone(), probe.cosine_similarity(clv).max(0.0)))
                .collect();
            cache.put(hash, version, s.clone());
            s
        }
    };

    // 3. Top-K（select_nth_unstable_by，红线）——按分数降序取 top_k
    let mut scored: Vec<(&BlockId, f32)> = scores.iter().map(|(id, s)| (id, *s)).collect();
    let k = top_k.min(scored.len());
    let nth = k - 1;
    scored.select_nth_unstable_by(nth, |a, b| {
        b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(k);

    // 4. 组装（模拟三区顺序拼接——选中块 ID 列表）
    let selected: Vec<&BlockId> = scored.iter().map(|(id, _)| *id).collect();

    (start.elapsed().as_micros(), selected.len())
}

#[test]
#[ignore]
fn test_probe_window_p95_below_10ms_and_throughput() {
    // P1.7 双红线：装窗 p95<10ms + 吞吐 ≥10K 块/秒（release 模式运行）
    let (probe, blocks) = build_corpus();
    let mut cache = ScoreCache::new();

    // warmup（对齐 fine_recall_p95_test 范式）
    for i in 0..WARMUP_ITERS {
        let _ = probe_window_pass(&probe, &blocks, &mut cache, 1, 128);
        let _ = i;
    }

    // 采样（版本递增模拟块集演化，首轮重算、后续命中缓存）
    let mut latencies: Vec<u128> = Vec::with_capacity(SAMPLE_COUNT);
    let mut total_blocks_scored: usize = 0;
    let start_total = Instant::now();
    for i in 0..SAMPLE_COUNT {
        let (us, selected) = probe_window_pass(&probe, &blocks, &mut cache, i as u64, 128);
        latencies.push(us);
        total_blocks_scored += blocks.len();
        let _ = selected;
    }
    let total_elapsed = start_total.elapsed();

    // p95 计算（全量重算轮次才代表真实打分延迟——首轮已 warmup 缓存）
    latencies.sort_unstable();
    let p95 = latencies[(SAMPLE_COUNT as f64 * 0.95) as usize].min(latencies[SAMPLE_COUNT - 1]);

    // 吞吐：块数 / 总耗时（含缓存命中复用——"查询期零计算"的收益）
    let throughput = total_blocks_scored as f64 / total_elapsed.as_secs_f64();

    // P1-5: 阈值参数化（默认 10ms / 10K 块每秒，CI 可覆盖）
    let p95_ms = probe_window_p95_ms();
    let throughput_min = probe_throughput_min();

    eprintln!(
        "[probe_select_p95] samples={} mean={}us p95={}us p95<{}ms={} \
         throughput={:.0} blocks/s >= {:.0}={}",
        SAMPLE_COUNT,
        latencies.iter().sum::<u128>() / SAMPLE_COUNT as u128,
        p95,
        p95_ms,
        p95 < p95_ms * 1000,
        throughput,
        throughput_min,
        throughput >= throughput_min
    );

    assert!(
        p95 < p95_ms * 1000,
        "探针装窗 p95 {p95}us 超红线 {}ms",
        p95_ms
    );
    assert!(
        throughput >= throughput_min,
        "打分吞吐 {throughput:.0} 块/秒低于红线 {throughput_min}"
    );
}

#[test]
fn test_probe_health_ok_on_corpus() {
    // 常规测试（非 ignore）：语料探针健康（判别性前提）
    let (probe, blocks) = build_corpus();
    let clvs: Vec<CLV> = blocks.iter().map(|(_, c)| c.clone()).collect();
    assert_eq!(probe_health(&probe, &clvs), ProbeHealth::Healthy);
}

#[test]
fn test_score_cache_skip_recompute() {
    // 双因子命中时跳过重算（增量重打分语义验证，非性能断言）
    let (probe, blocks) = build_corpus();
    let mut cache = ScoreCache::new();
    let hash = ScoreCache::probe_fingerprint(&probe);
    // 首轮重算并缓存
    let _ = probe_window_pass(&probe, &blocks, &mut cache, 1, 128);
    assert!(cache.try_hit(hash, 1).is_some(), "首轮后应命中缓存");
    // 版本变化 → miss（块集演化强制重算）
    assert!(cache.try_hit(hash, 2).is_none(), "版本变化应失效");
}
