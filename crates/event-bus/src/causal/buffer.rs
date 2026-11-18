//! 因果缓冲区 — 依赖未满足的跨膜事件暂存与投递判定。
//!
//! 从 `causal` 模块按职责拆出:本文件只承载缓冲区数据结构,
//! 向量时钟与偏序比较仍留在父模块 [`super`]。

use super::VectorClock;

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
