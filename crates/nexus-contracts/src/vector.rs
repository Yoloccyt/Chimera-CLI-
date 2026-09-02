//! 向量存储契约 — 跨层向量检索抽象（L0 契约层）
//!
//! 对应架构层: **L0 Contracts**
//! 对应 ADR: **ADR-033**（L0 nexus-contracts 契约层建立）
//! 对应设计源: `NEXUS-OMEGA_v5.0_系统性完整设计文档.md` §C6 + spec.md P2-W7.3
//!
//! # 核心职责
//!
//! 抽象出统一的向量存储接口，消除 L5 `repo-wiki::VectorIndex` 的深模块问题（D4）：
//! 当前 `VectorIndex` 仅支持 ≤1000 entry 的内存 KNN，P3 HCW-Sparse v2.0 精排
//! 需要 10K-100K entry 的 HNSW 检索。本 trait 让 `MemoryKnnStore`（fallback）
//! 与 `HnswStore`（生产）可替换，调用方面对统一接口编程。
//!
//! # 设计约束（ADR-033）
//!
//! - **零 crate 依赖**: 仅 `std` + `serde` 派生宏，不依赖 `nexus-core::CLV` 等具体类型
//! - **向量抽象**: 用 `&[f32]` 切片而非 `CLV` 类型，让 L0 不被 L1 类型污染
//! - **关联类型解耦**: `type Meta` / `type Error` 由实现方决定，trait 不预设具体类型
//! - **命名空间隔离**: `top_k(query, k, ns)` 的 `ns` 参数支持多租户场景
//!
//! # 方法签名约定（spec.md P2-W7.3.1）
//!
//! | 方法 | 语义 | 备注 |
//! |------|------|------|
//! | `upsert(id, vector, meta)` | UPSERT 语义，id 已存在则覆盖 | 维度不匹配返回 Error |
//! | `top_k(query, k, ns)` | 返回 Top-K 相似条目（score 降序） | ns="" 表示单租户 |
//! | `remove(id)` | 删除条目（幂等） | id 不存在返回 Ok(()) |
//! | `default()` | 返回默认实现（fallback 路径） | Self: Sized，trait 对象不可调用 |
//!
//! # 示例
//!
//! ```
//! use nexus_contracts::{VectorStore, VectorHit};
//!
//! /// 测试用 mock 实现 — 验证 trait 契约
//! pub struct InMemoryStore {
//!     dim: usize,
//! }
//!
//! impl VectorStore for InMemoryStore {
//!     type Meta = ();
//!     type Error = String;
//!
//!     fn upsert(&self, _id: &str, _vector: &[f32], _meta: Self::Meta) -> Result<(), Self::Error> {
//!         Ok(())
//!     }
//!     fn top_k(&self, _query: &[f32], _k: usize, _ns: &str) -> Result<Vec<VectorHit>, Self::Error> {
//!         Ok(Vec::new())
//!     }
//!     fn remove(&self, _id: &str) -> Result<(), Self::Error> {
//!         Ok(())
//!     }
//!     fn default() -> Self {
//!         Self { dim: 512 }
//!     }
//! }
//!
//! let store = InMemoryStore::default();
//! assert!(store.top_k(&[0.0; 512], 10, "").unwrap().is_empty());
//! ```

use serde::{Deserialize, Serialize};

// ============================================================
// 公开类型定义
// ============================================================

/// 向量检索命中 — `top_k` 检索返回的单条结果
///
/// # 字段
/// - `id`: 条目 ID（与 `upsert` 时的 id 一致）
/// - `score`: 相似度分数 ∈ [0.0, 1.0]（余弦相似度，1.0 = 完全相同）
///
/// # 序列化
/// 派生 `Serialize`/`Deserialize`，可跨进程传输（如 MCP Mesh 调用响应）。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VectorHit {
    /// 命中条目 ID
    pub id: String,
    /// 相似度分数 ∈ [0.0, 1.0]
    pub score: f32,
}

impl VectorHit {
    /// 创建新的命中记录
    pub fn new(id: impl Into<String>, score: f32) -> Self {
        Self {
            id: id.into(),
            score,
        }
    }
}

/// 向量存储契约 — 跨层向量检索抽象
///
/// # 设计目标
/// 消除 L5 `repo-wiki::VectorIndex` 深模块问题（D4）：
/// 当前 `VectorIndex` 仅支持 ≤1000 entry 的内存 KNN，
/// P3 HCW-Sparse v2.0 精排需要 10K-100K entry 的 HNSW 检索。
/// 本 trait 抽象出统一接口，让 `MemoryKnnStore`（fallback）与 `HnswStore`（生产）可替换。
///
/// # 关联类型
/// - `Meta`: 元数据类型（由实现方决定，如 `WikiEntry` 摘要 / Agent 上下文片段）
/// - `Error`: 错误类型（实现方自定义，如 `WikiError` / `HnswError`）
///
/// # 命名空间（`ns` 参数）
/// `top_k(query, k, ns)` 的 `ns` 参数支持多租户隔离：
/// - chimera-mas 多 Agent 命名空间（INV-7 上下文预算界）
/// - hcw-window 分层上下文窗口（4K/32K/128K/1M）
/// - repo-wiki 多仓库 Wiki 隔离
///
/// 单租户场景传 `""`（空字符串）即可。
///
/// # `default()` 方法的 trait 对象限制
/// `default()` 返回 `Self`，要求 `Self: Sized`。
/// 这意味着 `dyn VectorStore` trait 对象**无法**调用 `default()`
/// （该方法从 vtable 排除，是 Rust 标准模式）。
/// 调用方需用具体类型调用 `default()`，或通过 `VectorStoreExt::backend()`
/// 在运行时诊断后由工厂函数构造。
pub trait VectorStore {
    /// 元数据类型 — 由实现方决定（如 `WikiEntryMeta` / `AgentContextMeta`）
    type Meta;
    /// 错误类型 — 由实现方决定（如 `WikiError` / `HnswError`）
    type Error;

    /// 插入或更新向量（UPSERT 语义）
    ///
    /// 若 `id` 已存在，覆盖旧向量与元数据。
    /// 维度不匹配时返回错误。
    ///
    /// # 参数
    /// - `id`: 条目标识符（与 `top_k` 返回的 `VectorHit.id` 一致）
    /// - `vector`: 向量数据（f32 切片，维度由实现方约定）
    /// - `meta`: 元数据（由实现方定义类型）
    fn upsert(&self, id: &str, vector: &[f32], meta: Self::Meta) -> Result<(), Self::Error>;

    /// KNN 检索 — 返回与查询向量最相似的 Top-K 条目
    ///
    /// 返回 `Vec<VectorHit>`，按 `score` 降序排列。
    ///
    /// # 参数
    /// - `query`: 查询向量（维度需与 `upsert` 时一致）
    /// - `k`: 返回的 Top-K 数量（若索引条目数 < k，返回全部）
    /// - `ns`: 命名空间隔离（多租户场景，单租户传 `""`）
    fn top_k(&self, query: &[f32], k: usize, ns: &str) -> Result<Vec<VectorHit>, Self::Error>;

    /// 删除条目（幂等 — `id` 不存在返回 `Ok(())`）
    ///
    /// # 参数
    /// - `id`: 待删除条目标识符
    fn remove(&self, id: &str) -> Result<(), Self::Error>;

    /// 返回默认实现（fallback 路径）
    ///
    /// # 用途
    /// 当生产实现（如 `HnswStore`）不可用时，调用方降级到 fallback 实现
    /// （如 `MemoryKnnStore`，≤1000 entry 规模）。
    ///
    /// # trait 对象限制
    /// 此方法返回 `Self`，要求 `Self: Sized`。
    /// `dyn VectorStore` 无法调用此方法（vtable 排除），
    /// 需用具体类型调用。
    fn default() -> Self;
}

// ============================================================
// 扩展 trait：VectorStoreExt（P2-W7.3.2）
// ============================================================

/// 向量后端类型标识 — 用于运行时诊断（非运行时旗，C4 合规）
///
/// # 设计原则
/// 此枚举仅用于**诊断与统计**（如 `stats()` 返回后端类型、`VectorStoreExt::backend()`
/// 运行时探测后端类型），**不用于**控制流程分支（避免散落运行时旗，违反 ADR-034）。
///
/// 灰度切换走 `decay-engine` 能力场（C4 红线正路），而非基于 `backend()` 的 if-else。
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum VectorBackend {
    /// 内存 KNN（fallback 路径，≤1000 entry 规模）
    #[default]
    Memory,
    /// HNSW 图索引（生产路径，10K-100K entry 规模）
    Hnsw,
    /// SQLite-vec 扩展（未来路径，当前因 forbid(unsafe_code) 降级）
    SqliteVec,
}

impl VectorBackend {
    /// 返回后端的人类可读名称
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Hnsw => "hnsw",
            Self::SqliteVec => "sqlite-vec",
        }
    }

    /// 返回后端的理论容量上限（entry 数量级）
    ///
    /// 用于诊断与容量规划，非硬性限制。
    pub fn capacity_tier(&self) -> usize {
        match self {
            Self::Memory => 1_000,
            Self::Hnsw => 100_000,
            Self::SqliteVec => 1_000_000,
        }
    }
}

impl std::fmt::Display for VectorBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// 向量存储统计信息 — `VectorStoreExt::stats()` 的返回类型
///
/// # 字段
/// - `entry_count`: 当前存储的向量条目数
/// - `dimension`: 向量维度
/// - `memory_bytes`: 估算的内存占用（字节）
/// - `backend`: 后端类型标识
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct VectorStoreStats {
    /// 当前存储的向量条目数
    pub entry_count: usize,
    /// 向量维度
    pub dimension: usize,
    /// 估算的内存占用（字节）
    pub memory_bytes: u64,
    /// 后端类型标识
    pub backend: VectorBackend,
}

impl VectorStoreStats {
    /// 创建空的统计信息（backend 默认 Memory）
    pub fn new() -> Self {
        Self::default()
    }

    /// 返回是否为空索引
    pub fn is_empty(&self) -> bool {
        self.entry_count == 0
    }

    /// 返回是否接近后端容量上限（≥80%）
    ///
    /// 用于诊断容量瓶颈，触发迁移至更大容量后端。
    pub fn is_near_capacity(&self) -> bool {
        let capacity = self.backend.capacity_tier();
        if capacity == 0 {
            return false;
        }
        self.entry_count * 100 / capacity >= 80
    }
}

/// 向量存储扩展 trait — 批量操作与诊断接口（P2-W7.3.2）
///
/// # 设计原则
/// 主 trait `VectorStore` 仅含 4 个核心方法（`upsert`/`top_k`/`remove`/`default`），
/// 保持接口精简；批量操作与诊断功能分离到扩展 trait，避免主 trait 膨胀。
///
/// # 扩展方法
/// - `insert_batch`: 批量插入（性能优化路径，单次事务/批量构建索引）
/// - `compact`: 压缩索引（碎片整理，HNSW 重建等）
/// - `stats`: 返回存储统计信息（条目数、维度、内存占用、后端类型）
/// - `backend`: 返回后端类型标识（运行时诊断）
///
/// # 实现约定
/// 实现方应同时实现 `VectorStore` 与 `VectorStoreExt`。
/// 调用方可用 `impl VectorStore + VectorStoreExt` 约束获取完整能力。
pub trait VectorStoreExt: VectorStore {
    /// 批量插入向量（性能优化路径）
    ///
    /// 单次事务/批量构建索引，比循环调用 `upsert` 更高效。
    /// 任一条目插入失败时，整个批次回滚（原子性）。
    ///
    /// # 参数
    /// - `entries`: `(id, vector, meta)` 三元组列表
    ///
    /// # 实现注意
    /// - HnswStore: 批量构建 HNSW 图，比逐条 upsert 快 10-100x
    /// - MemoryKnnStore: 单次写锁，比多次获取写锁快
    fn insert_batch(&self, entries: Vec<(String, Vec<f32>, Self::Meta)>)
        -> Result<(), Self::Error>;

    /// 压缩索引（碎片整理）
    ///
    /// # 后端行为差异
    /// - HnswStore: 重建 HNSW 图，消除删除产生的空洞
    /// - MemoryKnnStore: 无操作（HashMap 无碎片问题），返回 Ok(())
    /// - SqliteVec: VACUUM 操作
    ///
    /// # 调用时机
    /// 大量 `remove` 操作后调用，或 `stats().is_near_capacity()` 返回 true 时。
    fn compact(&self) -> Result<(), Self::Error>;

    /// 返回存储统计信息
    ///
    /// 用于诊断与容量规划，不用于控制流程（C4 合规）。
    fn stats(&self) -> Result<VectorStoreStats, Self::Error>;

    /// 返回后端类型标识（运行时诊断）
    ///
    /// # 注意
    /// 此方法仅用于诊断与日志，**不用于**控制流程分支。
    /// 灰度切换走 `decay-engine` 能力场（ADR-034）。
    fn backend(&self) -> VectorBackend;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;
    // 余弦相似度权威实现已下沉至本 crate L0 `util` 模块(第四轮冗余收敛 实施-8)。
    // WHY 不再用 nexus_core:L0 禁止依赖 L1,历史上此处靠 dev-dependency 反向伸手取 L1 实现;
    // 定义下沉后同 crate 直取即可,依赖面收缩而口径仍唯一。
    use crate::util::cosine_similarity_slices;

    /// 测试用 mock 实现 — 真实存储数据的内存 HashMap，验证 trait 契约
    ///
    /// WHY 独立 mock 而非用真实 MemoryKnnStore:
    /// L0 nexus-contracts 禁止依赖 workspace crate（含 repo-wiki），
    /// 故契约测试需用本 crate 内的 mock 实现验证 trait 行为。
    ///
    /// 此 mock 真实存储向量并实现 KNN 检索（O(n) 遍历 + 余弦相似度），
    /// 用于验证 trait 契约的语义正确性（UPSERT 幂等、remove 幂等、top_k 排序等）。
    /// 生产实现（MemoryKnnStore/HnswStore）用 RwLock 替代 RefCell 保证线程安全。
    pub struct InMemoryVectorStore {
        dim: usize,
        vectors: RefCell<HashMap<String, Vec<f32>>>,
    }

    // NOTE: 不实现 std::default::Default trait，避免与 VectorStore::default() 产生 E0034 歧义。
    // 调用 default() 时走 VectorStore trait 方法（语义更明确）。
    impl InMemoryVectorStore {
        /// 创建指定维度的空向量存储
        pub fn with_dim(dim: usize) -> Self {
            Self {
                dim,
                vectors: RefCell::new(HashMap::new()),
            }
        }
    }

    impl VectorStore for InMemoryVectorStore {
        type Meta = ();
        type Error = String;

        fn upsert(&self, id: &str, vector: &[f32], _meta: Self::Meta) -> Result<(), Self::Error> {
            if vector.len() != self.dim {
                return Err(format!(
                    "dimension mismatch: expected {}, got {}",
                    self.dim,
                    vector.len()
                ));
            }
            self.vectors
                .borrow_mut()
                .insert(id.to_string(), vector.to_vec());
            Ok(())
        }

        fn top_k(&self, query: &[f32], k: usize, _ns: &str) -> Result<Vec<VectorHit>, Self::Error> {
            if query.len() != self.dim {
                return Err(format!(
                    "query dimension mismatch: expected {}, got {}",
                    self.dim,
                    query.len()
                ));
            }
            let vectors = self.vectors.borrow();
            let mut scored: Vec<VectorHit> = vectors
                .iter()
                .map(|(id, vec)| VectorHit::new(id.clone(), cosine_similarity_slices(query, vec)))
                .collect();
            // Top-K 用 select_nth_unstable_by（O(n)），符合工程约定
            if k < scored.len() {
                scored.select_nth_unstable_by(k, |a, b| {
                    b.score
                        .partial_cmp(&a.score)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
            }
            scored.truncate(k);
            // 最终降序排序（K log K）
            scored.sort_by(|a, b| {
                b.score
                    .partial_cmp(&a.score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            Ok(scored)
        }

        fn remove(&self, id: &str) -> Result<(), Self::Error> {
            // 幂等：不存在的 id 不报错
            self.vectors.borrow_mut().remove(id);
            Ok(())
        }

        fn default() -> Self {
            // 直接构造，避免 Self::default() 在 impl VectorStore 内的递归歧义
            Self {
                dim: 512,
                vectors: RefCell::new(HashMap::new()),
            }
        }
    }

    impl VectorStoreExt for InMemoryVectorStore {
        fn insert_batch(
            &self,
            entries: Vec<(String, Vec<f32>, Self::Meta)>,
        ) -> Result<(), Self::Error> {
            let mut vectors = self.vectors.borrow_mut();
            for (id, vector, _meta) in entries {
                if vector.len() != self.dim {
                    return Err(format!(
                        "batch entry {id} dimension mismatch: expected {}, got {}",
                        self.dim,
                        vector.len()
                    ));
                }
                vectors.insert(id, vector);
            }
            Ok(())
        }

        fn compact(&self) -> Result<(), Self::Error> {
            // HashMap 无碎片问题，无操作
            Ok(())
        }

        fn stats(&self) -> Result<VectorStoreStats, Self::Error> {
            let vectors = self.vectors.borrow();
            let memory_bytes = vectors
                .values()
                .map(|v| (v.len() * std::mem::size_of::<f32>()) as u64)
                .sum();
            Ok(VectorStoreStats {
                entry_count: vectors.len(),
                dimension: self.dim,
                memory_bytes,
                backend: VectorBackend::Memory,
            })
        }

        fn backend(&self) -> VectorBackend {
            VectorBackend::Memory
        }
    }

    // ============================================================
    // VectorHit 契约测试
    // ============================================================

    #[test]
    fn test_vector_hit_new() {
        let hit = VectorHit::new("entry-1", 0.95);
        assert_eq!(hit.id, "entry-1");
        assert!((hit.score - 0.95).abs() < 1e-6);
    }

    #[test]
    fn test_vector_hit_equality() {
        let h1 = VectorHit::new("a", 0.5);
        let h2 = VectorHit::new("a", 0.5);
        let h3 = VectorHit::new("a", 0.6);
        assert_eq!(h1, h2);
        assert_ne!(h1, h3);
    }

    // 统一使用 Result 返回模式，便于定位序列化/反序列化失败点
    #[test]
    fn test_vector_hit_serde_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
        let hit = VectorHit::new("entry-1", 0.95);
        let json = serde_json::to_string(&hit)?;
        let decoded: VectorHit = serde_json::from_str(&json)?;
        assert_eq!(hit, decoded);
        Ok(())
    }

    #[test]
    fn test_vector_hit_clone_preserves_fields() {
        let hit = VectorHit::new("entry-1", 0.95);
        let cloned = hit.clone();
        assert_eq!(hit, cloned);
    }

    // ============================================================
    // VectorStore 主 trait 契约测试
    // ============================================================

    #[test]
    fn test_vector_store_default_returns_usable_instance() -> Result<(), String> {
        let store = InMemoryVectorStore::default();
        // default() 返回的实例应立即可用
        assert!(store.top_k(&[0.0; 512], 10, "")?.is_empty());
        Ok(())
    }

    #[test]
    fn test_vector_store_upsert_returns_ok() {
        let store = InMemoryVectorStore::with_dim(3);
        assert!(store.upsert("id-1", &[1.0, 0.0, 0.0], ()).is_ok());
    }

    #[test]
    fn test_vector_store_upsert_dimension_mismatch_returns_err() {
        let store = InMemoryVectorStore::with_dim(4);
        let result = store.upsert("id-1", &[1.0, 0.0, 0.0], ());
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("dimension mismatch"));
    }

    #[test]
    fn test_vector_store_upsert_overwrites_existing() -> Result<(), String> {
        // UPSERT 语义：相同 id 覆盖旧向量
        let store = InMemoryVectorStore::with_dim(2);
        store.upsert("a", &[1.0, 0.0], ())?;
        store.upsert("a", &[0.0, 1.0], ())?;

        // 查询 [0.0, 1.0] 应命中 "a"（score ≈ 1.0）
        let results = store.top_k(&[0.0, 1.0], 1, "")?;
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "a");
        assert!((results[0].score - 1.0).abs() < 1e-5);
        Ok(())
    }

    #[test]
    fn test_vector_store_remove_is_idempotent() {
        let store = InMemoryVectorStore::with_dim(3);
        // 删除不存在的 id 应幂等返回 Ok
        assert!(store.remove("nonexistent").is_ok());
    }

    #[test]
    fn test_vector_store_remove_deletes_entry() -> Result<(), String> {
        let store = InMemoryVectorStore::with_dim(3);
        store.upsert("a", &[1.0, 0.0, 0.0], ())?;
        assert_eq!(store.stats()?.entry_count, 1);

        store.remove("a")?;
        assert_eq!(store.stats()?.entry_count, 0);
        Ok(())
    }

    #[test]
    fn test_vector_store_top_k_empty_returns_empty() -> Result<(), String> {
        let store = InMemoryVectorStore::with_dim(3);
        let results = store.top_k(&[1.0, 0.0, 0.0], 5, "")?;
        assert!(results.is_empty());
        Ok(())
    }

    #[test]
    fn test_vector_store_top_k_returns_sorted_by_score_desc() -> Result<(), String> {
        let store = InMemoryVectorStore::with_dim(2);
        store.upsert("a", &[1.0, 0.0], ())?; // 最相似
        store.upsert("b", &[0.9, 0.1], ())?; // 次相似
        store.upsert("c", &[0.0, 1.0], ())?; // 正交

        let results = store.top_k(&[1.0, 0.0], 2, "")?;
        assert_eq!(results.len(), 2);
        // 按分数降序
        assert_eq!(results[0].id, "a");
        assert_eq!(results[1].id, "b");
        assert!(results[0].score > results[1].score);
        Ok(())
    }

    #[test]
    fn test_vector_store_top_k_k_larger_than_size() -> Result<(), String> {
        let store = InMemoryVectorStore::with_dim(2);
        store.upsert("a", &[1.0, 0.0], ())?;

        let results = store.top_k(&[1.0, 0.0], 10, "")?;
        assert_eq!(results.len(), 1); // 返回全部条目
        Ok(())
    }

    #[test]
    fn test_vector_store_namespace_param_accepted() {
        // 验证 ns 参数被 trait 接受（具体语义由实现方决定）
        let store = InMemoryVectorStore::with_dim(1);
        assert!(store.top_k(&[1.0], 1, "agent-namespace").is_ok());
        assert!(store.top_k(&[1.0], 1, "").is_ok());
    }

    #[test]
    fn test_vector_store_top_k_query_dimension_mismatch() {
        let store = InMemoryVectorStore::with_dim(4);
        store.upsert("a", &[1.0, 0.0, 0.0, 0.0], ()).unwrap();
        let result = store.top_k(&[1.0, 0.0, 0.0], 1, "");
        assert!(result.is_err());
    }

    // ============================================================
    // VectorStoreExt 扩展 trait 契约测试
    // ============================================================

    #[test]
    fn test_vector_store_ext_insert_batch_inserts_all() -> Result<(), String> {
        let store = InMemoryVectorStore::with_dim(2);
        let entries = vec![
            ("a".to_string(), vec![1.0, 0.0], ()),
            ("b".to_string(), vec![0.0, 1.0], ()),
            ("c".to_string(), vec![0.5, 0.5], ()),
        ];
        store.insert_batch(entries)?;
        assert_eq!(store.stats()?.entry_count, 3);
        Ok(())
    }

    #[test]
    fn test_vector_store_ext_insert_batch_empty_is_ok() {
        let store = InMemoryVectorStore::with_dim(2);
        assert!(store.insert_batch(Vec::new()).is_ok());
    }

    #[test]
    fn test_vector_store_ext_insert_batch_dimension_mismatch() {
        let store = InMemoryVectorStore::with_dim(4);
        let entries = vec![("a".to_string(), vec![1.0, 0.0], ())]; // dim 2 vs 4
        let result = store.insert_batch(entries);
        assert!(result.is_err());
    }

    #[test]
    fn test_vector_store_ext_compact_returns_ok() -> Result<(), String> {
        let store = InMemoryVectorStore::with_dim(2);
        store.upsert("a", &[1.0, 0.0], ())?;
        store.remove("a")?;
        // compact 应幂等返回 Ok
        assert!(store.compact().is_ok());
        Ok(())
    }

    #[test]
    fn test_vector_store_ext_stats_returns_correct_count() -> Result<(), String> {
        let store = InMemoryVectorStore::with_dim(3);
        store.upsert("a", &[1.0, 0.0, 0.0], ())?;
        store.upsert("b", &[0.0, 1.0, 0.0], ())?;

        let stats = store.stats()?;
        assert_eq!(stats.entry_count, 2);
        assert_eq!(stats.dimension, 3);
        assert_eq!(stats.backend, VectorBackend::Memory);
        // 2 entries × 3 dims × 4 bytes/f32 = 24 bytes
        assert_eq!(stats.memory_bytes, 24);
        Ok(())
    }

    #[test]
    fn test_vector_store_ext_stats_empty_store() -> Result<(), String> {
        let store = InMemoryVectorStore::with_dim(512);
        let stats = store.stats()?;
        assert_eq!(stats.entry_count, 0);
        assert!(stats.is_empty());
        assert_eq!(stats.memory_bytes, 0);
        Ok(())
    }

    #[test]
    fn test_vector_store_ext_backend_returns_memory() {
        let store = InMemoryVectorStore::default();
        assert_eq!(store.backend(), VectorBackend::Memory);
    }

    // ============================================================
    // VectorBackend 契约测试
    // ============================================================

    #[test]
    fn test_vector_backend_as_str() {
        assert_eq!(VectorBackend::Memory.as_str(), "memory");
        assert_eq!(VectorBackend::Hnsw.as_str(), "hnsw");
        assert_eq!(VectorBackend::SqliteVec.as_str(), "sqlite-vec");
    }

    #[test]
    fn test_vector_backend_capacity_tier() {
        assert_eq!(VectorBackend::Memory.capacity_tier(), 1_000);
        assert_eq!(VectorBackend::Hnsw.capacity_tier(), 100_000);
        assert_eq!(VectorBackend::SqliteVec.capacity_tier(), 1_000_000);
    }

    #[test]
    fn test_vector_backend_display() {
        assert_eq!(format!("{}", VectorBackend::Memory), "memory");
        assert_eq!(format!("{}", VectorBackend::Hnsw), "hnsw");
    }

    #[test]
    fn test_vector_backend_default_is_memory() {
        assert_eq!(VectorBackend::default(), VectorBackend::Memory);
    }

    #[test]
    fn test_vector_backend_serde_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
        let backend = VectorBackend::Hnsw;
        let json = serde_json::to_string(&backend)?;
        let decoded: VectorBackend = serde_json::from_str(&json)?;
        assert_eq!(backend, decoded);
        Ok(())
    }

    #[test]
    fn test_vector_backend_equality() {
        assert_eq!(VectorBackend::Memory, VectorBackend::Memory);
        assert_ne!(VectorBackend::Memory, VectorBackend::Hnsw);
    }

    // ============================================================
    // VectorStoreStats 契约测试
    // ============================================================

    #[test]
    fn test_vector_store_stats_new_is_empty() {
        let stats = VectorStoreStats::new();
        assert!(stats.is_empty());
        assert_eq!(stats.entry_count, 0);
        assert_eq!(stats.backend, VectorBackend::Memory);
    }

    #[test]
    fn test_vector_store_stats_is_empty_true_when_zero() {
        let stats = VectorStoreStats {
            entry_count: 0,
            dimension: 512,
            memory_bytes: 0,
            backend: VectorBackend::Hnsw,
        };
        assert!(stats.is_empty());
    }

    #[test]
    fn test_vector_store_stats_is_near_capacity_memory_at_80_percent() {
        // Memory capacity_tier = 1000, 800 entries = 80%
        let stats = VectorStoreStats {
            entry_count: 800,
            dimension: 512,
            memory_bytes: 0,
            backend: VectorBackend::Memory,
        };
        assert!(stats.is_near_capacity());
    }

    #[test]
    fn test_vector_store_stats_is_near_capacity_memory_below_80_percent() {
        let stats = VectorStoreStats {
            entry_count: 799,
            dimension: 512,
            memory_bytes: 0,
            backend: VectorBackend::Memory,
        };
        assert!(!stats.is_near_capacity());
    }

    #[test]
    fn test_vector_store_stats_is_near_capacity_hnsw_at_80_percent() {
        // Hnsw capacity_tier = 100000, 80000 entries = 80%
        let stats = VectorStoreStats {
            entry_count: 80_000,
            dimension: 512,
            memory_bytes: 0,
            backend: VectorBackend::Hnsw,
        };
        assert!(stats.is_near_capacity());
    }

    #[test]
    fn test_vector_store_stats_serde_roundtrip() -> Result<(), Box<dyn std::error::Error>> {
        let stats = VectorStoreStats {
            entry_count: 42,
            dimension: 512,
            memory_bytes: 1024,
            backend: VectorBackend::Hnsw,
        };
        let json = serde_json::to_string(&stats)?;
        let decoded: VectorStoreStats = serde_json::from_str(&json)?;
        assert_eq!(stats, decoded);
        Ok(())
    }

    #[test]
    fn test_vector_store_stats_clone_preserves_fields() {
        let stats = VectorStoreStats {
            entry_count: 42,
            dimension: 512,
            memory_bytes: 1024,
            backend: VectorBackend::Memory,
        };
        let cloned = stats.clone();
        assert_eq!(stats, cloned);
    }

    // ============================================================
    // 综合 trait 契约测试（验证 VectorStore + VectorStoreExt 同时实现可用）
    // ============================================================

    #[test]
    fn test_combined_vector_store_and_ext_traits() -> Result<(), String> {
        // 验证 impl VectorStore + VectorStoreExt 约束的泛型函数可用
        // 通过 trait bound 约束验证两种 trait 可同时用于同一类型
        fn use_store<S>(store: &S) -> Result<(), S::Error>
        where
            S: VectorStore<Meta = ()> + VectorStoreExt,
            S::Error: std::fmt::Debug,
        {
            store.upsert("a", &[1.0, 0.0], ())?;
            store.upsert("b", &[0.0, 1.0], ())?;
            let results = store.top_k(&[1.0, 0.0], 1, "")?;
            assert_eq!(results.len(), 1);
            assert_eq!(results[0].id, "a");
            let stats = store.stats()?;
            assert_eq!(stats.entry_count, 2);
            assert_eq!(store.backend(), VectorBackend::Memory);
            Ok(())
        }

        let store = InMemoryVectorStore::with_dim(2);
        use_store(&store)?;
        Ok(())
    }

    #[test]
    fn test_full_lifecycle_upsert_search_remove_compact() -> Result<(), String> {
        // 端到端生命周期：upsert → top_k → remove → compact → stats
        let store = InMemoryVectorStore::with_dim(3);
        assert!(store.stats()?.is_empty());

        // 批量插入
        store.insert_batch(vec![
            ("a".to_string(), vec![1.0, 0.0, 0.0], ()),
            ("b".to_string(), vec![0.0, 1.0, 0.0], ()),
            ("c".to_string(), vec![0.0, 0.0, 1.0], ()),
        ])?;
        assert_eq!(store.stats()?.entry_count, 3);

        // 检索
        let results = store.top_k(&[1.0, 0.0, 0.0], 2, "")?;
        assert_eq!(results[0].id, "a");
        assert!((results[0].score - 1.0).abs() < 1e-5);

        // 删除
        store.remove("b")?;
        assert_eq!(store.stats()?.entry_count, 2);

        // 压缩（幂等）
        store.compact()?;
        assert_eq!(store.stats()?.entry_count, 2);
        Ok(())
    }
}
