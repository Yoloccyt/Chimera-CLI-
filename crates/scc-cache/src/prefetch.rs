//! 访问模式学习与推测性预取 — 基于一阶马尔可夫链的上下文访问预测
//!
//! 对应架构层:L3 Storage
//! 对应创新点:SCC(Speculative Context Cache)的推测性预取机制
//!
//! # 核心职责
//! - `AccessPatternLearner`:学习上下文访问转移模式(一阶马尔可夫链)
//! - `record_access`:记录上下文转移(current → next),更新转移计数
//! - `predict_next`:预测下一步可能访问的上下文及概率(按概率降序)
//! - `prefetch`:对高概率上下文异步预取(预热)到缓存,发布 CachePrefetched 事件
//!
//! # 设计决策(WHY)
//! - **一阶马尔可夫链**:当前状态 → 下一步状态概率,简单有效(spec.md 决策 1)。
//!   不用高阶马尔可夫链(N-gram),因为上下文访问的马尔可夫性质足够强,
//!   且一阶模型内存开销低(HashMap<ContextId, HashMap<ContextId, u32>>)
//! - **std::sync::RwLock 而非 tokio::sync::RwLock**:record_access/predict_next
//!   是同步方法(spec 签名要求),std::sync::RwLock 支持同步读写在非 async 上下文调用
//! - **tokio::spawn 后台更新**:record_access_background 将模式更新放入后台任务,
//!   不阻塞主流程。WHY 函数调用包装:std::sync::RwLockWriteGuard 是 !Send,
//!   不能直接在 async 块中持有。将锁获取放在 record_access 函数调用内,
//!   守卫是函数栈帧的局部变量,不进入 Future 状态机,Future 仍为 Send
//! - **预取阈值 0.6**:平衡预取命中率与预取消耗(spec.md 决策 2)
//! - **预取失败静默处理**:预取的上下文不在缓存中时仅 tracing::warn! 日志,
//!   不返回错误,不阻塞主流程(spec.md 要求)

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use event_bus::{EventBus, EventMetadata, NexusEvent};

use crate::cache::SccCache;
use crate::types::{AccessPattern, ContextId};

/// 默认转移矩阵容量上限
///
/// WHY 10000: 在长期运行场景中,上下文 ID 数量可能无限增长。
/// 10000 个活跃上下文足以覆盖典型会话的局部性,同时将内存占用
/// 控制在可预测范围(约数 MB),符合 Ω-Sparse 定律。
const DEFAULT_PATTERN_CAPACITY: usize = 10_000;

/// LRU 节点 — 使用 Vec 索引实现的无 unsafe 双向链表节点
///
/// WHY 不用 `std::collections::LinkedList`:其 Cursor API 在 Rust 2021
/// 中不稳定,无法在不使用 unsafe 指针的情况下 O(1) 移动节点。
/// 用 Vec 索引 + prev/next 指针可在 `#![forbid(unsafe_code)]` 约束下
/// 实现真正的 O(1) LRU 维护。
#[derive(Debug)]
struct LruNode {
    /// 当前上下文 ID
    key: ContextId,
    /// 前驱节点索引(`None` 表示当前节点是 LRU 头)
    prev: Option<usize>,
    /// 后继节点索引(`None` 表示当前节点是 MRU 尾)
    next: Option<usize>,
}

/// 容量受限的 LRU 访问模式图
///
/// 存储结构:current → (节点索引, {next → count})。
///
/// WHY: 一阶马尔可夫链随上下文 ID 数量线性增长;无界 HashMap 在
/// 长期运行中会导致内存无限膨胀。LruPatternMap 在保持 O(1) 查找/
/// 更新的前提下,通过 LRU 策略将活跃上下文数量限制在固定容量内,
/// 符合 Ω-Sparse 定律。
struct LruPatternMap {
    /// current → (节点索引, 转移计数表)
    data: HashMap<ContextId, (usize, HashMap<ContextId, u32>)>,
    /// 双向链表节点池。使用 Vec 索引而非指针,避免 unsafe。
    nodes: Vec<LruNode>,
    /// 可复用的节点索引(被驱逐节点留下的空位)
    free_indices: Vec<usize>,
    /// 链表头索引:最近最少使用(LRU)
    head: Option<usize>,
    /// 链表尾索引:最近最多使用(MRU)
    tail: Option<usize>,
    /// 最大容量
    capacity: usize,
}

impl LruPatternMap {
    /// 创建指定容量的空模式图
    fn with_capacity(capacity: usize) -> Self {
        assert!(capacity > 0, "LruPatternMap capacity must be > 0");
        Self {
            data: HashMap::with_capacity(capacity),
            nodes: Vec::with_capacity(capacity),
            free_indices: Vec::new(),
            head: None,
            tail: None,
            capacity,
        }
    }

    /// 当前存储的 current 上下文数量
    fn len(&self) -> usize {
        self.data.len()
    }

    /// 记录一次状态转移,并在需要时触发 LRU 淘汰
    ///
    /// 复杂度:O(1) 平均。
    fn record_transition(&mut self, current: &ContextId, next: &ContextId) {
        // 先在一个独立作用域内更新转移计数,避免 `self.data.get_mut` 借用
        // 与后续 `self.move_to_tail` 的 `&mut self` 冲突。
        let idx = if let Some((idx, transitions)) = self.data.get_mut(current) {
            transitions
                .entry(next.clone())
                .and_modify(|count| *count = count.saturating_add(1))
                .or_insert(1);
            Some(*idx)
        } else {
            None
        };

        if let Some(idx) = idx {
            // 已存在:移到 MRU
            self.move_to_tail(idx);
        } else {
            // 新 current:先淘汰 LRU 再插入
            if self.data.len() >= self.capacity {
                self.evict_lru();
            }

            let mut transitions = HashMap::new();
            transitions.insert(next.clone(), 1);

            let idx = self.alloc_node(current.clone());
            self.append_to_tail(idx);
            self.data.insert(current.clone(), (idx, transitions));
        }
    }

    /// 获取指定 current 的转移计数表(只读,不更新 LRU)
    fn get_transitions(&self, current: &ContextId) -> Option<&HashMap<ContextId, u32>> {
        self.data.get(current).map(|(_, t)| t)
    }

    /// 分配一个节点(复用空闲索引或追加新节点)
    fn alloc_node(&mut self, key: ContextId) -> usize {
        if let Some(idx) = self.free_indices.pop() {
            self.nodes[idx] = LruNode {
                key,
                prev: None,
                next: None,
            };
            idx
        } else {
            let idx = self.nodes.len();
            self.nodes.push(LruNode {
                key,
                prev: None,
                next: None,
            });
            idx
        }
    }

    /// 将节点追加到 MRU 尾
    fn append_to_tail(&mut self, idx: usize) {
        if let Some(tail_idx) = self.tail {
            self.nodes[tail_idx].next = Some(idx);
            self.nodes[idx].prev = Some(tail_idx);
        } else {
            // 第一个节点
            self.head = Some(idx);
            self.nodes[idx].prev = None;
        }
        self.nodes[idx].next = None;
        self.tail = Some(idx);
    }

    /// 将已存在节点移动到 MRU 尾
    fn move_to_tail(&mut self, idx: usize) {
        if self.tail == Some(idx) {
            return;
        }

        let node = &self.nodes[idx];
        let prev = node.prev;
        let next = node.next;

        // 从当前位置移除
        if let Some(p) = prev {
            self.nodes[p].next = next;
        } else {
            self.head = next;
        }
        if let Some(n) = next {
            self.nodes[n].prev = prev;
        }

        // 追加到尾部
        let tail_idx = self.tail.expect("tail must exist when len > 0");
        self.nodes[tail_idx].next = Some(idx);
        self.nodes[idx].prev = Some(tail_idx);
        self.nodes[idx].next = None;
        self.tail = Some(idx);
    }

    /// 驱逐最近最少使用的 current 上下文
    fn evict_lru(&mut self) {
        let lru_idx = self.head.expect("cannot evict from empty map");
        let lru_key = self.nodes[lru_idx].key.clone();
        let new_head = self.nodes[lru_idx].next;

        self.data.remove(&lru_key);

        if let Some(n) = new_head {
            self.nodes[n].prev = None;
        } else {
            // 唯一节点被移除
            self.tail = None;
        }
        self.head = new_head;
        self.free_indices.push(lru_idx);
    }
}

/// 访问模式学习器 — 基于一阶马尔可夫链的上下文访问预测
///
/// # 马尔可夫链模型
/// `patterns: RwLock<LruPatternMap>`
/// - 外层 key:当前上下文 ID
/// - 内层 key:下一步上下文 ID
/// - 内层 value:转移计数(current → next 出现次数)
///
/// 概率计算:`P(next | current) = count(current → next) / Σ count(current → *)`
///
/// # 线程安全
/// `patterns` 使用 `std::sync::RwLock` 保护,支持并发读、独占写。
/// `record_access` 获取写锁,`predict_next` 获取读锁,两者均满足 `Send + Sync`。
pub struct AccessPatternLearner {
    /// 一阶马尔可夫链:current → {next → count},带 LRU 容量上限
    patterns: RwLock<LruPatternMap>,
    /// 事件总线(预取完成后发布 CachePrefetched 事件)
    event_bus: EventBus,
    /// 预取概率阈值(默认 0.6)
    prefetch_threshold: f32,
}

impl AccessPatternLearner {
    /// 创建访问模式学习器(使用默认容量 10000)
    ///
    /// # 参数
    /// - `event_bus`:事件总线(预取完成后发布 CachePrefetched 事件)
    /// - `prefetch_threshold`:预取概率阈值,概率 >= 此值的上下文会被预取
    pub fn new(event_bus: EventBus, prefetch_threshold: f32) -> Self {
        Self::with_capacity(event_bus, prefetch_threshold, DEFAULT_PATTERN_CAPACITY)
    }

    /// 创建指定容量的访问模式学习器
    ///
    /// # 参数
    /// - `event_bus`:事件总线(预取完成后发布 CachePrefetched 事件)
    /// - `prefetch_threshold`:预取概率阈值
    /// - `capacity`:转移矩阵容量上限,至少为 1
    ///
    /// WHY 显式容量构造函数:测试需要构造小容量场景以快速验证 LRU 行为,
    /// 同时生产代码通过 `new()` 获得合理的默认上限。
    pub fn with_capacity(event_bus: EventBus, prefetch_threshold: f32, capacity: usize) -> Self {
        Self {
            patterns: RwLock::new(LruPatternMap::with_capacity(capacity)),
            event_bus,
            prefetch_threshold,
        }
    }

    /// 返回当前存储的 current 上下文数量(用于监控与测试)
    ///
    /// WHY 暴露此指标:调用方可据此观察学习器内存占用,并在测试中
    /// 验证 LRU 容量上限是否生效。
    pub fn pattern_count(&self) -> usize {
        let patterns = self.patterns.read().unwrap_or_else(|e| {
            tracing::warn!("patterns RwLock poisoned, recovering");
            e.into_inner()
        });
        patterns.len()
    }

    /// 记录上下文访问转移 — 更新马尔可夫链转移计数
    ///
    /// # 参数
    /// - `current`:当前访问的上下文 ID
    /// - `next`:下一步访问的上下文 ID
    ///
    /// # 并发安全
    /// 获取 `patterns` 写锁更新转移计数。锁持有时间极短(HashMap entry 操作),
    /// 不影响并发性能。
    pub fn record_access(&self, current: &ContextId, next: &ContextId) {
        let mut patterns = self.patterns.write().unwrap_or_else(|e| {
            tracing::warn!("patterns RwLock poisoned, recovering");
            e.into_inner()
        });

        patterns.record_transition(current, next);
    }

    /// 异步后台记录访问转移 — 不阻塞主流程
    ///
    /// 将 `record_access` 包装在 `tokio::spawn` 后台任务中,调用方无需 await。
    ///
    /// # WHY self: `Arc<Self>`
    /// `tokio::spawn` 要求 Future 为 `Send + 'static`。`self: Arc<Self>` 将
    /// 学习器的所有权移入任务,任务内调用 `self.record_access(&current, &next)`。
    /// `record_access` 内部的 `RwLockWriteGuard` 是函数栈帧局部变量,
    /// 不进入 Future 状态机,Future 仍为 `Send`。
    ///
    /// # 使用方式
    /// ```no_run
    /// # use std::sync::Arc;
    /// # use scc_cache::{AccessPatternLearner, ContextId};
    /// # use event_bus::EventBus;
    /// # async fn demo() {
    /// let learner = Arc::new(AccessPatternLearner::new(EventBus::new(), 0.6));
    /// Arc::clone(&learner).record_access_background(
    ///     ContextId::new("ctx-a"),
    ///     ContextId::new("ctx-b"),
    /// );
    /// // learner 仍可使用(Arc::clone 保留了引用)
    /// # }
    /// ```
    pub fn record_access_background(self: Arc<Self>, current: ContextId, next: ContextId) {
        tokio::spawn(async move {
            self.record_access(&current, &next);
        });
    }

    /// 预测下一步可能访问的上下文及概率 — 按概率降序排列
    ///
    /// # 参数
    /// - `current`:当前上下文 ID
    ///
    /// # 返回
    /// `(ContextId, 概率)` 列表,按概率降序。未知上下文返回空 Vec。
    ///
    /// # 概率计算
    /// `P(next | current) = count(current → next) / Σ count(current → *)`
    pub fn predict_next(&self, current: &ContextId) -> Vec<(ContextId, f32)> {
        let patterns = self.patterns.read().unwrap_or_else(|e| {
            tracing::warn!("patterns RwLock poisoned, recovering");
            e.into_inner()
        });

        let transitions = match patterns.get_transitions(current) {
            Some(t) => t,
            None => return Vec::new(),
        };

        let total: u32 = transitions.values().sum();
        if total == 0 {
            return Vec::new();
        }

        let mut predictions: Vec<(ContextId, f32)> = transitions
            .iter()
            .map(|(id, &count)| (id.clone(), count as f32 / total as f32))
            .collect();

        // 按概率降序排列(partial_cmp 安全处理 NaN)
        predictions.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        predictions
    }

    /// 获取指定上下文的访问模式快照
    ///
    /// 返回 `AccessPattern`,包含当前上下文 ID 与转移计数列表(按计数降序)。
    /// 未知上下文返回 None。
    pub fn get_pattern(&self, current: &ContextId) -> Option<AccessPattern> {
        let patterns = self.patterns.read().unwrap_or_else(|e| {
            tracing::warn!("patterns RwLock poisoned, recovering");
            e.into_inner()
        });

        patterns.get_transitions(current).map(|transitions| {
            let mut sorted: Vec<(ContextId, u32)> =
                transitions.iter().map(|(id, &c)| (id.clone(), c)).collect();
            sorted.sort_by_key(|b| std::cmp::Reverse(b.1));
            AccessPattern {
                current: current.clone(),
                transitions: sorted,
            }
        })
    }

    /// 推测性预取 — 对高概率上下文异步预热到缓存
    ///
    /// # 行为
    /// 1. 调用 `predict_next` 获取预测列表
    /// 2. 过滤概率 >= `prefetch_threshold` 的上下文
    /// 3. `tokio::spawn` 后台任务:对每个预测上下文调用 `cache.warm_entry`
    /// 4. 后台任务完成后发布 `CachePrefetched` 事件(携带成功预热的 ID 列表)
    /// 5. 立即返回预测 ID 列表(不等待后台任务完成)
    ///
    /// # 跨层依赖修正(spec.md 决策 5)
    /// SCC(L3)→ GQEP(L6) 向上依赖禁止。预取逻辑在 SCC 内部用 `tokio::spawn`
    /// 后台任务完成,不调用上层 crate。
    ///
    /// # 预取失败处理
    /// 预测的上下文不在缓存中时(无后端存储可加载),仅 `tracing::warn!` 日志,
    /// 不返回错误,不阻塞主流程。
    ///
    /// # 注意
    /// 此方法调用 `tokio::spawn`,必须在 Tokio 运行时上下文中调用。
    pub fn prefetch(&self, current: &ContextId, cache: &SccCache) -> Vec<ContextId> {
        let predictions = self.predict_next(current);
        let threshold = self.prefetch_threshold;

        // 过滤概率 >= 阈值的上下文
        let to_prefetch: Vec<ContextId> = predictions
            .into_iter()
            .filter(|(_, prob)| *prob >= threshold)
            .map(|(id, _)| id)
            .collect();

        if to_prefetch.is_empty() {
            return Vec::new();
        }

        // 克隆数据用于后台任务(SccCache 是 Clone,共享内部 Arc 状态)
        let cache_clone = cache.clone();
        let event_bus = self.event_bus.clone();
        let task_ids = to_prefetch.clone();

        tokio::spawn(async move {
            let mut warmed_ids = Vec::new();
            for id in &task_ids {
                if cache_clone.warm_entry(id) {
                    warmed_ids.push(id.to_string());
                } else {
                    // 预取失败:上下文不在缓存中,静默处理(仅 warn 日志)
                    tracing::warn!(context_id = %id, "预取失败:上下文不在缓存中");
                }
            }

            // 预取完成后发布 CachePrefetched 事件(仅携带成功预热的 ID)
            if !warmed_ids.is_empty() {
                let _ = event_bus
                    .publish(NexusEvent::CachePrefetched {
                        metadata: EventMetadata::new("scc-cache"),
                        prefetched_ids: warmed_ids,
                    })
                    .await;
            }
        });

        to_prefetch
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SccConfig;
    use crate::ContextEntry;

    fn make_learner() -> AccessPatternLearner {
        AccessPatternLearner::new(EventBus::new(), 0.6)
    }

    #[test]
    fn test_record_access_and_predict() {
        let learner = make_learner();
        let ctx_a = ContextId::new("ctx-a");
        let ctx_b = ContextId::new("ctx-b");
        let ctx_c = ContextId::new("ctx-c");

        // 记录转移:a → b 三次,a → c 一次
        learner.record_access(&ctx_a, &ctx_b);
        learner.record_access(&ctx_a, &ctx_b);
        learner.record_access(&ctx_a, &ctx_b);
        learner.record_access(&ctx_a, &ctx_c);

        let predictions = learner.predict_next(&ctx_a);
        assert_eq!(predictions.len(), 2);

        // b 概率 3/4 = 0.75,c 概率 1/4 = 0.25
        assert_eq!(predictions[0].0.as_str(), "ctx-b");
        assert!((predictions[0].1 - 0.75).abs() < 0.01);
        assert_eq!(predictions[1].0.as_str(), "ctx-c");
        assert!((predictions[1].1 - 0.25).abs() < 0.01);
    }

    #[test]
    fn test_predict_unknown_context() {
        let learner = make_learner();
        let unknown = ContextId::new("ctx-unknown");
        let predictions = learner.predict_next(&unknown);
        assert!(predictions.is_empty());
    }

    #[test]
    fn test_predict_sorted_by_probability_desc() {
        let learner = make_learner();
        let ctx_a = ContextId::new("ctx-a");

        // a → b 一次,a → c 五次
        learner.record_access(&ctx_a, &ContextId::new("ctx-b"));
        for _ in 0..5 {
            learner.record_access(&ctx_a, &ContextId::new("ctx-c"));
        }

        let predictions = learner.predict_next(&ctx_a);
        // c (5/6 ≈ 0.83) 应排在 b (1/6 ≈ 0.17) 前面
        assert_eq!(predictions[0].0.as_str(), "ctx-c");
        assert_eq!(predictions[1].0.as_str(), "ctx-b");
    }

    #[test]
    fn test_get_pattern() {
        let learner = make_learner();
        let ctx_a = ContextId::new("ctx-a");

        learner.record_access(&ctx_a, &ContextId::new("ctx-b"));
        learner.record_access(&ctx_a, &ContextId::new("ctx-c"));
        learner.record_access(&ctx_a, &ContextId::new("ctx-c"));

        let pattern = learner.get_pattern(&ctx_a);
        assert!(pattern.is_some());
        let pattern = pattern.unwrap();
        assert_eq!(pattern.current.as_str(), "ctx-a");
        assert_eq!(pattern.transitions.len(), 2);
        // 按计数降序:c (2) 在 b (1) 前面
        assert_eq!(pattern.transitions[0].0.as_str(), "ctx-c");
        assert_eq!(pattern.transitions[0].1, 2);
        assert_eq!(pattern.transitions[1].0.as_str(), "ctx-b");
        assert_eq!(pattern.transitions[1].1, 1);
    }

    #[test]
    fn test_get_pattern_unknown() {
        let learner = make_learner();
        let unknown = ContextId::new("ctx-unknown");
        assert!(learner.get_pattern(&unknown).is_none());
    }

    #[tokio::test]
    async fn test_record_access_background() {
        let learner = Arc::new(make_learner());
        let ctx_a = ContextId::new("ctx-a");
        let ctx_b = ContextId::new("ctx-b");

        // 后台记录转移
        Arc::clone(&learner).record_access_background(ctx_a.clone(), ctx_b.clone());

        // 等待后台任务完成
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // 验证模式已记录
        let predictions = learner.predict_next(&ctx_a);
        assert_eq!(predictions.len(), 1);
        assert_eq!(predictions[0].0.as_str(), "ctx-b");
        assert!((predictions[0].1 - 1.0).abs() < 0.01);
    }

    #[tokio::test]
    async fn test_prefetch_returns_high_probability_ids() {
        let bus = EventBus::new();
        let cache = SccCache::new(SccConfig::default(), bus.clone());
        let learner = AccessPatternLearner::new(bus, 0.6);

        let ctx_a = ContextId::new("ctx-a");
        let ctx_b = ContextId::new("ctx-b");
        let ctx_c = ContextId::new("ctx-c");

        // 训练模式:a → b 概率 0.75(>= 0.6),a → c 概率 0.25(< 0.6)
        for _ in 0..3 {
            learner.record_access(&ctx_a, &ctx_b);
        }
        learner.record_access(&ctx_a, &ctx_c);

        // 预取应只返回 ctx-b(概率 0.75 >= 0.6)
        let prefetched = learner.prefetch(&ctx_a, &cache);
        assert_eq!(prefetched.len(), 1);
        assert_eq!(prefetched[0].as_str(), "ctx-b");
    }

    #[tokio::test]
    async fn test_prefetch_no_predictions() {
        let bus = EventBus::new();
        let cache = SccCache::new(SccConfig::default(), bus.clone());
        let learner = AccessPatternLearner::new(bus, 0.6);

        // 未知上下文,无预测
        let unknown = ContextId::new("ctx-unknown");
        let prefetched = learner.prefetch(&unknown, &cache);
        assert!(prefetched.is_empty());
    }

    #[tokio::test]
    async fn test_prefetch_warms_existing_entries() {
        let bus = EventBus::new();
        let cache = SccCache::new(SccConfig::default(), bus.clone());
        let learner = AccessPatternLearner::new(bus, 0.5);

        let ctx_a = ContextId::new("ctx-a");
        let ctx_b = ContextId::new("ctx-b");

        // 插入 ctx-b 到缓存
        cache.insert(ContextEntry::new("ctx-b", "content-b"));

        // 训练模式:a → b 概率 1.0
        learner.record_access(&ctx_a, &ctx_b);

        // 预取:ctx-b 在缓存中,应被预热
        let prefetched = learner.prefetch(&ctx_a, &cache);
        assert_eq!(prefetched.len(), 1);

        // 等待后台任务完成
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // 验证 ctx-b 被预热(access_count 增加)
        let entry = cache.get_or_prefetch(&ctx_b).unwrap();
        // warm_entry 调用 record_access 一次,get_or_prefetch 又一次
        assert!(entry.access_count() >= 2);
    }

    #[tokio::test]
    async fn test_prefetch_missing_entry_silent() {
        let bus = EventBus::new();
        let cache = SccCache::new(SccConfig::default(), bus.clone());
        let learner = AccessPatternLearner::new(bus, 0.5);

        let ctx_a = ContextId::new("ctx-a");
        let ctx_b = ContextId::new("ctx-b");

        // 训练模式但不插入 ctx-b 到缓存
        learner.record_access(&ctx_a, &ctx_b);

        // 预取:ctx-b 不在缓存中,应静默失败(仅 warn 日志)
        let prefetched = learner.prefetch(&ctx_a, &cache);
        assert_eq!(prefetched.len(), 1); // 返回预测 ID(不管是否在缓存中)

        // 等待后台任务完成(不应 panic)
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;

        // ctx-b 不在缓存中
        assert!(!cache.contains(&ctx_b));
    }
}
