//! SccCache — 推测上下文缓存主体
//!
//! 对应架构层:L3 Storage
//! 对应创新点:SCC(Speculative Context Cache)
//!
//! # 核心职责
//! - 基于 DashMap 的并发安全上下文缓存
//! - Draft/Verify 共享:PVL Producer 与 Verifier 通过 `Arc<ContextEntry>` 共享同一份内容
//! - LRU 驱逐:容量满时驱逐最久未访问条目(Arc 引用保护)
//! - 命中率统计:每 100 次访问发布 CacheStatsReported 事件
//! - EventBus 集成:发布 CacheHit/CacheMiss/CacheStatsReported 事件
//!
//! # 设计决策(WHY)
//! - **`Arc<DashMap>` 包装**:SccCache 需要 Clone(预取后台任务需要克隆缓存实例),
//!   DashMap 本身不是 Clone(克隆会复制数据),用 Arc 包装实现共享
//! - **逻辑时钟(AtomicU64)**:LRU 驱逐依据,替代墙钟时间避免 Windows 15ms 精度问题
//! - **insert_lock 临界区**:保护"检查容量 → 驱逐 → 插入"原子性,避免并发超容
//! - **publish_blocking 用于同步方法**:get_or_prefetch/insert 是同步方法,
//!   使用 publish_blocking 发布事件,避免 async 污染调用方

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use dashmap::DashMap;
use event_bus::{EventBus, EventMetadata, NexusEvent};
use tracing::{debug, warn};

use crate::config::SccConfig;
use crate::lru;
use crate::types::{CacheStats, ContextEntry, ContextId};

/// 推测上下文缓存 — 基于 DashMap 的并发安全 LRU 缓存
///
/// # Draft/Verify 共享
/// PVL 的 Producer 与 Verifier 访问相同上下文,通过 `get_or_prefetch` 获取
/// `Arc<ContextEntry>`,两者引用同一份内容,避免重复加载(spec.md 决策 3)。
///
/// # LRU 驱逐
/// 容量达上限时,按逻辑时钟值(最久未访问)驱逐条目。被 Arc 引用的条目
/// (strong_count > 1)不会被驱逐,保证运行中的操作不受影响(spec.md 决策 4)。
///
/// # 线程安全
/// 所有字段通过 Arc 共享或原子操作,Clone 廉价(仅 Arc 引用计数)。
/// 所有同步方法满足 `Send + Sync`,可在多线程环境自由调用。
#[derive(Clone)]
pub struct SccCache {
    /// 缓存条目表(ContextId → `Arc<ContextEntry>`)
    entries: Arc<DashMap<ContextId, Arc<ContextEntry>>>,
    /// 引擎配置
    config: SccConfig,
    /// 事件总线(发布 CacheHit/CacheMiss/CacheStatsReported 事件)
    event_bus: EventBus,
    /// LRU 逻辑时钟访问顺序(ContextId → 时钟值,越小越久未访问)
    access_order: Arc<DashMap<ContextId, u64>>,
    /// 逻辑时钟计数器(单调递增,保证 LRU 顺序确定性)
    clock: Arc<AtomicU64>,
    /// 缓存统计快照(每 100 次访问更新)
    stats: Arc<RwLock<CacheStats>>,
    /// 总访问次数(命中 + 未命中)
    access_count: Arc<AtomicU64>,
    /// 缓存命中次数
    hit_count: Arc<AtomicU64>,
    /// 累计驱逐次数
    eviction_count: Arc<AtomicU64>,
    /// 插入临界区锁:保护"检查容量 → 驱逐 → 插入"原子性
    insert_lock: Arc<Mutex<()>>,
}

impl SccCache {
    /// 创建推测上下文缓存,使用指定配置与 EventBus
    pub fn new(config: SccConfig, event_bus: EventBus) -> Self {
        let capacity = config.capacity;
        Self {
            entries: Arc::new(DashMap::with_capacity(capacity)),
            config,
            event_bus,
            access_order: Arc::new(DashMap::with_capacity(capacity)),
            clock: Arc::new(AtomicU64::new(0)),
            stats: Arc::new(RwLock::new(CacheStats::default())),
            access_count: Arc::new(AtomicU64::new(0)),
            hit_count: Arc::new(AtomicU64::new(0)),
            eviction_count: Arc::new(AtomicU64::new(0)),
            insert_lock: Arc::new(Mutex::new(())),
        }
    }

    /// 获取或预取上下文 — 命中返回 `Arc<ContextEntry>`,未命中返回 None
    ///
    /// # 行为
    /// - **命中**:返回 `Some(Arc<ContextEntry>)`,更新 access_count/last_accessed_at,
    ///   发布 `CacheHit` 事件
    /// - **未命中**:返回 `None`,发布 `CacheMiss` 事件
    /// - **统计**:每 100 次访问更新统计快照并发布 `CacheStatsReported` 事件
    ///
    /// # Draft/Verify 共享
    /// Producer 与 Verifier 调用此方法获取同一 `Arc<ContextEntry>`,
    /// Arc 引用计数保证内容不被 LRU 驱逐(strong_count > 1 时跳过)
    pub fn get_or_prefetch(&self, context_id: &ContextId) -> Option<Arc<ContextEntry>> {
        let total = self.access_count.fetch_add(1, Ordering::Relaxed) + 1;

        // 尝试从缓存获取(Arc::clone 在 Ref 守卫存活期间完成,守卫随后释放)
        let entry_arc = self.entries.get(context_id).map(|r| Arc::clone(r.value()));

        match entry_arc {
            Some(entry_arc) => {
                // 缓存命中
                self.hit_count.fetch_add(1, Ordering::Relaxed);

                // 更新访问元数据(access_count + last_accessed_at)
                entry_arc.record_access();

                // 更新 LRU 逻辑时钟
                let tick = self.clock.fetch_add(1, Ordering::Relaxed);
                self.access_order.insert(context_id.clone(), tick);

                // 发布 CacheHit 事件
                let _ = self.event_bus.publish_blocking(NexusEvent::CacheHit {
                    metadata: EventMetadata::new("scc-cache"),
                    cache_key: context_id.to_string(),
                });

                // 每 100 次访问发布统计
                if total.is_multiple_of(100) {
                    self.update_and_publish_stats();
                }

                Some(entry_arc)
            }
            None => {
                // 缓存未命中
                let _ = self.event_bus.publish_blocking(NexusEvent::CacheMiss {
                    metadata: EventMetadata::new("scc-cache"),
                    cache_key: context_id.to_string(),
                });
                None
            }
        }
    }

    /// 插入缓存条目
    ///
    /// 若容量达上限且为新条目,按 LRU 策略驱逐最久未访问条目(Arc 引用保护)。
    /// 若条目已存在,更新内容并刷新 LRU 时钟。
    pub fn insert(&self, entry: ContextEntry) {
        // 获取临界区锁,保证"检查容量 → 驱逐 → 插入"原子性
        // WHY unwrap_or_else(into_inner):锁中毒表示前序持有者 panic,
        // 数据仍可访问,恢复执行比 abort 更安全
        let _guard = self.insert_lock.lock().unwrap_or_else(|e| {
            warn!("insert_lock poisoned, recovering");
            e.into_inner()
        });

        let id = entry.id.clone();
        let is_new = !self.entries.contains_key(&id);

        // 容量满且是新条目,驱逐 LRU 受害者
        if is_new && self.entries.len() >= self.config.capacity {
            if let Some(victim_id) = lru::select_victim(&self.access_order, &self.entries) {
                self.access_order.remove(&victim_id);
                self.entries.remove(&victim_id);
                self.eviction_count.fetch_add(1, Ordering::Relaxed);
                debug!(victim_id = %victim_id, "LRU 驱逐完成");
            }
            // 若 select_victim 返回 None(所有条目被引用),跳过驱逐,
            // 缓存临时超容,下次驱逐时清理
        }

        // 更新逻辑时钟
        let tick = self.clock.fetch_add(1, Ordering::Relaxed);
        self.access_order.insert(id.clone(), tick);

        // 插入条目
        self.entries.insert(id, Arc::new(entry));
    }

    /// 预热条目 — 更新访问元数据与 LRU 时钟,但不计入访问统计
    ///
    /// WHY pub(crate):仅供 `AccessPatternLearner::prefetch` 后台任务调用,
    /// 不对外暴露。与 `get_or_prefetch` 的区别:不递增 access_count/hit_count,
    /// 不发布 CacheHit 事件(预取有独立的 CachePrefetched 事件)
    pub(crate) fn warm_entry(&self, id: &ContextId) -> bool {
        let entry_arc = self.entries.get(id).map(|r| Arc::clone(r.value()));
        match entry_arc {
            Some(entry) => {
                entry.record_access();
                let tick = self.clock.fetch_add(1, Ordering::Relaxed);
                self.access_order.insert(id.clone(), tick);
                true
            }
            None => false,
        }
    }

    /// 返回当前缓存统计快照
    pub fn stats(&self) -> CacheStats {
        self.stats.read().map(|s| s.clone()).unwrap_or_default()
    }

    /// 返回当前缓存条目数
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// 缓存是否为空
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// 判断是否包含指定上下文
    pub fn contains(&self, id: &ContextId) -> bool {
        self.entries.contains_key(id)
    }

    /// 返回累计驱逐次数
    pub fn evictions(&self) -> u64 {
        self.eviction_count.load(Ordering::Relaxed)
    }

    /// 返回总访问次数
    pub fn access_count(&self) -> u64 {
        self.access_count.load(Ordering::Relaxed)
    }

    /// 返回缓存命中次数
    pub fn hit_count(&self) -> u64 {
        self.hit_count.load(Ordering::Relaxed)
    }

    /// 更新统计快照并发布 CacheStatsReported 事件
    ///
    /// WHY 每 100 次访问调用:避免每次访问都发布事件造成事件总线过载。
    /// 100 次访问的统计窗口足够反映缓存健康度
    fn update_and_publish_stats(&self) {
        let total = self.access_count.load(Ordering::Relaxed);
        let hits = self.hit_count.load(Ordering::Relaxed);
        let evictions = self.eviction_count.load(Ordering::Relaxed);
        let entry_count = self.entries.len();

        let hit_rate = if total > 0 {
            hits as f32 / total as f32
        } else {
            0.0
        };

        let new_stats = CacheStats {
            hit_rate,
            eviction_count: evictions,
            entry_count,
        };

        // 更新缓存统计快照
        if let Ok(mut stats) = self.stats.write() {
            *stats = new_stats.clone();
        }

        // 发布 CacheStatsReported 事件
        let _ = self
            .event_bus
            .publish_blocking(NexusEvent::CacheStatsReported {
                metadata: EventMetadata::new("scc-cache"),
                hit_rate: new_stats.hit_rate,
                eviction_count: new_stats.eviction_count,
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_cache(capacity: usize) -> SccCache {
        SccCache::new(
            SccConfig::default().with_capacity(capacity),
            EventBus::new(),
        )
    }

    #[test]
    fn test_insert_and_get_hit() {
        let cache = make_cache(256);
        let id = ContextId::new("ctx-1");
        cache.insert(ContextEntry::new("ctx-1", "content-1"));

        let entry = cache.get_or_prefetch(&id);
        assert!(entry.is_some());
        let entry = entry.unwrap();
        assert_eq!(entry.id.as_str(), "ctx-1");
        assert_eq!(&*entry.content, "content-1");
        // get_or_prefetch 命中时 record_access 一次
        assert_eq!(entry.access_count(), 1);
    }

    #[test]
    fn test_get_miss() {
        let cache = make_cache(256);
        let id = ContextId::new("ctx-miss");
        let result = cache.get_or_prefetch(&id);
        assert!(result.is_none());
        assert_eq!(cache.access_count(), 1);
        assert_eq!(cache.hit_count(), 0);
    }

    #[test]
    fn test_arc_sharing_producer_verifier() {
        // 验证 Draft/Verify 共享:Producer 与 Verifier 获取同一 Arc
        let cache = make_cache(256);
        let id = ContextId::new("ctx-shared");
        cache.insert(ContextEntry::new("ctx-shared", "shared content"));

        let producer_entry = cache.get_or_prefetch(&id).unwrap();
        let verifier_entry = cache.get_or_prefetch(&id).unwrap();

        // 两个 Arc 指向同一分配
        assert!(Arc::ptr_eq(&producer_entry, &verifier_entry));
    }

    #[test]
    fn test_lru_eviction_on_capacity() {
        // 容量 2,插入 3 个条目,最久未访问的应被驱逐
        let cache = make_cache(2);

        cache.insert(ContextEntry::new("ctx-a", "a"));
        cache.insert(ContextEntry::new("ctx-b", "b"));

        // 访问 ctx-a,使 ctx-b 成为最久未访问
        let _ = cache.get_or_prefetch(&ContextId::new("ctx-a"));

        // 插入 ctx-c,应驱逐 ctx-b
        cache.insert(ContextEntry::new("ctx-c", "c"));

        assert!(cache.contains(&ContextId::new("ctx-a")));
        assert!(!cache.contains(&ContextId::new("ctx-b")));
        assert!(cache.contains(&ContextId::new("ctx-c")));
        assert_eq!(cache.evictions(), 1);
    }

    #[test]
    fn test_arc_reference_protection() {
        // 持有 Arc 的条目不应被 LRU 驱逐
        let cache = make_cache(2);

        cache.insert(ContextEntry::new("ctx-a", "a"));
        cache.insert(ContextEntry::new("ctx-b", "b"));

        // 获取 ctx-a 的 Arc 并持有(模拟 Producer 正在使用)
        let held_arc = cache.get_or_prefetch(&ContextId::new("ctx-a")).unwrap();

        // 访问 ctx-b 使 ctx-a 成为最久未访问
        let _ = cache.get_or_prefetch(&ContextId::new("ctx-b"));

        // 插入 ctx-c,ctx-a 虽最久未访问但被 Arc 引用,应驱逐 ctx-b
        // 但 ctx-b 也被 get_or_prefetch 返回过... 不过返回的 Arc 已 drop
        // 所以 ctx-b 的 strong_count == 1,可以被驱逐
        cache.insert(ContextEntry::new("ctx-c", "c"));

        // ctx-a 仍在缓存中(被 held_arc 保护)
        assert!(cache.contains(&ContextId::new("ctx-a")));
        // held_arc 仍然有效
        assert_eq!(&*held_arc.content, "a");
    }

    #[test]
    fn test_hit_rate_statistics() {
        let cache = make_cache(256);

        // 插入 10 个条目
        for i in 0..10 {
            cache.insert(ContextEntry::new(
                format!("ctx-{i}"),
                format!("content-{i}"),
            ));
        }

        // 80 次命中(访问已存在的条目)
        for _ in 0..80 {
            for i in 0..8 {
                let id = ContextId::new(format!("ctx-{i}"));
                assert!(cache.get_or_prefetch(&id).is_some());
            }
        }

        // 20 次未命中(访问不存在的条目)
        for _ in 0..20 {
            let id = ContextId::new("ctx-miss");
            assert!(cache.get_or_prefetch(&id).is_none());
        }

        // 总访问 640 + 20 = 660,命中 640
        // hit_rate = 640 / 660 ≈ 0.97
        let stats = cache.stats();
        assert!(
            stats.hit_rate > 0.7,
            "hit rate {} should be > 0.7",
            stats.hit_rate
        );
        assert_eq!(stats.entry_count, 10);
    }

    #[test]
    fn test_update_existing_no_eviction() {
        let cache = make_cache(2);
        cache.insert(ContextEntry::new("ctx-a", "a"));
        cache.insert(ContextEntry::new("ctx-b", "b"));

        // 更新已存在的 ctx-a,不应触发驱逐
        cache.insert(ContextEntry::new("ctx-a", "updated"));
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.evictions(), 0);

        // 验证内容已更新
        let entry = cache.get_or_prefetch(&ContextId::new("ctx-a")).unwrap();
        assert_eq!(&*entry.content, "updated");
    }

    #[test]
    fn test_stats_snapshot_updated() {
        let cache = make_cache(256);

        // 初始统计为零
        let stats = cache.stats();
        assert_eq!(stats.hit_rate, 0.0);
        assert_eq!(stats.entry_count, 0);

        cache.insert(ContextEntry::new("ctx-1", "content"));

        // 触发 100 次访问以更新统计快照
        let id = ContextId::new("ctx-1");
        for _ in 0..100 {
            let _ = cache.get_or_prefetch(&id);
        }

        let stats = cache.stats();
        assert!(
            (stats.hit_rate - 1.0).abs() < 0.01,
            "hit rate should be ~1.0"
        );
        assert_eq!(stats.entry_count, 1);
    }
}
