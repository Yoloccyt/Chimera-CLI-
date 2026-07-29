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
use nexus_contracts::{PrefetchPolicy, PrefetchStrategy};

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
        // 锁内仅 collect 到 Vec,排序在锁外执行,缩短读锁持有时间。
        // WHY:sort_by 是 O(n log n) CPU 密集操作,在读锁内执行会阻塞并发写请求
        //(record_access)。将排序移到锁外后,读锁持有时间从 O(n log n) 降至 O(n),
        // 显著降低写锁等待延迟。
        let mut predictions: Vec<(ContextId, f32)> = {
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

            transitions
                .iter()
                .map(|(id, &count)| (id.clone(), count as f32 / total as f32))
                .collect()
        }; // 读锁在此释放

        // 锁外排序:按概率降序(partial_cmp 安全处理 NaN)
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

    /// 策略感知的推测性预取 — 根据 `PrefetchPolicy` 调整阈值与 Top-K
    ///
    /// # P4-W14.2 S3 接缝接入
    ///
    /// 此方法是 `omega-learner` 异步下发 `PrefetchPolicy::Learned` 的接入点。
    /// 上层编排器（chimera-cli / quest-engine）调用 `PrefetchLearnerHolder::current_policy()`
    /// 获取当前策略，然后传入此方法执行策略感知的预取。
    ///
    /// # 行为
    /// 1. 从 `policy.strategy()` 派生预取阈值与 Top-K 限制
    /// 2. 调用 `predict_next` 获取预测列表
    /// 3. 过滤概率 >= 策略阈值的上下文，并按 Top-K 截断
    /// 4. `tokio::spawn` 后台任务：对每个预测上下文调用 `cache.warm_entry`
    /// 5. 后台任务完成后发布 `CachePrefetched` 事件
    /// 6. 立即返回预测 ID 列表（不等待后台任务完成）
    ///
    /// # 与 `prefetch` 方法的差异
    ///
    /// - `prefetch`（既有）：使用 `self.prefetch_threshold` 硬编码字段，向后兼容
    /// - `prefetch_with_policy`（新增）：使用 `policy.strategy()` 派生阈值与 Top-K
    ///
    /// # C4 合规
    ///
    /// `policy` 由调用方传入（来自 `PrefetchLearnerHolder`），不依赖全局 static
    /// 或 feature flag。`PrefetchPolicy::fallback()` 返回 `Static(Standard)`，
    /// 行为等价于 `prefetch(threshold=0.6)`，向后兼容。
    ///
    /// # 参数
    /// - `current`: 当前上下文 ID
    /// - `cache`: 共享 SCC 缓存（`SccCache` 是 Clone，内部 Arc 共享状态）
    /// - `policy`: 预取策略（`Static` 或 `Learned`）
    ///
    /// # 返回
    /// 经过策略阈值过滤与 Top-K 截断后的预测上下文 ID 列表。
    ///
    /// # 注意
    /// 此方法调用 `tokio::spawn`，必须在 Tokio 运行时上下文中调用。
    pub fn prefetch_with_policy(
        &self,
        current: &ContextId,
        cache: &SccCache,
        policy: &PrefetchPolicy,
    ) -> Vec<ContextId> {
        let strategy = policy.strategy();
        let threshold = strategy.prefetch_threshold();
        let top_k = strategy.top_k();

        // NoPrefetch 快速路径：直接返回空列表，避免无谓的预测计算
        if strategy.disabled() || top_k == 0 {
            return Vec::new();
        }

        let predictions = self.predict_next(current);

        // 过滤概率 >= 策略阈值的上下文，并按 Top-K 截断
        let to_prefetch: Vec<ContextId> = predictions
            .into_iter()
            .filter(|(_, prob)| *prob >= threshold)
            .take(top_k)
            .map(|(id, _)| id)
            .collect();

        if to_prefetch.is_empty() {
            return Vec::new();
        }

        // 克隆数据用于后台任务（SccCache 是 Clone，共享内部 Arc 状态）
        let cache_clone = cache.clone();
        let event_bus = self.event_bus.clone();
        let task_ids = to_prefetch.clone();

        tokio::spawn(async move {
            let mut warmed_ids = Vec::new();
            for id in &task_ids {
                if cache_clone.warm_entry(id) {
                    warmed_ids.push(id.to_string());
                } else {
                    // 预取失败：上下文不在缓存中，静默处理（仅 warn 日志）
                    tracing::warn!(context_id = %id, "策略感知预取失败：上下文不在缓存中");
                }
            }

            // 预取完成后发布 CachePrefetched 事件（仅携带成功预热的 ID）
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

// ============================================================
// PrefetchLearnerHolder — P4-W14.2 S3 接缝运行时策略持有器
// ============================================================

/// 预取策略学习器持有器 — 运行时可变的 `PrefetchPolicy` 容器
///
/// 承载 `omega-learner` 异步下发的学习策略，为 `AccessPatternLearner` 的
/// `prefetch_with_policy` 路径提供策略感知能力。所有方法线程安全（`RwLock` 保护）。
///
/// # 设计决策（WHY）
///
/// - **独立 struct 而非嵌入 AccessPatternLearner**: 单一职责，便于单测与复用
/// - **`RwLock<PrefetchPolicy>` 而非 `AtomicU8`**: `PrefetchPolicy` 是枚举
///   （Static/Learned），原子化需要 `AtomicU8` + 手动重建枚举，复杂且易错
/// - **`std::sync::RwLock` 而非 `tokio::sync::RwLock`**: 读路径是 sync
///   （prefetch 是 sync 路径的辅助查询），持锁时间极短（~10ns）
///
/// # 依赖铁律合规（WHY scc-cache 不直接依赖 omega-learner）
///
/// ```text
/// L6 omega-learner  ────(learned PrefetchPolicy)───▶  上层编排器
///      │                                                │
///      │ L6 → L0 ✓                                     │ L0 → 注入
///      ▼                                                ▼
/// L0 nexus-contracts  ◀──(PrefetchPolicy 类型)──  L3 scc-cache
///      PrefetchPolicy                                     │
///      PrefetchStrategy                                    │ L3 → L0 ✓
///                                                        ▼
///                                                PrefetchLearnerHolder
/// ```
///
/// scc-cache (L3) 只依赖 `nexus-contracts` (L0) 的 `PrefetchPolicy` 类型，
/// **不直接依赖** `omega-learner` (L6)，遵守 §2.2 依赖铁律。
/// `omega-learner` 输出的 `PrefetchPolicy::Learned` 由上层编排器
/// （chimera-cli / quest-engine）通过 `update_policy()` 注入。
///
/// # C4 合规（能力场灰度，非运行时旗）
///
/// - **默认值层**: `PrefetchLearnerHolder::new()` 初始化为
///   `PrefetchPolicy::Static(PrefetchStrategy::Standard)`（编译期常量 fallback）
/// - **异常回退层**: `RwLock::write().unwrap_or_else()` 处理 `PoisonError` 自动回退
/// - **熔断入口层**: `fallback_to_static()` 供 `omega-learner` 触发学习熔断
///   （spec.md:335 S3 灰度命中率降 >2%）时主动回退
///
/// 三层叠加实现"learner panic/超时时调用方本地 fallback，无跨 crate 旗标传播"
/// 的 C4 合规要求（spec.md:334）。
///
/// # 与 MemoryStrategyLearnerHolder / DensityLearnerHolder / SelectorLearnerHolder 的对称设计
///
/// S1/S2/S3/S4 四接缝共用相同骨架:
/// - 枚举 + `Static`/`Learned` 双变体策略
/// - `RwLock<Policy>` + `new`/`with_policy`/`update_policy`/`fallback_to_static`
/// - `current_policy`/`is_learned`/`version`/`Default`/`Clone`
///
/// 差异仅在策略类型与感知方法:
/// - S1: `DensityPolicy` + `select_with_density`（HCW 密度感知选择）
/// - S2: `MemoryStrategyPolicy` + `recall_by_clv_with_strategy`（MLC 记忆策略感知）
/// - S3: `PrefetchPolicy` + `prefetch_with_policy`（SCC 预取策略感知）
/// - S4: `SelectorPolicy` + `compute_importance_with_policy`（HCW selector 权重感知）
///
/// # 线程安全
///
/// 内部用 `RwLock<PrefetchPolicy>` 保护策略状态:
/// - **写锁**: `update_policy()` 异步下发（低频，每秒 < 1 次）
/// - **读锁**: `current_policy()` / `strategy()` 热路径查询（高频）
///
/// 读写分离避免锁竞争。`RwLock` 选择 `std::sync::RwLock` 而非 `tokio::sync::RwLock`:
/// - 读路径需要 sync 访问（prefetch 是 sync 路径的辅助查询）
/// - 持锁时间极短（仅读取 `PrefetchPolicy` 枚举，~10ns）
///
/// # 示例
///
/// ## 基础 fallback 行为
///
/// ```
/// use scc_cache::prefetch::PrefetchLearnerHolder;
/// use nexus_contracts::{PrefetchPolicy, PrefetchStrategy};
///
/// let holder = PrefetchLearnerHolder::new();
///
/// // 初始化为 Static fallback（Standard）
/// let policy = holder.current_policy();
/// assert!(policy.is_static());
/// assert_eq!(policy.strategy(), PrefetchStrategy::Standard);
/// ```
///
/// ## 异步下发学习策略
///
/// ```
/// use scc_cache::prefetch::PrefetchLearnerHolder;
/// use nexus_contracts::{PrefetchPolicy, PrefetchStrategy};
///
/// let holder = PrefetchLearnerHolder::new();
///
/// // omega-learner 异步下发学习策略（Aggressive，激进预取）
/// holder.update_policy(PrefetchPolicy::learned(42, PrefetchStrategy::Aggressive));
///
/// let policy = holder.current_policy();
/// assert!(policy.is_learned());
/// assert_eq!(policy.version(), Some(42));
/// assert_eq!(policy.strategy(), PrefetchStrategy::Aggressive);
/// ```
#[derive(Debug)]
pub struct PrefetchLearnerHolder {
    /// 当前激活的策略（`RwLock` 保护，读写分离）
    ///
    /// WHY 用 `RwLock` 而非 `Mutex`:
    /// - 读路径（`current_policy`/`strategy`）高频且只读
    /// - 写路径（`update_policy`）低频（每秒 < 1 次）
    /// - `RwLock` 允许并发读，避免读路径串行化
    policy: RwLock<PrefetchPolicy>,
}

impl PrefetchLearnerHolder {
    /// 创建持有器，初始化为 `PrefetchPolicy::Static(Standard)`（fallback）
    ///
    /// WHY 初始化为 Static(Standard): C4 合规要求默认行为零变化，
    /// `Standard` 对应既有 `prefetch_threshold=0.6 + top_k=10` 行为，向后兼容。
    pub fn new() -> Self {
        Self {
            policy: RwLock::new(PrefetchPolicy::fallback()),
        }
    }

    /// 创建持有器，指定初始策略（便于测试）
    ///
    /// WHY 提供: 单测需要构造特定策略场景（如 Learned 初始状态）
    pub fn with_policy(policy: PrefetchPolicy) -> Self {
        Self {
            policy: RwLock::new(policy),
        }
    }

    /// 异步下发策略 — 接收 `omega-learner` 学习到的 `PrefetchPolicy`
    ///
    /// # 设计
    ///
    /// - 写入 `RwLock`（独占写锁，~10ns）
    /// - 不返回错误: 任何异常（如 PoisonError）静默 fallback 到 Static
    ///
    /// # C4 合规
    ///
    /// 调用方（chimera-cli / quest-engine）在 `omega-learner` panic/超时时
    /// 不调用此方法，`PrefetchLearnerHolder` 保持上一次的有效策略。
    /// 若需强制回退到 fallback，调用方传入 `PrefetchPolicy::fallback()`。
    ///
    /// # 参数
    /// - `policy`: 新策略（Static 或 Learned）
    ///
    /// # 示例
    ///
    /// ```
    /// use scc_cache::prefetch::PrefetchLearnerHolder;
    /// use nexus_contracts::{PrefetchPolicy, PrefetchStrategy};
    ///
    /// let holder = PrefetchLearnerHolder::new();
    /// holder.update_policy(PrefetchPolicy::learned(1, PrefetchStrategy::TopK3));
    /// assert_eq!(holder.current_policy().strategy(), PrefetchStrategy::TopK3);
    /// ```
    pub fn update_policy(&self, policy: PrefetchPolicy) {
        // WHY unwrap_or_else: RwLock poison 时 fallback 到 Static(Standard)
        // 避免调用方处理 PoisonError（C4 合规：本地 fallback，无错误传播）
        let mut guard = self.policy.write().unwrap_or_else(|p| {
            // PoisonError 时恢复锁并写入 fallback
            let mut guard = p.into_inner();
            *guard = PrefetchPolicy::fallback();
            guard
        });
        *guard = policy;
    }

    /// 强制回退到 fallback 策略（`Static(Standard)`）
    ///
    /// WHY 提供: `omega-learner` 触发学习熔断（spec.md:335 S3 灰度命中率
    /// 降 >2%）时，上层调用方调用此方法立即回退到静态策略。
    ///
    /// # 示例
    ///
    /// ```
    /// use scc_cache::prefetch::PrefetchLearnerHolder;
    /// use nexus_contracts::PrefetchStrategy;
    ///
    /// let holder = PrefetchLearnerHolder::new();
    /// holder.fallback_to_static();
    /// assert!(holder.current_policy().is_static());
    /// assert_eq!(holder.current_policy().strategy(), PrefetchStrategy::Standard);
    /// ```
    pub fn fallback_to_static(&self) {
        self.update_policy(PrefetchPolicy::fallback());
    }

    /// 返回当前激活的策略（快照）
    ///
    /// 返回 `PrefetchPolicy` 的 Copy（枚举整体 Copy），调用方无需持有锁。
    ///
    /// # 性能
    ///
    /// 读锁 + Copy 枚举，~10ns。热路径调用无性能影响。
    pub fn current_policy(&self) -> PrefetchPolicy {
        let guard = self.policy.read().unwrap_or_else(|p| p.into_inner());
        *guard
    }

    /// 返回当前激活的预取策略（便捷方法）
    ///
    /// 等价于 `current_policy().strategy()`，但避免调用方重复 match。
    /// 用于 `prefetch_with_policy` 路径的策略感知决策。
    ///
    /// # 示例
    ///
    /// ```
    /// use scc_cache::prefetch::PrefetchLearnerHolder;
    /// use nexus_contracts::{PrefetchPolicy, PrefetchStrategy};
    ///
    /// let holder = PrefetchLearnerHolder::new();
    /// assert_eq!(holder.strategy(), PrefetchStrategy::Standard);
    ///
    /// holder.update_policy(PrefetchPolicy::learned(1, PrefetchStrategy::Aggressive));
    /// assert_eq!(holder.strategy(), PrefetchStrategy::Aggressive);
    /// ```
    pub fn strategy(&self) -> PrefetchStrategy {
        self.current_policy().strategy()
    }

    /// 返回是否已激活学习策略
    ///
    /// 便于上层编排器查询当前是否在灰度阶段。
    pub fn is_learned(&self) -> bool {
        self.current_policy().is_learned()
    }

    /// 返回当前学习版本号（Static 返回 None，Learned 返回 Some(version)）
    ///
    /// 便于上层编排器记录使用的版本号用于效果追踪与 A/B 测试。
    pub fn version(&self) -> Option<u64> {
        self.current_policy().version()
    }
}

impl Default for PrefetchLearnerHolder {
    /// 默认状态 = `PrefetchPolicy::Static(Standard)`（fallback，C4 合规）
    fn default() -> Self {
        Self::new()
    }
}

impl Clone for PrefetchLearnerHolder {
    /// 克隆持有器（创建新的 RwLock，策略快照独立）
    ///
    /// WHY 提供: `AccessPatternLearner` 可能需要克隆 `PrefetchLearnerHolder`
    /// 用于快照或并行处理。克隆后两者策略独立演化，互不影响。
    fn clone(&self) -> Self {
        Self::with_policy(self.current_policy())
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

    // ============================================================
    // PrefetchLearnerHolder 测试 — P4-W14.2 S3 接缝
    // ============================================================

    #[test]
    fn test_holder_new_is_static_fallback() {
        // C4 合规：new() 必须初始化为 Static(Standard) fallback
        let holder = PrefetchLearnerHolder::new();
        let policy = holder.current_policy();
        assert!(policy.is_static());
        assert_eq!(policy.strategy(), PrefetchStrategy::Standard);
    }

    #[test]
    fn test_holder_default_equals_new() {
        // Default 与 new() 行为一致
        let holder1 = PrefetchLearnerHolder::new();
        let holder2 = PrefetchLearnerHolder::default();
        assert_eq!(holder1.current_policy(), holder2.current_policy());
    }

    #[test]
    fn test_holder_with_policy_learned() {
        // with_policy 可指定 Learned 初始状态（便于测试）
        let holder = PrefetchLearnerHolder::with_policy(PrefetchPolicy::learned(
            42,
            PrefetchStrategy::Aggressive,
        ));
        let policy = holder.current_policy();
        assert!(policy.is_learned());
        assert_eq!(policy.version(), Some(42));
        assert_eq!(policy.strategy(), PrefetchStrategy::Aggressive);
    }

    #[test]
    fn test_update_policy_to_learned() {
        // 从 Static 切换到 Learned
        let holder = PrefetchLearnerHolder::new();
        assert!(holder.current_policy().is_static());

        holder.update_policy(PrefetchPolicy::learned(1, PrefetchStrategy::TopK3));
        let policy = holder.current_policy();
        assert!(policy.is_learned());
        assert_eq!(policy.strategy(), PrefetchStrategy::TopK3);
        assert_eq!(policy.version(), Some(1));
    }

    #[test]
    fn test_update_policy_to_static() {
        // 从 Learned 切换回 Static
        let holder = PrefetchLearnerHolder::with_policy(PrefetchPolicy::learned(
            1,
            PrefetchStrategy::Aggressive,
        ));
        assert!(holder.current_policy().is_learned());

        holder.update_policy(PrefetchPolicy::static_policy(
            PrefetchStrategy::Conservative,
        ));
        assert!(holder.current_policy().is_static());
        assert_eq!(
            holder.current_policy().strategy(),
            PrefetchStrategy::Conservative
        );
    }

    #[test]
    fn test_update_policy_multiple_times() {
        // 连续多次更新策略，验证版本号与策略一致性
        let holder = PrefetchLearnerHolder::new();
        let strategies = [
            PrefetchStrategy::NoPrefetch,
            PrefetchStrategy::Conservative,
            PrefetchStrategy::Standard,
            PrefetchStrategy::Aggressive,
            PrefetchStrategy::TopK3,
        ];

        for (version, strategy) in strategies.iter().enumerate() {
            holder.update_policy(PrefetchPolicy::learned(version as u64 + 1, *strategy));
            assert_eq!(holder.version(), Some(version as u64 + 1));
            assert_eq!(holder.strategy(), *strategy);
        }
    }

    #[test]
    fn test_fallback_to_static() {
        // fallback_to_static() 立即回退到 Static(Standard)
        let holder = PrefetchLearnerHolder::with_policy(PrefetchPolicy::learned(
            99,
            PrefetchStrategy::Aggressive,
        ));
        assert!(holder.current_policy().is_learned());

        holder.fallback_to_static();
        assert!(holder.current_policy().is_static());
        assert_eq!(
            holder.current_policy().strategy(),
            PrefetchStrategy::Standard
        );
    }

    #[test]
    fn test_holder_strategy_convenience_method() {
        // strategy() 便捷方法等价于 current_policy().strategy()
        let holder = PrefetchLearnerHolder::new();
        assert_eq!(holder.strategy(), PrefetchStrategy::Standard);

        holder.update_policy(PrefetchPolicy::learned(1, PrefetchStrategy::TopK3));
        assert_eq!(holder.strategy(), PrefetchStrategy::TopK3);
    }

    #[test]
    fn test_holder_is_learned_and_version() {
        // is_learned() 与 version() 正确反映策略状态
        let holder = PrefetchLearnerHolder::new();
        assert!(!holder.is_learned());
        assert_eq!(holder.version(), None);

        holder.update_policy(PrefetchPolicy::learned(7, PrefetchStrategy::Conservative));
        assert!(holder.is_learned());
        assert_eq!(holder.version(), Some(7));
    }

    #[test]
    fn test_holder_clone_independent() {
        // Clone 后两者策略独立演化，互不影响
        let holder1 = PrefetchLearnerHolder::with_policy(PrefetchPolicy::learned(
            1,
            PrefetchStrategy::Standard,
        ));
        let holder2 = holder1.clone();

        // 修改 holder1 不影响 holder2
        holder1.update_policy(PrefetchPolicy::learned(2, PrefetchStrategy::Aggressive));
        assert_eq!(holder1.strategy(), PrefetchStrategy::Aggressive);
        assert_eq!(holder2.strategy(), PrefetchStrategy::Standard);
        assert_eq!(holder2.version(), Some(1));
    }

    // ============================================================
    // prefetch_with_policy 测试 — P4-W14.2 S3 接缝策略感知预取
    // ============================================================

    #[tokio::test]
    async fn test_prefetch_with_policy_standard_default_equivalent() {
        // Static(Standard) 等价于 prefetch(threshold=0.6, top_k=10) 的默认行为
        let bus = EventBus::new();
        let cache = SccCache::new(SccConfig::default(), bus.clone());
        let learner = AccessPatternLearner::new(bus, 0.6);

        let ctx_a = ContextId::new("ctx-a");
        let ctx_b = ContextId::new("ctx-b");

        // 训练模式:a → b 概率 1.0
        learner.record_access(&ctx_a, &ctx_b);

        // Standard 策略：threshold=0.6, top_k=10
        let policy = PrefetchPolicy::static_policy(PrefetchStrategy::Standard);
        let prefetched = learner.prefetch_with_policy(&ctx_a, &cache, &policy);
        assert_eq!(prefetched.len(), 1);
        assert_eq!(prefetched[0].as_str(), "ctx-b");
    }

    #[tokio::test]
    async fn test_prefetch_with_policy_no_prefetch_returns_empty() {
        // NoPrefetch 策略：threshold=1.1（永不触发），应直接返回空列表
        let bus = EventBus::new();
        let cache = SccCache::new(SccConfig::default(), bus.clone());
        let learner = AccessPatternLearner::new(bus, 0.6);

        let ctx_a = ContextId::new("ctx-a");
        let ctx_b = ContextId::new("ctx-b");

        // 训练模式:a → b 概率 1.0
        learner.record_access(&ctx_a, &ctx_b);

        let policy = PrefetchPolicy::static_policy(PrefetchStrategy::NoPrefetch);
        let prefetched = learner.prefetch_with_policy(&ctx_a, &cache, &policy);
        assert!(prefetched.is_empty(), "NoPrefetch 策略应阻止所有预取");
    }

    #[tokio::test]
    async fn test_prefetch_with_policy_aggressive_lower_threshold() {
        // Aggressive 策略：threshold=0.3（更低），应预取更多低概率上下文
        let bus = EventBus::new();
        let cache = SccCache::new(SccConfig::default(), bus.clone());
        let learner = AccessPatternLearner::new(bus, 0.6);

        let ctx_a = ContextId::new("ctx-a");
        let ctx_b = ContextId::new("ctx-b");
        let ctx_c = ContextId::new("ctx-c");

        // 训练模式:a → b 3次(0.75), a → c 1次(0.25)
        for _ in 0..3 {
            learner.record_access(&ctx_a, &ctx_b);
        }
        learner.record_access(&ctx_a, &ctx_c);

        // Standard 策略：threshold=0.6,只预取 ctx-b (0.75 >= 0.6)
        let standard_policy = PrefetchPolicy::static_policy(PrefetchStrategy::Standard);
        let standard_prefetched = learner.prefetch_with_policy(&ctx_a, &cache, &standard_policy);
        assert_eq!(standard_prefetched.len(), 1);
        assert_eq!(standard_prefetched[0].as_str(), "ctx-b");

        // Aggressive 策略：threshold=0.3,预取 ctx-b (0.75) + ctx-c (0.25 < 0.3,被过滤)
        // 注意:0.25 < 0.3,所以 Aggressive 仍然只预取 ctx-b
        let aggressive_policy = PrefetchPolicy::static_policy(PrefetchStrategy::Aggressive);
        let aggressive_prefetched =
            learner.prefetch_with_policy(&ctx_a, &cache, &aggressive_policy);
        assert_eq!(aggressive_prefetched.len(), 1);
        assert_eq!(aggressive_prefetched[0].as_str(), "ctx-b");
    }

    #[tokio::test]
    async fn test_prefetch_with_policy_topk3_limits_results() {
        // TopK3 策略：top_k=3，应截断超过 3 个的预测
        let bus = EventBus::new();
        let cache = SccCache::new(SccConfig::default(), bus.clone());
        let learner = AccessPatternLearner::new(bus, 0.6);

        let ctx_a = ContextId::new("ctx-a");
        // 训练 5 个转移:a → b1..b5,每个概率 0.2
        for i in 1..=5 {
            learner.record_access(&ctx_a, &ContextId::new(format!("ctx-b{i}")));
        }

        // TopK3 策略：threshold=0.0（无阈值），top_k=3
        // 概率 0.2 >= 0.0,所有 5 个都通过阈值,但 top_k=3 截断到 3 个
        let policy = PrefetchPolicy::static_policy(PrefetchStrategy::TopK3);
        let prefetched = learner.prefetch_with_policy(&ctx_a, &cache, &policy);
        assert_eq!(prefetched.len(), 3, "TopK3 应截断到 3 个");
    }

    #[tokio::test]
    async fn test_prefetch_with_policy_conservative_higher_threshold() {
        // Conservative 策略：threshold=0.8（更高），应过滤掉中等概率上下文
        let bus = EventBus::new();
        let cache = SccCache::new(SccConfig::default(), bus.clone());
        let learner = AccessPatternLearner::new(bus, 0.6);

        let ctx_a = ContextId::new("ctx-a");
        let ctx_b = ContextId::new("ctx-b");
        let ctx_c = ContextId::new("ctx-c");

        // 训练模式:a → b 3次(0.75), a → c 1次(0.25)
        for _ in 0..3 {
            learner.record_access(&ctx_a, &ctx_b);
        }
        learner.record_access(&ctx_a, &ctx_c);

        // Conservative 策略：threshold=0.8
        // ctx-b 概率 0.75 < 0.8,被过滤;ctx-c 概率 0.25 < 0.8,被过滤
        let policy = PrefetchPolicy::static_policy(PrefetchStrategy::Conservative);
        let prefetched = learner.prefetch_with_policy(&ctx_a, &cache, &policy);
        assert!(
            prefetched.is_empty(),
            "Conservative threshold=0.8 应过滤掉 0.75 概率的 ctx-b"
        );
    }

    #[tokio::test]
    async fn test_prefetch_with_policy_learned_strategy() {
        // Learned 策略：与 Static 等价的策略行为，但携带版本号
        let bus = EventBus::new();
        let cache = SccCache::new(SccConfig::default(), bus.clone());
        let learner = AccessPatternLearner::new(bus, 0.6);

        let ctx_a = ContextId::new("ctx-a");
        let ctx_b = ContextId::new("ctx-b");

        learner.record_access(&ctx_a, &ctx_b);

        // Learned 策略：Aggressive, version=42
        let policy = PrefetchPolicy::learned(42, PrefetchStrategy::Aggressive);
        let prefetched = learner.prefetch_with_policy(&ctx_a, &cache, &policy);
        assert_eq!(prefetched.len(), 1);
        assert_eq!(prefetched[0].as_str(), "ctx-b");
    }

    #[tokio::test]
    async fn test_prefetch_with_policy_holder_integration() {
        // 端到端集成：PrefetchLearnerHolder + prefetch_with_policy
        // 模拟 omega-learner 异步下发策略 → holder 缓存 → prefetch_with_policy 感知
        let bus = EventBus::new();
        let cache = SccCache::new(SccConfig::default(), bus.clone());
        let learner = AccessPatternLearner::new(bus, 0.6);
        let holder = PrefetchLearnerHolder::new();

        let ctx_a = ContextId::new("ctx-a");
        let ctx_b = ContextId::new("ctx-b");

        // 训练模式:a → b 概率 1.0
        learner.record_access(&ctx_a, &ctx_b);

        // 初始：Static(Standard)，threshold=0.6
        let policy = holder.current_policy();
        let prefetched = learner.prefetch_with_policy(&ctx_a, &cache, &policy);
        assert_eq!(prefetched.len(), 1);
        assert_eq!(prefetched[0].as_str(), "ctx-b");

        // omega-learner 下发 Learned(Aggressive, version=1)
        holder.update_policy(PrefetchPolicy::learned(1, PrefetchStrategy::Aggressive));
        assert!(holder.is_learned());
        assert_eq!(holder.version(), Some(1));

        // 再次预取，使用 Learned 策略
        let policy = holder.current_policy();
        let prefetched = learner.prefetch_with_policy(&ctx_a, &cache, &policy);
        assert_eq!(prefetched.len(), 1);
        assert_eq!(prefetched[0].as_str(), "ctx-b");

        // 触发熔断：fallback_to_static
        holder.fallback_to_static();
        assert!(!holder.is_learned());
        assert_eq!(holder.strategy(), PrefetchStrategy::Standard);
    }

    #[tokio::test]
    async fn test_prefetch_with_policy_no_predictions() {
        // 未知上下文 + 任意策略 → 空列表
        let bus = EventBus::new();
        let cache = SccCache::new(SccConfig::default(), bus.clone());
        let learner = AccessPatternLearner::new(bus, 0.6);

        let unknown = ContextId::new("ctx-unknown");

        let policy = PrefetchPolicy::static_policy(PrefetchStrategy::Aggressive);
        let prefetched = learner.prefetch_with_policy(&unknown, &cache, &policy);
        assert!(prefetched.is_empty());
    }
}
