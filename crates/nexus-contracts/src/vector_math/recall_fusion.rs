//! 多路召回 RRF 融合原语 — L0 共享算法（v2.3 §(b.3) 二期启动 / §(d)⑨）
//!
//! 对应架构层: **L0 Contracts**（纯算法 + 零 crate 依赖，`util::xts_top_k_by` 同先例）
//! 对应设计源: `Chimera架构重构与优化方案_v2.3` §(b.3)「二期四路召回+RRF
//! （复用 `repo-wiki search.rs:171 k=60` + hcw `rerank.rs` 预算模式）」
//!
//! # WHY 下沉 L0
//! 四路召回（L2 语义向量 / L1 情节 quest·时间 / memory_graph 图扩召回 /
//! epistemic 分库）的融合属**算法原语**，与既有 `xts_top_k_by`（Top-K 稳定
//! 选择）同类。若放 mlc-engine(L2) 则 repo-wiki(L5) 无法复用；放各自层则
//! 重复实现且漂移。L0 单源使所有检索层可向下依赖共用。
//!
//! 算法与 `repo-wiki::search::rrf_fuse` 同参数制：**k=60、rank 1-based、
//! weight/(k+rank) 累加、f32 全程**（该处为双路特化；本原语为泛化 N 路）。
//! 两路行为与旧实现在等权下数学同构，为未来 repo-wiki 收敛到本原语留路
//! （不在本波强行重构，避免双写面扩大）。
//!
//! # 确定性
//! 输出按 `(score 降序, first_rank 升序, 首现列表索引升序, id 升序)` 全序，
//! 无任何 HashMap 迭代序泄漏（融合结果跨进程可复现，供回归断言）。

use serde::{Deserialize, Serialize};

/// RRF 平滑常数默认值（业界惯例 k=60，与 repo-wiki `HybridSearchConfig` 同值）。
pub const DEFAULT_RRF_K: u32 = 60;

/// 单路融合结果：一路的 ranked id 列表 + 该路权重。
#[derive(Debug, Clone, PartialEq)]
pub struct WeightedList<'a> {
    /// 该路召回的 id 序列（**必须按相关性降序**；rank 由位置导出，1-based）
    pub ids: &'a [String],
    /// 该路权重（1.0 为等权；<=0 的列表被跳过）
    pub weight: f32,
}

impl<'a> WeightedList<'a> {
    /// 新建一路输入（等权）。
    #[must_use]
    pub fn new(ids: &'a [String]) -> Self {
        Self { ids, weight: 1.0 }
    }

    /// 设定权重（链式）。
    #[must_use]
    pub fn with_weight(mut self, weight: f32) -> Self {
        self.weight = weight;
        self
    }
}

/// 融合后的单条结果。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FusedItem {
    /// 被融合命中的文档/条目 id
    pub id: String,
    /// RRF 累计分：`Σ_list weight / (k + rank)`（f32，与 repo-wiki 同精度纪律）
    pub score: f32,
    /// 该 id 在各输入列表中的最优（最小）1-based rank；单路命中时即该路 rank
    pub best_rank: usize,
    /// 命中的列表数（跨路一致性信号：≥2 表示多路共同召回）
    pub list_hits: u32,
    /// 确定性 tie-break 内部元数据（首现列表索引, 首现 rank）；输出前置零，
    /// 不入序列化（`serde(skip)`），避免污染对外形状。
    #[serde(skip)]
    _ord: (usize, usize),
}

/// N 路 ranked id 列表 → RRF 融合 Top-K。
///
/// - `k`：RRF 平滑常数（传 [`DEFAULT_RRF_K`] 与 repo-wiki 同参）；
/// - `top_k`：输出上限；0 ⇒ 空；
/// - 同一列表内重复 id 仅按**首个 rank** 计一次（防御去重）；
/// - 输出确定性全序（见模块文档），无 HashMap 迭代序泄漏。
#[must_use]
pub fn fuse_ranked_lists(lists: &[WeightedList<'_>], k: u32, top_k: usize) -> Vec<FusedItem> {
    if top_k == 0 || lists.is_empty() {
        return Vec::new();
    }
    // (id) -> (score, best_rank, list_hits, 首现列表索引, 首现 rank)
    let mut acc: std::collections::HashMap<&str, (f32, usize, u32, usize, usize)> =
        std::collections::HashMap::new();
    let kk = k.max(1) as f32;
    for (li, list) in lists.iter().enumerate() {
        if list.weight <= 0.0 {
            continue;
        }
        let mut seen_in_list: std::collections::HashSet<&str> = std::collections::HashSet::new();
        for (idx, id) in list.ids.iter().enumerate() {
            let rank = idx + 1; // 1-based，与 repo-wiki rrf_fuse 一致
            if !seen_in_list.insert(id.as_str()) {
                continue; // 同列表重复 id：仅首 rank 计一次
            }
            let contrib = list.weight / (kk + rank as f32);
            acc.entry(id.as_str())
                .and_modify(|(s, br, lh, ..)| {
                    *s += contrib;
                    *br = (*br).min(rank);
                    *lh += 1;
                })
                .or_insert((contrib, rank, 1, li, rank));
        }
    }
    let mut fused: Vec<FusedItem> = acc
        .into_iter()
        .map(
            |(id, (score, best_rank, list_hits, first_list, first_rank))| FusedItem {
                id: id.to_string(),
                score,
                best_rank,
                list_hits,
                // 确定性 tie-break 元数据入排序键，不出现在公共结构
                // （借私有排序 tuple 实现，避免污染 API 形状）
                _ord: (first_list, first_rank),
            },
        )
        .collect();
    // 全序键：score 降序 → best_rank 升序 → 首现列表索引升序 → 首现 rank 升序 → id 升序。
    // 红线 #8：Top-K 用 O(n) `select_nth_unstable_by` + O(k log k) 前段排序，
    // 复用同 crate `util::xts_top_k_by`（本模块头 :3 所声明的“同先例”由注释落地为实调用），
    // 取代此前“`sort_by` 全排后 `truncate`”反模式。xts 内部已含前段排序，
    // 本模块不再出现独立的 sort-then-truncate 站点（无需新登记 topk 基线）。
    fn cmp_fused(a: &FusedItem, b: &FusedItem) -> std::cmp::Ordering {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.best_rank.cmp(&b.best_rank))
            .then(a._ord.cmp(&b._ord))
            .then(a.id.cmp(&b.id))
    }
    let k = top_k.min(fused.len());
    {
        // xts_top_k_by 就地将前 k 个重排为按 cmp_fused 的严格全序并返回前段切片。
        let head = crate::util::xts_top_k_by(&mut fused, k, cmp_fused);
        for f in head.iter_mut() {
            f._ord = (0, 0); // 输出不携带内部排序元数据
        }
    }
    fused.truncate(k);
    fused
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)] // 测试码 unwrap 为惯用法；E-5 意在治理生产码
mod tests {
    use super::*;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn cross_list_agreement_wins() {
        // doc-2 双路命中(dense rank2, sparse rank1)应排最前 —— 与 repo-wiki 示例同构
        let dense = ids(&["doc-1", "doc-2"]);
        let sparse = ids(&["doc-2", "doc-3"]);
        let lists = [WeightedList::new(&dense), WeightedList::new(&sparse)];
        let out = fuse_ranked_lists(&lists, DEFAULT_RRF_K, 3);
        assert_eq!(out[0].id, "doc-2");
        assert_eq!(out[0].list_hits, 2);
        assert_eq!(out[0].best_rank, 1);
    }

    #[test]
    fn weight_breaks_single_rank_tie() {
        // 同为 rank1：权重高者胜
        let a = ids(&["x"]);
        let b = ids(&["y"]);
        let lists = [
            WeightedList::new(&a).with_weight(2.0),
            WeightedList::new(&b).with_weight(1.0),
        ];
        let out = fuse_ranked_lists(&lists, DEFAULT_RRF_K, 2);
        assert_eq!(out[0].id, "x");
    }

    #[test]
    fn determinism_across_runs_and_input_shuffle_meta_stable() {
        let a = ids(&["m", "n", "o"]);
        let b = ids(&["n", "o", "p"]);
        let lists = [WeightedList::new(&a), WeightedList::new(&b)];
        let x = fuse_ranked_lists(&lists, DEFAULT_RRF_K, 5);
        let y = fuse_ranked_lists(&lists, DEFAULT_RRF_K, 5);
        assert_eq!(x, y, "同输入两次融合必须逐位一致（无哈希迭代序泄漏）");
    }

    #[test]
    fn duplicate_id_within_list_counted_once_at_first_rank() {
        let dup = ids(&["d", "e", "d"]);
        let lists = [WeightedList::new(&dup)];
        let out = fuse_ranked_lists(&lists, DEFAULT_RRF_K, 2);
        let d = out.iter().find(|f| f.id == "d").unwrap();
        assert_eq!(d.best_rank, 1, "重复 id 按首 rank 计");
        assert_eq!(d.list_hits, 1, "同列表重复不累加 hits");
    }

    #[test]
    fn zero_top_k_and_nonpositive_weight_short_circuit() {
        let a = ids(&["x"]);
        assert!(fuse_ranked_lists(&[WeightedList::new(&a)], DEFAULT_RRF_K, 0).is_empty());
        let lists = [WeightedList::new(&a).with_weight(0.0)];
        assert!(fuse_ranked_lists(&lists, DEFAULT_RRF_K, 3).is_empty());
    }

    #[test]
    fn top_k_selects_largest_k_by_partial_sort() {
        // 红线 #8：top_k 用 select_nth_unstable 部分选择，结果与“全排后截断”逐位一致
        let a = ids(&["a1", "a2", "a3", "a4"]);
        let lists = [WeightedList::new(&a)];
        let out = fuse_ranked_lists(&lists, DEFAULT_RRF_K, 2);
        assert_eq!(out.len(), 2);
        assert_eq!(out[0].id, "a1");
        assert_eq!(out[1].id, "a2");
    }
}
