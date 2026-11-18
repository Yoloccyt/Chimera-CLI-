//! 分片队列实现 — 构造、入片、排空与深度/累计计数读写。
//!
//! 从 `shard` 单文件按职责拆出:本文件只承载队列侧行为,
//! 车道判定、哈希与 worker 循环仍在父模块 [`super`]。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use crossbeam_queue::ArrayQueue;
use tokio::sync::Notify;
use tokio::task::JoinHandle;

use super::{fnv1a, payload_within_limit, shard_worker, ShardedEventBus, MAX_EVENT_PAYLOAD_BYTES};
use crate::credit_flow::CreditFlow;
use crate::types::NexusEvent;

impl ShardedEventBus {
    /// 创建分片总线(不启动 worker;worker 由 [`spawn_workers`](Self::spawn_workers) 启动)
    ///
    /// # 参数
    /// - `n_shards`:分片数(建议 2 的幂,如 [`DEFAULT_SHARD_COUNT`])
    /// - `shard_capacity`:每片容量(建议 [`SHARD_CAPACITY`])
    #[must_use = "构造的分片总线需被持有并调用 try_push/spawn_workers"]
    pub fn new(n_shards: usize, shard_capacity: usize) -> Self {
        // WHY n_shards == 0 回退 1:0 片时取模除零 panic,单片退化为串行
        // (灰度安全:任何参数错误都不应导致运行时 panic)
        let n = n_shards.max(1);
        let shards: Vec<ArrayQueue<NexusEvent>> = (0..n)
            .map(|_| ArrayQueue::new(shard_capacity.max(1)))
            .collect();
        let notifies: Vec<Arc<Notify>> = (0..n).map(|_| Arc::new(Notify::new())).collect();
        let depths: Vec<AtomicU64> = (0..n).map(|_| AtomicU64::new(0)).collect();
        Self {
            n_shards: n,
            shard_capacity: shard_capacity.max(1),
            shards: shards.into_boxed_slice(),
            notifies: notifies.into_boxed_slice(),
            depths: depths.into_boxed_slice(),
            sharded_total: AtomicU64::new(0),
            merged_total: AtomicU64::new(0),
            merge_closed_total: AtomicU64::new(0),
            draining: AtomicBool::new(false),
        }
    }

    /// 在 tokio runtime 上启动每片消费任务(shard_worker,手册 §10.3)
    ///
    /// worker 持有 broadcast `sender` 与信用流 `credit_flow`:
    /// - 攒批 64 条后汇入既有 broadcast(分片是「发布端并行化」,消费端仍汇入
    ///   既有 broadcast 语义,订阅者 API 零变化);
    /// - 汇入后按 ADR-125 批提交语义归还信用(信用流自动平衡:片满消耗信用,
    ///   worker 汇入归还,形成闭环)。
    ///
    /// # 调用上下文
    /// 必须持有 tokio runtime(handle.spawn)。无 runtime 时调用方应自行
    /// 捕获 [`crate::EventBusError::ShardingRequiresRuntime`](crate::error::EventBusError::ShardingRequiresRuntime)
    /// 并降级(EventBus::enable_sharding 已处理)。
    ///
    /// # 返回
    /// 每片一个 [`JoinHandle<()>`]，供调用方（`EventBus::enable_sharding`）纳入
    /// [`ShardWorkerGuard`](crate::bus) 生命周期托管。worker 仅持本总线的 `Weak`，
    /// 故这些句柄是阻止任务被回收的唯一强引用链（性能重构 D2）。
    pub fn spawn_workers(
        self: &Arc<Self>,
        sender: tokio::sync::broadcast::Sender<NexusEvent>,
        credit_flow: Arc<CreditFlow>,
        handle: &tokio::runtime::Handle,
    ) -> Vec<JoinHandle<()>> {
        // D2：worker 持 Weak<ShardedEventBus>，而非强引用。否则 worker 会阻止
        // ShardedEventBus（及其 64×256 队列）在 EventBus drop 后回收 —— 即旧设计
        // 重复 build 泄漏与 composition_root_bench OOM 的根因。
        let weak = Arc::downgrade(self);
        (0..self.n_shards)
            .map(|idx| {
                let weak = weak.clone();
                let tx = sender.clone();
                let cf = Arc::clone(&credit_flow);
                handle.spawn(async move { shard_worker(weak, idx, tx, cf).await })
            })
            .collect()
    }

    /// 请求优雅排空（性能重构 D2，供 [`EventBus::shutdown_sharding`](crate::EventBus::shutdown_sharding)）。
    ///
    /// 置 `draining` 标志并对每片 `notify_one()`（无 waiter 时存 permit）：已 await 的 worker 被唤醒，
    /// 尚未 await 的 worker 下次 `notified()` 立即返回（permit 语义避免丢失唤醒）。
    /// worker 见标志后将本片 drain-to-empty 再 break，保证已入片事件不丢。
    pub fn request_drain(&self) {
        self.draining.store(true, Ordering::Release);
        for n in self.notifies.iter() {
            n.notify_one();
        }
    }

    /// 关闭期兜底：把各片残留事件 pop 后补投 broadcast（P1-1）。
    ///
    /// WHY 必要：worker 的退出判定是「draining && is_empty()」，与发布者 push
    /// 之间仍有极小窗口（push 发生在 empty 检查之后、worker break 之前）。
    /// `EventBus::shutdown_sharding` 在 worker 已退出且在飞 Arc 收敛后调用本方法，
    /// 把这类残余补投与 worker 同口径地汇入 broadcast。
    ///
    /// 返回补投条数；调用方负责 `CreditFlow::release_many(drained)` 归还信用。
    pub(crate) fn drain_residual(
        &self,
        sender: &tokio::sync::broadcast::Sender<NexusEvent>,
    ) -> u64 {
        let mut drained = 0u64;
        for idx in 0..self.n_shards {
            while let Some(event) = self.shards[idx].pop() {
                self.depths[idx].fetch_sub(1, Ordering::Relaxed);
                // 与 worker 同口径：send 无人接收记 merge_closed_total，
                // merged_total 无条件计入（漏发率等式的分母）。
                if sender.send(event).is_err() {
                    self.merge_closed_total.fetch_add(1, Ordering::Relaxed);
                }
                self.merged_total.fetch_add(1, Ordering::Relaxed);
                drained += 1;
            }
        }
        drained
    }

    /// 尝试入片(Unordered 事件专用)
    ///
    /// - 成功:递增深度 + 唤醒 worker + 累计 `sharded_total`,返回 `Ok(())`;
    /// - 失败(片满或关闭期):返回 `Err(event)`,由调用方(EventBus)按 CBF 信用流裁决
    ///   (有信用重试、无信用 shed 回退 broadcast)。
    ///
    /// # 事件所有权
    /// 入片失败时返回事件所有权(调用方回退 broadcast 需要事件本体,
    /// 避免热路径 clone)。
    ///
    /// # 64KiB 载荷断言
    /// debug 构建下序列化检查载荷 ≤ 64KiB(超过视为实现 bug,panic 于 debug;
    /// release 零开销 —— `debug_assert!` 表达式不被求值)。
    ///
    /// # 关闭期语义(P1-1)
    /// `draining` 置位后返回 `Err(event)`：调用方(EventBus)走既有 broadcast 回退，
    /// 避免「worker 已退出但事件仍能入片」的静默丢失。
    #[must_use = "入片结果决定是否回退 broadcast,忽略将导致事件丢失"]
    // WHY allow(result_large_err):Err 返回事件所有权供调用方回退 broadcast（接口契约,避免热路径 clone）
    #[allow(clippy::result_large_err)]
    pub fn try_push(&self, event: NexusEvent) -> Result<(), NexusEvent> {
        // 关闭期早退：worker 已进入 drain-to-empty 并可能已退出，再入片会
        // 返回 Ok 却永不汇入（信用也不归还）。交回调用方回退 broadcast。
        if self.draining.load(Ordering::Acquire) {
            return Err(event);
        }
        debug_assert!(
            payload_within_limit(&event),
            "事件载荷超过 {MAX_EVENT_PAYLOAD_BYTES} 字节(64KiB 断言)"
        );
        let idx = self.shard_index(&event);
        match self.shards[idx].push(event) {
            Ok(()) => {
                self.depths[idx].fetch_add(1, Ordering::Relaxed);
                // WHY notify_one 而非 notify_waiters:单片单消费者,唤醒一个
                // 即足够;notify_one 的 permit 语义在「pop 后、await 前」窗口
                // 内推送时存储许可,下次 await 立即返回(无丢失唤醒窗口)
                self.notifies[idx].notify_one();
                self.sharded_total.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            Err(event) => Err(event),
        }
    }

    /// 分片键(Unordered 事件)
    ///
    /// 设计文档口径 `kind() as usize % 64`;crate 无 `kind()` 方法,
    /// 以 `type_name()` FNV-1a 哈希替代(同类型事件确定性同片,见模块文档)。
    pub(super) fn shard_index(&self, event: &NexusEvent) -> usize {
        (fnv1a(event.type_name().as_bytes()) as usize) % self.n_shards
    }

    /// 会话键分片(OrderSensitive 事件的保序路由键,供未来阶段 / 测试使用)
    ///
    /// 本任务阶段 OrderSensitive 事件**不分片**(走单流保序,E8-4),
    /// 本函数提供会话键哈希的确定性实现:同一会话键恒映射同一片,
    /// 为 W21-24 双跑切换(会话级保序分片)预留语义锚点。
    #[must_use = "会话键哈希用于分片路由,忽略返回值无意义"]
    pub fn session_shard(key: &str, n_shards: usize) -> usize {
        (fnv1a(key.as_bytes()) as usize) % n_shards.max(1)
    }

    /// 分片数
    #[must_use = "观测分片拓扑,忽略返回值无意义"]
    pub fn n_shards(&self) -> usize {
        self.n_shards
    }

    /// 每片容量
    #[must_use = "观测分片容量,忽略返回值无意义"]
    pub fn shard_capacity(&self) -> usize {
        self.shard_capacity
    }

    /// 指定片的当前深度(指标 `bus_shard_depth{shard}`,Relaxed 观测)
    #[must_use = "观测分片深度,忽略返回值无意义"]
    pub fn shard_depth(&self, shard: usize) -> u64 {
        self.depths
            .get(shard)
            .map(|d| d.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// 全部分片深度快照(运维拉取 bus_shard_depth 序列)
    #[must_use = "观测分片深度快照,忽略返回值无意义"]
    pub fn shard_depths(&self) -> Vec<u64> {
        self.depths
            .iter()
            .map(|d| d.load(Ordering::Relaxed))
            .collect()
    }

    /// 入分片事件总数(影子双跑前哨)
    #[must_use = "观测漏发率分子,忽略返回值无意义"]
    pub fn sharded_total(&self) -> u64 {
        self.sharded_total.load(Ordering::Relaxed)
    }

    /// worker 汇入 broadcast 事件总数(影子双跑前哨)
    #[must_use = "观测漏发率分母,忽略返回值无意义"]
    pub fn merged_total(&self) -> u64 {
        self.merged_total.load(Ordering::Relaxed)
    }

    /// worker 汇入时无订阅者丢弃累计(第三轮 T1.1;非送达错误的观测面)
    #[must_use = "观测汇入即丢弃数,忽略返回值无意义"]
    pub fn merge_closed_total(&self) -> u64 {
        self.merge_closed_total.load(Ordering::Relaxed)
    }
}
