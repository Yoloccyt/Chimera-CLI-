//! 召回评测 harness — PROBE P0 评测尺子（feature `hcw-recall`，ADR-034 编译期门控）
//!
//! 对应任务: PROBE 实施计划 §2.2 P0.2（四类评测 harness）
//! 对应病理: H3（召回质量零度量）— 本模块是"先造尺子"的落点
//!
//! # 职责
//! 对**给定选中块集**（由外部选择路径产生，P0.5 接入 Static/recall 双路径）计算
//! 四类召回质量指标：
//! - `recall@tier`（单针）：针块是否被选中（needle.rs）
//! - `needle_recall@8`（多针）：8 根针命中数 / 8（needle.rs）
//! - `position_bias`（位置扫描）：中段召回率 ÷ 头尾召回率，lost-in-the-middle 量化（position.rs）
//! - `chain_success_rate`（多跳）：3 事实链全部命中的比例（multihop.rs）
//!
//! # 与 v5.0 流水线正交
//! `recall/` 的 coarse/fine/rerank/streaming 是**生产检索**（给定 query 选块）；
//! `eval/` 是**评测尺子**（给定块集打分）。二者解耦：同一选中块集可被任何
//! 路径（Static compressor / recall 管线 / 随机基线）产生后送评。
//!
//! # 确定性
//! 语料生成与 CLV 构造全部用 SplitMix64 强混合哈希（纯安全算术），固定种子
//! 可复现——P0.5 双基线对照表要求数字可复现（固定种子）。
//! SplitMix64 保证不同 seed 的 512 维向量近似正交，避免生成器系统性偏差
//! 混入测度（对照组验证：分离度 ≈ 0，见 tests/probe_clv_separation_test.rs）。

use std::collections::HashSet;

use nexus_core::CLV;

use super::types::BlockId;

// === 子模块声明（PROBE P0.2 四类评测基准）===
pub mod multihop;
pub mod needle;
pub mod position;
pub mod report;
// PROBE P2.3: 召回哨兵（首个生产发布者 HcwRecallReported/Degraded）
pub mod sentinel;

// === 类型重导出（子模块经 eval 统一访问 recall::types，避免跨级 use）===
pub use super::types::BlockId as EvalBlockId;

/// CLV 固定维度（与 nexus-core 一致）
pub const CLV_DIM: usize = 512;

/// 评测用块 — 语料的最小组成单元
///
/// # 字段
/// - `id`: 块标识（与 `BlockId` 一致，`String`）
/// - `content`: 块文本（针事实可机器判定的载体，二期接入真实代码切片）
/// - `clv`: 块代表向量（探针打分的语义基础）
/// - `temporal`: 强时序标记（P1 位置重排豁免依据；评测语料含时序块验证原序性）
#[derive(Debug, Clone, PartialEq)]
pub struct EvalBlock {
    /// 块标识（与 VectorStore/选中集一致）
    pub id: BlockId,
    /// 块文本内容
    pub content: String,
    /// 块代表向量（SplitMix64 确定性生成）
    pub clv: CLV,
    /// 强时序标记（diff/对话流等，P1 重排豁免）
    pub temporal: bool,
}

impl EvalBlock {
    /// 创建评测块
    ///
    /// # 参数
    /// - `id`: 块标识
    /// - `content`: 块文本
    /// - `clv`: 块代表向量
    /// - `temporal`: 强时序标记
    pub fn new(
        id: impl Into<String>,
        content: impl Into<String>,
        clv: CLV,
        temporal: bool,
    ) -> Self {
        Self {
            id: id.into(),
            content: content.into(),
            clv,
            temporal,
        }
    }
}

/// 评测语料 — 块集合 + 针块标记
///
/// 由 [`CorpusBuilder`] 确定性构造；针块 = 需被召回的事实块（ground truth）。
#[derive(Debug, Clone)]
pub struct EvalCorpus {
    /// 全部块（按语料顺序，位置扫描依赖索引深度）
    pub blocks: Vec<EvalBlock>,
    /// 针块 ID 集合（ground truth）
    pub needle_ids: HashSet<BlockId>,
}

impl EvalCorpus {
    /// 返回语料块数
    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    /// 语料是否为空
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    /// 返回针块 ID 列表（确定性顺序：按语料索引排序）
    ///
    /// WHY 排序: 保证多针评测的针顺序可复现（HashSet 无序，输出需稳定）
    pub fn needle_ids_sorted(&self) -> Vec<BlockId> {
        let mut ids: Vec<BlockId> = self.needle_ids.iter().cloned().collect();
        ids.sort();
        ids
    }
}

/// 确定性 CLV 生成（SplitMix64 强混合）
///
/// # 参数
/// - `seed`: 种子（决定噪声分量）
/// - `topic`: 可选主题向量（`None` 时纯随机）
/// - `topic_bias`: 主题占比 [0,1]，`0.0` 时纯噪声
///
/// # 返回值
/// 512 维 CLV，非零向量
///
/// WHY SplitMix64: 简单线性哈希在相邻 seed 下产生高度相关序列（余弦 0.998），
/// 会污染分离度测度；SplitMix64 保证不同 seed 近似正交。
/// WHY 零均值映射 [−1,1): 全正分量映射（如 [0,1)）使任意两个随机向量的
/// 余弦系统性偏高（实测 0.8），压缩判别力信号；零均值下高维随机向量
/// 近似正交（余弦 ≈ 0），主题信号（topic_bias² 量级）更可分辨。
pub fn make_clv(seed: u64, topic: Option<&CLV>, topic_bias: f32) -> CLV {
    let mut v: Vec<f32> = (0..CLV_DIM)
        .map(|j| {
            let mut z = seed.wrapping_add((j as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15));
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            // 映射到 [−1,1)（零均值，高维近似正交）
            ((z >> 11) as f32) / (1u64 << 53) as f32 * 2.0 - 1.0
        })
        .collect();
    if let Some(t) = topic {
        let t_slice = t.as_slice();
        for (i, val) in v.iter_mut().enumerate() {
            *val = topic_bias * t_slice[i] + (1.0 - topic_bias) * *val;
        }
    }
    CLV::from_vec(v).expect("CLV dimension must be 512")
}

/// 语料构建器 — 确定性合成语料（P0 一期；二期替换为真实代码库切片）
///
/// # 用法
/// ```rust
/// use hcw_window::recall::eval::CorpusBuilder;
/// let corpus = CorpusBuilder::new()
///     .with_block_count(256)
///     .with_needle_count(8)
///     .with_needle_topic_bias(0.6)
///     .build()
///     .expect("build should succeed");
/// assert_eq!(corpus.needle_ids.len(), 8);
/// assert_eq!(corpus.len(), 256);
/// ```
#[derive(Debug, Clone)]
pub struct CorpusBuilder {
    /// 总块数（含针块与干扰块）
    block_count: usize,
    /// 针块数（共享主题）
    needle_count: usize,
    /// 针块主题占比（判别力来源）
    needle_topic_bias: f32,
    /// 时序块占比（验证 temporal 豁免路径）
    temporal_ratio: f32,
}

impl Default for CorpusBuilder {
    fn default() -> Self {
        Self {
            block_count: 256,
            needle_count: 8,
            needle_topic_bias: 0.6,
            temporal_ratio: 0.1,
        }
    }
}

impl CorpusBuilder {
    /// 创建默认语料构建器（256 块 / 8 针 / 主题 0.6 / 时序 10%）
    pub fn new() -> Self {
        Self::default()
    }

    /// 设置总块数
    ///
    /// # 约束
    /// `block_count` 必须 ≥ `needle_count`（否则 build 返回 `InvalidCorpus`）
    pub fn with_block_count(mut self, count: usize) -> Self {
        self.block_count = count;
        self
    }

    /// 设置针块数（共享主题的事实块）
    pub fn with_needle_count(mut self, count: usize) -> Self {
        self.needle_count = count;
        self
    }

    /// 设置针块主题占比（默认 0.6；分离度 ≥ 0.1 的判别力来源）
    pub fn with_needle_topic_bias(mut self, bias: f32) -> Self {
        self.needle_topic_bias = bias;
        self
    }

    /// 设置时序块占比（默认 0.1）
    pub fn with_temporal_ratio(mut self, ratio: f32) -> Self {
        self.temporal_ratio = ratio;
        self
    }

    /// 构造语料
    ///
    /// # 错误
    /// - `block_count < needle_count`: 针块数超过总块数
    /// - `needle_count == 0`: 无语料针（评测无 ground truth）
    pub fn build(self) -> Result<EvalCorpus, EvalError> {
        if self.block_count < self.needle_count {
            return Err(EvalError::InvalidCorpus {
                reason: format!(
                    "block_count {} < needle_count {}",
                    self.block_count, self.needle_count
                ),
            });
        }
        if self.needle_count == 0 {
            return Err(EvalError::InvalidCorpus {
                reason: "needle_count must be > 0".into(),
            });
        }

        let topic = make_clv(0x5EED_CAFE, None, 0.0);
        let mut blocks = Vec::with_capacity(self.block_count);
        let mut needle_ids = HashSet::with_capacity(self.needle_count);

        // 针块：前 needle_count 个（语料头部；位置扫描任务另行指定深度）
        for i in 0..self.needle_count {
            let id = format!("needle-{i:03}");
            let clv = make_clv(1000 + i as u64, Some(&topic), self.needle_topic_bias);
            let temporal = ((i as f32) / self.needle_count as f32) < self.temporal_ratio;
            blocks.push(EvalBlock::new(
                id.clone(),
                format!("needle fact {i}: topic fact sentence for machine check"),
                clv,
                temporal,
            ));
            needle_ids.insert(id);
        }

        // 干扰块：独立随机 CLV（与针块可判别，分离度 ≥ 0.1）
        for i in 0..(self.block_count - self.needle_count) {
            let id = format!("noise-{i:03}");
            let clv = make_clv(5000 + i as u64, None, 0.0);
            let temporal =
                ((i as f32) / (self.block_count - self.needle_count) as f32) < self.temporal_ratio;
            blocks.push(EvalBlock::new(
                id,
                format!("noise block {i}"),
                clv,
                temporal,
            ));
        }

        Ok(EvalCorpus { blocks, needle_ids })
    }
}

/// 评测错误 — 语料构造失败
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum EvalError {
    /// 语料参数不合法（块数/针数约束违反）
    #[error("invalid corpus: {reason}")]
    InvalidCorpus {
        /// 不合法原因（人类可读描述）
        reason: String,
    },
}

// ============================================================
// 共享指标函数（所有评测任务的核心计算，纯函数无状态）
// ============================================================

/// 计算多针召回率 needle_recall@k — 命中针数 / 总针数
///
/// # 参数
/// - `selected`: 选中块集（任意选择路径的输出）
/// - `needle_ids`: 针块 ID 集合（ground truth）
///
/// # 返回值
/// ∈ [0.0, 1.0]；`needle_ids` 为空时返回 0.0（无 ground truth 不评分）
///
/// # 复杂度
/// O(|selected| + |needle_ids|)（HashSet 查找）
pub fn needle_recall_at_k(selected: &[BlockId], needle_ids: &HashSet<BlockId>) -> f32 {
    if needle_ids.is_empty() {
        return 0.0;
    }
    let hit = selected
        .iter()
        .filter(|id| needle_ids.contains(*id))
        .count();
    hit as f32 / needle_ids.len() as f32
}

/// 计算位置偏置比 position_bias — 中段召回率 ÷ 头尾召回率
///
/// lost-in-the-middle 量化指标：理想值 → 1.0；中段注意力衰减时 < 0.6。
/// 头尾召回率为 0 时返回 0.0（避免除零；无头尾命中即中段无参照）。
///
/// # 参数
/// - `selected`: 选中块集
/// - `head_needles`: 头部深度（10%/30%）针块
/// - `middle_needles`: 中段深度（50%）针块
/// - `tail_needles`: 尾部深度（70%/90%）针块
///
/// # 返回值
/// ∈ [0.0, 1.0]（f32 全程，禁止 `as f64` 红线）
pub fn position_bias(
    selected: &[BlockId],
    head_needles: &HashSet<BlockId>,
    middle_needles: &HashSet<BlockId>,
    tail_needles: &HashSet<BlockId>,
) -> f32 {
    let head = needle_recall_at_k(selected, head_needles);
    let mid = needle_recall_at_k(selected, middle_needles);
    let tail = needle_recall_at_k(selected, tail_needles);
    let edge = (head + tail) * 0.5;
    if edge <= f32::EPSILON {
        return 0.0;
    }
    (mid / edge).clamp(0.0, 1.0)
}

/// 计算多跳链路成功率 chain_success_rate — 完整链路全部命中的比例
///
/// 每条链由 N 个分散事实块构成（如 A→B→C 三跳），缺一块即该链失败。
///
/// # 参数
/// - `selected`: 选中块集
/// - `chains`: 链路集合（每链为针块 ID 序列）
///
/// # 返回值
/// ∈ [0.0, 1.0]；`chains` 为空时返回 0.0
pub fn chain_success_rate(selected: &[BlockId], chains: &[Vec<BlockId>]) -> f32 {
    if chains.is_empty() {
        return 0.0;
    }
    let selected_set: HashSet<&BlockId> = selected.iter().collect();
    let success = chains
        .iter()
        .filter(|chain| chain.iter().all(|id| selected_set.contains(id)))
        .count();
    success as f32 / chains.len() as f32
}

/// 检查选中块集是否包含指定针块（单针 recall@tier）
///
/// # 返回值
/// `true` 当且仅当针块在选中集中
pub fn needle_hit(selected: &[BlockId], needle: &BlockId) -> bool {
    selected.iter().any(|id| id == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_corpus_builder_defaults() {
        let corpus = CorpusBuilder::new().build().expect("build should succeed");
        assert_eq!(corpus.len(), 256);
        assert_eq!(corpus.needle_ids.len(), 8);
        assert_eq!(corpus.needle_ids_sorted().len(), 8);
    }

    #[test]
    fn test_corpus_builder_custom() {
        let corpus = CorpusBuilder::new()
            .with_block_count(128)
            .with_needle_count(4)
            .build()
            .expect("build should succeed");
        assert_eq!(corpus.len(), 128);
        assert_eq!(corpus.needle_ids.len(), 4);
        // 针块在语料前部
        assert!(corpus.blocks[0].id.starts_with("needle-"));
    }

    #[test]
    fn test_corpus_builder_invalid() {
        // 针数 > 块数
        let err = CorpusBuilder::new()
            .with_block_count(4)
            .with_needle_count(8)
            .build()
            .expect_err("should fail: needles > blocks");
        assert!(matches!(err, EvalError::InvalidCorpus { .. }));
        // 零针
        let err = CorpusBuilder::new()
            .with_needle_count(0)
            .build()
            .expect_err("should fail: zero needles");
        assert!(matches!(err, EvalError::InvalidCorpus { .. }));
    }

    #[test]
    fn test_needle_recall_at_k_full_and_partial() {
        let corpus = CorpusBuilder::new()
            .with_block_count(64)
            .with_needle_count(8)
            .build()
            .unwrap();
        let needles = &corpus.needle_ids;
        // 全中
        let all: Vec<BlockId> = corpus.blocks.iter().map(|b| b.id.clone()).collect();
        assert!((needle_recall_at_k(&all, needles) - 1.0).abs() < 1e-6);
        // 半中：选中前 4 根针
        let half: Vec<BlockId> = corpus.needle_ids_sorted().into_iter().take(4).collect();
        assert!((needle_recall_at_k(&half, needles) - 0.5).abs() < 1e-6);
        // 空选中集
        assert_eq!(needle_recall_at_k(&[], needles), 0.0);
        // 空针集
        assert_eq!(needle_recall_at_k(&all, &HashSet::new()), 0.0);
    }

    #[test]
    fn test_position_bias_ideal_and_degraded() {
        let corpus = CorpusBuilder::new()
            .with_block_count(64)
            .with_needle_count(6)
            .build()
            .unwrap();
        let ids = corpus.needle_ids_sorted();
        let head: HashSet<BlockId> = ids.iter().take(2).cloned().collect();
        let middle: HashSet<BlockId> = ids.iter().skip(2).take(2).cloned().collect();
        let tail: HashSet<BlockId> = ids.iter().skip(4).take(2).cloned().collect();

        // 理想：全选中 → bias = 1.0
        let all: Vec<BlockId> = corpus.blocks.iter().map(|b| b.id.clone()).collect();
        let ideal = position_bias(&all, &head, &middle, &tail);
        assert!((ideal - 1.0).abs() < 1e-6, "ideal bias={ideal}");

        // 退化：只选中头尾，中段全丢 → bias = 0.0
        let edge_only: Vec<BlockId> = head.iter().chain(tail.iter()).cloned().collect();
        let degraded = position_bias(&edge_only, &head, &middle, &tail);
        assert_eq!(degraded, 0.0, "degraded bias={degraded}");

        // 空头尾 → 0.0（防除零）
        let no_edge = position_bias(
            &middle.iter().cloned().collect::<Vec<_>>(),
            &HashSet::new(),
            &middle,
            &HashSet::new(),
        );
        assert_eq!(no_edge, 0.0);
    }

    #[test]
    fn test_chain_success_rate() {
        let corpus = CorpusBuilder::new()
            .with_block_count(64)
            .with_needle_count(8)
            .build()
            .unwrap();
        let ids = corpus.needle_ids_sorted();
        let chain_a = vec![ids[0].clone(), ids[1].clone(), ids[2].clone()];
        let chain_b = vec![ids[3].clone(), ids[4].clone(), ids[5].clone()];
        let chains = vec![chain_a.clone(), chain_b.clone()];

        // 全中 → 1.0
        let all: Vec<BlockId> = corpus.blocks.iter().map(|b| b.id.clone()).collect();
        assert!((chain_success_rate(&all, &chains) - 1.0).abs() < 1e-6);
        // 缺链 A 的一个块 → 0.5
        let partial: Vec<BlockId> = all
            .iter()
            .filter(|id| **id != chain_a[1])
            .cloned()
            .collect();
        assert!((chain_success_rate(&partial, &chains) - 0.5).abs() < 1e-6);
        // 空链集 → 0.0
        assert_eq!(chain_success_rate(&all, &[]), 0.0);
    }

    #[test]
    fn test_needle_hit() {
        let corpus = CorpusBuilder::new()
            .with_block_count(16)
            .with_needle_count(2)
            .build()
            .unwrap();
        let n0 = corpus.needle_ids_sorted()[0].clone();
        assert!(needle_hit(std::slice::from_ref(&n0), &n0));
        assert!(!needle_hit(&[], &n0));
        assert!(!needle_hit(&["other".into()], &n0));
    }

    #[test]
    fn test_make_clv_deterministic() {
        // 同种子同输出（可复现性）
        let a = make_clv(42, None, 0.0);
        let b = make_clv(42, None, 0.0);
        assert_eq!(a, b);
        // 不同种子弱相关（零均值高维向量近似正交）
        let c = make_clv(43, None, 0.0);
        let sim = a.cosine_similarity(&c);
        assert!(
            sim.abs() < 0.5,
            "random vectors should be weakly correlated (sim={sim:.4})"
        );
        // 同主题向量显著高于随机基线（判别性前提）
        let topic = make_clv(0x5EED_CAFE, None, 0.0);
        let n1 = make_clv(1000, Some(&topic), 0.6);
        let n2 = make_clv(1001, Some(&topic), 0.6);
        let intra = n1.cosine_similarity(&n2);
        assert!(
            intra > sim + 0.2,
            "same-topic similarity {intra:.4} should exceed random baseline {sim:.4}"
        );
    }
}
