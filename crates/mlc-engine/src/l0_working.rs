//! L0 工作记忆 — 基于 DashMap 的并发安全 LRU 缓存
//!
//! 对应架构层:L2 Memory(L0 Working tier)
//!
//! # 设计决策(WHY)
//! - **DashMap 而非 `RwLock<HashMap>`**:DashMap 分片锁支持并发读写,
//!   工作记忆是高频访问层(延迟 < 1μs),分片锁减少锁竞争
//! - **O(1) LRU 链表(SubTask 13.2)**:维护 `LruList`(手动双向链表 + HashMap 索引),
//!   `evict_lru`/`touch`/`remove` 均为 O(1),原 O(n) 扫描在 64 条目下约 10μs,
//!   O(1) 化后降至 < 1μs
//! - **get 时更新 LRU 链表**:实现 LRU 语义,最近访问的条目移到链表尾部(MRU 端),
//!   驱逐时从链表头部(LRU 端)弹出
//! - **insert 时若满则驱逐**:保证容量不超限,驱逐最久未访问的条目
//!
//! # 性能基准
//! - L0 访问延迟 < 1μs(内存操作,DashMap 分片锁)
//! - L0 LRU 驱逐:O(1),64 条目驱逐延迟 < 1μs(基准测试验证)

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use dashmap::mapref::entry::Entry;
use dashmap::DashMap;
use tracing::{debug, trace};

use crate::error::MlcError;
use crate::types::{MemoryEntry, MemoryId, MemoryTier};

/// LRU 双向链表节点
struct LruNode {
    /// 节点对应的条目 ID
    id: MemoryId,
    /// 前驱节点索引(LRU 方向),head 节点为 None
    prev: Option<usize>,
    /// 后继节点索引(MRU 方向),tail 节点为 None
    next: Option<usize>,
}

/// O(1) LRU 链表 — 手动双向链表 + HashMap 索引
///
/// # 数据结构
/// - `nodes: Vec<Option<LruNode>>`:节点存储,用 Option 区分活跃/空闲槽位
/// - `free_list: Vec<usize>`:回收的节点索引,复用避免 Vec 无限增长
/// - `index: HashMap<MemoryId, usize>`:ID → 节点索引,O(1) 查找
/// - `head`:LRU 端(最久未访问),驱逐时弹出
/// - `tail`:MRU 端(最近访问),touch/insert 时追加
///
/// # 复杂度
/// - `push_back`:O(1)
/// - `touch`(move_to_tail):O(1)
/// - `pop_front`:O(1)
/// - `remove`:O(1)
///
/// WHY 手动实现而非 `linked_hash_map` crate:workspace 未收录该 crate,
/// 用 std 集合手动实现避免引入新依赖,且可控性更强。
struct LruList {
    /// 节点存储(Some = 活跃,None = 空闲槽位)
    nodes: Vec<Option<LruNode>>,
    /// LRU 端(最久未访问),驱逐时从此端弹出
    head: Option<usize>,
    /// MRU 端(最近访问),touch/insert 时追加到此端
    tail: Option<usize>,
    /// 回收的空闲槽位索引,复用避免 Vec 无限增长
    free_list: Vec<usize>,
    /// ID → 节点索引,O(1) 查找
    index: HashMap<MemoryId, usize>,
}

impl LruList {
    fn new() -> Self {
        Self {
            nodes: Vec::new(),
            head: None,
            tail: None,
            free_list: Vec::new(),
            index: HashMap::new(),
        }
    }

    #[allow(dead_code)]
    fn len(&self) -> usize {
        self.index.len()
    }

    #[allow(dead_code)]
    fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    #[allow(dead_code)]
    fn contains(&self, id: &str) -> bool {
        self.index.contains_key(id)
    }

    /// 在 MRU 端(尾部)插入节点
    ///
    /// 若 ID 已存在则不重复插入(调用方应先 touch 或 remove)。
    fn push_back(&mut self, id: MemoryId) {
        if self.index.contains_key(&id) {
            return;
        }
        let node = LruNode {
            id: id.clone(),
            prev: self.tail,
            next: None,
        };
        let idx = self.alloc_node(node);
        if let Some(old_tail) = self.tail {
            if let Some(n) = self.nodes[old_tail].as_mut() {
                n.next = Some(idx);
            }
        }
        self.tail = Some(idx);
        if self.head.is_none() {
            self.head = Some(idx);
        }
        self.index.insert(id, idx);
    }

    /// 将节点移动到 MRU 端(尾部)
    ///
    /// 若 ID 不存在则 no-op;若已是尾部则无需移动。
    fn touch(&mut self, id: &str) {
        let idx = match self.index.get(id) {
            Some(&i) => i,
            None => return,
        };
        if self.tail == Some(idx) {
            return;
        }
        self.detach(idx);
        self.attach_to_tail(idx);
    }

    /// 从 LRU 端(头部)弹出最久未访问的节点 ID
    fn pop_front(&mut self) -> Option<MemoryId> {
        let idx = self.head?;
        let node = self.nodes[idx].take()?;
        self.index.remove(&node.id);
        self.free_list.push(idx);
        self.head = node.next;
        match self.head {
            Some(new_head) => {
                if let Some(n) = self.nodes[new_head].as_mut() {
                    n.prev = None;
                }
            }
            None => self.tail = None,
        }
        Some(node.id)
    }

    /// 移除指定 ID 的节点
    fn remove(&mut self, id: &str) -> Option<MemoryId> {
        let idx = self.index.remove(id)?;
        self.detach(idx);
        let node = self.nodes[idx].take()?;
        self.free_list.push(idx);
        Some(node.id)
    }

    fn clear(&mut self) {
        self.nodes.clear();
        self.free_list.clear();
        self.index.clear();
        self.head = None;
        self.tail = None;
    }

    /// 分配节点槽位(优先复用 free_list,否则 push)
    fn alloc_node(&mut self, node: LruNode) -> usize {
        if let Some(idx) = self.free_list.pop() {
            self.nodes[idx] = Some(node);
            idx
        } else {
            self.nodes.push(Some(node));
            self.nodes.len() - 1
        }
    }

    /// 断开节点 idx 与前后驱的连接
    fn detach(&mut self, idx: usize) {
        let (prev, next) = {
            let node = match self.nodes[idx].as_ref() {
                Some(n) => n,
                None => return,
            };
            (node.prev, node.next)
        };
        match prev {
            Some(p) => {
                if let Some(n) = self.nodes[p].as_mut() {
                    n.next = next;
                }
            }
            None => self.head = next,
        }
        match next {
            Some(n) => {
                if let Some(n2) = self.nodes[n].as_mut() {
                    n2.prev = prev;
                }
            }
            None => self.tail = prev,
        }
        if let Some(node) = self.nodes[idx].as_mut() {
            node.prev = None;
            node.next = None;
        }
    }

    /// 将节点 idx 接到 MRU 端(尾部)
    fn attach_to_tail(&mut self, idx: usize) {
        if let Some(node) = self.nodes[idx].as_mut() {
            node.prev = self.tail;
            node.next = None;
        }
        if let Some(old_tail) = self.tail {
            if let Some(n) = self.nodes[old_tail].as_mut() {
                n.next = Some(idx);
            }
        }
        self.tail = Some(idx);
        if self.head.is_none() {
            self.head = Some(idx);
        }
    }
}

/// L0 工作记忆 — 当前活跃上下文的高速缓存
///
/// 基于 `DashMap<MemoryId, MemoryEntry>` 支持并发读写,
/// 辅以 `Mutex<LruList>` 实现 O(1) LRU 驱逐(SubTask 13.2)。
///
/// # 线程安全
/// DashMap 内部分片锁支持多线程并发访问,所有方法满足 `Send + Sync`。
/// `get` 方法获取写锁以更新 `last_accessed_at`(LRU 语义所需),
/// 同时锁 LruList 将节点移到 MRU 端。
pub struct WorkingMemory {
    /// 记忆条目分片表(MemoryId → Arc<MemoryEntry>)
    ///
    /// WHY Arc<MemoryEntry> 而非 MemoryEntry:`list_all_arc()` 通过 `Arc::clone`
    /// 共享条目(仅原子计数+1),避免 `list_all()` 全量深拷贝(4096 条目 ~8MB)。
    /// `get()`/`peek()` 等 API 仍返回 owned `MemoryEntry`(clone 出 Arc),保持兼容。
    entries: DashMap<MemoryId, Arc<MemoryEntry>>,
    /// O(1) LRU 索引(SubTask 13.2),与 entries 保持一致
    ///
    /// WHY Mutex 而非 DashMap:LruList 是单一链表结构,需整体操作,
    /// 分片锁无意义;64 条目规模下 Mutex 竞争可忽略
    lru: Mutex<LruList>,
    /// 容量上限(超出时 LRU 驱逐)
    capacity: usize,
    /// 累计驱逐次数(用于指标上报)
    ///
    /// WHY AtomicU64:WorkingMemory 通过 `&self` 方法共享(线程安全),
    /// 普通字段无法在 `&self` 方法中修改,用原子类型支持并发递增
    evictions: AtomicU64,
}

impl WorkingMemory {
    /// 创建 L0 工作记忆,指定容量上限
    pub fn new(capacity: usize) -> Self {
        Self {
            entries: DashMap::with_capacity(capacity),
            lru: Mutex::new(LruList::new()),
            capacity,
            evictions: AtomicU64::new(0),
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

    /// 插入或更新记忆条目
    ///
    /// - 若 `id` 已存在,更新条目(重置 tier 为 L0),并 touch LRU(移到 MRU 端)
    /// - 若 `id` 不存在且容量已满,先驱逐 LRU 端条目再插入
    /// - 新条目追加到 LRU 的 MRU 端
    ///
    /// 返回被驱逐的条目(若有),调用方可将其降级到 L1
    ///
    /// # 并发安全(SubTask 18.4)
    /// 使用 `DashMap::entry()` 原子性 check-then-insert,消除原 `contains_key` +
    /// `insert` 两步操作间的 TOCTOU 窗口(并发插入同 id 导致重复驱逐与丢失更新)。
    pub fn insert(&self, mut entry: MemoryEntry) -> Result<Option<MemoryEntry>, MlcError> {
        // 强制设置 tier 为 L0(防止上层传入错误层级)
        entry.tier = MemoryTier::L0Working;
        entry.touch();
        let id = entry.id.clone();

        // SubTask 18.4:DashMap::entry() 占用 shard 写锁,保证 check-then-insert 原子性
        match self.entries.entry(id.clone()) {
            Entry::Occupied(mut occupied) => {
                // 条目已存在:原子更新,无需驱逐
                occupied.insert(Arc::new(entry));
                drop(occupied);

                // 更新 LRU:移到 MRU 端
                self.lru
                    .lock()
                    .map_err(|e| MlcError::StorageError(format!("L0 LRU mutex poisoned: {e}")))?
                    .touch(&id);

                Ok(None)
            }
            Entry::Vacant(vacant) => {
                // 条目不存在:需检查容量并可能驱逐
                // WHY drop vacant:evict_lru() 内部调用 self.entries.remove(),
                // 若 victim 与 id 同属一个 shard 会死锁,必须先释放 shard 锁
                drop(vacant);

                // 容量满时驱逐 LRU 端条目
                let victim = if self.entries.len() >= self.capacity {
                    let v = self.evict_lru()?;
                    if v.is_some() {
                        self.evictions.fetch_add(1, Ordering::Relaxed);
                    }
                    v
                } else {
                    None
                };

                // 二次 entry():drop vacant 与 insert 之间,其他线程可能已插入同 id
                // 使用 entry() 再次原子检查,正确处理并发插入(更新而非覆盖)
                match self.entries.entry(id.clone()) {
                    Entry::Occupied(mut occupied) => {
                        // 并发插入:其他线程已插入同 id,更新而非插入
                        // victim 已驱逐(缓存场景下少量过度驱逐可接受)
                        occupied.insert(Arc::new(entry));
                        drop(occupied);

                        self.lru
                            .lock()
                            .map_err(|e| {
                                MlcError::StorageError(format!("L0 LRU mutex poisoned: {e}"))
                            })?
                            .touch(&id);
                    }
                    Entry::Vacant(vacant) => {
                        // insert 消费 vacant 并返回 RefMut,无需显式 drop
                        let _ = vacant.insert(Arc::new(entry));

                        // push_back 幂等(内部检查 contains,防止重复入队)
                        self.lru
                            .lock()
                            .map_err(|e| {
                                MlcError::StorageError(format!("L0 LRU mutex poisoned: {e}"))
                            })?
                            .push_back(id.clone());
                    }
                }

                if let Some(ref v) = victim {
                    debug!(
                        entry_id = %id,
                        victim_id = %v.id,
                        "L0 容量满,驱逐最久未访问条目"
                    );
                }
                Ok(victim)
            }
        }
    }

    /// 获取记忆条目(更新 last_accessed_at 与 LRU,实现 LRU 语义)
    ///
    /// 返回条目克隆;若不存在返回 `EntryNotFound` 错误。
    ///
    /// WHY 获取写锁:DashMap 的 get_mut 获取分片写锁,更新 last_accessed_at。
    /// 同时锁 LruList 将节点移到 MRU 端,实现 O(1) LRU。
    pub fn get(&self, id: &str) -> Result<MemoryEntry, MlcError> {
        // 先更新 LRU 索引(touch 对不存在的 ID 是 no-op,无需回滚)
        {
            let mut lru = self
                .lru
                .lock()
                .map_err(|e| MlcError::StorageError(format!("L0 LRU mutex poisoned: {e}")))?;
            lru.touch(id);
        }
        // 再获取条目并更新访问时间
        if let Some(mut arc_entry) = self.entries.get_mut(id) {
            // Arc::make_mut:若 Arc 唯一(refcount==1)直接返回 &mut T;
            // 若共享(refcount>1,如 list_all_arc 持有引用)则克隆内层 T 再返回 &mut T。
            // WHY make_mut 而非 get_mut:get_mut 返回 Option,共享时 None 无法 touch;
            // make_mut 保证始终返回 &mut T,共享时自动 clone(仅 ~2KB,且 get 已 clone 返回)
            let entry_ref = Arc::make_mut(&mut *arc_entry);
            entry_ref.touch();
            return Ok(entry_ref.clone());
        }
        Err(MlcError::EntryNotFound(format!("L0 工作记忆条目: {id}")))
    }

    /// 尝试获取条目(不更新 last_accessed_at,不返回错误)
    ///
    /// 用于内部检查或不需要 LRU 语义的场景
    pub fn peek(&self, id: &str) -> Option<MemoryEntry> {
        // **r:Ref<Arc<MemoryEntry>> → Arc<MemoryEntry> → MemoryEntry
        // 显式双层解引用确保调用 MemoryEntry::clone 而非 Arc::clone
        self.entries.get(id).map(|r| (**r).clone())
    }

    /// 驱逐最久未访问的条目(LRU)— O(1)
    ///
    /// 从 LruList 头部(LRU 端)弹出 ID,再从 DashMap 移除对应条目。
    /// SubTask 13.2:从原 O(n) 扫描优化为 O(1) 链表弹出。
    ///
    /// 返回被驱逐的条目;若记忆为空返回 None。
    pub fn evict_lru(&self) -> Result<Option<MemoryEntry>, MlcError> {
        // 从 LRU 链表头部弹出最久未访问的 ID
        let victim_id = {
            let mut lru = self
                .lru
                .lock()
                .map_err(|e| MlcError::StorageError(format!("L0 LRU mutex poisoned: {e}")))?;
            lru.pop_front()
        };

        let victim = victim_id.and_then(|id| {
            self.entries.remove(&id).map(|(_, v)| {
                // 条目正从存储移除,通常 Arc refcount==1(无 list_all_arc 引用),
                // try_unwrap 零拷贝提取 owned 值;若被 list_all_arc 持有则 fallback clone
                Arc::try_unwrap(v).unwrap_or_else(|arc| (*arc).clone())
            })
        });

        if let Some(ref v) = victim {
            trace!(victim_id = %v.id, "L0 LRU 驱逐完成");
        }

        Ok(victim)
    }

    /// 移除指定条目(不更新驱逐计数)
    ///
    /// 用于主动删除或迁移到下层,同步从 LRU 索引移除。
    pub fn remove(&self, id: &str) -> Option<MemoryEntry> {
        let entry = self
            .entries
            .remove(id)
            .map(|(_, v)| Arc::try_unwrap(v).unwrap_or_else(|arc| (*arc).clone()));
        if entry.is_some() {
            if let Ok(mut lru) = self.lru.lock() {
                lru.remove(id);
            }
        }
        entry
    }

    /// 判断是否包含指定条目
    pub fn contains(&self, id: &str) -> bool {
        self.entries.contains_key(id)
    }

    /// 列出所有条目(深拷贝,用于迁移或快照)
    ///
    /// WHY 保留 list_all:API 兼容,调用方需 owned `MemoryEntry` 的场景(如序列化、跨层迁移)。
    /// 热路径(批量只读扫描)应优先使用 `list_all_arc()` 避免 4096 条目 ~8MB 深拷贝。
    pub fn list_all(&self) -> Vec<MemoryEntry> {
        // **r:RefMulti<Arc<MemoryEntry>> → Arc<MemoryEntry> → MemoryEntry
        self.entries.iter().map(|r| (**r).clone()).collect()
    }

    /// 列出所有条目的 Arc 引用(零拷贝共享,避免全量深拷贝)
    ///
    /// WHY list_all_arc:`list_all()` 返回 `Vec<MemoryEntry>` 需深拷贝每个条目,
    /// 4096 条目时 ~8MB 堆分配。本方法返回 `Vec<Arc<MemoryEntry>>`,
    /// 通过 `Arc::clone` 仅增加引用计数(原子 +1),无堆分配,适用于迁移/快照等热路径。
    pub fn list_all_arc(&self) -> Vec<Arc<MemoryEntry>> {
        self.entries.iter().map(|r| Arc::clone(&*r)).collect()
    }

    /// 清空所有条目与 LRU 索引(不重置驱逐计数)
    pub fn clear(&self) {
        self.entries.clear();
        if let Ok(mut lru) = self.lru.lock() {
            lru.clear();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(id: &str) -> MemoryEntry {
        MemoryEntry::new(id, format!("content-{id}"), MemoryTier::L0Working)
    }

    #[test]
    fn test_lru_list_push_pop_order() {
        let mut lru = LruList::new();
        lru.push_back("a".into());
        lru.push_back("b".into());
        lru.push_back("c".into());
        assert_eq!(lru.len(), 3);

        // FIFO 顺序弹出
        assert_eq!(lru.pop_front().as_deref(), Some("a"));
        assert_eq!(lru.pop_front().as_deref(), Some("b"));
        assert_eq!(lru.pop_front().as_deref(), Some("c"));
        assert!(lru.pop_front().is_none());
    }

    #[test]
    fn test_lru_list_touch_moves_to_tail() {
        let mut lru = LruList::new();
        lru.push_back("a".into());
        lru.push_back("b".into());
        lru.push_back("c".into());

        // touch "a",移到尾部,弹出顺序变为 b, c, a
        lru.touch("a");
        assert_eq!(lru.pop_front().as_deref(), Some("b"));
        assert_eq!(lru.pop_front().as_deref(), Some("c"));
        assert_eq!(lru.pop_front().as_deref(), Some("a"));
    }

    #[test]
    fn test_lru_list_remove_middle() {
        let mut lru = LruList::new();
        lru.push_back("a".into());
        lru.push_back("b".into());
        lru.push_back("c".into());

        // 移除中间的 "b"
        assert_eq!(lru.remove("b").as_deref(), Some("b"));
        assert_eq!(lru.len(), 2);
        assert_eq!(lru.pop_front().as_deref(), Some("a"));
        assert_eq!(lru.pop_front().as_deref(), Some("c"));
    }

    #[test]
    fn test_lru_list_free_list_reuse() {
        let mut lru = LruList::new();
        lru.push_back("a".into());
        lru.push_back("b".into());
        lru.remove("a");
        // 复用 "a" 的槽位
        lru.push_back("c".into());
        assert_eq!(lru.len(), 2);
        // nodes Vec 不应增长(复用 free_list)
        assert_eq!(lru.nodes.len(), 2);
    }

    #[test]
    fn test_insert_and_get() {
        let mem = WorkingMemory::new(64);
        let entry = make_entry("m-1");
        mem.insert(entry.clone()).unwrap();

        let fetched = mem.get("m-1").unwrap();
        assert_eq!(fetched.id.as_str(), "m-1");
        assert_eq!(fetched.content, "content-m-1");
        // insert 时 touch 一次,get 时 touch 一次,共 2 次
        assert_eq!(fetched.access_count, 2);
    }

    #[test]
    fn test_get_nonexistent_returns_error() {
        let mem = WorkingMemory::new(64);
        let result = mem.get("nonexistent");
        assert!(matches!(result, Err(MlcError::EntryNotFound(_))));
    }

    #[test]
    fn test_capacity_and_len() {
        let mem = WorkingMemory::new(64);
        assert_eq!(mem.capacity(), 64);
        assert_eq!(mem.len(), 0);
        assert!(mem.is_empty());

        mem.insert(make_entry("m-1")).unwrap();
        assert_eq!(mem.len(), 1);
        assert!(!mem.is_empty());
    }

    #[test]
    fn test_lru_eviction_on_overflow() {
        // 容量 2,插入 3 个条目,最久未访问的应被驱逐
        // WorkingMemory 的 LRU 基于链表(LruList),顺序由操作顺序决定,
        // 与墙钟时间无关,无需 thread::sleep(SubTask 20.2)
        let mem = WorkingMemory::new(2);
        mem.insert(make_entry("m-1")).unwrap();
        mem.insert(make_entry("m-2")).unwrap();

        // 访问 m-1,使 m-2 成为最久未访问(链表 touch 移到 MRU 端)
        mem.get("m-1").unwrap();

        // 插入 m-3,应驱逐 m-2
        let evicted = mem.insert(make_entry("m-3")).unwrap();
        assert_eq!(evicted.as_ref().map(|e| e.id.as_str()), Some("m-2"));
        assert_eq!(mem.evictions(), 1);

        // m-1 和 m-3 应存在,m-2 应被驱逐
        assert!(mem.contains("m-1"));
        assert!(!mem.contains("m-2"));
        assert!(mem.contains("m-3"));
    }

    #[test]
    fn test_lru_eviction_65_entries() {
        // 任务要求:插入 65 条目后最久未访问的被驱逐(容量 64)
        // WorkingMemory 的 LRU 基于链表(LruList),顺序由操作顺序决定,
        // 与墙钟时间无关,无需 thread::sleep(SubTask 20.2)
        let mem = WorkingMemory::new(64);

        // 插入 64 个条目
        for i in 0..64 {
            mem.insert(make_entry(&format!("m-{i}"))).unwrap();
        }
        assert_eq!(mem.len(), 64);
        assert_eq!(mem.evictions(), 0);

        // 访问 m-1 到 m-63,使 m-0 成为最久未访问(链表 touch 移到 MRU 端)
        for i in 1..64 {
            mem.get(&format!("m-{i}")).unwrap();
        }

        // 插入第 65 个条目,应驱逐 m-0
        let evicted = mem.insert(make_entry("m-64")).unwrap();
        assert_eq!(evicted.as_ref().map(|e| e.id.as_str()), Some("m-0"));
        assert_eq!(mem.len(), 64); // 容量不变
        assert_eq!(mem.evictions(), 1);
        assert!(!mem.contains("m-0"));
        assert!(mem.contains("m-64"));
    }

    #[test]
    fn test_peek_does_not_update_access_time() {
        let mem = WorkingMemory::new(64);
        let entry = make_entry("m-1");
        mem.insert(entry).unwrap();

        // 记录 insert 后的 last_accessed_at(insert 时 touch 一次)
        let after_insert = mem.peek("m-1").unwrap().last_accessed_at;

        // 逻辑时钟替代 thread::sleep:peek 不更新 last_accessed_at 与 access_count,
        // 直接验证字段不变即可,无需等待墙钟时间流逝(SubTask 20.2)
        let peeked = mem.peek("m-1").unwrap();
        // peek 不更新 last_accessed_at(与 get 区分)
        assert_eq!(peeked.last_accessed_at, after_insert);
        // insert 时 touch 一次,peek 不增加 access_count
        assert_eq!(peeked.access_count, 1);
    }

    #[test]
    fn test_remove() {
        let mem = WorkingMemory::new(64);
        mem.insert(make_entry("m-1")).unwrap();
        assert!(mem.contains("m-1"));

        let removed = mem.remove("m-1");
        assert!(removed.is_some());
        assert!(!mem.contains("m-1"));
        assert_eq!(mem.len(), 0);
    }

    #[test]
    fn test_clear() {
        let mem = WorkingMemory::new(64);
        for i in 0..5 {
            mem.insert(make_entry(&format!("m-{i}"))).unwrap();
        }
        assert_eq!(mem.len(), 5);

        mem.clear();
        assert_eq!(mem.len(), 0);
        assert!(mem.is_empty());
    }

    #[test]
    fn test_list_all() {
        let mem = WorkingMemory::new(64);
        for i in 0..3 {
            mem.insert(make_entry(&format!("m-{i}"))).unwrap();
        }
        let all = mem.list_all();
        assert_eq!(all.len(), 3);
    }

    #[test]
    fn test_list_all_arc_shares_entries() {
        // 验证 list_all_arc 返回 Arc 引用,内容与 list_all 一致
        let mem = WorkingMemory::new(64);
        for i in 0..3 {
            mem.insert(make_entry(&format!("m-{i}"))).unwrap();
        }
        let arcs = mem.list_all_arc();
        assert_eq!(arcs.len(), 3);
        // 验证 Arc 内数据正确(每个条目 id 以 "m-" 开头)
        assert!(arcs.iter().all(|a| a.id.as_str().starts_with("m-")));
        // 验证 Arc 共享:存储中的 Arc 与返回的 Arc 是同一份(refcount > 1)
        assert!(arcs.iter().all(|a| Arc::strong_count(a) >= 2));
    }

    #[test]
    fn test_update_existing_no_eviction() {
        let mem = WorkingMemory::new(2);
        mem.insert(make_entry("m-1")).unwrap();
        mem.insert(make_entry("m-2")).unwrap();

        // 更新已存在的 m-1,不应触发驱逐
        let evicted = mem.insert(make_entry("m-1")).unwrap();
        assert!(evicted.is_none());
        assert_eq!(mem.len(), 2);
        assert_eq!(mem.evictions(), 0);
    }

    #[test]
    fn test_evict_lru_empty_returns_none() {
        let mem = WorkingMemory::new(64);
        let result = mem.evict_lru().unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn test_remove_syncs_lru_index() {
        // 验证 remove 同步清理 LRU 索引:remove 后再 insert 应驱逐正确的条目
        let mem = WorkingMemory::new(2);
        mem.insert(make_entry("m-1")).unwrap();
        mem.insert(make_entry("m-2")).unwrap();
        // LRU 顺序:[m-1, m-2],entries = {m-1, m-2},len = 2
        mem.remove("m-1");
        // LRU 顺序:[m-2],entries = {m-2},len = 1

        // 插入 m-3,len=1 < capacity=2,不驱逐
        let evicted = mem.insert(make_entry("m-3")).unwrap();
        assert!(evicted.is_none(), "len=1 < 2,不应驱逐");
        // LRU 顺序:[m-2, m-3],entries = {m-2, m-3},len = 2

        // 插入 m-4,容量满,应驱逐 m-2(而非已移除的 m-1)
        // 若 remove 未同步 LRU,会驱逐 m-1(已不存在),evicted = None,测试失败
        let evicted = mem.insert(make_entry("m-4")).unwrap();
        assert_eq!(evicted.as_ref().map(|e| e.id.as_str()), Some("m-2"));
        assert!(mem.contains("m-3"));
        assert!(mem.contains("m-4"));
        assert!(!mem.contains("m-2"));
    }
}
