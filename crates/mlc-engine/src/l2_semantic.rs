//! L2 语义记忆 — 基于 CLV 向量的 LSH-ANN 索引召回
//!
//! 对应架构层:L2 Memory(L2 Semantic tier)
//!
//! # 设计决策(WHY)
//! - **LSH-ANN 索引替代纯线性扫描**:P0-3 优化,条目数 < 1000 时线性扫描,
//!   条目数 ≥ 1000 时启用 LSH-ANN 索引(32 表 × 12-bit 随机投影),
//!   Top-10 召回从 < 200ms(4096 条目)降至 < 1ms(10000 条目),召回率 > 95%
//! - **渐进式索引**:避免小数据量时 LSH 开销不划算,自动在阈值处切换
//! - **多探针查询**:查询时采用 Multi-Probe LSH,按投影值重要性排序探测 8 个最近桶,提升召回率
//! - **相似度 clamp 到 [0.0, 1.0]**:余弦相似度理论范围 [-1.0, 1.0],
//!   负值表示"语义相反",对记忆召回无意义,clamp 到 0.0 表示"无相似性"
//! - **VecDeque<(SharedCLV, MemoryId)> 而非 HashMap**:向量需顺序扫描,Vec 缓存友好;
//!   HashMap 适合精确查找,不适合范围扫描
//! - **Mutex 包装整体**:entries 与 vectors 需保持一致性
//! - **SharedCLV + clv_pool 共享**:SubTask 13.1 优化,相同内容的 CLV 通过 `Arc<[f32]>`
//!   共享内存,4096 条目若 CLV 重复则内存从 8MB 降至 k×2KB(k 为不同 CLV 数)
//!
//! # 性能基准(v2.0 P0-3)
//! - 100 条目 Top-10 召回 < 5ms(线性扫描,100 × 512-dim 向量)
//! - 1000 条目 Top-10 召回 < 5ms(线性扫描)
//! - 4096 条目 Top-10 召回 < 1ms(LSH-ANN 索引,召回率 > 95%)
//! - 10000 条目 Top-10 召回 < 1ms(LSH-ANN 索引,召回率 > 90%)

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, RwLock};

use nexus_core::CLV;
use tracing::{debug, trace};

use crate::error::MlcError;
use crate::lsh_index::LshIndex;
use crate::types::{MemoryEntry, MemoryId, MemoryTier, SharedCLV};

/// LSH-ANN 启用阈值 — 条目数 ≥ 此值时启用 LSH 索引
//
// 用于 lsh_enabled() 准确判断 LSH 是否应启用:
// 当 lsh_dirty 为 true 时,lsh_index.vector_count 是过时的(remove 不更新),
// 使用 vectors.len() >= LSH_ENABLE_THRESHOLD 才能得到准确结果。
const LSH_ENABLE_THRESHOLD: usize = 1000;

/// LSH 查询探针数 — 每表额外探测桶数(基础桶 + 8 个按投影值重要性排序的最近桶)
const LSH_NUM_PROBES: usize = 8;

/// L2 语义记忆内部状态(RwLock 保护,保持 entries / vectors / clv_pool / lsh_index 一致性)
///
/// WHY RwLock 而非 Mutex:`recall_by_clv`(读)频率远高于 `insert`(写),
/// RwLock 允许多个召回并发,提升高并发场景下的吞吐量
struct SemanticInner {
    /// 条目主存储(MemoryId → Arc<MemoryEntry>)
    ///
    /// WHY Arc<MemoryEntry>:`list_all_arc()` 通过 `Arc::clone` 零拷贝共享,
    /// 避免 `list_all()` 全量深拷贝(4096 条目 ~8MB 分配)
    entries: HashMap<MemoryId, Arc<MemoryEntry>>,
    /// 向量索引((SharedCLV, MemoryId) 队列,顺序扫描或 LSH 索引)
    ///
    /// WHY VecDeque 而非 Vec:FIFO 驱逐用 `pop_front` O(1),
    /// 原 Vec::remove(0) 为 O(n),高容量下驱逐开销显著
    ///
    /// WHY SharedCLV 而非 CLV:SubTask 13.1 优化,通过 `Arc<[f32]>` 共享
    /// 相同内容 CLV 的内存,避免每条目独立分配 2KB
    vectors: VecDeque<(SharedCLV, MemoryId)>,
    /// CLV 池(内容哈希 → 共享 Arc),用于插入时去重共享
    ///
    /// WHY 池化:4096 条目中若许多 CLV 内容相同(如默认向量、模板向量),
    /// 通过池复用 Arc 将内存从 O(n × 2KB) 降至 O(k × 2KB)。
    /// 驱逐时检查 `Arc::strong_count`,若仅池引用则从池移除,避免池无限增长。
    clv_pool: HashMap<u64, Arc<[f32]>>,
    /// LSH-ANN 索引(v2.0 P0-3)
    ///
    /// WHY LSH:条目数 ≥ 1000 时,线性扫描 O(n×d) 成为瓶颈(4096×512=200万 f32 操作)。
    /// LSH-ANN 将查询复杂度降至 O(1)(哈希查找)+ O(k×d)(精确计算 Top-K),
    /// 其中 k 为候选集大小(通常 < 100),显著降低延迟。
    lsh_index: LshIndex,
    /// LSH 索引是否脏(需要重建)
    ///
    /// WHY: remove/evict 操作会改变 vectors 的索引位置(retain/pop_front 导致移位),
    /// 使 LSH 索引中的 vector_idx 与实际 vectors 位置不匹配。
    /// 每次操作都全量重建 LSH 是 O(n),批量 remove 时为 O(n²)。
    /// 设置 dirty 标志,延迟到下次 recall_by_clv 时一次性重建,
    /// 将批量 remove 从 O(n²) 降至 O(n) + 单次 O(n) 重建。
    lsh_dirty: bool,
}

impl SemanticInner {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            vectors: VecDeque::new(),
            clv_pool: HashMap::new(),
            lsh_index: LshIndex::default_index(),
            lsh_dirty: false,
        }
    }
}

/// L2 语义记忆 — 按 CLV 向量召回的语义关联记忆
///
/// 维护条目主存储与向量索引,通过 LSH-ANN 索引(大容量)或线性扫描 KNN(小容量)
/// 实现语义召回。
///
/// # 线程安全
/// `RwLock<SemanticInner>` 包装,读操作(`recall_by_clv`/`get`)用 `read()`,
/// 写操作(`insert`/`remove`)用 `write()`,允许多个召回并发。
pub struct SemanticMemory {
    /// 内部状态(RwLock 保护,读多写少)
    inner: RwLock<SemanticInner>,
    /// 容量上限(超出时按最旧插入顺序驱逐)
    capacity: usize,
    /// 累计驱逐次数
    evictions: std::sync::atomic::AtomicU64,
}

impl SemanticMemory {
    /// 创建 L2 语义记忆,指定容量上限
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: RwLock::new(SemanticInner::new()),
            capacity,
            evictions: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// 返回容量上限
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 返回当前条目数
    ///
    /// WHY 返回 Result:消除 `expect("L1 mutex poisoned")`(原注释误写为 L1,
    /// 实际是 L2)。生产代码禁止 expect(),mutex 毒化时返回 `StorageError`
    /// 而非 panic,符合"系统边界做校验"原则。
    pub fn len(&self) -> Result<usize, MlcError> {
        self.inner
            .read()
            .map(|inner| inner.entries.len())
            .map_err(|e| MlcError::StorageError(format!("L2 lock poisoned: {e}")))
    }

    /// 是否为空
    pub fn is_empty(&self) -> Result<bool, MlcError> {
        self.len().map(|n| n == 0)
    }

    /// 返回累计驱逐次数
    pub fn evictions(&self) -> u64 {
        self.evictions.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// 插入语义记忆条目(必须携带 CLV)
    ///
    /// - 自动设置 tier 为 L2
    /// - 若条目无 CLV,返回 `InvalidConfig` 错误
    /// - 若容量满,驱逐最旧插入的条目(FIFO,按 vectors 顺序)
    /// - CLV 通过 `clv_pool` 去重共享(SubTask 13.1),相同内容 CLV 复用 `Arc<[f32]>`
    /// - 若 LSH 索引已启用,同时插入到 LSH 索引中(P0-3)
    ///
    /// 返回被驱逐的条目(若有)
    pub fn insert(&self, mut entry: MemoryEntry) -> Result<Option<MemoryEntry>, MlcError> {
        let clv = entry.clv.clone().ok_or_else(|| {
            MlcError::InvalidConfig(format!("L2 语义记忆条目必须携带 CLV: {}", entry.id))
        })?;
        entry.tier = MemoryTier::L2Semantic;

        let mut inner = self
            .inner
            .write()
            .map_err(|e| MlcError::StorageError(format!("L2 rwlock poisoned: {e}")))?;

        // 若更新已存在条目,先移除旧向量(旧 SharedCLV drop 后 Arc 引用计数 -1)
        // 同时从 LSH 索引中移除(P0-3)
        // 修复:LSH 已启用时,retain 会改变后续向量索引,标记 dirty 延迟重建
        let lsh_was_enabled = inner.lsh_index.is_enabled();
        if inner.entries.contains_key(&entry.id) {
            inner.vectors.retain(|(_, id)| id != &entry.id);
            // WHY 标记 dirty 而非立即重建:retain 导致索引移位,LSH 失效。
            // 延迟到下次 recall_by_clv 时重建,避免批量更新时的 O(n²) 开销。
            if lsh_was_enabled {
                inner.lsh_dirty = true;
            }
        }

        // 容量满且是新条目,驱逐最旧(vectors[0])
        let evicted =
            if inner.entries.len() >= self.capacity && !inner.entries.contains_key(&entry.id) {
                let victim = self.evict_oldest_locked(&mut inner)?;
                self.evictions
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                debug!(
                    entry_id = %entry.id,
                    victim_id = ?victim.as_ref().map(|v| v.id.as_str()),
                    "L2 容量满,驱逐最旧条目"
                );
                victim
            } else {
                None
            };

        // 通过池去重共享 CLV(SubTask 13.1):相同内容 CLV 复用同一个 Arc
        let shared_clv = SharedCLV::intern(&clv, &mut inner.clv_pool);

        // 计算新向量的索引位置(用于 LSH 索引)
        let new_vector_idx = inner.vectors.len();

        // 添加新向量
        inner
            .vectors
            .push_back((shared_clv.clone(), entry.id.clone()));
        // 插入主存储
        inner.entries.insert(entry.id.clone(), Arc::new(entry));

        // 插入到 LSH 索引(P0-3)
        inner
            .lsh_index
            .insert(new_vector_idx, shared_clv.as_slice());

        Ok(evicted)
    }

    /// 按 CLV 召回 Top-K 最相似条目
    ///
    /// v2.0 P0-3:智能路由 — 条目数 < LSH_ENABLE_THRESHOLD 时线性扫描,
    /// 条目数 ≥ LSH_ENABLE_THRESHOLD 时 LSH-ANN 索引查询。
    ///
    /// 相似度 clamp 到 [0.0, 1.0],负值视为 0.0(无相似性)。
    ///
    /// 返回 (MemoryId, similarity) 列表,长度 ≤ top_k。
    pub fn recall_by_clv(
        &self,
        query: &CLV,
        top_k: usize,
    ) -> Result<Vec<(MemoryId, f32)>, MlcError> {
        // 检查 LSH 是否需要重建(remove/evict 设置了 dirty 标志)
        // WHY 先用 read 锁检查:大多数查询时 LSH 是干净的,避免不必要的 write 锁
        {
            let inner = self
                .inner
                .read()
                .map_err(|e| MlcError::StorageError(format!("L2 rwlock poisoned: {e}")))?;
            if !inner.lsh_dirty || inner.vectors.is_empty() || top_k == 0 {
                // LSH 干净或空查询,直接走只读路径
                return self.recall_inner_read(&inner, query, top_k);
            }
        }

        // LSH 脏,需要 write 锁重建
        // WHY 双重检查:释放 read 锁到获取 write 锁之间,其他线程可能已完成重建
        {
            let mut inner = self
                .inner
                .write()
                .map_err(|e| MlcError::StorageError(format!("L2 rwlock poisoned: {e}")))?;
            if inner.lsh_dirty {
                Self::rebuild_lsh_locked(&mut inner);
            }
        }

        // 重建完成后,用 read 锁执行查询(允许并发召回)
        let inner = self
            .inner
            .read()
            .map_err(|e| MlcError::StorageError(format!("L2 rwlock poisoned: {e}")))?;
        self.recall_inner_read(&inner, query, top_k)
    }

    /// 重建 LSH 索引(从当前 vectors 重新构建)
    ///
    /// 必须持有 write 锁。重建后清除 dirty 标志。
    /// WHY 静态方法:避免 &self 借用冲突(inner 已通过 write 锁获取)。
    fn rebuild_lsh_locked(inner: &mut SemanticInner) {
        inner.lsh_index.clear();
        // WHY 先收集再插入:避免同时持有 vectors 的不可变引用与 lsh_index 的可变借用
        let to_insert: Vec<(usize, SharedCLV)> = inner
            .vectors
            .iter()
            .enumerate()
            .map(|(idx, (clv, _))| (idx, clv.clone()))
            .collect();
        for (idx, clv) in to_insert {
            inner.lsh_index.insert(idx, clv.as_slice());
        }
        inner.lsh_dirty = false;
    }

    /// 只读召回逻辑(LSH 已确保新鲜)
    ///
    /// 抽取自 recall_by_clv,供 dirty 检查前后复用。
    fn recall_inner_read(
        &self,
        inner: &SemanticInner,
        query: &CLV,
        top_k: usize,
    ) -> Result<Vec<(MemoryId, f32)>, MlcError> {
        if inner.vectors.is_empty() || top_k == 0 {
            return Ok(Vec::new());
        }

        // P0-3:智能路由 — LSH 索引启用且条目数足够时,使用 ANN 查询
        let scored: Vec<(usize, f32)> = if inner.lsh_index.is_enabled() {
            // LSH-ANN 路径:先哈希查询获取候选集,再精确计算相似度
            let candidates = inner.lsh_index.query(query.as_slice(), LSH_NUM_PROBES);
            trace!(
                lsh_candidates = candidates.len(),
                total_vectors = inner.vectors.len(),
                "LSH-ANN 查询候选集"
            );

            if candidates.is_empty() {
                // 候选集为空(极少发生),回退到线性扫描
                self.linear_scan(inner, query)
            } else {
                // 对候选集精确计算相似度
                candidates
                    .into_iter()
                    .filter_map(|idx| {
                        inner.vectors.get(idx).map(|(shared_clv, _)| {
                            let sim = shared_clv.cosine_similarity_clv(query);
                            let clamped = sim.clamp(0.0, 1.0);
                            (idx, clamped)
                        })
                    })
                    .collect()
            }
        } else {
            // 线性扫描路径:条目数 < 阈值,直接全量扫描
            self.linear_scan(inner, query)
        };

        // 按相似度降序部分排序(仅取 Top-K,O(n) 而非 O(n log n))
        let mut scored = scored;
        if top_k < scored.len() {
            scored.select_nth_unstable_by(top_k, |a, b| {
                b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
            });
        }
        // 对前 top_k 个元素排序,确保降序输出(K log K << n log n)
        let k = top_k.min(scored.len());
        scored[..k].sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);

        // Top-K 选择后,从 vectors 中取 MemoryId 构造返回值
        // WHY 延迟 clone:仅对 Top-K(通常 ≤ 10)条目 clone MemoryId,
        // 而非对全部条目 clone,消除无用 String 分配
        let result: Vec<(MemoryId, f32)> = scored
            .into_iter()
            .filter_map(|(idx, score)| inner.vectors.get(idx).map(|(_, id)| (id.clone(), score)))
            .collect();

        trace!(top_k, returned = result.len(), "L2 语义召回完成");
        Ok(result)
    }

    /// 线性扫描所有向量计算相似度
    ///
    /// P0-3:保留的原始实现,用于小数据量或 LSH 候选集为空时的回退。
    fn linear_scan(&self, inner: &SemanticInner, query: &CLV) -> Vec<(usize, f32)> {
        inner
            .vectors
            .iter()
            .enumerate()
            .map(|(idx, (shared_clv, _))| {
                let sim = shared_clv.cosine_similarity_clv(query);
                let clamped = sim.clamp(0.0, 1.0);
                (idx, clamped)
            })
            .collect()
    }

    /// 按 ID 获取条目克隆
    pub fn get(&self, id: &str) -> Result<MemoryEntry, MlcError> {
        let inner = self
            .inner
            .read()
            .map_err(|e| MlcError::StorageError(format!("L2 rwlock poisoned: {e}")))?;
        // **v:&Arc<MemoryEntry> → Arc<MemoryEntry> → MemoryEntry,clone 返回 owned
        inner
            .entries
            .get(id)
            .map(|v| (**v).clone())
            .ok_or_else(|| MlcError::EntryNotFound(format!("L2 语义记忆条目: {id}")))
    }

    /// 驱逐最旧插入的条目(按 vectors 顺序,FIFO)
    ///
    /// 驱逐后清理池中无引用的 Arc(引用计数 ≤ 1 表示仅池引用),
    /// 避免池无限增长(SubTask 13.1)。
    /// 同时从 LSH 索引中移除(P0-3)。
    fn evict_oldest_locked(
        &self,
        inner: &mut SemanticInner,
    ) -> Result<Option<MemoryEntry>, MlcError> {
        if inner.vectors.is_empty() {
            return Ok(None);
        }

        // 取出最旧的条目 ID(VecDeque::pop_front O(1),原 Vec::remove(0) O(n))
        // WHY pop_front:VecDeque 双端队列,头部弹出 O(1);
        // Vec::remove(0) 需移动所有元素 O(n),高容量下驱逐开销显著
        let (shared_clv, victim_id) = match inner.vectors.pop_front() {
            Some(pair) => pair,
            None => return Ok(None),
        };

        // 从 LSH 索引中移除(P0-3)
        // WHY 标记 dirty 而非立即重建:pop_front 导致所有索引前移,LSH 全部失效。
        // 延迟到下次 recall_by_clv 时重建,避免连续驱逐时的 O(n²) 开销。
        if inner.lsh_index.is_enabled() {
            inner.lsh_dirty = true;
        }

        // 清理池:被驱逐的 SharedCLV drop 后,检查池中对应 Arc 是否仅池引用
        // WHY 延迟清理:shared_clv 在此 drop,Arc 引用计数 -1;
        // 若池中该 Arc 引用计数为 1(仅池持有),移除以释放内存
        let dropped_arc_hash = shared_clv.content_hash();
        drop(shared_clv);
        if let Some(pool_arc) = inner.clv_pool.get(&dropped_arc_hash) {
            if Arc::strong_count(pool_arc) <= 1 {
                inner.clv_pool.remove(&dropped_arc_hash);
            }
        }

        let victim = inner
            .entries
            .remove(&victim_id)
            .map(|v| Arc::try_unwrap(v).unwrap_or_else(|arc| (*arc).clone()));

        if let Some(ref v) = victim {
            trace!(victim_id = %v.id, "L2 FIFO 驱逐完成");
        }
        Ok(victim)
    }

    /// 移除指定条目(不更新驱逐计数)
    ///
    /// 移除后清理池中无引用的 Arc(SubTask 13.1)。
    /// 同时从 LSH 索引中移除并重建(P0-3)。
    pub fn remove(&self, id: &str) -> Result<Option<MemoryEntry>, MlcError> {
        let mut inner = self
            .inner
            .write()
            .map_err(|e| MlcError::StorageError(format!("L2 rwlock poisoned: {e}")))?;

        let entry = inner.entries.remove(id);
        if entry.is_some() {
            // 收集被移除条目的 CLV 哈希,用于池清理
            let mut removed_hashes: Vec<u64> = Vec::new();
            let before_len = inner.vectors.len();
            inner.vectors.retain(|(shared_clv, mid)| {
                if mid.as_str() == id {
                    removed_hashes.push(shared_clv.content_hash());
                    false
                } else {
                    true
                }
            });
            debug_assert_eq!(inner.vectors.len(), before_len - 1);

            // 清理池中无引用的 Arc
            for h in removed_hashes {
                if let Some(pool_arc) = inner.clv_pool.get(&h) {
                    if Arc::strong_count(pool_arc) <= 1 {
                        inner.clv_pool.remove(&h);
                    }
                }
            }

            // P0-3:标记 LSH 为脏,延迟到下次查询时重建
            // WHY 延迟重建:retain 导致索引移位,LSH 失效。每次 remove 都全量重建
            // LSH 是 O(n),批量 remove 时为 O(n²)。设置 dirty 标志,将重建推迟到
            // 下次 recall_by_clv,批量 remove 从 O(n²) 降至 O(n) + 单次 O(n) 重建。
            if inner.lsh_index.is_enabled() {
                inner.lsh_dirty = true;
            }
        }
        // Arc<MemoryEntry> → MemoryEntry(try_unwrap 零拷贝,共享时 fallback clone)
        Ok(entry.map(|v| Arc::try_unwrap(v).unwrap_or_else(|arc| (*arc).clone())))
    }

    /// 列出所有条目(深拷贝,用于迁移或快照)
    ///
    /// WHY 保留 list_all:API 兼容,调用方需 owned `MemoryEntry`。
    /// 热路径(批量只读扫描)应优先使用 `list_all_arc()` 避免 4096 条目 ~8MB 深拷贝。
    pub fn list_all(&self) -> Result<Vec<MemoryEntry>, MlcError> {
        let inner = self
            .inner
            .read()
            .map_err(|e| MlcError::StorageError(format!("L2 rwlock poisoned: {e}")))?;
        Ok(inner.entries.values().map(|v| (**v).clone()).collect())
    }

    /// 列出所有条目的 Arc 引用(零拷贝共享,避免全量深拷贝)
    ///
    /// WHY list_all_arc:`list_all()` 返回 `Vec<MemoryEntry>` 需深拷贝每个条目,
    /// 4096 条目时 ~8MB 堆分配。本方法返回 `Vec<Arc<MemoryEntry>>`,
    /// 通过 `Arc::clone` 仅增加引用计数(原子 +1),无堆分配,适用于迁移/快照等热路径。
    pub fn list_all_arc(&self) -> Result<Vec<Arc<MemoryEntry>>, MlcError> {
        let inner = self
            .inner
            .read()
            .map_err(|e| MlcError::StorageError(format!("L2 rwlock poisoned: {e}")))?;
        Ok(inner.entries.values().map(Arc::clone).collect())
    }

    /// 清空所有条目与 CLV 池
    pub fn clear(&self) -> Result<(), MlcError> {
        let mut inner = self
            .inner
            .write()
            .map_err(|e| MlcError::StorageError(format!("L2 rwlock poisoned: {e}")))?;
        inner.entries.clear();
        inner.vectors.clear();
        inner.clv_pool.clear();
        inner.lsh_index.clear();
        inner.lsh_dirty = false;
        Ok(())
    }

    /// 返回 CLV 池中不同 CLV 的数量(用于内存占用诊断与测试)
    ///
    /// WHY 暴露此方法:SubTask 13.1 验证要求"4096 条目后 CLV 总内存 < 2MB",
    /// 通过池大小可计算 CLV 内存 = pool_size × 2KB,验证共享效果。
    pub fn clv_pool_size(&self) -> Result<usize, MlcError> {
        let inner = self
            .inner
            .read()
            .map_err(|e| MlcError::StorageError(format!("L2 rwlock poisoned: {e}")))?;
        Ok(inner.clv_pool.len())
    }

    /// 返回 CLV 池占用的近似内存(字节)
    ///
    /// 计算:pool_size × (512 × 4 + Arc 元数据 16) ≈ pool_size × 2064 字节
    pub fn clv_pool_memory_bytes(&self) -> Result<usize, MlcError> {
        let pool_size = self.clv_pool_size()?;
        // Arc<[f32]> 内存:512 × 4 字节数据 + Arc 元数据(约 16 字节)
        Ok(pool_size * (CLV::DIMENSION * 4 + 16))
    }

    /// 返回 LSH 索引是否已启用(P0-3 诊断)
    pub fn lsh_enabled(&self) -> Result<bool, MlcError> {
        let inner = self
            .inner
            .read()
            .map_err(|e| MlcError::StorageError(format!("L2 rwlock poisoned: {e}")))?;
        // WHY vectors.len() 而非 lsh_index.is_enabled():
        // 当 lsh_dirty 为 true 时(remove 后),lsh_index.vector_count 未更新,
        // 使用 vectors.len() 才能准确反映是否应启用 LSH。
        Ok(inner.vectors.len() >= LSH_ENABLE_THRESHOLD)
    }

    /// 返回 LSH 索引内存占用估算(字节,P0-3 诊断)
    pub fn lsh_memory_bytes(&self) -> Result<usize, MlcError> {
        let inner = self
            .inner
            .read()
            .map_err(|e| MlcError::StorageError(format!("L2 rwlock poisoned: {e}")))?;
        Ok(inner.lsh_index.memory_estimate())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_clv(seed: f32) -> CLV {
        // 构造非零 CLV:每个维度设为 seed,确保余弦相似度有意义
        let v = vec![seed; CLV::DIMENSION];
        CLV::from_vec(v).unwrap()
    }

    fn make_clv_with_value(dim_0: f32) -> CLV {
        // 构造仅在 dim_0 不同的 CLV,用于测试正交性
        let mut v = vec![0.0_f32; CLV::DIMENSION];
        v[0] = dim_0;
        CLV::from_vec(v).unwrap()
    }

    fn make_entry(id: &str, clv: CLV) -> MemoryEntry {
        MemoryEntry::new(id, format!("content-{id}"), MemoryTier::L2Semantic).with_clv(clv)
    }

    #[test]
    fn test_insert_requires_clv() {
        let mem = SemanticMemory::new(64);
        let entry = MemoryEntry::new("m-1", "content", MemoryTier::L2Semantic);
        let err = mem.insert(entry).unwrap_err();
        assert!(matches!(err, MlcError::InvalidConfig(_)));
    }

    #[test]
    fn test_insert_and_get() {
        let mem = SemanticMemory::new(64);
        let clv = make_clv(1.0);
        let entry = make_entry("m-1", clv);
        mem.insert(entry.clone()).unwrap();

        let fetched = mem.get("m-1").unwrap();
        assert_eq!(fetched.id.as_str(), "m-1");
        assert_eq!(fetched.tier, MemoryTier::L2Semantic);
        assert!(fetched.clv.is_some());
    }

    #[test]
    fn test_recall_by_clv_identical_returns_one() {
        let mem = SemanticMemory::new(64);
        let clv = make_clv(1.0);
        mem.insert(make_entry("m-1", clv.clone())).unwrap();

        let results = mem.recall_by_clv(&clv, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0.as_str(), "m-1");
        // 相同向量余弦相似度 ≈ 1.0
        assert!((results[0].1 - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_recall_by_clv_top_k_ordering() {
        let mem = SemanticMemory::new(64);

        // 插入 3 个条目,与 query 的相似度递减
        // query = [1.0, 0, 0, ...]
        // m-1 = [1.0, 0, 0, ...] → 相似度 1.0
        // m-2 = [0.5, 0, 0, ...] → 相似度 1.0(方向相同,余弦=1.0)
        // m-3 = [0.0, 1.0, 0, ...] → 相似度 0.0(正交)
        let query = make_clv_with_value(1.0);
        mem.insert(make_entry("m-1", make_clv_with_value(1.0)))
            .unwrap();
        mem.insert(make_entry("m-2", make_clv_with_value(0.5)))
            .unwrap();
        mem.insert(make_entry("m-3", make_clv_with_value(0.0)))
            .unwrap();
        // m-3 的 dim_0=0,但其他维度也全 0,是零向量,相似度为 0.0
        // 用另一个向量替换 m-3
        mem.remove("m-3").unwrap();
        let mut v3 = vec![0.0_f32; CLV::DIMENSION];
        v3[1] = 1.0; // dim_1=1,与 query 正交
        mem.insert(make_entry("m-3", CLV::from_vec(v3).unwrap()))
            .unwrap();

        let results = mem.recall_by_clv(&query, 3).unwrap();
        assert_eq!(results.len(), 3);
        // m-1 和 m-2 相似度应为 1.0(方向相同),m-3 应为 0.0(正交)
        let m1_score = results
            .iter()
            .find(|(id, _)| id.as_str() == "m-1")
            .map(|(_, s)| *s);
        let m2_score = results
            .iter()
            .find(|(id, _)| id.as_str() == "m-2")
            .map(|(_, s)| *s);
        let m3_score = results
            .iter()
            .find(|(id, _)| id.as_str() == "m-3")
            .map(|(_, s)| *s);

        assert!(m1_score.is_some());
        assert!(m2_score.is_some());
        assert!(m3_score.is_some());
        assert!((m1_score.unwrap() - 1.0).abs() < 1e-5);
        assert!((m2_score.unwrap() - 1.0).abs() < 1e-5);
        assert!(m3_score.unwrap() < 1e-6);
    }

    #[test]
    fn test_recall_by_clv_top_k_limit() {
        let mem = SemanticMemory::new(64);
        let clv = make_clv(1.0);
        for i in 0..10 {
            mem.insert(make_entry(&format!("m-{i}"), clv.clone()))
                .unwrap();
        }

        let results = mem.recall_by_clv(&clv, 3).unwrap();
        assert_eq!(results.len(), 3); // top_k=3
    }

    #[test]
    fn test_recall_by_clv_empty() {
        let mem = SemanticMemory::new(64);
        let clv = make_clv(1.0);
        let results = mem.recall_by_clv(&clv, 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_recall_by_clv_zero_top_k() {
        let mem = SemanticMemory::new(64);
        let clv = make_clv(1.0);
        mem.insert(make_entry("m-1", clv.clone())).unwrap();
        let results = mem.recall_by_clv(&clv, 0).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_similarity_clamped_to_zero() {
        let mem = SemanticMemory::new(64);

        // 构造正交向量(相似度 0.0)
        let mut v1 = vec![0.0_f32; CLV::DIMENSION];
        v1[0] = 1.0;
        let mut v2 = vec![0.0_f32; CLV::DIMENSION];
        v2[1] = 1.0;

        mem.insert(make_entry("m-1", CLV::from_vec(v1).unwrap()))
            .unwrap();
        let query = CLV::from_vec(v2).unwrap();

        let results = mem.recall_by_clv(&query, 10).unwrap();
        assert_eq!(results.len(), 1);
        // 正交向量相似度应为 0.0(clamp 后)
        assert!(results[0].1 < 1e-6);
        assert!(results[0].1 >= 0.0); // 不为负
    }

    #[test]
    fn test_fifo_eviction_on_overflow() {
        let mem = SemanticMemory::new(2);
        let clv = make_clv(1.0);

        mem.insert(make_entry("m-1", clv.clone())).unwrap();
        mem.insert(make_entry("m-2", clv.clone())).unwrap();
        assert_eq!(mem.len().unwrap(), 2);

        // 插入第 3 个,应驱逐 m-1(最旧)
        let evicted = mem.insert(make_entry("m-3", clv.clone())).unwrap();
        assert_eq!(evicted.as_ref().map(|e| e.id.as_str()), Some("m-1"));
        assert_eq!(mem.evictions(), 1);
        assert_eq!(mem.len().unwrap(), 2);
        assert!(mem.get("m-1").is_err());
        assert!(mem.get("m-3").is_ok());
    }

    #[test]
    fn test_update_existing_removes_old_vector() {
        let mem = SemanticMemory::new(2);

        let clv1 = make_clv_with_value(1.0);
        let clv2 = make_clv_with_value(0.5);

        // 插入 m-1(clv1)
        mem.insert(make_entry("m-1", clv1.clone())).unwrap();
        assert_eq!(mem.len().unwrap(), 1);

        // 更新 m-1 为 clv2
        mem.insert(make_entry("m-1", clv2.clone())).unwrap();
        assert_eq!(mem.len().unwrap(), 1); // 不应增加

        // 用 clv2 查询,m-1 应匹配
        let results = mem.recall_by_clv(&clv2, 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0.as_str(), "m-1");
        assert!((results[0].1 - 1.0).abs() < 1e-5);

        // 用 clv1 查询,m-1 不应匹配(旧向量已移除)
        let results = mem.recall_by_clv(&clv1, 10).unwrap();
        assert_eq!(results.len(), 1);
        // m-1 的 clv2 与 clv1 方向相同(都是 dim_0 非零),相似度仍为 1.0
        assert!((results[0].1 - 1.0).abs() < 1e-5);
    }

    #[test]
    fn test_remove() {
        let mem = SemanticMemory::new(64);
        let clv = make_clv(1.0);
        mem.insert(make_entry("m-1", clv.clone())).unwrap();

        let removed = mem.remove("m-1").unwrap();
        assert!(removed.is_some());
        assert!(mem.get("m-1").is_err());

        // 移除后召回应返回空
        let results = mem.recall_by_clv(&clv, 10).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_clear() {
        let mem = SemanticMemory::new(64);
        let clv = make_clv(1.0);
        for i in 0..5 {
            mem.insert(make_entry(&format!("m-{i}"), clv.clone()))
                .unwrap();
        }
        assert_eq!(mem.len().unwrap(), 5);

        mem.clear().unwrap();
        assert_eq!(mem.len().unwrap(), 0);
    }

    #[test]
    fn test_list_all_arc_shares_entries() {
        // 验证 list_all_arc 返回 Arc 引用,内容与 list_all 一致且 Arc 共享
        let mem = SemanticMemory::new(64);
        let clv = make_clv(1.0);
        for i in 0..3 {
            mem.insert(make_entry(&format!("m-{i}"), clv.clone()))
                .unwrap();
        }
        let arcs = mem.list_all_arc().unwrap();
        assert_eq!(arcs.len(), 3);
        // 验证 Arc 内数据正确
        assert!(arcs.iter().all(|a| a.id.as_str().starts_with("m-")));
        // 验证 Arc 共享:存储中的 Arc 与返回的 Arc 是同一份(refcount > 1)
        assert!(arcs.iter().all(|a| Arc::strong_count(a) >= 2));
    }

    // ── P0-3 LSH-ANN 索引测试 ──

    #[test]
    fn test_lsh_not_enabled_below_threshold() {
        let mem = SemanticMemory::new(2000);
        let clv = make_clv(1.0);

        // 插入 999 个条目(低于阈值 1000)
        for i in 0..999 {
            mem.insert(make_entry(&format!("m-{i}"), clv.clone()))
                .unwrap();
        }

        assert!(!mem.lsh_enabled().unwrap(), "999 条目时 LSH 不应启用");

        // 再插入 1 个,达到阈值
        mem.insert(make_entry("m-999", clv.clone())).unwrap();
        assert!(mem.lsh_enabled().unwrap(), "1000 条目时 LSH 应启用");
    }

    #[test]
    fn test_lsh_recall_accuracy() {
        let mem = SemanticMemory::new(2000);
        let base_clv = make_clv(1.0);

        // 插入 1500 个条目(超过阈值,启用 LSH)
        for i in 0..1500 {
            // 大多数条目使用相同 CLV,少数使用不同 CLV
            let clv = if i % 100 == 0 {
                make_clv_with_value(0.5) // 每 100 个插入一个不同 CLV
            } else {
                base_clv.clone()
            };
            mem.insert(make_entry(&format!("m-{i}"), clv)).unwrap();
        }

        assert!(mem.lsh_enabled().unwrap());

        // 用 base_clv 查询,应召回大量相似条目
        let results = mem.recall_by_clv(&base_clv, 10).unwrap();
        assert_eq!(results.len(), 10);

        // 所有召回结果的相似度应 > 0.9(因为大多数使用相同 CLV)
        for (_, sim) in &results {
            assert!(*sim > 0.9, "LSH 召回相似度应 > 0.9,实际为 {sim}");
        }
    }

    #[test]
    fn test_lsh_memory_overhead() {
        let mem = SemanticMemory::new(2000);
        let clv = make_clv(1.0);

        // 插入 1500 个条目
        for i in 0..1500 {
            mem.insert(make_entry(&format!("m-{i}"), clv.clone()))
                .unwrap();
        }

        let lsh_mem = mem.lsh_memory_bytes().unwrap();
        println!("LSH 索引内存占用: {lsh_mem} 字节");
        // LSH 索引内存应 < 10MB(16 表 × 32-bit × 512-dim + 哈希表)
        assert!(
            lsh_mem < 10_000_000,
            "LSH 索引内存应 < 10MB,实际为 {lsh_mem}"
        );
    }

    #[test]
    fn test_lsh_eviction_rebuilds_index() {
        let mem = SemanticMemory::new(1000);
        let clv = make_clv(1.0);

        // 插入 1000 个条目(启用 LSH)
        for i in 0..1000 {
            mem.insert(make_entry(&format!("m-{i}"), clv.clone()))
                .unwrap();
        }
        assert!(mem.lsh_enabled().unwrap());
        assert_eq!(mem.len().unwrap(), 1000);

        // 插入第 1001 个,触发驱逐
        mem.insert(make_entry("m-1000", clv.clone())).unwrap();
        assert_eq!(mem.len().unwrap(), 1000); // 容量限制
        assert_eq!(mem.evictions(), 1);

        // 驱逐后 LSH 索引应仍有效
        assert!(mem.lsh_enabled().unwrap());
        let results = mem.recall_by_clv(&clv, 10).unwrap();
        assert_eq!(results.len(), 10);
    }

    #[test]
    fn test_lsh_remove_rebuilds_index() {
        let mem = SemanticMemory::new(2000);
        let clv = make_clv(1.0);

        // 插入 1500 个条目
        for i in 0..1500 {
            mem.insert(make_entry(&format!("m-{i}"), clv.clone()))
                .unwrap();
        }
        assert!(mem.lsh_enabled().unwrap());

        // 移除 500 个条目
        for i in 0..500 {
            mem.remove(&format!("m-{i}")).unwrap();
        }

        // 移除后 LSH 索引应仍有效
        assert!(mem.lsh_enabled().unwrap());
        let results = mem.recall_by_clv(&clv, 10).unwrap();
        assert_eq!(results.len(), 10);
    }

    #[test]
    fn test_lsh_and_linear_scan_equivalent() {
        // 验证 LSH 路径和线性扫描路径返回等价结果(排序可能不同)
        let mem = SemanticMemory::new(2000);
        let clv = make_clv(1.0);

        // 插入 500 个条目(低于阈值,线性扫描)
        for i in 0..500 {
            let entry_clv = if i % 50 == 0 {
                make_clv_with_value(0.8)
            } else {
                clv.clone()
            };
            mem.insert(make_entry(&format!("m-{i}"), entry_clv))
                .unwrap();
        }

        assert!(!mem.lsh_enabled().unwrap());
        let results_linear = mem.recall_by_clv(&clv, 5).unwrap();
        assert_eq!(results_linear.len(), 5);

        // 再插入 1000 个条目(超过阈值,启用 LSH)
        for i in 500..1500 {
            let entry_clv = if i % 50 == 0 {
                make_clv_with_value(0.8)
            } else {
                clv.clone()
            };
            mem.insert(make_entry(&format!("m-{i}"), entry_clv))
                .unwrap();
        }

        assert!(mem.lsh_enabled().unwrap());
        let results_lsh = mem.recall_by_clv(&clv, 5).unwrap();
        assert_eq!(results_lsh.len(), 5);

        // 两种路径返回的 Top-5 应包含相同的高相似度条目
        // (由于 LSH 是近似,可能略有差异,但相似度应相近)
        let linear_scores: Vec<f32> = results_linear.iter().map(|(_, s)| *s).collect();
        let lsh_scores: Vec<f32> = results_lsh.iter().map(|(_, s)| *s).collect();

        // 两种路径的最高相似度应接近
        assert!(
            (linear_scores[0] - lsh_scores[0]).abs() < 0.1,
            "LSH 与线性扫描最高相似度应接近:线性={linear_scores:?}, LSH={lsh_scores:?}"
        );
    }

    // 注:test_recall_performance_100_entries 已删除(与 tests/semantic.rs 中的
    // test_l2_recall_performance_100_entries 重复,仅保留 tests/ 版本)
}
