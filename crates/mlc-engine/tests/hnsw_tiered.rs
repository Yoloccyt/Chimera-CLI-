//! L2 分层检索(精确层 + HNSW 近似层)集成测试 — ADR-192
//!
//! # 断言纪律(近似算法测试三条红线的全盘继承)
//! hnsw_rs `Hnsw::new` 内部按 OS 随机种子生成层级(repo-wiki `hnsw_store.rs`
//! 测试注释已记录同型坑),故此处:
//! - **不断言**"结果集成员逐位等于某历史快照",只断言 **分数制质量判据**:
//!   召回质量比 Σgot/Σgt ≥ 0.99 与**分数容差 recall ≥ 95%**,
//!   外加 id 制 recall 的**回归哨兵**(≥ 0.90,参数接线错位时会大幅塌缩);
//!   返回分数本身逐位等于精确分数;
//! - 精确层(低于阈值)的**逐位不变**由
//!   `test_below_threshold_is_bitwise_identical_to_disabled_tier` 固定;
//! - 语料由固定种子 LCG 生成(同种子同语料,测试可复现)。
//!
//! # 规模选择(测试耗时预算纪律)
//! 4096 档(真实 `HNSW_MIN_ENTRIES`)经 `OnceLock` 共享一次性建图,避免重复
//! debug 建图成本;10k/100k 档的 recall@10 质量与延迟判据放
//! `benches/hnsw_tiered.rs`(release 静默态),因其 debug 建图 + 全量
//! ground-truth 扫描会击穿单 target 120s 预算。

#![allow(clippy::unwrap_used, clippy::expect_used)] // test code idiom; E-5 targets production code
#![forbid(unsafe_code)]

use std::sync::OnceLock;

use mlc_engine::l2_semantic::HNSW_MIN_ENTRIES;
use mlc_engine::{MemoryEntry, MemoryTier, SemanticMemory};
use nexus_contracts::util::xts_top_k_by;
use nexus_core::CLV;

/// 固定种子 LCG(不用 rand,避免依赖与夹具漂移)
fn lcg_next(state: &mut u64) -> u64 {
    *state = state
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407);
    *state >> 33
}

/// [-1.0, 1.0] 均匀伪随机数
fn lcg_f32(state: &mut u64) -> f32 {
    let raw = lcg_next(state) as f64;
    (raw / (1u64 << 31) as f64) as f32 - 1.0
}

/// 生成 `n_clusters` 个簇心(方向可分的 512 维向量)
fn make_centers(n_clusters: usize, rng: &mut u64) -> Vec<Vec<f32>> {
    (0..n_clusters)
        .map(|c| {
            let mut v = vec![0.0_f32; CLV::DIMENSION];
            v[c % CLV::DIMENSION] = 1.0;
            for _ in 0..16 {
                let d = (lcg_next(rng) as usize) % CLV::DIMENSION;
                v[d] += lcg_f32(rng) * 0.2;
            }
            v
        })
        .collect()
}

/// 生成 n 条语料(簇心 0.85 权重 + 0.15 噪声,64 簇),返回 (向量, id)
fn make_corpus(n: usize) -> (Vec<Vec<f32>>, Vec<String>) {
    let mut rng: u64 = 0x5eed_2026_0926;
    let centers = make_centers(64, &mut rng);
    let mut vectors = Vec::with_capacity(n);
    let mut ids = Vec::with_capacity(n);
    for i in 0..n {
        let center = &centers[i % centers.len()];
        let v: Vec<f32> = center
            .iter()
            .map(|&c| 0.85 * c + 0.15 * lcg_f32(&mut rng))
            .collect();
        vectors.push(v);
        ids.push(format!("m-{i}"));
    }
    (vectors, ids)
}

fn entry_with_raw(id: &str, raw: &[f32]) -> MemoryEntry {
    MemoryEntry::new(id, format!("content-{id}"), MemoryTier::L2Semantic)
        .with_clv(CLV::from_vec(raw.to_vec()).expect("CLV 构造应成功"))
}

fn fill(mem: &SemanticMemory, vectors: &[Vec<f32>], ids: &[String]) {
    for (raw, id) in vectors.iter().zip(ids.iter()) {
        mem.insert(entry_with_raw(id, raw)).unwrap();
    }
}

/// 查询向量:簇心方向加轻微噪声(模拟真实召回 query)
fn make_query(seed_offset: u64) -> Vec<f32> {
    let mut rng: u64 = 0x0a11_ce00_0000 ^ seed_offset;
    let centers = make_centers(64, &mut rng);
    let center = &centers[(seed_offset as usize) % centers.len()];
    center
        .iter()
        .map(|&c| 0.95 * c + 0.05 * lcg_f32(&mut rng))
        .collect()
}

/// 测试侧独立 ground-truth:逐条全量打分 + Top-K 截断
///
/// WHY 独立于实现的是**候选来源**（本函数对全部向量逐条打分 vs 实现的图候选选取）；
/// 截断工具与距离核均与实现共用（`xts_top_k_by` / `cosine_similarity_slices`）——
/// 前者避开红线 R8 的新站点，后者使分数逐位相等可断言。
fn ground_truth(
    vectors: &[Vec<f32>],
    ids: &[String],
    query: &[f32],
    k: usize,
) -> Vec<(String, f32)> {
    let mut scored: Vec<(String, f32)> = vectors
        .iter()
        .zip(ids.iter())
        .map(|(v, id)| {
            let sim = nexus_core::cosine_similarity_slices(v, query).clamp(0.0, 1.0);
            (id.clone(), sim)
        })
        .collect();
    // 红线 R8 合规范式：L0 `xts_top_k_by`（select_nth O(n) + 前 k 段二次排序），
    // 与实现侧同一截断工具→不新增 sort-then-truncate 站点（topk 登记门 V-newsite）
    let picked = xts_top_k_by(&mut scored, k, |a, b| {
        b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
    })
    .to_vec();
    picked
}

/// 达阈语料 + 已填充实例(多测共享一次建图,守测试耗时预算)
struct Corpus {
    vectors: Vec<Vec<f32>>,
    ids: Vec<String>,
    mem: SemanticMemory,
}

impl Corpus {
    /// 按 id 取回语料原始向量(id 形如 `m-<索引>`)
    fn vector_of(&self, id: &str) -> &[f32] {
        let idx: usize = id
            .strip_prefix("m-")
            .unwrap_or_else(|| panic!("语料 id 形态非 m-<n>: {id}"))
            .parse()
            .expect("语料 id 后缀应为 usize");
        &self.vectors[idx]
    }
}

/// 4096 条目(= `HNSW_MIN_ENTRIES`)语料 + 默认分层实例,进程内共享一次构造
fn shared_corpus() -> &'static Corpus {
    static CORPUS: OnceLock<Corpus> = OnceLock::new();
    CORPUS.get_or_init(|| {
        let n = HNSW_MIN_ENTRIES;
        let (vectors, ids) = make_corpus(n);
        let mem = SemanticMemory::new(n);
        fill(&mem, &vectors, &ids);
        Corpus { vectors, ids, mem }
    })
}

/// 10k 条目（真实 SLO 场景档）语料 + 默认分层实例，进程内共享一次构造
///
/// WHY 单独一份 static：建图是一次性成本，两档语料规模不同不可复用。
fn corpus_10k() -> &'static Corpus {
    static CORPUS: OnceLock<Corpus> = OnceLock::new();
    CORPUS.get_or_init(|| {
        let n = 10_000;
        let (vectors, ids) = make_corpus(n);
        let mem = SemanticMemory::new(n);
        fill(&mem, &vectors, &ids);
        Corpus { vectors, ids, mem }
    })
}

/// 分数制三档质量统计（4096 与 10k 两档共用，避免两处各写一份判据而漂移）
///
/// 返回 `(id_recall, 分数容差 recall, 质量比)`；过程中同时硬断言
/// 长度/降序契约与「返回分数逐位等于精确重算值」。
fn quality_stats(corpus: &Corpus, queries: u64) -> (f64, f64, f64) {
    let mut id_hits = 0usize;
    let mut tol_hits = 0usize;
    let mut sum_got = 0f64;
    let mut sum_gt = 0f64;
    for q in 0..queries {
        let raw_query = make_query(q);
        let query = CLV::from_vec(raw_query.clone()).unwrap();
        let got = corpus.mem.recall_by_clv(&query, 10).unwrap();
        assert_eq!(got.len(), 10, "长度契约:min(top_k, 条目数)");
        for w in got.windows(2) {
            assert!(w[0].1 >= w[1].1, "降序契约被破坏");
        }
        let gt = ground_truth(&corpus.vectors, &corpus.ids, &raw_query, 10);
        let gt_min = gt.iter().map(|(_, s)| *s).fold(f32::MAX, f32::min);
        sum_gt += gt.iter().map(|(_, s)| *s as f64).sum::<f64>();
        for (id, score) in &got {
            let exact_score =
                nexus_core::cosine_similarity_slices(corpus.vector_of(id.as_str()), &raw_query)
                    .clamp(0.0, 1.0);
            assert_eq!(
                score.to_bits(),
                exact_score.to_bits(),
                "id {} 分数偏离精确值",
                id.as_str()
            );
            sum_got += exact_score as f64;
            if gt.iter().any(|(gid, _)| gid.as_str() == id.as_str()) {
                id_hits += 1;
            }
            if exact_score >= gt_min - 1e-4 {
                tol_hits += 1;
            }
        }
    }
    let samples = (queries as usize * 10) as f64;
    (
        id_hits as f64 / samples,
        tol_hits as f64 / samples,
        sum_got / sum_gt,
    )
}

#[test]
fn test_tiered_recall_quality_at_10k_real_threshold() {
    // Spec Phase 2 integration 判据的 10k 档（真实 SLO 场景 + 默认阈值 4096）：
    // 近似层 recall@10 对精确 ground-truth 的分数制三档均须达标（ADR-192 D2b）。
    // 100k 档因 debug 下建图十秒级 + 全量 ground-truth 扫描会击穿单 target 时长预算，
    // 留在 benches/hnsw_tiered.rs（release 静默态，同样带硬断言）取数。
    let corpus = corpus_10k();
    let (id_recall, tol_recall, quality) = quality_stats(corpus, 3);
    let snap = corpus.mem.hnsw_tier_snapshot().unwrap();
    assert!(snap.built, "10k 达阈应已建图");
    assert_eq!(snap.live_points, corpus.mem.len().unwrap(), "图-主表一致性");
    assert_eq!(snap.tombstones, 0);
    println!(
        "[hnsw_tiered test 10k] id-recall={id_recall:.4} tol-recall={tol_recall:.4} quality={quality:.6}"
    );
    assert!(quality >= 0.99, "召回质量比 {quality:.4} < 0.99");
    assert!(tol_recall >= 0.95, "分数容差 recall {tol_recall:.4} < 0.95");
    assert!(
        id_recall >= 0.90,
        "id 制 recall {id_recall:.4} < 0.90（疑似图参数/接线回归）"
    );
}

#[test]
fn test_below_threshold_is_bitwise_identical_to_disabled_tier() {
    // 4095 条目(< HNSW_MIN_ENTRIES):分层实现必须与"关闭近似层"逐位一致,
    // 且不建图 —— ADR-192 D1「精确层逐位不变」的机检形式
    let n = HNSW_MIN_ENTRIES - 1;
    let (vectors, ids) = make_corpus(n);
    let tiered = SemanticMemory::new(n + 1);
    let off = SemanticMemory::with_hnsw_threshold(n + 1, 0);
    fill(&tiered, &vectors, &ids);
    fill(&off, &vectors, &ids);

    let raw_query = make_query(3);
    let query = CLV::from_vec(raw_query).unwrap();
    let a = tiered.recall_by_clv(&query, 10).unwrap();
    let b = off.recall_by_clv(&query, 10).unwrap();
    assert_eq!(a.len(), 10);
    for (x, y) in a.iter().zip(b.iter()) {
        assert_eq!(x.0, y.0, "精确层 id 序列漂移");
        assert_eq!(x.1.to_bits(), y.1.to_bits(), "精确层分数漂移");
    }
    assert!(
        !tiered.hnsw_tier_snapshot().unwrap().built,
        "低于阈值不应建图"
    );
}

#[test]
fn test_at_threshold_tier_engages_and_recall_quality_holds() {
    // 恰达阈值(4096):首召回惰性建图,图-主表一致;
    // 10 次 query 的召回质量按**分数制**三档判据把关（共用 `quality_stats`，
    // 与 10k 档同一份判据实现）——原因见文件头断言纪律：
    // id 制受图层级随机性波动影响,分数制稳定且贴近“召回质量”本意
    let corpus = shared_corpus();
    let (id_recall, tol_recall, quality) = quality_stats(corpus, 10);
    let snap = corpus.mem.hnsw_tier_snapshot().unwrap();
    assert!(snap.built, "达阈召回后应已建图");
    assert_eq!(snap.live_points, corpus.mem.len().unwrap(), "图-主表一致性");
    assert_eq!(snap.tombstones, 0);
    assert!(!snap.stale);
    println!(
        "[hnsw_tiered 4096] id-recall={id_recall:.4} tol-recall={tol_recall:.4} quality={quality:.6}"
    );
    assert!(
        quality >= 0.99,
        "召回质量比 Σgot/Σgt = {quality:.4} 低于契约 0.99"
    );
    assert!(
        tol_recall >= 0.95,
        "分数容差 recall@10 = {tol_recall:.4} 低于 ADR-192 契约阈值 0.95"
    );
    // id 制仅做回归哨兵:图不连通参数错位时会大幅塌缩(实测基线 0.96-1.00)
    assert!(
        id_recall >= 0.90,
        "id 制 recall@10 = {id_recall:.4} 大幅偏低,疑似图参数/接线回归"
    );
}

#[test]
fn test_repeated_recall_is_deterministic_at_scale() {
    // 同一实例同一 query 多次召回逐位一致(图不重建则搜索确定性)
    let corpus = shared_corpus();
    let query = CLV::from_vec(make_query(9)).unwrap();
    let first = corpus.mem.recall_by_clv(&query, 10).unwrap();
    for _ in 0..4 {
        let again = corpus.mem.recall_by_clv(&query, 10).unwrap();
        assert_eq!(first, again, "多次召回结果必须逐位一致");
    }
    let snap = corpus.mem.hnsw_tier_snapshot().unwrap();
    assert_eq!(snap.tombstones, 0, "只读召回不应改变侧表");
    assert_eq!(snap.live_points, corpus.mem.len().unwrap());
}

#[test]
fn test_heavy_eviction_keeps_graph_consistent_and_results_live() {
    // 容量饱和 → FIFO 驱逐持续打墓碑;任何时刻结果 id 必须仍存活,
    // 且图侧存活点 == 条目数(死点 200/1024 未达 25%,不应 stale)
    //
    // WHY 阈值取 256 而非 4096:本测要验的是"驱逐 ↔ 墓碑"接线与
    // 结果存活集,与规模无关;真实 4096 档由上面两测把关,避免重复 debug 建图。
    let (vectors, ids) = make_corpus(1_024);
    let mem = SemanticMemory::with_hnsw_threshold(1_024, 256);
    fill(&mem, &vectors, &ids);
    let query = CLV::from_vec(make_query(1)).unwrap();
    mem.recall_by_clv(&query, 10).unwrap(); // 建图
    assert_eq!(mem.hnsw_tier_snapshot().unwrap().live_points, 1_024);

    for i in 0..200 {
        let mut raw = vec![0.0_f32; CLV::DIMENSION];
        raw[(i * 7) % CLV::DIMENSION] = 1.0;
        raw[(i * 11) % CLV::DIMENSION] = 0.5;
        mem.insert(entry_with_raw(&format!("new-{i}"), &raw))
            .unwrap();
    }
    let snap = mem.hnsw_tier_snapshot().unwrap();
    assert_eq!(
        snap.live_points,
        mem.len().unwrap(),
        "驱逐后图-主表仍应一致"
    );
    assert_eq!(snap.tombstones, 200, "每次驱逐应留一个墓碑");
    assert!(!snap.stale, "200 死点 / 1024 存活未达 25%,不应 stale");

    let got = mem.recall_by_clv(&query, 10).unwrap();
    assert_eq!(got.len(), 10);
    for (id, _) in &got {
        assert!(
            mem.get(id.as_str()).is_ok(),
            "结果含已驱逐条目:{}",
            id.as_str()
        );
    }
    // 被驱逐的最旧 200 条不得再出现
    assert!(!got.iter().any(|(id, _)| id.as_str() == "m-0"));
    assert!(!got.iter().any(|(id, _)| id.as_str() == "m-199"));
}
