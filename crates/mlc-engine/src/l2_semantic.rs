//! L2 语义记忆 — 基于 CLV 向量的线性扫描 KNN 召回
//!
//! 对应架构层:L2 Memory(L2 Semantic tier)
//!
//! # 设计决策(WHY)
//! - **线性扫描而非 ANN 索引**:100-4096 条目规模下,线性扫描 O(n×d) 足够快
//!   (Top-10 召回 < 5ms),Week 6 后接入 sqlite-vec 提升到 10万级规模
//! - **相似度 clamp 到 [0.0, 1.0]**:余弦相似度理论范围 [-1.0, 1.0],
//!   负值表示"语义相反",对记忆召回无意义,clamp 到 0.0 表示"无相似性"
//! - **VecDeque<(SharedCLV, MemoryId)> 而非 HashMap**:向量需顺序扫描,Vec 缓存友好;
//!   HashMap 适合精确查找,不适合范围扫描
//! - **Mutex 包装整体**:entries 与 vectors 需保持一致性
//! - **SharedCLV + clv_pool 共享**:SubTask 13.1 优化,相同内容的 CLV 通过 `Arc<[f32]>`
//!   共享内存,4096 条目若 CLV 重复则内存从 8MB 降至 k×2KB(k 为不同 CLV 数)
//!
//! # 性能基准
//! - 100 条目 Top-10 召回 < 5ms(线性扫描 100 × 512-dim 向量)
//! - 4096 条目 Top-10 召回 < 200ms(基准测试验证)

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, RwLock};

use nexus_core::CLV;
use tracing::{debug, trace};

use crate::error::MlcError;
use crate::types::{MemoryEntry, MemoryId, MemoryTier, SharedCLV};

/// L2 语义记忆内部状态(RwLock 保护,保持 entries / vectors / clv_pool 一致性)
///
/// WHY RwLock 而非 Mutex:`recall_by_clv`(读)频率远高于 `insert`(写),
/// RwLock 允许多个召回并发,提升高并发场景下的吞吐量
struct SemanticInner {
    /// 条目主存储(MemoryId → MemoryEntry)
    entries: HashMap<MemoryId, MemoryEntry>,
    /// 向量索引((SharedCLV, MemoryId) 队列,顺序扫描)
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
}

impl SemanticInner {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            vectors: VecDeque::new(),
            clv_pool: HashMap::new(),
        }
    }
}

/// L2 语义记忆 — 按 CLV 向量召回的语义关联记忆
///
/// 维护条目主存储与向量索引,通过线性扫描 KNN 实现语义召回。
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
        if inner.entries.contains_key(&entry.id) {
            inner.vectors.retain(|(_, id)| id != &entry.id);
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

        // 添加新向量
        inner.vectors.push_back((shared_clv, entry.id.clone()));
        // 插入主存储
        inner.entries.insert(entry.id.clone(), entry);

        Ok(evicted)
    }

    /// 按 CLV 召回 Top-K 最相似条目
    ///
    /// 线性扫描所有向量,计算余弦相似度,返回 Top-K(按相似度降序)。
    /// 相似度 clamp 到 [0.0, 1.0],负值视为 0.0(无相似性)。
    ///
    /// 返回 (MemoryId, similarity) 列表,长度 ≤ top_k。
    ///
    /// WHY 索引替代 MemoryId clone(SubTask 19.3):原实现评分阶段创建
    /// `Vec<(MemoryId, f32)>`,4096 条目意味着 4096 次 String 堆分配/释放。
    /// 现改为 `Vec<(usize, f32)>` 存储 vectors 索引,Top-K 选择后
    /// 再从 vectors 中取 MemoryId 构造返回值,消除 4096 次 String 分配。
    pub fn recall_by_clv(
        &self,
        query: &CLV,
        top_k: usize,
    ) -> Result<Vec<(MemoryId, f32)>, MlcError> {
        // WHY read() 而非 write():召回是只读操作,RwLock 允许多个召回并发
        let inner = self
            .inner
            .read()
            .map_err(|e| MlcError::StorageError(format!("L2 rwlock poisoned: {e}")))?;

        if inner.vectors.is_empty() || top_k == 0 {
            return Ok(Vec::new());
        }

        // 计算所有向量的相似度,存储 vectors 索引而非 MemoryId
        // WHY 索引:usize 是 Copy 类型,8 字节栈分配,无需堆分配;
        // MemoryId(String) clone 需堆分配,4096 条目 = 4096 次堆分配/释放
        let mut scored: Vec<(usize, f32)> = inner
            .vectors
            .iter()
            .enumerate()
            .map(|(idx, (shared_clv, _))| {
                let sim = shared_clv.cosine_similarity_clv(query);
                // clamp 到 [0.0, 1.0]:负值表示语义相反,对召回无意义
                let clamped = sim.clamp(0.0, 1.0);
                (idx, clamped)
            })
            .collect();

        // 按相似度降序部分排序(仅取 Top-K,O(n) 而非 O(n log n))
        // WHY select_nth_unstable_by:Top-K 召回只需前 K 个最相似元素,
        // 全排序浪费计算。部分排序将第 K 大元素放到正确位置,
        // 前 K 个元素为 Top-K(内部无序),再对前 K 个排序确保降序
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
        // 而非对全部 4096 条目 clone,消除 4086+ 次无用 String 分配
        let result: Vec<(MemoryId, f32)> = scored
            .into_iter()
            .filter_map(|(idx, score)| inner.vectors.get(idx).map(|(_, id)| (id.clone(), score)))
            .collect();

        trace!(top_k, returned = result.len(), "L2 KNN 召回完成");
        Ok(result)
    }

    /// 按 ID 获取条目克隆
    pub fn get(&self, id: &str) -> Result<MemoryEntry, MlcError> {
        let inner = self
            .inner
            .read()
            .map_err(|e| MlcError::StorageError(format!("L2 rwlock poisoned: {e}")))?;
        inner
            .entries
            .get(id)
            .cloned()
            .ok_or_else(|| MlcError::EntryNotFound(format!("L2 语义记忆条目: {id}")))
    }

    /// 驱逐最旧插入的条目(按 vectors 顺序,FIFO)
    ///
    /// 驱逐后清理池中无引用的 Arc(引用计数 ≤ 1 表示仅池引用),
    /// 避免池无限增长(SubTask 13.1)。
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

        let victim = inner.entries.remove(&victim_id);

        if let Some(ref v) = victim {
            trace!(victim_id = %v.id, "L2 FIFO 驱逐完成");
        }
        Ok(victim)
    }

    /// 移除指定条目(不更新驱逐计数)
    ///
    /// 移除后清理池中无引用的 Arc(SubTask 13.1)。
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
        }
        Ok(entry)
    }

    /// 列出所有条目(克隆,用于迁移或快照)
    pub fn list_all(&self) -> Result<Vec<MemoryEntry>, MlcError> {
        let inner = self
            .inner
            .read()
            .map_err(|e| MlcError::StorageError(format!("L2 rwlock poisoned: {e}")))?;
        Ok(inner.entries.values().cloned().collect())
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

    // 注:test_recall_performance_100_entries 已删除(与 tests/semantic.rs 中的
    // test_l2_recall_performance_100_entries 重复,仅保留 tests/ 版本)
}
