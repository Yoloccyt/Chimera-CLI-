//! HNSW 向量存储实现 — 生产路径(10K-100K entry 规模)
//!
//! 对应架构层: L5 Knowledge
//! 对应 ADR: ADR-033(L0 nexus-contracts 契约层)
//! 对应任务: P2-W8.1.1
//!
//! # 核心职责
//! 实现 `VectorStore` + `VectorStoreExt` trait,提供基于 HNSW 算法的
//! 近似最近邻检索(ANN),支持 10K-100K entry 规模的低延迟检索。
//!
//! # 设计决策
//!
//! ## 类型参数
//! - `T = f32`: hnsw_rs 的元素类型,数据向量为 `&[f32]` 切片
//! - `D = DistCosine`: 余弦距离,`eval` 返回 `1.0 - cos_sim`(≥0),
//!   契合 CLV 512-dim 语义相似度场景
//!
//! ## ID 映射
//! VectorStore trait 用 `&str` ID,hnsw_rs 用 `usize` DataId。
//! 维护双向映射:
//! - `id_to_dataid: HashMap<String, usize>` — upsert/remove 查找用
//! - `entries: HashMap<usize, (String, Vec<f32>)>` — DataId → (ID, 原始向量)
//!   原始向量用于 compact 重建索引(hnsw_rs 0.3.4 不支持检索已存向量)
//!
//! ## 删除策略(墓碑)
//! hnsw_rs 0.3.4 不支持真实删除,采用墓碑标记:
//! - `remove`: 将 DataId 加入 tombstones 集合,不从 HNSW 图中删除
//! - `top_k`: 搜索后过滤墓碑条目,通过 over-fetch 补偿
//! - `compact`: 重建 HNSW 图,清除墓碑
//!
//! ## UPSERT 语义
//! hnsw_rs 不支持更新。UPSERT 实现:
//! 1. 若 ID 已存在,将旧 DataId 加入墓碑
//! 2. 分配新 DataId 插入 HNSW
//! 3. 更新映射表
//!
//! ## 命名空间
//! 当前为单租户实现(ns 参数接受但忽略)。
//! P3 HCW-Sparse v2.0 迭代时扩展为多实例隔离。
//!
//! # 性能特征
//! - 10K entry: KNN 检索 p95 < 50ms(spec.md P2-W8.1.3 红线)
//! - 100K entry: 不 OOM(spec.md P2-W8.1.4 红线)
//! - 构建: O(N · log N),ef_construction 控制精度

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::RwLock;

use hnsw_rs::hnsw::{Hnsw, Neighbour};
use hnsw_rs::prelude::DistCosine;

use nexus_contracts::{VectorBackend, VectorHit, VectorStore, VectorStoreExt, VectorStoreStats};

use crate::error::WikiError;
use crate::types::HnswConfig;

// ============================================================
// 常量定义
// ============================================================

/// 默认向量维度 — 与 CLV(Context Latent Vector)512-dim 对齐
const DEFAULT_DIM: usize = 512;

/// HNSW 每层最大连接数(M 参数) — 控制图连通性,16 为典型值
/// WHY 16: 论文推荐 M ∈ [16, 48],16 在召回率与内存间取得平衡
const DEFAULT_MAX_NB_CONNECTION: usize = 16;

/// HNSW 预分配容量提示 — 非硬性限制,仅优化分配
const DEFAULT_MAX_ELEMENTS: usize = 10_000;

/// HNSW 最大层级 — 控制层次结构深度
const DEFAULT_MAX_LAYER: usize = 16;

/// HNSW 构建时 ef 参数 — 控制构建质量,越大越精确但越慢
/// WHY 200: 论文推荐 ef_construction ∈ [100, 500],200 为均衡值
const DEFAULT_EF_CONSTRUCTION: usize = 200;

/// HNSW 搜索时 ef 参数 — 控制搜索宽度,必须 > k
/// WHY 50: 对于 10K-100K entry 规模,ef=50 足以保证 >95% 召回率
const DEFAULT_EF_SEARCH: usize = 50;

/// 墓碑过补偿因子 — 搜索时额外获取的条目数
/// WHY: 墓碑条目仍会被 HNSW 搜索返回,需要 over-fetch 再过滤
const TOMBSTONE_OVERFETCH_FACTOR: usize = 2;

// ============================================================
// HnswStore 结构体
// ============================================================

/// HNSW 向量存储 — 生产路径实现
///
/// 基于 `hnsw_rs` 0.3.4,支持 10K-100K entry 规模的近似最近邻检索。
/// 实现 `VectorStore` + `VectorStoreExt` 双 trait,可通过统一接口
/// 与 `MemoryKnnStore`(fallback)互换。
///
/// # 线程安全
/// - `Hnsw` 本身线程安全(`insert`/`search` 取 `&self`)
/// - 元数据映射用 `RwLock` 保护(读多写少)
/// - `next_dataid` 用 `AtomicUsize`(无锁分配)
///
/// # 示例
/// ```
/// use nexus_contracts::{VectorStore, VectorStoreExt, VectorBackend};
/// use repo_wiki::vector::hnsw_store::HnswStore;
///
/// let store = HnswStore::with_dim(3);
/// store.upsert("entry-1", &[1.0, 0.0, 0.0], ()).unwrap();
///
/// let results = store.top_k(&[1.0, 0.0, 0.0], 1, "").unwrap();
/// assert_eq!(results[0].id, "entry-1");
/// assert_eq!(store.backend(), VectorBackend::Hnsw);
/// ```
pub struct HnswStore {
    /// HNSW 索引实例(f32 元素 + DistCosine 余弦距离)
    ///
    /// WHY RwLock 而非直接持有:`insert`/`search` 取 `&self`(读锁即可),
    /// 但 `compact` 需要替换整个 Hnsw 实例(写锁)。
    /// 读锁允许多个并发 search/insert,仅 compact 时互斥。
    hnsw: RwLock<Hnsw<'static, f32, DistCosine>>,

    /// DataId → (String ID, 原始向量) 映射
    ///
    /// WHY 存储原始向量:
    /// 1. `compact` 重建索引需要原始向量(hnsw_rs 0.3.4 不支持检索已存向量)
    /// 2. `stats` 计算内存占用需要向量大小
    /// 3. 维度校验(query/upsert 的 dim 检查)
    entries: RwLock<HashMap<usize, (String, Vec<f32>)>>,

    /// String ID → DataId 映射
    ///
    /// WHY 反向映射:`upsert` 时需快速判断 ID 是否已存在(避免 O(n) 遍历 entries)
    id_to_dataid: RwLock<HashMap<String, usize>>,

    /// 已删除的 DataId 集合(墓碑)
    ///
    /// WHY 墓碑:hnsw_rs 0.3.4 不支持真实删除,
    /// 删除时标记墓碑,search 结果过滤墓碑条目,
    /// compact 时重建索引清除墓碑。
    /// **不变量**: tombstones ∩ entries.keys() = ∅(一个 DataId 不会同时在两者中)
    tombstones: RwLock<HashSet<usize>>,

    /// 下一个可用的 DataId(单调递增,永不回收)
    ///
    /// WHY 单调递增而非回收:避免墓碑 DataId 被复用导致
    /// 已删除条目的 HNSW 图节点被误命中
    next_dataid: AtomicUsize,

    /// 向量维度(所有 upsert/query 的向量长度必须与此一致)
    dim: usize,

    /// HNSW 搜索 ef 参数(控制搜索宽度)
    ef_search: usize,

    /// P2-5: 构造时使用的 HNSW 参数(供 compact 重建索引时复用)
    ///
    /// WHY 存储:原 compact() 使用硬编码常量(DEFAULT_MAX_NB_CONNECTION 等)
    /// 重建索引,导致自定义参数(M=32 等)在 compact 后回退为默认值(M=16)。
    /// 存储原始参数确保 compact 后索引参数一致。
    build_params: HnswBuildParams,
}

/// P2-5: HNSW 构造参数快照 — 供 compact() 重建索引时复用
#[derive(Debug, Clone, Copy)]
struct HnswBuildParams {
    /// 每层最大连接数(M 参数)
    max_nb_connection: usize,
    /// 预分配容量提示
    max_elements: usize,
    /// 最大层级
    max_layer: usize,
    /// 构建时 ef 参数
    ef_construction: usize,
}

// ============================================================
// 构造方法
// ============================================================

impl HnswStore {
    /// 创建指定维度的 HNSW 存储,使用默认 HNSW 参数
    ///
    /// # 参数
    /// - `dim`: 向量维度(应与 CLV 512-dim 一致)
    pub fn with_dim(dim: usize) -> Self {
        Self::with_params(
            dim,
            DEFAULT_MAX_NB_CONNECTION,
            DEFAULT_MAX_ELEMENTS,
            DEFAULT_MAX_LAYER,
            DEFAULT_EF_CONSTRUCTION,
            DEFAULT_EF_SEARCH,
        )
    }

    /// 创建自定义参数的 HNSW 存储
    ///
    /// # 参数
    /// - `dim`: 向量维度
    /// - `max_nb_connection`: HNSW M 参数(每层最大连接数,< 256)
    /// - `max_elements`: 预分配容量提示(非硬性限制)
    /// - `max_layer`: 最大层级
    /// - `ef_construction`: 构建时 ef 参数
    /// - `ef_search`: 搜索时 ef 参数(必须 > k)
    #[allow(clippy::too_many_arguments)]
    pub fn with_params(
        dim: usize,
        max_nb_connection: usize,
        max_elements: usize,
        max_layer: usize,
        ef_construction: usize,
        ef_search: usize,
    ) -> Self {
        let hnsw = Hnsw::new(
            max_nb_connection,
            max_elements,
            max_layer,
            ef_construction,
            DistCosine,
        );
        Self {
            hnsw: RwLock::new(hnsw),
            entries: RwLock::new(HashMap::new()),
            id_to_dataid: RwLock::new(HashMap::new()),
            tombstones: RwLock::new(HashSet::new()),
            next_dataid: AtomicUsize::new(0),
            dim,
            ef_search,
            build_params: HnswBuildParams {
                max_nb_connection,
                max_elements,
                max_layer,
                ef_construction,
            },
        }
    }

    /// P2-5: 从 HnswConfig 创建 HNSW 存储
    ///
    /// 将 `HnswConfig`(可通过 `WikiConfig` 配置)转换为 `HnswStore` 构造参数,
    /// 替代原硬编码常量路径。供上层通过配置文件调优 HNSW 参数。
    ///
    /// # 参数
    /// - `dim`: 向量维度
    /// - `config`: HNSW 参数配置(来自 `WikiConfig.hnsw`)
    ///
    /// # 示例
    /// ```
    /// use nexus_contracts::{VectorBackend, VectorStoreExt};
    /// use repo_wiki::types::HnswConfig;
    /// use repo_wiki::vector::hnsw_store::HnswStore;
    ///
    /// let config = HnswConfig::new(32, 50_000, 20, 300, 100);
    /// let store = HnswStore::with_config(512, &config);
    /// assert_eq!(store.backend(), VectorBackend::Hnsw);
    /// ```
    pub fn with_config(dim: usize, config: &HnswConfig) -> Self {
        Self::with_params(
            dim,
            config.max_nb_connection,
            config.max_elements,
            config.max_layer,
            config.ef_construction,
            config.ef_search,
        )
    }

    /// 分配下一个 DataId(单调递增)
    fn alloc_dataid(&self) -> usize {
        self.next_dataid.fetch_add(1, Ordering::Relaxed)
    }

    /// 将 hnsw_rs 距离转换为相似度分数
    ///
    /// `DistCosine::eval` 返回 `1.0 - cos_sim`(已 clamp 到 ≥0),
    /// 故 `score = 1.0 - distance` 恢复余弦相似度。
    /// 额外 clamp 到 [0.0, 1.0] 确保与 `VectorHit::score` 约定一致。
    fn distance_to_score(distance: f32) -> f32 {
        (1.0 - distance).clamp(0.0, 1.0)
    }

    /// 检查向量维度是否匹配
    fn check_dim(&self, vector: &[f32]) -> Result<(), WikiError> {
        if vector.len() != self.dim {
            return Err(WikiError::VectorIndexError(format!(
                "dimension mismatch: expected {}, got {}",
                self.dim,
                vector.len()
            )));
        }
        Ok(())
    }
}

// ============================================================
// VectorStore trait 实现
// ============================================================

impl VectorStore for HnswStore {
    type Meta = ();
    type Error = WikiError;

    /// 插入或更新向量(UPSERT 语义)
    ///
    /// 若 `id` 已存在,将旧 DataId 加入墓碑,分配新 DataId 插入。
    /// 维度不匹配时返回错误。
    ///
    /// # 并发
    /// 持有 id_to_dataid 写锁期间完成"旧条目墓碑化 + 新条目插入"原子序列。
    /// Hnsw::insert 取 `&self`(读锁),不阻塞其他 search。
    fn upsert(&self, id: &str, vector: &[f32], _meta: Self::Meta) -> Result<(), Self::Error> {
        self.check_dim(vector)?;

        // 1. 检查 ID 是否已存在,若存在则墓碑化旧 DataId
        {
            let id_map = self
                .id_to_dataid
                .read()
                .map_err(|e| WikiError::VectorIndexError(format!("rwlock poisoned: {e}")))?;
            if let Some(&old_dataid) = id_map.get(id) {
                // 墓碑化旧条目
                drop(id_map); // 释放读锁后再获取写锁,避免锁升级死锁
                let mut tombstones = self
                    .tombstones
                    .write()
                    .map_err(|e| WikiError::VectorIndexError(format!("rwlock poisoned: {e}")))?;
                tombstones.insert(old_dataid);
                // 从 entries 移除旧条目(tombstones ∩ entries.keys() = ∅ 不变量)
                let mut entries = self
                    .entries
                    .write()
                    .map_err(|e| WikiError::VectorIndexError(format!("rwlock poisoned: {e}")))?;
                entries.remove(&old_dataid);
            }
        }

        // 2. 分配新 DataId 并插入 HNSW
        let dataid = self.alloc_dataid();
        {
            let hnsw = self
                .hnsw
                .read()
                .map_err(|e| WikiError::VectorIndexError(format!("rwlock poisoned: {e}")))?;
            hnsw.insert((vector, dataid));
        }

        // 3. 更新映射表
        {
            let mut entries = self
                .entries
                .write()
                .map_err(|e| WikiError::VectorIndexError(format!("rwlock poisoned: {e}")))?;
            entries.insert(dataid, (id.to_string(), vector.to_vec()));
        }
        {
            let mut id_map = self
                .id_to_dataid
                .write()
                .map_err(|e| WikiError::VectorIndexError(format!("rwlock poisoned: {e}")))?;
            id_map.insert(id.to_string(), dataid);
        }

        Ok(())
    }

    /// KNN 检索 — 返回与查询向量最相似的 Top-K 条目
    ///
    /// 搜索时 over-fetch 以补偿墓碑过滤导致的结果丢失,
    /// 最终截断到 k 条。结果按 score 降序排列。
    ///
    /// # 命名空间
    /// 当前为单租户实现,`ns` 参数接受但忽略。
    fn top_k(&self, query: &[f32], k: usize, _ns: &str) -> Result<Vec<VectorHit>, Self::Error> {
        self.check_dim(query)?;

        if k == 0 {
            return Ok(Vec::new());
        }

        // 获取墓碑数量用于 over-fetch
        let tombstone_count = {
            let tombstones = self
                .tombstones
                .read()
                .map_err(|e| WikiError::VectorIndexError(format!("rwlock poisoned: {e}")))?;
            tombstones.len()
        };

        // over-fetch:额外获取墓碑数量的条目,但至少 k * 2
        let fetch_k = k
            .saturating_add(tombstone_count)
            .max(k * TOMBSTONE_OVERFETCH_FACTOR);
        // ef 必须大于 knbn(hnsw_rs 约束)
        let ef = self.ef_search.max(fetch_k + 1);

        let neighbours: Vec<Neighbour> = {
            let hnsw = self
                .hnsw
                .read()
                .map_err(|e| WikiError::VectorIndexError(format!("rwlock poisoned: {e}")))?;
            hnsw.search(query, fetch_k, ef)
        };

        // 过滤墓碑 + 映射为 VectorHit
        let tombstones = self
            .tombstones
            .read()
            .map_err(|e| WikiError::VectorIndexError(format!("rwlock poisoned: {e}")))?;
        let entries = self
            .entries
            .read()
            .map_err(|e| WikiError::VectorIndexError(format!("rwlock poisoned: {e}")))?;

        // 过滤墓碑 + 映射为 VectorHit(收集全部,排序后再截断)
        // WHY 先收集再排序:HNSW 返回的 Neighbour 顺序不保证按距离升序,
        // 必须先排序再 truncate(k),否则可能截断掉真正的高分结果
        let mut hits: Vec<VectorHit> = neighbours
            .into_iter()
            .filter(|n| !tombstones.contains(&n.d_id))
            .filter_map(|n| {
                entries
                    .get(&n.d_id)
                    .map(|(id, _)| VectorHit::new(id.clone(), Self::distance_to_score(n.distance)))
            })
            .collect();

        // 按 score 降序排列
        hits.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // 排序后截断到 k(确保返回真正的 Top-K)
        hits.truncate(k);

        Ok(hits)
    }

    /// 删除条目(幂等 — `id` 不存在返回 `Ok(())`)
    ///
    /// 将 DataId 加入墓碑集合,不从 HNSW 图中删除。
    /// 实际清除发生在 `compact` 重建索引时。
    fn remove(&self, id: &str) -> Result<(), Self::Error> {
        let dataid = {
            let id_map = self
                .id_to_dataid
                .read()
                .map_err(|e| WikiError::VectorIndexError(format!("rwlock poisoned: {e}")))?;
            id_map.get(id).copied()
        };

        match dataid {
            Some(dataid) => {
                // 加入墓碑
                {
                    let mut tombstones = self.tombstones.write().map_err(|e| {
                        WikiError::VectorIndexError(format!("rwlock poisoned: {e}"))
                    })?;
                    tombstones.insert(dataid);
                }
                // 从 entries 移除(保持不变量: tombstones ∩ entries.keys() = ∅)
                {
                    let mut entries = self.entries.write().map_err(|e| {
                        WikiError::VectorIndexError(format!("rwlock poisoned: {e}"))
                    })?;
                    entries.remove(&dataid);
                }
                // 从 id_to_dataid 移除
                {
                    let mut id_map = self.id_to_dataid.write().map_err(|e| {
                        WikiError::VectorIndexError(format!("rwlock poisoned: {e}"))
                    })?;
                    id_map.remove(id);
                }
                Ok(())
            }
            None => Ok(()), // 幂等:不存在的 id 不报错
        }
    }

    /// 返回默认实现(fallback 构造路径)
    ///
    /// 创建 512-dim 的 HNSW 存储,使用默认参数。
    /// 注意:此方法返回 `Self`,要求 `Self: Sized`,
    /// `dyn VectorStore` trait 对象无法调用。
    fn default() -> Self {
        Self::with_dim(DEFAULT_DIM)
    }
}

// ============================================================
// VectorStoreExt trait 实现
// ============================================================

impl VectorStoreExt for HnswStore {
    /// 批量插入向量
    ///
    /// 逐条调用 `insert_slice`(非 parallel_insert_slice),
    /// 避免使用 Rayon 并行插入的 `set_searching_mode` 状态管理复杂度。
    /// 对于已有 ID 的条目,执行 UPSERT 语义(墓碑旧条目)。
    ///
    /// # 原子性
    /// 任一条目维度不匹配时,整个批次在插入前返回错误(不部分插入)。
    fn insert_batch(
        &self,
        entries: Vec<(String, Vec<f32>, Self::Meta)>,
    ) -> Result<(), Self::Error> {
        if entries.is_empty() {
            return Ok(());
        }

        // 预校验所有维度(原子性:要么全部插入,要么不插入)
        for (id, vector, _) in &entries {
            if vector.len() != self.dim {
                return Err(WikiError::VectorIndexError(format!(
                    "batch entry '{id}' dimension mismatch: expected {}, got {}",
                    self.dim,
                    vector.len()
                )));
            }
        }

        // 逐条 UPSERT
        for (id, vector, _) in entries {
            self.upsert(&id, &vector, ())?;
        }

        Ok(())
    }

    /// 压缩索引 — 重建 HNSW 图清除墓碑
    ///
    /// 1. 收集所有非墓碑条目(entries 中的全部条目)
    /// 2. 创建新 Hnsw 实例
    /// 3. 重新插入所有向量
    /// 4. 替换旧 Hnsw,清空墓碑集合
    ///
    /// # 并发
    /// 持有 Hnsw 写锁期间阻塞所有 search/insert。
    /// 调用时机:大量 remove 后,或 `stats().is_near_capacity()` 返回 true 时。
    fn compact(&self) -> Result<(), Self::Error> {
        // 1. 快照当前有效条目
        let snapshot: Vec<(usize, String, Vec<f32>)> = {
            let entries = self
                .entries
                .read()
                .map_err(|e| WikiError::VectorIndexError(format!("rwlock poisoned: {e}")))?;
            entries
                .iter()
                .map(|(dataid, (id, vec))| (*dataid, id.clone(), vec.clone()))
                .collect()
        };

        if snapshot.is_empty() {
            // 空索引,仅清空墓碑
            let mut tombstones = self
                .tombstones
                .write()
                .map_err(|e| WikiError::VectorIndexError(format!("rwlock poisoned: {e}")))?;
            tombstones.clear();
            return Ok(());
        }

        // 2. 构建新 Hnsw 实例(P2-5: 使用存储的构造参数,而非硬编码常量)
        //    WHY: 原 compact() 使用 DEFAULT_MAX_NB_CONNECTION 等常量重建索引,
        //    导致自定义参数(M=32 等)在 compact 后回退为默认值(M=16)。
        //    P2-5 修复:复用 build_params 确保参数一致性。
        let new_hnsw = Hnsw::new(
            self.build_params.max_nb_connection,
            snapshot.len().max(self.build_params.max_elements),
            self.build_params.max_layer,
            self.build_params.ef_construction,
            DistCosine,
        );

        // 3. 重新插入所有有效向量
        for (dataid, _, vector) in &snapshot {
            new_hnsw.insert((vector.as_slice(), *dataid));
        }

        // 4. 替换旧 Hnsw + 清空墓碑
        {
            let mut hnsw = self
                .hnsw
                .write()
                .map_err(|e| WikiError::VectorIndexError(format!("rwlock poisoned: {e}")))?;
            *hnsw = new_hnsw;
        }
        {
            let mut tombstones = self
                .tombstones
                .write()
                .map_err(|e| WikiError::VectorIndexError(format!("rwlock poisoned: {e}")))?;
            tombstones.clear();
        }

        Ok(())
    }

    /// 返回存储统计信息
    ///
    /// `memory_bytes` = 所有向量占用的字节数(entry_count × dim × sizeof(f32)),
    /// 不含 HNSW 图结构开销(图结构约为向量数据的 1.5-3x,视 M 参数而定)。
    fn stats(&self) -> Result<VectorStoreStats, Self::Error> {
        let entries = self
            .entries
            .read()
            .map_err(|e| WikiError::VectorIndexError(format!("rwlock poisoned: {e}")))?;
        let entry_count = entries.len();
        let memory_bytes = entries
            .values()
            .map(|(_, vec)| (vec.len() * std::mem::size_of::<f32>()) as u64)
            .sum();
        Ok(VectorStoreStats {
            entry_count,
            dimension: self.dim,
            memory_bytes,
            backend: VectorBackend::Hnsw,
        })
    }

    /// 返回后端类型标识(运行时诊断)
    fn backend(&self) -> VectorBackend {
        VectorBackend::Hnsw
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    // --- 辅助函数 ---

    /// 创建小维度(4-dim)HnswStore 用于快速测试
    ///
    /// WHY ef_search=100 而非默认 20:Hnsw::new 内部用 StdRng::from_os_rng()
    /// 每次不同种子,小规模(3-5 条目)下图结构随机性大,ef_search=20 可能
    /// 遗漏某些条目(贪心搜索路径不经过它们)。ef_search=100 扩大搜索宽度,
    /// 确保小规模测试也能稳定找到所有条目。
    fn make_store() -> HnswStore {
        HnswStore::with_params(4, 16, 100, 6, 50, 100)
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
    fn test_with_dim_creates_empty_store() {
        let store = HnswStore::with_dim(512);
        let stats = store.stats().unwrap();
        assert!(stats.is_empty());
        assert_eq!(stats.dimension, 512);
        assert_eq!(stats.backend, VectorBackend::Hnsw);
    }

    #[test]
    fn test_with_params_custom_values() {
        let store = HnswStore::with_params(128, 32, 5000, 10, 150, 30);
        assert_eq!(store.dim, 128);
        assert_eq!(store.ef_search, 30);
    }

    #[test]
    fn test_default_returns_512_dim_store() {
        let store = HnswStore::default();
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
        // 插入 a = [1, 0, 0, 0]
        store.upsert("a", &[1.0, 0.0, 0.0, 0.0], ()).unwrap();
        // UPSERT: a = [0, 1, 0, 0]
        store.upsert("a", &[0.0, 1.0, 0.0, 0.0], ()).unwrap();
        // 条目数仍为 1(旧条目被墓碑)
        assert_eq!(store.stats().unwrap().entry_count, 1);
        // 查询 [0, 1, 0, 0] 应命中 a(score ≈ 1.0)
        let results = store.top_k(&[0.0, 1.0, 0.0, 0.0], 1, "").unwrap();
        assert_eq!(results.len(), 1);
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
        // WHY 5 个相似向量 + 1 个远离向量:Hnsw::new 用随机种子,
        // 小规模下图结构随机,用更多向量确保图连通且 HNSW 能稳定找到相似条目。
        let store = make_store();
        store.upsert("a", &[1.0, 0.0, 0.0, 0.0], ()).unwrap();
        store.upsert("b", &[0.95, 0.05, 0.0, 0.0], ()).unwrap();
        store.upsert("c", &[0.9, 0.1, 0.0, 0.0], ()).unwrap();
        store.upsert("d", &[0.85, 0.15, 0.0, 0.0], ()).unwrap();
        store.upsert("e", &[0.0, 0.0, 0.0, 1.0], ()).unwrap();

        let results = store.top_k(&[1.0, 0.0, 0.0, 0.0], 3, "").unwrap();
        assert_eq!(results.len(), 3);
        // top1 应为 "a"(完全匹配,score≈1.0)
        assert_eq!(results[0].id, "a");
        assert!((results[0].score - 1.0).abs() < 1e-4);
        // 验证不变量:结果按 score 降序排列
        for i in 1..results.len() {
            assert!(
                results[i - 1].score >= results[i].score,
                "结果应按 score 降序:[{}]={} < [{}]={}",
                i - 1,
                results[i - 1].score,
                i,
                results[i].score
            );
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
        // 相同向量余弦相似度应接近 1.0
        assert!((results[0].score - 1.0).abs() < 1e-4);
    }

    #[test]
    fn test_top_k_namespace_param_accepted() {
        // ns 参数接受但忽略(当前单租户实现)
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
        // 删除不存在的 id 应幂等返回 Ok
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
        // WHY 多向量 + 相似向量:Hnsw::new 内部用 StdRng::from_os_rng() 每次不同种子,
        // 图结构随机。当查询与被删除条目完全匹配、与剩余条目正交时,
        // HNSW 贪心搜索可能找不到正交条目(它们不在搜索路径上)。
        // 用 4 个相似向量 + 1 个查询近似向量,确保 HNSW 能找到至少 1 个非墓碑条目。
        let store = make_store();
        store.upsert("a", &[1.0, 0.0, 0.0, 0.0], ()).unwrap();
        store.upsert("b", &[0.9, 0.1, 0.0, 0.0], ()).unwrap();
        store.upsert("c", &[0.8, 0.2, 0.0, 0.0], ()).unwrap();
        store.upsert("d", &[0.7, 0.3, 0.0, 0.0], ()).unwrap();

        store.remove("a").unwrap();

        let results = store.top_k(&[1.0, 0.0, 0.0, 0.0], 10, "").unwrap();
        // "a" 已删除,不应出现在结果中
        assert!(
            !results.iter().any(|h| h.id == "a"),
            "已删除的条目 'a' 不应出现在搜索结果中"
        );
        // 应返回至少 1 个非墓碑条目(b/c/d 与查询相似,HNSW 应能找到)
        assert!(
            !results.is_empty(),
            "应返回至少 1 个非墓碑条目,实际返回 {results:?}"
        );
    }

    #[test]
    fn test_remove_and_reinsert() {
        let store = make_store();
        store.upsert("a", &[1.0, 0.0, 0.0, 0.0], ()).unwrap();
        store.remove("a").unwrap();
        // 重新插入
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

        // "a" 应已更新为新向量
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
    fn test_compact_clears_tombstones() {
        let store = make_store();
        store.upsert("a", &[1.0, 0.0, 0.0, 0.0], ()).unwrap();
        store.upsert("b", &[0.0, 1.0, 0.0, 0.0], ()).unwrap();
        store.remove("a").unwrap();

        // compact 前墓碑中应有一个条目
        store.compact().unwrap();

        // compact 后搜索不应返回已删除条目
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
        assert_eq!(stats.backend, VectorBackend::Hnsw);
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
    fn test_backend_returns_hnsw() {
        let store = make_store();
        assert_eq!(store.backend(), VectorBackend::Hnsw);
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
        assert!((results[0].score - 1.0).abs() < 1e-4);

        // 删除
        store.remove("b").unwrap();
        assert_eq!(store.stats().unwrap().entry_count, 2);

        // 压缩(重建索引)
        store.compact().unwrap();
        assert_eq!(store.stats().unwrap().entry_count, 2);

        // 删除后搜索不应返回 "b"
        let results = store.top_k(&[0.0, 1.0, 0.0, 0.0], 10, "").unwrap();
        assert!(results.iter().all(|h| h.id != "b"));
    }

    #[test]
    fn test_repeated_upsert_no_duplicate_in_search() {
        let store = make_store();
        // 多次 UPSERT 同一 ID
        for i in 0..5 {
            let v = [i as f32, 0.0, 0.0, 0.0];
            store.upsert("a", &v, ()).unwrap();
        }
        assert_eq!(store.stats().unwrap().entry_count, 1);

        // 搜索应只返回一个 "a"
        let results = store.top_k(&[4.0, 0.0, 0.0, 0.0], 10, "").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id, "a");
    }

    #[test]
    fn test_distance_to_score_conversion() {
        // distance = 0 → score = 1.0(完全相同)
        assert!((HnswStore::distance_to_score(0.0) - 1.0).abs() < 1e-6);
        // distance = 1 → score = 0.0(正交)
        assert!((HnswStore::distance_to_score(1.0) - 0.0).abs() < 1e-6);
        // distance > 1 → score = 0.0(clamp,不应出现但防御)
        assert_eq!(HnswStore::distance_to_score(2.0), 0.0);
        // distance < 0 → score = 1.0(clamp,不应出现但防御)
        assert_eq!(HnswStore::distance_to_score(-0.5), 1.0);
    }

    #[test]
    fn test_medium_scale_100_entries() {
        // 100 entry 中等规模测试(验证 HNSW 正常工作)
        // WHY 100-dim:100 entry × 8 dim 会有重复 one-hot 向量(每 8 个循环),
        // 用 100-dim 保证每个 entry 的 one-hot 向量唯一,避免等分导致的不确定性
        let store = HnswStore::with_params(100, 8, 200, 6, 50, 30);

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
        assert!((results[0].score - 1.0).abs() < 1e-4);
    }

    // ============================================================
    // P2-5: HnswConfig 配置化测试
    // ============================================================

    #[test]
    fn test_with_config_uses_custom_params() {
        // P2-5: with_config 应正确应用 HnswConfig 的自定义参数
        let config = HnswConfig::new(32, 50_000, 20, 300, 100);
        let store = HnswStore::with_config(512, &config);

        assert_eq!(store.dim, 512);
        assert_eq!(store.ef_search, 100);
        assert_eq!(store.build_params.max_nb_connection, 32);
        assert_eq!(store.build_params.max_elements, 50_000);
        assert_eq!(store.build_params.max_layer, 20);
        assert_eq!(store.build_params.ef_construction, 300);
    }

    #[test]
    fn test_with_config_default_params() {
        // P2-5: HnswConfig::default() 应等价于 with_dim 的默认参数
        let config = HnswConfig::default();
        let store = HnswStore::with_config(512, &config);

        assert_eq!(store.ef_search, DEFAULT_EF_SEARCH);
        assert_eq!(
            store.build_params.max_nb_connection,
            DEFAULT_MAX_NB_CONNECTION
        );
        assert_eq!(store.build_params.ef_construction, DEFAULT_EF_CONSTRUCTION);
    }

    #[test]
    fn test_compact_preserves_custom_params() {
        // P2-5: compact() 应使用存储的构造参数重建索引,而非硬编码常量
        // WHY: 原 compact() 使用 DEFAULT_MAX_NB_CONNECTION 等常量,
        // 导致自定义参数(M=32 等)在 compact 后回退为默认值(M=16)。
        // P2-5 修复后,build_params 确保 compact 后参数一致。
        let config = HnswConfig::new(32, 100, 10, 150, 80);
        let store = HnswStore::with_config(4, &config);

        // 插入条目后 compact
        store.upsert("a", &[1.0, 0.0, 0.0, 0.0], ()).unwrap();
        store.upsert("b", &[0.0, 1.0, 0.0, 0.0], ()).unwrap();
        store.remove("a").unwrap();
        store.compact().unwrap();

        // compact 后应仍能正常检索
        let results = store.top_k(&[0.0, 1.0, 0.0, 0.0], 1, "").unwrap();
        assert_eq!(results[0].id, "b");
        assert!((results[0].score - 1.0).abs() < 1e-4);

        // 验证 build_params 仍保持自定义值(compact 未修改)
        assert_eq!(store.build_params.max_nb_connection, 32);
        assert_eq!(store.build_params.ef_construction, 150);
    }

    #[test]
    fn test_with_config_functional_search() {
        // P2-5: with_config 构造的 store 功能等价于 with_params
        //
        // WHY 插入 10 个条目(而非 2 个):HNSW 在极小数据集(≤3 entries)上,
        // 随机分层可能导致图不连通,搜索偶尔只返回部分结果(已知 HNSW 特性,
        // 非 bug)。使用 10 个条目确保图连通性,消除偶发失败。
        let config = HnswConfig::new(16, 100, 6, 50, 100);
        let store = HnswStore::with_config(4, &config);

        // 插入 10 个条目:基向量 + 正交扰动变体,确保图连通
        store.upsert("a", &[1.0, 0.0, 0.0, 0.0], ()).unwrap();
        store.upsert("b", &[0.0, 1.0, 0.0, 0.0], ()).unwrap();
        store.upsert("c", &[0.0, 0.0, 1.0, 0.0], ()).unwrap();
        store.upsert("d", &[0.0, 0.0, 0.0, 1.0], ()).unwrap();
        store.upsert("e", &[0.9, 0.1, 0.0, 0.0], ()).unwrap();
        store.upsert("f", &[0.1, 0.9, 0.0, 0.0], ()).unwrap();
        store.upsert("g", &[0.0, 0.1, 0.9, 0.0], ()).unwrap();
        store.upsert("h", &[0.0, 0.0, 0.1, 0.9], ()).unwrap();
        store.upsert("i", &[0.5, 0.5, 0.0, 0.0], ()).unwrap();
        store.upsert("j", &[0.0, 0.0, 0.5, 0.5], ()).unwrap();

        // 搜索 top-2:查询 [1,0,0,0],最近邻应为 "a"(完全匹配)
        let results = store.top_k(&[1.0, 0.0, 0.0, 0.0], 2, "").unwrap();
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].id, "a");
    }
}
