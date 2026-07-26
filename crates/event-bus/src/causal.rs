//! 因果一致性 — 向量时钟(P2-W7.2.1,ADR-033 后续膜深化)
//!
//! 对应架构层:L1 Core(event-bus 膜深化)
//! 对应设计源:spec.md L251-254 "因果一致性" Scenario
//!
//! # 核心职责
//! 跨膜事件投递的因果一致性跟踪:每个节点维护 `BTreeMap<NodeId, u64>`
//! 计数器,通过偏序比较(happens_before / concurrent)判定事件因果关系。
//!
//! # 算法(Lamport 向量时钟)
//! - 本地事件:`clock[self] += 1`
//! - 接收事件:`clock[self] += 1`,对每个 i:`clock[i] = max(clock[i], msg_clock[i])`
//!
//! # 偏序关系
//! - `a → b`(a 先于 b):`a[i] <= b[i]` 对所有 i 成立,且至少一个 i `a[i] < b[i]`
//! - `concurrent(a, b)`:既非 a→b 也非 b→a 也非相等
//!
//! # 设计原则
//! - **BTreeMap 而非 HashMap**:确定性迭代顺序,序列化字节顺序稳定
//!   (便于哈希/签名/Merkle 审计链),节点数通常 <20 时 O(log n) 查找不显著
//! - **String 而非 u64 NodeId**:节点数动态变化,直接复用
//!   `EventMetadata.source` 字段(如 "quest-engine")避免新标识符空间
//! - **零 BREAKING**:VectorClock 独立模块,不修改既有 NexusEvent 类型;
//!   调用方(膜或外环 crate)选择是否使用向量时钟
//!
//! # 因果一致性三层(spec.md L252-254)
//! 1. 跨膜事件因果一致:向量时钟 + 因果缓冲区(本模块 + causal_buffer)
//! 2. 内环内部最终一致 + 单调读:arc-swap RCU(P2-W7.2.3)
//! 3. 外环持久状态强一致:Checkpoint/Quest 走 WAL + MessagePack(P2-W7.2.4)
//!
//! # 使用示例
//! ```
//! use event_bus::causal::{CausalRelation, VectorClock};
//!
//! // 节点 A 发送事件给节点 B:clock_a → clock_b
//! let mut clock_a = VectorClock::new();
//! clock_a.increment("node-a"); // A 的第一个事件
//! let mut clock_b = clock_a.clone();
//! clock_b.increment("node-b"); // B 接收 A 的事件后递增
//!
//! // clock_a → clock_b(A 先于 B)
//! assert_eq!(clock_a.compare(&clock_b), CausalRelation::Before);
//!
//! // 并发事件:节点 C 独立递增(未接收 A 的事件)
//! let mut clock_c = VectorClock::new();
//! clock_c.increment("node-c");
//! assert_eq!(clock_a.compare(&clock_c), CausalRelation::Concurrent);
//! ```

use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::fmt;

/// 节点 ID 类型 — 内环 9 crate 或外环 25 crate 的逻辑标识
///
/// 复用 `EventMetadata.source` 字段(如 "quest-engine" / "nexus-core")
/// 作为 NodeId,避免引入新标识符空间。
pub type NodeId = String;

/// 因果关系类型 — 两个 VectorClock 的偏序比较结果
///
/// 向量时钟是偏序关系,任意两个时钟的关系必为以下四种之一:
/// - `Before`:self 先于 other 发生(self → other)
/// - `After`:other 先于 self 发生(other → self)
/// - `Concurrent`:并发(无因果关系)
/// - `Equal`:逻辑时刻相同(所有计数器相等)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CausalRelation {
    /// self → other(self 先于 other 发生)
    Before,
    /// other → self(other 先于 self 发生)
    After,
    /// 并发(无因果关系,既非 self→other 也非 other→self)
    Concurrent,
    /// 相等(逻辑时刻相同,所有计数器相等)
    Equal,
}

impl CausalRelation {
    /// 是否为偏序关系(self 先于 other,严格偏序)
    pub fn is_before(self) -> bool {
        matches!(self, CausalRelation::Before)
    }

    /// 是否为偏序关系(other 先于 self)
    pub fn is_after(self) -> bool {
        matches!(self, CausalRelation::After)
    }

    /// 是否并发(无因果关系)
    pub fn is_concurrent(self) -> bool {
        matches!(self, CausalRelation::Concurrent)
    }

    /// 是否相等
    pub fn is_equal(self) -> bool {
        matches!(self, CausalRelation::Equal)
    }
}

impl fmt::Display for CausalRelation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CausalRelation::Before => write!(f, "before"),
            CausalRelation::After => write!(f, "after"),
            CausalRelation::Concurrent => write!(f, "concurrent"),
            CausalRelation::Equal => write!(f, "equal"),
        }
    }
}

/// 向量时钟 — 跨膜事件因果一致性跟踪
///
/// # 算法
/// 每个节点维护一个 `BTreeMap<NodeId, u64>` 计数器:
/// - 本地事件:`clock[self] += 1`
/// - 接收事件:`clock[self] += 1`,对每个 i:`clock[i] = max(clock[i], msg_clock[i])`
///
/// # 偏序比较
/// - `a → b`(a 先于 b):`a[i] <= b[i]` 对所有 i 成立,且至少一个 i `a[i] < b[i]`
/// - `concurrent(a, b)`:既非 a→b 也非 b→a 也非相等
///
/// # 序列化
/// 派生 `Serialize`/`Deserialize`,可附加到事件载荷或独立序列化传输。
/// BTreeMap 保证序列化字节顺序稳定(便于哈希/签名/审计)。
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct VectorClock {
    /// 节点计数器映射:BTreeMap 保证确定性迭代顺序
    ///
    /// WHY BTreeMap 而非 HashMap:
    /// - 偏序比较需遍历两个时钟的所有节点,BTreeMap 提供确定迭代顺序
    /// - 序列化字节顺序稳定(便于哈希/签名/Merkle 审计)
    /// - 节点数通常 <20(35 crate + 内环 9 crate),BTreeMap 的 O(log n) 查找不显著
    counters: BTreeMap<NodeId, u64>,
}

impl VectorClock {
    /// 创建空时钟(无任何节点条目)
    ///
    /// 等价于 `VectorClock::default()`。空时钟 `is_empty()` 返回 true,
    /// `has_events()` 返回 false。
    pub fn new() -> Self {
        Self::default()
    }

    /// 从已知节点列表初始化(所有计数器=0)
    ///
    /// 用于预先声明节点集合(如启动时注册所有 crate 名称),
    /// 避免运行时动态插入的开销。`is_empty()` 返回 false(有节点条目),
    /// 但 `has_events()` 返回 false(无事件发生)。
    pub fn with_nodes(nodes: &[&str]) -> Self {
        let mut counters = BTreeMap::new();
        for n in nodes {
            counters.insert((*n).to_string(), 0);
        }
        Self { counters }
    }

    /// 本地事件:递增指定节点的计数器
    ///
    /// 若节点不在时钟中,先插入 0 再递增为 1。
    /// 返回递增后的值(便于链式调用与调试)。
    ///
    /// # 示例
    /// ```
    /// use event_bus::causal::VectorClock;
    ///
    /// let mut vc = VectorClock::new();
    /// assert_eq!(vc.increment("node-a"), 1); // 首次事件
    /// assert_eq!(vc.increment("node-a"), 2); // 第二次事件
    /// ```
    pub fn increment(&mut self, node: &str) -> u64 {
        let counter = self.counters.entry(node.to_string()).or_insert(0);
        *counter += 1;
        *counter
    }

    /// 合并另一时钟(取 max):接收消息时调用
    ///
    /// 对每个节点:`self[i] = max(self[i], other[i])`
    /// 不修改 self 的本地计数器(本地计数器通过 `increment` 递增)。
    ///
    /// 注意:此方法仅合并时钟,不递增自身计数器。
    /// 完整的 Lamport 接收规则请用 [`receive`](Self::receive)。
    pub fn merge(&mut self, other: &VectorClock) {
        for (node, &other_count) in &other.counters {
            let self_count = self.counters.entry(node.clone()).or_insert(0);
            if other_count > *self_count {
                *self_count = other_count;
            }
        }
    }

    /// 本地接收事件:merge + increment(标准 Lamport 接收规则)
    ///
    /// 接收消息 m:① merge(m.clock);② increment(self.node)
    /// 这是因果跟踪的标准接收规则,保证因果依赖被记录。
    ///
    /// # 示例
    /// ```
    /// use event_bus::causal::VectorClock;
    ///
    /// let mut clock_a = VectorClock::new();
    /// clock_a.increment("node-a");
    ///
    /// let mut clock_b = VectorClock::new();
    /// clock_b.receive(&clock_a, "node-b"); // B 接收 A 的事件
    ///
    /// // clock_a → clock_b(A 先于 B)
    /// assert!(clock_a.happens_before(&clock_b));
    /// ```
    pub fn receive(&mut self, other: &VectorClock, self_node: &str) {
        self.merge(other);
        self.increment(self_node);
    }

    /// 获取指定节点的计数器值(缺失返回 0)
    pub fn get(&self, node: &str) -> u64 {
        self.counters.get(node).copied().unwrap_or(0)
    }

    /// 是否无任何节点条目(尚未注册任何节点)
    ///
    /// 与 [`has_events`](Self::has_events) 区别:
    /// - `is_empty()`:`counters` 为空(无节点注册)
    /// - `has_events()`:至少一个节点计数器 > 0(有事件发生)
    pub fn is_empty(&self) -> bool {
        self.counters.is_empty()
    }

    /// 是否有事件发生(任何节点计数器 > 0)
    pub fn has_events(&self) -> bool {
        self.counters.values().any(|&v| v > 0)
    }

    /// 节点数量(注册的节点条目数,含计数器为 0 的节点)
    pub fn node_count(&self) -> usize {
        self.counters.len()
    }

    /// 总事件数(所有节点计数器之和)
    ///
    /// 用于统计与监控,不用于因果关系判定。
    pub fn total_events(&self) -> u64 {
        self.counters.values().sum()
    }

    /// 判断 self 是否先于 other 发生(self → other)
    ///
    /// 规则:`self[i] <= other[i]` 对所有 i 成立,且至少一个 i `self[i] < other[i]`
    ///
    /// 注意:缺失节点视为 0,因此 `VectorClock::new().happens_before(&any)`
    /// 仅在 any 有非零计数器时为 true。
    ///
    /// # 示例
    /// ```
    /// use event_bus::causal::VectorClock;
    ///
    /// let mut a = VectorClock::new();
    /// a.increment("n1");
    /// let mut b = a.clone();
    /// b.increment("n2");
    /// assert!(a.happens_before(&b)); // a → b
    /// assert!(!b.happens_before(&a)); // b 不先于 a
    /// ```
    pub fn happens_before(&self, other: &VectorClock) -> bool {
        // 1. self[i] <= other[i] for all i (including implicit 0 for missing keys)
        let self_le_other = self.counters.iter().all(|(node, &self_count)| {
            other.counters.get(node).copied().unwrap_or(0) >= self_count
        });
        if !self_le_other {
            return false;
        }
        // 2. self[i] < other[i] for some i (including nodes only in other, where self is implicitly 0)
        //    若 other 中某节点计数器 > 0 而 self 中该节点缺失或为 0,则 self < other
        let self_lt_other =
            self.counters.iter().any(|(node, &self_count)| {
                other.counters.get(node).copied().unwrap_or(0) > self_count
            }) || other.counters.iter().any(|(node, &other_count)| {
                other_count > 0 && self.counters.get(node).copied().unwrap_or(0) < other_count
            });
        self_lt_other
    }

    /// 判断 self 与 other 是否并发(无因果关系)
    ///
    /// 规则:既非 self → other 也非 other → self 也非相等
    ///
    /// # 示例
    /// ```
    /// use event_bus::causal::VectorClock;
    ///
    /// let mut a = VectorClock::new();
    /// a.increment("n1");
    /// let mut b = VectorClock::new();
    /// b.increment("n2");
    /// assert!(a.concurrent_with(&b)); // 独立事件,并发
    /// ```
    pub fn concurrent_with(&self, other: &VectorClock) -> bool {
        !self.happens_before(other) && !other.happens_before(self) && self != other
    }

    /// 比较两个时钟的因果关系
    ///
    /// 返回 [`CausalRelation`] 枚举,便于 match 处理。
    ///
    /// # 示例
    /// ```
    /// use event_bus::causal::{CausalRelation, VectorClock};
    ///
    /// let mut a = VectorClock::new();
    /// a.increment("n1");
    /// let mut b = a.clone();
    /// b.increment("n2");
    /// assert_eq!(a.compare(&b), CausalRelation::Before);
    /// assert_eq!(b.compare(&a), CausalRelation::After);
    /// assert_eq!(a.compare(&a.clone()), CausalRelation::Equal);
    /// ```
    pub fn compare(&self, other: &VectorClock) -> CausalRelation {
        if self == other {
            return CausalRelation::Equal;
        }
        if self.happens_before(other) {
            return CausalRelation::Before;
        }
        if other.happens_before(self) {
            return CausalRelation::After;
        }
        CausalRelation::Concurrent
    }

    /// 返回节点计数器迭代器(确定性顺序)
    pub fn iter(&self) -> impl Iterator<Item = (&NodeId, &u64)> {
        self.counters.iter()
    }

    /// 返回所有节点 ID(确定性顺序)
    pub fn nodes(&self) -> impl Iterator<Item = &NodeId> {
        self.counters.keys()
    }
}

impl PartialOrd for VectorClock {
    /// 偏序比较:仅当 `self → other` 时返回 `Less`
    ///
    /// 注意:向量时钟是偏序,两个并发时钟无法用 PartialOrd 完全表达。
    /// 此实现仅支持 `Equal` / `Less` / `Greater`,并发时钟返回 `None`
    /// (与 std 约定一致,如 `f64::partial_cmp(&NaN)`)。
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        match self.compare(other) {
            CausalRelation::Equal => Some(Ordering::Equal),
            CausalRelation::Before => Some(Ordering::Less),
            CausalRelation::After => Some(Ordering::Greater),
            CausalRelation::Concurrent => None,
        }
    }
}

impl fmt::Display for VectorClock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "VC{{")?;
        let mut first = true;
        for (node, count) in &self.counters {
            if !first {
                write!(f, ", ")?;
            }
            write!(f, "{node}:{count}")?;
            first = false;
        }
        write!(f, "}}")
    }
}

// ============================================================
// P2-W7.2.2: CausalBuffer 因果缓冲区
// ============================================================

/// 因果缓冲区默认容量 — 防止 OOM
///
/// WHY 1024:与 EventBus broadcast 默认容量([`DEFAULT_CAPACITY`](crate::DEFAULT_CAPACITY))
/// 一致,平衡内存占用与突发因果依赖未满足的场景。每个 `PendingEvent<T>` 含
/// T + VectorClock,1024 容量约 0.5-1MB,可吸收短时突发。
pub const DEFAULT_CAUSAL_BUFFER_CAPACITY: usize = 1024;

/// 因果缓冲区错误类型
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CausalBufferError {
    /// 缓冲区已满(达到容量上限),事件被丢弃
    #[error("因果缓冲区已满(容量 {capacity}),事件被丢弃")]
    BufferFull {
        /// 缓冲区容量
        capacity: usize,
    },
}

/// 因果缓冲区条目 — 暂存的事件 + 其向量时钟
///
/// 每个待处理事件携带一个 [`VectorClock`],代表事件发送方在发送时的时钟快照。
/// 缓冲区根据本地时钟判定事件依赖是否满足。
#[derive(Debug, Clone)]
pub struct PendingEvent<T> {
    /// 事件负载(泛型,允许任意事件类型)
    pub event: T,
    /// 事件携带的向量时钟(发送方快照)
    pub clock: VectorClock,
}

impl<T> PendingEvent<T> {
    /// 创建待处理事件条目
    pub fn new(event: T, clock: VectorClock) -> Self {
        Self { event, clock }
    }

    /// 解构为 (event, clock) 元组
    pub fn into_parts(self) -> (T, VectorClock) {
        (self.event, self.clock)
    }
}

/// 因果缓冲区 — 暂存依赖未满足的事件,依赖满足后投递
///
/// # 设计原则(spec.md L252-253 "因果一致性")
/// - **依赖未满足事件暂存**:跨膜事件依赖未满足时进入缓冲区,不乱序投递
/// - **有界容量**:防止 OOM(§6.1 红线"1M Token 暴力加载")
/// - **纯数据结构**:不集成事件投递通道,由调用方决定如何处理投递结果
/// - **零 BREAKING**:独立类型,不修改既有 EventBus 接口
///
/// # 投递算法
/// 遍历整个缓冲区,投递所有"依赖已满足"的事件(不保留 FIFO 顺序,
/// 因为因果一致性允许并行事件乱序投递,只要依赖满足)。
///
/// # 使用示例
/// ```
/// use event_bus::causal::{CausalBuffer, VectorClock};
///
/// let mut buf: CausalBuffer<String> = CausalBuffer::new();
///
/// // 节点 A 发事件 e1(clock_a),节点 B 当前本地时钟不含 A 的依赖
/// let mut clock_a = VectorClock::new();
/// clock_a.increment("node-a");
/// buf.push("event-e1".to_string(), clock_a.clone()).unwrap();
///
/// // 本地时钟未包含 A 的更新 → 不投递
/// let local_clock = VectorClock::new();
/// let delivered = buf.try_deliver(&local_clock);
/// assert!(delivered.is_empty());
///
/// // 本地时钟更新到满足依赖 → 投递
/// let mut updated_local = VectorClock::new();
/// updated_local.receive(&clock_a, "node-b");
/// let delivered = buf.try_deliver(&updated_local);
/// assert_eq!(delivered.len(), 1);
/// ```
#[derive(Debug, Clone)]
pub struct CausalBuffer<T> {
    /// 待处理事件队列
    ///
    /// WHY VecDeque:O(1) push_back/pop_front,支持 FIFO 入队与 O(n) 投递判定。
    pending: std::collections::VecDeque<PendingEvent<T>>,
    /// 缓冲区容量上限(防止 OOM)
    capacity: usize,
    /// 累计丢弃事件数(容量满时丢弃新事件并递增,§4.4 红线"无锁"语义:
    /// CausalBuffer 由调用方持有可变借用,dropped_count 用普通 u64 即可,
    /// 无需 AtomicU64 因为不跨 await)
    dropped_count: u64,
}

impl<T> CausalBuffer<T> {
    /// 创建默认容量的因果缓冲区(1024)
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAUSAL_BUFFER_CAPACITY)
    }

    /// 创建指定容量的因果缓冲区
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            pending: std::collections::VecDeque::with_capacity(capacity),
            capacity,
            dropped_count: 0,
        }
    }

    /// 添加事件到缓冲区,记录其向量时钟
    ///
    /// 容量满时返回 `BufferFull` 错误并递增 `dropped_count`(不阻塞调用方)。
    /// 调用方应处理错误(如记日志或拒绝投递),不应重试(可能死锁)。
    ///
    /// # 示例
    /// ```
    /// use event_bus::causal::{CausalBuffer, VectorClock};
    ///
    /// let mut buf: CausalBuffer<i32> = CausalBuffer::new();
    /// let mut clock = VectorClock::new();
    /// clock.increment("n1");
    /// assert!(buf.push(42, clock).is_ok());
    /// ```
    pub fn push(&mut self, event: T, clock: VectorClock) -> Result<(), CausalBufferError> {
        if self.pending.len() >= self.capacity {
            self.dropped_count += 1;
            return Err(CausalBufferError::BufferFull {
                capacity: self.capacity,
            });
        }
        self.pending.push_back(PendingEvent { event, clock });
        Ok(())
    }

    /// 尝试投递依赖已满足的事件
    ///
    /// 遍历整个缓冲区,投递所有"依赖已满足"的事件(不保留 FIFO 顺序,
    /// 因为因果一致性允许并行事件乱序投递,只要依赖满足)。
    ///
    /// # 投递判定
    /// 事件 e 携带时钟 `e.clock`,本地时钟 `local_clock`:
    /// - 若 `e.clock <= local_clock`(偏序意义下,e 的所有依赖都已到达本地),投递
    /// - 否则保留在缓冲区
    ///
    /// # 返回
    /// 已投递事件的 `Vec<PendingEvent<T>>`,从缓冲区移除。
    /// 未满足依赖的事件保留在缓冲区(下次 `try_deliver` 再判定)。
    pub fn try_deliver(&mut self, local_clock: &VectorClock) -> Vec<PendingEvent<T>> {
        // 用 std::mem::take 转移所有权,避免 clone;partition 后未满足的放回
        let pending = std::mem::take(&mut self.pending);
        let (delivered, retained): (std::collections::VecDeque<_>, std::collections::VecDeque<_>) =
            pending
                .into_iter()
                .partition(|pe| Self::is_satisfied(&pe.clock, local_clock));
        self.pending = retained;
        delivered.into_iter().collect()
    }

    /// 返回依赖已满足但未投递的事件数(预检查)
    ///
    /// 不修改缓冲区,仅查询。用于决定是否调用 [`try_deliver`](Self::try_deliver)。
    pub fn ready_count(&self, local_clock: &VectorClock) -> usize {
        self.pending
            .iter()
            .filter(|pe| Self::is_satisfied(&pe.clock, local_clock))
            .count()
    }

    /// 是否为空
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// 当前缓冲区事件数
    pub fn len(&self) -> usize {
        self.pending.len()
    }

    /// 缓冲区容量上限
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 累计丢弃事件数(容量满时累计,单调递增不重置)
    pub fn dropped_count(&self) -> u64 {
        self.dropped_count
    }

    /// 清空缓冲区(放弃所有待处理事件)
    ///
    /// 用于重置或紧急清理。调用方应处理丢失事件(如记日志)。
    pub fn clear(&mut self) {
        self.pending.clear();
    }

    /// 判断事件依赖是否满足(纯函数,无副作用)
    ///
    /// 规则:`event_clock <= local_clock`(偏序意义下)
    /// - `event_clock.happens_before(local_clock)`:严格偏序,e 的所有依赖已到达
    /// - `event_clock == local_clock`:相等,e 与 local 同时刻(无依赖未满足)
    ///
    /// 注意:并发事件(event_clock 与 local_clock 无偏序关系)视为依赖未满足,
    /// 因为 event_clock 中可能有 local_clock 尚未到达的节点计数。
    fn is_satisfied(event_clock: &VectorClock, local_clock: &VectorClock) -> bool {
        event_clock.happens_before(local_clock) || event_clock == local_clock
    }
}

impl<T> Default for CausalBuffer<T> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmp_serde::{from_slice, to_vec_named};
    use serde_json;

    // ============================================================
    // 基础操作:new / with_nodes / increment / get
    // ============================================================

    #[test]
    fn test_new_is_empty() {
        let vc = VectorClock::new();
        assert!(vc.is_empty());
        assert!(!vc.has_events());
        assert_eq!(vc.node_count(), 0);
        assert_eq!(vc.total_events(), 0);
    }

    #[test]
    fn test_default_equals_new() {
        let a = VectorClock::new();
        let b = VectorClock::default();
        assert_eq!(a, b);
    }

    #[test]
    fn test_with_nodes_initializes_zero() {
        let vc = VectorClock::with_nodes(&["a", "b", "c"]);
        // 有节点条目但所有计数器为 0
        assert!(!vc.is_empty()); // counters 非空(3 个节点)
        assert!(!vc.has_events()); // 无事件发生
        assert_eq!(vc.node_count(), 3);
        assert_eq!(vc.total_events(), 0);
        assert_eq!(vc.get("a"), 0);
        assert_eq!(vc.get("b"), 0);
        assert_eq!(vc.get("c"), 0);
        assert_eq!(vc.get("nonexistent"), 0);
    }

    #[test]
    fn test_with_nodes_empty_slice() {
        let vc = VectorClock::with_nodes(&[]);
        assert!(vc.is_empty());
        assert_eq!(vc.node_count(), 0);
    }

    #[test]
    fn test_increment_returns_new_value() {
        let mut vc = VectorClock::new();
        assert_eq!(vc.increment("node-a"), 1);
        assert_eq!(vc.increment("node-a"), 2);
        assert_eq!(vc.increment("node-a"), 3);
    }

    #[test]
    fn test_increment_creates_node_if_missing() {
        let mut vc = VectorClock::new();
        vc.increment("new-node");
        assert!(!vc.is_empty());
        assert!(vc.has_events());
        assert_eq!(vc.node_count(), 1);
        assert_eq!(vc.get("new-node"), 1);
    }

    #[test]
    fn test_increment_multiple_nodes_independent() {
        let mut vc = VectorClock::new();
        vc.increment("a");
        vc.increment("b");
        vc.increment("a");
        assert_eq!(vc.get("a"), 2);
        assert_eq!(vc.get("b"), 1);
        assert_eq!(vc.node_count(), 2);
        assert_eq!(vc.total_events(), 3);
    }

    #[test]
    fn test_get_returns_zero_for_missing_node() {
        let vc = VectorClock::new();
        assert_eq!(vc.get("nonexistent"), 0);
    }

    #[test]
    fn test_is_empty_vs_has_events_semantics() {
        // 区别:is_empty()=无节点条目;has_events()=至少一个节点计数器>0
        let mut vc = VectorClock::with_nodes(&["a"]); // 注册节点但无事件
        assert!(!vc.is_empty()); // 有节点条目
        assert!(!vc.has_events()); // 无事件
        vc.increment("a");
        assert!(!vc.is_empty()); // 仍有节点条目
        assert!(vc.has_events()); // 现在有事件
    }

    // ============================================================
    // Merge 操作
    // ============================================================

    #[test]
    fn test_merge_takes_max_per_node() {
        let mut a = VectorClock::new();
        a.increment("n1");
        a.increment("n1"); // n1=2
        a.increment("n2"); // n2=1

        let mut b = VectorClock::new();
        b.increment("n1"); // n1=1
        b.increment("n2");
        b.increment("n2");
        b.increment("n2"); // n2=3
        b.increment("n3"); // n3=1

        a.merge(&b);
        // n1: max(2, 1) = 2
        // n2: max(1, 3) = 3
        // n3: max(0, 1) = 1(新增节点)
        assert_eq!(a.get("n1"), 2);
        assert_eq!(a.get("n2"), 3);
        assert_eq!(a.get("n3"), 1);
        assert_eq!(a.node_count(), 3);
        assert_eq!(a.total_events(), 6);
    }

    #[test]
    fn test_merge_idempotent() {
        let mut a = VectorClock::new();
        a.increment("n1");
        a.increment("n2");

        let original = a.clone();
        a.merge(&original.clone()); // 合并自身
        assert_eq!(a, original); // 幂等:合并自身不改变时钟
    }

    #[test]
    fn test_merge_with_empty_no_change() {
        let mut a = VectorClock::new();
        a.increment("n1");
        let original = a.clone();

        a.merge(&VectorClock::new()); // 合并空时钟
        assert_eq!(a, original); // 无变化
    }

    #[test]
    fn test_merge_adds_new_nodes() {
        let mut a = VectorClock::new();
        a.increment("n1");

        let mut b = VectorClock::new();
        b.increment("n2");
        b.increment("n3");

        a.merge(&b);
        // a 应包含 n1(自身)+ n2, n3(从 b 合并)
        assert_eq!(a.get("n1"), 1);
        assert_eq!(a.get("n2"), 1);
        assert_eq!(a.get("n3"), 1);
        assert_eq!(a.node_count(), 3);
    }

    // ============================================================
    // Receive(Lamport 接收规则)
    // ============================================================

    #[test]
    fn test_receive_merges_and_increments() {
        let mut clock_a = VectorClock::new();
        clock_a.increment("node-a");
        clock_a.increment("node-a"); // a: {a:2}

        let mut clock_b = VectorClock::new();
        clock_b.increment("node-b"); // b 初始: {b:1}

        clock_b.receive(&clock_a, "node-b");
        // ① merge: b = {a:2, b:1}
        // ② increment(b): b = {a:2, b:2}
        assert_eq!(clock_b.get("node-a"), 2);
        assert_eq!(clock_b.get("node-b"), 2);
        assert!(clock_a.happens_before(&clock_b)); // a → b
    }

    // ============================================================
    // 偏序比较:happens_before
    // ============================================================

    #[test]
    fn test_happens_before_simple() {
        let mut a = VectorClock::new();
        a.increment("n1");

        let mut b = a.clone();
        b.increment("n2");

        assert!(a.happens_before(&b)); // a → b
        assert!(!b.happens_before(&a)); // b 不先于 a
    }

    #[test]
    fn test_happens_before_transitive() {
        // 传递性:a → b, b → c → a → c
        let mut a = VectorClock::new();
        a.increment("n1"); // a: {n1:1}

        let mut b = a.clone();
        b.increment("n2"); // b: {n1:1, n2:1}

        let mut c = b.clone();
        c.increment("n3"); // c: {n1:1, n2:1, n3:1}

        assert!(a.happens_before(&b));
        assert!(b.happens_before(&c));
        assert!(a.happens_before(&c)); // 传递性
    }

    #[test]
    fn test_happens_before_not_reflexive() {
        let mut a = VectorClock::new();
        a.increment("n1");
        // 自反性:任何时钟都不先于自身(除非完全无事件)
        assert!(!a.happens_before(&a.clone()));
    }

    #[test]
    fn test_happens_before_empty_vs_nonempty() {
        let empty = VectorClock::new();
        let mut nonempty = VectorClock::new();
        nonempty.increment("n1");

        // 空时钟先于任何有事件的时钟
        assert!(empty.happens_before(&nonempty));
        // 有事件的时钟不先于空时钟
        assert!(!nonempty.happens_before(&empty));
    }

    #[test]
    fn test_happens_before_two_empty_returns_false() {
        // 两个空时钟:无"严格小于"关系
        let a = VectorClock::new();
        let b = VectorClock::new();
        assert!(!a.happens_before(&b));
    }

    #[test]
    fn test_happens_before_with_zero_counters() {
        // with_nodes 创建的 0 计数器时钟与空时钟等价(均无事件)
        let empty = VectorClock::new();
        let zero = VectorClock::with_nodes(&["a", "b"]);
        // 两者均无事件,但 zero 有节点条目;happens_before 应返回 false(无严格小于)
        assert!(!empty.happens_before(&zero));
        assert!(!zero.happens_before(&empty));
    }

    // ============================================================
    // 偏序比较:concurrent_with
    // ============================================================

    #[test]
    fn test_concurrent_with_independent_events() {
        let mut a = VectorClock::new();
        a.increment("n1");

        let mut b = VectorClock::new();
        b.increment("n2");

        // n1 和 n2 独立递增,无因果关系
        assert!(a.concurrent_with(&b));
        assert!(b.concurrent_with(&a)); // 对称性
    }

    #[test]
    fn test_concurrent_with_not_reflexive() {
        let mut a = VectorClock::new();
        a.increment("n1");
        // 任何时钟与自身不并发
        assert!(!a.concurrent_with(&a.clone()));
    }

    #[test]
    fn test_concurrent_with_two_empty_returns_false() {
        // 两个空时钟相等,不并发
        let a = VectorClock::new();
        let b = VectorClock::new();
        assert!(!a.concurrent_with(&b));
    }

    #[test]
    fn test_concurrent_with_partial_overlap() {
        // 部分重叠:a={n1:1, n2:1}, b={n1:1, n3:1}
        // n1 相同,n2 vs n3 互斥 → 并发
        let mut a = VectorClock::new();
        a.increment("n1");
        a.increment("n2");

        let mut b = VectorClock::new();
        b.increment("n1");
        b.increment("n3");

        assert!(a.concurrent_with(&b));
    }

    // ============================================================
    // compare(CausalRelation 枚举)
    // ============================================================

    #[test]
    fn test_compare_equal_returns_equal() {
        let mut a = VectorClock::new();
        a.increment("n1");
        assert_eq!(a.compare(&a.clone()), CausalRelation::Equal);
    }

    #[test]
    fn test_compare_before_returns_before() {
        let mut a = VectorClock::new();
        a.increment("n1");
        let mut b = a.clone();
        b.increment("n2");
        assert_eq!(a.compare(&b), CausalRelation::Before);
    }

    #[test]
    fn test_compare_after_returns_after() {
        let mut a = VectorClock::new();
        a.increment("n1");
        let mut b = a.clone();
        b.increment("n2");
        // b.compare(a) 应返回 After(b 在 a 之后,a → b 即 other 先于 self)
        assert_eq!(b.compare(&a), CausalRelation::After);
    }

    #[test]
    fn test_compare_concurrent_returns_concurrent() {
        let mut a = VectorClock::new();
        a.increment("n1");
        let mut b = VectorClock::new();
        b.increment("n2");
        assert_eq!(a.compare(&b), CausalRelation::Concurrent);
        assert_eq!(b.compare(&a), CausalRelation::Concurrent);
    }

    // ============================================================
    // CausalRelation 谓词
    // ============================================================

    #[test]
    fn test_causal_relation_predicates() {
        assert!(CausalRelation::Before.is_before());
        assert!(!CausalRelation::Before.is_after());
        assert!(!CausalRelation::Before.is_concurrent());
        assert!(!CausalRelation::Before.is_equal());

        assert!(CausalRelation::After.is_after());
        assert!(CausalRelation::Concurrent.is_concurrent());
        assert!(CausalRelation::Equal.is_equal());
    }

    #[test]
    fn test_causal_relation_display() {
        assert_eq!(CausalRelation::Before.to_string(), "before");
        assert_eq!(CausalRelation::After.to_string(), "after");
        assert_eq!(CausalRelation::Concurrent.to_string(), "concurrent");
        assert_eq!(CausalRelation::Equal.to_string(), "equal");
    }

    // ============================================================
    // PartialOrd trait
    // ============================================================

    #[test]
    fn test_partial_ord_equal() {
        let mut a = VectorClock::new();
        a.increment("n1");
        let b = a.clone();
        assert_eq!(a.partial_cmp(&b), Some(Ordering::Equal));
    }

    #[test]
    fn test_partial_ord_less() {
        let mut a = VectorClock::new();
        a.increment("n1");
        let mut b = a.clone();
        b.increment("n2");
        assert_eq!(a.partial_cmp(&b), Some(Ordering::Less));
    }

    #[test]
    fn test_partial_ord_greater() {
        let mut a = VectorClock::new();
        a.increment("n1");
        let mut b = a.clone();
        b.increment("n2");
        assert_eq!(b.partial_cmp(&a), Some(Ordering::Greater));
    }

    #[test]
    fn test_partial_ord_concurrent_returns_none() {
        // 并发时钟返回 None(与 f64::partial_cmp(&NaN) 一致)
        let mut a = VectorClock::new();
        a.increment("n1");
        let mut b = VectorClock::new();
        b.increment("n2");
        assert_eq!(a.partial_cmp(&b), None);
    }

    // ============================================================
    // 序列化(JSON / MessagePack)
    // ============================================================

    #[test]
    fn test_serde_json_roundtrip() {
        let mut vc = VectorClock::new();
        vc.increment("quest-engine");
        vc.increment("quest-engine");
        vc.increment("parliament");

        let json = serde_json::to_string(&vc).expect("JSON 序列化失败");
        let deserialized: VectorClock = serde_json::from_str(&json).expect("JSON 反序列化失败");
        assert_eq!(vc, deserialized);
    }

    #[test]
    fn test_serde_msgpack_roundtrip() {
        let mut vc = VectorClock::new();
        vc.increment("quest-engine");
        vc.increment("nexus-core");
        vc.increment("nexus-core");

        let bytes = to_vec_named(&vc).expect("MessagePack 序列化失败");
        let deserialized: VectorClock = from_slice(&bytes).expect("MessagePack 反序列化失败");
        assert_eq!(vc, deserialized);
    }

    #[test]
    fn test_serde_json_stable_byte_order() {
        // BTreeMap 保证序列化字节顺序稳定(便于哈希/签名/审计)
        let mut vc1 = VectorClock::new();
        vc1.increment("zeta");
        vc1.increment("alpha");
        vc1.increment("mid");

        let mut vc2 = VectorClock::new();
        vc2.increment("alpha");
        vc2.increment("mid");
        vc2.increment("zeta");

        // 不同插入顺序,但 BTreeMap 排序后字节相同
        let json1 = serde_json::to_string(&vc1).expect("序列化失败");
        let json2 = serde_json::to_string(&vc2).expect("序列化失败");
        assert_eq!(json1, json2);
    }

    // ============================================================
    // Display 格式化
    // ============================================================

    #[test]
    fn test_display_empty() {
        let vc = VectorClock::new();
        assert_eq!(vc.to_string(), "VC{}");
    }

    #[test]
    fn test_display_with_events() {
        let mut vc = VectorClock::new();
        vc.increment("alpha");
        vc.increment("beta");
        vc.increment("beta");
        // BTreeMap 按 key 排序:alpha:1, beta:2
        assert_eq!(vc.to_string(), "VC{alpha:1, beta:2}");
    }

    // ============================================================
    // 膜集成场景:跨膜事件因果跟踪
    // ============================================================

    #[test]
    fn test_membrane_scenario_causal_tracking() {
        // 场景:节点 A 发事件 e1,B 接收 e1 后发 e2,C 独立发 e3
        // 因果:e1 → e2,e3 与 e1/e2 并发
        let mut clock_e1 = VectorClock::new();
        clock_e1.increment("node-a"); // e1: {a:1}

        let mut clock_e2 = VectorClock::new();
        clock_e2.receive(&clock_e1, "node-b"); // e2: {a:1, b:1}

        let mut clock_e3 = VectorClock::new();
        clock_e3.increment("node-c"); // e3: {c:1}

        // e1 → e2(A 先于 B)
        assert_eq!(clock_e1.compare(&clock_e2), CausalRelation::Before);
        // e3 与 e1 并发,e3 与 e2 并发
        assert_eq!(clock_e1.compare(&clock_e3), CausalRelation::Concurrent);
        assert_eq!(clock_e2.compare(&clock_e3), CausalRelation::Concurrent);
    }

    #[test]
    fn test_membrane_scenario_chain_delivery() {
        // 场景:链式投递 A → B → C,每步接收并递增
        let mut clock_a = VectorClock::new();
        clock_a.increment("node-a");

        let mut clock_b = VectorClock::new();
        clock_b.receive(&clock_a, "node-b");

        let mut clock_c = VectorClock::new();
        clock_c.receive(&clock_b, "node-c");

        // 链式因果:a → b → c
        assert!(clock_a.happens_before(&clock_b));
        assert!(clock_b.happens_before(&clock_c));
        assert!(clock_a.happens_before(&clock_c)); // 传递性
    }

    #[test]
    fn test_membrane_scenario_independent_branches() {
        // 场景:从同一时钟 fork 两个分支,各自独立演化
        let mut root = VectorClock::new();
        root.increment("root");

        let mut branch_a = root.clone();
        branch_a.increment("branch-a");

        let mut branch_b = root.clone();
        branch_b.increment("branch-b");

        // root → branch_a, root → branch_b(两者都先于分支)
        assert!(root.happens_before(&branch_a));
        assert!(root.happens_before(&branch_b));
        // 两个分支互相并发(无因果关系)
        assert!(branch_a.concurrent_with(&branch_b));
    }

    // ============================================================
    // 迭代器
    // ============================================================

    #[test]
    fn test_iter_returns_sorted_nodes() {
        let mut vc = VectorClock::new();
        vc.increment("zeta");
        vc.increment("alpha");
        vc.increment("mid");

        let nodes: Vec<&String> = vc.nodes().collect();
        // BTreeMap 按 key 排序
        assert_eq!(nodes, vec!["alpha", "mid", "zeta"]);

        let pairs: Vec<(&String, &u64)> = vc.iter().collect();
        assert_eq!(pairs[0], (&"alpha".to_string(), &1));
        assert_eq!(pairs[1], (&"mid".to_string(), &1));
        assert_eq!(pairs[2], (&"zeta".to_string(), &1));
    }

    #[test]
    fn test_iter_empty_returns_nothing() {
        let vc = VectorClock::new();
        assert_eq!(vc.iter().count(), 0);
        assert_eq!(vc.nodes().count(), 0);
    }

    // ============================================================
    // CausalBuffer: 基础操作(new / with_capacity / push / len)
    // ============================================================

    #[test]
    fn test_causal_buffer_new_default_capacity() {
        let buf: CausalBuffer<i32> = CausalBuffer::new();
        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);
        assert_eq!(buf.capacity(), DEFAULT_CAUSAL_BUFFER_CAPACITY);
        assert_eq!(buf.dropped_count(), 0);
    }

    #[test]
    fn test_causal_buffer_default_equals_new() {
        let a: CausalBuffer<i32> = CausalBuffer::new();
        let b: CausalBuffer<i32> = CausalBuffer::default();
        assert_eq!(a.capacity(), b.capacity());
        assert_eq!(a.len(), b.len());
    }

    #[test]
    fn test_causal_buffer_with_capacity_custom() {
        let buf: CausalBuffer<String> = CausalBuffer::with_capacity(8);
        assert_eq!(buf.capacity(), 8);
        assert!(buf.is_empty());
    }

    #[test]
    fn test_causal_buffer_push_increments_len() {
        let mut buf: CausalBuffer<i32> = CausalBuffer::with_capacity(4);
        let mut clk = VectorClock::new();
        clk.increment("n1");

        assert!(buf.push(10, clk.clone()).is_ok());
        assert_eq!(buf.len(), 1);
        assert!(!buf.is_empty());

        assert!(buf.push(20, clk.clone()).is_ok());
        assert_eq!(buf.len(), 2);

        assert!(buf.push(30, clk.clone()).is_ok());
        assert_eq!(buf.len(), 3);

        assert!(buf.push(40, clk).is_ok());
        assert_eq!(buf.len(), 4);
    }

    #[test]
    fn test_causal_buffer_push_full_returns_buffer_full_error() {
        // 容量 = 2,推入 3 个事件,第 3 个应失败
        let mut buf: CausalBuffer<i32> = CausalBuffer::with_capacity(2);
        let mut clk = VectorClock::new();
        clk.increment("n1");

        assert!(buf.push(1, clk.clone()).is_ok());
        assert!(buf.push(2, clk.clone()).is_ok());
        // 缓冲区满,第 3 个应返回 BufferFull 错误
        let err = buf.push(3, clk).expect_err("满时应返回 BufferFull");
        assert_eq!(err, CausalBufferError::BufferFull { capacity: 2 });
        // len 不变(被拒的事件未入队)
        assert_eq!(buf.len(), 2);
    }

    #[test]
    fn test_causal_buffer_push_full_increments_dropped_count() {
        let mut buf: CausalBuffer<i32> = CausalBuffer::with_capacity(1);
        let mut clk = VectorClock::new();
        clk.increment("n1");

        assert!(buf.push(1, clk.clone()).is_ok());
        assert_eq!(buf.dropped_count(), 0);

        // 容量满,丢弃事件并递增 dropped_count
        assert!(buf.push(2, clk.clone()).is_err());
        assert_eq!(buf.dropped_count(), 1);

        // 再次满,继续递增(单调递增)
        assert!(buf.push(3, clk).is_err());
        assert_eq!(buf.dropped_count(), 2);
        // len 仍为 1(被拒事件未入队)
        assert_eq!(buf.len(), 1);
    }

    #[test]
    fn test_causal_buffer_capacity_accessor() {
        let buf: CausalBuffer<u8> = CausalBuffer::with_capacity(256);
        assert_eq!(buf.capacity(), 256);
    }

    // ============================================================
    // CausalBuffer: try_deliver 投递判定
    // ============================================================

    #[test]
    fn test_causal_buffer_try_deliver_empty_returns_empty() {
        let mut buf: CausalBuffer<i32> = CausalBuffer::new();
        let local = VectorClock::new();
        let delivered = buf.try_deliver(&local);
        assert!(delivered.is_empty());
        assert!(buf.is_empty());
    }

    #[test]
    fn test_causal_buffer_try_deliver_satisfied_all() {
        // 所有事件依赖满足 → 全部投递
        let mut buf: CausalBuffer<&'static str> = CausalBuffer::new();

        // 事件 e1 时钟:{n1:1},本地时钟已含 n1:1 → 满足
        let mut clk_e1 = VectorClock::new();
        clk_e1.increment("n1");
        buf.push("e1", clk_e1.clone()).unwrap();

        // 事件 e2 时钟:{n1:1, n2:1},本地时钟已含 → 满足
        let mut clk_e2 = VectorClock::new();
        clk_e2.increment("n1");
        clk_e2.increment("n2");
        buf.push("e2", clk_e2.clone()).unwrap();

        // 本地时钟:{n1:1, n2:1}(满足两个事件依赖)
        let mut local = VectorClock::new();
        local.increment("n1");
        local.increment("n2");

        let delivered = buf.try_deliver(&local);
        assert_eq!(delivered.len(), 2);
        assert!(buf.is_empty()); // 全部投递后缓冲区空
    }

    #[test]
    fn test_causal_buffer_try_deliver_unsatisfied_retained() {
        // 事件依赖未满足 → 保留在缓冲区
        let mut buf: CausalBuffer<&'static str> = CausalBuffer::new();

        // e1 时钟:{n1:2}(本地需到达 n1:2 才满足)
        let mut clk_e1 = VectorClock::new();
        clk_e1.increment("n1");
        clk_e1.increment("n1");
        buf.push("e1", clk_e1).unwrap();

        // 本地时钟仅 {n1:1} → 依赖未满足
        let mut local = VectorClock::new();
        local.increment("n1");

        let delivered = buf.try_deliver(&local);
        assert!(delivered.is_empty()); // 无投递
        assert_eq!(buf.len(), 1); // e1 仍保留在缓冲区
    }

    #[test]
    fn test_causal_buffer_try_deliver_partial_delivery() {
        // 部分事件依赖满足 → 仅投递满足的,未满足的保留
        let mut buf: CausalBuffer<i32> = CausalBuffer::new();

        // e_satisfied:{n1:1} → 本地 {n1:1} 满足
        let mut clk_sat = VectorClock::new();
        clk_sat.increment("n1");
        buf.push(100, clk_sat).unwrap();

        // e_unsatisfied:{n2:1} → 本地不含 n2 → 未满足
        let mut clk_unsat = VectorClock::new();
        clk_unsat.increment("n2");
        buf.push(200, clk_unsat).unwrap();

        // 本地时钟:{n1:1}(不含 n2)
        let mut local = VectorClock::new();
        local.increment("n1");

        let delivered = buf.try_deliver(&local);
        assert_eq!(delivered.len(), 1); // 仅投递 1 个(satisfied)
        assert_eq!(delivered[0].event, 100);
        assert_eq!(buf.len(), 1); // unsat 仍保留
    }

    #[test]
    fn test_causal_buffer_try_deliver_equal_clock_satisfied() {
        // 事件时钟 == 本地时钟 → 视为依赖满足(is_satisfied 包含 ==)
        let mut buf: CausalBuffer<i32> = CausalBuffer::new();

        let mut clk = VectorClock::new();
        clk.increment("n1");
        clk.increment("n2");
        buf.push(42, clk.clone()).unwrap();

        // 本地时钟与事件时钟完全相等
        let local = clk.clone();
        let delivered = buf.try_deliver(&local);
        assert_eq!(delivered.len(), 1);
        assert!(buf.is_empty());
    }

    #[test]
    fn test_causal_buffer_try_deliver_concurrent_unsatisfied() {
        // 并发事件(无偏序关系)→ 视为依赖未满足
        // e 时钟:{n2:1},本地时钟:{n1:1},两者并发 → 未满足
        let mut buf: CausalBuffer<i32> = CausalBuffer::new();

        let mut clk_e = VectorClock::new();
        clk_e.increment("n2");
        buf.push(7, clk_e).unwrap();

        let mut local = VectorClock::new();
        local.increment("n1"); // 本地不含 n2,与事件并发

        let delivered = buf.try_deliver(&local);
        assert!(delivered.is_empty());
        assert_eq!(buf.len(), 1); // 事件保留
    }

    #[test]
    fn test_causal_buffer_try_deliver_progressive() {
        // 渐进式投递:本地时钟逐步推进,事件逐步满足
        let mut buf: CausalBuffer<i32> = CausalBuffer::new();

        // e 需要 {n1:2}
        let mut clk_e = VectorClock::new();
        clk_e.increment("n1");
        clk_e.increment("n1");
        buf.push(1, clk_e).unwrap();

        // 第 1 轮:本地 {n1:1} → 未满足
        let mut local = VectorClock::new();
        local.increment("n1");
        assert!(buf.try_deliver(&local).is_empty());
        assert_eq!(buf.len(), 1);

        // 第 2 轮:本地 {n1:2} → 满足
        local.increment("n1");
        let delivered = buf.try_deliver(&local);
        assert_eq!(delivered.len(), 1);
        assert!(buf.is_empty());
    }

    // ============================================================
    // CausalBuffer: ready_count 预检查
    // ============================================================

    #[test]
    fn test_causal_buffer_ready_count_zero_when_unsatisfied() {
        let mut buf: CausalBuffer<i32> = CausalBuffer::new();
        let mut clk = VectorClock::new();
        clk.increment("n1");
        buf.push(1, clk).unwrap();

        let local = VectorClock::new(); // 不含 n1
        assert_eq!(buf.ready_count(&local), 0);
    }

    #[test]
    fn test_causal_buffer_ready_count_nonzero_when_satisfied() {
        let mut buf: CausalBuffer<i32> = CausalBuffer::new();

        // 3 个事件,2 个满足依赖
        let mut clk_sat1 = VectorClock::new();
        clk_sat1.increment("n1");
        buf.push(1, clk_sat1).unwrap();

        let mut clk_sat2 = VectorClock::new();
        clk_sat2.increment("n1");
        clk_sat2.increment("n2");
        buf.push(2, clk_sat2).unwrap();

        let mut clk_unsat = VectorClock::new();
        clk_unsat.increment("n3"); // 本地不含 n3
        buf.push(3, clk_unsat).unwrap();

        let mut local = VectorClock::new();
        local.increment("n1");
        local.increment("n2");

        // ready_count 不修改缓冲区
        assert_eq!(buf.ready_count(&local), 2);
        assert_eq!(buf.len(), 3); // 缓冲区未变
    }

    #[test]
    fn test_causal_buffer_ready_count_empty_buffer() {
        let buf: CausalBuffer<i32> = CausalBuffer::new();
        let local = VectorClock::new();
        assert_eq!(buf.ready_count(&local), 0);
    }

    // ============================================================
    // CausalBuffer: clear / dropped_count
    // ============================================================

    #[test]
    fn test_causal_buffer_clear_empties_buffer() {
        let mut buf: CausalBuffer<i32> = CausalBuffer::with_capacity(4);
        let mut clk = VectorClock::new();
        clk.increment("n1");
        buf.push(1, clk.clone()).unwrap();
        buf.push(2, clk.clone()).unwrap();
        buf.push(3, clk).unwrap();
        assert_eq!(buf.len(), 3);

        buf.clear();
        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);
        // dropped_count 不因 clear 重置(它是累计指标)
        assert_eq!(buf.dropped_count(), 0); // 此例中无丢弃
    }

    #[test]
    fn test_causal_buffer_clear_preserves_dropped_count() {
        let mut buf: CausalBuffer<i32> = CausalBuffer::with_capacity(1);
        let mut clk = VectorClock::new();
        clk.increment("n1");
        buf.push(1, clk.clone()).unwrap();
        // 容量满,push 返回 Err(BufferFull) 并递增 dropped_count(API 显式错误,非静默丢弃)
        buf.push(2, clk.clone()).unwrap_err(); // 满,丢弃
        assert_eq!(buf.dropped_count(), 1);

        buf.clear();
        // dropped_count 是累计指标,clear 后保留(便于监控累计丢弃)
        assert_eq!(buf.dropped_count(), 1);
        assert!(buf.is_empty());
    }

    #[test]
    fn test_causal_buffer_dropped_count_monotonic() {
        let mut buf: CausalBuffer<i32> = CausalBuffer::with_capacity(1);
        let mut clk = VectorClock::new();
        clk.increment("n1");

        buf.push(1, clk.clone()).unwrap(); // OK(容量 1,首次入队)
        buf.push(2, clk.clone()).unwrap_err(); // drop 1(满,返回 Err)
        buf.push(3, clk.clone()).unwrap_err(); // drop 2
        buf.push(4, clk).unwrap_err(); // drop 3
        assert_eq!(buf.dropped_count(), 3);

        // 投递后 len 减少,但 dropped_count 不重置
        let mut local = VectorClock::new();
        local.increment("n1");
        let _ = buf.try_deliver(&local);
        assert_eq!(buf.dropped_count(), 3); // 仍为 3
    }

    // ============================================================
    // PendingEvent
    // ============================================================

    #[test]
    fn test_pending_event_new() {
        let mut clk = VectorClock::new();
        clk.increment("n1");
        let pe: PendingEvent<i32> = PendingEvent::new(42, clk.clone());
        assert_eq!(pe.event, 42);
        assert_eq!(pe.clock, clk);
    }

    #[test]
    fn test_pending_event_into_parts() {
        let mut clk = VectorClock::new();
        clk.increment("n1");
        clk.increment("n1");
        let pe: PendingEvent<String> = PendingEvent::new("hello".to_string(), clk.clone());

        let (event, clock) = pe.into_parts();
        assert_eq!(event, "hello");
        assert_eq!(clock, clk);
    }

    #[test]
    fn test_causal_buffer_default_capacity_constant() {
        // 默认容量与 EventBus broadcast 默认容量一致(防 OOM 平衡)
        assert_eq!(DEFAULT_CAUSAL_BUFFER_CAPACITY, 1024);
    }

    // ============================================================
    // CausalBuffer 跨膜投递场景
    // ============================================================

    #[test]
    fn test_causal_buffer_scenario_dependency_chain() {
        // 场景:链式依赖 A → B → C
        // B 依赖 A,C 依赖 B;本地时钟逐步推进,逐步投递
        let mut buf: CausalBuffer<&'static str> = CausalBuffer::new();

        // A 发 e_a:{a:1}
        let mut clk_a = VectorClock::new();
        clk_a.increment("a");
        buf.push("e_a", clk_a.clone()).unwrap();

        // B 发 e_b(接收 A 后):{a:1, b:1}
        let mut clk_b = VectorClock::new();
        clk_b.receive(&clk_a, "b");
        buf.push("e_b", clk_b.clone()).unwrap();

        // C 发 e_c(接收 B 后):{a:1, b:1, c:1}
        let mut clk_c = VectorClock::new();
        clk_c.receive(&clk_b, "c");
        buf.push("e_c", clk_c).unwrap();

        // 本地初始时钟:空 → 无投递
        let mut local = VectorClock::new();
        assert!(buf.try_deliver(&local).is_empty());
        assert_eq!(buf.len(), 3);

        // 本地推进到 {a:1} → 投递 e_a
        local.increment("a");
        let d1 = buf.try_deliver(&local);
        assert_eq!(d1.len(), 1);
        assert_eq!(d1[0].event, "e_a");
        assert_eq!(buf.len(), 2);

        // 本地推进到 {a:1, b:1} → 投递 e_b
        local.increment("b");
        let d2 = buf.try_deliver(&local);
        assert_eq!(d2.len(), 1);
        assert_eq!(d2[0].event, "e_b");
        assert_eq!(buf.len(), 1);

        // 本地推进到 {a:1, b:1, c:1} → 投递 e_c
        local.increment("c");
        let d3 = buf.try_deliver(&local);
        assert_eq!(d3.len(), 1);
        assert_eq!(d3[0].event, "e_c");
        assert!(buf.is_empty());
    }

    #[test]
    fn test_causal_buffer_scenario_concurrent_events_retained() {
        // 场景:并发事件(无因果关系)→ 依赖未满足,保留在缓冲区
        let mut buf: CausalBuffer<i32> = CausalBuffer::new();

        // 节点 X 发事件:{x:1}
        let mut clk_x = VectorClock::new();
        clk_x.increment("x");
        buf.push(10, clk_x).unwrap();

        // 本地时钟:{y:1}(与 x 并发,不含 x)
        let mut local = VectorClock::new();
        local.increment("y");

        // 并发事件依赖未满足 → 保留
        assert!(buf.try_deliver(&local).is_empty());
        assert_eq!(buf.len(), 1);

        // 本地接收 x 的事件后推进到 {x:1, y:1} → 满足
        let mut clk_x = VectorClock::new();
        clk_x.increment("x");
        local.merge(&clk_x);
        let delivered = buf.try_deliver(&local);
        assert_eq!(delivered.len(), 1);
        assert_eq!(delivered[0].event, 10);
    }

    #[test]
    fn test_causal_buffer_scenario_batch_delivery_order_irrelevant() {
        // 场景:因果一致性允许并行事件乱序投递(只要依赖满足)
        // 缓冲区中事件按依赖满足投递,不保证 FIFO
        let mut buf: CausalBuffer<i32> = CausalBuffer::new();

        // 三个独立事件(均仅依赖自身节点,本地时钟满足后全部投递)
        for i in 0..3 {
            let mut clk = VectorClock::new();
            clk.increment(&format!("n{i}"));
            buf.push(i, clk).unwrap();
        }

        // 本地时钟满足所有依赖(包含 n0/n1/n2)
        let mut local = VectorClock::with_nodes(&["n0", "n1", "n2"]);
        local.increment("n0");
        local.increment("n1");
        local.increment("n2");

        let delivered = buf.try_deliver(&local);
        assert_eq!(delivered.len(), 3);
        // 投递顺序不保证 FIFO,但事件集合正确
        let mut events: Vec<i32> = delivered.into_iter().map(|pe| pe.event).collect();
        events.sort();
        assert_eq!(events, vec![0, 1, 2]);
    }

    #[test]
    fn test_causal_buffer_scenario_full_then_recover() {
        // 场景:缓冲区满 → 丢弃 → 投递腾出空间 → 后续事件可入队
        let mut buf: CausalBuffer<i32> = CausalBuffer::with_capacity(2);
        let mut clk = VectorClock::new();
        clk.increment("n1");

        // 填满缓冲区
        buf.push(1, clk.clone()).unwrap();
        buf.push(2, clk.clone()).unwrap();
        // 第 3 个被丢弃
        assert!(buf.push(3, clk.clone()).is_err());
        assert_eq!(buf.dropped_count(), 1);

        // 投递腾出空间(本地时钟满足依赖)
        let mut local = VectorClock::new();
        local.increment("n1");
        let delivered = buf.try_deliver(&local);
        assert_eq!(delivered.len(), 2);
        assert!(buf.is_empty());

        // 现在可继续入队
        assert!(buf.push(4, clk).is_ok());
        assert_eq!(buf.len(), 1);
        // dropped_count 保留累计值(不重置)
        assert_eq!(buf.dropped_count(), 1);
    }
}
