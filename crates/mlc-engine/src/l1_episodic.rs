//! L1 情节记忆 — 按时间与 Quest 索引的执行历史
//!
//! 对应架构层:L2 Memory(L1 Episodic tier)
//!
//! # 设计决策(WHY)
//! - **BTreeMap 时间索引**:按 `created_at` 排序,支持高效的范围查询
//!   (`query_range`),BTreeMap 的 range 查询为 O(log n + k)
//! - **HashMap Quest 索引**:按 Quest ID 关联,支持 O(1) 查找某 Quest 的所有记忆
//! - **RwLock 包装整体**:三个索引(entries/time_index/quest_index)需保持一致性,
//!   用单一 RwLock 包装避免分布式锁的复杂性,1024 条目规模下锁竞争可接受。
//!   读操作(get/query_range/query_by_quest/list_all/len)用 read() 允许多并发,
//!   写操作(insert/remove/clear)用 write() 独占
//! - **FIFO 驱逐**:按 `created_at` 最旧的条目驱逐,与 L0 的 LRU 区分
//!   (情节记忆按时间顺序,不按访问频率)
//!
//! # 性能基准
//! - 时间范围查询:1024 条目规模下 < 1ms(BTreeMap range)
//! - Quest 关联查询:O(1) HashMap 查找

use std::collections::{BTreeMap, HashMap};
use std::sync::RwLock;

use chrono::{DateTime, Utc};
use tracing::{debug, trace};

use crate::error::MlcError;
use crate::types::{MemoryEntry, MemoryId, MemoryTier, QuestId};

/// L1 情节记忆内部状态(RwLock 保护,保持三索引一致性)
struct EpisodicInner {
    /// 条目主存储(MemoryId → MemoryEntry)
    entries: HashMap<MemoryId, MemoryEntry>,
    /// 时间索引(created_at → Vec<MemoryId>),支持范围查询
    ///
    /// WHY Vec<MemoryId>:同一时间戳可能插入多个条目(并发场景),
    /// 用 Vec 聚合同一时刻的多个条目
    time_index: BTreeMap<DateTime<Utc>, Vec<MemoryId>>,
    /// Quest 索引(QuestId → Vec<MemoryId>),支持 Quest 关联查询
    quest_index: HashMap<QuestId, Vec<MemoryId>>,
}

impl EpisodicInner {
    fn new() -> Self {
        Self {
            entries: HashMap::new(),
            time_index: BTreeMap::new(),
            quest_index: HashMap::new(),
        }
    }
}

/// L1 情节记忆 — 按时间与 Quest 索引的执行历史
///
/// 维护三个索引:条目主存储、时间索引、Quest 索引,
/// 通过单一 RwLock 保证三索引一致性。
///
/// # 线程安全
/// `RwLock<EpisodicInner>` 包装,所有方法满足 `Send + Sync`。
/// 读操作用 `read()`(允许多并发),写操作用 `write()`(独占)。
/// 锁粒度为整个 L1,1024 条目规模下锁竞争可接受。
pub struct EpisodicMemory {
    /// 内部状态(RwLock 保护)
    inner: RwLock<EpisodicInner>,
    /// 容量上限(超出时 FIFO 驱逐)
    capacity: usize,
    /// 累计驱逐次数(用于指标上报)
    evictions: std::sync::atomic::AtomicU64,
}

impl EpisodicMemory {
    /// 创建 L1 情节记忆,指定容量上限
    pub fn new(capacity: usize) -> Self {
        Self {
            inner: RwLock::new(EpisodicInner::new()),
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
    /// WHY 返回 Result:RwLock lock 失败(poison)不可恢复,需向上传播而非 panic
    pub fn len(&self) -> Result<usize, MlcError> {
        let inner = self
            .inner
            .read()
            .map_err(|e| MlcError::StorageError(format!("L1 rwlock poisoned: {e}")))?;
        Ok(inner.entries.len())
    }

    /// 是否为空
    pub fn is_empty(&self) -> Result<bool, MlcError> {
        Ok(self.len()? == 0)
    }

    /// 返回累计驱逐次数
    pub fn evictions(&self) -> u64 {
        self.evictions.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// 插入情节记忆条目
    ///
    /// - 自动设置 tier 为 L1
    /// - 更新时间索引(按 created_at)
    /// - 若有 quest_id,更新 Quest 索引
    /// - 若容量满,先 FIFO 驱逐最旧条目
    ///
    /// 返回被驱逐的条目(若有),调用方可将其降级到 L2
    pub fn insert(&self, mut entry: MemoryEntry) -> Result<Option<MemoryEntry>, MlcError> {
        entry.tier = MemoryTier::L1Episodic;

        let mut inner = self
            .inner
            .write()
            .map_err(|e| MlcError::StorageError(format!("L1 rwlock poisoned: {e}")))?;

        let evicted =
            if inner.entries.len() >= self.capacity && !inner.entries.contains_key(&entry.id) {
                // 容量满且是新条目,FIFO 驱逐最旧
                let victim = self.evict_fifo_locked(&mut inner)?;
                self.evictions
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                debug!(
                    entry_id = %entry.id,
                    victim_id = ?victim.as_ref().map(|v| v.id.as_str()),
                    "L1 容量满,FIFO 驱逐最旧条目"
                );
                victim
            } else {
                None
            };

        // 更新 Quest 索引(若有 quest_id)
        if let Some(qid) = &entry.quest_id {
            inner
                .quest_index
                .entry(qid.clone())
                .or_default()
                .push(entry.id.clone());
        }

        // 更新时间索引
        inner
            .time_index
            .entry(entry.created_at)
            .or_default()
            .push(entry.id.clone());

        // 插入主存储
        inner.entries.insert(entry.id.clone(), entry);

        Ok(evicted)
    }

    /// 按 ID 获取条目克隆(不更新访问时间,L1 按 FIFO 不按 LRU)
    pub fn get(&self, id: &str) -> Result<MemoryEntry, MlcError> {
        let inner = self
            .inner
            .read()
            .map_err(|e| MlcError::StorageError(format!("L1 rwlock poisoned: {e}")))?;
        inner
            .entries
            .get(id)
            .cloned()
            .ok_or_else(|| MlcError::EntryNotFound(format!("L1 情节记忆条目: {id}")))
    }

    /// 按时间范围查询条目 [start, end)
    ///
    /// 返回时间戳在 `[start, end)` 范围内的所有条目(按时间升序)。
    /// 利用 BTreeMap 的 range 查询,复杂度 O(log n + k)。
    pub fn query_range(
        &self,
        start: DateTime<Utc>,
        end: DateTime<Utc>,
    ) -> Result<Vec<MemoryEntry>, MlcError> {
        let inner = self
            .inner
            .read()
            .map_err(|e| MlcError::StorageError(format!("L1 rwlock poisoned: {e}")))?;

        let mut results = Vec::new();
        // BTreeMap::range 是半开区间 [start, end)
        for (_ts, ids) in inner.time_index.range(start..end) {
            for id in ids {
                if let Some(entry) = inner.entries.get(id) {
                    results.push(entry.clone());
                }
            }
        }
        Ok(results)
    }

    /// 按 Quest ID 查询关联的所有条目
    ///
    /// 返回该 Quest 的所有情节记忆(按插入顺序)。
    /// 利用 HashMap 的 O(1) 查找。
    pub fn query_by_quest(&self, quest_id: &str) -> Result<Vec<MemoryEntry>, MlcError> {
        let inner = self
            .inner
            .read()
            .map_err(|e| MlcError::StorageError(format!("L1 rwlock poisoned: {e}")))?;

        let ids = inner.quest_index.get(quest_id).cloned().unwrap_or_default();
        let mut results = Vec::with_capacity(ids.len());
        for id in &ids {
            if let Some(entry) = inner.entries.get(id) {
                results.push(entry.clone());
            }
        }
        Ok(results)
    }

    /// FIFO 驱逐最旧条目(按 created_at 排序)
    ///
    /// 在已持有锁的情况下调用(内部方法)。
    /// 返回被驱逐的条目;若记忆为空返回 None。
    fn evict_fifo_locked(
        &self,
        inner: &mut EpisodicInner,
    ) -> Result<Option<MemoryEntry>, MlcError> {
        // 找到时间索引中最旧的时间戳及其第一个条目 ID
        let oldest = inner
            .time_index
            .iter()
            .next()
            .map(|(ts, ids)| (*ts, ids.first().cloned()));

        let (ts, victim_id) = match oldest {
            Some((ts, Some(id))) => (ts, id),
            _ => return Ok(None),
        };

        // 从主存储移除
        let victim = inner.entries.remove(&victim_id);

        // 从时间索引移除
        if let Some(vec) = inner.time_index.get_mut(&ts) {
            vec.retain(|id| id != &victim_id);
            if vec.is_empty() {
                inner.time_index.remove(&ts);
            }
        }

        // 从 Quest 索引移除
        if let Some(ref v) = victim {
            if let Some(qid) = &v.quest_id {
                if let Some(vec) = inner.quest_index.get_mut(qid) {
                    vec.retain(|id| id != &victim_id);
                    if vec.is_empty() {
                        inner.quest_index.remove(qid);
                    }
                }
            }
            trace!(victim_id = %v.id, "L1 FIFO 驱逐完成");
        }

        Ok(victim)
    }

    /// 移除指定条目(不更新驱逐计数)
    pub fn remove(&self, id: &str) -> Result<Option<MemoryEntry>, MlcError> {
        let mut inner = self
            .inner
            .write()
            .map_err(|e| MlcError::StorageError(format!("L1 rwlock poisoned: {e}")))?;

        let entry = inner.entries.remove(id);
        if let Some(ref e) = entry {
            // 从时间索引移除
            if let Some(vec) = inner.time_index.get_mut(&e.created_at) {
                vec.retain(|mid| mid.as_str() != id);
                if vec.is_empty() {
                    inner.time_index.remove(&e.created_at);
                }
            }
            // 从 Quest 索引移除
            if let Some(qid) = &e.quest_id {
                if let Some(vec) = inner.quest_index.get_mut(qid) {
                    vec.retain(|mid| mid.as_str() != id);
                    if vec.is_empty() {
                        inner.quest_index.remove(qid);
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
            .map_err(|e| MlcError::StorageError(format!("L1 rwlock poisoned: {e}")))?;
        Ok(inner.entries.values().cloned().collect())
    }

    /// 清空所有条目
    pub fn clear(&self) -> Result<(), MlcError> {
        let mut inner = self
            .inner
            .write()
            .map_err(|e| MlcError::StorageError(format!("L1 rwlock poisoned: {e}")))?;
        inner.entries.clear();
        inner.time_index.clear();
        inner.quest_index.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn make_entry(id: &str, ts: DateTime<Utc>) -> MemoryEntry {
        let mut entry = MemoryEntry::new(id, format!("content-{id}"), MemoryTier::L1Episodic);
        entry.created_at = ts;
        entry.last_accessed_at = ts;
        entry
    }

    fn make_quest_entry(id: &str, ts: DateTime<Utc>, quest_id: &str) -> MemoryEntry {
        make_entry(id, ts).with_quest(quest_id)
    }

    #[test]
    fn test_insert_and_get() {
        let mem = EpisodicMemory::new(1024);
        let now = Utc::now();
        let entry = make_entry("m-1", now);
        mem.insert(entry.clone()).unwrap();

        let fetched = mem.get("m-1").unwrap();
        assert_eq!(fetched.id.as_str(), "m-1");
        assert_eq!(fetched.tier, MemoryTier::L1Episodic);
    }

    #[test]
    fn test_get_nonexistent_returns_error() {
        let mem = EpisodicMemory::new(1024);
        let result = mem.get("nonexistent");
        assert!(matches!(result, Err(MlcError::EntryNotFound(_))));
    }

    #[test]
    fn test_query_range() {
        let mem = EpisodicMemory::new(1024);
        let base = Utc::now();

        // 插入 3 个条目,时间戳分别为 base, base+1s, base+2s
        mem.insert(make_entry("m-1", base)).unwrap();
        mem.insert(make_entry("m-2", base + Duration::seconds(1)))
            .unwrap();
        mem.insert(make_entry("m-3", base + Duration::seconds(2)))
            .unwrap();

        // 查询 [base, base+2s) 应返回 m-1 和 m-2
        let results = mem.query_range(base, base + Duration::seconds(2)).unwrap();
        assert_eq!(results.len(), 2);
        assert!(results.iter().any(|e| e.id.as_str() == "m-1"));
        assert!(results.iter().any(|e| e.id.as_str() == "m-2"));
        assert!(!results.iter().any(|e| e.id.as_str() == "m-3"));
    }

    #[test]
    fn test_query_range_empty() {
        let mem = EpisodicMemory::new(1024);
        let now = Utc::now();
        let results = mem.query_range(now, now + Duration::seconds(60)).unwrap();
        assert!(results.is_empty());
    }

    #[test]
    fn test_query_by_quest() {
        let mem = EpisodicMemory::new(1024);
        let now = Utc::now();

        // 插入 3 个条目,2 个属于 quest-A,1 个属于 quest-B
        mem.insert(make_quest_entry("m-1", now, "quest-A")).unwrap();
        mem.insert(make_quest_entry("m-2", now, "quest-A")).unwrap();
        mem.insert(make_quest_entry("m-3", now, "quest-B")).unwrap();

        let quest_a = mem.query_by_quest("quest-A").unwrap();
        assert_eq!(quest_a.len(), 2);
        assert!(quest_a
            .iter()
            .all(|e| e.quest_id.as_deref() == Some("quest-A")));

        let quest_b = mem.query_by_quest("quest-B").unwrap();
        assert_eq!(quest_b.len(), 1);

        let quest_c = mem.query_by_quest("quest-C").unwrap();
        assert!(quest_c.is_empty());
    }

    #[test]
    fn test_fifo_eviction_on_overflow() {
        let mem = EpisodicMemory::new(2);
        let base = Utc::now();

        // 插入 2 个条目
        mem.insert(make_entry("m-1", base)).unwrap();
        mem.insert(make_entry("m-2", base + Duration::seconds(1)))
            .unwrap();
        assert_eq!(mem.len().unwrap(), 2);

        // 插入第 3 个条目,应驱逐 m-1(最旧)
        let evicted = mem
            .insert(make_entry("m-3", base + Duration::seconds(2)))
            .unwrap();
        assert_eq!(evicted.as_ref().map(|e| e.id.as_str()), Some("m-1"));
        assert_eq!(mem.evictions(), 1);
        assert_eq!(mem.len().unwrap(), 2);

        // m-1 被驱逐,m-2 和 m-3 存在
        assert!(mem.get("m-1").is_err());
        assert!(mem.get("m-2").is_ok());
        assert!(mem.get("m-3").is_ok());
    }

    #[test]
    fn test_fifo_eviction_preserves_quest_index() {
        let mem = EpisodicMemory::new(2);
        let base = Utc::now();

        // 插入 2 个 quest-A 的条目
        mem.insert(make_quest_entry("m-1", base, "quest-A"))
            .unwrap();
        mem.insert(make_quest_entry(
            "m-2",
            base + Duration::seconds(1),
            "quest-A",
        ))
        .unwrap();

        // 插入第 3 个条目,驱逐 m-1
        mem.insert(make_entry("m-3", base + Duration::seconds(2)))
            .unwrap();

        // quest-A 应只剩 m-2
        let quest_a = mem.query_by_quest("quest-A").unwrap();
        assert_eq!(quest_a.len(), 1);
        assert_eq!(quest_a[0].id.as_str(), "m-2");
    }

    #[test]
    fn test_remove() {
        let mem = EpisodicMemory::new(1024);
        let now = Utc::now();
        mem.insert(make_quest_entry("m-1", now, "quest-A")).unwrap();

        let removed = mem.remove("m-1").unwrap();
        assert!(removed.is_some());

        // 移除后 Quest 索引也应清理
        let quest_a = mem.query_by_quest("quest-A").unwrap();
        assert!(quest_a.is_empty());
    }

    #[test]
    fn test_clear() {
        let mem = EpisodicMemory::new(1024);
        let now = Utc::now();
        for i in 0..5 {
            mem.insert(make_entry(&format!("m-{i}"), now)).unwrap();
        }
        assert_eq!(mem.len().unwrap(), 5);

        mem.clear().unwrap();
        assert_eq!(mem.len().unwrap(), 0);
    }

    #[test]
    fn test_list_all() {
        let mem = EpisodicMemory::new(1024);
        let now = Utc::now();
        for i in 0..3 {
            mem.insert(make_entry(&format!("m-{i}"), now)).unwrap();
        }
        let all = mem.list_all().unwrap();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn test_update_existing_no_eviction() {
        let mem = EpisodicMemory::new(2);
        let now = Utc::now();
        mem.insert(make_entry("m-1", now)).unwrap();
        mem.insert(make_entry("m-2", now)).unwrap();

        // 更新已存在的 m-1,不应触发驱逐
        let evicted = mem.insert(make_entry("m-1", now)).unwrap();
        assert!(evicted.is_none());
        assert_eq!(mem.len().unwrap(), 2);
    }
}
