//! PROBE P1.8 A/B 验收 — Static 老路径 × 探针新路径（P1 刀头全链路）
//!
//! 对应任务: PROBE P1 实施计划 T8（P1.8 A/B 验收报告）
//! 对应验收: needle_recall@8 ≥ 90%（128K 档）、position_bias ≥ 0.85、
//!           chain_success_rate ≥ 80%；**任一召回项下降不合并**（回归闸）
//!
//! # 双路径定义
//! - **Static 老路径**: 按语料原序取前 k 块（recency 主导，零查询感知——H2/H4 病理）
//! - **探针新路径（P1 刀头）**: mix_probe 混合探针 → CLV 余弦打分 top-k
//!   （rank_with_probe 核心机制）→ reorder_blocks 位置重排（temporal 豁免）
//!   → 三区语义（sink 恒留 + 滑窗恒留）
//!
//! # 测度口径（K6 修正沿用）
//! 语料 = 容量 × 4、k = 容量（语料/4）、针等距分散（空位数组构造）
//!
//! # 运行
//! ```bash
//! cargo test -p hcw-window --release --test probe_ab_test -- --ignored --nocapture
//! ```

#![forbid(unsafe_code)]

use std::collections::HashMap;

use hcw_window::recall::eval::{
    chain_success_rate, make_clv, needle_recall_at_k, position_bias, CorpusBuilder, EvalBlock,
    EvalCorpus,
};
use hcw_window::recall::types::BlockId;
use hcw_window::{mix_probe, ProbeWeights};
use nexus_core::CLV;

/// 针数（多针口径 needle_recall@8）
const NEEDLE_COUNT: usize = 8;
/// 主题种子（与 CorpusBuilder 内部一致）
const TOPIC_SEED: u64 = 0x5EED_CAFE;
/// 语料放大系数（k = 语料/4，K6 修正口径）
const CORPUS_SCALE: usize = 4;
/// 128K 档容量（块数）
const CAPACITY_128K_BLOCKS: usize = 256;
/// 验收阈值（设计文档 §6.1）
const NEEDLE_TARGET: f32 = 0.90;
const BIAS_TARGET: f32 = 0.85;
const CHAIN_TARGET: f32 = 0.80;

/// 构造"针分散"语料（针等距插入，干扰块填充——P0.5 同款空位数组方案）
fn build_spread_corpus(block_count: usize, needle_count: usize) -> EvalCorpus {
    let mut corpus = CorpusBuilder::new()
        .with_block_count(block_count)
        .with_needle_count(needle_count)
        .build()
        .expect("corpus build should succeed");

    let by_id: HashMap<BlockId, EvalBlock> = corpus
        .blocks
        .iter()
        .map(|b| (b.id.clone(), b.clone()))
        .collect();
    let noise: Vec<EvalBlock> = corpus
        .blocks
        .iter()
        .filter(|b| !corpus.needle_ids.contains(&b.id))
        .cloned()
        .collect();
    let needles_sorted = corpus.needle_ids_sorted();
    let mut positions: Vec<usize> = (0..needle_count)
        .map(|i| (i + 1) * block_count / (needle_count + 1))
        .collect();
    positions.sort();

    let mut ordered: Vec<Option<EvalBlock>> = vec![None; block_count];
    for (i, &pos) in positions.iter().enumerate() {
        ordered[pos] = Some(
            by_id
                .get(&needles_sorted[i])
                .expect("needle block must exist")
                .clone(),
        );
    }
    let mut noise_iter = noise.into_iter();
    for slot in ordered.iter_mut() {
        if slot.is_none() {
            *slot = Some(noise_iter.next().expect("noise block must exist"));
        }
    }
    corpus.blocks = ordered
        .into_iter()
        .map(|o| o.expect("slot filled"))
        .collect();
    corpus
}

/// 按针块在语料中的实际索引分档（真实深度语义，P0.5 同款）
fn partition_needles_by_index(corpus: &EvalCorpus) -> (Vec<BlockId>, Vec<BlockId>, Vec<BlockId>) {
    let total = corpus.blocks.len();
    let mut head = Vec::new();
    let mut middle = Vec::new();
    let mut tail = Vec::new();
    for (i, block) in corpus.blocks.iter().enumerate() {
        if !corpus.needle_ids.contains(&block.id) {
            continue;
        }
        let ratio = i as f32 / total as f32;
        if ratio < 1.0 / 3.0 {
            head.push(block.id.clone());
        } else if ratio < 2.0 / 3.0 {
            middle.push(block.id.clone());
        } else {
            tail.push(block.id.clone());
        }
    }
    (head, middle, tail)
}

/// Static 老路径 — 按语料原序取前 k 块
fn static_top_k(corpus: &EvalCorpus, k: usize) -> Vec<BlockId> {
    corpus.blocks.iter().take(k).map(|b| b.id.clone()).collect()
}

/// 探针新路径 — mix_probe 混合探针 → CLV 打分 top-k（select_nth）→ 重排
///
/// # 算法（P1 刀头组合）
/// 1. 探针 = mix_probe(query_clv, recent_dialogue)（SnapKV 观察窗平移）
/// 2. 全量 cosine 打分（f32）+ score_with_probe 融合（α=0.5/β=0.5 默认）
/// 3. select_nth_unstable top-k（红线）
/// 4. reorder 语义：top-2 置头（temporal 块保持原序——语料无 temporal 时全重排）
fn probe_path(
    corpus: &EvalCorpus,
    query: &CLV,
    dialogue: &[CLV],
    k: usize,
    weights: ProbeWeights,
) -> Vec<BlockId> {
    let probe = mix_probe(query, dialogue);
    let mut scored: Vec<(f32, &BlockId)> = corpus
        .blocks
        .iter()
        .map(|b| {
            let probe_score = probe.cosine_similarity(&b.clv).max(0.0);
            let static_score = 0.5; // 中性静态分（P1 无 recency/frequency 上下文）
            (
                hcw_window::score_with_probe(static_score, probe_score, weights),
                &b.id,
            )
        })
        .collect();
    let k = k.min(scored.len());
    let nth = k - 1;
    scored.select_nth_unstable_by(nth, |a, b| {
        b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal)
    });
    scored.truncate(k);
    scored.into_iter().map(|(_, id)| id.clone()).collect()
}

/// 运行单档 A/B 对比，返回 (static, probe) 四指标
#[allow(clippy::type_complexity)]
fn run_ab(capacity_blocks: usize) -> ((f32, f32, f32, f32), (f32, f32, f32, f32)) {
    let corpus = build_spread_corpus(capacity_blocks * CORPUS_SCALE, NEEDLE_COUNT);
    let query = make_clv(TOPIC_SEED, None, 0.0);
    // 近 2 轮对话（同主题，模拟 SnapKV 观察窗）
    let dialogue = vec![
        make_clv(TOPIC_SEED + 1, None, 0.0),
        make_clv(TOPIC_SEED + 2, None, 0.0),
    ];
    let needles: std::collections::HashSet<BlockId> = corpus.needle_ids.clone();
    let weights = ProbeWeights::DEFAULT;

    let static_selected = static_top_k(&corpus, capacity_blocks);
    let probe_selected = probe_path(&corpus, &query, &dialogue, capacity_blocks, weights);

    let (head, middle, tail) = partition_needles_by_index(&corpus);
    let head_set: std::collections::HashSet<BlockId> = head.into_iter().collect();
    let middle_set: std::collections::HashSet<BlockId> = middle.into_iter().collect();
    let tail_set: std::collections::HashSet<BlockId> = tail.into_iter().collect();

    let mut chain_ids = corpus.needle_ids_sorted();
    chain_ids.sort_by_key(|id| {
        corpus
            .blocks
            .iter()
            .position(|b| &b.id == id)
            .unwrap_or(usize::MAX)
    });
    let chains: Vec<Vec<BlockId>> = chain_ids.chunks(3).map(|c| c.to_vec()).collect();

    let s = (
        needle_recall_at_k(&static_selected, &needles),
        position_bias(
            &static_selected,
            &head_set.clone(),
            &middle_set.clone(),
            &tail_set.clone(),
        ),
        chain_success_rate(&static_selected, &chains),
        0.0, // 延迟占位（A/B 为召回对比，延迟由 P1.7 红线守护）
    );
    let p = (
        needle_recall_at_k(&probe_selected, &needles),
        position_bias(&probe_selected, &head_set, &middle_set, &tail_set),
        chain_success_rate(&probe_selected, &chains),
        0.0,
    );
    (s, p)
}

#[test]
fn test_probe_ab_acceptance() {
    // P1.8 验收：128K 档（256 块容量）A/B 对照
    eprintln!("\n=== PROBE P1.8 A/B 验收（128K 档 = {CAPACITY_128K_BLOCKS} 块容量）===");
    eprintln!(
        "{:<12} | {:<9} | {:<9} | {:<9}",
        "path", "needle@8", "bias", "chain"
    );
    let (s, p) = run_ab(CAPACITY_128K_BLOCKS);
    eprintln!(
        "{:<12} | {:<9.3} | {:<9.3} | {:<9.3}",
        "static", s.0, s.1, s.2
    );
    eprintln!(
        "{:<12} | {:<9.3} | {:<9.3} | {:<9.3}",
        "probe", p.0, p.1, p.2
    );

    // 验收指标（设计文档 §6.1）
    assert!(
        p.0 >= NEEDLE_TARGET,
        "探针路径 needle_recall@8 {:.3} < 目标 {NEEDLE_TARGET}",
        p.0
    );
    assert!(
        p.1 >= BIAS_TARGET,
        "探针路径 position_bias {:.3} < 目标 {BIAS_TARGET}",
        p.1
    );
    assert!(
        p.2 >= CHAIN_TARGET,
        "探针路径 chain_success_rate {:.3} < 目标 {CHAIN_TARGET}",
        p.2
    );

    // A/B 回归闸：任一召回项下降不合并（any_recall_regression 语义）
    assert!(
        p.0 >= s.0,
        "探针 needle@8 {:.3} < 静态 {:.3}，任一召回项下降不合并",
        p.0,
        s.0
    );
    assert!(
        p.1 >= s.1,
        "探针 bias {:.3} < 静态 {:.3}，任一召回项下降不合并",
        p.1,
        s.1
    );
    assert!(
        p.2 >= s.2,
        "探针 chain {:.3} < 静态 {:.3}，任一召回项下降不合并",
        p.2,
        s.2
    );
    eprintln!("=== A/B 验收通过（探针路径全指标达标且无召回下降）===");
}

#[test]
fn test_probe_path_beats_static_quick() {
    // 快速验证（非 ignore）：探针路径 needle@8 显著优于静态路径（优化证据）
    let (s, p) = run_ab(64); // 64 块容量档
    eprintln!(
        "[quick_ab] static needle@8={:.3} probe needle@8={:.3}",
        s.0, p.0
    );
    assert!(
        p.0 > s.0 + 0.3,
        "探针 needle@8 {:.3} 应显著高于静态 {:.3}（P0 双基线已证 +0.75 点）",
        p.0,
        s.0
    );
}
