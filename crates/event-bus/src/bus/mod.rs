//! 事件总线实现 — 基于 tokio::broadcast 的 typed broadcast bus
//!
//! 对应架构:L1 Core,所有跨层通信的唯一通道(§2.2 依赖铁律)
//!
//! # 设计要点
//! - 封装 `tokio::broadcast::channel`,提供类型安全的发布订阅
//! - 序列化采用 MessagePack(ADR-004),提供跨进程投递能力
//! - 关键事件标注 Critical,背压策略据此保护(见 backpressure 模块)
//! - 所有 async fn 满足 Send 约束,可被 tokio::spawn

use crate::backpressure::{pressure_latch_next, BackpressurePolicy};
use crate::credit_flow::{CreditFlow, CreditStats};
use crate::critical_sink::{CriticalSink, LogCriticalSink};
use crate::error::EventBusError;
use crate::logging::BusLogger;
use crate::shard::ShardedEventBus;
use crate::types::{EventMetadata, EventSeverity, NexusEvent};
// F-a(M0):ArcSwapOption 承载分片总线单一真值源(取代 Mutex<Option<Arc<_>>> +
// AtomicBool 双表达 —— 双源真相靠手工 Release/Relaxed 配对维持,分叉即灰度时灵时不灵)
use arc_swap::ArcSwapOption;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use tracing::warn;

/// 序列化工具子模块（纯编解码，无总线状态）— ADR-314 W6 / BC-01 按职责拆分
mod codec;
/// 发布面子模块（事件投递路径）— ADR-314 W6 / BC-01 按职责拆分
mod publish;
/// 分片控制面子模块（分片生命周期与路由裁决）— ADR-314 W6 / BC-01 按职责拆分
mod shard_ctl;
/// 订阅面子模块（事件接收路径）— ADR-314 W6 / BC-01 按职责拆分
mod subscribe;

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
/// 守护测试断言旁路清单规模不回退(mpsc 旁路清单 ⊆ severity()==Critical 清单,规模见下常量——注释不抄数字,2026-11-08 X2-N5 订正)。
/// 常量规模本身由 `test_critical_double_list_d8_counts` +
/// `mpsc_bypass_exact_*` + `credit_flow_test` 三处机器锁定;而正文里
/// 「当前 N 类」这类自由文本计数、以及「常量 == `is_critical_mpsc_event` 臂数」
/// 两个子断言,自 2026-10-09 起由 `check_perf_redlines.py` 的 **I8c** 锁死。
/// 此前那是已知盲区(I8/I8b 只校验「N 个 Critical 事件」措辞 == `CRITICAL_TOTAL`,
/// 不校验 mpsc 旁路子数),第三轮 T1.3 因此只能人工把散落的旧值 13/14/17/18
/// 纠为 16/20;I8c 落地当日即抓到一处残留旧值(backpressure_test.rs 注释),
/// 说明该盲区真实会咬人而不是理论风险。
/// L4 深度优化 P1-1:FormalVerificationFailed 升入旁路(13 → 14)——
/// 验证器否决候选若丢失则违规候选继续进入后续阶段,与 FormalViolation 同语义。
/// C-13(M09,2026-10-25):ShadowBreakerTripped 升入旁路(16 → 17)——
/// 熔断跳闸为不可逆 fail-closed 状态变更,丢失则治理面无从感知;升级把
/// 「API 决定旁路」(gsoe 侧 publish_critical 无条件入旁路而定级 Normal)
/// 的失配收敛为「分类决定旁路」的一致语义(ADR-224 D2)。
pub const CRITICAL_MPSC_VARIANTS: usize = 18;

/// severity() 返回 Critical 的变体总数 — D-8 口径
///
/// 总数 = CRITICAL_TOTAL 常量 = CRITICAL_MPSC_VARIANTS(mpsc 旁路)+ 4(历史 Critical 只走 broadcast:
/// CheckpointSaved / ConsensusReached / SlowConsumerDropped / OrphanCallDetected)。
/// 本任务不改变 4 个历史 Critical 事件的通道归属(既定设计,推演 D-8 裁决)。
pub const CRITICAL_TOTAL: usize = 22;

/// 分片禁区声明 — 22 个 Critical 变体名清单(T12 分片时使用)
///
/// WHY 声明但不实现:T12 分片改造(SPSC 环阵列 + 分片总线)将按此清单
/// 禁止把 Critical 单流切片 —— Critical 分片会破坏"发布方 → 订阅方"的
/// 全序投递语义与 mpsc 旁路免背压保证(推演 9:Critical 背压 = 死锁源)。
/// 本任务只声明常量 + 测试守护(断言 22 个名字与 severity() Critical 清单
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
    // L4 深度优化 P1-1:验证器失败升入 Critical(双清单同步)
    "FormalVerificationFailed",
    // v2.3 (b.1)/(b.4): Certify 与 FailureRegistry 进分片禁区(Critical)
    "CertifyIssued",
    "FailureRegistered",
    // C-13(M09): ShadowBreakerTripped 升 Critical 后进分片禁区
    // (定级与旁路双清单同批同步,registry.rs 注册表定级对齐)
    "ShadowBreakerTripped",
    // C-64b：SandboxViolation 升 Critical 后进分片禁区（与定级、旁路同批）
    "SandboxViolation",
];

/// §(b.1) 裁决权外置·消费辅助:供 UI/下游**只读**提取 CertifyIssued 的 claim_ref,
/// 无需在允许白名单外的 crate 里拼写 `NexusEvent::CertifyIssued { field }`(那会被
/// Certify 白名单门误判为构造/签发)。定义于 event-bus(白名单成员),为合法的
/// 封装访问器。非 Certify 事件返回 `None`。
impl NexusEvent {
    /// 若本事件为 `CertifyIssued`,返回其被认证的 `claim_ref`。
    #[must_use]
    pub fn certify_claim_ref(&self) -> Option<&str> {
        match self {
            NexusEvent::CertifyIssued { claim_ref, .. } => Some(claim_ref),
            _ => None,
        }
    }
}

/// 判断事件是否走 mpsc 旁路通道(Critical 安全/治理告警事件)
///
/// §6.2 红线要求:Critical 安全/治理告警事件必须用 mpsc channel
/// 确保送达,避免 broadcast 在 Lagged 场景下丢失。这与 `NexusEvent::severity()`
/// 部分重叠但语义不同:
/// - `severity()` 是事件总线背压级别(同步函数,不依赖运行时值;
///   AsaIntervention 自 P1-W2.1.4 起统一返回 Critical,见 registry.rs 注册表定级)
/// - `is_critical_mpsc_event` 是 mpsc 旁路通道判定,权威清单即下方 matches! 臂,
///   规模锚定 [`CRITICAL_MPSC_VARIANTS`](当前 18)
///
/// WHY 单独定义:AsaIntervention 的 severity() 曾返回 Normal(P1-W2.1.4 修复前),
/// 旁路判定因此独立于 severity() 存在;修复后两清单语义已对齐,由 D-8/R7
/// 互锁测试守护(mpsc 旁路清单 ⊆ severity()==Critical 清单)。
///
/// # 双清单同步红线(MCA M0 起显式声明)
/// 本函数与 `NexusEvent::severity()` 是两张独立清单:新增 Critical 事件
/// **必须同时修改两处**,只改 severity() 会导致"标 Critical 但 broadcast
/// Lagged 时丢失"(旁路不生效)。同步性由**本文件测试模块**三层守护:
/// `test_mpsc_required_events_hit_both_lists`(14 项手抄清单:每一项须**同时**命中
/// 旁路清单与 severity()==Critical —— 断言的是"清单内即两清单皆在",
/// **不是** "severity Critical ⇒ 旁路",后者按设计不成立),
/// `test_critical_double_list_d8_counts`(常量锚定 + LANE_FORBIDDEN_SHARD
/// 双向一一对应)与 R7 互锁断言(清单项 severity() 反查)。
fn is_critical_mpsc_event(event: &NexusEvent) -> bool {
    matches!(
        event,
        NexusEvent::SkepticVeto { .. }
            | NexusEvent::RedTeamAudit { .. }
            | NexusEvent::BudgetExceeded { .. }
            // P1-2:AgentTaskFailed 纳入双清单(severity() 已是 Critical,registry.rs 注册表定级)
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
            // L4 深度优化 P1-1:FormalVerificationFailed 纳入 mpsc 旁路——
            // 验证器否决候选必须确保投递(丢失则违规候选继续进入后续阶段,
            // 与 FormalViolation 同语义;双清单同步红线,registry.rs 定级对齐)
            | NexusEvent::FormalVerificationFailed { .. }
            // v2.3 (b.1)/(b.4): Certify 与 FailureRegistry 走 mpsc 旁路(必达;
            // 双清单同步——registry.rs severity() 已定 Critical,此处补齐旁路清单)
            // 第三轮 T1.3 纠漂:下方全量覆盖断言旧写 14,未计入本两变体 ⇒ 16
            | NexusEvent::CertifyIssued { .. }
            | NexusEvent::FailureRegistered { .. }
            // C-13(M09,2026-10-25): ShadowBreakerTripped 升入 mpsc 旁路(16 → 17)
            // —— 熔断跳闸为不可逆 fail-closed 状态变更,必须确保投递;双清单
            // 同步(registry.rs 定级已升 Critical)。此前 gsoe 侧走 publish_critical
            // 已无条件入旁路(bus.rs send_critical_mpsc),本臂使 decay 侧
            // publish_blocking 的分类路由同样命中旁路,两生产方语义收敛。
            | NexusEvent::ShadowBreakerTripped { .. }
            // C-64b（M49 后半）：沙箱违规为安全类 Critical，拦截事件必须确保投递
            | NexusEvent::SandboxViolation { .. }
    )
}

/// E-2 压力下降采样运行时(滞回 latch + 计数,全原子无锁,ADR-191 D2)
///
/// 状态机真值在 `backpressure::pressure_latch_next` 纯函数(可 proptest);
/// 本结构只存状态不判状态(热路径与可测性分离)。
#[derive(Debug)]
pub(crate) struct DsRuntime {
    /// 降采样参数 (every, trigger_permille, resume_permille);None = 策略未启用
    params: Option<(u64, u64, u64)>,
    /// 压力 latch:0=正常区,1=压力区(滞回态,由纯函数刷新)
    engaged: AtomicU8,
    /// 压力区内 Normal 事件序号(模 every 放行)
    normal_seq: AtomicU64,
    /// 累计被降采样(未入广播)的 Normal 事件数——运维可见性指标
    downsampled: AtomicU64,
}

impl DsRuntime {
    fn disabled() -> Self {
        Self {
            params: None,
            engaged: AtomicU8::new(0),
            normal_seq: AtomicU64::new(0),
            downsampled: AtomicU64::new(0),
        }
    }

    fn from_policy(policy: &BackpressurePolicy) -> Self {
        Self {
            params: policy.downsample_params(),
            engaged: AtomicU8::new(0),
            normal_seq: AtomicU64::new(0),
            downsampled: AtomicU64::new(0),
        }
    }
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
/// 规模锚定 [`CRITICAL_MPSC_VARIANTS`],当前 18 类)额外走 mpsc 旁路通道,
/// 确保在 broadcast Lagged 场景下仍能被订阅者接收。订阅者通过
/// [`subscribe_critical_events`](Self::subscribe_critical_events)
/// 获取 mpsc Receiver。旁路通道按需初始化(首次订阅时创建),无订阅者时
/// `publish` 仅走 broadcast,并投递到保底 sink(B-a,M0;原为 warn 后放弃)。
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
    /// E-2(ADR-191 D2,2026-09-26)压力下降采样运行时状态
    ///
    /// WHY Arc<DsRuntime> + 原子字段:滞回 latch 与计数器必须跨 Clone 副本
    /// 共享(同一总线一个压力状态机);全部操作为 Relaxed 原子,无锁不跨
    /// await(§4.4 红线 1);默认(非降采样)策略下 params=None,热路径仅
    /// 一次 Option 判空即早退,行为与性能零变更。
    ds: Arc<DsRuntime>,
    /// Critical 通道累计丢弃事件数(P1-W2.1 优先级采样丢弃策略)
    ///
    /// WHY Arc<AtomicU64> 而非 Mutex<u64>:
    /// - 避免 §4.4 红线 1 "持锁跨 await"(AtomicU64 无锁,store/load 不跨 await)
    /// - 单调递增,不重置(运维观察累计丢弃趋势)
    /// - Relaxed 内存序:丢弃计数为统计指标,非控制流信号,无需强一致性
    critical_dropped_count: Arc<AtomicU64>,
    /// Critical 保底送达 sink(B-a,M0) — 空订阅者分支的投递目标
    ///
    /// WHY Arc<dyn CriticalSink>:
    /// - `EventBus` 派生 `Clone`,sink 须跨线程共享(可插拔 + 默认实现替换)
    /// - 默认 `LogCriticalSink`(结构化 error! 日志),使**任何** `EventBus::new()`
    ///   路径零接线即获得 Critical 保底落点 —— 修复 8 条 ephemeral 命令路径
    ///   "旁路无订阅者 → warn + 放弃"的送达缺口(覆盖率 1/9 → 9/9)
    /// - at-least-once 语义:并发交错下与 mpsc 投递可能重复,保底通道重复无害
    ///   (见 critical_sink 模块文档"语义边界")
    critical_fallback: Arc<dyn CriticalSink>,
    /// Critical 空订阅者保底投递累计次数(B-a 观测指标)
    ///
    /// WHY 单独计数而非复用 critical_dropped_count:两者语义不同 ——
    /// dropped = 订阅者存在但通道满被丢弃;no_subscriber = 无任何订阅者,
    /// 事件走了保底 sink。前者是容量问题,后者是接线覆盖问题,运维处置不同。
    critical_no_subscriber_count: Arc<AtomicU64>,
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
    /// WHY Arc<ArcSwapOption<ShardedEventBus>>(F-a 收敛,2026-09-12):
    /// - **单一真值源**:原实现为 `Mutex<Option<Arc<_>>>`(权威)+ `AtomicBool`
    ///   (快速路径缓存)双表达,靠写端手工 Release / 读端 Relaxed 配对维持一致
    ///   —— 双源真相一旦漏写一处分叉,症状是"分片灰度时灵时不灵",极难复现。
    ///   ArcSwapOption 以一次无锁读(~5ns)同时提供"是否已启用"(`load().is_some()`,
    ///   替代 AtomicBool)与"总线本体"(`load_full()`,替代 Mutex 锁内 clone),
    ///   双表达合而为一,内存序由 arc-swap 内部保证(正确性不再依赖手工配对);
    /// - 灰度开关语义不变:默认 `None`(不分片),`enable_sharding` 首次调用时
    ///   注册并 spawn worker(写端幂等由 [`Self::enable_lock`] 门闩串行化);
    /// - 外层 `Arc` 使 EventBus 保留 Clone 派生(所有 Clone 副本共享同一分片总线);
    /// - 内部 `Arc<ShardedEventBus>`:worker 任务持有分片总线所有权
    ///   (独立于 EventBus 生命周期)。
    shard_bus: Arc<ArcSwapOption<ShardedEventBus>>,
    /// enable_sharding 写端串行化门闩(F-a,DCL 写端)
    ///
    /// WHY 仅一把 `Mutex<()>` 而非原 `Mutex<Option<Arc<_>>>`:本锁**不存数据**,
    /// 只串行化 enable 的"检查-构造-spawn-注册"四步(低频一次性路径,竞争仅
    /// 发生在并发首次启用时);数据真值源唯一归属 `shard_bus`(读端无锁)。
    /// 读端热路径永不触碰本锁 —— 与原"AtomicBool 快速路径"等价的无锁读体验。
    enable_lock: Arc<Mutex<()>>,
    /// Critical 事件发布计数(影子双跑前哨 `critical_total`)
    ///
    /// WHY Arc<AtomicU64>:EventBus 派生 Clone,所有副本共享同一计数器;
    /// 无条件统计(分片启用与否均递增),供 T13 前哨观测 Critical 车道流量。
    critical_total: Arc<AtomicU64>,
    /// 分片 opt-in 开关(性能重构 D1)—— 默认 `true`(保持既有 leaf
    /// `try_enable_sharding` 行为)。`without_sharding()` 置 false 后,leaf 的
    /// `try_enable_sharding` 在未被 `CHIMERA_BUS_SHARDS` 显式覆盖时早退,
    /// 避免 one-shot 命令为 64 个常驻 worker 付费。
    ///
    /// WHY Arc<AtomicBool> + Relaxed:EventBus 派生 Clone,开关须跨副本共享;
    /// 仅在低频的 try_enable_sharding 入口读一次(非 publish 热路径),
    /// Relaxed 内存序足够(无跨变量同步需求)。
    sharding_allowed: Arc<AtomicBool>,
    /// 分片 worker 生命周期 guard(性能重构 D2)—— 持有 `spawn_workers`
    /// 返回的 `JoinHandle`,最后一个 EventBus clone drop 时 abort worker,
    /// 根治"worker 持强引用且无退出条件"导致的泄漏(与 composition_root_bench
    /// 的 OOM 根因同源)。`Arc<OnceLock<..>>` 使 Clone 共享且仅触发一次 drop。
    shard_guard: Arc<OnceLock<ShardWorkerGuard>>,
}

/// 分片 worker 生命周期 guard（性能重构 D2）。
///
/// 持有 [`spawn_workers`](ShardedEventBus::spawn_workers) 返回的全部 `JoinHandle`。
/// 当最后一个 [`EventBus`] clone drop（即 `Arc<OnceLock<ShardWorkerGuard>>` 归零）时，
/// `Drop` abort 所有 worker，释放其持有的 `broadcast::Sender` 克隆与 `Arc<Notify>`——
/// 根治"worker 持强引用且无退出条件"导致 `ShardedEventBus`/队列内存永不回收的泄漏。
///
/// [`shutdown_sharding`](EventBus::shutdown_sharding) 会先把句柄 `take_handles` 出来
/// await（优雅排空）；届时 `Vec` 已空，`Drop` 不再 abort。两条路径互斥且幂等。
struct ShardWorkerGuard {
    handles: Mutex<Vec<JoinHandle<()>>>,
}

impl ShardWorkerGuard {
    fn new(handles: Vec<JoinHandle<()>>) -> Self {
        Self {
            handles: Mutex::new(handles),
        }
    }

    /// 取走全部尚未被取走的句柄（取走后 `Drop` 不会再 abort 它们）。
    fn take_handles(&self) -> Vec<JoinHandle<()>> {
        let mut guard = self.handles.lock().unwrap_or_else(|p| p.into_inner());
        std::mem::take(&mut *guard)
    }
}

impl Drop for ShardWorkerGuard {
    fn drop(&mut self) {
        let mut guard = self.handles.lock().unwrap_or_else(|p| p.into_inner());
        for handle in std::mem::take(&mut *guard) {
            handle.abort();
        }
    }
}

impl EventBus {
    /// 创建事件总线,使用默认容量(1024),不启用日志埋点
    ///
    /// 新代码优先用 [`EventBus::builder`](Self::builder)。
    pub fn new() -> Self {
        Self::builder().build()
    }

    /// 创建事件总线,指定通道容量,不启用日志埋点
    ///
    /// 新代码优先用 [`EventBus::builder`](Self::builder)。
    pub fn with_capacity(capacity: usize) -> Self {
        Self::builder().capacity(capacity).build()
    }

    /// 创建事件总线,指定通道容量并启用日志埋点
    ///
    /// `logger` 会被包装在 Arc 中,Clone 时共享同一计数器。
    ///
    /// 新代码优先用 [`EventBus::builder`](Self::builder)。
    pub fn with_logger(capacity: usize, logger: BusLogger) -> Self {
        Self::builder().capacity(capacity).logger(logger).build()
    }

    /// 创建启用指定背压策略的事件总线(E-2,ADR-191 D2)
    ///
    /// 容量取 `policy.broadcast_capacity()`;`DownsampleUnderPressure` 策略在
    /// 占用进入压力区(千分比滞回,见 `backpressure::pressure_latch_next`)后
    /// 对非 Critical 事件按「每 every 条广播 1 条」降采样;其余策略行为与
    /// `with_capacity` 完全一致(零变更)。
    ///
    /// 新代码优先用 [`EventBus::builder`](Self::builder)。
    ///
    /// # 参数
    /// - `policy`:背压策略(默认 `LagThreshold` 即历史行为)
    pub fn with_policy(policy: BackpressurePolicy) -> Self {
        Self::builder().policy(policy).build()
    }

    /// 构造 [`EventBusBuilder`] — **新代码优先使用 builder 构造总线**
    ///
    /// WHY(R4.1):EventBus 有约 18 个 Arc 字段,原 3 个构造函数各持一份
    /// 近 20 行字段初始化(复制粘贴,新增字段须同步 N 处)。builder 把全字段
    /// 初始化收敛为唯一落点 [`EventBusBuilder::build`],旧构造函数转为 thin
    /// wrapper 经此路径(字段默认值与构造语义零变化,不标 deprecated)。
    pub fn builder() -> EventBusBuilder {
        EventBusBuilder {
            capacity: DEFAULT_CAPACITY,
            logger: None,
            policy: None,
            critical_fallback: None,
            sharding_allowed: true,
        }
    }

    /// E-2 降采样判定与状态机刷新(热路径单点)。
    ///
    /// 返回 true = 本事件应入广播;false = 已降采样(调用方跳过 send 并递增计数)。
    /// 关键不变量(由纯函数+此处结构保证,均有 proptest 覆盖):
    /// - 非降采样策略或 latch 未锁存 → 永远 true(默认路径零行为)
    /// - Critical 事件 → 永远 true(红线:降级不得伤害关键告警投递)
    /// - 锁存期内严格按 seq % every == 0 放行,间隔恒定 = every
    ///
    /// 借用注意:`event` 在本函数后仍被 send 消费,故只收 severity/借用标志。
    #[inline]
    pub(super) fn ds_admit(&self, severity: EventSeverity, queued: usize) -> bool {
        let Some((every, trigger, resume)) = self.ds.params else {
            return true;
        };
        // latch 刷新:每次 publish 采样点同步(与 A3 告警共用 queued,零额外原子)
        let prev = self.ds.engaged.load(Ordering::Relaxed) == 1;
        let next = pressure_latch_next(prev, queued, self.capacity, trigger, resume);
        if next != prev {
            self.ds.engaged.store(next as u8, Ordering::Relaxed);
        }
        if !next || severity == EventSeverity::Critical {
            return true;
        }
        // 压力区内:Normal/其它非 Critical 按模 every 放行
        self.ds
            .normal_seq
            .fetch_add(1, Ordering::Relaxed)
            .is_multiple_of(every)
    }

    /// E-2 累计被降采样的事件数(运维可见性;默认策略下恒 0)
    #[must_use]
    pub fn downsampled_total(&self) -> u64 {
        self.ds.downsampled.load(Ordering::Relaxed)
    }

    /// E-2 当前是否处于压力区(latch 锁存态;默认策略下恒 false)
    #[must_use]
    pub fn pressure_engaged(&self) -> bool {
        self.ds.engaged.load(Ordering::Relaxed) == 1
    }

    /// 替换 Critical 保底送达 sink(B-a,M0,builder 风格)
    ///
    /// 默认 sink 为 [`LogCriticalSink`](crate::critical_sink::LogCriticalSink)
    /// (结构化 `error!` 日志);需要告警管道/持久化等自定义落点时,以
    /// `Arc<dyn CriticalSink>` 注入替换。仅影响"无 mpsc 订阅者"分支的投递目标,
    /// 既有 mpsc 订阅语义(`subscribe_critical_events`)完全不变。
    ///
    /// # 参数
    /// - `sink`:保底落点实现(`Send + Sync + 'static`,内部 `Arc` 共享)
    ///
    /// # 返回
    /// 消费 `self` 并返回,支持链式调用:`EventBus::new().with_critical_fallback(..)`
    ///
    /// # 示例
    /// ```no_run
    /// use std::sync::Arc;
    /// use event_bus::{EventBus, CriticalSink, NexusEvent};
    ///
    /// struct MySink;
    /// impl CriticalSink for MySink {
    ///     fn on_critical(&self, _event: &NexusEvent) { /* 转发告警管道 */ }
    /// }
    /// let bus = EventBus::new().with_critical_fallback(Arc::new(MySink));
    /// ```
    pub fn with_critical_fallback(mut self, sink: Arc<dyn CriticalSink>) -> Self {
        self.critical_fallback = sink;
        self
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

    /// 当前订阅者数量(broadcast 接收端计数)
    ///
    /// WHY 观测面:发布方据此判断"事件是否有人听"(无订阅者时 Critical 事件
    /// 仍走保底 sink,见 [`with_critical_fallback`](Self::with_critical_fallback))。
    ///
    /// 注:本方法的 rustdoc 曾误挂 `publish` 的说明(拆分时发现的历史文档错位,
    /// 已就地纠正);`publish` 的说明现位于 `bus/publish.rs::publish`。
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

/// EventBus 构造器(R4.1)—— 全字段初始化的唯一落点
///
/// 字段默认值与 `EventBus::new()` 语义完全一致;链式方法只覆盖既有构造路径
/// 可表达的维度(容量/日志/背压策略/保底 sink/分片开关),不新增能力。
/// `build()` 之外不存在其它字段初始化代码 —— 新增 EventBus 字段时只需改
/// `build()` 一处(旧构造函数均为 wrapper,自动继承)。
///
/// 新代码优先用 `EventBus::builder()` 替代 `new`/`with_capacity`/`with_logger`/`with_policy`。
pub struct EventBusBuilder {
    capacity: usize,
    logger: Option<BusLogger>,
    policy: Option<BackpressurePolicy>,
    critical_fallback: Option<Arc<dyn CriticalSink>>,
    sharding_allowed: bool,
}

impl EventBusBuilder {
    /// 通道容量(默认 [`DEFAULT_CAPACITY`])
    ///
    /// 与 [`policy`](Self::policy) 同设时以 `policy.broadcast_capacity()` 为准
    /// (对齐既有 `with_policy` 语义:策略自带容量)。
    pub fn capacity(mut self, n: usize) -> Self {
        self.capacity = n;
        self
    }

    /// 启用日志埋点(包装为 `Arc<BusLogger>`,Clone 共享同一计数器)
    pub fn logger(mut self, logger: BusLogger) -> Self {
        self.logger = Some(logger);
        self
    }

    /// 背压策略(E-2,ADR-191 D2)
    ///
    /// 设置后容量取 `policy.broadcast_capacity()`、降采样按策略启用;
    /// 未设置时行为与默认策略(`LagThreshold`)完全一致。
    pub fn policy(mut self, policy: BackpressurePolicy) -> Self {
        self.policy = Some(policy);
        self
    }

    /// 替换 Critical 保底送达 sink(默认 [`LogCriticalSink`],B-a/M0)
    ///
    /// 仅影响「无 mpsc 订阅者」分支的投递目标,语义同
    /// [`EventBus::with_critical_fallback`](EventBus::with_critical_fallback)。
    pub fn critical_fallback(mut self, sink: Arc<dyn CriticalSink>) -> Self {
        self.critical_fallback = Some(sink);
        self
    }

    /// 分片 opt-in 开关(默认 true;false 对齐 [`EventBus::without_sharding`] 语义,
    /// 避免 one-shot 命令为常驻分片 worker 付费)
    pub fn sharding_allowed(mut self, allowed: bool) -> Self {
        self.sharding_allowed = allowed;
        self
    }

    /// 构建 [`EventBus`] — 全字段初始化的唯一落点(旧构造函数经 wrapper 汇入此路径)
    pub fn build(self) -> EventBus {
        // policy 优先推导容量与 ds 运行时(对齐原 with_policy);默认策略走显式 capacity
        let (capacity, ds) = match &self.policy {
            Some(policy) => (policy.broadcast_capacity(), DsRuntime::from_policy(policy)),
            None => (self.capacity, DsRuntime::disabled()),
        };
        let (sender, _) = broadcast::channel(capacity);
        EventBus {
            sender,
            capacity,
            logger: self.logger.map(Arc::new),
            critical_tx: Arc::new(Mutex::new(Vec::new())),
            ds: Arc::new(ds),
            critical_dropped_count: Arc::new(AtomicU64::new(0)),
            // B-a(M0):默认保底 sink = 结构化 error! 日志(可经 critical_fallback 替换)
            critical_fallback: self
                .critical_fallback
                .unwrap_or_else(|| Arc::new(LogCriticalSink)),
            critical_no_subscriber_count: Arc::new(AtomicU64::new(0)),
            lagged_count: Arc::new(AtomicU64::new(0)),
            backpressure_warning_count: Arc::new(AtomicU64::new(0)),
            published_total: Arc::new(AtomicU64::new(0)),
            // P1-T11:默认信用池 256(手册 §8.5 初始值)
            credit_flow: Arc::new(CreditFlow::new()),
            credit_shed_total: Arc::new(AtomicU64::new(0)),
            // P1-T12 分片灰度:默认不启用(零回归,与 v2.27.1 行为完全一致);
            // F-a:ArcSwapOption 单一真值源(empty = 未启用) + enable 门闩
            shard_bus: Arc::new(ArcSwapOption::empty()),
            enable_lock: Arc::new(Mutex::new(())),
            critical_total: Arc::new(AtomicU64::new(0)),
            sharding_allowed: Arc::new(AtomicBool::new(self.sharding_allowed)),
            shard_guard: Arc::new(OnceLock::new()),
        }
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
        // 第三轮 T1.1:shadow_stats 的首个生产消费者 —— 零订阅丢弃以窗口限速
        // 浮出(≤1 warn/窗口;worker 侧仅首见 warn 一次,长期静默会掩盖持续增长)
        let mut prev_closed = bus.shadow_stats().merge_closed_total;
        loop {
            interval.tick().await;
            let now = bus.published_total();
            let events_per_sec = (now.saturating_sub(prev)) as f64 / window_secs as f64;
            prev = now;
            let closed_now = bus.shadow_stats().merge_closed_total;
            if closed_now > prev_closed {
                warn!(
                    window_delta = closed_now - prev_closed,
                    merge_closed_total = closed_now,
                    "分片汇入零订阅者丢弃增长:merged_total 含此类丢弃,送达观测见 ShadowStats"
                );
                prev_closed = closed_now;
            }
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
// 序列化工具 — 已迁至 `bus/codec.rs`（ADR-314 W6 / BC-01 按职责拆分）
// WHY 保留 `pub use` 转出：外部路径 `event_bus::bus::serialize_msgpack`
// 逐字不变（零消费方 diff），内部实现归位到纯编解码模块。
// ============================================================
pub use codec::{deserialize_json, deserialize_msgpack, serialize_json, serialize_msgpack};

// ============================================================
// 测试(R4.4):tests_helpers + tests 整体下迁至 bus/tests.rs
// ============================================================

/// 测试子模块(发布/订阅/分片/双清单守护)—— R4.4 起整体位于 `bus/tests.rs`,
/// 此处仅保留模块声明,使 bus/mod.rs 生产码一屏可读。
#[cfg(test)]
mod tests;

// WHY re-export 而非把 tests_helpers 留在本文件:shard.rs 的 Lane 全量断言按
// `crate::bus::tests_helpers` 路径引用 D-8 构造辅助(shard.rs 不在本批编辑范围);
// 整体迁移后须经此 re-export 保持原路径可达(re-export 与直接声明对引用方等价)。
#[cfg(test)]
pub(crate) use tests::tests_helpers;
