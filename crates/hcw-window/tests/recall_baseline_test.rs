//! PROBE P0.5 双基线对照表 — Static 老路径 vs CLV 探针路径（四档窗口）
//!
//! 对应任务: PROBE 实施计划 §2.2 P0.5（双基线对照表产出并冻结）
//! 对应验收: "基线对照表产出且冻结；无基线对照表不进入 P1"（度量先行）
//!
//! # 双路径定义
//! - **Static 老路径**: 按语料原序取前 k 块（近似 recency 主导的静态启发式，
//!   零查询感知、零位置修正——对应病理 H2/H4）
//! - **CLV 探针路径**: query CLV 对全部候选块余弦打分，`select_nth_unstable`
//!   取 top-k（等价 recall/fine.rs 精确重排的核心机制，无需 VectorStore）
//!
//! # 针分布
//! 针块等距分散于语料（25%/50%/75% 深度附近），模拟真实"关键信息不在头部"场景：
//! 静态路径预期只能命中头部针（位置偏置病理 H4 显现），CLV 路径应全中。
//!
//! # 指标
//! recall@tier / needle_recall@8 / position_bias / chain_success_rate（eval 四指标）
//!
//! # 运行
//! ```bash
//! cargo test -p hcw-window --release --test recall_baseline_test -- --ignored --nocapture
//! ```
//! 输出对照表行（RecallReport Display 格式），数字固定种子可复现。
//!
//! # 断言
//! 1. CLV 路径 needle_recall@8 ≥ Static 路径（探针优势可证——P1 接线的正当性证据）
//! 2. Static 路径 position_bias 显著 < 1.0（位置偏置病理量化）
//! 3. 全 f32 运算（红线），`select_nth_unstable_by` Top-K（红线）

#![forbid(unsafe_code)]

use std::collections::HashMap;

use hcw_window::recall::eval::{
    chain_success_rate, make_clv, needle_recall_at_k, position_bias, CorpusBuilder, EvalBlock,
    EvalCorpus,
};
use hcw_window::recall::types::BlockId;
use nexus_core::CLV;

/// 四档窗口的块容量（512 token/块：4K=8 / 32K=64 / 128K=256 / 1M 等效加载=512）
const TIER_BLOCKS: [usize; 4] = [8, 64, 256, 512];
/// 窗口档名（对照表行标识）
const TIER_NAMES: [&str; 4] = ["L0-4K", "L1-32K", "L2-128K", "L3-1M"];
/// 针数（多针评测口径 needle_recall@8）
const NEEDLE_COUNT: usize = 8;
/// 主题种子（与 CorpusBuilder 内部主题种子一致，保证 query 与针块同主题）
const TOPIC_SEED: u64 = 0x5EED_CAFE;
/// 语料放大系数（语料 = 窗口容量 × 4，保证选择有区分度）
///
/// WHY 4×: 窗口容量 k = 语料/4，静态"取前 k 块"路径只覆盖语料前 25%，
/// 针等距分散后仅头部针被命中（中段/尾部全丢）——位置偏置病理（H4）
/// 可量化（bias → 0）；若 k = 语料/2，中段针恰好半覆盖使 bias 恒 = 1.0
/// （数学巧合，测度失效）。
const CORPUS_SCALE: usize = 4;

/// 构造"针分散"语料 — 针块等距插入语料，干扰块填充其余位置
///
/// WHY 分散针: 静态"取前 k 块"路径只能命中头部针，量化位置偏置病理（H4）；
/// 若针全在头部，静态路径也能命中，无法证明探针优化价值。
///
/// # 参数
/// - `block_count`: 总块数（≥ needle_count）
/// - `needle_count`: 针数
///
/// # 返回值
/// 重排后的语料（块序 = 分散针 + 干扰块，needle_ids 不变）
fn build_spread_corpus(block_count: usize, needle_count: usize) -> EvalCorpus {
    let mut corpus = CorpusBuilder::new()
        .with_block_count(block_count)
        .with_needle_count(needle_count)
        .build()
        .expect("corpus build should succeed");

    // 分离针块与干扰块
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

    // 等距插入位置：(i+1) * block_count / (needle_count+1)
    let mut positions: Vec<usize> = (0..needle_count)
        .map(|i| (i + 1) * block_count / (needle_count + 1))
        .collect();
    positions.sort();

    // 空位数组构造（WHY 不用 Vec::insert：insert 位置基于当前数组长度，
    // 针数接近块数时（如 8 针/32 块）插入索引可越界 panic；
    // 空位数组按总块数定位，任何 pos < block_count 都安全）
    let mut ordered: Vec<Option<EvalBlock>> = vec![None; block_count];
    for (i, &pos) in positions.iter().enumerate() {
        ordered[pos] = Some(
            by_id
                .get(&needles_sorted[i])
                .expect("needle block must exist")
                .clone(),
        );
    }
    // 干扰块填充空位（保持相对顺序）
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

/// Static 老路径 — 按语料原序取前 k 块
///
/// 近似"recency 主导 + 零查询感知 + 零位置修正"的静态启发式行为（H2/H4 病理）。
fn static_top_k(corpus: &EvalCorpus, k: usize) -> Vec<BlockId> {
    corpus.blocks.iter().take(k).map(|b| b.id.clone()).collect()
}

/// CLV 探针路径 — query CLV 对全部候选块余弦打分，select_nth_unstable 取 top-k
///
/// 等价 recall/fine.rs 精确重排核心机制（无 VectorStore 的全量版本，
/// 语料 ≤ 1024 块场景直接打分，复杂度 O(n) 打分 + O(n) 选择）。
fn clv_top_k(corpus: &EvalCorpus, query: &CLV, k: usize) -> Vec<BlockId> {
    // 1. 全量打分（f32 全程）
    let scores: Vec<f32> = corpus
        .blocks
        .iter()
        .map(|b| query.cosine_similarity(&b.clv))
        .collect();
    let ids: Vec<BlockId> = corpus.blocks.iter().map(|b| b.id.clone()).collect();
    // 2. O(n) Top-K（红线：select_nth_unstable 替代 sort_by）
    //    对索引数组选择，比较闭包只读 scores——避免 select_nth_unstable_by
    //    的可变借用与比较闭包不可变借用冲突（E0502）
    let mut idx: Vec<usize> = (0..scores.len()).collect();
    let nth = k.min(idx.len()) - 1; // 先算 nth 再调用，避免 idx.len() 借用冲突
    idx.select_nth_unstable_by(nth, |a, b| {
        scores[*b]
            .partial_cmp(&scores[*a])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    idx.truncate(k);
    idx.into_iter().map(|i| ids[i].clone()).collect()
}

// 按针块在语料中的实际索引分档（真实深度语义）
///
/// # 参数
/// - `corpus`: 针分散语料
///
/// # 返回值
/// `(head, middle, tail)` 三组针 ID：索引 < 1/3 为头、1/3~2/3 为中、> 2/3 为尾
///
/// WHY 按块索引而非 ID 顺序分档: 位置偏置量化的是"针埋在语料的哪个深度被选中"，
/// 必须用针在 `corpus.blocks` 中的真实位置（build_spread_corpus 已等距分散）。
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

/// 运行单档窗口的双路径评测
///
/// # 参数
/// - `tier_name`: 窗口档名
/// - `capacity_blocks`: 窗口块容量 k
///
/// # 返回值
/// `(static_report_line, pipeline_report_line)`（RecallReport Display 文本）
fn run_tier(tier_name: &str, capacity_blocks: usize) -> (String, String) {
    let corpus = build_spread_corpus(capacity_blocks * CORPUS_SCALE, NEEDLE_COUNT);
    let query = make_clv(TOPIC_SEED, None, 0.0);
    let needles: std::collections::HashSet<BlockId> = corpus.needle_ids.clone();

    // 双路径选中集
    let static_selected = static_top_k(&corpus, capacity_blocks);
    let pipeline_selected = clv_top_k(&corpus, &query, capacity_blocks);

    // 四指标（eval 共享函数）
    let static_needle = needle_recall_at_k(&static_selected, &needles);
    let pipe_needle = needle_recall_at_k(&pipeline_selected, &needles);
    // 位置分档：按针块在语料中的实际索引（真实深度语义）
    let (head, middle, tail) = partition_needles_by_index(&corpus);
    let head_set: std::collections::HashSet<BlockId> = head.into_iter().collect();
    let middle_set: std::collections::HashSet<BlockId> = middle.into_iter().collect();
    let tail_set: std::collections::HashSet<BlockId> = tail.into_iter().collect();
    let static_bias = position_bias(&static_selected, &head_set, &middle_set, &tail_set);
    let pipe_bias = position_bias(&pipeline_selected, &head_set, &middle_set, &tail_set);
    // 多跳链：按语料中针出现顺序两两成链（3 链 × 2-3 针，模拟 A→B→C）
    let mut chain_ids = corpus.needle_ids_sorted();
    chain_ids.sort_by_key(|id| {
        corpus
            .blocks
            .iter()
            .position(|b| &b.id == id)
            .unwrap_or(usize::MAX)
    });
    let chains: Vec<Vec<BlockId>> = chain_ids.chunks(3).map(|c| c.to_vec()).collect();
    let static_chain = chain_success_rate(&static_selected, &chains);
    let pipe_chain = chain_success_rate(&pipeline_selected, &chains);

    let static_line = format!(
        "{tier_name}-static | recall@tier={:.3} needle@8={:.3} bias={:.3} chain={:.3} selected={}",
        f32::from(static_selected.iter().any(|id| needles.contains(id))),
        static_needle,
        static_bias,
        static_chain,
        static_selected.len()
    );
    let pipeline_line = format!(
        "{tier_name}-probe  | recall@tier={:.3} needle@8={:.3} bias={:.3} chain={:.3} selected={}",
        f32::from(pipeline_selected.iter().any(|id| needles.contains(id))),
        pipe_needle,
        pipe_bias,
        pipe_chain,
        pipeline_selected.len()
    );
    (static_line, pipeline_line)
}

#[test]
#[ignore]
fn test_baseline_comparison_table() {
    // P0.5 验收：产出可复现的双基线对照表（固定种子）
    eprintln!("\n=== PROBE P0.5 双基线对照表（Static vs CLV 探针，四档窗口）===");
    eprintln!(
        "{:<10} | {:<10} | {:<10} | {:<10} | {:<10}",
        "path", "recall@tier", "needle@8", "bias", "chain"
    );
    let mut all_pass = true;
    for (i, &cap) in TIER_BLOCKS.iter().enumerate() {
        let (s_line, p_line) = run_tier(TIER_NAMES[i], cap);
        eprintln!("{s_line}");
        eprintln!("{p_line}");
        // 断言：探针路径多针召回 ≥ 静态路径（探针优势可证）
        let s_needle = s_line
            .split("needle@8=")
            .nth(1)
            .and_then(|s| s.split(' ').next())
            .and_then(|s| s.parse::<f32>().ok());
        let p_needle = p_line
            .split("needle@8=")
            .nth(1)
            .and_then(|s| s.split(' ').next())
            .and_then(|s| s.parse::<f32>().ok());
        if let (Some(sn), Some(pn)) = (s_needle, p_needle) {
            if pn < sn {
                all_pass = false;
                eprintln!(
                    "[WARN] {}-probe needle@8 {pn:.3} < static {sn:.3}",
                    TIER_NAMES[i]
                );
            }
        }
    }
    eprintln!("=== 对照表结束（数字固定种子可复现，作为 P1 A/B 对照组冻结）===");
    assert!(
        all_pass,
        "探针路径任一档 needle@8 低于静态路径，探针优化证据不足"
    );
}

#[test]
fn test_static_path_position_bias_pathology() {
    // 病理量化（非 ignore，快速验证）：静态路径 position_bias 显著 < 1.0
    // 针分散场景下"取前 k 块"（k = 语料/4）只能命中头部针 → 中段/尾部全丢 → bias ≈ 0
    let capacity = 64; // 模拟 L2-128K 档
    let corpus = build_spread_corpus(capacity * CORPUS_SCALE, NEEDLE_COUNT);
    let static_selected = static_top_k(&corpus, capacity);
    let (head, middle, tail) = partition_needles_by_index(&corpus);
    let head_set: std::collections::HashSet<BlockId> = head.into_iter().collect();
    let middle_set: std::collections::HashSet<BlockId> = middle.into_iter().collect();
    let tail_set: std::collections::HashSet<BlockId> = tail.into_iter().collect();
    let bias = position_bias(&static_selected, &head_set, &middle_set, &tail_set);
    eprintln!("[static_path_bias] position_bias={bias:.3}（预期 < 0.6，量化 lost-in-the-middle）");
    assert!(
        bias < 0.6,
        "静态路径位置偏置应显著（bias={bias:.3}），否则病理不成立"
    );
}

#[test]
fn test_clv_path_recovers_middle_needles() {
    // 探针路径应命中分散针（同主题 query → 针块高分）：needle@8 ≥ 0.75
    let corpus = build_spread_corpus(128, NEEDLE_COUNT);
    let query = make_clv(TOPIC_SEED, None, 0.0);
    let selected = clv_top_k(&corpus, &query, 64);
    let rec = needle_recall_at_k(&selected, &corpus.needle_ids);
    eprintln!("[clv_path_recall] needle@8={rec:.3}（探针路径应 ≥ 0.75）");
    assert!(
        rec >= 0.75,
        "探针路径多针召回 {rec:.3} < 0.75，CLV 探针机制失效"
    );
}

#[test]
fn test_top_k_select_nth_stability() {
    // Top-K 结果确定性：同输入同输出（可复现性）
    let corpus = build_spread_corpus(128, NEEDLE_COUNT);
    let query = make_clv(TOPIC_SEED, None, 0.0);
    let a = clv_top_k(&corpus, &query, 32);
    let b = clv_top_k(&corpus, &query, 32);
    assert_eq!(a, b);
    // 选中集大小 = k（未超界）
    assert_eq!(a.len(), 32);
}
