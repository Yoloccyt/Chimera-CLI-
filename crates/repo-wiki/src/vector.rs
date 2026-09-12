//! 向量检索层 — 内存 KNN 检索(降级实现) + HNSW 生产路径
//!
//! 对应架构层:L5 Knowledge
//!
//! # 模块组成
//! - `vector`(本文件): `VectorIndex` — 内存 KNN(降级实现,≤1000 entry)
//! - `vector::hnsw_store`: `HnswStore` — HNSW 生产路径(10K-100K entry)
//! - `vector::memory_knn_store`: `MemoryKnnStore` — `VectorIndex` 的 `VectorStore` trait 适配器(P2-W8.2)
//!
//! `MemoryKnnStore` 内部组合 `VectorIndex`,通过 `VectorStore` trait 提供统一接口,
//! 可与 `HnswStore` 互换。`VectorIndex` 保留固有 API 向后兼容。
//!
//! # 降级说明(WHY)
//! 原计划使用 `sqlite-vec` 扩展提供 SQLite 原生向量检索,但:
//! 1. `sqlite-vec 0.1.9` 的 Rust binding 仅暴露 C 入口 `sqlite3_vec_init`
//! 2. 注册扩展需调用 `rusqlite::ffi::sqlite3_auto_extension` + `unsafe` 代码
//! 3. 项目铁律 `#![forbid(unsafe_code)]` 禁止任何 unsafe 块
//! 4. 因此触发任务预设的降级分支:内存向量检索
//!
//! # 性能特征
//! - `VectorIndex`(本文件): 10 条目 < 1ms,1000 条目 < 10ms
//! - `HnswStore`(子模块): 10K 条目 p95 < 50ms,100K 条目不 OOM
//!
//! # 后续演进
//! Week 6 NMC 编码器实现后,本层可替换为基于 `nexus_core::CLV` 的
//! 专用向量索引(如 HNSW),同时保持 API 不变。

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use crate::error::WikiError;

/// HNSW 生产路径实现(P2-W8.1)
pub mod hnsw_store;

/// 内存 KNN fallback 路径实现(P2-W8.2,VectorStore trait 适配器)
pub mod memory_knn_store;

/// P1-T14（WI-34）:批量 KNN 检索的 ComputeBridge 并行注入（env 开关 + 路由核心）
pub mod parallel;

/// 向量索引 — 内存 KNN 检索(降级实现)
///
/// 使用 `RwLock<HashMap<String, Vec<f32>>>` 存储向量,
/// `search`/`len` 持读锁(可并发),`upsert`/`delete` 持写锁(互斥)。
///
/// WHY RwLock 而非 Mutex:B1 优化,search 是高频读操作(KNN 遍历),
/// RwLock 允许多个并发 search 同时执行,仅在写入时互斥。
///
/// # 向后兼容
/// `VectorIndex` 保留固有 API(`new`/`upsert`/`search`/`delete`/`len`/`is_empty`),
/// 新代码应优先使用 `MemoryKnnStore`(通过 `VectorStore` trait 提供统一接口)。
pub struct VectorIndex {
    /// 向量维度(应与 WikiConfig.vector_dim 一致)
    dim: usize,
    /// 内存向量存储(entry_id → embedding)
    vectors: RwLock<HashMap<String, Vec<f32>>>,
    /// P1-T14: 批量检索并行开关（WI-34 注入配置回退）
    ///
    /// 默认 true;置 false 强制 `search_batch` 走串行（env `CHIMERA_NO_PARALLEL_WIKI`
    /// 为二次关闭开关,见 [`crate::vector::parallel`]）。仅作用于 `search_batch`
    /// 批量路径,单查询 `search` 行为零影响。
    parallel_search: bool,
}

impl VectorIndex {
    /// 创建指定维度的空向量索引
    pub fn new(dim: usize) -> Self {
        Self {
            dim,
            vectors: RwLock::new(HashMap::new()),
            parallel_search: true,
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

    /// 批量 KNN 检索 — 对 `queries` 中每个查询向量返回 Top-K 结果
    ///
    /// 返回 `Vec<Vec<(entry_id, similarity_score)>>`,**结果序 = 输入 query 序**
    /// （查询 i 的结果对应输入 i）。核心计算经
    /// [`crate::vector::parallel::knn_core`](crate::vector::parallel::knn_core)
    /// 路由（`TaskKind::KnnSearch`,阈值 5,000,`n_items = q × v` 总相似度计算数）
    /// → 并行或串行。
    ///
    /// # 语义（快照语义,确定性,Ω₂）
    /// - 全部查询基于**调用时刻一次性快照**的向量集计算（RwLock 读锁在快照窗口内
    ///   短暂持有,不跨闭包边界;快照后按 id 排序,消除 HashMap 迭代序对
    ///   分数 tie 相对顺序的影响 → 并行与串行逐 query 逐位一致）;
    /// - 查询间完全独立（互不依赖）→ 可安全并行;
    /// - `top_k` 语义与单查询 `search` 一致（select_nth_unstable + 稳定降序排序）。
    ///
    /// # 原子性
    /// 任一 query 维度不匹配 → 返回 [`WikiError::VectorIndexError`],
    /// **零计算零检索**（维度预校验,不产生部分结果）。
    ///
    /// # 回退
    /// 配置开关 `parallel_search`（默认 true,`with_parallel_search(false)` 关闭）
    ///   加 env `CHIMERA_NO_PARALLEL_WIKI`（OnceLock 启动期一次读取）双重关闭
    ///   强制串行;并行失败（理论不可达:闭包纯计算）自动回退串行。
    ///
    /// # 示例
    /// ```
    /// use repo_wiki::vector::VectorIndex;
    ///
    /// let idx = VectorIndex::new(4);
    /// idx.upsert("a", &[1.0, 0.0, 0.0, 0.0]).unwrap();
    /// idx.upsert("b", &[0.0, 1.0, 0.0, 0.0]).unwrap();
    ///
    /// let results = idx.search_batch(&[
    ///     vec![1.0, 0.0, 0.0, 0.0],
    ///     vec![0.0, 1.0, 0.0, 0.0],
    /// ], 1).unwrap();
    /// assert_eq!(results.len(), 2);
    /// assert_eq!(results[0][0].0, "a", "查询 0 的 top-1 应是 a");
    /// assert_eq!(results[1][0].0, "b", "查询 1 的 top-1 应是 b");
    /// ```
    pub fn search_batch(
        &self,
        queries: &[Vec<f32>],
        top_k: usize,
    ) -> Result<Vec<Vec<(String, f32)>>, WikiError> {
        let q = queries.len();
        if q == 0 {
            return Ok(Vec::new());
        }

        // 维度预校验（原子性:任一不匹配 → 零计算零检索）
        for query in queries {
            if query.len() != self.dim {
                return Err(WikiError::VectorIndexError(format!(
                    "query dimension mismatch: expected {}, got {}",
                    self.dim,
                    query.len()
                )));
            }
        }

        // ① 快照分离:主线程读锁一次(短暂持锁,不跨闭包边界);按 id 排序保证
        //    分数 tie 的相对顺序确定（HashMap 迭代序不确定,Ω₂ 确定性前提）
        let snapshot: Vec<(String, Vec<f32>)> = {
            let vectors = self
                .vectors
                .read()
                .map_err(|e| WikiError::VectorIndexError(format!("rwlock poisoned: {e}")))?;
            let mut v: Vec<(String, Vec<f32>)> = vectors
                .iter()
                .map(|(id, vec)| (id.clone(), vec.clone()))
                .collect();
            v.sort_by(|a, b| a.0.cmp(&b.0));
            v
        };

        // ② 计算:ComputeBridge 路由(并行/串行),结果序 = 输入 query 序
        let queries_arc = Arc::new(queries.to_vec());
        let vectors_arc = Arc::new(snapshot);
        Ok(parallel::knn_core(
            &queries_arc,
            &vectors_arc,
            top_k,
            self.parallel_search,
        ))
    }

    /// 设置批量检索并行开关（builder 风格,默认 true）
    ///
    /// 置 false 强制 `search_batch` 走串行路径（回退语义,与 env
    /// `CHIMERA_NO_PARALLEL_WIKI` 双重关闭;见 [`crate::vector::parallel`]）。
    /// 仅作用于批量路径,单查询 `search` 行为零影响。
    #[must_use]
    pub fn with_parallel_search(mut self, flag: bool) -> Self {
        self.parallel_search = flag;
        self
    }

    /// 查询批量检索并行开关（默认 true）
    #[must_use]
    pub fn parallel_search(&self) -> bool {
        self.parallel_search
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
