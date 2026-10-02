//! 四路召回融合入口 — 二期启动（v2.3 §(d)⑨ / §(b.3)「二期四路召回+RRF」）
//!
//! 对应架构层: L2 Memory（mlc-engine）
//! 对应设计源: `Chimera架构重构与优化方案_v2.3` §(b.3)「检索分期：…二期四路召回
//! +RRF（复用 repo-wiki search.rs:171 k=60 + hcw rerank.rs 预算模式）。触发条件
//! 进 Wave 4」；§(d)⑨ Wave 3「检索预算裁剪(一期)+**四路召回入口(二期启动)**」
//!
//! # 范围纪律（启动 ≠ 全量启用）
//! 本模块交付 **融合入口层**（Wave 3「二期启动」义务）：把仓库已落地的四条召回
//! 路径的**有序 id 输出**统一经 L0 `recall_fusion::fuse_ranked_lists`（k=60，
//! 与 repo-wiki `rrf_fuse` 同参）融合：
//!
//! | 路 | 既有实现（已验证） | 本入口的输入形状 |
//! |----|--------------------|--------------------|
//! | 语义向量 | `l2_semantic::recall_by_clv` / `recall_within_token_budget`(§(b.3) 一期) | `(MemoryId, score)` 降序表 |
//! | 情节(quest/时间) | `engine::recall_by_quest` → `l1_episodic::query_by_quest` | 有序 `MemoryId` 表 |
//! | 图扩召回 | `memory_graph::recall_with_graph` | 有序 `MemoryId` 表 |
//! | 认识分库 | `engine::partition_by_epistemic`(§(b.3) 正交维) | 分库内有序 `MemoryId` 表 |
//!
//! **完整编排/启用属 Wave 4 触发条件**（§(b.3) 明示）：调用方按需接入的自动
//! 四路并行调度、cross-encoder 重排（仓内实锤 0 命中，不虚构）均不在本波范围。
//! 本层不修改任何既有 `recall_*` API（非破坏启动）；hcw rerank 预算模式接入点
//! 经 `FusionTuning` 显式留出（默认等权 k=60）。
//!
//! # WHY 融合入口而非引擎内并行调度
//! L(N)→L(N-1) 依赖铁律下各召回路径已属正确归属层；引擎内并行编排需要策略/
//! 配额联动（omega-learner S7 接缝，R2 冻结不动），属 Wave 4 决策面。启动层把
//! 「算法融合」与「编排调度」解耦，正合 §(b.3)「触发条件进 Wave 4」的分期纪律。

use nexus_contracts::recall_fusion::{fuse_ranked_lists, FusedItem, WeightedList, DEFAULT_RRF_K};

use crate::error::MlcError;
use crate::types::MemoryId;

/// 一条召回路径的有序输出（相关性降序）。
#[derive(Debug, Clone, PartialEq)]
pub struct RecallPath {
    /// 路径标识（诊断/rerank 权重路由用；如 "semantic"/"episodic"/"graph"/"epistemic"）
    pub name: &'static str,
    /// 该路权重（默认 1.0 等权；hcw rerank 预算模式接入点）
    pub weight: f32,
    /// 有序命中 id 表（rank 由位置导出，1-based）
    pub hits: Vec<MemoryId>,
}

impl RecallPath {
    /// 新建等权路径。
    #[must_use]
    pub fn new(name: &'static str, hits: Vec<MemoryId>) -> Self {
        Self {
            name,
            weight: 1.0,
            hits,
        }
    }

    /// 设定权重（链式；hcw rerank/路质量接入点）。
    #[must_use]
    pub fn with_weight(mut self, weight: f32) -> Self {
        self.weight = weight;
        self
    }
}

/// 融合调参 — RRF 平滑常数与 Top-K（默认 k=60，与 repo-wiki 同参）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FusionTuning {
    /// RRF 平滑常数 k
    pub rrf_k: u32,
    /// 融合输出上限
    pub top_k: usize,
}

impl Default for FusionTuning {
    fn default() -> Self {
        Self {
            rrf_k: DEFAULT_RRF_K,
            top_k: 10,
        }
    }
}

/// 四路召回融合入口（二期启动）：多路有序命中 → 确定性 RRF 融合 Top-K。
///
/// 错误路径预留给 Wave 4 编排期（如跨路配额联调）；当前纯融合永不失败，
/// 返回 `Result` 以稳定未来签名（避免破坏性变更，D1 接口零破坏纪律）。
///
/// ```
/// # use mlc_engine::recall_fusion::{fuse_recall_paths, RecallPath, FusionTuning};
/// # use mlc_engine::types::MemoryId;
/// # fn mid(s: &str) -> MemoryId { MemoryId::from(s.to_string()) }
/// let semantic = RecallPath::new("semantic", vec![mid("m-1"), mid("m-2")]);
/// let graph = RecallPath::new("graph", vec![mid("m-2"), mid("m-3")]);
/// let out = fuse_recall_paths(&[semantic, graph], FusionTuning::default()).unwrap();
/// // m-2 双路共同命中 → RRF 分数最高（跨路一致性优先）
/// assert_eq!(out[0].id, "m-2");
/// assert_eq!(out[0].list_hits, 2);
/// ```
pub fn fuse_recall_paths(
    paths: &[RecallPath],
    tuning: FusionTuning,
) -> Result<Vec<FusedItem>, MlcError> {
    // MemoryId(String) 借用为零拷贝：WeightedList 需要 &[String]，newtype 内部
    // 即 String，经 as_str 集中物化一次（融合表规模 ≤ top_k×路数，非热路径放大）。
    let owned: Vec<Vec<String>> = paths
        .iter()
        .map(|p| p.hits.iter().map(|id| id.as_str().to_string()).collect())
        .collect();
    let lists: Vec<WeightedList<'_>> = owned
        .iter()
        .zip(paths.iter())
        .map(|(ids, p)| WeightedList::new(ids.as_slice()).with_weight(p.weight))
        .collect();
    Ok(fuse_ranked_lists(&lists, tuning.rrf_k, tuning.top_k))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试码 unwrap 为惯用法；E-5 意在治理生产码
mod tests {
    use super::*;

    fn mid(s: &str) -> MemoryId {
        MemoryId::from(s.to_string())
    }

    #[test]
    fn cross_path_agreement_ranks_first() {
        // §(b.1) 语义类比：多路共同召回的条目优先（跨路一致性 = 最强信号）
        let semantic = RecallPath::new("semantic", vec![mid("a"), mid("b")]);
        let episodic = RecallPath::new("episodic", vec![mid("b"), mid("c")]);
        let out = fuse_recall_paths(
            &[semantic, episodic],
            FusionTuning {
                top_k: 3,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(out[0].id, "b", "双路共同命中应居首");
        assert_eq!(out[0].list_hits, 2);
    }

    #[test]
    fn four_paths_fuse_deterministically() {
        // 四路（语义/情节/图/认识分库）各自不同命中集，融合两次逐位一致
        let paths = vec![
            RecallPath::new("semantic", vec![mid("m1"), mid("m2"), mid("m3")]),
            RecallPath::new("episodic", vec![mid("m3"), mid("m4")]),
            RecallPath::new("graph", vec![mid("m2"), mid("m5")]),
            RecallPath::new("epistemic", vec![mid("m5"), mid("m1")]),
        ];
        let a = fuse_recall_paths(&paths, FusionTuning::default()).unwrap();
        let b = fuse_recall_paths(&paths, FusionTuning::default()).unwrap();
        assert_eq!(a, b, "融合输出必须确定性可复现");
        assert_eq!(a.len(), 5, "默认 top_k=10 ≥ 全集 5 条，应全量输出");
        // m1: semantic rank1 + epistemic rank2；m2: semantic rank2 + graph rank1
        // 二者同为双路；分数 m1=1/61+1/62 = m2=1/62+1/61 相等 → best_rank 均 1，
        // 首现路索引 semantic(0) 相同 → 首现 rank m1=1 < m2=2 → m1 在前
        assert_eq!(a[0].id, "m1");
        assert_eq!(a[1].id, "m2");
    }

    #[test]
    fn rerank_weight_hook_changes_order() {
        // hcw rerank 预算模式接入点：调高图扩路权重应把纯图命中顶前
        let semantic = RecallPath::new("semantic", vec![mid("s1")]);
        let graph = RecallPath::new("graph", vec![mid("g1")]).with_weight(3.0);
        let out = fuse_recall_paths(
            &[semantic, graph],
            FusionTuning {
                top_k: 2,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(out[0].id, "g1", "高权重路 rank1 应胜出");
    }

    #[test]
    fn empty_paths_yield_empty() {
        assert!(fuse_recall_paths(&[], FusionTuning::default())
            .unwrap()
            .is_empty());
        assert!(fuse_recall_paths(
            &[RecallPath::new("semantic", vec![])],
            FusionTuning::default()
        )
        .unwrap()
        .is_empty());
    }
}
