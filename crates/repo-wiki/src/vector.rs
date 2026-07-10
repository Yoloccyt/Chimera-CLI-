//! 向量检索层 — 内存 KNN 检索(降级实现)
//!
//! 对应架构层:L5 Knowledge
//!
//! # 降级说明(WHY)
//! 原计划使用 `sqlite-vec` 扩展提供 SQLite 原生向量检索,但:
//! 1. `sqlite-vec 0.1.9` 的 Rust binding 仅暴露 C 入口 `sqlite3_vec_init`
//! 2. 注册扩展需调用 `rusqlite::ffi::sqlite3_auto_extension` + `unsafe` 代码
//! 3. 项目铁律 `#![forbid(unsafe_code)]` 禁止任何 unsafe 块
//! 4. 因此触发任务预设的降级分支:内存向量检索
//!
//! # 性能特征
//! - 10 条目规模:KNN 检索 < 1ms(远低于 50ms 要求)
//! - 1000 条目规模:KNN 检索 < 10ms(可接受)
//! - 10000+ 条目规模:应迁移至 sqlite-vec 或专用向量数据库
//!
//! # 后续演进
//! Week 6 NMC 编码器实现后,本层可替换为基于 `nexus_core::CLV` 的
//! 专用向量索引(如 HNSW),同时保持 API 不变。

use std::collections::HashMap;
use std::sync::RwLock;

use crate::error::WikiError;

/// 向量索引 — 内存 KNN 检索(降级实现)
///
/// 使用 `RwLock<HashMap<String, Vec<f32>>>` 存储向量,
/// `search`/`len` 持读锁(可并发),`upsert`/`delete` 持写锁(互斥)。
///
/// WHY RwLock 而非 Mutex:B1 优化,search 是高频读操作(KNN 遍历),
/// RwLock 允许多个并发 search 同时执行,仅在写入时互斥。
pub struct VectorIndex {
    /// 向量维度(应与 WikiConfig.vector_dim 一致)
    dim: usize,
    /// 内存向量存储(entry_id → embedding)
    vectors: RwLock<HashMap<String, Vec<f32>>>,
}

impl VectorIndex {
    /// 创建指定维度的空向量索引
    pub fn new(dim: usize) -> Self {
        Self {
            dim,
            vectors: RwLock::new(HashMap::new()),
        }
    }

    /// 返回配置的向量维度
    pub fn dimension(&self) -> usize {
        self.dim
    }

    /// 插入或更新向量(UPSERT 语义)
    ///
    /// 若 `entry_id` 已存在,覆盖旧向量。
    /// 维度不匹配时返回 `VectorIndexError`。
    pub fn upsert(&self, entry_id: &str, embedding: &[f32]) -> Result<(), WikiError> {
        if embedding.len() != self.dim {
            return Err(WikiError::VectorIndexError(format!(
                "embedding dimension mismatch: expected {}, got {}",
                self.dim,
                embedding.len()
            )));
        }

        let mut vectors = self
            .vectors
            .write()
            .map_err(|e| WikiError::VectorIndexError(format!("rwlock poisoned: {e}")))?;
        vectors.insert(entry_id.to_string(), embedding.to_vec());
        Ok(())
    }

    /// KNN 检索 — 返回与查询向量最相似的 Top-K 条目
    ///
    /// 返回 `(entry_id, similarity_score)` 列表,按相似度降序排列。
    /// 相似度 ∈ [0.0, 1.0](余弦相似度,1.0 表示完全相同)。
    ///
    /// # 性能
    /// O(n) 遍历 + O(n) Top-K 选择(`select_nth_unstable_by`)+ O(K log K) 局部排序,
    /// n 为索引中的向量总数。在 10-1000 条目规模下延迟 < 10ms。
    pub fn search(&self, query: &[f32], top_k: usize) -> Result<Vec<(String, f32)>, WikiError> {
        if query.len() != self.dim {
            return Err(WikiError::VectorIndexError(format!(
                "query dimension mismatch: expected {}, got {}",
                self.dim,
                query.len()
            )));
        }

        let vectors = self
            .vectors
            .read()
            .map_err(|e| WikiError::VectorIndexError(format!("rwlock poisoned: {e}")))?;

        // 计算所有向量的余弦相似度
        // SubTask 21.4:使用 nexus_core 统一的 cosine_similarity_slices
        let mut scored: Vec<(String, f32)> = vectors
            .iter()
            .map(|(id, vec)| (id.clone(), nexus_core::cosine_similarity_slices(query, vec)))
            .collect();

        // Top-K 选择用 select_nth_unstable_by (O(n)),仅对前 K 做 K-log-K 排序
        // WHY 不用 sort_by:工程约定 Top-K 必须用 select_nth_unstable(O(n)) 替代 O(n log n)
        if top_k < scored.len() {
            scored.select_nth_unstable_by(top_k, |a, b| {
                b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        scored.truncate(top_k);
        // 前 K 元素已是无序的 Top-K 集合,这里做最终降序排序(K log K)
        scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        Ok(scored)
    }

    /// 删除向量
    ///
    /// 若 `entry_id` 不存在,返回 `Ok(())`(幂等)。
    pub fn delete(&self, entry_id: &str) -> Result<(), WikiError> {
        let mut vectors = self
            .vectors
            .write()
            .map_err(|e| WikiError::VectorIndexError(format!("rwlock poisoned: {e}")))?;
        vectors.remove(entry_id);
        Ok(())
    }

    /// 返回索引中的向量总数
    pub fn len(&self) -> Result<usize, WikiError> {
        let vectors = self
            .vectors
            .read()
            .map_err(|e| WikiError::VectorIndexError(format!("rwlock poisoned: {e}")))?;
        Ok(vectors.len())
    }

    /// 返回索引是否为空
    pub fn is_empty(&self) -> Result<bool, WikiError> {
        Ok(self.len()? == 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_upsert_and_len() {
        let idx = VectorIndex::new(4);
        assert!(idx.is_empty().unwrap());

        idx.upsert("a", &[1.0, 0.0, 0.0, 0.0]).unwrap();
        idx.upsert("b", &[0.0, 1.0, 0.0, 0.0]).unwrap();
        assert_eq!(idx.len().unwrap(), 2);
        assert!(!idx.is_empty().unwrap());
    }

    #[test]
    fn test_upsert_dimension_mismatch() {
        let idx = VectorIndex::new(4);
        let result = idx.upsert("a", &[1.0, 0.0, 0.0]);
        assert!(matches!(result, Err(WikiError::VectorIndexError(_))));
    }

    #[test]
    fn test_search_identical_vector() {
        let idx = VectorIndex::new(4);
        idx.upsert("a", &[1.0, 0.0, 0.0, 0.0]).unwrap();
        idx.upsert("b", &[0.0, 1.0, 0.0, 0.0]).unwrap();

        let results = idx.search(&[1.0, 0.0, 0.0, 0.0], 2).unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].0, "a");
        // 相同向量余弦相似度应接近 1.0
        assert!((results[0].1 - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_search_top_k() {
        let idx = VectorIndex::new(2);
        idx.upsert("a", &[1.0, 0.0]).unwrap();
        idx.upsert("b", &[0.9, 0.1]).unwrap();
        idx.upsert("c", &[0.0, 1.0]).unwrap();

        let results = idx.search(&[1.0, 0.0], 2).unwrap();
        assert_eq!(results.len(), 2);
        // 最相似的应是 "a"(完全相同),其次是 "b"(0.9, 0.1)
        assert_eq!(results[0].0, "a");
        assert_eq!(results[1].0, "b");
    }

    #[test]
    fn test_search_query_dimension_mismatch() {
        let idx = VectorIndex::new(4);
        idx.upsert("a", &[1.0, 0.0, 0.0, 0.0]).unwrap();
        let result = idx.search(&[1.0, 0.0, 0.0], 1);
        assert!(matches!(result, Err(WikiError::VectorIndexError(_))));
    }

    #[test]
    fn test_delete() {
        let idx = VectorIndex::new(4);
        idx.upsert("a", &[1.0, 0.0, 0.0, 0.0]).unwrap();
        assert_eq!(idx.len().unwrap(), 1);

        idx.delete("a").unwrap();
        assert_eq!(idx.len().unwrap(), 0);

        // 删除不存在的条目应幂等返回 Ok
        idx.delete("nonexistent").unwrap();
    }

    #[test]
    fn test_delete_removes_from_search() {
        let idx = VectorIndex::new(4);
        idx.upsert("a", &[1.0, 0.0, 0.0, 0.0]).unwrap();
        idx.upsert("b", &[0.0, 1.0, 0.0, 0.0]).unwrap();

        idx.delete("a").unwrap();

        let results = idx.search(&[1.0, 0.0, 0.0, 0.0], 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "b");
    }

    #[test]
    fn test_cosine_similarity_zero_vector() {
        // SubTask 21.4:使用 nexus_core 统一的 cosine_similarity_slices
        // 零向量与任意向量:返回 0.0(非 NaN)
        assert_eq!(
            nexus_core::cosine_similarity_slices(&[0.0; 4], &[1.0, 0.0, 0.0, 0.0]),
            0.0
        );
        assert_eq!(
            nexus_core::cosine_similarity_slices(&[0.0; 4], &[0.0; 4]),
            0.0
        );
    }

    #[test]
    fn test_cosine_similarity_orthogonal() {
        // 正交向量:相似度为 0
        let sim = nexus_core::cosine_similarity_slices(&[1.0, 0.0], &[0.0, 1.0]);
        assert!(sim.abs() < 1e-6);
    }

    #[test]
    fn test_cosine_similarity_identical() {
        let sim =
            nexus_core::cosine_similarity_slices(&[0.5, 0.5, 0.5, 0.5], &[0.5, 0.5, 0.5, 0.5]);
        assert!((sim - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_upsert_overwrites() {
        let idx = VectorIndex::new(4);
        idx.upsert("a", &[1.0, 0.0, 0.0, 0.0]).unwrap();
        idx.upsert("a", &[0.0, 1.0, 0.0, 0.0]).unwrap();

        assert_eq!(idx.len().unwrap(), 1);

        let results = idx.search(&[0.0, 1.0, 0.0, 0.0], 1).unwrap();
        assert_eq!(results[0].0, "a");
        assert!((results[0].1 - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_search_empty_index() {
        let idx = VectorIndex::new(4);
        let results = idx.search(&[1.0, 0.0, 0.0, 0.0], 5).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_search_top_k_larger_than_size() {
        let idx = VectorIndex::new(4);
        idx.upsert("a", &[1.0, 0.0, 0.0, 0.0]).unwrap();

        let results = idx.search(&[1.0, 0.0, 0.0, 0.0], 10).unwrap();
        assert_eq!(results.len(), 1);
    }
}
