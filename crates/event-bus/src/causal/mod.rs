//! 因果一致性 — 向量时钟(P2-W7.2.1,ADR-033 后续膜深化)
//!
//! 对应架构层:L1 Core(event-bus 膜深化)
//! 状态(ADR-200 D2 终裁,2026-10-07):PRODUCTION-WIRED(经 attribution
//!             tap 订阅侧时钟);对外 API 面不扩大。
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
//!    —— 该层原语 `rcu::MonotonicState` 已按 ADR-181 决策 2 退役删除
//!    (内环里程碑结束仍零消费);若内环改造需要此原语,从 git 基线
//!    `baseline-2026-09-13` 恢复
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
    pub fn iter(&self) -> VectorClockIter<'_> {
        VectorClockIter {
            inner: self.counters.iter(),
        }
    }

    /// 返回所有节点 ID(确定性顺序)
    pub fn nodes(&self) -> VectorClockNodes<'_> {
        VectorClockNodes {
            inner: self.counters.keys(),
        }
    }
}

/// [`VectorClock::iter`] 的具名迭代器(确定性顺序:`BTreeMap` 键序)。
///
/// 具名化的原因:`impl Iterator` 作为公开返回类型时,下游无法为其命名
/// (不能存进结构体字段、不能用作泛型参数),属公开 API 可用性缺陷。
#[derive(Debug)]
pub struct VectorClockIter<'a> {
    /// `BTreeMap::iter` 的原生迭代器(零拷贝借用)
    inner: std::collections::btree_map::Iter<'a, NodeId, u64>,
}

impl<'a> Iterator for VectorClockIter<'a> {
    type Item = (&'a NodeId, &'a u64);

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl ExactSizeIterator for VectorClockIter<'_> {}

/// [`VectorClock::nodes`] 的具名迭代器(确定性顺序:`BTreeMap` 键序)。
///
/// 与 [`VectorClockIter`] 同理具名化 —— 公开返回类型必须可被下游命名。
#[derive(Debug)]
pub struct VectorClockNodes<'a> {
    /// `BTreeMap::keys` 的原生迭代器(零拷贝借用)
    inner: std::collections::btree_map::Keys<'a, NodeId, u64>,
}

impl<'a> Iterator for VectorClockNodes<'a> {
    type Item = &'a NodeId;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.inner.size_hint()
    }
}

impl ExactSizeIterator for VectorClockNodes<'_> {}

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

/// 因果缓冲区 — 暂存依赖未满足的跨膜事件,依赖满足后投递
pub mod buffer;

pub use buffer::{CausalBuffer, CausalBufferError, PendingEvent, DEFAULT_CAUSAL_BUFFER_CAPACITY};

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests;
