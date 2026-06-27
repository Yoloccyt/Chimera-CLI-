//! SCC 核心领域类型 — 推测上下文缓存的统一数据模型
//!
//! 对应架构层:L3 Storage
//! 对应创新点:SCC(Speculative Context Cache,推测上下文缓存)
//!
//! # 类型职责
//! - `ContextId`:上下文唯一标识(基于 nexus_core::id_newtype! 宏)
//! - `ContextEntry`:缓存条目载体,content 使用 `Arc<str>` 实现 Producer/Verifier 共享
//! - `AccessPattern`:访问模式快照(当前上下文 + 转移计数列表)
//! - `CacheStats`:缓存运行时统计(命中率、驱逐数、条目数)
//!
//! # 设计决策(WHY)
//! - **ContextId 用 id_newtype! 宏**:与 nexus-core 的 ToolId/MemoryId 保持一致,
//!   编译器拦截类型混淆,Deref<Target=str> 保持 &str 接口兼容
//! - **content: `Arc<str>`**:PVL 的 Producer 与 Verifier 访问相同上下文,
//!   Arc 共享避免重复加载(继承 Week 3 Arc 共享经验)。`Arc<str>` 而非 `Arc<String>`
//!   节省一次指针解引用(str 直接挂在 Arc 内)
//! - **access_count: AtomicU64 / last_accessed_at: `Mutex<Instant>`**:ContextEntry
//!   通过 Arc 共享,需内部可变性才能在命中时更新访问元数据。AtomicU64 无锁,
//!   `Mutex<Instant>` 用于 Instant 的原子更新(Instant 无原子操作支持)
//! - **逻辑时钟替代 last_accessed_at 做 LRU**:Windows 系统时钟精度约 15ms,
//!   短时间内多次操作可能产生相同时间戳,导致 LRU 顺序不确定。逻辑时钟
//!   (SccCache 内的 AtomicU64)严格单调递增,消除墙钟精度依赖
//!   (继承 cmt-tiering SubTask 20.2 经验)

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Instant;

use std::sync::Arc;

// 上下文唯一标识 newtype — 基于 nexus_core::id_newtype! 宏生成
//
// WHY 使用宏:与 nexus-core 的 ToolId/MemoryId 保持一致的 newtype 行为
// (Debug/Clone/PartialEq/Eq/Hash/Serialize/Deserialize + Deref<Target=str>),
// 消除各 crate 重复实现 newtype 的样板代码
nexus_core::id_newtype!(ContextId, "上下文唯一标识");

/// 缓存条目 — Producer 与 Verifier 共享的上下文载体
///
/// # Arc 共享语义
/// `content: Arc<str>` 允许 PVL 的 Producer 与 Verifier 通过 `Arc::clone`
/// 获取同一份内容,避免重复加载。`Arc::strong_count > 1` 时 LRU 驱逐跳过
/// 该条目(引用保护,见 lru.rs)。
///
/// # 内部可变性
/// `access_count` 与 `last_accessed_at` 使用原子类型 / Mutex 实现内部可变性,
/// 使得通过 `&ContextEntry`(或 `Arc<ContextEntry>`)即可更新访问元数据,
/// 无需 `&mut` 或 `Arc<RwLock<ContextEntry>>`(减少锁开销)。
pub struct ContextEntry {
    /// 上下文唯一标识
    pub id: ContextId,
    /// 上下文内容(Arc 共享,Producer 与 Verifier 引用同一份内容)
    pub content: Arc<str>,
    /// 访问次数(原子操作,命中时递增)
    pub access_count: AtomicU64,
    /// 最后访问时间(内部可变性,命中时更新)
    ///
    /// WHY `Mutex<Instant>`:Instant 无原子操作支持,用 Mutex 实现独占更新。
    /// 锁持有时间极短(仅赋值),不影响并发性能
    pub last_accessed_at: Mutex<Instant>,
}

impl ContextEntry {
    /// 创建新缓存条目,access_count 初始化为 0,last_accessed_at 为当前时刻
    ///
    /// # 参数
    /// - `id`:上下文标识(接受 ContextId/String/&str)
    /// - `content`:上下文内容(接受 `Arc<str>`/`String`/`&str`)
    pub fn new(id: impl Into<ContextId>, content: impl Into<Arc<str>>) -> Self {
        Self {
            id: id.into(),
            content: content.into(),
            access_count: AtomicU64::new(0),
            last_accessed_at: Mutex::new(Instant::now()),
        }
    }

    /// 标记被访问:递增 access_count,更新 last_accessed_at
    ///
    /// WHY:每次缓存命中调用此方法,实现 LRU 语义(最近访问的不易被驱逐)
    /// 与访问热度统计(access_count 越高表示越热)
    pub fn record_access(&self) {
        self.access_count.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut t) = self.last_accessed_at.lock() {
            *t = Instant::now();
        }
    }

    /// 返回当前访问次数
    pub fn access_count(&self) -> u64 {
        self.access_count.load(Ordering::Relaxed)
    }

    /// 返回最后访问时间
    ///
    /// WHY 返回 Instant 而非 &Instant:`Mutex<Instant>` 只能返回守卫或拷贝,
    /// Instant 是 Copy 类型,直接返回值避免暴露锁实现
    pub fn last_accessed_at(&self) -> Instant {
        self.last_accessed_at
            .lock()
            .map(|t| *t)
            .unwrap_or_else(|_| Instant::now())
    }
}

impl std::fmt::Debug for ContextEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ContextEntry")
            .field("id", &self.id)
            .field("content_len", &self.content.len())
            .field("access_count", &self.access_count())
            .finish()
    }
}

/// 访问模式快照 — 某个上下文的转移计数列表
///
/// 由 `AccessPatternLearner::get_pattern` 返回,用于调试与模式分析。
/// `transitions` 是 `(目标 ContextId, 转移计数)` 的列表,按计数降序排列。
#[derive(Debug, Clone)]
pub struct AccessPattern {
    /// 当前上下文 ID
    pub current: ContextId,
    /// 转移计数列表(目标上下文 → 计数),按计数降序
    pub transitions: Vec<(ContextId, u32)>,
}

/// 缓存运行时统计 — 命中率、驱逐数、条目数
///
/// 由 `SccCache::stats()` 返回,每 100 次访问发布 `CacheStatsReported` 事件。
#[derive(Debug, Clone)]
pub struct CacheStats {
    /// 缓存命中率 [0.0, 1.0](hit_count / access_count)
    pub hit_rate: f32,
    /// 累计驱逐次数
    pub eviction_count: u64,
    /// 当前缓存条目数
    pub entry_count: usize,
}

impl Default for CacheStats {
    fn default() -> Self {
        Self {
            hit_rate: 0.0,
            eviction_count: 0,
            entry_count: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_context_id_new() {
        let id = ContextId::new("ctx-1");
        assert_eq!(id.as_str(), "ctx-1");
    }

    #[test]
    fn test_context_id_deref() {
        let id = ContextId::new("ctx-1");
        let s: &str = &id;
        assert_eq!(s, "ctx-1");
    }

    #[test]
    fn test_context_id_from_string() {
        let id = ContextId::from(String::from("from-string"));
        assert_eq!(id.as_str(), "from-string");
    }

    #[test]
    fn test_context_id_hash_eq() {
        use std::collections::HashMap;
        let id1 = ContextId::new("same");
        let id2 = ContextId::new("same");
        let mut map: HashMap<ContextId, i32> = HashMap::new();
        map.insert(id1, 42);
        assert_eq!(map.get(&id2), Some(&42));
    }

    #[test]
    fn test_context_entry_new() {
        let entry = ContextEntry::new("ctx-1", "content");
        assert_eq!(entry.id.as_str(), "ctx-1");
        assert_eq!(&*entry.content, "content");
        assert_eq!(entry.access_count(), 0);
    }

    #[test]
    fn test_context_entry_record_access() {
        let entry = ContextEntry::new("ctx-1", "content");
        assert_eq!(entry.access_count(), 0);

        entry.record_access();
        entry.record_access();
        assert_eq!(entry.access_count(), 2);
    }

    #[test]
    fn test_context_entry_arc_sharing() {
        // 验证 Arc<str> 共享:clone 后指向同一分配
        let entry = ContextEntry::new("ctx-1", "shared content");
        let content_clone = Arc::clone(&entry.content);
        assert!(Arc::ptr_eq(&entry.content, &content_clone));
    }

    #[test]
    fn test_cache_stats_default() {
        let stats = CacheStats::default();
        assert_eq!(stats.hit_rate, 0.0);
        assert_eq!(stats.eviction_count, 0);
        assert_eq!(stats.entry_count, 0);
    }
}
