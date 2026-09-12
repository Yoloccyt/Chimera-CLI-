//! 内存 KNN 向量存储实现 — Fallback 路径(≤1000 entry 规模)
//!
//! 对应架构层: L5 Knowledge
//! 对应 ADR: ADR-033(L0 nexus-contracts 契约层)
//! 对应任务: P2-W8.2(MemoryKnnStore fallback 实现)
//!
//! # 核心职责
//! 作为 `VectorIndex` 的 `VectorStore` trait 适配器,提供与 `HnswStore`
//! 统一的接口,使两者可通过 `VectorStore` trait 互换。
//!
//! # 设计模式 — 组合适配器(Adapter Pattern)
//! `MemoryKnnStore` 内部组合 `VectorIndex`,将 `VectorStore` trait 方法
//! 委托给 `VectorIndex` 的固有方法:
//! - `VectorStore::upsert` → `VectorIndex::upsert`(忽略 meta 参数)
//! - `VectorStore::top_k` → `VectorIndex::search`(转换返回类型)
//! - `VectorStore::remove` → `VectorIndex::delete`
//!
//! WHY 组合而非重实现:
//! 1. **避免 inherent/trait 方法名冲突**:Rust 方法解析规则下,inherent method
//!    优先级高于 trait method,且参数不匹配时不会 fallthrough 到 trait method。
//!    若 `MemoryKnnStore` 同时定义 inherent `upsert`(2 参)与 trait `upsert`(3 参),
//!    调用 `store.upsert("a", &vec, ())` 会匹配 inherent 版本并报参数数量错误。
//!    组合模式让 `MemoryKnnStore` 只暴露 trait 方法,避免此问题。
//! 2. **零代码重复**:核心逻辑由 `VectorIndex` 提供,`MemoryKnnStore` 仅做适配。
//! 3. **向后兼容**:`VectorIndex` 固有 API 不变,旧调用方零改动。
//!
//! # 适用场景
//! - 小规模向量集(≤1000 entry)
//! - 需要精确召回(100% recall)的场景
//! - HNSW 不可用时的 fallback 路径
//! - 测试与原型开发(无外部依赖)
//!
//! # 性能特征
//! 委托给 `VectorIndex`,性能特征一致:
//! - 插入: O(1) 平均(HashMap)
//! - 检索: O(n·d) + O(n) Top-K 选择 + O(K log K) 局部排序
//! - 删除: O(1) 平均(HashMap)
//! - 内存: n·d·4 bytes(向量) + HashMap 开销
//!
//! # 线程安全
//! 通过 `VectorIndex` 内部的 `RwLock<HashMap<String, Vec<f32>>>` 保证:
//! - `search`/`len` 持读锁(可并发)
//! - `upsert`/`delete` 持写锁(互斥)

use nexus_contracts::{VectorBackend, VectorHit, VectorStore, VectorStoreExt, VectorStoreStats};

use crate::error::WikiError;
use crate::vector::VectorIndex;

// ============================================================
// 常量定义
// ============================================================

/// 默认向量维度 — 与 CLV(Context Latent Vector)512-dim 对齐
const DEFAULT_DIM: usize = 512;

// ============================================================
// MemoryKnnStore 结构体
// ============================================================

/// 内存 KNN 向量存储 — Fallback 路径实现(VectorStore trait 适配器)
///
/// 组合 `VectorIndex`,通过 `VectorStore` + `VectorStoreExt` trait 提供统一接口,
/// 可与 `HnswStore`(生产路径)互换。
///
/// # 适用场景
/// - 小规模向量集(≤1000 entry)
/// - 需要精确召回(100% recall)的场景
/// - HNSW 不可用时的 fallback 路径
/// - 测试与原型开发(无外部依赖)
///
/// # 示例
/// ```
/// use nexus_contracts::{VectorStore, VectorStoreExt, VectorBackend};
/// use repo_wiki::vector::memory_knn_store::MemoryKnnStore;
///
/// let store = MemoryKnnStore::with_dim(3);
/// store.upsert("entry-1", &[1.0, 0.0, 0.0], ()).unwrap();
///
/// let results = store.top_k(&[1.0, 0.0, 0.0], 1, "").unwrap();
/// assert_eq!(results[0].id, "entry-1");
/// assert_eq!(store.backend(), VectorBackend::Memory);
/// ```
pub struct MemoryKnnStore {
    /// 内部委托的 VectorIndex 实例
    ///
    /// WHY 组合而非继承:Rust 无继承,组合是标准适配器模式。
    /// VectorIndex 提供 inherent API 向后兼容,MemoryKnnStore 提供 trait API。
    inner: VectorIndex,
}

// ============================================================
// 构造方法
// ============================================================

impl MemoryKnnStore {
    /// 创建指定维度的空内存向量存储
    ///
    /// # 参数
    /// - `dim`: 向量维度(应与 CLV 512-dim 一致)
    ///
    /// # 示例
    /// ```
    /// use repo_wiki::vector::memory_knn_store::MemoryKnnStore;
    ///
    /// let store = MemoryKnnStore::new(512);
    /// assert_eq!(store.dimension(), 512);
    /// ```
    pub fn new(dim: usize) -> Self {
        Self {
            inner: VectorIndex::new(dim),
        }
    }

    /// 创建指定维度的空内存向量存储(语义更明确的别名)
    ///
    /// 与 `new` 等价,提供与 `HnswStore::with_dim` 一致的命名风格。
    pub fn with_dim(dim: usize) -> Self {
        Self::new(dim)
    }

    /// 返回配置的向量维度
    pub fn dimension(&self) -> usize {
        self.inner.dimension()
    }

    /// 批量 KNN 检索 — 对 `queries` 中每个查询向量返回 Top-K 命中（P1-T14）
    ///
    /// 委托给 [`VectorIndex::search_batch`](crate::vector::VectorIndex::search_batch)
    /// （ComputeBridge 并行注入入口,`TaskKind::KnnSearch`）;返回
    /// `Vec<Vec<VectorHit>>`,**结果序 = 输入 query 序**（查询 i 的结果对应输入 i）。
    /// 并行开关继承 `inner` 的 `parallel_search`（默认 true,
    /// `with_parallel_search(false)` + env `CHIMERA_NO_PARALLEL_WIKI` 双重关闭）。
    ///
    /// # 示例
    /// ```
    /// use nexus_contracts::VectorStore;
    /// use repo_wiki::vector::memory_knn_store::MemoryKnnStore;
    ///
    /// let store = MemoryKnnStore::with_dim(3);
    /// store.upsert("a", &[1.0, 0.0, 0.0], ()).unwrap();
    /// store.upsert("b", &[0.0, 1.0, 0.0], ()).unwrap();
    ///
    /// let results = store.top_k_batch(&[vec![1.0, 0.0, 0.0], vec![0.0, 1.0, 0.0]], 1, "")
    ///     .unwrap();
    /// assert_eq!(results.len(), 2);
    /// assert_eq!(results[0][0].id, "a", "查询 0 的 top-1 应是 a");
    /// assert_eq!(results[1][0].id, "b", "查询 1 的 top-1 应是 b");
    /// ```
    pub fn top_k_batch(
        &self,
        queries: &[Vec<f32>],
        k: usize,
        _ns: &str,
    ) -> Result<Vec<Vec<VectorHit>>, WikiError> {
        Ok(self
            .inner
            .search_batch(queries, k)?
            .into_iter()
            .map(|hits| {
                hits.into_iter()
                    .map(|(id, score)| VectorHit::new(id, score))
                    .collect()
            })
            .collect())
    }
}

// ============================================================
// VectorStore trait 实现
// ============================================================

impl VectorStore for MemoryKnnStore {
    type Meta = ();
    type Error = WikiError;

    /// 插入或更新向量(UPSERT 语义)
    ///
    /// 委托给 `VectorIndex::upsert`,忽略 `meta` 参数(当前 Meta = ())。
    /// 维度不匹配时返回错误。
    fn upsert(&self, id: &str, vector: &[f32], _meta: Self::Meta) -> Result<(), Self::Error> {
        self.inner.upsert(id, vector)
    }

    /// KNN 检索 — 返回与查询向量最相似的 Top-K 条目
    ///
    /// 委托给 `VectorIndex::search`,将 `(String, f32)` 转换为 `VectorHit`。
    /// 结果按 score 降序排列。
    ///
    /// # 命名空间
    /// 当前为单租户实现,`ns` 参数接受但忽略。
    fn top_k(&self, query: &[f32], k: usize, _ns: &str) -> Result<Vec<VectorHit>, Self::Error> {
        let results = self.inner.search(query, k)?;
        Ok(results
            .into_iter()
            .map(|(id, score)| VectorHit::new(id, score))
            .collect())
    }

    /// 删除条目(幂等 — `id` 不存在返回 `Ok(())`)
    ///
    /// 委托给 `VectorIndex::delete`。
    fn remove(&self, id: &str) -> Result<(), Self::Error> {
        self.inner.delete(id)
    }

    /// 返回默认实现(fallback 构造路径)
    ///
    /// 创建 512-dim 的空内存向量存储。
    /// 注意:此方法返回 `Self`,要求 `Self: Sized`,
    /// `dyn VectorStore` trait 对象无法调用。
    fn default() -> Self {
        Self::new(DEFAULT_DIM)
    }
}

// ============================================================
// VectorStoreExt trait 实现
// ============================================================

impl VectorStoreExt for MemoryKnnStore {
    /// 批量插入向量(性能优化路径)
    ///
    /// 逐条调用 `VectorIndex::upsert`。
    /// 任一条目维度不匹配时,整个批次在插入前返回错误(原子性)。
    ///
    /// # 原子性
    /// 预校验所有维度后,逐条插入。
    /// 若任一条目维度不匹配,不插入任何条目。
    ///
    /// # 性能说明
    /// 当前实现逐条 upsert(每次获取/释放写锁)。
    /// 与 `HnswStore::insert_batch` 的批量构建相比性能较低,
    /// 但对于 ≤1000 entry 的 fallback 场景可接受。
    fn insert_batch(
        &self,
        entries: Vec<(String, Vec<f32>, Self::Meta)>,
    ) -> Result<(), Self::Error> {
        if entries.is_empty() {
            return Ok(());
        }

        // 预校验所有维度(原子性:要么全部插入,要么不插入)
        for (id, vector, _) in &entries {
            if vector.len() != self.dimension() {
                return Err(WikiError::VectorIndexError(format!(
                    "batch entry '{id}' dimension mismatch: expected {}, got {}",
                    self.dimension(),
                    vector.len()
                )));
            }
        }

        // 逐条 UPSERT(委托给 VectorIndex)
        for (id, vector, _) in entries {
            self.inner.upsert(&id, &vector)?;
        }
        Ok(())
    }

    /// 压缩索引(碎片整理)
    ///
    /// HashMap 无碎片问题,此方法为无操作,直接返回 `Ok(())`。
    /// 与 `HnswStore::compact` 接口对齐,确保调用方代码可互换。
    fn compact(&self) -> Result<(), Self::Error> {
        // HashMap 无碎片问题,无需操作
        Ok(())
    }

    /// 返回存储统计信息
    ///
    /// `memory_bytes` = entry_count × dim × sizeof(f32)(仅向量数据,不含 HashMap 开销)。
    /// 通过 `VectorIndex::len()` + `dimension()` 计算,无需访问 VectorIndex 内部字段。
    fn stats(&self) -> Result<VectorStoreStats, Self::Error> {
        let entry_count = self.inner.len()?;
        let dimension = self.dimension();
        let memory_bytes = (entry_count * dimension * std::mem::size_of::<f32>()) as u64;
        Ok(VectorStoreStats {
            entry_count,
            dimension,
            memory_bytes,
            backend: VectorBackend::Memory,
        })
    }

    /// 返回后端类型标识(运行时诊断)
    fn backend(&self) -> VectorBackend {
        VectorBackend::Memory
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // --- 辅助函数 ---

    /// 创建小维度(4-dim)MemoryKnnStore 用于快速测试
    fn make_store() -> MemoryKnnStore {
        MemoryKnnStore::new(4)
    }

    /// 生成单位向量(余弦相似度有意义)
    fn unit_vec(components: &[f32]) -> Vec<f32> {
        let norm: f32 = components.iter().map(|x| x * x).sum::<f32>().sqrt();
        if norm == 0.0 {
            return vec![0.0; components.len()];
        }
        components.iter().map(|x| x / norm).collect()
    }

    // ============================================================
    // 构造方法测试
    // ============================================================

    #[test]
    fn test_new_creates_empty_store() {
        let store = MemoryKnnStore::new(512);
        let stats = store.stats().unwrap();
        assert!(stats.is_empty());
        assert_eq!(stats.dimension, 512);
        assert_eq!(stats.backend, VectorBackend::Memory);
    }

    #[test]
    fn test_with_dim_alias_of_new() {
        let store = MemoryKnnStore::with_dim(128);
        assert_eq!(store.dimension(), 128);
    }

    #[test]
    fn test_default_returns_512_dim_store() {
        let store = MemoryKnnStore::default();
        let stats = store.stats().unwrap();
        assert_eq!(stats.dimension, 512);
        assert!(stats.is_empty());
    }

    // ============================================================
    // VectorStore::upsert 测试
    // ============================================================

    #[test]
    fn test_upsert_single_entry() {
        let store = make_store();
        store.upsert("a", &[1.0, 0.0, 0.0, 0.0], ()).unwrap();
        assert_eq!(store.stats().unwrap().entry_count, 1);
    }

    #[test]
    fn test_upsert_multiple_entries() {
        let store = make_store();
        store.upsert("a", &[1.0, 0.0, 0.0, 0.0], ()).unwrap();
        store.upsert("b", &[0.0, 1.0, 0.0, 0.0], ()).unwrap();
        store.upsert("c", &[0.0, 0.0, 1.0, 0.0], ()).unwrap();
        assert_eq!(store.stats().unwrap().entry_count, 3);
    }

    #[test]
    fn test_upsert_dimension_mismatch() {
        let store = make_store(); // 4-dim
        let result = store.upsert("a", &[1.0, 0.0, 0.0], ()); // 3-dim
        assert!(result.is_err());
        assert!(matches!(result, Err(WikiError::VectorIndexError(_))));
    }

    #[test]
    fn test_upsert_overwrites_existing() {
        let store = make_store();
        store.upsert("a", &[1.0, 0.0, 0.0, 0.0], ()).unwrap();
        store.upsert("a", &[0.0, 1.0, 0.0, 0.0], ()).unwrap();
        assert_eq!(store.stats().unwrap().entry_count, 1);

        let results = store.top_k(&[0.0, 1.0, 0.0, 0.0], 1, "").unwrap();
        assert_eq!(results[0].id, "a");
        assert!((results[0].score - 1.0).abs() < 1e-5);
    }

    // ============================================================
    // VectorStore::top_k 测试
    // ============================================================

    #[test]
    fn test_top_k_empty_store() {
        let store = make_store();
        let results = store.top_k(&[1.0, 0.0, 0.0, 0.0], 5, "").unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_top_k_returns_sorted_by_score_desc() {
        let store = make_store();
        store.upsert("a", &[1.0, 0.0, 0.0, 0.0], ()).unwrap();
        store.upsert("b", &[0.9, 0.1, 0.0, 0.0], ()).unwrap();
        store.upsert("c", &[0.0, 0.0, 0.0, 1.0], ()).unwrap();

        let results = store.top_k(&[1.0, 0.0, 0.0, 0.0], 2, "").unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, "a");
        assert!((results[0].score - 1.0).abs() < 1e-5);
        // 验证不变量:结果按 score 降序排列
        for i in 1..results.len() {
            assert!(results[i - 1].score >= results[i].score);
        }
    }

    #[test]
    fn test_top_k_k_larger_than_size() {
        let store = make_store();
        store.upsert("a", &[1.0, 0.0, 0.0, 0.0], ()).unwrap();

        let results = store.top_k(&[1.0, 0.0, 0.0, 0.0], 10, "").unwrap();
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_top_k_k_zero_returns_empty() {
        let store = make_store();
        store.upsert("a", &[1.0, 0.0, 0.0, 0.0], ()).unwrap();

        let results = store.top_k(&[1.0, 0.0, 0.0, 0.0], 0, "").unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_top_k_query_dimension_mismatch() {
        let store = make_store(); // 4-dim
        store.upsert("a", &[1.0, 0.0, 0.0, 0.0], ()).unwrap();
        let result = store.top_k(&[1.0, 0.0, 0.0], 1, ""); // 3-dim
        assert!(result.is_err());
    }

    #[test]
    fn test_top_k_identical_vector_high_score() {
        let store = make_store();
        store.upsert("a", &[1.0, 0.0, 0.0, 0.0], ()).unwrap();

        let results = store.top_k(&[1.0, 0.0, 0.0, 0.0], 1, "").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "a");
        assert!((results[0].score - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_top_k_namespace_param_accepted() {
        let store = make_store();
        store.upsert("a", &[1.0, 0.0, 0.0, 0.0], ()).unwrap();
        assert!(store.top_k(&[1.0, 0.0, 0.0, 0.0], 1, "agent-ns").is_ok());
        assert!(store.top_k(&[1.0, 0.0, 0.0, 0.0], 1, "").is_ok());
    }

    #[test]
    fn test_top_k_score_in_valid_range() {
        let store = make_store();
        let v1 = unit_vec(&[1.0, 0.0, 0.0, 0.0]);
        let v2 = unit_vec(&[0.0, 1.0, 0.0, 0.0]);
        let query = unit_vec(&[1.0, 1.0, 0.0, 0.0]);

        store.upsert("a", &v1, ()).unwrap();
        store.upsert("b", &v2, ()).unwrap();

        let results = store.top_k(&query, 2, "").unwrap();
        for hit in &results {
            assert!(
                hit.score >= 0.0 && hit.score <= 1.0,
                "score out of range: {}",
                hit.score
            );
        }
    }

    // ============================================================
    // VectorStore::remove 测试
    // ============================================================

    #[test]
    fn test_remove_idempotent() {
        let store = make_store();
        assert!(store.remove("nonexistent").is_ok());
    }

    #[test]
    fn test_remove_deletes_entry() {
        let store = make_store();
        store.upsert("a", &[1.0, 0.0, 0.0, 0.0], ()).unwrap();
        assert_eq!(store.stats().unwrap().entry_count, 1);

        store.remove("a").unwrap();
        assert_eq!(store.stats().unwrap().entry_count, 0);
    }

    #[test]
    fn test_remove_excludes_from_search() {
        let store = make_store();
        store.upsert("a", &[1.0, 0.0, 0.0, 0.0], ()).unwrap();
        store.upsert("b", &[0.0, 1.0, 0.0, 0.0], ()).unwrap();

        store.remove("a").unwrap();

        let results = store.top_k(&[1.0, 0.0, 0.0, 0.0], 10, "").unwrap();
        assert!(!results.iter().any(|h| h.id == "a"));
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "b");
    }

    #[test]
    fn test_remove_and_reinsert() {
        let store = make_store();
        store.upsert("a", &[1.0, 0.0, 0.0, 0.0], ()).unwrap();
        store.remove("a").unwrap();
        store.upsert("a", &[0.0, 1.0, 0.0, 0.0], ()).unwrap();
        assert_eq!(store.stats().unwrap().entry_count, 1);

        let results = store.top_k(&[0.0, 1.0, 0.0, 0.0], 1, "").unwrap();
        assert_eq!(results[0].id, "a");
    }

    // ============================================================
    // VectorStoreExt::insert_batch 测试
    // ============================================================

    #[test]
    fn test_insert_batch_inserts_all() {
        let store = make_store();
        let entries = vec![
            ("a".to_string(), vec![1.0, 0.0, 0.0, 0.0], ()),
            ("b".to_string(), vec![0.0, 1.0, 0.0, 0.0], ()),
            ("c".to_string(), vec![0.0, 0.0, 1.0, 0.0], ()),
        ];
        store.insert_batch(entries).unwrap();
        assert_eq!(store.stats().unwrap().entry_count, 3);
    }

    #[test]
    fn test_insert_batch_empty_is_ok() {
        let store = make_store();
        assert!(store.insert_batch(Vec::new()).is_ok());
    }

    #[test]
    fn test_insert_batch_dimension_mismatch() {
        let store = make_store(); // 4-dim
        let entries = vec![("a".to_string(), vec![1.0, 0.0], ())]; // 2-dim
        let result = store.insert_batch(entries);
        assert!(result.is_err());
        // 原子性:失败后 store 仍为空
        assert_eq!(store.stats().unwrap().entry_count, 0);
    }

    #[test]
    fn test_insert_batch_upsert_existing() {
        let store = make_store();
        store.upsert("a", &[1.0, 0.0, 0.0, 0.0], ()).unwrap();

        let entries = vec![
            ("a".to_string(), vec![0.0, 1.0, 0.0, 0.0], ()),
            ("b".to_string(), vec![0.0, 0.0, 1.0, 0.0], ()),
        ];
        store.insert_batch(entries).unwrap();
        assert_eq!(store.stats().unwrap().entry_count, 2);

        let results = store.top_k(&[0.0, 1.0, 0.0, 0.0], 1, "").unwrap();
        assert_eq!(results[0].id, "a");
    }

    // ============================================================
    // VectorStoreExt::compact 测试
    // ============================================================

    #[test]
    fn test_compact_empty_store() {
        let store = make_store();
        assert!(store.compact().is_ok());
        assert!(store.stats().unwrap().is_empty());
    }

    #[test]
    fn test_compact_preserves_entries() {
        let store = make_store();
        store.upsert("a", &[1.0, 0.0, 0.0, 0.0], ()).unwrap();
        store.upsert("b", &[0.0, 1.0, 0.0, 0.0], ()).unwrap();

        store.compact().unwrap();

        assert_eq!(store.stats().unwrap().entry_count, 2);
        let results = store.top_k(&[1.0, 0.0, 0.0, 0.0], 1, "").unwrap();
        assert_eq!(results[0].id, "a");
    }

    #[test]
    fn test_compact_after_remove() {
        let store = make_store();
        store.upsert("a", &[1.0, 0.0, 0.0, 0.0], ()).unwrap();
        store.upsert("b", &[0.0, 1.0, 0.0, 0.0], ()).unwrap();
        store.remove("a").unwrap();

        store.compact().unwrap();

        let results = store.top_k(&[1.0, 0.0, 0.0, 0.0], 10, "").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "b");
    }

    // ============================================================
    // VectorStoreExt::stats 测试
    // ============================================================

    #[test]
    fn test_stats_correct_count() {
        let store = make_store();
        store.upsert("a", &[1.0, 0.0, 0.0, 0.0], ()).unwrap();
        store.upsert("b", &[0.0, 1.0, 0.0, 0.0], ()).unwrap();

        let stats = store.stats().unwrap();
        assert_eq!(stats.entry_count, 2);
        assert_eq!(stats.dimension, 4);
        assert_eq!(stats.backend, VectorBackend::Memory);
        // 2 entries × 4 dims × 4 bytes/f32 = 32 bytes
        assert_eq!(stats.memory_bytes, 32);
    }

    #[test]
    fn test_stats_empty_store() {
        let store = make_store();
        let stats = store.stats().unwrap();
        assert!(stats.is_empty());
        assert_eq!(stats.memory_bytes, 0);
    }

    #[test]
    fn test_stats_after_remove() {
        let store = make_store();
        store.upsert("a", &[1.0, 0.0, 0.0, 0.0], ()).unwrap();
        store.upsert("b", &[0.0, 1.0, 0.0, 0.0], ()).unwrap();
        store.remove("a").unwrap();

        let stats = store.stats().unwrap();
        assert_eq!(stats.entry_count, 1);
    }

    // ============================================================
    // VectorStoreExt::backend 测试
    // ============================================================

    #[test]
    fn test_backend_returns_memory() {
        let store = make_store();
        assert_eq!(store.backend(), VectorBackend::Memory);
    }

    // ============================================================
    // trait 契约验证
    // ============================================================

    #[test]
    fn test_trait_upsert_delegates_to_vector_index() {
        // 验证 trait upsert 委托给 VectorIndex::upsert
        let store = make_store();
        store.upsert("a", &[1.0, 0.0, 0.0, 0.0], ()).unwrap();

        // 通过 top_k 验证数据已插入
        let results = store.top_k(&[1.0, 0.0, 0.0, 0.0], 1, "").unwrap();
        assert_eq!(results[0].id, "a");
        assert!((results[0].score - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_trait_top_k_returns_vector_hits() {
        let store = make_store();
        store.upsert("a", &[1.0, 0.0, 0.0, 0.0], ()).unwrap();

        let results: Vec<VectorHit> = store.top_k(&[1.0, 0.0, 0.0, 0.0], 1, "").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "a");
        assert!((results[0].score - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_trait_remove_delegates_to_vector_index() {
        let store = make_store();
        store.upsert("a", &[1.0, 0.0, 0.0, 0.0], ()).unwrap();
        assert_eq!(store.stats().unwrap().entry_count, 1);

        store.remove("a").unwrap();
        assert_eq!(store.stats().unwrap().entry_count, 0);
    }

    // ============================================================
    // 综合生命周期测试
    // ============================================================

    #[test]
    fn test_full_lifecycle_upsert_search_remove_compact() {
        let store = make_store();
        assert!(store.stats().unwrap().is_empty());

        // 批量插入
        store
            .insert_batch(vec![
                ("a".to_string(), vec![1.0, 0.0, 0.0, 0.0], ()),
                ("b".to_string(), vec![0.0, 1.0, 0.0, 0.0], ()),
                ("c".to_string(), vec![0.0, 0.0, 1.0, 0.0], ()),
            ])
            .unwrap();
        assert_eq!(store.stats().unwrap().entry_count, 3);

        // 检索
        let results = store.top_k(&[1.0, 0.0, 0.0, 0.0], 2, "").unwrap();
        assert_eq!(results[0].id, "a");
        assert!((results[0].score - 1.0).abs() < 1e-5);

        // 删除
        store.remove("b").unwrap();
        assert_eq!(store.stats().unwrap().entry_count, 2);

        // 压缩(HashMap 无操作)
        store.compact().unwrap();
        assert_eq!(store.stats().unwrap().entry_count, 2);

        // 删除后搜索不应返回 "b"
        let results = store.top_k(&[0.0, 1.0, 0.0, 0.0], 10, "").unwrap();
        assert!(results.iter().all(|h| h.id != "b"));
    }

    #[test]
    fn test_repeated_upsert_no_duplicate_in_search() {
        let store = make_store();
        for i in 0..5 {
            let v = [i as f32, 0.0, 0.0, 0.0];
            store.upsert("a", &v, ()).unwrap();
        }
        assert_eq!(store.stats().unwrap().entry_count, 1);

        let results = store.top_k(&[4.0, 0.0, 0.0, 0.0], 10, "").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "a");
    }

    #[test]
    fn test_medium_scale_100_entries() {
        // 100 entry 中等规模测试(验证精确 KNN 正常工作)
        let store = MemoryKnnStore::new(100);

        for i in 0..100u32 {
            let mut vec = vec![0.0f32; 100];
            vec[i as usize] = 1.0;
            store.upsert(&format!("entry-{i}"), &vec, ()).unwrap();
        }

        assert_eq!(store.stats().unwrap().entry_count, 100);

        // 查询与 entry-0 相同的向量(one-hot at index 0),应返回 entry-0 为最佳匹配
        let mut query = vec![0.0f32; 100];
        query[0] = 1.0;
        let results = store.top_k(&query, 5, "").unwrap();
        assert_eq!(results.len(), 5);
        assert_eq!(results[0].id, "entry-0");
        assert!((results[0].score - 1.0).abs() < 1e-5);
    }
}
