//! 事件总线实现 — 基于 tokio::broadcast 的 typed broadcast bus
//!
//! 对应架构:L1 Core,所有跨层通信的唯一通道(§2.2 依赖铁律)
//!
//! # 设计要点
//! - 封装 `tokio::broadcast::channel`,提供类型安全的发布订阅
//! - 序列化采用 MessagePack(ADR-004),提供跨进程投递能力
//! - 关键事件标注 Critical,背压策略据此保护(见 backpressure 模块)
//! - 所有 async fn 满足 Send 约束,可被 tokio::spawn

use crate::credit_flow::{CreditFlow, CreditStats};
use crate::error::EventBusError;
use crate::logging::BusLogger;
use crate::membrane::{MembraneFilter, PermeationDecision};
use crate::shard::{event_lane, Lane, ShadowStats, ShardedEventBus, SHARD_CAPACITY};
use crate::types::{EventMetadata, EventSeverity, NexusEvent};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, mpsc};
use tracing::warn;

/// 默认广播容量
///
/// WHY:1024 平衡内存占用与突发流量。每个 NexusEvent 约 200-500 字节,
/// 1024 容量约占 0.5MB,可吸收短时突发;持续高吞吐应增大容量或加背压策略。
pub const DEFAULT_CAPACITY: usize = 1024;

/// Critical 旁路通道容量 — P1-W2.1(D3 改造)
///
/// WHY 4096:spec.md L84 明确要求 "改有界 Sender<4096>"。
/// 4096 容量可吸收短时突发(如 Quest 启动初期多个安全告警并发),
/// 同时提供硬上限防止慢消费者导致 OOM(§6.1 红线 "1M Token 暴力加载")。
/// 容量满时按优先级采样丢弃(见 `send_critical_mpsc` 的 try_send 策略)。
pub const CRITICAL_CHANNEL_CAPACITY: usize = 4096;

/// Critical mpsc 旁路变体数 — D-8 口径(与 `is_critical_mpsc_event` 匹配数)
///
/// WHY 常量而非硬编码:双清单同步红线(MCA M0 起)要求新增 Critical 事件
/// 必须同时修改 severity() 与 is_critical_mpsc_event 两处;此常量供
/// 守护测试断言旁路清单规模不回退(13 ⊆ 17)。
pub const CRITICAL_MPSC_VARIANTS: usize = 13;

/// severity() 返回 Critical 的变体总数 — D-8 口径
///
/// 17 = 13(mpsc 旁路)+ 4(历史 Critical 只走 broadcast:
/// CheckpointSaved / ConsensusReached / SlowConsumerDropped / OrphanCallDetected)。
/// 本任务不改变 17 个 Critical 事件的通道归属(既定设计,推演 D-8 裁决)。
pub const CRITICAL_TOTAL: usize = 17;

/// 分片禁区声明 — 17 个 Critical 变体名清单(T12 分片时使用)
///
/// WHY 声明但不实现:T12 分片改造(SPSC 环阵列 + 分片总线)将按此清单
/// 禁止把 Critical 单流切片 —— Critical 分片会破坏"发布方 → 订阅方"的
/// 全序投递语义与 mpsc 旁路免背压保证(推演 9:Critical 背压 = 死锁源)。
/// 本任务只声明常量 + 测试守护(断言 17 个名字与 severity() Critical 清单
/// 一一对应),不实现分片总线。
pub const LANE_FORBIDDEN_SHARD: &[&str] = &[
    "CheckpointSaved",
    "ConsensusReached",
    "SlowConsumerDropped",
    "OrphanCallDetected",
    "SkepticVeto",
    "VetoOverridden",
    "RedTeamAudit",
    "BudgetExceeded",
    "AgentTaskFailed",
    "AsaIntervention",
    "R1ShadowRollbackFailed",
    "R2FreezeViolation",
    "R2FreezeRollbackFailed",
    "AffinityQuotaExhausted",
    "FormalViolation",
    "StopRulingIssued",
    "ErrorSignatureMatched",
];

/// 判断事件是否走 mpsc 旁路通道(Critical 安全/治理告警事件)
///
/// §6.2 红线要求:Critical 安全/治理告警事件必须用 mpsc channel
/// 确保送达,避免 broadcast 在 Lagged 场景下丢失。这与 `NexusEvent::severity()`
/// 部分重叠但语义不同:
/// - `severity()` 是事件总线背压级别(同步函数,不依赖运行时值;
///   AsaIntervention 自 P1-W2.1.4 起统一返回 Critical,见 types.rs L878-887)
/// - `is_critical_mpsc_event` 是 mpsc 旁路通道判定,权威清单即下方 matches! 臂,
///   规模锚定 [`CRITICAL_MPSC_VARIANTS`](当前 13)
///
/// WHY 单独定义:AsaIntervention 的 severity() 曾返回 Normal(P1-W2.1.4 修复前),
/// 旁路判定因此独立于 severity() 存在;修复后两清单语义已对齐,由 D-8/R7
/// 互锁测试守护(13 ⊆ 17)。
///
/// # 双清单同步红线(MCA M0 起显式声明)
/// 本函数与 `NexusEvent::severity()` 是两张独立清单:新增 Critical 事件
/// **必须同时修改两处**,只改 severity() 会导致"标 Critical 但 broadcast
/// Lagged 时丢失"(旁路不生效)。同步性由**本文件测试模块**三层守护:
/// `test_critical_severity_implies_mpsc_bypass`(13 项手抄清单)、
/// `test_critical_double_list_d8_counts`(常量锚定 + LANE_FORBIDDEN_SHARD
/// 双向一一对应)与 R7 互锁断言(清单项 severity() 反查)。
fn is_critical_mpsc_event(event: &NexusEvent) -> bool {
    matches!(
        event,
        NexusEvent::SkepticVeto { .. }
            | NexusEvent::RedTeamAudit { .. }
            | NexusEvent::BudgetExceeded { .. }
            // P1-2:AgentTaskFailed 纳入双清单(severity() 已是 Critical,types.rs L2579-2582)
            // WHY:delegation.rs 走 publish_critical 双通道(行为已合规),但若未来其他发布方
            // 改用 publish/publish_batch,则仅依赖 is_critical_mpsc_event 判定走旁路——
            // 缺失此变体将导致失败事件在 broadcast Lagged 场景下丢失(孤儿任务,§6.1 红线)。
            | NexusEvent::AgentTaskFailed { .. }
            | NexusEvent::AsaIntervention { .. }
            // MCA M0(ADR-065):厂商额度耗尽必须确保投递(触发降级链切换,
            // 与 types.rs severity() 双清单同步)
            | NexusEvent::AffinityQuotaExhausted { .. }
            // ADR-042 决策 4:R2 冻结违反 + 回滚失败为 Critical
            // WHY:severity() 注释已声明"必须走 mpsc 旁路通道确保投递",
            // 但实际未列入 is_critical_mpsc_event (P3-14 修复)。
            // R2 违反等同于安全事件(奖励黑客风险立即生效),回滚失败意味着
            // R2 路径代码可能仍在生效。必须走 mpsc 旁路确保投递,对齐 §6.2 红线 5。
            | NexusEvent::R2FreezeViolation { .. }
            | NexusEvent::R2FreezeRollbackFailed { .. }
            // P1-5:FormalViolation 纳入 mpsc 旁路(违反即否决的投递保证,
            // 与 types.rs severity() 双清单同步;丢失导致契约违反无人审议,
            // 候选继续进入后续阶段,违反九层防御 L0 语义)
            | NexusEvent::FormalViolation { .. }
            // Phase 10 Wave 5 双清单对齐(§16 审计修复):
            // VetoOverridden(否决覆盖审计不可追溯风险)与 R1ShadowRollbackFailed
            // (退化策略可能仍在生效)此前标 Critical 但未入旁路——丢失风险
            // 与 severity() 注释的"必须确保投递"语义冲突,现对齐。
            | NexusEvent::VetoOverridden { .. }
            | NexusEvent::R1ShadowRollbackFailed { .. }
            // §16.4 新增 Critical(Phase 10 Wave 4):停止裁决丢失导致 Quest
            // 无界运行;错误签名匹配丢失导致 Debug 算子无法检索同签名兄弟。
            | NexusEvent::StopRulingIssued { .. }
            | NexusEvent::ErrorSignatureMatched { .. }
    )
}

/// 事件总线 — 跨层通信的唯一通道
///
/// 基于 `tokio::broadcast::Sender<NexusEvent>`,支持多订阅者广播。
/// Clone 廉价(仅 Arc 引用计数),可在任务间自由传递。
///
/// 可选配备 `BusLogger` 实现全链路结构化日志埋点,
/// 记录订阅者连接/断开、事件发布/接收、错误码、重连尝试等关键信息。
///
/// # Critical 事件双通道(§6.2 红线,2026-06-29)
/// Critical 安全/治理告警事件(权威清单 = [`is_critical_mpsc_event`],
/// 规模锚定 [`CRITICAL_MPSC_VARIANTS`],当前 13 类)额外走 mpsc 旁路通道,
/// 确保在 broadcast Lagged 场景下仍能被订阅者接收。订阅者通过
/// [`subscribe_critical_events`](Self::subscribe_critical_events)
/// 获取 mpsc Receiver。旁路通道按需初始化(首次订阅时创建),无订阅者时
/// `publish` 仅走 broadcast 并发出 warn 告警(C3,2026-09-04)。
///
/// # P1-W2.1 有界化改造(D3 修复,2026-07-23)
/// Critical 旁路通道从 `Vec<UnboundedSender>` 改为 `Vec<mpsc::Sender<4096>>`,
/// 提供硬上限防止慢消费者导致 OOM(§6.1 红线)。容量满时按优先级采样丢弃
/// (见 `send_critical_mpsc` 的 try_send 策略),丢弃计数累计在
/// `critical_dropped_count` 字段(Arc<AtomicU64>,避免持锁跨 await)。
/// `subscribe_critical_events` 返回类型从 `UnboundedReceiver` 改为有界
/// `mpsc::Receiver`,调用方仅需 `.recv().await`(两类型此方法签名兼容,零改动)。
#[derive(Clone)]
pub struct EventBus {
    sender: broadcast::Sender<NexusEvent>,
    /// 通道容量(创建时固定,broadcast::Sender 不暴露 capacity())
    capacity: usize,
    /// 可选日志记录器(Arc 共享,跨 Clone 共享同一计数器)
    logger: Option<Arc<BusLogger>>,
    /// Critical 事件 mpsc 旁路通道(§6.2 红线双通道化,P1-W2.1 有界化)
    ///
    /// WHY Arc<Mutex<Vec<mpsc::Sender>>>:
    /// - `Vec<mpsc::Sender>` fan-out 模式:每个 subscribe_critical_events
    ///   调用创建独立有界 mpsc channel(容量 CRITICAL_CHANNEL_CAPACITY=4096),
    ///   Sender 入 Vec,Receiver 返回给订阅者
    /// - `Mutex` 同步 Vec 修改(订阅/发送互斥),仅在 push/retain 时短暂持有,
    ///   不跨 await(§4.4 红线 1)
    /// - `Arc` 使 EventBus 保留 Clone 派生(所有 Clone 副本共享同一 Vec)
    /// - `mpsc::Sender` 实现 Clone,EventBus Clone 副本可向同一 Vec 投递
    /// - receiver drop 时 try_send 返回 Err(Closed),send_critical_mpsc
    ///   静默忽略并定期清理失效 sender(避免 Vec 无限增长)
    /// - 容量满时 try_send 返回 Err(Full),递增 critical_dropped_count 并丢弃
    critical_tx: Arc<Mutex<Vec<mpsc::Sender<NexusEvent>>>>,
    /// Critical 通道累计丢弃事件数(P1-W2.1 优先级采样丢弃策略)
    ///
    /// WHY Arc<AtomicU64> 而非 Mutex<u64>:
    /// - 避免 §4.4 红线 1 "持锁跨 await"(AtomicU64 无锁,store/load 不跨 await)
    /// - 单调递增,不重置(运维观察累计丢弃趋势)
    /// - Relaxed 内存序:丢弃计数为统计指标,非控制流信号,无需强一致性
    critical_dropped_count: Arc<AtomicU64>,
    /// Broadcast 通道 Lagged 丢弃的总事件数(背压监控,A3 容量监控)
    ///
    /// WHY Arc<AtomicU64> 而非 Mutex<u64>:
    /// - publish 路径为热路径,不能有锁竞争(AtomicU64 无锁,~1ns vs Mutex ~25ns)
    /// - 需在 EventReceiver 的 recv 路径递增(跨 struct 共享),Arc 提供共享所有权
    /// - 单调递增,Relaxed 内存序足够(统计指标,非控制流信号)
    /// - 接收端发现 Lagged 时递增,记录被丢弃的事件数量(非 Lagged 次数)
    lagged_count: Arc<AtomicU64>,
    /// 背压告警触发次数(背压监控,A3 容量监控)
    ///
    /// WHY Arc<AtomicU64> 而非 AtomicU64:
    /// - publish 热路径不能有锁竞争(同 lagged_count 理由)
    /// - 仅在 publish 端使用,但 EventBus 派生 Clone,AtomicU64 不实现 Clone
    /// - 使用 Arc 共享使 Clone 派生正常工作(所有副本共享同一计数器)
    /// - 当 sender.len() > capacity * 3/4 时递增,配合 tracing::warn! 记录告警
    backpressure_warning_count: Arc<AtomicU64>,
    /// 累计发布事件总数(§16.5 L1 吞吐量指标,Phase 10 Wave 6)
    ///
    /// WHY:审计发现规范要求"Event Bus 吞吐量"无实现。publish/publish_blocking
    /// 入口递增,监控方周期拉取计算速率(真实采集,非伪造指标)。
    published_total: Arc<AtomicU64>,
    /// CBF 信用流(P1-T11,手册 §8.5 / T-06 / v4.0 WI-08)
    ///
    /// WHY Arc<CreditFlow>:EventBus 派生 Clone,所有副本共享同一信用池
    /// (与 lagged_count 等计数器同构);publish 热路径 try_acquire 为
    /// 无锁 CAS(~1ns,见 credit_flow.rs 选型说明)。
    ///
    /// # 方案 A(评审 Issue 2 修复:信用扣减仅在分片路径发生)
    /// 信用流是**分片背压载体**:仅「分片启用 + Unordered」事件先
    /// `try_acquire(1)`;失败(信用耗尽)不丢弃 —— 回退既有 broadcast 语义
    /// (broadcast 自身有 Lagged 保护 + SlowConsumerDropped 告警),累计
    /// `credit_shed_total`。归还由 worker 汇入时 `release_many` 自动完成
    /// (ADR-125 批提交;无分片时无扣减,不存在无人归还)。Critical 豁免
    /// (红线:Critical 背压 = 死锁源,推演 9);OrderSensitive 直投单流
    /// 不扣减(无人归还,扣减即泄漏)。
    credit_flow: Arc<CreditFlow>,
    /// 分片路径信用耗尽回退 broadcast 的累计次数(P1-T11 观测指标;
    /// 方案 A 下语义 = 分片路径的 shed:片满 + 无信用回退 broadcast)
    ///
    /// WHY Arc<AtomicU64> 而非 Mutex<u64>:publish 热路径不能有锁竞争
    /// (同 lagged_count 理由);Relaxed 内存序足够(统计指标,非控制流信号)。
    credit_shed_total: Arc<AtomicU64>,
    /// 分片总线(P1-T12 灰度,默认未启用 = 与 v2.27.1 行为完全一致)
    ///
    /// WHY Arc<Mutex<Option<Arc<ShardedEventBus>>>>:
    /// - 灰度开关语义:默认 `None`(不分片),`enable_sharding` 首次调用时
    ///   初始化分片总线并 spawn worker;
    /// - `Mutex` 仅保护初始化(发布路径不触碰 —— 见 `shard_enabled` 快速路径);
    /// - `Arc` 使 EventBus 保留 Clone 派生(所有 Clone 副本共享同一分片总线);
    /// - 内部再包一层 Arc:worker 任务持有分片总线所有权(独立于 EventBus 生命周期)。
    shard_bus: Arc<Mutex<Option<Arc<ShardedEventBus>>>>,
    /// 分片启用标志(快速路径,灰度默认 false)
    ///
    /// WHY Arc<AtomicBool>:发布热路径只读此标志(一次原子 load ~1ns),
    /// 避免默认关闭时触碰 Mutex(零回归);Arc 使 Clone 副本共享同一标志
    /// (与 shard_bus 共享语义一致 —— enable_sharding 后所有 Clone 均分片)。
    shard_enabled: Arc<AtomicBool>,
    /// Critical 事件发布计数(影子双跑前哨 `critical_total`)
    ///
    /// WHY Arc<AtomicU64>:EventBus 派生 Clone,所有副本共享同一计数器;
    /// 无条件统计(分片启用与否均递增),供 T13 前哨观测 Critical 车道流量。
    critical_total: Arc<AtomicU64>,
}

impl EventBus {
    /// 创建事件总线,使用默认容量(1024),不启用日志埋点
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    /// 创建事件总线,指定通道容量,不启用日志埋点
    pub fn with_capacity(capacity: usize) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self {
            sender,
            capacity,
            logger: None,
            critical_tx: Arc::new(Mutex::new(Vec::new())),
            critical_dropped_count: Arc::new(AtomicU64::new(0)),
            lagged_count: Arc::new(AtomicU64::new(0)),
            backpressure_warning_count: Arc::new(AtomicU64::new(0)),
            published_total: Arc::new(AtomicU64::new(0)),
            // P1-T11:默认信用池 256(手册 §8.5 初始值)
            credit_flow: Arc::new(CreditFlow::new()),
            credit_shed_total: Arc::new(AtomicU64::new(0)),
            // P1-T12 分片灰度:默认不启用(零回归,与 v2.27.1 行为完全一致)
            shard_bus: Arc::new(Mutex::new(None)),
            shard_enabled: Arc::new(AtomicBool::new(false)),
            critical_total: Arc::new(AtomicU64::new(0)),
        }
    }

    /// 创建事件总线,指定通道容量并启用日志埋点
    ///
    /// `logger` 会被包装在 Arc 中,Clone 时共享同一计数器。
    pub fn with_logger(capacity: usize, logger: BusLogger) -> Self {
        let (sender, _) = broadcast::channel(capacity);
        Self {
            sender,
            capacity,
            logger: Some(Arc::new(logger)),
            critical_tx: Arc::new(Mutex::new(Vec::new())),
            critical_dropped_count: Arc::new(AtomicU64::new(0)),
            lagged_count: Arc::new(AtomicU64::new(0)),
            backpressure_warning_count: Arc::new(AtomicU64::new(0)),
            published_total: Arc::new(AtomicU64::new(0)),
            // P1-T11:与 with_capacity 一致的信用池初始化
            credit_flow: Arc::new(CreditFlow::new()),
            credit_shed_total: Arc::new(AtomicU64::new(0)),
            // P1-T12 分片灰度:默认不启用(零回归)
            shard_bus: Arc::new(Mutex::new(None)),
            shard_enabled: Arc::new(AtomicBool::new(false)),
            critical_total: Arc::new(AtomicU64::new(0)),
        }
    }

    // ============================================================
    // P1-T12 分片灰度开关 — 默认不启用,零回归
    // ============================================================

    /// 启用分片(灰度开关,非 feature flag;手册 §8.5 / v4.0 WI-15)
    ///
    /// 启用后:非 Critical 且非顺序敏感的 `Unordered` 事件按 FNV-1a 哈希
    /// 入 64 片之一,worker 攒批汇入既有 broadcast(订阅者 API 零变化);
    /// `Critical` 与 `OrderSensitive` 事件恒走既有单流(红线)。
    ///
    /// # 默认不启用(零回归)
    /// `EventBus::new()` 行为与 v2.27.1 完全一致(不调用本方法即不分片);
    /// 全量既有测试零回归是本任务最终门禁。
    ///
    /// # 灰度安全
    /// - 无 tokio runtime 上下文(如纯同步测试线程)时返回
    ///   [`ShardingRequiresRuntime`](crate::error::EventBusError::ShardingRequiresRuntime),
    ///   调用方 `let _ =` 忽略即自动降级回单流(分片失败零风险);
    /// - 重复调用返回 [`ShardingAlreadyEnabled`](crate::error::EventBusError::ShardingAlreadyEnabled)
    ///   (幂等保护;多个 crate 各自调用时仅首个生效,其余忽略即可)。
    ///
    /// # 参数
    /// - `n_shards`:分片数(建议 [`crate::shard::DEFAULT_SHARD_COUNT`]=64;0 时回退 1 片,防取模除零)
    ///
    /// # 返回
    /// `Ok(())` 启用成功;`Err(ShardingAlreadyEnabled)` 已启用;
    /// `Err(ShardingRequiresRuntime)` 无 runtime(调用方应忽略降级)。
    pub fn enable_sharding(&self, n_shards: usize) -> Result<(), EventBusError> {
        // 幂等保护(快速路径):已启用则拒绝重复初始化。锁内 guard 检查是权威判定,
        // 本 Relaxed load 仅是避免无谓取锁的快速路径 —— 并发交错下由锁内兜底
        // (调用方 let _ = 忽略 Err 即安全降级)
        if self.shard_enabled.load(Ordering::Relaxed) {
            return Err(EventBusError::ShardingAlreadyEnabled);
        }
        // 灰度安全:worker 是 tokio 异步任务,必须持有 runtime 上下文才能 spawn;
        // 无 runtime(同步线程)时返回 Err,调用方忽略即降级回单流(零风险)
        let handle = tokio::runtime::Handle::try_current()
            .map_err(|_| EventBusError::ShardingRequiresRuntime)?;
        // 初始化分片总线(Mutex 仅保护初始化;发布路径不触碰此锁 —— 见 shard_enabled
        // 快速路径。poison 恢复:持锁线程 panic 后数据仍有效,继续执行)
        let mut guard = match self.shard_bus.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        if guard.is_some() {
            // 并发竞态兜底:另一线程已初始化(与 shard_enabled 幂等检查互为补充,
            // 任何交错下不重复 spawn worker)
            return Err(EventBusError::ShardingAlreadyEnabled);
        }
        // 构造分片总线并 spawn 每片 worker(worker 持有 broadcast sender + 信用流,
        // 汇入后按 ADR-125 批提交归还信用 —— 信用流自动平衡闭环)
        //
        // WHY 在锁内 spawn(评审 Issue 1 修复):锁内 guard.is_some() 检查之后才
        // spawn,任何并发交错下至多一个线程进入 spawn 区 —— 后到者在 spawn 之前
        // 就已返回 Err,绝不产生「持有未注册总线的空转 worker」。若在锁外 spawn,
        // 后到者会先 spawn 完 64 个 worker 才发现冲突,造成 64 个 worker 永久
        // 空转(队列恒空,仅持有 sender/信用流引用,资源泄漏)。
        let sb = Arc::new(ShardedEventBus::new(n_shards, SHARD_CAPACITY));
        sb.spawn_workers(self.sender.clone(), Arc::clone(&self.credit_flow), &handle);
        *guard = Some(sb);
        drop(guard);
        // Release 内存序:shard_bus 写入完成后再发布启用标志,publish 侧
        // Relaxed/Acquire 读取时必然看到完整的分片总线(数据竞争安全)
        self.shard_enabled.store(true, Ordering::Release);
        Ok(())
    }

    /// 查询分片是否已启用(灰度开关状态)
    ///
    /// # 返回
    /// `true` 表示 `enable_sharding` 已成功调用(Unordered 事件走分片扇出)。
    #[must_use = "分片启用状态是灰度观测项,忽略返回值无意义"]
    pub fn sharding_enabled(&self) -> bool {
        self.shard_enabled.load(Ordering::Relaxed)
    }

    /// 影子双跑前哨统计(P1-T12;T13 漏发率=0 硬门禁的采集输入)
    ///
    /// 漏发率口径:`sharded_total`(入分片数)vs `merged_total`(worker 汇入
    /// broadcast 数),正常场景两者相等 = 漏发率 0。`shed_total` 复用 T11
    /// `credit_shed_total`(分片满 + 无信用回退 broadcast 的次数,事件不丢弃)。
    ///
    /// # 返回
    /// [`ShadowStats`]:published_total / sharded_total / merged_total /
    /// shed_total / critical_total 五元组(Relaxed 观测,单调累计)。
    #[must_use = "前哨统计是 T13 双跑门禁输入,忽略返回值无意义"]
    pub fn shadow_stats(&self) -> ShadowStats {
        let (sharded_total, merged_total) = match self.shard_bus.lock() {
            Ok(guard) => match guard.as_ref() {
                Some(sb) => (sb.sharded_total(), sb.merged_total()),
                None => (0, 0),
            },
            // poison 恢复:统计是观测面,不因锁异常中断前哨采集
            Err(poisoned) => match poisoned.into_inner().as_ref() {
                Some(sb) => (sb.sharded_total(), sb.merged_total()),
                None => (0, 0),
            },
        };
        ShadowStats {
            published_total: self.published_total.load(Ordering::Relaxed),
            sharded_total,
            merged_total,
            shed_total: self.credit_shed_total.load(Ordering::Relaxed),
            critical_total: self.critical_total.load(Ordering::Relaxed),
        }
    }

    /// 尝试将 Unordered 事件入分片 — CBF 信用流裁决背压(手册 §8.5)
    ///
    /// # 返回
    /// - `Ok(())`:事件已入分片(worker 将汇入 broadcast,本事件发布完成);
    /// - `Err(event)`:未入片(片满且无信用/重试失败),调用方回退既有
    ///   broadcast 路径直接投递 —— **事件不丢弃,漏发率恒 0**(前哨硬门禁前置)。
    ///
    /// # 信用账目(不变量:事件信用守恒,方案 A)
    /// - 事件的 1 信用由 publish 入口在入片前扣减(仅分片启用 + Unordered),
    ///   入片成功后由 worker 汇入时 `release_many(batch_len)` 归还
    ///   (ADR-125 批提交);
    /// - 片满重试借用的 1 信用**立即归还**(无论重试成败),仅作为裁决信号,
    ///   不改变事件信用账目 —— 避免重试事件被 worker 重复归还造成泄漏;
    /// - 回退 broadcast 时调用方归还入口扣的 1 信用(本方法返回 `Err` 后
    ///   事件不经分片、无 worker 归还,信用不可滞留 —— 见 publish 回退分支)。
    /// - shed 计数(credit_shed_total)仅在本方法回退点累计:片满 + 无信用,
    ///   语义 = 分片路径的 shed(回退 broadcast 次数,事件不丢弃)。
    // WHY allow(result_large_err):Err 携带事件所有权回退 broadcast（避免热路径 clone）,
    // 有意设计而非疏漏（P1-T12 接口契约,shard::try_push 同语义）
    #[allow(clippy::result_large_err)]
    fn try_shard_publish(&self, event: NexusEvent) -> Result<(), NexusEvent> {
        // 锁内 clone Arc,锁外操作(不跨 await,§4.4 红线 1 合规);
        // poison 恢复:数据仍有效,继续执行(与 critical_tx 锁同策略)
        let sb = match self.shard_bus.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        };
        let Some(sb) = sb else { return Err(event) };
        match sb.try_push(event) {
            Ok(()) => Ok(()),
            Err(event) => {
                // 片满 → CBF 信用流裁决:
                // - 有信用:重试入片一次(借用信用立即归还,不改变事件账目);
                // - 无信用:shed 计数(复用 T11 credit_shed_total 口径:
                //   「因背压未走最优通道,回退既有 broadcast」)并回退。
                if self.credit_flow.try_acquire(1).is_ok() {
                    let result = sb.try_push(event);
                    self.credit_flow.release(1);
                    result
                } else {
                    self.credit_shed_total.fetch_add(1, Ordering::Relaxed);
                    Err(event)
                }
            }
        }
    }

    /// 为已有总线设置日志记录器(仅在未设置时生效)
    ///
    /// 返回 self 以便链式调用。
    /// 若已设置 logger,此调用无效果(保留第一个 logger)。
    pub fn set_logger(&mut self, logger: BusLogger) {
        if self.logger.is_none() {
            self.logger = Some(Arc::new(logger));
        }
    }

    /// 获取日志记录器的引用(若已设置)
    pub fn logger(&self) -> Option<&BusLogger> {
        self.logger.as_deref()
    }

    /// 发布事件到所有订阅者
    ///
    /// 若无订阅者,事件被丢弃但不视为错误(返回 Ok(()))。
    /// 慢消费者导致的丢弃由接收端的 `recv()` 以 `SlowConsumerDropped` 错误暴露。
    ///
    /// WHY async 签名:当前内部为同步 send,但保留 async 以保证 API 稳定性 —
    /// 未来引入跨进程投递(MCP Mesh)或异步序列化时无需破坏调用方。
    ///
    /// # SubTask 17.2:Critical 事件无订阅者告警
    /// 当 `subscriber_count == 0` 且事件为 Critical 级时,记录 `warn` 日志。
    /// WHY:CheckpointSaved/ConsensusReached 等关键事件丢失会导致系统状态不一致
    /// (如 Quest 无法恢复),无订阅者时必须告警。
    /// Normal 级事件保持静默丢弃,避免日志噪声。
    ///
    /// # §6.2 红线双通道(2026-06-29)
    /// Critical 安全/治理告警事件(is_critical_mpsc_event 清单,当前 13 类)
    /// 额外走 mpsc 旁路通道,确保在 broadcast Lagged 场景下
    /// 仍能被 `subscribe_critical_events` 订阅者接收。旁路通道未初始化时
    /// (无 Critical 订阅者)仅走 broadcast,并发出 warn 告警(C3,2026-09-04)。
    ///
    /// # P1-T12 分片路由(灰度,默认关闭)
    /// 启用分片后,`Unordered` 车道事件入 64 片之一(worker 汇入既有 broadcast,
    /// 订阅者 API 零变化);`Critical`/`OrderSensitive` 恒走本方法原单流路径
    /// (红线:Critical 永不进分片,顺序敏感保序)。
    #[allow(clippy::unused_async)]
    pub async fn publish(&self, event: NexusEvent) -> Result<(), EventBusError> {
        // 遮蔽为可变:P1-T12 分片回退分支需重新赋值事件所有权
        // (Err(ev) => event = ev),保证回退事件继续走既有单流路径,
        // 事件永不因分片而丢失(漏发率恒 0)。
        let mut event = event;
        // §16.5 吞吐量计数(Phase 10 Wave 6):发布入口递增
        self.published_total.fetch_add(1, Ordering::Relaxed);
        // P1-T12 前哨:Critical 车道计数(无条件统计 —— 分片启用与否均递增,
        // 供 T13 影子双跑观测 Critical 车道流量;Critical 永不进分片)
        if event.severity() == EventSeverity::Critical {
            self.critical_total.fetch_add(1, Ordering::Relaxed);
        }
        // P1-T11 CBF 信用流接入 —— 方案 A(评审 Issue 2 修复):信用扣减仅在
        // 「分片启用 + Unordered」路径发生(信用流仅作分片背压载体):
        // - 未启用分片:不扣信用(broadcast 路径本有 Lagged 保护,无需信用);
        // - 启用分片 + OrderSensitive/Critical:不扣信用(直投单流无人归还,
        //   扣减即泄漏;Critical 另有豁免红线:Critical 背压 = 死锁源,推演 9);
        // - 启用分片 + Unordered:try_acquire(1) 扣信用(扣减失败静默 ——
        //   池空时事件仍可入片,信用仅作片满重试的裁决信号);
        // - shed 计数统一由 try_shard_publish 回退点完成(单一计数,防双重)。
        let mut credit_deducted = false;
        if event.severity() != EventSeverity::Critical
            && self.shard_enabled.load(Ordering::Relaxed)
            && matches!(event_lane(&event), Lane::Unordered)
        {
            credit_deducted = self.credit_flow.try_acquire(1).is_ok();
        }
        // P1-T12 分片路由(灰度开关;快速路径:一次 AtomicBool load ~1ns,
        // 默认关闭时零开销 —— 与 v2.27.1 行为完全一致)
        // - `Unordered` → 入分片(worker 攒批汇入 broadcast,订阅者 API 零变化);
        // - `Critical`/`OrderSensitive` → 走原单流(红线:Critical 永不进分片;
        //   顺序敏感通道保持单流,不分片,E8-4)。
        // WHY log_publish 不覆盖分片路径:分片是发布端并行化灰度增强,其可观测性
        // 由 shadow_stats()/bus_shard_depth 指标承载;log_publish 埋点保留在单流
        // 路径(Critical/OrderSensitive/回退事件),避免双记与热路径额外采样。
        if self.shard_enabled.load(Ordering::Relaxed)
            && matches!(event_lane(&event), Lane::Unordered)
        {
            match self.try_shard_publish(event) {
                Ok(()) => return Ok(()),
                // 片满且信用耗尽回退:重新赋值事件所有权,继续走既有
                // broadcast 单流路径 —— 事件永不因分片而丢失(漏发率恒 0)。
                // 同时归还入口扣的 1 信用(事件不经分片、无 worker 归还,
                // 信用不可滞留 —— 否则「片满回退」成为无人归还的泄漏路径)。
                Err(ev) => {
                    if credit_deducted {
                        self.credit_flow.release(1);
                    }
                    event = ev;
                }
            }
        }
        // WHY 在方法入口测量 start:log_publish 在 send 之前调用(event 所有权
        // 尚未 move),elapsed 主要覆盖 receiver_count() 调用(接近零),
        // 但保留测量点以便未来将 log/指标采集移到 send 之后时仍准确。
        let start = Instant::now();
        let subscriber_count = self.sender.receiver_count();

        // 记录发布日志(若已启用日志埋点)
        if let Some(logger) = &self.logger {
            logger.log_publish(&event, subscriber_count, start.elapsed());
        }

        // SubTask 17.2:Critical 事件无订阅者告警
        // WHY:关键事件(CheckpointSaved/ConsensusReached/SlowConsumerDropped)丢失
        // 会导致系统状态不一致,无订阅者时必须告警;Normal 级静默丢弃避免日志噪声
        if subscriber_count == 0 && event.severity() == EventSeverity::Critical {
            warn!(
                event_type = event.type_name(),
                "Critical 事件无订阅者,事件将被丢弃"
            );
        }

        // §6.2 红线双通道:Critical 安全/治理告警事件(is_critical_mpsc_event 清单)额外走 mpsc 旁路
        // WHY 先 mpsc 后 broadcast:mpsc UnboundedSender::send 不会阻塞,
        // 先投递 mpsc 确保 Critical 订阅者必收;broadcast 仍走以保证向后兼容
        if is_critical_mpsc_event(&event) {
            self.send_critical_mpsc(&event);
        }

        // A3 背压监控:发送前检查 broadcast 缓冲区占用率
        // WHY sender.len() 是 AtomicUsize load,开销极低(~1ns),不影响 publish 吞吐
        // 阈值 3/4:提前告警,留给运维反应缓冲;满容量时才告警已太晚(事件即将被丢弃)
        let queued = self.sender.len();
        let threshold = self.capacity * 3 / 4;
        if queued > threshold {
            self.backpressure_warning_count
                .fetch_add(1, Ordering::Relaxed);
            warn!(
                queued,
                threshold,
                capacity = self.capacity,
                "broadcast 通道背压告警:缓冲区占用超过 75%"
            );
        }

        // broadcast::Sender::send 返回 Ok(receiver_count) 表示有多少接收者收到了消息。
        // 若 receiver_count < subscriber_count(发送前采样),说明有慢消费者 lag
        // (其内部缓冲区已满,send 跳过了它)。Err(SendError) 表示无接收者。
        // WHY 零成本:send 返回值本身是 usize,仅需一次整数比较 + 原子 load,
        // 不影响 publish 热路径性能(~1ns 额外开销)。
        let expected = subscriber_count;
        match self.sender.send(event) {
            Ok(receivers) if receivers < expected => {
                warn!(
                    expected_subscribers = expected,
                    actual_receivers = receivers,
                    lagged_count = expected - receivers,
                    "broadcast 发送者 lag 检测:部分订阅者缓冲区已满,事件被跳过"
                );
            }
            Ok(_) => {}  // 所有订阅者正常接收
            Err(_) => {} // 无接收者,已在上方 Critical 告警路径处理
        }
        Ok(())
    }

    /// 同步发布 — 用于不便 await 的场景(如 Drop 实现)
    ///
    /// WHY:某些回调场景无法 await,提供同步版本避免阻塞
    ///
    /// # SubTask 17.2:Critical 事件无订阅者告警
    /// 与 `publish` 保持一致:无订阅者且 Critical 级时记录 `warn` 日志。
    ///
    /// # §6.2 红线双通道(2026-06-29)
    /// 与 `publish` 一致:Critical 安全/治理告警事件(is_critical_mpsc_event 清单)额外走 mpsc 旁路通道。
    pub fn publish_blocking(&self, event: NexusEvent) -> Result<(), EventBusError> {
        // 遮蔽为可变:与 publish 一致,分片回退分支需重新赋值事件所有权
        let mut event = event;
        // §16.5 吞吐量计数(Phase 10 Wave 6):与 publish 一致
        self.published_total.fetch_add(1, Ordering::Relaxed);
        // P1-T12 前哨:Critical 车道计数(与 publish 一致,无条件统计)
        if event.severity() == EventSeverity::Critical {
            self.critical_total.fetch_add(1, Ordering::Relaxed);
        }
        // P1-T11 CBF 信用流:与 publish 完全一致(方案 A:仅「分片启用 +
        // Unordered」扣信用,扣减失败静默;OrderSensitive/Critical/未启用
        // 分片不扣 —— 见 publish 注释;shed 由 try_shard_publish 统一计数)
        let mut credit_deducted = false;
        if event.severity() != EventSeverity::Critical
            && self.shard_enabled.load(Ordering::Relaxed)
            && matches!(event_lane(&event), Lane::Unordered)
        {
            credit_deducted = self.credit_flow.try_acquire(1).is_ok();
        }
        // P1-T12 分片路由(与 publish 一致的灰度开关;默认关闭零开销)
        if self.shard_enabled.load(Ordering::Relaxed)
            && matches!(event_lane(&event), Lane::Unordered)
        {
            match self.try_shard_publish(event) {
                Ok(()) => return Ok(()),
                // 回退语义与 publish 完全一致:重新赋值所有权,继续走既有
                // broadcast 单流路径(事件不丢弃,漏发率恒 0);同时归还入口
                // 扣的 1 信用(回退路径无 worker 归还,信用不可滞留)。
                Err(ev) => {
                    if credit_deducted {
                        self.credit_flow.release(1);
                    }
                    event = ev;
                }
            }
        }
        // 与 publish 保持一致:入口测量耗时(Phase V Task V-8 指标采集)
        let start = Instant::now();
        let subscriber_count = self.sender.receiver_count();

        // 记录发布日志(若已启用日志埋点)
        if let Some(logger) = &self.logger {
            logger.log_publish(&event, subscriber_count, start.elapsed());
        }

        // SubTask 17.2:Critical 事件无订阅者告警(与 publish 保持一致)
        if subscriber_count == 0 && event.severity() == EventSeverity::Critical {
            warn!(
                event_type = event.type_name(),
                "Critical 事件无订阅者,事件将被丢弃(同步发布)"
            );
        }

        // §6.2 红线双通道:Critical 安全/治理告警事件(is_critical_mpsc_event 清单)额外走 mpsc 旁路
        if is_critical_mpsc_event(&event) {
            self.send_critical_mpsc(&event);
        }

        // A3 背压监控:与 publish 保持一致的发送前缓冲区占用检查
        let queued = self.sender.len();
        let threshold = self.capacity * 3 / 4;
        if queued > threshold {
            self.backpressure_warning_count
                .fetch_add(1, Ordering::Relaxed);
            warn!(
                queued,
                threshold,
                capacity = self.capacity,
                "broadcast 通道背压告警:缓冲区占用超过 75%(同步发布)"
            );
        }

        // 与 publish 保持一致:broadcast lag 检测(零成本,仅一次整数比较)
        let expected = subscriber_count;
        match self.sender.send(event) {
            Ok(receivers) if receivers < expected => {
                warn!(
                    expected_subscribers = expected,
                    actual_receivers = receivers,
                    lagged_count = expected - receivers,
                    "broadcast 发送者 lag 检测:部分订阅者缓冲区已满,事件被跳过(同步发布)"
                );
            }
            Ok(_) => {}
            Err(_) => {}
        }
        Ok(())
    }

    /// 批量发布多个事件到所有订阅者(摊销固定采样开销)
    ///
    /// WHY(M1-T1.3):`publish` 每次调用重复 `receiver_count()` / `sender.len()`
    /// 背压采样等固定开销;当调用方需连续发布 N 条事件(如议会 N 个 VoteCast)时,
    /// `publish_batch` 将这些采样摊销为一次,减少 N-1 次重复固定开销。
    ///
    /// # 语义一致性(与 `publish` 完全对齐)
    /// 逐条保留:Critical 无订阅者告警、Critical 安全事件 mpsc 旁路双通道(is_critical_mpsc_event 清单)、
    /// broadcast lag 检测。仅 `receiver_count` 与背压采样摊销为一次。
    ///
    /// # 空 Vec 早退
    /// `events` 为空时直接返回 Ok(())(避免无意义采样)。
    ///
    /// WHY async 签名:与 `publish` 一致,内部纯同步 send,保留 async 以稳定 API。
    #[allow(clippy::unused_async)]
    pub async fn publish_batch(&self, events: Vec<NexusEvent>) -> Result<(), EventBusError> {
        self.dispatch_batch(events);
        Ok(())
    }

    /// 同步批量发布 — [`publish_batch`](Self::publish_batch) 的同步版本
    ///
    /// 供不便 await 的场景使用(如 sync 方法内批量发布)。
    pub fn publish_batch_blocking(&self, events: Vec<NexusEvent>) -> Result<(), EventBusError> {
        self.dispatch_batch(events);
        Ok(())
    }

    /// 批量发布内部实现(async/blocking 共用,内部纯同步 send)
    ///
    /// 摊销点:`receiver_count()` 与 `sender.len()` 背压采样各仅一次;
    /// 逐条事件的 send / mpsc 旁路 / Critical 判定为固有开销,无法摊销。
    fn dispatch_batch(&self, events: Vec<NexusEvent>) {
        if events.is_empty() {
            return; // 空批次早退,避免无意义采样
        }
        let start = Instant::now();
        // 摊销点 1:receiver_count 仅采样一次(原子 load)
        let subscriber_count = self.sender.receiver_count();
        // 摊销点 2:背压检查仅一次(sender.len 原子 load + 阈值比较)
        let queued = self.sender.len();
        let threshold = self.capacity * 3 / 4;
        if queued > threshold {
            self.backpressure_warning_count
                .fetch_add(1, Ordering::Relaxed);
            warn!(
                queued,
                threshold,
                capacity = self.capacity,
                "broadcast 通道背压告警:缓冲区占用超过 75%(批量发布)"
            );
        }
        // 逐条发布:与 publish 语义一致,仅采样已摊销到循环外
        for event in events {
            // 遮蔽为可变:分片回退分支需重新赋值事件所有权(Err(ev) => event = ev),
            // 保证回退事件继续走本循环既有单流逻辑(事件不丢弃,漏发率恒 0)。
            let mut event = event;
            // P1-T11 CBF 信用流:逐条判定(与 publish 一致,方案 A:仅「分片启用 +
            // Unordered」扣信用,扣减失败静默;OrderSensitive/Critical/未启用
            // 分片不扣 —— 见 publish 注释;shed 由 try_shard_publish 统一计数)
            let mut credit_deducted = false;
            if event.severity() != EventSeverity::Critical
                && self.shard_enabled.load(Ordering::Relaxed)
                && matches!(event_lane(&event), Lane::Unordered)
            {
                credit_deducted = self.credit_flow.try_acquire(1).is_ok();
            }
            // P1-T12 分片路由(逐条判定,与 publish 一致):
            // Unordered 事件入分片,worker 汇入 broadcast(订阅者 API 零变化);
            // Critical/OrderSensitive 恒走本循环原单流(红线)
            // WHY 在信用处理之后路由:与 publish 信用账目完全一致 —— 入分片事件
            // 的 1 信用由本循环扣除(条件与下方路由一致:shard_enabled + Unordered),
            // worker 汇入时 release_many 归还(信用守恒不变量见 try_shard_publish;
            // 若先路由后扣信用,worker 会归还未扣过的信用,破坏守恒语义;若扣信用
            // 条件宽于路由条件,则 OrderSensitive/未入片事件扣而无人归还)
            if self.shard_enabled.load(Ordering::Relaxed)
                && matches!(event_lane(&event), Lane::Unordered)
            {
                match self.try_shard_publish(event) {
                    Ok(()) => continue,
                    // 回退语义与 publish 一致:重新赋值所有权,继续走既有单流路径
                    // (批量路径同样保证事件不丢弃,漏发率恒 0);同时归还入口扣的
                    // 1 信用(回退路径无 worker 归还,信用不可滞留)。
                    Err(ev) => {
                        if credit_deducted {
                            self.credit_flow.release(1);
                        }
                        event = ev;
                    }
                }
            }
            if let Some(logger) = &self.logger {
                logger.log_publish(&event, subscriber_count, start.elapsed());
            }
            if subscriber_count == 0 && event.severity() == EventSeverity::Critical {
                warn!(
                    event_type = event.type_name(),
                    "Critical 事件无订阅者,事件将被丢弃(批量发布)"
                );
            }
            // §6.2 红线双通道:Critical 安全事件(is_critical_mpsc_event 清单)额外走 mpsc 旁路(逐条判定,不破坏语义)
            if is_critical_mpsc_event(&event) {
                self.send_critical_mpsc(&event);
            }
            match self.sender.send(event) {
                Ok(receivers) if receivers < subscriber_count => {
                    warn!(
                        expected_subscribers = subscriber_count,
                        actual_receivers = receivers,
                        lagged_count = subscriber_count - receivers,
                        "broadcast 发送者 lag 检测:部分订阅者缓冲区已满,事件被跳过(批量发布)"
                    );
                }
                Ok(_) => {}
                Err(_) => {}
            }
        }
    }

    /// 显式发布 Critical 事件到双通道(broadcast + mpsc 旁路)
    ///
    /// 调用方明确知道事件为 Critical 时使用此方法,语义清晰。
    /// 内部行为与 [`publish`](Self::publish) 对 is_critical_mpsc_event 清单事件的处理一致,
    /// 但不依赖 `is_critical_mpsc_event` 判定,直接走 mpsc 旁路(适用于
    /// 调用方自定义的 Critical 事件,如未来扩展的 AsaIntervention Block 级)。
    ///
    /// WHY 提供 explicit API:与 `publish_critical_blocking` 配对,
    /// 供 async 上下文调用方使用(如 spawn_overflow_monitor 中的 async 任务)。
    #[allow(clippy::unused_async)]
    pub async fn publish_critical(&self, event: NexusEvent) -> Result<(), EventBusError> {
        // 先走 mpsc 旁路确保 Critical 订阅者必收,再走 broadcast 保持向后兼容
        self.send_critical_mpsc(&event);
        let _ = self.sender.send(event);
        Ok(())
    }

    /// 显式同步发布 Critical 事件到双通道(broadcast + mpsc 旁路)
    ///
    /// 同步版本,供不便 await 的场景使用(如 sync 方法内调用)。
    /// 内部行为与 [`publish_blocking`](Self::publish_blocking) 对 is_critical_mpsc_event
    /// 清单事件的处理一致,但不依赖 `is_critical_mpsc_event` 判定。
    pub fn publish_critical_blocking(&self, event: NexusEvent) -> Result<(), EventBusError> {
        self.send_critical_mpsc(&event);
        let _ = self.sender.send(event);
        Ok(())
    }

    // ============================================================
    // P2-W7.1.1:膜感知发布(选择性渗透集成)
    // ============================================================

    /// 膜感知异步发布 — 通过 MembraneFilter 决策事件是否穿膜入内环
    ///
    /// 将膜过滤集成到发布路径(spec.md L423 "膜深化:选择性渗透"):
    /// - `PassToCore`:走正常发布路径(broadcast + Critical mpsc 旁路)
    /// - `LocalConsume`:不发布到任何 channel,在膜边界直接消化
    ///
    /// 返回 [`PermeationDecision`] 让调用方知道事件是否穿膜,可用于指标统计
    /// (如统计被膜拒绝的事件数、按 EventCategory 分类等)。
    ///
    /// # 零 BREAKING(spec.md L421)
    /// 既有 [`publish`](Self::publish) / [`publish_critical`](Self::publish_critical)
    /// 签名与行为不变。调用方显式选择 `publish_membrane` 才启用膜过滤。
    /// `publish_critical()` 始终穿膜(Critical 事件在膜分类中恒为 `PassToCore`)。
    ///
    /// # 背压语义(两层协同)
    /// - 第一层(膜):InnerLoad::Critical 档拒绝非 Critical 事件穿膜,
    ///   减少 channel 投递量(入口过滤)
    /// - 第二层(channel):P1-W2.1 有界 mpsc(4096 容量)防止 Critical 通道 OOM
    ///   (出口背压);容量满时优先级采样丢弃并递增 `critical_dropped_count`
    ///
    /// # 使用示例
    /// ```
    /// use event_bus::{EventBus, EventMetadata, NexusEvent};
    /// use event_bus::membrane::{InnerLoad, MembraneFilter};
    ///
    /// let bus = EventBus::new();
    /// let membrane = MembraneFilter::with_load(InnerLoad::High);
    ///
    /// let cache_hit = NexusEvent::CacheHit {
    ///     metadata: EventMetadata::new("scc-cache"),
    ///     cache_key: "k1".into(),
    /// };
    /// // 同步版本:不需 tokio runtime
    /// let decision = bus.publish_membrane_blocking(cache_hit, &membrane).unwrap();
    /// assert!(decision.is_local_consume()); // CacheLocal 在 High 档被本地消化
    /// ```
    pub async fn publish_membrane(
        &self,
        event: NexusEvent,
        membrane: &MembraneFilter,
    ) -> Result<PermeationDecision, EventBusError> {
        let decision = membrane.decide(&event);
        if decision.is_local_consume() {
            // 外环本地消化:不发布到任何 channel
            // WHY 不记录 warn:LocalConsume 是预期行为(如 CacheHit/ReadMetric),
            // 非异常丢弃;调用方可通过返回值统计
            return Ok(decision);
        }
        // PassToCore:走正常发布路径(broadcast + Critical mpsc 旁路)
        self.publish(event).await?;
        Ok(decision)
    }

    /// 膜感知同步发布 — [`publish_membrane`](Self::publish_membrane) 的同步版本
    ///
    /// 供不便 await 的场景使用(如 Drop 实现、sync 方法内调用)。
    /// 语义与异步版本完全一致:膜决策 → PassToCore 走 publish_blocking /
    /// LocalConsume 跳过。
    pub fn publish_membrane_blocking(
        &self,
        event: NexusEvent,
        membrane: &MembraneFilter,
    ) -> Result<PermeationDecision, EventBusError> {
        let decision = membrane.decide(&event);
        if decision.is_local_consume() {
            return Ok(decision);
        }
        self.publish_blocking(event)?;
        Ok(decision)
    }

    /// 订阅 Critical 事件 mpsc 旁路通道
    ///
    /// §6.2 红线:Critical 安全事件(SkepticVeto/RedTeamAudit/AsaIntervention/
    /// BudgetExceeded)必须用 mpsc channel 确保送达。此方法返回 mpsc Receiver,
    /// 订阅者通过它接收 Critical 事件,即使在 broadcast Lagged 场景下也不会丢失。
    ///
    /// # P1-W2.1 有界化改造(D3 修复,2026-07-23)
    /// 返回类型从 `mpsc::UnboundedReceiver` 改为有界 `mpsc::Receiver`(容量
    /// [`CRITICAL_CHANNEL_CAPACITY`] = 4096)。提供硬上限防止慢消费者导致 OOM。
    /// 容量满时 `publish_critical` 内部 `try_send` 失败,按优先级采样丢弃并
    /// 递增 `critical_dropped_count`(见 [`critical_dropped_count`](Self::critical_dropped_count))。
    /// 调用方仅需 `.recv().await`,此方法在 `mpsc::Receiver` 与
    /// `mpsc::UnboundedReceiver` 上签名兼容,零改动。
    ///
    /// # fan-out 多订阅者
    /// 每次调用创建独立有界 mpsc channel(容量 4096),Sender 入 `Vec` 内部状态,
    /// Receiver 返回。后续发布的 Critical 事件会向 `Vec` 中所有 Sender 投递
    /// (fan-out 广播)。receiver drop 后,对应 Sender 的 `try_send` 返回
    /// `Closed` 错误,下次发送时被清理。
    ///
    /// # 调用时机(§4.4 反模式 3)
    /// 必须在 `tokio::spawn()` **之前同步调用**此方法,确保不会错过后续发布的
    /// Critical 事件。在 spawn 的 async block 内调用可能导致事件静默丢失。
    pub fn subscribe_critical_events(&self) -> mpsc::Receiver<NexusEvent> {
        let (tx, rx) = mpsc::channel(CRITICAL_CHANNEL_CAPACITY);
        // WHY unwrap_or_else: 中毒锁降级访问内部数据而非 panic。
        // EventBus 是核心组件,前任持有者 panic 导致 poison 后,
        // 继续抛 panic 会中断所有事件发布,降级为访问中毒数据更稳健(§4.1 红线)。
        // 与 csn-substitutor/substitutor.rs 的 register_lock 处理方式保持一致。
        let mut guard = self.critical_tx.lock().unwrap_or_else(|e| e.into_inner());
        guard.push(tx);
        rx
    }

    /// 获取 Critical 旁路通道容量(P1-W2.1 新增)
    ///
    /// 返回固定值 [`CRITICAL_CHANNEL_CAPACITY`] = 4096,用于测试断言与
    /// 运维监控。容量为编译时常量,不随实例变化。
    pub fn critical_channel_capacity(&self) -> usize {
        CRITICAL_CHANNEL_CAPACITY
    }

    /// 获取 Critical 通道累计丢弃事件数(P1-W2.1 新增)
    ///
    /// 返回因容量满(Critical 旁路通道 4096 全满)而被优先级采样丢弃的
    /// Critical 事件总数。单调递增,不重置。供 efficiency-monitor 拉取并
    /// 发布 `EfficiencyAlertTriggered` 告警,同时供 TUI 显示丢弃计数
    /// (spec.md L188:丢弃事件计入 CriticalEventDropped 指标 + TUI 告警)。
    ///
    /// WHY Relaxed 内存序:丢弃计数为统计指标,非控制流信号,无需强一致性。
    /// 即使读到稍旧的值,仅影响告警时机的毫秒级延迟,不影响系统正确性。
    pub fn critical_dropped_count(&self) -> u64 {
        self.critical_dropped_count.load(Ordering::Relaxed)
    }

    /// 向 mpsc 旁路通道投递 Critical 事件(内部辅助方法)
    ///
    /// fan-out 投递:遍历 `Vec<mpsc::Sender>`,向每个 Sender 投递事件 clone。
    /// 使用 `try_send` 替代 `send().await`(本方法为同步,不可 await),
    /// 容量满时丢弃事件并递增 `critical_dropped_count`(优先级采样丢弃策略)。
    ///
    /// # 优先级采样丢弃策略(P1-W2.1.2)
    /// - `try_send` 返回 `Ok(())`:投递成功,保留 Sender
    /// - `try_send` 返回 `Err(Full)`:容量满,丢弃本事件 + 递增计数,
    ///   保留 Sender(下次发送可能成功,避免误清理可用 Sender)
    /// - `try_send` 返回 `Err(Closed)`:receiver 已 drop,移除 Sender
    ///
    /// WHY 保留 Full 状态的 Sender:容量满是临时状态(消费者可能很快赶上),
    /// 不应因临时满载就移除订阅者。仅 Closed 才真正移除。
    /// WHY retain 而非 filter:retain 原地修改,O(n) 一次遍历完成发送 + 清理。
    ///
    /// # P1-W4.1 tracing 贯穿观测
    /// span 携带 `event_type` / `severity` / `event_id` 三个字段,供 efficiency-monitor
    /// 关联丢弃事件与原始 Critical 事件。`event_id`(UUIDv7)是跨进程因果追踪的唯一
    /// 标识,即使事件被丢弃也能通过 event_id 在审计日志中定位原始发布点。
    /// `event` 含完整载荷(可能含敏感数据),故 `skip(self, event)` 仅记录类型、
    /// 级别与 ID,避免泄露事件内容。丢弃日志在 retain 之后单次发出(避免在闭包内
    /// 多次记录),用 `dropped_count` 字段表示本次发送导致的累计丢弃增量,而非全局累计值。
    #[tracing::instrument(
        skip(self, event),
        fields(
            event_type = %event.type_name(),
            severity = ?event.severity(),
            event_id = %event.metadata().event_id
        )
    )]
    fn send_critical_mpsc(&self, event: &NexusEvent) {
        // WHY unwrap_or_else: 中毒锁降级访问而非 panic(见 subscribe_critical_events 注释)。
        let mut guard = self.critical_tx.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_empty() {
            // C3(2026-09-04): 旁路无订阅者时不再静默(F-A5-6 风险点 2)——
            // 此时 Critical 事件仅有 broadcast 单通道保障,订阅者一旦 Lagged
            // 事件即永久丢失且无任何痕迹。Critical 事件低频,warn 噪声可控;
            // span(instrument 宏)已携带 event_type/severity/event_id 供定位。
            tracing::warn!(
                critical_dropped_count = self.critical_dropped_count.load(Ordering::Relaxed),
                "Critical mpsc 旁路无订阅者,旁路投递跳过(仅 broadcast 单通道)"
            );
            return;
        }
        // P1-W4.1: 所有路径发出 debug 事件,确保 span 字段(event_type / severity / event_id)
        // 被 tracing-test 捕获。WHY debug 而非 info:Critical 事件投递是高频常态,
        // info 级会产生日志噪声;debug 级在生产环境默认被 env-filter 过滤,仅测试与调试时可见。
        // 此事件使 event_type / severity / event_id 字段进入日志,供 efficiency-monitor
        // 跨日志关联丢弃事件与原始 Critical 事件(即使事件被丢弃也能通过 event_id 定位)。
        tracing::debug!(
            subscriber_count = guard.len(),
            "Critical 事件投递到 mpsc 旁路通道"
        );
        // P1-W4.1: 记录 retain 前的累计丢弃数,用于计算本次发送导致的丢弃增量
        // WHY 在 retain 之前采样:retain 闭包内会 fetch_add 多次(每个满 Sender 一次),
        // 闭包后用差值得到本次发送的总丢弃数,单次 warn 记录避免日志噪声
        let dropped_before = self.critical_dropped_count.load(Ordering::Relaxed);
        // 优先级采样丢弃:遍历所有 Sender,try_send 失败时按错误类型处理
        // - Full:递增丢弃计数,保留 Sender(临时满载)
        // - Closed:移除 Sender(receiver 已 drop)
        guard.retain(|tx| match tx.try_send(event.clone()) {
            Ok(()) => true, // 投递成功,保留
            Err(mpsc::error::TrySendError::Full(_)) => {
                // 容量满:按优先级采样丢弃,递增计数,保留 Sender
                // WHY fetch_add 而非 store(load + 1):原子操作避免读改写竞态
                self.critical_dropped_count.fetch_add(1, Ordering::Relaxed);
                true // 保留 Sender(临时满载,下次可能成功)
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false, // receiver 已 drop,移除
        });
        // P1-W4.1: 丢弃增量结构化日志 — 在 retain 闭包外单次发出,避免多次 warn 噪声
        // WHY 用差值而非全局累计值:全局累计值是单调递增的运维指标(由 efficiency-monitor
        // 周期性采样),而 warn 日志应反映"本次发送导致多少事件被丢弃",差值更精确
        let dropped_count = self.critical_dropped_count.load(Ordering::Relaxed) - dropped_before;
        if dropped_count > 0 {
            tracing::warn!(dropped_count, "Critical 事件被丢弃(订阅者通道满)");
        }
    }

    /// 查询是否存在 Critical mpsc 旁路订阅者(C3 运维观测 API)
    ///
    /// WHY:发布时告警是事件驱动的事后观测(send_critical_mpsc 空订阅者分支),
    /// 本 API 提供主动查询通道,供组合根启动自检/运维巡检使用。
    ///
    /// # 惰性清理说明
    /// Receiver drop 后其 Sender 由下次 send_critical_mpsc 的 retain 移除,
    /// 故本查询反映"已建立且尚未被清理"的订阅数,与活跃订阅可能存在短暂偏差;
    /// 精确活性以订阅方自身存活状态为准。
    pub fn has_critical_subscribers(&self) -> bool {
        !self
            .critical_tx
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_empty()
    }

    /// 订阅事件流,返回新的接收者
    ///
    /// 每次调用创建独立接收者,从订阅时刻开始接收新事件(不回放历史)。
    /// 接收者会继承总线的日志记录器,用于记录接收端事件。
    pub fn subscribe(&self) -> EventReceiver {
        let subscriber_id = format!("sub-{}", uuid::Uuid::now_v7());
        let subscriber_count = self.sender.receiver_count() + 1; // +1 包含即将创建的

        // 记录订阅者连接日志(若已启用日志埋点)
        if let Some(logger) = &self.logger {
            logger.log_subscriber_connected(&subscriber_id, subscriber_count, self.capacity);
        }

        // WHY 复用 from_broadcast:与 subscribe_filtered 共享构造逻辑,
        // 避免两处分别拼装 EventReceiver 导致字段初始化不一致风险
        EventReceiver::from_broadcast(
            self.sender.subscribe(),
            subscriber_id,
            self.logger.clone(),
            self.lagged_count.clone(),
        )
    }

    /// 订阅指定 topic 集合的事件,返回 [`FilteredSubscriber`](crate::topic::FilteredSubscriber)
    ///
    /// 仅接收 topic 匹配的事件,不匹配的事件在 FilteredSubscriber 内部被消费丢弃。
    /// 既有 [`subscribe`](Self::subscribe) 保持全量广播,不受影响。
    ///
    /// # 调用时机(§4.4 反模式 3)
    /// 必须在 `tokio::spawn()` **之前同步调用**,确保不错过后续事件。
    /// 在 spawn 的 async block 内调用可能导致事件静默丢失。
    ///
    /// # 使用场景
    /// - TTG 仲裁层只需 Parliament + Budget 事件
    /// - N9 PrerequisiteChecker 只需 Routing 事件
    /// - 减少无关事件对消费者缓冲区的占用
    ///
    /// # 示例
    /// ```no_run
    /// use event_bus::{EventBus, EventTopic};
    /// use std::collections::HashSet;
    ///
    /// let bus = EventBus::new();
    /// let topics: HashSet<EventTopic> = [EventTopic::Routing].into_iter().collect();
    /// let mut rx = bus.subscribe_filtered(topics);
    /// ```
    pub fn subscribe_filtered(
        &self,
        topics: std::collections::HashSet<crate::topic::EventTopic>,
    ) -> crate::topic::FilteredSubscriber {
        let subscriber_id = format!("filtered-{}", uuid::Uuid::now_v7());
        let subscriber_count = self.sender.receiver_count() + 1; // +1 包含即将创建的

        // 记录订阅者连接日志(若已启用日志埋点)
        if let Some(logger) = &self.logger {
            logger.log_subscriber_connected(&subscriber_id, subscriber_count, self.capacity);
        }

        // 复用 subscribe() 的内部构造逻辑,仅外层包一层 FilteredSubscriber
        let receiver = EventReceiver::from_broadcast(
            self.sender.subscribe(),
            subscriber_id,
            self.logger.clone(),
            self.lagged_count.clone(),
        );
        crate::topic::FilteredSubscriber::new(receiver, topics)
    }

    /// 创建普通事件订阅构建器 — 强制 subscribe-then-spawn 顺序(P1-W4.2)
    ///
    /// 返回 [`SubscriberBuilder`](crate::subscriber::SubscriberBuilder)`<Unsubscribed>`,
    /// 调用方必须先 `.subscribe()` 再 `.spawn()`,TypeState 在编译期保证顺序。
    ///
    /// WHY: Week 6 SSRA 教训 — `bus.subscribe()` 必须在 `tokio::spawn()` 之前
    /// 同步调用,否则事件静默丢失。此 API 将人为纪律升级为 API 结构保证。
    ///
    /// # 示例
    /// ```
    /// use event_bus::EventBus;
    /// # async fn demo() -> Result<(), Box<dyn std::error::Error>> {
    /// let bus = EventBus::new();
    /// let handle = bus.subscriber()
    ///     .subscribe()           // 同步订阅
    ///     .spawn(|mut rx| async move {
    ///         rx.recv().await
    ///     });
    /// # handle.await?;
    /// # Ok(())
    /// # }
    /// ```
    #[must_use = "subscriber() 返回构建器,需调用 subscribe() 完成订阅"]
    pub fn subscriber(
        &self,
    ) -> crate::subscriber::SubscriberBuilder<'_, crate::subscriber::Unsubscribed> {
        crate::subscriber::SubscriberBuilder::new(self)
    }

    /// 创建 Critical 事件订阅构建器 — 强制 subscribe-then-spawn 顺序(P1-W4.2)
    ///
    /// 返回 [`CriticalSubscriberBuilder`](crate::subscriber::CriticalSubscriberBuilder)`<Unsubscribed>`,
    /// 用于订阅 §6.2 红线定义的 Critical 安全/治理告警事件(is_critical_mpsc_event
    /// 清单,当前 13 类)的 mpsc 旁路通道。
    ///
    /// 与 [`subscriber`](Self::subscriber) 区别:返回 `mpsc::Receiver` 而非 `EventReceiver`,
    /// 确保在 broadcast Lagged 场景下仍能收到 Critical 事件。
    #[must_use = "critical_subscriber() 返回构建器,需调用 subscribe() 完成订阅"]
    pub fn critical_subscriber(
        &self,
    ) -> crate::subscriber::CriticalSubscriberBuilder<'_, crate::subscriber::Unsubscribed> {
        crate::subscriber::CriticalSubscriberBuilder::new(self)
    }

    /// 获取当前订阅者数量
    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }

    /// 获取通道容量
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 获取背压监控统计(A3 容量监控)
    ///
    /// 返回 `(lagged_count, backpressure_warning_count)`:
    /// - `lagged_count`:broadcast 通道因慢消费者 Lagged 而丢弃的累计事件数
    ///   (接收端递增,反映实际丢失的事件数量)
    /// - `backpressure_warning_count`:缓冲区占用超过 75% 阈值的累计告警次数
    ///   (发送端递增,反映背压触发频率)
    ///
    /// 两个计数器均单调递增,不重置。供 efficiency-monitor 周期性拉取并
    /// 发布告警,同时供 TUI 显示背压趋势。
    ///
    /// WHY 返回元组而非 struct:轻量 API,两个 u64 语义清晰,无需额外类型定义。
    /// WHY Relaxed 内存序:统计指标,非控制流信号,毫秒级读取偏差可容忍。
    pub fn backpressure_stats(&self) -> (u64, u64) {
        let lagged = self.lagged_count.load(Ordering::Relaxed);
        let warnings = self.backpressure_warning_count.load(Ordering::Relaxed);
        (lagged, warnings)
    }

    /// 累计发布事件总数(§16.5 L1 吞吐量指标,Phase 10 Wave 6)
    ///
    /// 监控方周期拉取两次采样差值/间隔即得吞吐速率(真实采集)。
    pub fn published_total(&self) -> u64 {
        self.published_total.load(Ordering::Relaxed)
    }

    /// 信用流观测统计(P1-T11,手册 §8.5 观测面)
    ///
    /// 返回:
    /// - `available`:当前可用信用(256 初始,随 publish 扣减 / release 归还)
    /// - `shed_total`:信用耗尽回退 broadcast 的累计事件数
    ///   (**不是丢弃数** —— 回退后事件仍走 broadcast,仅失去信用流信号)
    /// - `high_wait_total`:高优事件进入等待窗口的累计次数
    ///
    /// WHY 单一方法返回 struct 而非三个 getter:观测方一次性拉取关联指标,
    /// 避免多次原子 load 间读到的状态不一致(available 与 shed 属于同一
    /// 信用生命周期)。
    pub fn credit_stats(&self) -> CreditStats {
        CreditStats {
            available: self.credit_flow.credit_available(),
            shed_total: self.credit_shed_total.load(Ordering::Relaxed),
            high_wait_total: self.credit_flow.high_wait_total(),
        }
    }

    /// 手动归还信用(P1-T11)—— 归还时机文档
    ///
    /// # 归还时机(设计约定)
    /// 订阅者**按消费速率批量归还**(ADR-125 批提交语义):例如每消费 N 个
    /// 事件调用一次 `release_credit(N)`,而不是逐事件归还。
    ///
    /// WHY 逐事件归还的缺点:每次归还都触发 `notify_waiters` 核外唤醒
    /// (挂起→唤醒的调度开销),高频归还会放大调度噪声;批提交将归还
    /// 频率降至与消费批次一致,唤醒成本可忽略。
    ///
    /// # 本任务不强制后台归还任务
    /// 归还由调用方显式触发(本方法 + [`credit_flow`](Self::credit_flow)
    /// 直接访问原语);T12 分片改造时接入后台自动归还(按消费速率授信)。
    ///
    /// # 不会膨胀
    /// 归还封顶到初始信用池(见 [`CreditFlow::release`]),过度归还不破坏
    /// 信用守恒。
    pub fn release_credit(&self, n: u64) {
        self.credit_flow.release(n);
    }

    /// 获取信用流原语引用(高级用途)
    ///
    /// 供调用方执行高优等待语义 [`CreditFlow::acquire_priority`](crate::credit_flow::CreditFlow::acquire_priority)
    /// (如 High 事件在信用不足时异步等待 ≤100ms 窗口)或直接观测
    /// [`CreditFlow::credit_available`]。
    pub fn credit_flow(&self) -> &CreditFlow {
        &self.credit_flow
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

/// §16.5 L1 吞吐量周期报告器(Phase 10 Wave 6)
///
/// 周期拉取 [`EventBus::published_total`] 差分计算事件/秒速率,发布
/// `BusThroughputReported` 观测面事件(真实采集,非伪造指标)。
///
/// 首 tick 立即返回先采样基线,避免窗口 0 除零;窗口长度由
/// `interval_secs` 决定(下限 1 秒,防止 0 间隔忙循环)。
pub fn spawn_throughput_reporter(bus: EventBus, interval_secs: u64) -> tokio::task::JoinHandle<()> {
    let window_secs = interval_secs.max(1);
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(window_secs));
        interval.tick().await; // 基线采样:跳过首个立即 tick
        let mut prev = bus.published_total();
        loop {
            interval.tick().await;
            let now = bus.published_total();
            let events_per_sec = (now.saturating_sub(prev)) as f64 / window_secs as f64;
            prev = now;
            let event = NexusEvent::BusThroughputReported {
                metadata: EventMetadata::new("event-bus"),
                published_total: now,
                events_per_sec,
                window_secs,
            };
            if let Err(e) = bus.publish_blocking(event) {
                warn!(error = %e, "BusThroughputReported 发布失败");
            }
        }
    })
}

/// 事件接收者 — 包装 broadcast::Receiver
///
/// 每个接收者独立维护读取位置,慢消费者会收到 Lagged 错误。
/// 持有总线日志记录器的引用,用于记录接收/超时/错误事件。
pub struct EventReceiver {
    inner: broadcast::Receiver<NexusEvent>,
    /// 订阅者唯一标识,用于日志关联
    subscriber_id: String,
    /// 日志记录器(与总线共享)
    logger: Option<Arc<BusLogger>>,
    /// Broadcast Lagged 丢弃事件计数器(与 EventBus 共享,A3 背压监控)
    ///
    /// WHY Arc<AtomicU64>:接收端发现 Lagged 时递增,与 EventBus 共享同一计数器,
    /// AtomicU64 避免 recv 路径锁竞争(热路径,同 publish 理由)。
    lagged_count: Arc<AtomicU64>,
}

impl EventReceiver {
    /// 内部构造函数(crate 内可见,用于 FilteredSubscriber 包装)
    ///
    /// WHY pub(crate):避免外部直接拼装 EventReceiver 绕过 EventBus 的订阅者
    /// 计数与日志埋点;同时允许 topic.rs 在同 crate 内构造 FilteredSubscriber
    /// 时复用 EventReceiver 的 recv/try_recv 能力。
    pub(crate) fn from_broadcast(
        inner: broadcast::Receiver<NexusEvent>,
        subscriber_id: String,
        logger: Option<Arc<BusLogger>>,
        lagged_count: Arc<AtomicU64>,
    ) -> Self {
        EventReceiver {
            inner,
            subscriber_id,
            logger,
            lagged_count,
        }
    }

    /// 接收下一个事件
    ///
    /// 错误处理:
    /// - `ChannelClosed`:所有 Sender 已 drop,流结束
    /// - `SlowConsumerDropped`:lag 超限,需决定重订阅或告警
    pub async fn recv(&mut self) -> Result<NexusEvent, EventBusError> {
        match self.inner.recv().await {
            Ok(event) => {
                if let Some(logger) = &self.logger {
                    logger.log_recv(&event);
                }
                Ok(event)
            }
            Err(e) => {
                let eb_err = EventBusError::from(e);
                if let Some(logger) = &self.logger {
                    match &eb_err {
                        EventBusError::ChannelClosed => {
                            logger.log_channel_closed(&self.subscriber_id);
                        }
                        EventBusError::SlowConsumerDropped {
                            subscriber_id: _,
                            lag,
                        } => {
                            logger.log_slow_consumer_dropped(&self.subscriber_id, *lag, *lag);
                        }
                        _ => {}
                    }
                }
                // A3 背压监控:递增 lagged_count(记录被丢弃的事件数量)
                // WHY 在接收端递增:broadcast::Sender::send 不返回 Lagged,
                // Lagged 仅在接收端 recv 时检测到(发送方无法感知单个接收者的 lag)
                if matches!(&eb_err, EventBusError::SlowConsumerDropped { .. }) {
                    if let EventBusError::SlowConsumerDropped { lag, .. } = &eb_err {
                        self.lagged_count.fetch_add(*lag, Ordering::Relaxed);
                    }
                }
                Err(eb_err)
            }
        }
    }

    /// 带超时的接收
    ///
    /// WHY:架构红线要求所有异步操作有超时处理,避免孤儿调用
    pub async fn recv_timeout(&mut self, timeout: Duration) -> Result<NexusEvent, EventBusError> {
        match tokio::time::timeout(timeout, self.inner.recv()).await {
            Ok(Ok(event)) => {
                if let Some(logger) = &self.logger {
                    logger.log_recv(&event);
                }
                Ok(event)
            }
            Ok(Err(e)) => {
                let eb_err = EventBusError::from(e);
                if let Some(logger) = &self.logger {
                    match &eb_err {
                        EventBusError::ChannelClosed => {
                            logger.log_channel_closed(&self.subscriber_id);
                        }
                        EventBusError::SlowConsumerDropped {
                            subscriber_id: _,
                            lag,
                        } => {
                            logger.log_slow_consumer_dropped(&self.subscriber_id, *lag, *lag);
                        }
                        _ => {}
                    }
                }
                // A3 背压监控:与 recv 保持一致,递增 lagged_count
                if let EventBusError::SlowConsumerDropped { lag, .. } = &eb_err {
                    self.lagged_count.fetch_add(*lag, Ordering::Relaxed);
                }
                Err(eb_err)
            }
            Err(_) => {
                let timeout_ms = timeout.as_millis() as u64;
                if let Some(logger) = &self.logger {
                    logger.log_recv_timeout(&self.subscriber_id, timeout_ms);
                }
                Err(EventBusError::RecvTimeout(timeout_ms))
            }
        }
    }

    /// 尝试非阻塞接收
    ///
    /// 返回 Ok(Some(event)) 表示有事件,Ok(None) 表示暂无事件,Err 表示错误
    pub fn try_recv(&mut self) -> Result<Option<NexusEvent>, EventBusError> {
        use broadcast::error::TryRecvError;
        match self.inner.try_recv() {
            Ok(event) => {
                if let Some(logger) = &self.logger {
                    logger.log_recv(&event);
                }
                Ok(Some(event))
            }
            Err(TryRecvError::Empty) => Ok(None),
            Err(TryRecvError::Closed) => {
                if let Some(logger) = &self.logger {
                    logger.log_channel_closed(&self.subscriber_id);
                }
                Err(EventBusError::ChannelClosed)
            }
            Err(TryRecvError::Lagged(lag)) => {
                if let Some(logger) = &self.logger {
                    logger.log_slow_consumer_dropped(&self.subscriber_id, lag, lag);
                }
                // A3 背压监控:递增 lagged_count(与 recv 保持一致)
                self.lagged_count.fetch_add(lag, Ordering::Relaxed);
                Err(EventBusError::SlowConsumerDropped {
                    subscriber_id: self.subscriber_id.clone(),
                    lag,
                })
            }
        }
    }

    /// 接收下一个匹配谓词的事件 — 选择性订阅(主题过滤)
    ///
    /// 内部循环接收事件,跳过不匹配的事件,直到找到匹配的或通道关闭。
    /// 不匹配的事件被消费但不返回给调用方(类似 filter+find 语义)。
    ///
    /// # 使用场景
    /// - 只关心特定类型的事件(如只监听 Quest 生命周期事件)
    /// - 只关心特定 quest_id 的事件
    /// - 按 severity 过滤(如只处理 Critical 事件)
    ///
    /// # 注意事项
    /// 不匹配的事件会被消费(从接收缓冲区移除),无法再被此 receiver 读取。
    /// 如果需要同时处理多种事件,应使用 `recv` 并在调用方分派。
    ///
    /// # 错误
    /// - `ChannelClosed`:所有 Sender 已 drop,流结束
    /// - `SlowConsumerDropped`:lag 超限,可能需要重订阅
    ///
    /// # 示例
    /// ```no_run
    /// use event_bus::{EventBus, NexusEvent};
    ///
    /// # async fn example(bus: &EventBus) {
    /// let mut rx = bus.subscribe();
    /// // 只接收 QuestCreated 事件
    /// let event = rx.recv_matching(|e| matches!(e, NexusEvent::QuestCreated { .. })).await.unwrap();
    /// # }
    /// ```
    pub async fn recv_matching<F>(&mut self, mut predicate: F) -> Result<NexusEvent, EventBusError>
    where
        F: FnMut(&NexusEvent) -> bool,
    {
        loop {
            let event = self.recv().await?;
            if predicate(&event) {
                return Ok(event);
            }
            // 不匹配的事件被消费并丢弃(调用方明确只需要匹配的事件)
        }
    }

    /// 尝试非阻塞接收匹配谓词的事件
    ///
    /// 扫描当前缓冲区中的事件,返回第一个匹配的。
    /// 不匹配的事件被消费(从缓冲区移除)。
    ///
    /// # 返回值
    /// - `Ok(Some(event))`:找到匹配事件
    /// - `Ok(None)`:缓冲区为空(可能还有后续事件,但当前无可用)
    /// - `Err`:通道关闭或 lag 超限
    pub fn try_recv_matching<F>(
        &mut self,
        mut predicate: F,
    ) -> Result<Option<NexusEvent>, EventBusError>
    where
        F: FnMut(&NexusEvent) -> bool,
    {
        loop {
            match self.try_recv()? {
                Some(event) if predicate(&event) => return Ok(Some(event)),
                Some(_) => continue, // 不匹配,消费并继续
                None => return Ok(None),
            }
        }
    }

    /// 获取订阅者标识
    pub fn subscriber_id(&self) -> &str {
        &self.subscriber_id
    }
}

impl Drop for EventReceiver {
    fn drop(&mut self) {
        if let Some(logger) = &self.logger {
            // 记录订阅者断开连接
            // 注:广播通道的 receiver_count 在 drop 前已减 1,
            // 此处记录的是 drop 后的剩余数量
            logger.log_subscriber_disconnected(
                &self.subscriber_id,
                0, // 无法在 Drop 中获取精确剩余数,用 0 表示已断开
            );
        }
    }
}

// ============================================================
// 序列化工具 — 用于跨进程投递(MCP Mesh)与持久化
// ============================================================

/// 将事件序列化为 MessagePack 字节(ADR-004)
///
/// 跨进程通信(MCP Mesh)与事件日志持久化时使用。
pub fn serialize_msgpack(event: &NexusEvent) -> Result<Vec<u8>, EventBusError> {
    rmp_serde::to_vec_named(event).map_err(EventBusError::from)
}

/// 从 MessagePack 字节反序列化事件
pub fn deserialize_msgpack(bytes: &[u8]) -> Result<NexusEvent, EventBusError> {
    rmp_serde::from_slice(bytes).map_err(EventBusError::from)
}

/// 将事件序列化为 JSON 字符串(降级通道,调试与兼容场景)
///
/// WHY:MessagePack 不可读,调试时 JSON 更直观;
/// 部分 MCP 客户端可能仅支持 JSON
pub fn serialize_json(event: &NexusEvent) -> Result<String, EventBusError> {
    serde_json::to_string(event).map_err(EventBusError::from)
}

/// 从 JSON 字符串反序列化事件
pub fn deserialize_json(s: &str) -> Result<NexusEvent, EventBusError> {
    serde_json::from_str(s).map_err(EventBusError::from)
}

// ============================================================
// P1-T12:测试辅助 — Critical 变体构造(D-8 口径,双清单同步红线)
// ============================================================

/// Critical 变体构造测试辅助(D-8 口径)
///
/// WHY pub(crate):shard.rs 的 Lane 判定全量断言(17 Critical → Critical)
/// 复用本模块构造事件,避免两处维护 17 个变体构造代码漂移(新增 Critical
/// 事件时只需改此处一处,三处清单同步守护见 test_critical_double_list_d8_counts)。
/// 仅测试构建存在(cfg(test)),生产零足迹。
#[cfg(test)]
pub(crate) mod tests_helpers {
    use super::*;

    /// 全量 13 个 mpsc 旁路变体构造(D-8 口径,双清单同步红线)
    pub fn all_mpsc_critical_variants() -> Vec<NexusEvent> {
        vec![
            NexusEvent::SkepticVeto {
                metadata: EventMetadata::new("t"),
                quest_id: "q".into(),
                veto_reason: "r".into(),
                frozen_capabilities: vec![],
            },
            NexusEvent::RedTeamAudit {
                metadata: EventMetadata::new("t"),
                vulnerability_type: "prompt_injection".into(),
                failed_probes: 1,
                total_probes: 2,
                detection_rate: 0.5,
                remediation_suggestion: "s".into(),
            },
            NexusEvent::BudgetExceeded {
                metadata: EventMetadata::new("t"),
                budget_type: "token".into(),
                current: 2,
                limit: 1,
            },
            NexusEvent::AgentTaskFailed {
                metadata: EventMetadata::new("t"),
                from: "agent-a".into(),
                to: "root".into(),
                task_id: "t-1".into(),
                error: "timeout".into(),
                retry_count: 0,
            },
            NexusEvent::AsaIntervention {
                metadata: EventMetadata::new("t"),
                operation_id: "op-1".into(),
                action: "Block".into(),
                safety_score: 0.1,
                block_reason: Some("unsafe".into()),
                alternative_suggestion: None,
            },
            NexusEvent::AffinityQuotaExhausted {
                metadata: EventMetadata::new("t"),
                route_key: "zhipu/glm-5.2".into(),
                reason: "quota".into(),
            },
            NexusEvent::R2FreezeViolation {
                metadata: EventMetadata::new("t"),
                violation_type: "CiDetection".into(),
                evidence: "ev".into(),
            },
            NexusEvent::R2FreezeRollbackFailed {
                metadata: EventMetadata::new("t"),
                reason: "git revert conflict".into(),
            },
            NexusEvent::FormalViolation {
                metadata: EventMetadata::new("t"),
                contract_id: "bc-1".into(),
                target_type: "event_bus::EventBus".into(),
                violations: vec!["v1".into()],
                context: nexus_contracts::behavior_contract::ContractContext::Runtime,
            },
            NexusEvent::VetoOverridden {
                metadata: EventMetadata::new("t"),
                quest_id: "q".into(),
                proposal_id: "p".into(),
                veto_reason: "r".into(),
                override_reason: "o".into(),
                override_by: "admin".into(),
            },
            NexusEvent::R1ShadowRollbackFailed {
                metadata: EventMetadata::new("t"),
                reason: "rollback conflict".into(),
                trigger_type: crate::types::RollbackTriggerType::Unknown,
                triggered_at: None,
                details: String::new(),
                diagnostic: crate::types::RollbackDiagnosticContext::default(),
            },
            NexusEvent::StopRulingIssued {
                metadata: EventMetadata::new("t"),
                quest_id: "q".into(),
                reason: "stagnation".into(),
                preserve_best: true,
            },
            NexusEvent::ErrorSignatureMatched {
                metadata: EventMetadata::new("t"),
                error_hash: "h".into(),
                matched_card_ids: vec![],
            },
        ]
    }

    /// 全量 17 个 severity() Critical 变体构造(D-8 口径)
    ///
    /// 17 = 13(mpsc 旁路,见 [`all_mpsc_critical_variants`])
    ///   + 4(历史 Critical 只走 broadcast:CheckpointSaved/ConsensusReached/
    ///     SlowConsumerDropped/OrphanCallDetected,按既定设计不回退通道归属)。
    pub fn all_severity_critical_variants() -> Vec<NexusEvent> {
        let mut variants = all_mpsc_critical_variants();
        variants.extend([
            NexusEvent::CheckpointSaved {
                metadata: EventMetadata::new("t"),
                quest_id: "q".into(),
                checkpoint_id: "c".into(),
                memory_snapshot_hash: "h".into(),
            },
            NexusEvent::ConsensusReached {
                metadata: EventMetadata::new("t"),
                quest_id: "q".into(),
                decision_hash: "h".into(),
                dpo_pair_id: None,
            },
            NexusEvent::SlowConsumerDropped {
                metadata: EventMetadata::new("t"),
                subscriber_id: "sub".into(),
                lag: 1,
                dropped_count: 1,
            },
            NexusEvent::OrphanCallDetected {
                metadata: EventMetadata::new("t"),
                operation_id: "op".into(),
                spawn_location: "bus.rs".into(),
            },
        ]);
        variants
    }

    /// 按变体名构造 Critical 事件(供 LANE_FORBIDDEN_SHARD 逐名断言)
    ///
    /// 名字不在 17 清单中返回 None(调用方断言消息定位漂移名字)。
    pub fn critical_variant_by_name(name: &str) -> Option<NexusEvent> {
        all_severity_critical_variants()
            .into_iter()
            .find(|e| e.type_name() == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shard::DEFAULT_SHARD_COUNT;
    use crate::types::EventMetadata;

    fn make_test_event() -> NexusEvent {
        NexusEvent::QuestCreated {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: "q-001".into(),
            title: "测试任务".into(),
            task_count: 3,
        }
    }

    #[tokio::test]
    async fn test_publish_subscribe_basic() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let event = make_test_event();
        bus.publish(event.clone()).await.unwrap();
        let received = rx.recv().await.unwrap();
        assert_eq!(received, event);
    }

    #[tokio::test]
    async fn test_no_subscribers_ok() {
        let bus = EventBus::new();
        // 无订阅者时发布应返回 Ok(()),非错误
        bus.publish(make_test_event()).await.unwrap();
    }

    #[tokio::test]
    async fn test_multiple_subscribers() {
        let bus = EventBus::new();
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();
        let event = make_test_event();
        bus.publish(event.clone()).await.unwrap();
        assert_eq!(rx1.recv().await.unwrap(), event);
        assert_eq!(rx2.recv().await.unwrap(), event);
    }

    #[tokio::test]
    async fn test_recv_timeout() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let result = rx.recv_timeout(Duration::from_millis(50)).await;
        assert!(matches!(result, Err(EventBusError::RecvTimeout(_))));
    }

    // ============================================================
    // M1-T1.3:publish_batch 批量发布测试
    // ============================================================

    fn make_indexed_event(i: usize) -> NexusEvent {
        NexusEvent::QuestCreated {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: format!("q-{i}"),
            title: format!("任务 {i}"),
            task_count: 1,
        }
    }

    #[tokio::test]
    async fn test_publish_batch_delivers_all_events() {
        // 批量发布后所有事件均被订阅者接收(计数与内容完整)
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let events: Vec<NexusEvent> = (0..5).map(make_indexed_event).collect();

        bus.publish_batch(events.clone()).await.unwrap();

        for expected in &events {
            let received = rx.recv().await.unwrap();
            assert_eq!(&received, expected, "批量发布应保留逐条内容与顺序");
        }
    }

    #[tokio::test]
    async fn test_publish_batch_empty_early_return() {
        // 空 Vec 早退:不发布任何事件,不 panic
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        bus.publish_batch(Vec::new()).await.unwrap();
        // 超时无事件可收
        let result = rx.recv_timeout(Duration::from_millis(50)).await;
        assert!(matches!(result, Err(EventBusError::RecvTimeout(_))));
    }

    #[tokio::test]
    async fn test_publish_batch_critical_mpsc_bypass_preserved() {
        // 批量中含 Critical 安全事件(SkepticVeto)时,mpsc 旁路仍投递
        let bus = EventBus::new();
        let mut critical_rx = bus.subscribe_critical_events();
        let events = vec![
            make_indexed_event(0),
            NexusEvent::SkepticVeto {
                metadata: EventMetadata::new("parliament"),
                quest_id: "q-veto".into(),
                veto_reason: "恶意意图".into(),
                frozen_capabilities: vec!["cap-x".into()],
            },
            make_indexed_event(1),
        ];

        bus.publish_batch(events).await.unwrap();

        // Critical 旁路应收到 SkepticVeto(不被普通事件淹没)
        let received = tokio::time::timeout(Duration::from_millis(200), critical_rx.recv())
            .await
            .expect("不应超时")
            .expect("Critical 旁路应收到 SkepticVeto");
        assert_eq!(received.type_name(), "SkepticVeto");
    }

    #[tokio::test]
    async fn test_affinity_quota_exhausted_mpsc_delivery() {
        // MCA M0(ADR-065):AffinityQuotaExhausted 必须走 mpsc 旁路确保投递
        // (丢失导致降级链无人触发,请求持续打向死通道)
        let bus = EventBus::new();
        let mut critical_rx = bus.subscribe_critical_events();
        let event = NexusEvent::AffinityQuotaExhausted {
            metadata: EventMetadata::new("mca-gateway"),
            route_key: "deep_seek/deepseek-v4-flash".into(),
            reason: "429 quota exceeded for this month".into(),
        };

        bus.publish(event).await.unwrap();

        let received = tokio::time::timeout(Duration::from_millis(200), critical_rx.recv())
            .await
            .expect("不应超时")
            .expect("Critical 旁路应收到 AffinityQuotaExhausted");
        assert_eq!(received.type_name(), "AffinityQuotaExhausted");
        assert_eq!(received.severity(), crate::EventSeverity::Critical);
    }

    #[test]
    fn test_critical_severity_implies_mpsc_bypass() {
        // 双清单同步守护(MCA M0 起):§6.2 红线要求的安全/资源类 Critical
        // 事件必须同时在 severity() 与 is_critical_mpsc_event() 两张清单中。
        // WHY 不断言全部 Critical 变体:CheckpointSaved/ConsensusReached 等
        // 历史 Critical 事件按既定设计只走 broadcast(背压保护级别),
        // 本测试只锁定"必须确保投递"的安全/资源事件子集不回退。
        let mpsc_required = [
            NexusEvent::SkepticVeto {
                metadata: EventMetadata::new("t"),
                quest_id: "q".into(),
                veto_reason: "r".into(),
                frozen_capabilities: vec![],
            },
            NexusEvent::RedTeamAudit {
                metadata: EventMetadata::new("t"),
                vulnerability_type: "prompt_injection".into(),
                failed_probes: 1,
                total_probes: 2,
                detection_rate: 0.5,
                remediation_suggestion: "s".into(),
            },
            NexusEvent::BudgetExceeded {
                metadata: EventMetadata::new("t"),
                budget_type: "token".into(),
                current: 2,
                limit: 1,
            },
            NexusEvent::AgentTaskFailed {
                metadata: EventMetadata::new("t"),
                from: "agent-a".into(),
                to: "root".into(),
                task_id: "t-1".into(),
                error: "timeout".into(),
                retry_count: 0,
            },
            NexusEvent::AsaIntervention {
                metadata: EventMetadata::new("t"),
                operation_id: "op-1".into(),
                action: "Block".into(),
                safety_score: 0.1,
                block_reason: Some("unsafe".into()),
                alternative_suggestion: None,
            },
            NexusEvent::AffinityQuotaExhausted {
                metadata: EventMetadata::new("t"),
                route_key: "zhipu/glm-5.2".into(),
                reason: "quota".into(),
            },
            NexusEvent::R2FreezeViolation {
                metadata: EventMetadata::new("t"),
                violation_type: "CiDetection".into(),
                evidence: "ev".into(),
            },
            NexusEvent::R2FreezeRollbackFailed {
                metadata: EventMetadata::new("t"),
                reason: "git revert conflict".into(),
            },
            NexusEvent::FormalViolation {
                metadata: EventMetadata::new("t"),
                contract_id: "bc-1".into(),
                target_type: "event_bus::EventBus".into(),
                violations: vec!["v1".into()],
                context: nexus_contracts::behavior_contract::ContractContext::Runtime,
            },
            // Phase 10 Wave 5 双清单对齐(+4):否决覆盖审计/影子回滚失败/
            // 停止裁决/错误签名匹配(均 severity() Critical,必须确保投递)
            NexusEvent::VetoOverridden {
                metadata: EventMetadata::new("t"),
                quest_id: "q".into(),
                proposal_id: "p".into(),
                veto_reason: "r".into(),
                override_reason: "o".into(),
                override_by: "admin".into(),
            },
            NexusEvent::R1ShadowRollbackFailed {
                metadata: EventMetadata::new("t"),
                reason: "rollback conflict".into(),
                trigger_type: crate::types::RollbackTriggerType::Unknown,
                triggered_at: None,
                details: String::new(),
                diagnostic: crate::types::RollbackDiagnosticContext::default(),
            },
            NexusEvent::StopRulingIssued {
                metadata: EventMetadata::new("t"),
                quest_id: "q".into(),
                reason: "stagnation".into(),
                preserve_best: true,
            },
            NexusEvent::ErrorSignatureMatched {
                metadata: EventMetadata::new("t"),
                error_hash: "h".into(),
                matched_card_ids: vec![],
            },
        ];
        assert_eq!(
            mpsc_required.len(),
            13,
            "旁路清单应覆盖全量 13 个事件,新增 Critical 必须显式加入(双清单同步红线)"
        );
        for event in &mpsc_required {
            assert!(
                is_critical_mpsc_event(event),
                "{} 必须在 mpsc 旁路清单中(双清单同步红线)",
                event.type_name()
            );
            assert_eq!(
                event.severity(),
                crate::EventSeverity::Critical,
                "{} 的 severity() 必须为 Critical(双清单一致)",
                event.type_name()
            );
        }
    }

    /// D-8 双清单计数守护(P1-T11):CRITICAL_MPSC_VARIANTS(13)、
    /// CRITICAL_TOTAL(17)、13 ⊆ 17、LANE_FORBIDDEN_SHARD 一一对应
    #[test]
    fn test_critical_double_list_d8_counts() {
        // 口径断言:两清单规模与常量一致(常量是 D-8 裁决的编译期锚点)
        // WHY 引用 tests_helpers:变体构造已上移 pub(crate) 模块(D-8 口径唯一
        // 来源,shard.rs Lane 全量断言复用;本模块不再维护副本,防漂移)
        let mpsc_variants = tests_helpers::all_mpsc_critical_variants();
        let critical_variants = tests_helpers::all_severity_critical_variants();
        assert_eq!(
            mpsc_variants.len(),
            CRITICAL_MPSC_VARIANTS,
            "mpsc 旁路变体数必须等于 CRITICAL_MPSC_VARIANTS({CRITICAL_MPSC_VARIANTS})"
        );
        assert_eq!(
            critical_variants.len(),
            CRITICAL_TOTAL,
            "severity() Critical 变体数必须等于 CRITICAL_TOTAL({CRITICAL_TOTAL})"
        );

        // 13 个 mpsc 变体:is_critical_mpsc_event 必中 + severity() Critical
        for event in &mpsc_variants {
            assert!(
                is_critical_mpsc_event(event),
                "{} 必须在 mpsc 旁路清单中(双清单同步红线)",
                event.type_name()
            );
            assert_eq!(
                event.severity(),
                EventSeverity::Critical,
                "{} 的 severity() 必须为 Critical(双清单一致)",
                event.type_name()
            );
        }
        // 17 个 Critical 变体:severity() 全部为 Critical(17 口径)
        for event in &critical_variants {
            assert_eq!(
                event.severity(),
                EventSeverity::Critical,
                "{} 应命中 severity() Critical 清单(17 口径)",
                event.type_name()
            );
        }

        // 13 ⊆ 17:每个 mpsc 变体的名字必须在 severity() Critical 清单中
        let mpsc_names: Vec<&str> = mpsc_variants.iter().map(|e| e.type_name()).collect();
        let critical_names: Vec<&str> = critical_variants.iter().map(|e| e.type_name()).collect();
        for name in &mpsc_names {
            assert!(
                critical_names.contains(name),
                "mpsc 变体 {name} 不在 severity() Critical 清单中(13 ⊆ 17 违反)"
            );
        }

        // LANE_FORBIDDEN_SHARD 分片禁区声明:17 个名字与 Critical 清单一一对应
        // WHY 双向断言:T12 分片实现将按此清单禁止切片 Critical 单流;
        // 任一方向缺失都意味着声明与实际清单漂移
        assert_eq!(
            LANE_FORBIDDEN_SHARD.len(),
            CRITICAL_TOTAL,
            "LANE_FORBIDDEN_SHARD 必须恰好包含 {CRITICAL_TOTAL} 个名字"
        );
        for name in LANE_FORBIDDEN_SHARD {
            assert!(
                critical_names.contains(name),
                "LANE_FORBIDDEN_SHARD 含未知 Critical 变体名: {name}"
            );
        }
        for name in &critical_names {
            assert!(
                LANE_FORBIDDEN_SHARD.contains(name),
                "Critical 变体 {name} 未声明在 LANE_FORBIDDEN_SHARD(T12 分片禁区缺失)"
            );
        }
    }

    // ============================================================
    // R7-Critical 清单反向断言(bus.rs:1580/1674 手抄清单零反向断言治理)
    // ============================================================
    // WHY 替代"枚举反射"方案:本 crate 至少在 R7 时点 NexusEvent 无 all()/ALL/
    // iter() 全量遍历辅助,且 Rust 无反射,无法从 144 变体"枚举定义"可靠穷举
    // severity()-Critical 全集。故采用**三清单互锁 + 双向 is_critical_mpsc_event
    // 交叉断言**:把行为判定(severity() / is_critical_mpsc_event)与声明清单
    // (tests_helpers 双清单 + 常量注释记载的历史广播级)绑成一张闭环网,任何一环
    // 漂移都触发红灯。权威源:severity() 显式臂(classification.rs) + L44-49 注释
    // 记载的 4 个历史广播级 Critical。

    /// R7-正向反向断言:手抄双清单每一项必须与 severity() 权威判定一致,
    /// 且规模锚定(d8 常量)不回退。防"清单声称为 Critical 但 classification.rs
    /// 判为 Normal"(被通配 `_ => Normal` 吞掉的静默降级)。
    #[test]
    fn test_critical_lists_are_all_severity_critical() {
        let mpsc_variants = tests_helpers::all_mpsc_critical_variants();
        let severity_variants = tests_helpers::all_severity_critical_variants();

        // 现实值核验:13(mpsc 旁路)/ 17(severity Critical,含 4 个历史广播级)。
        // WHY 锚定常量而非硬编码 13/17:常量是 D-8 裁决的编译期锚点,清单规模
        // 一旦回退(误删)此测试即红。
        assert_eq!(
            mpsc_variants.len(),
            CRITICAL_MPSC_VARIANTS,
            "mpsc 旁路清单规模异常"
        );
        assert_eq!(
            severity_variants.len(),
            CRITICAL_TOTAL,
            "severity() Critical 清单规模异常"
        );

        // 正向反向:并集内每个声称为 Critical 的事件,severity() 必须真返回 Critical。
        // 一旦 classification.rs severity() 加显式臂时漏在此清单登记,或清单登记了
        // 某个实际被判 Normal 的事件,此断言红灯 —— 反向捕获两只清单漂移方向。
        for event in mpsc_variants.iter().chain(severity_variants.iter()) {
            assert_eq!(
                event.severity(),
                EventSeverity::Critical,
                "{} 声明为 Critical(手抄清单)但 severity() 返回 {:?} → 与 classification.rs 权威判定漂移",
                event.type_name(),
                event.severity()
            );
        }
    }

    /// R7-核心反向断言:severity-Critical 全集(清单)与 mpsc 旁路判定反查互锁。
    ///
    /// 覆盖任务约定的互锁断言 (c)/(d)+反向交叉:
    /// - (c) mpsc_set ⊆ severity_set:每个 mpsc 变体必须在 severity 清单中;
    /// - (d) severity_set − mpsc_set 恰为 4 个历史广播级 Critical
    ///   (CheckpointSaved/ConsensusReached/SlowConsumerDropped/OrphanCallDetected,
    ///   唯一权威源 = bus.rs L44-49 常量注释)——语义:除既定历史广播级 4 项外,
    ///   **任何 severity() Critical 变体都必须走 mpsc 旁路**。若未来新增 Critical
    ///   只看 severity 清单、漏接 mpsc,差值集合会出现非历史项 → 红。
    /// - 反向交叉 is_critical_mpsc_event:`mpsc 清单项必须判定 true`,历史广播级
    ///   4 项必须判定 false,锁定清单与判定函数不漂移(含 4 项通道归属不被误回退)。
    #[test]
    fn test_critical_severity_census_matches_lists() {
        use std::collections::{HashMap, HashSet};

        // 历史广播级 Critical(不走 mpsc 旁路)的唯一权威来源:bus.rs L44-49 常量注释。
        // 写死于此以独立于 tests_helpers,形成对"注释声明"的落点核对。
        let historical_broadcast_critical: [&str; 4] = [
            "CheckpointSaved",
            "ConsensusReached",
            "SlowConsumerDropped",
            "OrphanCallDetected",
        ];

        let mpsc_variants = tests_helpers::all_mpsc_critical_variants();
        let severity_variants = tests_helpers::all_severity_critical_variants();

        let mpsc_names: HashSet<&str> = mpsc_variants.iter().map(|e| e.type_name()).collect();
        let severity_by_name: HashMap<&str, &NexusEvent> = severity_variants
            .iter()
            .map(|e| (e.type_name(), e))
            .collect();

        // (c) mpsc_set ⊆ severity_set
        for name in &mpsc_names {
            assert!(
                severity_by_name.contains_key(name),
                "mpsc 变体 {name} 不在 severity() Critical 清单中(13 ⊆ 17 违反)"
            );
        }

        // (d) severity_set − mpsc_set 恰为 4 个历史广播级 Critical
        let broadcast_only: Vec<&str> = severity_by_name
            .keys()
            .copied()
            .filter(|n| !mpsc_names.contains(n))
            .collect();
        assert_eq!(
            broadcast_only.len(),
            historical_broadcast_critical.len(),
            "severity Critical − mpsc Critical 差值必须恰为 4 个历史广播级事件,实际 {}: {broadcast_only:?}",
            broadcast_only.len()
        );
        for name in &broadcast_only {
            assert!(
                historical_broadcast_critical.contains(name),
                "新增 Critical 变体 {name} 仅登记在 severity 清单(漏接 mpsc 旁路)或通道归属异常;历史广播级 4 项为 {historical_broadcast_critical:?}"
            );
        }

        // 反向交叉 is_critical_mpsc_event:
        // (i) mpsc 清单每一项判定必须 true(清单与判定函数漂移 → 红);
        // (ii) 历史广播级 4 项判定必须 false(4 项通道归属不得被误回退 → 红)。
        for event in &mpsc_variants {
            assert!(
                is_critical_mpsc_event(event),
                "{} 在 mpsc 清单但 is_critical_mpsc_event 判定 false → 清单与判定函数漂移",
                event.type_name()
            );
        }
        for name in historical_broadcast_critical {
            let event = severity_by_name
                .get(name)
                .expect("历史广播级 Critical 应存在于 severity() 清单");
            assert!(
                !is_critical_mpsc_event(event),
                "{name} 为历史广播级 Critical(只走 broadcast)但 is_critical_mpsc_event 判定 true → 通道归属被误回退"
            );
        }
    }

    #[test]
    fn test_msgpack_roundtrip() {
        let event = make_test_event();
        let bytes = serialize_msgpack(&event).unwrap();
        let decoded = deserialize_msgpack(&bytes).unwrap();
        assert_eq!(decoded, event);
    }

    #[test]
    fn test_json_roundtrip() {
        let event = make_test_event();
        let s = serialize_json(&event).unwrap();
        let decoded = deserialize_json(&s).unwrap();
        assert_eq!(decoded, event);
    }

    #[tokio::test]
    async fn test_subscriber_count() {
        let bus = EventBus::new();
        assert_eq!(bus.subscriber_count(), 0);
        let _rx1 = bus.subscribe();
        let _rx2 = bus.subscribe();
        assert_eq!(bus.subscriber_count(), 2);
    }

    // ============================================================
    // P1-1: 事件主题过滤测试
    // ============================================================

    #[tokio::test]
    async fn test_recv_matching_filters_events() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        // 发布不同类型的事件
        let quest_event = make_test_event();
        let progress_event = NexusEvent::QuestProgressUpdated {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: "q-001".into(),
            completed: 1,
            total: 3,
        };

        bus.publish(quest_event.clone()).await.unwrap();
        bus.publish(progress_event.clone()).await.unwrap();

        // recv_matching 只接收 QuestProgressUpdated
        let received = rx
            .recv_matching(|e| matches!(e, NexusEvent::QuestProgressUpdated { .. }))
            .await
            .unwrap();
        assert_eq!(received, progress_event);
        // QuestCreated 事件被消费但未返回
    }

    #[tokio::test]
    async fn test_recv_matching_skips_non_matching() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        // 发布 3 个事件,只有最后 1 个匹配
        for i in 0..2 {
            bus.publish(NexusEvent::QuestProgressUpdated {
                metadata: EventMetadata::new("quest-engine"),
                quest_id: format!("q-{i}"),
                completed: i as u32,
                total: 3,
            })
            .await
            .unwrap();
        }
        let target = make_test_event(); // QuestCreated
        bus.publish(target.clone()).await.unwrap();

        // 只匹配 QuestCreated — 前两个 ProgressUpdated 被跳过
        let received = rx
            .recv_matching(|e| matches!(e, NexusEvent::QuestCreated { .. }))
            .await
            .unwrap();
        assert_eq!(received, target);
    }

    #[tokio::test]
    async fn test_recv_matching_by_quest_id() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        bus.publish(NexusEvent::QuestCreated {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: "q-other".into(),
            title: "其他任务".into(),
            task_count: 1,
        })
        .await
        .unwrap();

        let target = NexusEvent::QuestCreated {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: "q-target".into(),
            title: "目标任务".into(),
            task_count: 5,
        };
        bus.publish(target.clone()).await.unwrap();

        // 按 quest_id 过滤
        let received = rx
            .recv_matching(|e| {
                matches!(e, NexusEvent::QuestCreated { quest_id, .. } if quest_id == "q-target")
            })
            .await
            .unwrap();
        assert_eq!(received, target);
    }

    #[test]
    fn test_try_recv_matching_finds_event() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        // 同步发布多个事件
        bus.publish_blocking(NexusEvent::QuestProgressUpdated {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: "q-1".into(),
            completed: 1,
            total: 3,
        })
        .unwrap();
        let target = make_test_event();
        bus.publish_blocking(target.clone()).unwrap();

        // 只匹配 QuestCreated
        let result = rx
            .try_recv_matching(|e| matches!(e, NexusEvent::QuestCreated { .. }))
            .unwrap();
        assert_eq!(result, Some(target));
    }

    #[test]
    fn test_try_recv_matching_empty_buffer() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        // 缓冲区为空
        let result = rx.try_recv_matching(|_| true).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_try_recv_matching_no_match_in_buffer() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();

        // 只有 ProgressUpdated 事件
        bus.publish_blocking(NexusEvent::QuestProgressUpdated {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: "q-1".into(),
            completed: 1,
            total: 3,
        })
        .unwrap();

        // 寻找 QuestCreated — 不应找到
        let result = rx
            .try_recv_matching(|e| matches!(e, NexusEvent::QuestCreated { .. }))
            .unwrap();
        assert_eq!(result, None, "缓冲区中没有匹配事件");
    }

    // ============================================================
    // A3: 背压监控测试
    // ============================================================

    #[test]
    fn test_backpressure_monitor_initial_zero() {
        // 验证初始计数器为 0
        let bus = EventBus::new();
        let (lagged, warnings) = bus.backpressure_stats();
        assert_eq!(lagged, 0, "初始 lagged_count 应为 0");
        assert_eq!(warnings, 0, "初始 backpressure_warning_count 应为 0");
    }

    #[tokio::test]
    async fn test_backpressure_monitor_tracks_lagged() {
        // 使用极小容量(4)模拟 broadcast 满容量场景
        let bus = EventBus::with_capacity(4);
        let mut rx = bus.subscribe();

        // 发布超过容量的事件,使 receiver 的缓冲区溢出
        // broadcast::channel(4) 最多保留 4 条,发布 10 条后 receiver 会 Lagged
        for i in 0..10 {
            bus.publish(NexusEvent::QuestProgressUpdated {
                metadata: EventMetadata::new("test"),
                quest_id: format!("q-{i}"),
                completed: i as u32,
                total: 10,
            })
            .await
            .unwrap();
        }

        // 接收端应触发 Lagged(慢消费者丢弃)
        let result = rx.recv().await;
        assert!(
            matches!(result, Err(EventBusError::SlowConsumerDropped { .. })),
            "应触发 SlowConsumerDropped,实际: {result:?}"
        );

        // 验证 lagged_count 已递增(丢弃的事件数 > 0)
        let (lagged, _warnings) = bus.backpressure_stats();
        assert!(lagged > 0, "lagged_count 应递增,实际: {lagged}");
    }

    #[tokio::test]
    async fn test_backpressure_monitor_tracks_warnings() {
        // 使用小容量(8)触发背压告警(> 75% = 6)
        let bus = EventBus::with_capacity(8);
        let _rx = bus.subscribe(); // 保持订阅者,防止 send 返回 Err

        // 发布 8 个事件(第 8 次 publish 时,send 前 sender.len() == 7 > 6 = 8*3/4)
        // 注意:背压检查在 send() 之前,所以需要 queued > threshold 即 queued >= 7
        for i in 0..8 {
            bus.publish(NexusEvent::QuestProgressUpdated {
                metadata: EventMetadata::new("test"),
                quest_id: format!("q-{i}"),
                completed: i as u32,
                total: 8,
            })
            .await
            .unwrap();
        }

        let (_lagged, warnings) = bus.backpressure_stats();
        assert!(
            warnings > 0,
            "backpressure_warning_count 应递增,实际: {warnings}"
        );
    }

    // ============================================================
    // P1-T12: ShardedBus 分片灰度 —— EventBus 级集成测试
    // ============================================================
    // 断言口径:漏发率 = 0(sharded_total == merged_total,影子双跑前哨硬门禁)、
    // 17 Critical 永不进分片、OrderSensitive 单流保序、灰度默认关闭零回归。

    /// 带超时批量接收(测试辅助:漏发率 0 的前提是能收满事件)
    async fn drain_events(
        rx: &mut EventReceiver,
        expect: usize,
        timeout_ms: u64,
    ) -> Vec<NexusEvent> {
        let mut received = Vec::with_capacity(expect);
        let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
        while received.len() < expect {
            if tokio::time::Instant::now() >= deadline {
                break;
            }
            match tokio::time::timeout(Duration::from_millis(50), rx.recv()).await {
                Ok(Ok(event)) => received.push(event),
                _ => break,
            }
        }
        received
    }

    /// 轮询等待前哨统计收敛(sharded == merged;T13 漏发率=0 口径的测试版)
    async fn wait_merged_equals_sharded(bus: &EventBus, timeout_ms: u64) -> ShadowStats {
        let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            let stats = bus.shadow_stats();
            if stats.sharded_total > 0 && stats.merged_total >= stats.sharded_total {
                return stats;
            }
            if tokio::time::Instant::now() >= deadline {
                return stats;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    #[test]
    fn test_sharding_default_off_zero_regression() {
        // 灰度默认关:EventBus::new() 行为与 v2.27.1 完全一致(零回归)
        let bus = EventBus::new();
        assert!(!bus.sharding_enabled(), "灰度默认必须关闭分片");
        let stats = bus.shadow_stats();
        assert_eq!(stats.published_total, 0);
        assert_eq!(stats.sharded_total, 0);
        assert_eq!(stats.merged_total, 0);
        assert_eq!(stats.shed_total, 0);
        assert_eq!(stats.critical_total, 0);
        // 无 runtime 上下文:enable_sharding 返回 Err 而非 panic(调用方 let _ 忽略即降级)
        assert!(matches!(
            bus.enable_sharding(64),
            Err(EventBusError::ShardingRequiresRuntime)
        ));
        assert!(!bus.sharding_enabled(), "降级后分片仍必须保持关闭");
    }

    #[tokio::test]
    async fn test_enable_sharding_idempotent() {
        let bus = EventBus::new();
        bus.enable_sharding(64).unwrap();
        assert!(bus.sharding_enabled());
        // 重复调用幂等拒绝(不重复 spawn worker,防泄漏)
        assert!(matches!(
            bus.enable_sharding(64),
            Err(EventBusError::ShardingAlreadyEnabled)
        ));
        assert!(matches!(
            bus.enable_sharding(128),
            Err(EventBusError::ShardingAlreadyEnabled)
        ));
        assert!(bus.sharding_enabled(), "幂等拒绝后分片状态不变");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_enable_sharding_concurrent_no_duplicate_spawn() {
        // 评审 Issue 1 回归守卫:并发双 enable_sharding 不重复 spawn worker
        // - 并发 8 个调用(barrier 同时进入):恰好 1 个 Ok,其余 Err(ShardingAlreadyEnabled);
        // - 失败路径不得 spawn worker:以 credit_flow 强引用计数观测 —— 每个 worker
        //   持有 Arc<CreditFlow> 一份,成功组 64 个 + 总线自身 1 份 = 65;
        //   修复前(锁外 spawn)失败者也 spawn 64 个 worker → 引用数 > 65,断言失败
        //   (64 个 worker 持有未注册总线永久空转,评审发现的资源泄漏)。
        let bus = EventBus::new();
        const CALLS: usize = 8;
        let barrier = Arc::new(tokio::sync::Barrier::new(CALLS));
        let handles: Vec<_> = (0..CALLS)
            .map(|_| {
                let b = bus.clone();
                let barrier = Arc::clone(&barrier);
                tokio::spawn(async move {
                    barrier.wait().await; // 全部就绪后同时进入,最大化并发交错
                    b.enable_sharding(DEFAULT_SHARD_COUNT)
                })
            })
            .collect();
        let mut ok = 0;
        for h in handles {
            match h.await.expect("并发 task panic") {
                Ok(()) => ok += 1,
                Err(EventBusError::ShardingAlreadyEnabled) => {}
                Err(e) => panic!("并发启用分片只应出现幂等拒绝: {e:?}"),
            }
        }
        assert_eq!(ok, 1, "并发启用分片必须恰好一个成功");
        assert!(bus.sharding_enabled(), "启用后分片标志必须为 true");
        // worker 不翻倍:credit_flow 强引用 = 总线 1 + 每片 worker 1(65)
        // (断言时 8 个 task 已结束、其 EventBus clone 已 drop;worker 任务仍挂载
        //  在测试 runtime 上持有引用 —— 修复前失败路径的 worker 亦在,计数 > 65)
        assert_eq!(
            Arc::strong_count(&bus.credit_flow),
            1 + DEFAULT_SHARD_COUNT,
            "并发失败路径不得 spawn worker(引用计数翻倍 = 重复 spawn 泄漏)"
        );
        // 安装的分片总线仅被总线持有 + 每片 worker 各一份(恰好一组 worker)
        let guard = bus.shard_bus.lock().expect("分片总线锁不应 poison");
        let sb = guard.as_ref().expect("分片总线必须已安装");
        assert_eq!(
            Arc::strong_count(sb),
            1 + DEFAULT_SHARD_COUNT,
            "安装的分片总线必须恰有一组 worker"
        );
    }

    #[tokio::test]
    async fn test_sharded_all_delivered_no_loss() {
        // 汇入完整性:100 个 Unordered 事件(< 256 片容量,无 shed)全部送达
        let bus = EventBus::new();
        bus.enable_sharding(64).unwrap();
        let mut rx = bus.subscribe();
        const N: usize = 100;
        for _ in 0..N {
            bus.publish(make_test_event()).await.unwrap();
        }
        let received = drain_events(&mut rx, N, 5000).await;
        assert_eq!(received.len(), N, "漏发率必须为 0:订阅者应收到全部事件");
        let stats = wait_merged_equals_sharded(&bus, 2000).await;
        assert_eq!(stats.sharded_total, N as u64, "容量内事件应全部入分片");
        assert_eq!(
            stats.merged_total, stats.sharded_total,
            "入片数 == 汇入数(影子双跑漏发率 = 0)"
        );
        assert_eq!(stats.shed_total, 0, "容量内发布不应触发 shed");
    }

    #[tokio::test]
    async fn test_order_sensitive_single_stream_preserves_order() {
        // 顺序敏感:correlation_id 为广义会话键 → OrderSensitive 车道,
        // 单流保序、不分片(E8-4:顺序敏感通道保持单流)
        let bus = EventBus::new();
        bus.enable_sharding(64).unwrap();
        let mut rx = bus.subscribe();
        for i in 0..5 {
            bus.publish(NexusEvent::QuestCreated {
                metadata: EventMetadata::with_correlation("test", format!("corr-{i}")),
                quest_id: format!("q-{i}"),
                title: format!("任务 {i}"),
                task_count: 1,
            })
            .await
            .unwrap();
        }
        let received = drain_events(&mut rx, 5, 5000).await;
        assert_eq!(received.len(), 5);
        for (i, ev) in received.iter().enumerate() {
            let NexusEvent::QuestCreated { quest_id, .. } = ev else {
                panic!("应收到 QuestCreated");
            };
            assert_eq!(
                quest_id,
                &format!("q-{i}"),
                "OrderSensitive 事件必须保持发布顺序"
            );
        }
        assert_eq!(
            bus.shadow_stats().sharded_total,
            0,
            "OrderSensitive 通道不分片(sharded_total 恒 0)"
        );
    }

    #[tokio::test]
    async fn test_shard_chaos_shed_and_critical_survive() {
        // 混沌:分片满 + 信用耗尽 → shed 计数增长;Critical 事件仍全送达
        // (current_thread runtime 下测试 task 无 yield,worker 无机会消费,
        //  同 type 事件确定性地填满同片 + 耗尽 256 信用池 → 确定性 shed)
        let bus = EventBus::new();
        bus.enable_sharding(64).unwrap();
        let mut rx = bus.subscribe();
        let mut critical_rx = bus.subscribe_critical_events();
        const FLOOD: usize = 400; // 256 入片 + 144 回退(< 1024 broadcast 容量,无 Lagged)
        for _ in 0..FLOOD {
            bus.publish(make_test_event()).await.unwrap();
        }
        // Critical 混发(3 个):永不进分片、永不丢弃
        let critical_events = tests_helpers::all_mpsc_critical_variants();
        for ev in critical_events.iter().take(3) {
            bus.publish(ev.clone()).await.unwrap();
        }
        // Critical 必须经 mpsc 旁路即时送达(分片/信用/背压均不影响)
        for _ in 0..3 {
            let critical = critical_rx.recv().await.unwrap();
            assert_eq!(
                critical.severity(),
                EventSeverity::Critical,
                "Critical 事件必须全送达(红线)"
            );
        }
        // shed 计数增长(片满 + 信用耗尽)
        assert!(
            bus.shadow_stats().shed_total > 0,
            "分片满 + 信用耗尽必须触发 shed 计数"
        );
        // 漏发率 0:订阅者收到全部(256 入片由 worker 汇入 + 144 回退直接投递)
        let received = drain_events(&mut rx, FLOOD + 3, 10000).await;
        assert_eq!(received.len(), FLOOD + 3, "混沌下漏发率必须为 0");
        let stats = wait_merged_equals_sharded(&bus, 3000).await;
        assert_eq!(stats.sharded_total, 256, "片容量 256:确定性入片数");
        assert_eq!(
            stats.merged_total, stats.sharded_total,
            "入片数 == 汇入数(漏发率 = 0)"
        );
        assert_eq!(stats.critical_total, 3, "Critical 车道前哨计数");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn test_sharded_concurrent_publish_no_loss() {
        // loom 替代(Windows GNU 工具链 loom 编译失败):8 线程 × 100 次并发
        // publish_blocking,验证 Mutex/Atomic/ArrayQueue/CAS 竞争安全 + 漏发率 0
        let bus = EventBus::new();
        bus.enable_sharding(64).unwrap();
        let mut rx = bus.subscribe();
        const THREADS: usize = 8;
        const PER_THREAD: usize = 100;
        let handles: Vec<_> = (0..THREADS)
            .map(|_| {
                let b = bus.clone();
                std::thread::spawn(move || {
                    for _ in 0..PER_THREAD {
                        b.publish_blocking(make_test_event()).unwrap();
                    }
                })
            })
            .collect();
        for h in handles {
            h.join().unwrap();
        }
        let received = drain_events(&mut rx, THREADS * PER_THREAD, 15000).await;
        assert_eq!(
            received.len(),
            THREADS * PER_THREAD,
            "多线程并发下漏发率必须为 0"
        );
        let stats = wait_merged_equals_sharded(&bus, 5000).await;
        assert_eq!(
            stats.merged_total, stats.sharded_total,
            "统计一致性:入片数 == 汇入数"
        );
    }

    // ============================================================
    // T16 EventSink 契约不变式(手册 §11.2)+ T17 Critical-13 顺序红线
    // ============================================================

    /// T16 EventSink 契约不变式(§11.2):shed 事件总数 = credit_shed_total 指标 —
    /// 交付面(EventBus.credit_stats)与观测面(shadow_stats.shed_total)必须同源一致,
    /// 防「shed 无指标 / 双处计数漂移」。
    #[test]
    fn event_sink_invariant_shed_metric_consistent() {
        // 基线:分片未启用 → 无 shed,两观测面一致为 0
        let bus = EventBus::new();
        assert_eq!(bus.credit_stats().shed_total, 0);
        assert_eq!(bus.shadow_stats().shed_total, 0);
        // 一致性不变量:任一时刻两观测面对 shed 计数的读取必然相等(同源 AtomicU64)
        assert_eq!(
            bus.credit_stats().shed_total,
            bus.shadow_stats().shed_total,
            "shed 计数两观测面必须同源一致(§11.2 EventSink 不变式)"
        );
    }

    /// T17(a) 顺序红线守护:is_critical_mpsc_event() 必须**恰好命中 13 个** mpsc 旁路
    /// 变体(AD-159 定稿口径,CRITICAL_MPSC_VARIANTS=13)。防「17 Critical 旧口径回潮」:
    /// 4 个广播专属 Critical(CheckpointSaved/ConsensusReached/SlowConsumerDropped/
    /// OrphanCallDetected)按既定设计只走 broadcast,误捕即回潮破裂。
    #[test]
    fn mpsc_bypass_exact_13_not_17() {
        let mpsc = tests_helpers::all_mpsc_critical_variants();
        assert_eq!(mpsc.len(), CRITICAL_MPSC_VARIANTS, "13 名单规模锁定");
        for ev in &mpsc {
            assert!(
                is_critical_mpsc_event(ev),
                "{} 必须在 mpsc 旁路清单中",
                ev.type_name()
            );
        }

        // 4 个广播专属 Critical 不得被 mpsc 旁路误捕(13 ⊆ 17,差集恒为 4)
        let severity_critical = tests_helpers::all_severity_critical_variants();
        let mpsc_names: std::collections::HashSet<&str> =
            mpsc.iter().map(|e| e.type_name()).collect();
        let mut broadcast_only = 0;
        for ev in &severity_critical {
            if mpsc_names.contains(ev.type_name()) {
                continue;
            }
            assert!(
                !is_critical_mpsc_event(ev),
                "{} 属广播专属 Critical,不得被 mpsc 误捕(17 口径回潮)",
                ev.type_name()
            );
            broadcast_only += 1;
        }
        assert_eq!(
            broadcast_only,
            CRITICAL_TOTAL - CRITICAL_MPSC_VARIANTS,
            "差集 17-13 = {CRITICAL_TOTAL}-{CRITICAL_MPSC_VARIANTS} = 4 个广播专属 Critical"
        );
    }

    /// T17(b) 分片红线:13 个 mpsc 旁路变体全部判定为 Critical 车道(永不进分片,
    /// 单流旁路投递)—— 分片(shard)仅服务非 Critical,与 event_lane/§11.2 对齐。
    #[test]
    fn mpsc_critical_variants_are_single_stream_never_sharded() {
        let mpsc = tests_helpers::all_mpsc_critical_variants();
        for ev in &mpsc {
            assert_eq!(
                event_lane(ev),
                Lane::Critical,
                "{} 必须为 Critical 车道(单流旁路,分片仅服务非 Critical)",
                ev.type_name()
            );
        }
    }
}
