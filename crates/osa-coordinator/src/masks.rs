//! 稀疏掩码容器 — 五维度稀疏化的统一数据结构
//!
//! 对应架构层:L6 Router
//!
//! # 设计决策(WHY)
//! - **泛型 T: Clone + PartialEq + Eq + Hash**:支持 ToolId/FileId/MemoryId/OperationId/TaskId 五种 ID 类型,
//!   Eq + Hash 约束使 `is_active` 可用 HashSet 实现 O(1) 查找(SubTask 13.9)
//! - **active_ids: `Vec<T>`**:保留活跃 ID 列表(序列化字段),稀疏化后仅包含被选中的项
//! - **active_set: `HashSet<T>`**:`active_ids` 的 HashSet 镜像(`#[serde(skip)]`),`is_active` O(1) 查找
//! - **sparsity_ratio: f32**:稀疏度 [0.0, 1.0],1.0 表示全稀疏(无活跃项),
//!   0.0 表示无稀疏(全部活跃)
//! - **派生 Serialize + 手动 Deserialize**:用于 mask_hash 计算(JSON 序列化后取 SHA-256),
//!   反序列化时从 `active_ids` 重建 `active_set`,保持向后兼容(旧序列化数据无 `active_set` 字段)
//!
//! # 使用场景
//! - `OmniSparseCoordinator::compute_all_masks` 返回 `OmniSparseMasks`,
//!   包含五个 `SparseMask<T>` 实例(routing/context/memory/audit/budget)
//! - HCW 订阅 `OmniSparseMasksComputed` 事件后,根据 context_mask 加载活跃文件

use std::collections::HashSet;
use std::hash::Hash;

use serde::{Deserialize, Serialize};

/// 稀疏掩码 — 泛型容器,存储活跃 ID 列表与稀疏度
///
/// 泛型约束 `T: Clone + PartialEq + Eq + Hash`,支持五种 ID 类型复用同一容器。
/// 稀疏度 `sparsity_ratio` ∈ [0.0, 1.0]:
/// - `0.0`:无稀疏,全部活跃(全掩码)
/// - `1.0`:全稀疏,无活跃项(空掩码)
///
/// # 性能优化(SubTask 13.9)
/// `active_set: HashSet<T>` 镜像 `active_ids`,`is_active` 用 `HashSet::contains` 实现 O(1) 查找。
/// 序列化时只存 `active_ids`(向后兼容),反序列化时重建 `active_set`。
///
/// # 示例
/// ```
/// use osa_coordinator::SparseMask;
///
/// let ids = vec!["tool-1".to_string(), "tool-2".to_string()];
/// let scores = vec![0.9, 0.1];
/// let mask = SparseMask::select_top_k(&ids, &scores, 1);
/// assert!(mask.is_active(&"tool-1".to_string()));
/// assert!(!mask.is_active(&"tool-2".to_string()));
/// ```
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SparseMask<T: Clone + PartialEq + Eq + Hash> {
    /// 活跃 ID 列表(稀疏化后保留的项)— 序列化字段
    pub active_ids: Vec<T>,
    /// 稀疏度 [0.0, 1.0],1.0 表示全稀疏(无活跃项)
    pub sparsity_ratio: f32,
    /// `active_ids` 的 HashSet 镜像 — `is_active` O(1) 查找
    ///
    /// WHY:SubTask 13.9 — 原 `is_active` 遍历 Vec O(n),改用 HashSet O(1)。
    /// `#[serde(skip)]` 不参与序列化,反序列化时从 `active_ids` 重建(见下方手动 Deserialize)
    #[serde(skip)]
    pub active_set: HashSet<T>,
}

/// 手动实现 Deserialize:反序列化后从 `active_ids` 重建 `active_set`
///
/// WHY:`active_set` 是 `#[serde(skip)]` 的,派生 Deserialize 会留空 HashSet,
/// 导致 `is_active` 始终返回 false。手动实现确保反序列化后 `active_set` 与 `active_ids` 一致,
/// 同时保持向后兼容(旧序列化数据无 `active_set` 字段)
impl<'de, T: Deserialize<'de> + Clone + PartialEq + Eq + Hash> Deserialize<'de> for SparseMask<T> {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct Helper<T> {
            active_ids: Vec<T>,
            sparsity_ratio: f32,
        }

        let helper = Helper::deserialize(deserializer)?;
        let active_set = HashSet::from_iter(helper.active_ids.iter().cloned());
        Ok(Self {
            active_ids: helper.active_ids,
            active_set,
            sparsity_ratio: helper.sparsity_ratio,
        })
    }
}

impl<T: Clone + PartialEq + Eq + Hash> SparseMask<T> {
    /// 创建空掩码(全稀疏,无活跃项)
    ///
    /// WHY:空掩码表示该维度被完全稀疏化,不加载任何资源。
    /// 用于极端稀疏场景(如超简单任务的 audit 维度可完全跳过)
    pub fn empty() -> Self {
        Self {
            active_ids: Vec::new(),
            sparsity_ratio: 1.0,
            active_set: HashSet::new(),
        }
    }

    /// 创建全掩码(无稀疏,全部活跃)
    ///
    /// WHY:全掩码表示该维度不稀疏化,加载所有资源。
    /// 用于极端复杂场景(如超复杂任务的 audit 维度需全审计)
    pub fn full(ids: Vec<T>) -> Self {
        let total = ids.len();
        let sparsity = if total == 0 { 1.0 } else { 0.0 };
        let active_set = HashSet::from_iter(ids.iter().cloned());
        Self {
            active_ids: ids,
            sparsity_ratio: sparsity,
            active_set,
        }
    }

    /// 从候选集中按评分选取 Top-K 作为活跃项(Top-K 稀疏化)
    ///
    /// WHY:SubTask 13.10 — 旧签名 `select_top_k(ids: Vec<T>, k: usize)` 取前 K 个(无评分),
    /// 且 `ids: Vec<T>` 被 move 后无法复用。新签名改用 `&[T]` 借用,并按 `scores` 降序选 Top-K。
    ///
    /// # 参数
    /// - `ids`:候选 ID 列表(借用,调用方可复用)
    /// - `scores`:与 `ids` 一一对应的评分(降序选 Top-K)
    /// - `k`:保留的活跃项数量
    ///
    /// # 稀疏度计算
    /// - 若 `ids` 为空或 `k == 0`:返回空掩码(sparsity = 1.0)
    /// - 若 `k >= ids.len()`:返回全掩码(sparsity = 0.0)
    /// - 若 `scores.len() != ids.len()`:返回空掩码(调用方错误,防御性处理)
    /// - 否则:sparsity = 1.0 - (k / ids.len())
    ///
    /// # 实现细节
    /// 用 `select_nth_unstable_by` 选前 K 个(O(n) 平均复杂度),再对 Top-K 按评分降序排序,
    /// 确保 `active_ids` 顺序确定(相同输入产生相同输出,保证 `mask_hash` 一致性)
    pub fn select_top_k(ids: &[T], scores: &[f32], k: usize) -> Self {
        let total = ids.len();
        if total == 0 || k == 0 {
            return Self::empty();
        }
        if k >= total {
            return Self::full(ids.to_vec());
        }
        // 防御性校验:scores 长度必须与 ids 一致
        if scores.len() != total {
            return Self::empty();
        }

        // 创建 (idx, score) 对,用 select_nth_unstable_by 选 Top-K
        let mut indexed_scores: Vec<(usize, f32)> =
            scores.iter().enumerate().map(|(i, &s)| (i, s)).collect();

        // 按评分降序选前 K 个:select_nth_unstable_by 将第 K-1 大的元素放到位置 K-1,
        // 前 K-1 个元素(left)均 >= pivot,后 N-K 个元素(right)均 <= pivot
        // WHY 用 partial_cmp + unwrap_or(Equal):scores 可能含 NaN,降级为相等避免 panic
        let (left, pivot, _) = indexed_scores.select_nth_unstable_by(k - 1, |a, b| {
            b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
        });

        // 收集 Top-K 的索引(left + pivot),按评分降序排序
        // WHY:select_nth_unstable_by 只保证分区,不保证 left 内部顺序,
        // 需再排序确保 active_ids 顺序确定(mask_hash 一致性)
        let mut top_k_indices: Vec<usize> = left.iter().map(|(i, _)| *i).collect();
        top_k_indices.push(pivot.0);
        top_k_indices.sort_by(|&a, &b| {
            scores[b]
                .partial_cmp(&scores[a])
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // 收集对应的 ID(按评分降序)
        let active_ids: Vec<T> = top_k_indices.iter().map(|&i| ids[i].clone()).collect();
        let sparsity = 1.0 - (k as f32 / total as f32);
        // WHY 双重存储(Vec + HashSet,m-05 评估):active_ids 提供有序遍历(O(K)),
        // active_set 提供 O(1) 包含性检查(contains)。二者互补,避免热路径排序开销。
        // 此处 `iter().cloned()` 是必要的二次 clone — HashSet 需要拥有所有权,
        // 而 active_ids 仍需保留供 Self.active_ids 字段使用。
        // 若 T 实现 Copy(trivial clone),开销可忽略;否则 K 通常 ≤ 64,clone 代价可接受。
        let active_set = HashSet::from_iter(active_ids.iter().cloned());
        Self {
            active_ids,
            active_set,
            sparsity_ratio: sparsity,
        }
    }

    /// 检查指定 ID 是否活跃(在 active_set 中)
    ///
    /// WHY:SubTask 13.9 — 用 `HashSet::contains` 实现 O(1) 查找,
    /// 原 `Vec::iter().any()` 为 O(n),1000 个 ID 时从 ~1μs 降到 ~50ns
    pub fn is_active(&self, id: &T) -> bool {
        self.active_set.contains(id)
    }

    /// 返回活跃项数量
    pub fn active_count(&self) -> usize {
        self.active_ids.len()
    }

    /// 返回稀疏度 [0.0, 1.0]
    pub fn sparsity(&self) -> f32 {
        self.sparsity_ratio
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_mask() {
        let mask: SparseMask<String> = SparseMask::empty();
        assert!(mask.active_ids.is_empty());
        assert_eq!(mask.sparsity_ratio, 1.0);
        assert!(!mask.is_active(&"any".to_string()));
        assert_eq!(mask.active_count(), 0);
        // active_set 应与 active_ids 一致(均为空)
        assert!(mask.active_set.is_empty());
    }

    #[test]
    fn test_full_mask() {
        let ids = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        let mask = SparseMask::full(ids.clone());
        assert_eq!(mask.active_ids, ids);
        assert_eq!(mask.sparsity_ratio, 0.0);
        assert!(mask.is_active(&"a".to_string()));
        assert!(mask.is_active(&"c".to_string()));
        assert_eq!(mask.active_count(), 3);
        // active_set 应包含所有 ID
        assert_eq!(mask.active_set.len(), 3);
    }

    #[test]
    fn test_full_mask_empty_input() {
        let mask: SparseMask<String> = SparseMask::full(Vec::new());
        assert!(mask.active_ids.is_empty());
        assert_eq!(mask.sparsity_ratio, 1.0);
    }

    #[test]
    fn test_select_top_k_partial() {
        let ids = vec![
            "a".to_string(),
            "b".to_string(),
            "c".to_string(),
            "d".to_string(),
        ];
        let scores = vec![0.9, 0.8, 0.5, 0.1]; // 降序,前 2 个为 Top-K
        let mask = SparseMask::select_top_k(&ids, &scores, 2);
        assert_eq!(mask.active_ids, vec!["a".to_string(), "b".to_string()]);
        assert!((mask.sparsity_ratio - 0.5).abs() < 1e-6);
        assert!(mask.is_active(&"a".to_string()));
        assert!(mask.is_active(&"b".to_string()));
        assert!(!mask.is_active(&"c".to_string()));
        assert!(!mask.is_active(&"d".to_string()));
    }

    #[test]
    fn test_select_top_k_exceeds_length() {
        let ids = vec!["a".to_string(), "b".to_string()];
        let scores = vec![0.5, 0.5];
        let mask = SparseMask::select_top_k(&ids, &scores, 10);
        assert_eq!(mask.active_ids, ids);
        assert_eq!(mask.sparsity_ratio, 0.0);
    }

    #[test]
    fn test_select_top_k_empty_input() {
        let mask: SparseMask<String> = SparseMask::select_top_k(&[], &[], 5);
        assert!(mask.active_ids.is_empty());
        assert_eq!(mask.sparsity_ratio, 1.0);
    }

    #[test]
    fn test_select_top_k_zero_k() {
        let ids = vec!["a".to_string(), "b".to_string()];
        let scores = vec![0.5, 0.5];
        let mask = SparseMask::select_top_k(&ids, &scores, 0);
        assert!(mask.active_ids.is_empty());
        assert_eq!(mask.sparsity_ratio, 1.0);
    }

    #[test]
    fn test_select_top_k_scores_length_mismatch() {
        // scores 长度与 ids 不一致应返回空掩码(防御性处理)
        let ids = vec!["a".to_string(), "b".to_string()];
        let scores = vec![0.5]; // 长度不匹配
        let mask = SparseMask::select_top_k(&ids, &scores, 1);
        assert!(mask.active_ids.is_empty());
        assert_eq!(mask.sparsity_ratio, 1.0);
    }

    #[test]
    fn test_serde_roundtrip() {
        let ids = vec!["x".to_string(), "y".to_string()];
        let scores = vec![0.9, 0.1];
        let mask = SparseMask::select_top_k(&ids, &scores, 1);
        let json = serde_json::to_string(&mask).expect("序列化失败");
        let restored: SparseMask<String> = serde_json::from_str(&json).expect("反序列化失败");
        assert_eq!(mask, restored);
        // 反序列化后 active_set 应已重建,is_active 应正常工作
        assert!(restored.is_active(&"x".to_string()));
        assert!(!restored.is_active(&"y".to_string()));
    }
}
