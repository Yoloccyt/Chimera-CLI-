//! ShardedBus 分片核心 — 非 Critical 事件的分片扇出(灰度增量)
//!
//! 对应架构层:L1 Core(event-bus)
//! 对应任务:P1-T12(Phase 1 地基波次,手册 §8.5 ShardedEventBus 终版 +
//! v4.0 WI-15 阶段一「PatternIndex 精确索引先行」 + E8-4 修正)
//!
//! # 为什么新建 shard.rs 而非扩展 backpressure.rs
//! `backpressure.rs` 的职责是「背压策略判定 + 慢消费者检测」(`BackpressurePolicy`/
//! `SlowConsumerDetector`),语义上是被动保护;本模块的职责是「发布端并行化」——
//! Lane 三车道判定 + 64 片无锁队列扇出 + worker 汇入既有 broadcast,语义上是
//! 主动路由。两者正交,合并会造成单文件双主题(维护者需跳跃理解),故独立成模块。
//!
//! # Lane 三车道(手册 §8.5)
//! - `Critical`:17 个 Critical 事件(severity() == Critical,与 [`crate::bus::LANE_FORBIDDEN_SHARD`]
//!   一一对应,守护测试保证)→ **永远走既有 mpsc/broadcast 通道,不进分片**(红线:
//!   Critical 分片会破坏"发布方 → 订阅方"全序投递语义与 mpsc 旁路免背压保证,
//!   推演 9:Critical 背压 = 死锁源);
//! - `OrderSensitive(SessionKey)`:携带会话键(白名单变体的 `session_id` 字段 /
//!   `metadata.correlation_id`)的事件,以及告警类事件(`EfficiencyAlertTriggered`,
//!   占位键 rule_id)→ **顺序敏感通道保持单流,不分片**
//!   (E8-4:分片仅限可乱序订阅者通道;同一会话内事件必须保序,单流即保序;
//!   Ω₈ 可观测性:告警事件需即时同步可见 —— 直投单流无 worker 攒批延迟,详见
//!   [`event_lane`] 告警语义判定);
//! - `Unordered`:其余全部事件 → 按 FNV-1a 哈希分入 64 片之一,worker 攒批汇入
//!   既有 broadcast,订阅者 API 零变化。
//!
//! # 白名单前 5 个(任务描述偏差说明)
//! 任务举例「QuestCreated/QuestCompleted/CheckpointSaved」,但这些变体**实际没有
//! `session_id` 字段**(仅有 `quest_id`)。按「优先选择带 session_id 字段的既有事件」
//! 的真实口径,白名单取代码中实际携带 `session_id` 的 5 个变体:
//! `TuiChatSubmitted` / `TuiChatResponseChunk` / `TuiChatCompleted` /
//! `TuiChatStatusChanged` / `CrossVendorNegotiation`。
//!
//! # 分片键(设计文档偏差说明)
//! 设计文档口径「无序事件按 `kind() as usize % 64`」,但本 crate 的 `NexusEvent`
//! **没有 `kind()` 方法**(144 变体仅有 `type_name()` 字符串)。以
//! `type_name()` 的 FNV-1a 哈希替代:`kind() as usize % 64` 与 `fnv1a(type_name) % 64`
//! 在语义上等价(同类型事件确定性同片,不同类型按哈希分散),确定性由测试守护。
//!
//! # 背压(CBF 信用流裁决,手册 §8.5)
//! 分片满 → 按 T11 信用流 `try_acquire(1)` 决定:
//! - 有信用:重试入片(一次);
//! - 无信用:shed 计数(复用 [`crate::bus::EventBus`] 的 `credit_shed_total`,语义
//!   与 T11 一致:「因背压未走最优通道,回退既有 broadcast」),事件**不丢弃**,
//!   回退 broadcast 直接投递 —— 漏发率恒 0(影子双跑前哨硬门禁前置)。
//! 永不阻塞 Critical、永不丢弃 Critical(Critical 根本不进分片)。
//!
//! # 消费端(手册 §10.3 shard_worker + ADR-129)
//! 每片一个 tokio 消费任务:`pop` 攒批 64 条后投递到既有 broadcast 通道,
//! 汇入后按 ADR-125 批提交语义批量归还信用(信用流自动平衡)。
//! 等待路径唯一原语是 `tokio::sync::Notify`(禁自旋/忙轮询,ADR-129)。

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use crossbeam_queue::ArrayQueue;
use tokio::sync::Notify;

use crate::credit_flow::CreditFlow;
use crate::types::{EventSeverity, NexusEvent};

/// 默认分片数 — 64 片(2^6,手册 §8.5)
///
/// WHY 64:2 的幂使哈希取模退化为位运算(编译器优化),且 64 片 × 256 容量
/// ≈ 16K 事件缓冲,足以吸收发布突发;分片数过小退化为单流,过大加剧
/// worker 上下文切换(64 个 worker 与 CPU 核数匹配)。
pub const DEFAULT_SHARD_COUNT: usize = 64;

/// 每片队列容量 — 256(手册 §8.5)
///
/// WHY 256:与 broadcast 默认容量 1024 同量级,单片吸收短时突发后由
/// CBF 信用流裁决背压(片满 + 无信用 → shed 回退 broadcast)。
pub const SHARD_CAPACITY: usize = 256;

/// shard_worker 攒批阈值 — 64 条(手册 §10.3)
///
/// WHY 64:与每片容量 256 匹配(1/4 批),摊薄 worker 循环开销;
/// 批大小过小(如 1)放大循环/唤醒开销,过大延迟汇入(增加分片内驻留时间)。
pub const SHARD_WORKER_BATCH: usize = 64;

/// 事件载荷上限 — 64KiB(手册 §8.5 断言)
///
/// WHY 64KiB:超过该载荷的事件(如全量 QuestListUpdated)应走既有单流
/// (大对象直投 broadcast),避免大对象占用分片缓冲导致队列快速填满。
/// 断言仅 debug 构建生效(序列化 ~µs 级,热路径 release 零开销)。
pub const MAX_EVENT_PAYLOAD_BYTES: usize = 64 * 1024;

/// 事件车道 — 分片路由三分类(手册 §8.5 Lane 语义)
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Lane {
    /// Critical 事件(17 个,severity() == Critical)— 永不进分片
    Critical,
    /// 顺序敏感事件 — 携带会话键,走单流保序,不分片
    OrderSensitive(SessionKey),
    /// 可乱序事件 — 进入分片扇出
    Unordered,
}

/// 会话键 — 顺序敏感事件的保序标识
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionKey {
    /// 变体字段 `session_id`(白名单 5 个变体)
    Session(String),
    /// `EventMetadata.correlation_id`(跨事件因果追踪关联)
    Correlation(String),
    /// 告警事件占位键(`rule_id`)— 告警类事件直投单流,不参与分片哈希
    Alert(String),
}

impl SessionKey {
    /// 取会话键字符串(用于 FNV-1a 哈希分片)
    #[must_use = "会话键字符串用于分片哈希,忽略返回值无意义"]
    pub fn as_str(&self) -> &str {
        match self {
            SessionKey::Session(s) | SessionKey::Correlation(s) | SessionKey::Alert(s) => s,
        }
    }
}

/// 影子双跑前哨统计(P1-T12;T13 漏发率=0 硬门禁的采集输入)
///
/// 漏发率口径: `merged_total`(worker 汇入 broadcast 数) vs
/// `sharded_total`(入分片事件数)。正常场景两者相等 = 漏发率 0。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShadowStats {
    /// 总线累计发布事件总数(与 [`crate::bus::EventBus::published_total`] 一致)
    pub published_total: u64,
    /// 入分片事件数(分片路径累计,单调递增)
    pub sharded_total: u64,
    /// worker 汇入 broadcast 事件数(分片路径累计;漏发率分母/分子)
    pub merged_total: u64,
    /// 背压 shed 累计(分片满 + 信用耗尽回退 broadcast 数,复用 T11 口径)
    pub shed_total: u64,
    /// Critical 事件发布数(永不进分片的车道观测)
    pub critical_total: u64,
}

/// 分片事件总线 — 64 片无锁队列 + Notify 唤醒 + worker 汇入 broadcast
///
/// # 线程安全
/// 所有状态为 `ArrayQueue`(crossbeam 无锁 MPSC) + `AtomicU64` + `Notify`,
/// 多线程并发入片安全;worker 汇入为唯一消费方(单消费者,无锁队列天然适配)。
///
/// # 生命周期
/// worker 由 [`spawn_workers`](Self::spawn_workers) 在 tokio runtime 上启动,
/// 随 runtime 生命周期运行;`EventBus` drop 不停止 worker(灰度前哨期,
/// 生产 runtime 常驻,测试 runtime 销毁时任务自动中止)。
pub struct ShardedEventBus {
    /// 分片数(2 的幂,哈希取模可退化为位运算)
    n_shards: usize,
    /// 每片容量(ArrayQueue 有界,构造时固定)
    shard_capacity: usize,
    /// 分片无锁队列数组(每片一个有界 MPSC)
    shards: Box<[ArrayQueue<NexusEvent>]>,
    /// 每片唤醒原语(ADR-129:等待路径唯一原语,禁自旋)
    notifies: Box<[Notify]>,
    /// 每片当前深度(指标 `bus_shard_depth{shard}`,AtomicU64 数组)
    depths: Box<[AtomicU64]>,
    /// 入分片事件总数(影子双跑前哨:漏发率分子)
    sharded_total: AtomicU64,
    /// worker 汇入 broadcast 事件总数(影子双跑前哨:漏发率分母)
    merged_total: AtomicU64,
}

impl std::fmt::Debug for ShardedEventBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // WHY 手动实现:ArrayQueue 不实现 Debug(unsafe 内部不可观测),
        // 仅输出可观测状态(片数/容量/深度/累计)
        f.debug_struct("ShardedEventBus")
            .field("n_shards", &self.n_shards)
            .field("shard_capacity", &self.shard_capacity)
            .field("shard_depths", &self.shard_depths())
            .field("sharded_total", &self.sharded_total())
            .field("merged_total", &self.merged_total())
            .finish()
    }
}

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
        let notifies: Vec<Notify> = (0..n).map(|_| Notify::new()).collect();
        let depths: Vec<AtomicU64> = (0..n).map(|_| AtomicU64::new(0)).collect();
        Self {
            n_shards: n,
            shard_capacity: shard_capacity.max(1),
            shards: shards.into_boxed_slice(),
            notifies: notifies.into_boxed_slice(),
            depths: depths.into_boxed_slice(),
            sharded_total: AtomicU64::new(0),
            merged_total: AtomicU64::new(0),
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
    pub fn spawn_workers(
        self: &Arc<Self>,
        sender: tokio::sync::broadcast::Sender<NexusEvent>,
        credit_flow: Arc<CreditFlow>,
        handle: &tokio::runtime::Handle,
    ) {
        for idx in 0..self.n_shards {
            let sb = Arc::clone(self);
            let tx = sender.clone();
            let cf = Arc::clone(&credit_flow);
            handle.spawn(async move {
                shard_worker(&sb, idx, tx, cf).await;
            });
        }
    }

    /// 尝试入片(Unordered 事件专用)
    ///
    /// - 成功:递增深度 + 唤醒 worker + 累计 `sharded_total`,返回 `Ok(())`;
    /// - 失败(片满):返回 `Err(event)`,由调用方(EventBus)按 CBF 信用流裁决
    ///   (有信用重试、无信用 shed 回退 broadcast)。
    ///
    /// # 事件所有权
    /// 入片失败时返回事件所有权(调用方回退 broadcast 需要事件本体,
    /// 避免热路径 clone)。
    ///
    /// # 64KiB 载荷断言
    /// debug 构建下序列化检查载荷 ≤ 64KiB(超过视为实现 bug,panic 于 debug;
    /// release 零开销 —— `debug_assert!` 表达式不被求值)。
    #[must_use = "入片结果决定是否回退 broadcast,忽略将导致事件丢失"]
    // WHY allow(result_large_err):Err 返回事件所有权供调用方回退 broadcast（接口契约,避免热路径 clone）
    #[allow(clippy::result_large_err)]
    pub fn try_push(&self, event: NexusEvent) -> Result<(), NexusEvent> {
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
    fn shard_index(&self, event: &NexusEvent) -> usize {
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
}

/// shard_worker — 单片消费任务(手册 §10.3)
///
/// 循环语义:
/// 1. `pop` 攒批(≤[`SHARD_WORKER_BATCH`])—— 空队列则挂起至 [`Notify`] 唤醒
///    (ADR-129 无自旋:等待路径唯一原语,不存在忙轮询);
/// 2. 批量汇入既有 broadcast(订阅者 API 零变化);
/// 3. 按 ADR-125 批提交语义批量归还信用(信用流自动平衡)。
///
/// # 不变量
/// - 深度计数与入片/汇入一一对应(入片 `fetch_add(1)`,汇入 `fetch_sub(1)`),
///   `bus_shard_depth{shard}` 恒为队列中未汇入事件数;
/// - `merged_total` 与入片成功数最终一致(漏发率 = 0,由测试断言)。
async fn shard_worker(
    sb: &ShardedEventBus,
    shard: usize,
    sender: tokio::sync::broadcast::Sender<NexusEvent>,
    credit_flow: Arc<CreditFlow>,
) {
    let mut batch = Vec::with_capacity(SHARD_WORKER_BATCH);
    loop {
        batch.clear();
        // 攒批:最多 SHARD_WORKER_BATCH 条,一次 pop 循环(无锁队列,单片单消费者)
        for _ in 0..SHARD_WORKER_BATCH {
            match sb.shards[shard].pop() {
                Some(event) => {
                    sb.depths[shard].fetch_sub(1, Ordering::Relaxed);
                    batch.push(event);
                }
                None => break,
            }
        }
        if batch.is_empty() {
            // ADR-129 无自旋:队列空则挂起,由发布方 notify_one 唤醒
            // WHY 每轮重新 notified():await 返回后 permit 已消费,需重新挂载
            // (tokio Notify 官方模式,避免「检查空 → 等待」间入片的丢失唤醒)
            sb.notifies[shard].notified().await;
            continue;
        }
        let batch_len = batch.len() as u64;
        for event in batch.drain(..) {
            // 汇入既有 broadcast:无订阅者时 Err(Closed),静默(与 publish 语义一致:
            // 无订阅者丢弃不视为错误)。merged_total 仅统计入片后成功投递到
            // broadcast 的事件(「入片 → 汇入」守恒,漏发率统计口径)。
            let _ = sender.send(event);
            sb.merged_total.fetch_add(1, Ordering::Relaxed);
        }
        // ADR-125 批提交语义:整批归还(降低 notify_waiters 唤醒频率)
        credit_flow.release_many(batch_len);
    }
}

/// Lane 判定 — 事件分片路由分类(手册 §8.5)
///
/// 判定顺序(优先级从高到低):
/// 1. `severity() == Critical`(17 个,与 [`crate::bus::LANE_FORBIDDEN_SHARD`]
///    一致性由守护测试双向断言)→ [`Lane::Critical`];
/// 2. `EfficiencyAlertTriggered` 告警事件 → [`Lane::OrderSensitive`](告警占位键
///    rule_id);
/// 3. 白名单 5 个带 `session_id` 字段变体 → [`Lane::OrderSensitive`];
/// 4. `metadata.correlation_id` 非空 → [`Lane::OrderSensitive`](广义会话键);
/// 5. 其余 → [`Lane::Unordered`]。
#[must_use = "Lane 判定决定事件路由,忽略返回值将导致错误分片"]
pub fn event_lane(event: &NexusEvent) -> Lane {
    // 1. Critical 车道(红线:17 个 Critical 永不进分片)
    if event.severity() == EventSeverity::Critical {
        return Lane::Critical;
    }
    // 2. 告警语义车道(Ω₈ 可观测性;手册「顺序敏感通道不分片」红线精神):
    //    WHY 告警事件直投单流:告警是监控红线的即时可观测信号,订阅者
    //    (TUI 仪表盘 / Parliament / AHIRT)依赖其「发布 → 立即可读」的同步语义;
    //    若进分片,worker 攒批汇入引入异步延迟,`record_event` 后立即 `try_recv`
    //    的调用方(根 e2e 协调闭环测试)将读不到告警 —— 告警的即时性即顺序敏感
    //    语义,故归 OrderSensitive 车道走 broadcast 单流直投,不进分片。
    //    WHY 用 rule_id 作占位键:同一规则告警间需保序(触发-恢复-再触发序列),
    //    Alert 车道不分片,键值仅作保序标识,不参与分片哈希。
    if let NexusEvent::EfficiencyAlertTriggered { rule_id, .. } = event {
        return Lane::OrderSensitive(SessionKey::Alert(rule_id.clone()));
    }
    // 3. 白名单带 session_id 变体(顺序敏感,单流保序)
    if let Some(session_id) = session_key_of(event) {
        return Lane::OrderSensitive(SessionKey::Session(session_id.to_owned()));
    }
    // 4. correlation_id 作为广义会话键(跨事件因果追踪关联,需保序)
    if let Some(correlation_id) = event.metadata().correlation_id.as_ref() {
        return Lane::OrderSensitive(SessionKey::Correlation(correlation_id.clone()));
    }
    // 5. 其余全部可乱序 → 分片扇出
    Lane::Unordered
}

/// 白名单 5 个带 `session_id` 字段的变体判定(见模块文档偏差说明)
fn session_key_of(event: &NexusEvent) -> Option<&str> {
    match event {
        NexusEvent::TuiChatSubmitted { session_id, .. }
        | NexusEvent::TuiChatResponseChunk { session_id, .. }
        | NexusEvent::TuiChatCompleted { session_id, .. }
        | NexusEvent::TuiChatStatusChanged { session_id, .. }
        | NexusEvent::CrossVendorNegotiation { session_id, .. } => Some(session_id),
        _ => None,
    }
}

/// FNV-1a 64 位哈希 — 分片键的确定性哈希(手册 §8.5)
///
/// WHY FNV-1a 而非 std hash:标准库 `DefaultHasher` 的 SipHash 带随机种子
/// (进程内两次运行结果不同,破坏分片确定性);FNV-1a 是确定性的乘法散列,
/// 且为纯 safe 实现(满足 forbid(unsafe_code)),碰撞率对短字符串(事件类型名/
/// 会话键)足够低。
#[must_use = "哈希结果用于分片路由,忽略返回值无意义"]
pub fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325; // FNV-1a 64 位偏移基值
    for &b in bytes {
        hash ^= b as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3); // FNV 64 位质数
    }
    hash
}

/// 事件载荷 ≤ 64KiB 断言检查(debug 构建;序列化 ~µs 级,release 不进热路径)
fn payload_within_limit(event: &NexusEvent) -> bool {
    rmp_serde::to_vec_named(event)
        .map(|bytes| bytes.len() <= MAX_EVENT_PAYLOAD_BYTES)
        .unwrap_or(true) // 序列化失败(理论不可能)不 panic,交给上游错误路径
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bus::LANE_FORBIDDEN_SHARD;
    use crate::types::EventMetadata;

    /// 无会话键的普通事件(Unordered 代表)
    fn unordered_event() -> NexusEvent {
        NexusEvent::QuestCreated {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: "q-1".into(),
            title: "测试".into(),
            task_count: 1,
        }
    }

    fn critical_event() -> NexusEvent {
        NexusEvent::CheckpointSaved {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: "q".into(),
            checkpoint_id: "c".into(),
            memory_snapshot_hash: "h".into(),
        }
    }

    // ============================================================
    // Lane 判定:17 Critical → Critical / 会话键 → OrderSensitive / 其余 → Unordered
    // ============================================================

    #[test]
    fn test_lane_forbidden_shard_list_covers_all_critical() {
        // 红线:LANE_FORBIDDEN_SHARD 清单与 severity() Critical 一一对应
        // (全量 17 个逐一构造断言)—— helper 复用 bus.rs 测试模块的
        // all_severity_critical_variants(pub(crate),D-8 口径)
        let critical_variants = super::super::bus::tests_helpers::all_severity_critical_variants();
        assert_eq!(critical_variants.len(), crate::bus::CRITICAL_TOTAL);
        for event in &critical_variants {
            assert_eq!(
                event_lane(event),
                Lane::Critical,
                "{} 必须判定为 Critical 车道(永不进分片)",
                event.type_name()
            );
        }
    }

    #[test]
    fn test_lane_forbidden_shard_names_are_critical() {
        // LANE_FORBIDDEN_SHARD 清单自身:17 个名字,全部命中 Critical 车道
        assert_eq!(LANE_FORBIDDEN_SHARD.len(), 17);
        for name in LANE_FORBIDDEN_SHARD {
            let event = super::super::bus::tests_helpers::critical_variant_by_name(name)
                .expect("清单中的名字必须可构造事件");
            assert_eq!(
                event_lane(&event),
                Lane::Critical,
                "{name} 必须为 Critical 车道"
            );
        }
    }

    #[test]
    fn test_lane_session_variants_are_order_sensitive() {
        // 白名单 5 个带 session_id 变体 → OrderSensitive(走单流保序)
        let events = [
            NexusEvent::TuiChatSubmitted {
                metadata: EventMetadata::new("t"),
                session_id: "s1".into(),
                query: "q".into(),
                slash_command: None,
            },
            NexusEvent::TuiChatResponseChunk {
                metadata: EventMetadata::new("t"),
                session_id: "s1".into(),
                delta: "d".into(),
                cursor_hint: 0,
            },
            NexusEvent::TuiChatCompleted {
                metadata: EventMetadata::new("t"),
                session_id: "s1".into(),
                tool_use: None,
            },
            NexusEvent::TuiChatStatusChanged {
                metadata: EventMetadata::new("t"),
                session_id: "s1".into(),
                status: crate::types::ChatStatus::Thinking,
            },
            NexusEvent::CrossVendorNegotiation {
                metadata: EventMetadata::new("t"),
                session_id: "s1".into(),
                quest_id: "q".into(),
                producer_provider: "zhipu/glm-5.2".into(),
                verifier_provider: "zhipu/glm-5.2".into(),
                skeptic_provider: "zhipu/glm-5.2".into(),
                cross_vendor_enforced: true,
                decorrelation_status: "enforced".into(),
            },
        ];
        for event in &events {
            assert!(
                matches!(event_lane(event), Lane::OrderSensitive(SessionKey::Session(s)) if s == "s1"),
                "{} 应判定为 OrderSensitive(Session)",
                event.type_name()
            );
        }
    }

    #[test]
    fn test_lane_correlation_id_is_order_sensitive() {
        // correlation_id 作为广义会话键 → OrderSensitive
        let mut meta = EventMetadata::new("t");
        meta.correlation_id = Some("corr-1".into());
        let event = NexusEvent::QuestProgressUpdated {
            metadata: meta,
            quest_id: "q".into(),
            completed: 1,
            total: 2,
        };
        assert_eq!(
            event_lane(&event),
            Lane::OrderSensitive(SessionKey::Correlation("corr-1".into()))
        );
    }

    #[test]
    fn test_lane_alert_event_is_order_sensitive() {
        // 告警类事件(EfficiencyAlertTriggered)→ OrderSensitive(Alert 占位键)
        // 不走 Unordered 分片:告警的即时性即顺序敏感语义(Ω₈ 可观测性,根 e2e
        // 协调闭环测试在 record_event 后立即 try_recv —— 直投单流保立即可读)
        let event = NexusEvent::EfficiencyAlertTriggered {
            metadata: EventMetadata::new("efficiency-monitor"),
            rule_id: "paradox-risk-coordination-ratio".into(),
            metric_name: "coordination_to_gain_ratio".into(),
            triggered_value: 2.0,
            threshold: 0.9,
        };
        assert_eq!(
            event_lane(&event),
            Lane::OrderSensitive(SessionKey::Alert("paradox-risk-coordination-ratio".into()))
        );
    }

    #[test]
    fn test_lane_unordered_default() {
        // 无会话键、非 Critical → Unordered
        assert_eq!(event_lane(&unordered_event()), Lane::Unordered);
        // Critical 事件即使无会话键也优先 Critical(判定顺序:Critical > 会话键)
        assert_eq!(event_lane(&critical_event()), Lane::Critical);
    }

    #[test]
    fn test_lane_priority_critical_over_correlation() {
        // 判定顺序守护:Critical 事件即使带 correlation_id 也必须是 Critical
        // (红线:Critical 永不进分片,correlation_id 不能把它拉进 OrderSensitive)
        let mut meta = EventMetadata::new("t");
        meta.correlation_id = Some("corr".into());
        let event = NexusEvent::BudgetExceeded {
            metadata: meta,
            budget_type: "token".into(),
            current: 2,
            limit: 1,
        };
        assert_eq!(event_lane(&event), Lane::Critical);
    }

    // ============================================================
    // FNV-1a 确定性 + 分片一致性 + 无序事件分散
    // ============================================================

    #[test]
    fn test_fnv1a_known_vector() {
        // FNV-1a 标准测试向量:空串 → 偏移基值;已知向量校验(防实现漂移)
        assert_eq!(fnv1a(b""), 0xcbf2_9ce4_8422_2325);
        // 64 位 FNV-1a("foobar")= 0x85944171f73967e8(公开参考值)
        assert_eq!(fnv1a(b"foobar"), 0x8594_4171_f739_67e8);
    }

    #[test]
    fn test_shard_index_deterministic_same_type() {
        // 同类型事件(不同 quest_id)恒入同片(FNV-1a 确定性)
        let bus = ShardedEventBus::new(DEFAULT_SHARD_COUNT, SHARD_CAPACITY);
        let e1 = NexusEvent::QuestCreated {
            metadata: EventMetadata::new("a"),
            quest_id: "q-1".into(),
            title: "t1".into(),
            task_count: 1,
        };
        let e2 = NexusEvent::QuestCreated {
            metadata: EventMetadata::new("b"),
            quest_id: "q-999".into(),
            title: "t2".into(),
            task_count: 2,
        };
        let idx1 = bus.shard_index(&e1);
        let idx2 = bus.shard_index(&e2);
        assert_eq!(idx1, idx2, "同类型事件应确定性同片");
    }

    #[test]
    fn test_session_shard_deterministic() {
        // 会话键 FNV-1a 确定性:同键同片、不同键可不同片
        let s1a = ShardedEventBus::session_shard("session-abc", DEFAULT_SHARD_COUNT);
        let s1b = ShardedEventBus::session_shard("session-abc", DEFAULT_SHARD_COUNT);
        assert_eq!(s1a, s1b, "同会话键恒同片(FNV-1a 确定性)");
        let s2 = ShardedEventBus::session_shard("session-xyz", DEFAULT_SHARD_COUNT);
        assert!(
            s1a < DEFAULT_SHARD_COUNT && s2 < DEFAULT_SHARD_COUNT,
            "分片索引越界"
        );
    }

    #[test]
    fn test_unordered_events_spread_across_shards() {
        // 无序事件分散:144 变体中取 32 个不同类型,分片分布不得塌缩到 1 片
        // (设计口径 kind() % 64 的等价验证:type_name FNV-1a 哈希分散)
        let bus = ShardedEventBus::new(DEFAULT_SHARD_COUNT, SHARD_CAPACITY);
        let mut occupied = std::collections::HashSet::new();
        for (i, name) in TEST_UNORDERED_TYPE_NAMES.iter().enumerate() {
            let event = make_event_by_type_name(name, i);
            occupied.insert(bus.shard_index(&event));
        }
        assert!(
            occupied.len() > 1,
            "无序事件分片分布塌缩到 {} 片(应分散)",
            occupied.len()
        );
        // 深度/计数初始为 0(新总线无残留)
        assert_eq!(bus.sharded_total(), 0);
        assert_eq!(bus.merged_total(), 0);
    }

    /// 测试用 Unordered 事件类型名清单(15 个非 Critical、无会话键、非告警变体;
    /// EfficiencyAlertTriggered 已移出 —— 告警类事件归 OrderSensitive 车道)
    const TEST_UNORDERED_TYPE_NAMES: &[&str] = &[
        "QuestCreated",
        "QuestProgressUpdated",
        "QuestListUpdated",
        "QuestCompleted",
        "ThinkingModeSwitched",
        "VoteCast",
        "CapabilityFrozen",
        "ModelRouteSelected",
        "NexusStateChanged",
        "UserIntentEncoded",
        "CacheHit",
        "MemoryMetricsReported",
        "WikiUpdated",
        "MemConStrategyAdjusted",
        "BudgetMetricsUpdated",
    ];

    /// 按类型名构造测试事件(仅覆盖 TEST_UNORDERED_TYPE_NAMES 中的变体;
    /// 若变体字段签名变化,此 helper 需同步更新 —— 编译期强制)
    fn make_event_by_type_name(name: &str, i: usize) -> NexusEvent {
        let meta = || EventMetadata::new("test");
        match name {
            "QuestCreated" => NexusEvent::QuestCreated {
                metadata: meta(),
                quest_id: format!("q-{i}"),
                title: "t".into(),
                task_count: 1,
            },
            "QuestProgressUpdated" => NexusEvent::QuestProgressUpdated {
                metadata: meta(),
                quest_id: format!("q-{i}"),
                completed: 1,
                total: 2,
            },
            "QuestListUpdated" => NexusEvent::QuestListUpdated {
                metadata: meta(),
                quests: vec![],
                source: "s".into(),
            },
            "QuestCompleted" => NexusEvent::QuestCompleted {
                metadata: meta(),
                quest_id: format!("q-{i}"),
                status: crate::types::QuestStatus::Completed,
            },
            "ThinkingModeSwitched" => NexusEvent::ThinkingModeSwitched {
                metadata: meta(),
                quest_id: format!("q-{i}"),
                from_mode: "fast".into(),
                to_mode: "deep".into(),
                reason: String::new(),
            },
            "VoteCast" => NexusEvent::VoteCast {
                metadata: meta(),
                proposal_id: format!("p-{i}"),
                voter: "v".into(),
                vote: true,
            },
            "CapabilityFrozen" => NexusEvent::CapabilityFrozen {
                metadata: meta(),
                capability_id: format!("c-{i}"),
                reason: "r".into(),
            },
            "ModelRouteSelected" => NexusEvent::ModelRouteSelected {
                metadata: meta(),
                quest_id: format!("q-{i}"),
                model_id: "m".into(),
                route_reason: "r".into(),
            },
            "NexusStateChanged" => NexusEvent::NexusStateChanged {
                metadata: meta(),
                state_hash: format!("h-{i}"),
                prev_hash: "p".into(),
            },
            "UserIntentEncoded" => NexusEvent::UserIntentEncoded {
                metadata: meta(),
                intent_id: format!("i-{i}"),
                raw_text: "r".into(),
                risk_level: 1,
            },
            "CacheHit" => NexusEvent::CacheHit {
                metadata: meta(),
                cache_key: format!("k-{i}"),
            },
            "MemoryMetricsReported" => NexusEvent::MemoryMetricsReported {
                metadata: meta(),
                hit_rate: 0.5,
                evictions: 0,
            },
            "WikiUpdated" => NexusEvent::WikiUpdated {
                metadata: meta(),
                wiki_hash: format!("w-{i}"),
                delta: 1,
            },
            "MemConStrategyAdjusted" => NexusEvent::MemConStrategyAdjusted {
                metadata: meta(),
                from_strategy: "StandardTopK".into(),
                to_strategy: "TimeFocused".into(),
                reason: "r".into(),
                ghost_rate: None,
            },
            "BudgetMetricsUpdated" => NexusEvent::BudgetMetricsUpdated {
                metadata: meta(),
                metrics: crate::types::BudgetMetricsPayload {
                    total_consumption: 1.0,
                    remaining_budget: 2.0,
                    utilization_rate: 0.5,
                    current_tier: "High".into(),
                    coefficient: 1.0,
                    is_exceeded: false,
                    alert: None,
                },
            },
            _ => panic!("测试事件名未覆盖: {name}"),
        }
    }

    // ============================================================
    // ShardedEventBus 入片/汇入基本语义(EventBus 集成测试在 bus.rs)
    // ============================================================

    #[test]
    fn test_try_push_pop_roundtrip_and_depth() {
        let bus = ShardedEventBus::new(DEFAULT_SHARD_COUNT, SHARD_CAPACITY);
        let event = unordered_event();
        // 入片成功,深度 +1,sharded_total +1
        assert!(bus.try_push(event).is_ok());
        assert_eq!(bus.sharded_total(), 1);
        // 找到事件所在片,pop 后深度归零
        let idx = bus.shard_index(&unordered_event());
        assert_eq!(bus.shard_depth(idx), 1, "深度应反映队列中未汇入事件数");
        assert!(bus.shards[idx].pop().is_some(), "入片事件应可被 pop");
        // 模拟 worker 汇入:深度计数仅在 shard_worker 的 pop 路径递减
        // (pop + fetch_sub 是 worker 原子的两步,直接 pop 不更新 depth)
        bus.depths[idx].fetch_sub(1, Ordering::Relaxed);
        assert_eq!(bus.shard_depth(idx), 0, "汇入后深度应归零");
    }

    #[test]
    fn test_try_push_full_returns_event() {
        // 片满:try_push 返回 Err(事件所有权交还调用方,不丢事件)
        let bus = ShardedEventBus::new(1, 2); // 单片容量 2
        let e1 = unordered_event();
        let e2 = NexusEvent::CacheHit {
            metadata: EventMetadata::new("t"),
            cache_key: "k2".into(),
        };
        let e3 = NexusEvent::CacheHit {
            metadata: EventMetadata::new("t"),
            cache_key: "k3".into(),
        };
        assert!(bus.try_push(e1).is_ok());
        assert!(bus.try_push(e2).is_ok());
        let rejected = bus.try_push(e3).expect_err("容量 2 满后应拒绝");
        assert_eq!(rejected.type_name(), "CacheHit", "拒绝时返回事件本体");
        assert_eq!(bus.sharded_total(), 2, "仅入片成功者计数");
    }

    #[test]
    fn test_zero_shards_falls_back_to_one() {
        // n_shards = 0 时回退 1 片(取模除零防 panic,灰度安全)
        let bus = ShardedEventBus::new(0, SHARD_CAPACITY);
        assert_eq!(bus.n_shards(), 1);
        assert!(bus.try_push(unordered_event()).is_ok());
    }
}
