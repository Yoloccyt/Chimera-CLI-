//! Hot 层 — 基于 DashMap 的并发安全 LRU 缓存
//!
//! 对应架构层:L3 Storage(Hot tier)
//!
//! # 设计决策(WHY)
//! - **DashMap 而非 `RwLock<HashMap>`**:DashMap 分片锁支持并发读写,
//!   Hot 层是高频访问层(延迟 < 1μs),分片锁减少锁竞争
//! - **LRU 基于逻辑时钟(SubTask 20.2)**:用 `AtomicU64` 单调递增计数器
//!   替代 `last_accessed_at` 墙钟时间作为 LRU 驱逐依据。Windows 系统时钟
//!   精度约 15ms,2ms 内多次操作可能产生相同时间戳,导致 LRU 顺序不确定。
//!   逻辑时钟每次 touch/insert 严格递增,消除墙钟精度依赖。
//! - **get 时更新逻辑时钟**:实现 LRU 语义,最近访问的条目不易被驱逐
//! - **insert 时若满则驱逐**:保证容量不超限,驱逐最久未访问的条目
//! - **insert_preserving 保留原始时间戳**:用于衰减测试与内部迁移,
//!   避免调用 `touch()` 覆盖原始 `last_accessed_at` 与 `access_count`
//!
//! # 性能基准
//! - Hot 层访问延迟 < 1μs(内存操作,DashMap 分片锁)
//! - Hot 层 LRU 驱逐:插入 257 条目后最久未访问的被驱逐(测试验证)

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use dashmap::DashMap;
use tracing::{debug, trace};

use crate::error::CmtError;
use crate::types::{CapabilityEntry, CapabilityId, Tier};

/// Hot 层 — 当前活跃能力的高速缓存
///
/// 基于 `DashMap<CapabilityId, CapabilityEntry>`,支持并发读写。
/// LRU 驱逐策略:容量满时驱逐逻辑时钟值最小(最久未访问)的条目。
///
/// # 线程安全
/// DashMap 内部分片锁支持多线程并发访问,所有方法满足 `Send + Sync`。
/// `get` 方法获取写锁以更新 `last_accessed_at` 与逻辑时钟(LRU 语义所需)。
pub struct HotTier {
    /// 能力条目分片表(CapabilityId → CapabilityEntry)
    entries: DashMap<CapabilityId, CapabilityEntry>,
    /// 容量上限(超出时 LRU 驱逐)
    capacity: usize,
    /// 累计驱逐次数(用于指标上报)
    ///
    /// WHY AtomicU64:HotTier 通过 `&self` 方法共享(线程安全),
    /// 普通字段无法在 `&self` 方法中修改,用原子类型支持并发递增
    evictions: AtomicU64,
    /// 临界区锁:保护 insert/insert_preserving 的"检查 → 驱逐 → 插入"原子性
    ///
    /// WHY Mutex<()>:DashMap 的分片锁无法保证 `len()` 检查与 `insert()` 之间的
    /// 原子性。并发插入时多个线程可能同时通过容量检查,导致超容。
    /// 用 `Mutex<()>` 作为粗粒度临界区保护,简单可靠且不引入新依赖。
    /// Hot 层操作 < 1μs,锁竞争开销可接受(参考 Week 2 DashMap 经验教训)。
    insert_lock: Mutex<()>,
    /// 逻辑时钟(SubTask 20.2):单调递增计数器,替代墙钟时间作为 LRU 依据
    ///
    /// WHY 逻辑时钟:Windows 系统时钟精度约 15ms,2ms 内多次 touch 可能产生
    /// 相同 `last_accessed_at`,导致 `evict_lru` 无法区分先后顺序。
    /// 逻辑时钟每次 `fetch_add` 严格递增,保证 LRU 顺序确定性。
    clock: AtomicU64,
    /// 逻辑访问顺序表(CapabilityId → 逻辑时钟值)
    ///
    /// 与 `entries` 保持一致:insert/get 时更新,remove/clear 时同步清除。
    /// `evict_lru` 通过 `min_by_key` 找到时钟值最小的条目(最久未访问)。
    access_order: DashMap<CapabilityId, u64>,
}

impl HotTier {
    /// 创建 Hot 层,指定容量上限
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: DashMap::with_capacity(capacity),
            capacity,
            evictions: AtomicU64::new(0),
            insert_lock: Mutex::new(()),
            clock: AtomicU64::new(0),
            access_order: DashMap::with_capacity(capacity),
        }
    }

    /// 返回容量上限
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 返回当前条目数
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 返回累计驱逐次数(用于指标上报)
    pub fn evictions(&self) -> u64 {
        self.evictions.load(Ordering::Relaxed)
    }

    /// 插入或更新能力条目
    ///
    /// - 若 `id` 已存在,更新条目(重置 tier 为 Hot)
    /// - 若 `id` 不存在且容量已满,先驱逐最久未访问的条目再插入
    ///
    /// 返回被驱逐的条目(若有),调用方可将其降级到 Warm 层。
    ///
    /// # 并发安全(WHY)
    /// 用 `insert_lock` 保护"检查容量 → 驱逐 → 插入"临界区,避免
    /// check-then-act 竞态导致超容。Hot 层操作 < 1μs,锁开销可接受。
    ///
    /// # 副作用
    /// 调用 `entry.touch()` 更新 `last_accessed_at` 为当前时间,
    /// 并递增 `access_count`(LRU 语义所需)。
    pub fn insert(&self, mut entry: CapabilityEntry) -> Result<Option<CapabilityEntry>, CmtError> {
        // 强制设置 tier 为 Hot(防止上层传入错误层级)
        entry.tier = Tier::Hot;
        entry.touch();

        // 获取临界区锁,保证"检查 → 驱逐 → 插入"原子性
        let _guard = self
            .insert_lock
            .lock()
            .map_err(|e| CmtError::StorageError(format!("Hot 层 insert_lock poisoned: {e}")))?;

        let evicted =
            if !self.entries.contains_key(&entry.id) && self.entries.len() >= self.capacity {
                // 容量满且是新条目,驱逐最久未访问的
                let victim = self.evict_lru()?;
                self.evictions.fetch_add(1, Ordering::Relaxed);
                debug!(
                    entry_id = %entry.id,
                    victim_id = ?victim.as_ref().map(|v| v.id.as_str()),
                    "Hot 层容量满,驱逐最久未访问条目"
                );
                victim
            } else {
                None
            };

        // 记录逻辑时钟值(单调递增,用于 LRU 驱逐排序)
        let tick = self.clock.fetch_add(1, Ordering::Relaxed);
        self.access_order.insert(entry.id.clone(), tick);
        self.entries.insert(entry.id.clone(), entry);
        Ok(evicted)
    }

    /// 插入或更新能力条目(保留原始 last_accessed_at 与 access_count)
    ///
    /// 与 `insert` 方法的区别:不调用 `touch()`,保留条目的原始访问时间戳与计数。
    ///
    /// WHY:用于衰减测试与内部迁移。衰减测试需要插入具有特定 `last_accessed_at`
    /// 的条目,如果调用 `insert` 会覆盖时间戳,导致衰减计算不正确。
    /// 内部迁移(Warm/Cold → Hot)时,条目已经有一个有意义的 `last_accessed_at`,
    /// 不应该被重置。
    ///
    /// 返回被驱逐的条目(若有),调用方可将其降级到 Warm 层。
    pub fn insert_preserving(
        &self,
        mut entry: CapabilityEntry,
    ) -> Result<Option<CapabilityEntry>, CmtError> {
        // 强制设置 tier 为 Hot(防止上层传入错误层级)
        entry.tier = Tier::Hot;

        // 获取临界区锁,保证"检查 → 驱逐 → 插入"原子性(与 insert 共享同一把锁)
        let _guard = self
            .insert_lock
            .lock()
            .map_err(|e| CmtError::StorageError(format!("Hot 层 insert_lock poisoned: {e}")))?;

        let evicted =
            if !self.entries.contains_key(&entry.id) && self.entries.len() >= self.capacity {
                let victim = self.evict_lru()?;
                self.evictions.fetch_add(1, Ordering::Relaxed);
                debug!(
                    entry_id = %entry.id,
                    victim_id = ?victim.as_ref().map(|v| v.id.as_str()),
                    "Hot 层容量满,驱逐最久未访问条目(preserving)"
                );
                victim
            } else {
                None
            };

        // 记录逻辑时钟值(即使保留原始时间戳,LRU 顺序仍按插入时间计算)
        let tick = self.clock.fetch_add(1, Ordering::Relaxed);
        self.access_order.insert(entry.id.clone(), tick);
        self.entries.insert(entry.id.clone(), entry);
        Ok(evicted)
    }

    /// 获取能力条目(更新 last_accessed_at 与逻辑时钟,实现 LRU 语义)
    ///
    /// 返回条目克隆;若不存在返回 `EntryNotFound` 错误。
    ///
    /// WHY 获取写锁:DashMap 的 get_mut 获取分片写锁,更新 last_accessed_at。
    /// 分片锁竞争范围小(仅当前分片),不影响其他分片的并发读写。
    pub fn get(&self, id: &str) -> Result<CapabilityEntry, CmtError> {
        // 先尝试获取写锁更新 last_accessed_at
        if let Some(mut entry) = self.entries.get_mut(id) {
            entry.touch();
            // 更新逻辑时钟(单调递增,保证 LRU 顺序确定性)
            let tick = self.clock.fetch_add(1, Ordering::Relaxed);
            self.access_order.insert(id.into(), tick);
            return Ok(entry.clone());
        }
        Err(CmtError::EntryNotFound(format!("Hot 层能力条目: {id}")))
    }

    /// 尝试获取条目(不更新 last_accessed_at,不返回错误)
    ///
    /// 用于内部检查或不需要 LRU 语义的场景
    pub fn peek(&self, id: &str) -> Option<CapabilityEntry> {
        self.entries.get(id).map(|r| r.clone())
    }

    /// 驱逐最久未访问的条目(LRU)
    ///
    /// 扫描 `access_order` 找到逻辑时钟值最小(最久未访问)的条目并移除。
    /// 256 条目规模下 O(n) 扫描约 1μs,性能可接受。
    ///
    /// WHY 使用逻辑时钟而非 `last_accessed_at`:Windows 系统时钟精度约 15ms,
    /// 短时间内多次操作可能产生相同时间戳,导致 LRU 顺序不确定。
    /// 逻辑时钟严格单调递增,消除墙钟精度依赖(SubTask 20.2)。
    ///
    /// 返回被驱逐的条目;若 Hot 层为空返回 None。
    pub fn evict_lru(&self) -> Result<Option<CapabilityEntry>, CmtError> {
        if self.entries.is_empty() {
            return Ok(None);
        }

        // 扫描 access_order 找逻辑时钟值最小的条目(最久未访问)
        let victim_id = self
            .access_order
            .iter()
            .min_by_key(|r| *r.value())
            .map(|r| r.key().clone());

        let victim = victim_id.and_then(|id| {
            self.access_order.remove(&id);
            self.entries.remove(&id).map(|(_, v)| v)
        });

        if let Some(ref v) = victim {
            trace!(victim_id = %v.id, "Hot 层 LRU 驱逐完成");
        }

        Ok(victim)
    }

    /// 移除指定条目(不更新驱逐计数)
    ///
    /// 用于主动删除或迁移到下层
    pub fn remove(&self, id: &str) -> Option<CapabilityEntry> {
        self.access_order.remove(id);
        self.entries.remove(id).map(|(_, v)| v)
    }

    /// 判断是否包含指定条目
    pub fn contains(&self, id: &str) -> bool {
        self.entries.contains_key(id)
    }

    /// 列出所有条目(克隆,用于迁移或快照)
    pub fn list_all(&self) -> Vec<CapabilityEntry> {
        self.entries.iter().map(|r| r.clone()).collect()
    }

    /// 清空所有条目(不重置驱逐计数)
    pub fn clear(&self) {
        self.access_order.clear();
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(id: &str) -> CapabilityEntry {
        CapabilityEntry::new(id, format!("content-{id}"), Tier::Hot)
    }

    #[test]
    fn test_insert_and_get() {
        let tier = HotTier::new(256);
        let entry = make_entry("cap-1");
        tier.insert(entry.clone()).unwrap();

        let fetched = tier.get("cap-1").unwrap();
        assert_eq!(fetched.id.as_str(), "cap-1");
        assert_eq!(fetched.content, "content-cap-1");
        // insert 时 touch 一次,get 时 touch 一次,共 2 次
        assert_eq!(fetched.access_count, 2);
    }

    #[test]
    fn test_get_nonexistent_returns_error() {
        let tier = HotTier::new(256);
        let result = tier.get("nonexistent");
        assert!(matches!(result, Err(CmtError::EntryNotFound(_))));
    }

    #[test]
    fn test_capacity_and_len() {
        let tier = HotTier::new(256);
        assert_eq!(tier.capacity(), 256);
        assert_eq!(tier.len(), 0);
        assert!(tier.is_empty());

        tier.insert(make_entry("cap-1")).unwrap();
        assert_eq!(tier.len(), 1);
        assert!(!tier.is_empty());
    }

    #[test]
    fn test_lru_eviction_on_overflow() {
        // 容量 2,插入 3 个条目,最久未访问的应被驱逐
        // 逻辑时钟保证 LRU 顺序确定性,无需 thread::sleep(SubTask 20.2)
        let tier = HotTier::new(2);
        tier.insert(make_entry("cap-1")).unwrap();
        tier.insert(make_entry("cap-2")).unwrap();

        // 访问 cap-1,使 cap-2 成为最久未访问(逻辑时钟递增)
        tier.get("cap-1").unwrap();

        // 插入 cap-3,应驱逐 cap-2
        let evicted = tier.insert(make_entry("cap-3")).unwrap();
        assert_eq!(evicted.as_ref().map(|e| e.id.as_str()), Some("cap-2"));
        assert_eq!(tier.evictions(), 1);

        // cap-1 和 cap-3 应存在,cap-2 应被驱逐
        assert!(tier.contains("cap-1"));
        assert!(!tier.contains("cap-2"));
        assert!(tier.contains("cap-3"));
    }

    #[test]
    fn test_lru_eviction_257_entries() {
        // 任务要求:插入 257 条目后最久未访问的被驱逐(容量 256)
        // 逻辑时钟保证 LRU 顺序确定性,无需 thread::sleep(SubTask 20.2)
        let tier = HotTier::new(256);

        // 插入 256 个条目
        for i in 0..256 {
            tier.insert(make_entry(&format!("cap-{i}"))).unwrap();
        }
        assert_eq!(tier.len(), 256);
        assert_eq!(tier.evictions(), 0);

        // 访问 cap-1 到 cap-255,使 cap-0 成为最久未访问(逻辑时钟递增)
        for i in 1..256 {
            tier.get(&format!("cap-{i}")).unwrap();
        }

        // 插入第 257 个条目,应驱逐 cap-0
        let evicted = tier.insert(make_entry("cap-256")).unwrap();
        assert_eq!(evicted.as_ref().map(|e| e.id.as_str()), Some("cap-0"));
        assert_eq!(tier.len(), 256); // 容量不变
        assert_eq!(tier.evictions(), 1);
        assert!(!tier.contains("cap-0"));
        assert!(tier.contains("cap-256"));
    }

    #[test]
    fn test_peek_does_not_update_access_time() {
        let tier = HotTier::new(256);
        let entry = make_entry("cap-1");
        tier.insert(entry).unwrap();

        // 记录 insert 后的 last_accessed_at(insert 时 touch 一次)
        let after_insert = tier.peek("cap-1").unwrap().last_accessed_at;

        // 逻辑时钟替代 thread::sleep:peek 不更新 last_accessed_at 与 access_count,
        // 直接验证字段不变即可,无需等待墙钟时间流逝(SubTask 20.2)
        let peeked = tier.peek("cap-1").unwrap();
        // peek 不更新 last_accessed_at(与 get 区分)
        assert_eq!(peeked.last_accessed_at, after_insert);
        // insert 时 touch 一次,peek 不增加 access_count
        assert_eq!(peeked.access_count, 1);
    }

    #[test]
    fn test_remove() {
        let tier = HotTier::new(256);
        tier.insert(make_entry("cap-1")).unwrap();
        assert!(tier.contains("cap-1"));

        let removed = tier.remove("cap-1");
        assert!(removed.is_some());
        assert!(!tier.contains("cap-1"));
        assert_eq!(tier.len(), 0);
    }

    #[test]
    fn test_clear() {
        let tier = HotTier::new(256);
        for i in 0..5 {
            tier.insert(make_entry(&format!("cap-{i}"))).unwrap();
        }
        assert_eq!(tier.len(), 5);

        tier.clear();
        assert_eq!(tier.len(), 0);
        assert!(tier.is_empty());
    }

    #[test]
    fn test_list_all() {
        let tier = HotTier::new(256);
        for i in 0..3 {
            tier.insert(make_entry(&format!("cap-{i}"))).unwrap();
        }
        let all = tier.list_all();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn test_update_existing_no_eviction() {
        let tier = HotTier::new(2);
        tier.insert(make_entry("cap-1")).unwrap();
        tier.insert(make_entry("cap-2")).unwrap();

        // 更新已存在的 cap-1,不应触发驱逐
        let evicted = tier.insert(make_entry("cap-1")).unwrap();
        assert!(evicted.is_none());
        assert_eq!(tier.len(), 2);
        assert_eq!(tier.evictions(), 0);
    }

    #[test]
    fn test_evict_lru_empty_returns_none() {
        let tier = HotTier::new(256);
        let result = tier.evict_lru().unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_insert_preserving_retains_timestamp() {
        // insert_preserving 不应更新 last_accessed_at 与 access_count
        let tier = HotTier::new(256);
        let mut entry = make_entry("cap-1");
        entry.last_accessed_at = chrono::Utc::now() - chrono::Duration::hours(72);
        entry.access_count = 5;

        tier.insert_preserving(entry).unwrap();

        let peeked = tier.peek("cap-1").unwrap();
        // access_count 应保持为 5,不应被 touch 递增
        assert_eq!(peeked.access_count, 5);
        // last_accessed_at 应保持为 72 小时前(近似验证)
        let delta = chrono::Utc::now().signed_duration_since(peeked.last_accessed_at);
        assert!(
            delta.num_hours() >= 71,
            "last_accessed_at 应保留为 72 小时前"
        );
    }
}
