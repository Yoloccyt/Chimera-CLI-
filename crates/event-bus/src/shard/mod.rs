//! ShardedBus 分片核心 — 非 Critical 事件的分片扇出(灰度增量)
//!
//! 状态(ADR-181 发现 1 订正;接线清单 2026-10-05 第三轮 T1.3 重测):PRODUCTION-WIRED ——
//! cmt-tiering/efficiency-monitor/faae-router/gqep-executor/mlc-engine/nmc-encoder/
//! osa-coordinator/scc-cache/omega-learner 等 10 处运行时 opt-in `try_enable_sharding(...)`
//! (无 tokio runtime 自动降级单流,幂等拒绝重复启用);旧述「五处含 gsoe」已失实
//! ——gsoe-evolution 实测零调用(第三轮审计 F-X5 漂移 #4),非"未接线"定性与本行无关。
//!
//! 对应架构层:L1 Core(event-bus)
//! 对应任务:P1-T12(Phase 1 地基波次,手册 §8.5 ShardedEventBus 终版 + E8-4 修正;
//! v4.0 WI-15 阶段一「PatternIndex 精确索引先行」所指向的 pattern_index 模块已随
//! B5/ADR-312 退役,精确索引接续点由本模块的 Lane 判定承担)
//!
//! # 为什么新建 shard.rs 而非扩展 backpressure.rs
//! `backpressure.rs` 的职责是「背压策略判定 + 慢消费者检测」(`BackpressurePolicy`/
//! `SlowConsumerDetector`),语义上是被动保护;本模块的职责是「发布端并行化」——
//! Lane 三车道判定 + 64 片无锁队列扇出 + worker 汇入既有 broadcast,语义上是
//! 主动路由。两者正交,合并会造成单文件双主题(维护者需跳跃理解),故独立成模块。
//!
//! # Lane 三车道(手册 §8.5)
//! - `Critical`：22 个 Critical 事件（`severity() == Critical`，与
//!   [`crate::bus::LANE_FORBIDDEN_SHARD`] 一一对应，守护测试保证）
//!   —— 上方数字**不是无人管的裸注释**：它由 `scripts/check_perf_redlines.py` 不变式 **I8**
//!   机器校验必须等于 `bus::CRITICAL_TOTAL`（深审 E-7c：此处曾手工漂过 17→18，
//!   I8 使其不可再现）；改常量而不同步本注释会直接判红，故与 `registry.rs` 里
//!   **无门校验**、只能指向常量名的旁路计数注释属两种口径。→ **永远走既有 mpsc/broadcast 通道，不进分片**（红线：
//!   Critical 分片会破坏"发布方 → 订阅方"全序投递语义与 mpsc 旁路免背压保证,
//!   推演 9:Critical 背压 = 死锁源);
//! - `OrderSensitive(SessionKey)`:携带会话键(白名单变体的 `session_id` 字段 /
//!   `metadata.correlation_id`)的事件,以及告警类事件(`EfficiencyAlertTriggered`,
//!   占位键 rule_id)→ **顺序敏感通道保持单流,不分片**
//!   (E8-4:分片仅限可乱序订阅者通道;同一会话内事件必须保序,单流即保序;
//!   Ω₈ 可观测性:告警事件需即时同步可见 —— 直投单流无 worker 攒批延迟,详见
//!   [`event_lane`] 告警语义判定);

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Weak};

use crossbeam_queue::ArrayQueue;
use tokio::sync::Notify;
use tracing::{debug, warn};

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
    /// Critical 事件(21 个,severity() == Critical)— 永不进分片
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
///
/// 送达语义(第三轮 F-A2 补齐,2026-10-05):`merged_total` 无条件计入,
/// 包含「汇入 broadcast 但当下无订阅者而被丢弃」的部分——该等式只证
/// 「入汇流通道」不证「有人收到」;无订阅者丢弃数由 `merge_closed_total`
/// 单独观测(第三轮 T1.1),慢消费者丢弃仍在 recv 侧 `lagged_count`
/// (两条路径共用同一订阅者层计数器,非分片路径新增面)。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShadowStats {
    /// 总线累计发布事件总数(与 [`crate::bus::EventBus::published_total`] 一致)
    pub published_total: u64,
    /// 入分片事件数(分片路径累计,单调递增)
    pub sharded_total: u64,
    /// worker 汇入 broadcast 事件总数(分片路径累计;漏发率分母;
    /// 含无订阅者丢弃,送达观测见 [`ShadowStats::merge_closed_total`])
    pub merged_total: u64,
    /// 背压 shed 累计(分片满 + 信用耗尽回退 broadcast 数,复用 T11 口径)
    pub shed_total: u64,
    /// Critical 事件发布数(永不进分片的车道观测)
    pub critical_total: u64,
    /// worker 汇入时 `send` 返回 Err(当下无订阅者)的事件数(第三轮 T1.1)。
    /// 非丢发错误(慢消费者是 recv 侧 Lagged),是「入通道即丢弃」的观测面:
    /// 它为 `merged_total` 的「入通道≠送达」提供可归因数字。
    pub merge_closed_total: u64,
}

/// 分片事件总线 — 64 片无锁队列 + Notify 唤醒 + worker 汇入 broadcast
///
/// # 线程安全
/// 所有状态为 `ArrayQueue`(crossbeam 无锁 MPSC) + `AtomicU64` + `Notify`,
/// 多线程并发入片安全;worker 汇入为唯一消费方(单消费者,无锁队列天然适配)。
///
/// # 生命周期
/// worker 由 [`spawn_workers`](Self::spawn_workers) 在 tokio runtime 上启动,
/// 仅持 `Weak<ShardedEventBus>` 引用（性能重构 D2）：当所有 [`EventBus`] clone drop
/// 时，外层 guard abort worker 且分片总线可被回收；长驻宿主可经
/// [`EventBus::shutdown_sharding`](crate::EventBus::shutdown_sharding) 优雅排空。
/// （旧设计"worker 持强引用且无退出条件"导致重复 build 泄漏，已根治。）
pub struct ShardedEventBus {
    /// 分片数(2 的幂,哈希取模可退化为位运算)
    n_shards: usize,
    /// 每片容量(ArrayQueue 有界,构造时固定)
    shard_capacity: usize,
    /// 分片无锁队列数组(每片一个有界 MPSC)
    shards: Box<[ArrayQueue<NexusEvent>]>,
    /// 每片唤醒原语(ADR-129:等待路径唯一原语,禁自旋)
    ///
    /// 性能重构 D2:`Arc<Notify>` 使 worker 可在 `await` 前克隆唤醒句柄并释放
    /// `ShardedEventBus` 强引用 —— 否则 worker 持强引用会阻止总线 drop。
    notifies: Box<[Arc<Notify>]>,
    /// 每片当前深度(指标 `bus_shard_depth{shard}`,AtomicU64 数组)
    depths: Box<[AtomicU64]>,
    /// 入分片事件总数(影子双跑前哨:漏发率分子)
    sharded_total: AtomicU64,
    /// worker 汇入 broadcast 事件总数(影子双跑前哨:漏发率分母)
    merged_total: AtomicU64,
    /// worker 汇入时 send 因无订阅者返回 Err 的累计数(第三轮 T1.1 观测面;
    /// 首见 warn 后续 debug 的 once 语义由 fetch_add 返回值 `closed == 0` 承担,
    /// 无需额外标志位 —— 计数本身就是「首次」的无锁仲裁)
    merge_closed_total: AtomicU64,
    /// 优雅排空标志(性能重构 D2)—— [`request_drain`](Self::request_drain) 置 true
    /// 后，worker 将本片 drain-to-empty 再退出（而非在 `Notify` 上永久挂起）。
    /// Acquire/Release 配对：发布者 `request_drain` Release 存，worker Acquire 读。
    draining: AtomicBool,
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

mod queue;

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
    weak: Weak<ShardedEventBus>,
    shard: usize,
    sender: tokio::sync::broadcast::Sender<NexusEvent>,
    credit_flow: Arc<CreditFlow>,
) {
    let mut batch = Vec::with_capacity(SHARD_WORKER_BATCH);
    loop {
        // 每轮 upgrade 取一份强引用，处理完/挂起前释放（D2：不跨 await 持强引用）。
        let sb = match weak.upgrade() {
            Some(sb) => sb,
            // 总线已释放：无强引用可持有，本片残留随 ShardedEventBus 一并 drop
            //（进程 teardown 语义；正常投递期总线始终存活，不触发此分支）。
            None => break,
        };
        let draining = sb.draining.load(Ordering::Acquire);

        // 攒批:最多 SHARD_WORKER_BATCH 条,一次 pop 循环(无锁队列,单片单消费者)
        batch.clear();
        for _ in 0..SHARD_WORKER_BATCH {
            match sb.shards[shard].pop() {
                Some(event) => {
                    sb.depths[shard].fetch_sub(1, Ordering::Relaxed);
                    batch.push(event);
                }
                None => break,
            }
        }
        let popped = !batch.is_empty();
        if popped {
            let batch_len = batch.len() as u64;
            for event in batch.drain(..) {
                // 汇入既有 broadcast。语义登记(第三轮 F-A2/T1.1)：
                //   - tokio broadcast 的 `send` 错误形态只有「当下无订阅者」(Closed)；
                //     慢消费者 Lagged 发生在 recv 侧且已由订阅者层 `lagged_count` 计数。
                //   - merged_total 无条件递增的契约保持不变（漏发率等式口径），
                //     「入通道≠送达」由 merge_closed_total 单独归因，首见 warn 后续 debug。
                match sender.send(event) {
                    Ok(_) => {}
                    Err(_) => {
                        let closed = sb.merge_closed_total.fetch_add(1, Ordering::Relaxed);
                        if closed == 0 {
                            warn!("分片汇入时零订阅者:merged_total 含此类丢弃,送达观测见 merge_closed_total");
                        } else {
                            debug!(
                                merge_closed_total = closed + 1,
                                "分片汇入零订阅者丢弃(已告警一次)"
                            );
                        }
                    }
                }
                sb.merged_total.fetch_add(1, Ordering::Relaxed);
            }
            // ADR-125 批提交语义:整批归还(降低 notify_waiters 唤醒频率)
            credit_flow.release_many(batch_len);
        }

        // 终止/挂起判定
        if draining {
            // 优雅排空：本片排空则退出；否则让出后继续 pop（不阻塞在 Notify 上）。
            if sb.shards[shard].is_empty() {
                break;
            }
            drop(sb);
            tokio::task::yield_now().await;
            continue;
        }
        if !popped {
            // ADR-129 无自旋：队列空则挂起，由发布方 notify_one 唤醒。
            // 克隆唤醒句柄并**释放强引用后**再 await（否则持强引用会阻止总线 drop）；
            // 每轮重新 notified() 避免「检查空 → 等待」间入片的丢失唤醒。
            let notify = Arc::clone(&sb.notifies[shard]);
            drop(sb);
            notify.notified().await;
        }
        // popped && !draining → 立即进入下一轮继续攒批
    }
}

/// Lane 判定 — 事件分片路由分类(手册 §8.5)
///
/// 判定顺序(优先级从高到低):
/// 1. `severity() == Critical`(21 个,与 [`crate::bus::LANE_FORBIDDEN_SHARD`]
///    一致性由守护测试双向断言)→ [`Lane::Critical`];
/// 2. `EfficiencyAlertTriggered` 告警事件 → [`Lane::OrderSensitive`](告警占位键
///    rule_id);
/// 3. 白名单 5 个带 `session_id` 字段变体 → [`Lane::OrderSensitive`];
/// 4. `metadata.correlation_id` 非空 → [`Lane::OrderSensitive`](广义会话键);
/// 5. 其余 → [`Lane::Unordered`]。
#[must_use = "Lane 判定决定事件路由,忽略返回值将导致错误分片"]
pub fn event_lane(event: &NexusEvent) -> Lane {
    // 1. Critical 车道(红线:22 个 Critical 永不进分片)
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
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests;
