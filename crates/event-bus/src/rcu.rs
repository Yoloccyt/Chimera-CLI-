//! RCU 单调读状态容器 — 内环最终一致性基础设施（P2-W7.2.3, §9.1）
//!
//! 对应架构层:L1 Core(event-bus 因果一致性三层之二)
//! 状态(ADR-181):EXPERIMENTAL-UNWIRED —— 设计意图已实现、当前零生产消费方;
//!             勿按"已接线"引用,退役/接入条件见 ADR-181 决策 2。
//! 对应设计源:`NEXUS-OMEGA_v5.0_系统性完整设计文档.md` §9.1 RCU(arc-swap)
//!             spec.md L255 "内环内部最终一致 + 单调读（arc-swap RCU）"
//!
//! # 核心职责
//! 为内环 9 crate 共享状态提供 RCU(Read-Copy-Update)语义:
//! - **无锁读**:读者通过 `load()` 获取 `Arc<T>` 快照,无竞争、无等待
//! - **原子写**:写者构建新值后 `store()` 原子替换,读者不会看到撕裂状态
//! - **最终一致**:写入后新 `load()` 返回新值;已在读的旧快照不被回收
//! - **单调读**:单次 `load()` 返回的快照在整个 `Arc<T>` 生命周期内有效,不会被更旧版本覆盖
//!
//! # 性能(§9.1)
//! - 读(load):~5ns(原子加载指针 + Arc 引用计数)
//! - 写(store):~50ns(原子交换指针 + 旧值引用计数递减)
//! - 对比 Arc<RwLock<T>>:读路径 ~5ns vs RwLock read ~15-25ns(无锁 vs 轻量锁)
//!
//! # 因果一致性三层(spec.md L251-255)
//! 1. 跨膜事件因果一致:向量时钟 + 因果缓冲区(本 crate `causal` 模块)
//! 2. **内环内部最终一致 + 单调读**:arc-swap RCU(本模块)
//! 3. 外环持久状态强一致:Checkpoint/Quest 走 WAL + MessagePack(P2-W7.2.4)
//!
//! # 设计原则
//! - **零 BREAKING**:`MonotonicState<T>` 是新增类型,不修改既有 EventBus 接口
//! - **纯 safe**:arc-swap 内部使用 unsafe(原子指针操作),但封装在依赖内;
//!   event-bus crate 源码仍 `#![forbid(unsafe_code)]`(§4.1:不传播到依赖)
//! - **泛型 T**:不限定具体状态类型,由调用方(内环 crate)决定存什么
//! - **Arc<T> 快照**:`load()` 返回 `Arc<T>` 而非 `Guard`,便于跨 await 持有
//!   (Guard 生命周期绑定 &self,跨 await 需显式 load_full 转 Arc)
//!
//! # 使用示例
//! ```
//! use event_bus::rcu::MonotonicState;
//! use std::sync::Arc;
//! use std::thread;
//!
//! // 内环共享状态(如 MLC 记忆快照)
//! let state = Arc::new(MonotonicState::new(42i32));
//!
//! // 多读者并发 load(无锁)
//! let r1 = state.clone();
//! let h1 = thread::spawn(move || {
//!     let snap = r1.load();
//!     assert!(*snap >= 42); // 单调:不会看到比初始更旧的值
//! });
//!
//! // 写者原子替换
//! state.store(100);
//! let snap = state.load();
//! assert_eq!(*snap, 100);
//!
//! h1.join().unwrap();
//! ```

use arc_swap::ArcSwap;
use std::sync::Arc;

/// RCU 单调读状态容器 — 无锁读 + 原子写的最终一致性原语
///
/// 基于 `arc_swap::ArcSwap<T>` 封装,提供内环共享状态的 RCU 语义:
/// - `load()` 返回 `Arc<T>` 快照(无锁,~5ns)
/// - `store()` 原子替换为新值(~50ns)
///
/// # 单调读保证
/// `load()` 返回的 `Arc<T>` 在其生命周期内始终指向同一份数据。
/// 即使其他线程调用 `store()` 替换了内部状态,已返回的 `Arc<T>` 仍指向旧值
/// (旧值在最后一个 Arc 引用释放后才回收)。这保证读者看到的状态**单调递增**:
/// 不会在看到新值后突然看到旧值。
///
/// # 最终一致性
/// `store()` 后,**新** `load()` 调用保证返回新值(arc-swap 用 SeqCst 原子交换)。
/// 已在进行中的旧 `load()` 可能返回旧值(取决于调用时序),但最终所有读者
/// 都会看到新值。这是"最终一致"而非"线性一致"——适合内环认知状态
/// (记忆/策略/能力),不需要跨节点强一致。
///
/// # 线程安全
/// `MonotonicState<T>` 满足 `Send + Sync`(当 `T: Send + Sync` 时),
/// 可安全跨线程共享(通常包在 `Arc<MonotonicState<T>>` 中)。
///
/// # 对比 Arc<RwLock<T>>
/// - **读性能**:RCU ~5ns(无锁) vs RwLock ~15-25ns(锁竞争)
/// - **写性能**:RCU ~50ns(原子交换) vs RwLock ~20-40ns(独占锁)
/// - **一致性**:RCU 最终一致 vs RwLock 线性一致
/// - **适用场景**:RCU 适合"读多写少 + 最终一致可接受"(内环状态快照);
///   RwLock 适合"需要强一致读写"(如实时预算扣减)
pub struct MonotonicState<T> {
    /// arc-swap 内部持有 `Arc<T>`,通过原子指针交换实现 RCU
    inner: ArcSwap<T>,
}

// WHY 手动 impl 而非 derive:`ArcSwap<T>` 不派生 Clone(克隆语义不明确:
// 克隆应共享同一 ArcSwap 还是独立副本?共享应包在 Arc<MonotonicState> 中)。
// 刻意不实现 Clone,强制调用方用 `Arc<MonotonicState<T>>` 共享。

impl<T> MonotonicState<T> {
    /// 创建初始状态
    ///
    /// `initial` 被 `Arc::new` 包装后存入 `ArcSwap`。
    /// 后续 `load()` 返回 `Arc<T>` 快照,`store()` 原子替换。
    ///
    /// # 示例
    /// ```
    /// use event_bus::rcu::MonotonicState;
    ///
    /// let state = MonotonicState::new("hello".to_string());
    /// assert_eq!(*state.load(), "hello");
    /// ```
    pub fn new(initial: T) -> Self {
        Self {
            inner: ArcSwap::from_pointee(initial),
        }
    }

    /// 从已有 `Arc<T>` 创建(避免克隆)
    ///
    /// 适用于调用方已持有 `Arc<T>` 且不想克隆大对象的场景。
    pub fn from_arc(initial: Arc<T>) -> Self {
        Self {
            inner: ArcSwap::new(initial),
        }
    }

    /// 加载当前状态快照(无锁,~5ns)
    ///
    /// 返回 `Arc<T>`,持有当前状态的不可变快照。该快照在整个 `Arc<T>`
    /// 生命周期内始终有效,即使其他线程调用 `store()` 替换了内部状态。
    ///
    /// # 单调性保证
    /// 多次 `load()` 调用返回的状态**单调递增**(在偏序意义上):
    /// - 若 `store(new)` 在 `load()` 之前完成(happens-before),`load()` 返回 `new`
    /// - 若 `store(new)` 与 `load()` 并发,`load()` 可能返回旧值或新值
    /// - **不会**:在看到 `new` 后,后续 `load()` 返回旧值(单调性)
    ///
    /// # 跨 await 安全
    /// 返回 `Arc<T>` 而非 `Guard`(后者生命周期绑定 `&self`),
    /// 可安全跨 `.await` 持有(§4.4 反模式 1:不持锁跨 await,但 Arc 不是锁)。
    ///
    /// # 示例
    /// ```
    /// use event_bus::rcu::MonotonicState;
    ///
    /// let state = MonotonicState::new(10i32);
    /// let snap = state.load();
    /// assert_eq!(*snap, 10);
    /// // store 后新 load 返回新值
    /// state.store(20);
    /// assert_eq!(*state.load(), 20);
    /// // 旧快照仍有效(旧值不被回收)
    /// assert_eq!(*snap, 10);
    /// ```
    pub fn load(&self) -> Arc<T> {
        // load_full() 返回 Arc<T>(而非 Guard),可独立于 &self 生命周期持有。
        // 内部:原子加载指针 + Arc 引用计数递增,~5ns。
        self.inner.load_full()
    }

    /// 原子存储新值(~50ns)
    ///
    /// 用 `new` 替换内部状态。旧值在所有 `Arc<T>` 快照引用释放后回收
    /// (RCU 回收语义)。此操作是原子的:读者不会看到"半新半旧"的撕裂状态。
    ///
    /// # 性能
    /// ~50ns(原子交换指针 + 旧 Arc 引用计数递减)。比 `Arc<Mutex<T>>` 的
    /// lock+write+unlock 快(无系统调用、无线程切换)。
    ///
    /// # 示例
    /// ```
    /// use event_bus::rcu::MonotonicState;
    ///
    /// let state = MonotonicState::new(vec![1, 2, 3]);
    /// state.store(vec![4, 5, 6]); // 原子替换
    /// assert_eq!(*state.load(), vec![4, 5, 6]);
    /// ```
    pub fn store(&self, new: T) {
        // store(Arc::new(new)):原子交换内部指针,旧 Arc 引用计数递减。
        self.inner.store(Arc::new(new));
    }

    /// 原子存储已有 `Arc<T>`(避免克隆大对象)
    ///
    /// 与 `store()` 区别:直接接收 `Arc<T>` 而非 `T`,避免在调用方
    /// 已持有 `Arc<T>` 时再 `Arc::new` 一次(虽然 Arc::new 很便宜,
    /// 但在热路径上避免不必要的分配是良好实践)。
    ///
    /// # 示例
    /// ```
    /// use event_bus::rcu::MonotonicState;
    /// use std::sync::Arc;
    ///
    /// let state = MonotonicState::new(0i32);
    /// let new_val = Arc::new(42);
    /// state.store_arc(new_val);
    /// assert_eq!(*state.load(), 42);
    /// ```
    pub fn store_arc(&self, new: Arc<T>) {
        self.inner.store(new);
    }

    /// 读-拷贝-更新:基于当前快照计算新值并替换
    ///
    /// 闭包接收当前快照(`&T`),返回新值(`T`)。内部执行:
    /// 1. `load()` 获取当前快照
    /// 2. 调用闭包计算新值
    /// 3. `store()` 替换
    ///
    /// **注意**:此方法是**非原子**的 load-compute-store(最终一致语义),
    /// 并发 `update` 可能发生 lost update(后写覆盖先写)。
    /// 适用于内环认知状态(记忆/策略快照),这些场景的最终一致可接受
    /// (本模块的设计目标即"最终一致 + 单调读",非线性一致)。
    ///
    /// 若需要原子 CAS(无 lost update),应直接使用 `arc_swap::ArcSwap::rcu`
    /// 或 `compare_and_swap`(本封装不暴露,避免过度抽象)。
    ///
    /// # 示例
    /// ```
    /// use event_bus::rcu::MonotonicState;
    ///
    /// let state = MonotonicState::new(10i32);
    /// // 基于当前值计算新值
    /// state.update(|old| old + 5);
    /// assert_eq!(*state.load(), 15);
    /// ```
    pub fn update<F>(&self, f: F)
    where
        F: FnOnce(&T) -> T,
    {
        // 非原子 load-compute-store(最终一致):
        // 并发 update 可能 lost update,但符合本模块"最终一致"设计目标。
        let old = self.load();
        let new = f(&old);
        self.store(new);
    }
}

impl<T> std::fmt::Debug for MonotonicState<T>
where
    T: std::fmt::Debug,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MonotonicState")
            .field("value", &self.inner.load())
            .finish()
    }
}

impl<T> Default for MonotonicState<T>
where
    T: Default,
{
    fn default() -> Self {
        Self::new(T::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;
    use std::thread;

    // ============================================================
    // 基础操作:new / load / store
    // ============================================================

    #[test]
    fn test_new_and_load() {
        let state = MonotonicState::new(42i32);
        let snap = state.load();
        assert_eq!(*snap, 42);
    }

    #[test]
    fn test_store_replaces_value() {
        let state = MonotonicState::new(10i32);
        assert_eq!(*state.load(), 10);

        state.store(20);
        assert_eq!(*state.load(), 20);

        state.store(30);
        assert_eq!(*state.load(), 30);
    }

    #[test]
    fn test_old_snapshot_remains_valid_after_store() {
        // RCU 核心语义:旧快照在新 store 后仍指向旧值(不被回收)
        let state = MonotonicState::new("old".to_string());
        let old_snap = state.load();
        assert_eq!(*old_snap, "old");

        state.store("new".to_string());
        assert_eq!(*state.load(), "new");
        // 旧快照仍有效,仍指向 "old"
        assert_eq!(*old_snap, "old");
    }

    #[test]
    fn test_store_arc_avoids_clone() {
        let state = MonotonicState::new(0i32);
        let new_val = Arc::new(99);
        state.store_arc(new_val);
        assert_eq!(*state.load(), 99);
    }

    #[test]
    fn test_from_arc_constructor() {
        let arc = Arc::new("hello".to_string());
        let state = MonotonicState::from_arc(arc);
        assert_eq!(*state.load(), "hello");
    }

    // ============================================================
    // 单调读保证
    // ============================================================

    #[test]
    fn test_monotonic_read_no_regression() {
        // 验证:load() 返回的快照不会因为后续 store 而"变旧"
        // (Arc<T> 不可变,store 只替换内部指针,旧 Arc 仍指向旧值)
        let state = MonotonicState::new(1u32);

        let snap1 = state.load();
        assert_eq!(*snap1, 1);

        state.store(2);
        state.store(3);
        state.store(4);

        // snap1 仍是 1(它指向初始值,不受 store 影响)
        assert_eq!(*snap1, 1);

        // 新 load 返回最新值
        let snap4 = state.load();
        assert_eq!(*snap4, 4);

        // snap4 不会被后续 store 影响
        state.store(5);
        assert_eq!(*snap4, 4);
    }

    #[test]
    fn test_multiple_snapshots_coexist() {
        // 多个快照可同时存在,各自指向不同版本
        let state = MonotonicState::new(0i32);

        let s0 = state.load();
        state.store(1);
        let s1 = state.load();
        state.store(2);
        let s2 = state.load();

        assert_eq!(*s0, 0);
        assert_eq!(*s1, 1);
        assert_eq!(*s2, 2);
    }

    // ============================================================
    // update(RCU 读-拷贝-更新)
    // ============================================================

    #[test]
    fn test_update_transforms_value() {
        let state = MonotonicState::new(10i32);
        state.update(|old| old + 5);
        assert_eq!(*state.load(), 15);

        state.update(|old| old * 2);
        assert_eq!(*state.load(), 30);
    }

    #[test]
    fn test_update_with_complex_type() {
        let state = MonotonicState::new(vec![1, 2, 3]);
        state.update(|old| {
            let mut new = old.clone();
            new.push(4);
            new
        });
        assert_eq!(*state.load(), vec![1, 2, 3, 4]);
    }

    // ============================================================
    // Default / Debug
    // ============================================================

    #[test]
    fn test_default_for_default_t() {
        let state = MonotonicState::<i32>::default();
        assert_eq!(*state.load(), 0); // i32::default() == 0
    }

    #[test]
    fn test_default_for_vec() {
        let state = MonotonicState::<Vec<u8>>::default();
        assert!(state.load().is_empty());
    }

    #[test]
    fn test_debug_format() {
        let state = MonotonicState::new(42i32);
        let debug_str = format!("{:?}", state);
        assert!(debug_str.contains("MonotonicState"));
        assert!(debug_str.contains("42"));
    }

    // ============================================================
    // 并发测试:多读者 + 写者
    // ============================================================

    #[test]
    fn test_concurrent_readers_see_consistent_snapshots() {
        // 多读者并发 load,每个快照必须是一致的(非撕裂)
        let state = Arc::new(MonotonicState::new(vec![1, 2, 3, 4, 5]));

        let mut handles = vec![];
        for _ in 0..8 {
            let s = state.clone();
            handles.push(thread::spawn(move || {
                for _ in 0..1000 {
                    let snap = s.load();
                    // 快照必须是一致的:长度要么是 5(旧)要么是 10(新),不会撕裂
                    let len = snap.len();
                    assert!(len == 5 || len == 10, "撕裂快照: len={}", len);
                    // 每个元素必须是有效的(i32 范围内)
                    for &v in snap.iter() {
                        assert!((1..=10).contains(&v), "元素越界: {}", v);
                    }
                }
            }));
        }

        // 写者并发替换
        for i in 0..100 {
            let val = if i % 2 == 0 {
                vec![1, 2, 3, 4, 5]
            } else {
                vec![6, 7, 8, 9, 10, 1, 2, 3, 4, 5]
            };
            state.store(val);
        }

        for h in handles {
            h.join().expect("读者线程 panic");
        }
    }

    #[test]
    fn test_concurrent_update_no_panic_eventual_consistency() {
        // update 是非原子 load-compute-store(最终一致语义),
        // 并发 update 会有 lost update,但不应 panic 且最终值应合理增长。
        let state = Arc::new(MonotonicState::new(0u64));

        let num_writers = 4;
        let writes_per_thread = 1000u64;
        let barrier = Arc::new(Barrier::new(num_writers));

        let mut handles = vec![];
        for _ in 0..num_writers {
            let s = state.clone();
            let b = barrier.clone();
            handles.push(thread::spawn(move || {
                b.wait(); // 同时开始
                for _ in 0..writes_per_thread {
                    // 非原子 update:可能 lost update(最终一致,非线性一致)
                    s.update(|old| old + 1);
                }
            }));
        }

        for h in handles {
            h.join().expect("写者线程 panic");
        }

        // 最终值 <= 理论上限(lost update 导致部分丢失),
        // 但应 > 0(至少部分 update 成功)
        let final_val = *state.load();
        let expected_max = num_writers as u64 * writes_per_thread;
        assert!(
            final_val > 0 && final_val <= expected_max,
            "最终值 {} 不在合理区间 (0, {}]",
            final_val,
            expected_max
        );
    }

    #[test]
    fn test_concurrent_store_and_load() {
        // 高频 store + 高频 load,验证不 panic 且每次 load 的快照内部一致。
        //
        // RCU 单调读语义:单次 load() 返回的 Arc<T> 快照在其生命周期内不变
        // (不会被后续 store 覆盖)。这不等同于"多次 load() 看到单调递增序列"
        // ——后者取决于写者 store 的时序与读者 load 的时序交错。
        let state = Arc::new(MonotonicState::new(0i32));

        let writer = {
            let s = state.clone();
            thread::spawn(move || {
                for i in 0..10000 {
                    s.store(i);
                }
            })
        };

        let reader = {
            let s = state.clone();
            thread::spawn(move || {
                for _ in 0..10000 {
                    let snap = s.load();
                    // 快照内部一致:值在 [0, 10000) 范围内(无撕裂)
                    assert!((0..10000).contains(&*snap), "撕裂快照: {}", *snap);
                }
            })
        };

        writer.join().unwrap();
        reader.join().unwrap();
        // 无 panic 即通过:store/load 并发安全
    }

    // ============================================================
    // 类型安全:不同 T 类型
    // ============================================================

    #[test]
    fn test_works_with_string() {
        let state = MonotonicState::new("hello".to_string());
        assert_eq!(*state.load(), "hello");

        state.store("world".to_string());
        assert_eq!(*state.load(), "world");
    }

    #[test]
    fn test_works_with_struct() {
        #[derive(Debug, PartialEq)]
        struct Config {
            threshold: u32,
            enabled: bool,
        }

        let state = MonotonicState::new(Config {
            threshold: 100,
            enabled: true,
        });
        assert_eq!(state.load().threshold, 100);
        assert!(state.load().enabled);

        state.store(Config {
            threshold: 200,
            enabled: false,
        });
        assert_eq!(state.load().threshold, 200);
        assert!(!state.load().enabled);
    }

    #[test]
    fn test_works_with_nested_arc() {
        // MonotonicState<Arc<Inner>>:外层 ArcSwap 持有 Arc<Arc<Inner>>
        // 实际上 MonotonicState<Arc<Inner>> 会让 load() 返回 Arc<Arc<Inner>>,
        // 这在语义上正确(Arc<Arc<T>> deref 到 Arc<T>)
        let inner = Arc::new(42i32);
        let state = MonotonicState::new(inner);
        let snap = state.load();
        assert_eq!(**snap, 42); // **snap 解引用 Arc<Arc<i32>> → i32
    }
}
