//! 查询探针打分 — PROBE P1.2（SnapKV 式查询探针 × CLV 通用语义货币）
//!
//! 对应任务: PROBE P1 实施计划 T1（P1.2 探针打分）
//! 对应机制: SnapKV 观察窗的脚手架平移——模型内 attention 投票 → 语义相似度投票
//! 对应创新点: N1（CLV 探针打分，设计文档 §5.3）
//!
//! # 核心职责
//! - [`ProbeWeights`]: α·静态分 + β·探针分的融合权重（hcw-window 内部定义，
//!   **不入 nexus-contracts**——L0 契约零改动，计划 C3/E8 硬约束）
//! - [`mix_probe`]: 当前 query 的 CLV + 最近 K 轮对话的混合探针向量
//!   （SnapKV"观察窗"的脚手架对应物，均值池化保留 512 维）
//! - [`score_with_probe`]: 候选块最终分 = α·静态分 + β·探针分（全 f32）
//! - [`probe_health`]: 探针质量检测（NaN / 零向量率 >50%）——异常时调用方
//!   回退 Static 路径并留痕（R13 降级必告知，设计文档 §4.6）
//!
//! # 红线
//! - f32 全程不转 f64（§4.4 #6）
//! - `#![forbid(unsafe_code)]`（crate 级已声明）
//! - 零向量边界复用 `CLV::cosine_similarity` 既有处理（返回 0.0 非 NaN）

use std::collections::HashMap;

use nexus_core::CLV;

use crate::recall::types::BlockId;

/// 探针融合权重 — 静态分与探针分的配比（α + β ≈ 1.0）
///
/// # 设计决策（WHY）
/// - 定义于 hcw-window 内部而非 nexus-contracts：L0 契约零改动（计划硬约束）；
///   若未来需跨 crate 共享，由 ADR 评估升级路径
/// - Copy + Clone：f32 聚合体，零成本传递
/// - `is_valid` 容差 1e-3：对齐 `nexus_contracts::SelectorWeights::is_valid` 范式
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProbeWeights {
    /// 静态分权重（默认 0.5）
    pub alpha: f32,
    /// 探针分权重（默认 0.5）
    pub beta: f32,
}

impl ProbeWeights {
    /// 默认权重 — 静态/探针均衡（0.5 : 0.5）
    ///
    /// WHY 0.5/0.5: P1 无参数优先原则（设计原则 P2）的起始点；
    /// α/β 配比的学习放 P2 阶段（S4 臂空间扩展，计划 §2.4）
    pub const DEFAULT: Self = Self {
        alpha: 0.5,
        beta: 0.5,
    };

    /// 创建探针权重
    ///
    /// # 参数
    /// - `alpha`: 静态分权重（应 ≥ 0）
    /// - `beta`: 探针分权重（应 ≥ 0）
    pub const fn new(alpha: f32, beta: f32) -> Self {
        Self { alpha, beta }
    }

    /// 校验权重合法性 — 非负且和 ≈ 1.0（容差 1e-3）
    ///
    /// WHY: 权重为评分公式系数，负值无意义；和偏离 1.0 会使综合分偏离
    /// 静态分/探针分的原始区间（对齐 SelectorWeights::is_valid 范式）
    pub fn is_valid(&self) -> bool {
        self.alpha >= 0.0 && self.beta >= 0.0 && (self.alpha + self.beta - 1.0).abs() <= 1e-3
    }
}

impl Default for ProbeWeights {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// 计算候选块最终分 = α·静态分 + β·探针分（全 f32）
///
/// # 参数
/// - `static_score`: 静态启发式分数（recency/frequency/relevance 加权，
///   来自 `compressor::compute_importance_score`）
/// - `probe_score`: 探针相似度（query 混合向量 × 块代表向量的余弦）
/// - `weights`: 融合权重（α + β ≈ 1.0）
///
/// # 返回值
/// 融合后分数 ∈ [0, 1] 量级（静态分与余弦分均在该区间）
///
/// # 异常处理
/// 调用方应先用 [`probe_health`] 检测探针质量；本函数对 NaN 输入返回 0.0
/// 防御（避免 NaN 污染 Top-K 排序——`select_nth_unstable_by` 对 NaN 行为未定义）
pub fn score_with_probe(static_score: f32, probe_score: f32, weights: ProbeWeights) -> f32 {
    if !static_score.is_finite() || !probe_score.is_finite() {
        return 0.0;
    }
    weights.alpha * static_score + weights.beta * probe_score
}

/// 混合探针向量 — 当前 query CLV + 最近 K 轮对话 CLV 的均值池化
///
/// # 参数
/// - `query_clv`: 当前 query 的 CLV
/// - `recent_dialogue`: 最近 K 轮对话的 CLV 列表（可为空）
///
/// # 返回值
/// 512 维混合向量（均值池化）；`recent_dialogue` 为空时返回 query 本身
///
/// # 设计决策（WHY）
/// 均值池化: SnapKV 观察窗的脚手架对应物——模型内用 attention 投票聚合，
/// 我们用量级相同的均值聚合（无参数、O(K×512)）；若未来需加权，
/// 由 P2 学习臂提供配比（接口已隔离为函数签名）
pub fn mix_probe(query_clv: &CLV, recent_dialogue: &[CLV]) -> CLV {
    if recent_dialogue.is_empty() {
        return query_clv.clone();
    }
    // 均值池化：query 权重 1 + 每轮对话权重 1（等权），全部 f32
    let n = (recent_dialogue.len() + 1) as f32;
    let mut acc: Vec<f32> = query_clv.as_slice().to_vec();
    for d in recent_dialogue {
        let slice = d.as_slice();
        for (i, v) in slice.iter().enumerate() {
            acc[i] += v;
        }
    }
    for v in acc.iter_mut() {
        *v /= n;
    }
    CLV::from_vec(acc).expect("CLV dimension must be 512")
}

/// 探针质量检测结果 — 异常时调用方回退 Static 路径（R13 降级必告知）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeHealth {
    /// 探针健康，可正常打分
    Healthy,
    /// 探针含 NaN（余弦计算输入异常，上游 CLV 污染）
    NotFinite,
    /// 候选块零向量率 > 50%（块向量缺失/退化，探针分无判别力）
    ExcessiveZeroVectors,
}

/// 检测探针质量 — NaN / 零向量率 >50% 单趟检测（不二次扫描）
///
/// # 参数
/// - `probe`: 混合探针向量
/// - `block_clvs`: 候选块代表向量集合
///
/// # 返回值
/// [`ProbeHealth`]：Healthy / NotFinite / ExcessiveZeroVectors
///
/// # 性能
/// O(|block_clvs|) 单趟；零向量判定复用 `cosine_similarity` 的范数边界语义
/// （零向量返回 0.0，此处直接按范数判断）
pub fn probe_health(probe: &CLV, block_clvs: &[CLV]) -> ProbeHealth {
    // 1. 探针自身有限性（NaN 污染最危险——会传染整个 Top-K 排序）
    if !probe.as_slice().iter().all(|v| v.is_finite()) {
        return ProbeHealth::NotFinite;
    }
    // 2. 候选块零向量率（单趟计数）
    let mut zero_count = 0usize;
    for clv in block_clvs {
        let norm_sq: f32 = clv.as_slice().iter().map(|v| v * v).sum();
        if norm_sq == 0.0 {
            zero_count += 1;
        }
    }
    if block_clvs.is_empty() {
        return ProbeHealth::Healthy;
    }
    let zero_ratio = zero_count as f32 / block_clvs.len() as f32;
    if zero_ratio > 0.5 {
        ProbeHealth::ExcessiveZeroVectors
    } else {
        ProbeHealth::Healthy
    }
}

/// 增量重打分缓存 — probe 哈希 + 块集版本双因子（PROBE P1.6）
///
/// # 设计（性能铁律"查询期零计算"）
/// 同 Quest 内探针平滑更新时，若探针指纹与块集版本均未变，复用上次分数
/// （跳过 cosine 全量重算，耗时 ≈ 0）；任一因子变化强制重算。
///
/// # 双因子（R-perf6：防误复用）
/// - `probe_hash`: 探针向量指纹（块集打分输入侧）
/// - `version`: 块集版本号（块增删改后由调用方递增——T3 缓存失效联动）
///
/// # 线程安全
/// 单写者（打分管线）单读者（快照消费）场景，无需锁；并发场景由调用方
/// 用 Arc<Mutex<ScoreCache>> 包裹（与 RecallCollector 同构）
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScoreCache {
    /// 探针指纹（双因子 1）
    probe_hash: u64,
    /// 块集版本号（双因子 2）
    version: u64,
    /// 缓存分数（BlockId → 探针分）
    scores: HashMap<BlockId, f32>,
}

impl ScoreCache {
    /// 创建空缓存
    pub fn new() -> Self {
        Self::default()
    }

    /// 计算探针向量指纹（确定性 FNV-1a，取前 16 维降低哈希碰撞面）
    ///
    /// WHY FNV-1a: 纯安全算术、O(1) 常数时间、与 SplitMix64 正交的简单散列；
    /// 指纹只用于"探针是否变化"判定，碰撞概率经 16 维 × 64 位混合可忽略
    pub fn probe_fingerprint(probe: &CLV) -> u64 {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for v in probe.as_slice().iter().take(16) {
            let bytes = v.to_bits(); // u32（4 字节）；循环 0..4 避免移位越界 panic
            for i in 0..4 {
                hash ^= ((bytes >> (i * 8)) & 0xFF) as u64;
                hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
            }
        }
        hash
    }

    /// 尝试命中缓存（双因子完全匹配才复用）
    ///
    /// # 参数
    /// - `probe_hash`: 当前探针指纹（`probe_fingerprint` 产出）
    /// - `version`: 当前块集版本号
    ///
    /// # 返回
    /// `Some(&scores)`：命中（可复用，跳过重算）；`None`：未命中（需重算后 put）
    pub fn try_hit(&self, probe_hash: u64, version: u64) -> Option<&HashMap<BlockId, f32>> {
        if self.probe_hash == probe_hash && self.version == version {
            Some(&self.scores)
        } else {
            None
        }
    }

    /// 写入缓存（重算完成后更新指纹/版本/分数）
    ///
    /// # 参数
    /// - `probe_hash`: 当前探针指纹
    /// - `version`: 当前块集版本号
    /// - `scores`: 本次重算的分数表
    pub fn put(&mut self, probe_hash: u64, version: u64, scores: HashMap<BlockId, f32>) {
        self.probe_hash = probe_hash;
        self.version = version;
        self.scores = scores;
    }

    /// 缓存是否非空（诊断）
    pub fn is_empty(&self) -> bool {
        self.scores.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_clv(seed: u64) -> CLV {
        // SplitMix64 强混合（与 recall/eval::make_clv 同模式，确定性）
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

    #[test]
    fn test_probe_weights_valid() {
        assert!(ProbeWeights::DEFAULT.is_valid());
        assert!(ProbeWeights::new(0.3, 0.7).is_valid());
        assert!(!ProbeWeights::new(-0.1, 1.1).is_valid()); // 负值
        assert!(!ProbeWeights::new(0.5, 0.6).is_valid()); // 和 = 1.1
    }

    #[test]
    fn test_score_with_probe_blend() {
        let w = ProbeWeights::new(0.3, 0.7);
        // 0.3×0.4 + 0.7×0.8 = 0.12 + 0.56 = 0.68
        let s = score_with_probe(0.4, 0.8, w);
        assert!((s - 0.68).abs() < 1e-5);
        // 纯静态（beta=0）
        let s2 = score_with_probe(0.4, 0.8, ProbeWeights::new(1.0, 0.0));
        assert!((s2 - 0.4).abs() < 1e-5);
        // 纯探针（alpha=0）
        let s3 = score_with_probe(0.4, 0.8, ProbeWeights::new(0.0, 1.0));
        assert!((s3 - 0.8).abs() < 1e-5);
    }

    #[test]
    fn test_score_with_probe_nan_defensive() {
        let w = ProbeWeights::DEFAULT;
        assert_eq!(score_with_probe(f32::NAN, 0.5, w), 0.0);
        assert_eq!(score_with_probe(0.5, f32::NAN, w), 0.0);
        assert_eq!(score_with_probe(f32::INFINITY, 0.5, w), 0.0);
    }

    #[test]
    fn test_mix_probe_empty_dialogue() {
        let q = make_clv(1);
        let m = mix_probe(&q, &[]);
        assert_eq!(m, q, "空对话时探针 = query 本身");
    }

    #[test]
    fn test_mix_probe_averages() {
        let q = make_clv(1);
        let d1 = make_clv(2);
        let d2 = make_clv(3);
        let m = mix_probe(&q, &[d1.clone(), d2.clone()]);
        // 均值池化：m[i] = (q[i] + d1[i] + d2[i]) / 3
        let qs = q.as_slice();
        let d1s = d1.as_slice();
        let d2s = d2.as_slice();
        let ms = m.as_slice();
        for i in 0..512 {
            let expected = (qs[i] + d1s[i] + d2s[i]) / 3.0;
            assert!((ms[i] - expected).abs() < 1e-5, "index {i}");
        }
    }

    #[test]
    fn test_probe_health_healthy() {
        let q = make_clv(1);
        let blocks = vec![make_clv(10), make_clv(11), make_clv(12)];
        assert_eq!(probe_health(&q, &blocks), ProbeHealth::Healthy);
        // 空块集视为 Healthy（无判别对象）
        assert_eq!(probe_health(&q, &[]), ProbeHealth::Healthy);
    }

    #[test]
    fn test_probe_health_not_finite() {
        // 构造含 NaN 的探针
        let mut v = vec![0.0f32; 512];
        v[0] = f32::NAN;
        let bad = CLV::from_vec(v).expect("512 dims");
        let blocks = vec![make_clv(10)];
        assert_eq!(probe_health(&bad, &blocks), ProbeHealth::NotFinite);
    }

    #[test]
    fn test_probe_health_excessive_zero_vectors() {
        let q = make_clv(1);
        // 4 个块中 3 个零向量（75% > 50%）
        let blocks = vec![CLV::zero(), CLV::zero(), CLV::zero(), make_clv(10)];
        assert_eq!(probe_health(&q, &blocks), ProbeHealth::ExcessiveZeroVectors);
        // 2 个零 + 2 个正常（50% 不超阈值）
        let blocks2 = vec![CLV::zero(), CLV::zero(), make_clv(10), make_clv(11)];
        assert_eq!(probe_health(&q, &blocks2), ProbeHealth::Healthy);
    }

    // ============================================================
    // PROBE P1.6: 增量重打分缓存测试
    // ============================================================

    #[test]
    fn test_probe_fingerprint_deterministic() {
        let a = make_clv(42);
        let b = make_clv(42);
        assert_eq!(
            ScoreCache::probe_fingerprint(&a),
            ScoreCache::probe_fingerprint(&b)
        );
        // 不同探针不同指纹
        let c = make_clv(43);
        assert_ne!(
            ScoreCache::probe_fingerprint(&a),
            ScoreCache::probe_fingerprint(&c)
        );
    }

    #[test]
    fn test_score_cache_hit_and_miss() {
        let mut cache = ScoreCache::new();
        let probe = make_clv(1);
        let hash = ScoreCache::probe_fingerprint(&probe);
        let mut scores = HashMap::new();
        scores.insert("b1".to_string(), 0.9f32);
        scores.insert("b2".to_string(), 0.7f32);

        // 未写入 → miss
        assert!(cache.try_hit(hash, 1).is_none());
        // 写入 → hit（双因子匹配）
        cache.put(hash, 1, scores.clone());
        let hit = cache.try_hit(hash, 1).expect("双因子匹配应命中");
        assert_eq!(hit.get("b1"), Some(&0.9));
    }

    #[test]
    fn test_score_cache_version_change_invalidates() {
        // 块集版本变化（块增删改）→ 强制重算
        let mut cache = ScoreCache::new();
        let probe = make_clv(1);
        let hash = ScoreCache::probe_fingerprint(&probe);
        let scores = HashMap::new();
        cache.put(hash, 1, scores);
        // 版本 2 → miss（必须重算）
        assert!(cache.try_hit(hash, 2).is_none());
    }

    #[test]
    fn test_score_cache_probe_change_invalidates() {
        // 探针变化（新 query）→ 强制重算
        let mut cache = ScoreCache::new();
        let p1 = make_clv(1);
        let p2 = make_clv(2);
        let h1 = ScoreCache::probe_fingerprint(&p1);
        let h2 = ScoreCache::probe_fingerprint(&p2);
        cache.put(h1, 1, HashMap::new());
        // 探针指纹变化 → miss
        assert!(cache.try_hit(h2, 1).is_none());
        // 原探针仍命中（同 Quest 平滑更新复用）
        assert!(cache.try_hit(h1, 1).is_some());
    }

    #[test]
    fn test_score_cache_overwrite() {
        // 重算后 put 覆盖旧缓存
        let mut cache = ScoreCache::new();
        let probe = make_clv(1);
        let hash = ScoreCache::probe_fingerprint(&probe);
        cache.put(hash, 1, HashMap::new());
        let mut scores = HashMap::new();
        scores.insert("b1".to_string(), 0.5f32);
        cache.put(hash, 2, scores);
        assert!(cache.try_hit(hash, 1).is_none(), "旧版本应失效");
        assert!(cache.try_hit(hash, 2).is_some(), "新版本应命中");
    }
}
