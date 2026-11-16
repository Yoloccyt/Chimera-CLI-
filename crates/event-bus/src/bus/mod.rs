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
/// 订阅面子模块（事件接收路径）— ADR-314 W6 / BC-01 按职责拆分
mod subscribe;
/// 分片控制面子模块（分片生命周期与路由裁决）— ADR-314 W6 / BC-01 按职责拆分
mod shard_ctl;

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
            ds: Arc::new(DsRuntime::disabled()),
            critical_dropped_count: Arc::new(AtomicU64::new(0)),
            // B-a(M0):默认保底 sink = 结构化 error! 日志(可经 with_critical_fallback 替换)
            critical_fallback: Arc::new(LogCriticalSink),
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
            // 性能重构 D1/D2：默认允许分片 opt-in + 空 worker guard（与既有行为零变更）
            sharding_allowed: Arc::new(AtomicBool::new(true)),
            shard_guard: Arc::new(OnceLock::new()),
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
            ds: Arc::new(DsRuntime::disabled()),
            critical_dropped_count: Arc::new(AtomicU64::new(0)),
            // B-a(M0):默认保底 sink = 结构化 error! 日志(可经 with_critical_fallback 替换)
            critical_fallback: Arc::new(LogCriticalSink),
            critical_no_subscriber_count: Arc::new(AtomicU64::new(0)),
            lagged_count: Arc::new(AtomicU64::new(0)),
            backpressure_warning_count: Arc::new(AtomicU64::new(0)),
            published_total: Arc::new(AtomicU64::new(0)),
            // P1-T11:与 with_capacity 一致的信用池初始化
            credit_flow: Arc::new(CreditFlow::new()),
            credit_shed_total: Arc::new(AtomicU64::new(0)),
            // P1-T12 分片灰度:默认不启用(零回归);F-a:单一真值源 + enable 门闩
            shard_bus: Arc::new(ArcSwapOption::empty()),
            enable_lock: Arc::new(Mutex::new(())),
            critical_total: Arc::new(AtomicU64::new(0)),
            // 性能重构 D1/D2：默认允许分片 opt-in + 空 worker guard（与既有行为零变更）
            sharding_allowed: Arc::new(AtomicBool::new(true)),
            shard_guard: Arc::new(OnceLock::new()),
        }
    }

    /// 创建启用指定背压策略的事件总线(E-2,ADR-191 D2)
    ///
    /// 容量取 `policy.broadcast_capacity()`;`DownsampleUnderPressure` 策略在
    /// 占用进入压力区(千分比滞回,见 `backpressure::pressure_latch_next`)后
    /// 对非 Critical 事件按「每 every 条广播 1 条」降采样;其余策略行为与
    /// `with_capacity` 完全一致(零变更)。
    ///
    /// # 参数
    /// - `policy`:背压策略(默认 `LagThreshold` 即历史行为)
    pub fn with_policy(policy: BackpressurePolicy) -> Self {
        let capacity = policy.broadcast_capacity();
        let (sender, _) = broadcast::channel(capacity);
        Self {
            sender,
            capacity,
            logger: None,
            critical_tx: Arc::new(Mutex::new(Vec::new())),
            ds: Arc::new(DsRuntime::from_policy(&policy)),
            critical_dropped_count: Arc::new(AtomicU64::new(0)),
            critical_fallback: Arc::new(LogCriticalSink),
            critical_no_subscriber_count: Arc::new(AtomicU64::new(0)),
            lagged_count: Arc::new(AtomicU64::new(0)),
            backpressure_warning_count: Arc::new(AtomicU64::new(0)),
            published_total: Arc::new(AtomicU64::new(0)),
            credit_flow: Arc::new(CreditFlow::new()),
            credit_shed_total: Arc::new(AtomicU64::new(0)),
            shard_bus: Arc::new(ArcSwapOption::empty()),
            enable_lock: Arc::new(Mutex::new(())),
            critical_total: Arc::new(AtomicU64::new(0)),
            // 性能重构 D1/D2：默认允许分片 opt-in + 空 worker guard（与既有行为零变更）
            sharding_allowed: Arc::new(AtomicBool::new(true)),
            shard_guard: Arc::new(OnceLock::new()),
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
pub use codec::{
    deserialize_json, deserialize_msgpack, serialize_json, serialize_msgpack,
};

// ============================================================
// P1-T12:测试辅助 — Critical 变体构造(D-8 口径,双清单同步红线)
// ============================================================

/// Critical 变体构造测试辅助(D-8 口径)
///
/// WHY pub(crate):shard.rs 的 Lane 判定全量断言(21 Critical → Critical)
/// 复用本模块构造事件,避免两处维护 22 个变体构造代码漂移(新增 Critical
/// 事件时只需改此处一处,三处清单同步守护见 test_critical_double_list_d8_counts)。
/// 仅测试构建存在(cfg(test)),生产零足迹。
#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
pub(crate) mod tests_helpers {
    use super::*;

    /// 全量 18 个 mpsc 旁路变体构造(D-8 口径,双清单同步红线)
    pub fn all_mpsc_critical_variants() -> Vec<NexusEvent> {
        vec![
            // C-64b（M49 后半）：沙箱违规升 Critical 后进 mpsc 旁路
            NexusEvent::SandboxViolation {
                metadata: EventMetadata::new("t"),
                violation_type: "path_traversal".into(),
                detail: "attempted /etc/passwd".into(),
            },
            // v2.3 (b.1)/(b.4): Certify 与 FailureRegistry 进 mpsc 旁路清单(D-8 口径)
            NexusEvent::CertifyIssued {
                metadata: EventMetadata::new("t"),
                claim_ref: "c".into(),
                subject: "s".into(),
                verifier: "GovernanceGate".into(),
            },
            NexusEvent::FailureRegistered {
                metadata: EventMetadata::new("t"),
                signature: "sig".into(),
                affected_component: "crate".into(),
                fix_status: "Open".into(),
            },
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
            // L4 深度优化 P1-1:验证器失败升入 mpsc 旁路(双清单同步)
            NexusEvent::FormalVerificationFailed {
                metadata: EventMetadata::new("t"),
                property: "inv-1".into(),
                counterexample: "s".into(),
                generation: 1,
            },
            // C-13(M09): ShadowBreakerTripped 升入 mpsc 旁路(16 → 17,双清单同步)
            NexusEvent::ShadowBreakerTripped {
                metadata: EventMetadata::new("t"),
                reason: "r".into(),
            },
        ]
    }

    /// 全量 22 个 severity() Critical 变体构造(D-8 口径)
    ///
    /// 22 = 18(mpsc 旁路,见 [`all_mpsc_critical_variants`])
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
    /// 名字不在 22 清单中返回 None(调用方断言消息定位漂移名字)。
    pub fn critical_variant_by_name(name: &str) -> Option<NexusEvent> {
        all_severity_critical_variants()
            .into_iter()
            .find(|e| e.type_name() == name)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::shard::{event_lane, Lane, ShadowStats, DEFAULT_SHARD_COUNT};
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

    // ============================================================
    // 方向 3:单事件发布路径去重 — 两变体共享 dispatch_one 的等价守卫
    // ============================================================

    fn make_critical_event() -> NexusEvent {
        NexusEvent::SkepticVeto {
            metadata: EventMetadata::new("parliament"),
            quest_id: "q-veto".into(),
            veto_reason: "等价性守卫".into(),
            frozen_capabilities: vec!["cap-x".into()],
        }
    }

    /// `publish` 与 `publish_blocking` 必须行为等价(共享同一实现)
    ///
    /// # WHY 需要该用例
    /// 重构前这两条路径是约 88 行**逐行复制粘贴**的独立实现,日志文案的
    /// "(同步发布)" 后缀是唯一差异。任何发布逻辑改动都必须同步改两处,漏改即
    /// **静默分叉** —— 两条路径行为不再等价却无人察觉。本用例把"两路径等价"
    /// 固化为契约:若未来有人再次各写一份并只改其中一处,这里会红。
    #[tokio::test]
    async fn test_publish_and_blocking_are_equivalent() {
        let bus_async = EventBus::new();
        let mut rx_async = bus_async.subscribe();
        let bus_sync = EventBus::new();
        let mut rx_sync = bus_sync.subscribe();

        let before_async = bus_async.published_total();
        let before_sync = bus_sync.published_total();

        bus_async.publish(make_test_event()).await.unwrap();
        bus_sync.publish_blocking(make_test_event()).unwrap();

        // 1) 吞吐计数点一致:两条路径递增同一计数器
        assert_eq!(bus_async.published_total(), before_async + 1);
        assert_eq!(bus_sync.published_total(), before_sync + 1);

        // 2) 两条路径都实际投递到 broadcast 订阅者
        assert!(rx_async.recv().await.is_ok(), "async 路径应投递事件");
        assert!(rx_sync.recv().await.is_ok(), "blocking 路径应投递事件");
    }

    /// Critical 事件在两条路径上都必须走 mpsc 旁路(§6.2 红线双通道)
    ///
    /// 这是"共享实现"最容易被分叉破坏的可观测点:旁路投递若只在其中一条路径上
    /// 被漏掉,Critical 治理事件会在 broadcast Lagged 时永久丢失。
    #[tokio::test]
    async fn test_critical_bypass_present_on_both_publish_paths() {
        let bus_async = EventBus::new();
        let mut crit_async = bus_async.subscribe_critical_events();
        let bus_sync = EventBus::new();
        let mut crit_sync = bus_sync.subscribe_critical_events();

        bus_async.publish(make_critical_event()).await.unwrap();
        bus_sync.publish_blocking(make_critical_event()).unwrap();

        let got_async = tokio::time::timeout(Duration::from_millis(200), crit_async.recv())
            .await
            .expect("async 路径不应超时")
            .expect("async 路径应走 mpsc 旁路");
        let got_sync = tokio::time::timeout(Duration::from_millis(200), crit_sync.recv())
            .await
            .expect("blocking 路径不应超时")
            .expect("blocking 路径应走 mpsc 旁路");

        assert_eq!(got_async.type_name(), "SkepticVeto");
        assert_eq!(got_sync.type_name(), "SkepticVeto");
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
    fn test_mpsc_required_events_hit_both_lists() {
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
            // L4 深度优化 P1-1(+1):验证器失败升入旁路(与 registry.rs 定级同步)
            NexusEvent::FormalVerificationFailed {
                metadata: EventMetadata::new("t"),
                property: "inv-1".into(),
                counterexample: "s".into(),
                generation: 1,
            },
        ];
        assert_eq!(
            mpsc_required.len(),
            14,
            "本安全/资源子集 14 个必投事件须齐全(权威全量 16 见 test_critical_double_list_d8_counts;\n             此为 MCA M0 子集不回退锁,非全量清单——第三轮 T1.3 注:旧误称'全量')"
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

    /// D-8 双清单计数守护(P1-T11):CRITICAL_MPSC_VARIANTS、
    /// CRITICAL_TOTAL、mpsc ⊆ Critical、LANE_FORBIDDEN_SHARD 一一对应
    /// (常量真值见 :62/:69 两侧定义——注释不再抄数字,防 M44 同型漂移;2026-11-08 X2-N5 订正)
    /// (第三轮 T1.3 纠漂:旧注 14/18 为 v2.3 补入 CertifyIssued/FailureRegistered
    ///  前的滞后值;断言体以常量为准故恒绿,漂移仅存于此注 —— I8 盲区见上方常量注)
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

        // 18 个 mpsc 变体:is_critical_mpsc_event 必中 + severity() Critical
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
        // 22 个 Critical 变体:severity() 全部为 Critical(21 口径)
        for event in &critical_variants {
            assert_eq!(
                event.severity(),
                EventSeverity::Critical,
                "{} 应命中 severity() Critical 清单(21 口径)",
                event.type_name()
            );
        }

        // 17 ⊆ 21:每个 mpsc 变体的名字必须在 severity() Critical 清单中
        let mpsc_names: Vec<&str> = mpsc_variants.iter().map(|e| e.type_name()).collect();
        let critical_names: Vec<&str> = critical_variants.iter().map(|e| e.type_name()).collect();
        for name in &mpsc_names {
            assert!(
                critical_names.contains(name),
                "mpsc 变体 {name} 不在 severity() Critical 清单中(17 ⊆ 21 违反)"
            );
        }

        // LANE_FORBIDDEN_SHARD 分片禁区声明:22 个名字与 Critical 清单一一对应
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
    // 漂移都触发红灯。权威源:severity() 显式臂(registry.rs) + L44-49 注释
    // 记载的 4 个历史广播级 Critical。

    /// R7-正向反向断言:手抄双清单每一项必须与 severity() 权威判定一致,
    /// 且规模锚定(d8 常量)不回退。防"清单声称为 Critical 但 registry.rs
    /// 判为 Normal"(被通配 `_ => Normal` 吞掉的静默降级)。
    #[test]
    fn test_critical_lists_are_all_severity_critical() {
        let mpsc_variants = tests_helpers::all_mpsc_critical_variants();
        let severity_variants = tests_helpers::all_severity_critical_variants();

        // 现实值核验:16(mpsc 旁路)/ 20(severity Critical,含 4 个历史广播级)。
        // WHY 锚定常量而非硬编码 16/20:常量是 D-8 裁决的编译期锚点,清单规模
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
        // 一旦 registry.rs severity() 加显式臂时漏在此清单登记,或清单登记了
        // 某个实际被判 Normal 的事件,此断言红灯 —— 反向捕获两只清单漂移方向。
        for event in mpsc_variants.iter().chain(severity_variants.iter()) {
            assert_eq!(
                event.severity(),
                EventSeverity::Critical,
                "{} 声明为 Critical(手抄清单)但 severity() 返回 {:?} → 与 registry.rs 权威判定漂移",
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
                "mpsc 变体 {name} 不在 severity() Critical 清单中(17 ⊆ 21 违反)"
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

    // T-4(ADR-191「日志即契约」):补齐 §7 四可靠性站点中的背压 WARN 站点
    // (bus.rs:416-425)的日志断言。`test_backpressure_monitor_tracks_warnings`
    // 仅断言 counter(背压告警次数),此处断言“缓冲区 >75% 必发 WARN”本身是
    // 运维契约——日志文案漂移即回归。行为触发条件与上测完全一致(容量 8,订
    // 阅者不消费,第 8 次 publish 前 sender.len()==7 > 6=8*3/4)。
    #[tracing_test::traced_test]
    #[tokio::test]
    async fn test_backpressure_warn_emits_log_contract() {
        let bus = EventBus::with_capacity(8);
        let _rx = bus.subscribe(); // 保持订阅者,防止 send 返回 Err
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
        assert!(
            logs_contain("broadcast 通道背压告警"),
            "缓冲区占用 >75% 时应记录背压 WARN 日志(T-4 日志契约)"
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

    /// 第三轮 T1.2 结构判据：`event_lane` 在每条发布路径**只求值一次**。
    ///
    /// WHY 结构断言而非行为断言：`event_lane` 是纯函数，双份求值与单份求值
    /// **行为完全等价**（`recv==sent` 抓不到），回归只会表现为热路径多做的
    /// OrderSensitive `SessionKey` String clone（F-P2）——故只能以源码形态锁定。
    /// 不变式：全文件禁止在 `matches!` 里内联求值 `event_lane(&event)`（计数 0），
    /// 且单发宏 `dispatch_one_impl` 与批量 `publish_batch` 两条路径各恰好
    /// `let lane = event_lane(&event);` 提升一次（计数 2）。新增发布路径须显式更新计数。
    #[test]
    fn dispatch_paths_hoist_lane_exactly_once() {
        // 只扫描生产区（`mod tests {` 之前）——否则 include_str! 会把本测试自己的
        // 字面量（包含待匹配串）也数进去，造成自引用假阳性。
        //
        // WHY 扫「整个 bus 模块的全部子文件」而非单一文件：W6(ADR-314/BC-01)把发布
        // 路径拆到 `bus/publish.rs`（单发宏 + 批量循环），两条路径现在同处该文件；
        // 若未来新增发布路径落在别的子模块，单文件扫描会漏检。逐文件扫描 + 拼接后
        // 统一计数，不变式的覆盖面与拆分前等价（拆分前全在 bus.rs 一个文件里）。
        const BUS_MODULE_FILES: [&str; 5] = [
            "mod.rs",
            "codec.rs",
            "publish.rs",
            "subscribe.rs",
            "shard_ctl.rs",
        ];
        let sources = [
            include_str!("mod.rs"),
            include_str!("codec.rs"),
            include_str!("publish.rs"),
            include_str!("subscribe.rs"),
            include_str!("shard_ctl.rs"),
        ];
        debug_assert_eq!(sources.len(), BUS_MODULE_FILES.len());
        let prod = sources
            .iter()
            .map(|src| src.split("mod tests {").next().unwrap_or(src))
            .collect::<Vec<_>>()
            .join("\n");
        let inline_eval = prod.matches("matches!(event_lane(&event)").count();
        let hoisted = prod.matches("let lane = event_lane(&event);").count();
        assert_eq!(
            inline_eval, 0,
            "禁止在 matches! 内联求值 event_lane（双份求值 = F-P2 热路径 clone 回归）"
        );
        assert_eq!(
            hoisted, 2,
            "两条发布路径（单发宏 + 批量循环）应各提升一次 lane；实测 {hoisted}（新增发布路径请显式复核本计数）"
        );
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
        // 安装的分片总线:worker 恰好一组(评审 Issue 1 的守护本意 = 不重复 spawn)。
        //
        // 性能重构 D2 断言语义调整(2026-10-22):worker 现改持 `Weak<ShardedEventBus>`
        // (根治旧设计"worker 持强引用且无退出条件"的内存泄漏 —— 即 composition_root_bench
        // OOM 根因)。故分片总线的**强引用计数不再随 worker 数增长**:此刻仅 `load_full()`
        // 返回的临时 guard(+ arc-swap 内部至多 1)持有强引用,worker 持 0 份。"不重复
        // spawn" 的精确守护已由上方 `credit_flow == 1 + DEFAULT_SHARD_COUNT` 承担(每个
        // spawn 出的 worker 恰持一份 `Arc<CreditFlow>`,失败路径不 spawn → 计数不翻倍)。
        // 本断言相应反转:强引用计数必须**远小于** worker 数(旧断言是 `> worker 数`),
        // 若未来 worker 重新持强引用(回退泄漏设计)即判红。
        let guard = bus.shard_bus.load_full();
        let sb = guard.as_ref().expect("分片总线必须已安装");
        let sb_count = Arc::strong_count(sb);
        assert!(
            sb_count < DEFAULT_SHARD_COUNT,
            "worker 应只持 Weak 引用,分片总线强计数不得随 worker 数增长(实测 {sb_count})"
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

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn without_sharding_blocks_opt_in_and_propagates_to_clones() {
        // 性能重构 D1：without_sharding 后 leaf 的 try_enable_sharding 应早退
        // （即使有 runtime，否则 Handle::try_current 会成功 → 真启用）。对照总线可启用。
        let bus = EventBus::new().without_sharding();
        let clone = bus.clone();
        assert!(
            !bus.try_enable_sharding("test-blocked"),
            "without_sharding 应屏蔽 leaf opt-in"
        );
        assert!(!bus.sharding_enabled());
        assert!(
            !clone.sharding_enabled(),
            "开关应跨 Clone 共享（同一 Arc<AtomicBool>）"
        );
        // 对照：未关闭的总线在同 runtime 下 try_enable 应成功
        let open = EventBus::new();
        assert!(open.try_enable_sharding("test-open"), "默认允许 opt-in");
        assert!(open.sharding_enabled());
        open.shutdown_sharding().await; // 清理：避免 worker 泄漏到测试 runtime
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shard_workers_terminable_after_bus_drop() {
        // 性能重构 D2（漏发内存泄漏回归守卫）：worker 仅持 Weak<ShardedEventBus>，
        // 故最后一个 EventBus clone drop 后，分片总线必须被回收（旧设计 worker 持强
        // 引用 → 永不回收 → composition_root_bench OOM）。用 Weak 直接观测，不依赖
        // 全局计数，对并行测试鲁棒。
        let weak_sb;
        {
            let bus = EventBus::new();
            bus.enable_sharding(8).unwrap();
            weak_sb = Arc::downgrade(bus.shard_bus.load_full().as_ref().expect("已安装分片总线"));
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            assert!(weak_sb.upgrade().is_some(), "投递期总线必须存活");
        } // bus drop → 最后一个强引用释放（worker 持 Weak，不阻止回收）
        for _ in 0..100 {
            if weak_sb.upgrade().is_none() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(
            weak_sb.upgrade().is_none(),
            "EventBus drop 后 ShardedEventBus 必须被回收（worker 应仅持 Weak）"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn shutdown_sharding_drains_queued_events_no_loss() {
        // 性能重构 D2 优雅关闭：shutdown_sharding 应排空已入片但未汇入的事件，
        // 漏发率 = 0，且订阅者能收到全部事件。
        // 注：shutdown 会 store(None) 释放分片总线，故计数器必须在 shutdown **前**读 sharded_total，
        // 汇入完整性在 shutdown **后**经订阅者侧验证（而非读已释放的计数器）。
        let bus = EventBus::new();
        bus.enable_sharding(4).unwrap();
        let mut rx = bus.subscribe();
        const N: usize = 50;
        for _ in 0..N {
            bus.publish(make_test_event()).await.unwrap();
        }
        // 入片计数在 publish 路径同步递增（try_push 内 fetch_add）
        assert_eq!(
            bus.shadow_stats().sharded_total,
            N as u64,
            "容量内事件应全部入片"
        );
        // 优雅排空：唤醒 worker 将其片 drain-to-empty 汇入 broadcast 后退出
        bus.shutdown_sharding().await;
        let received = drain_events(&mut rx, N, 1000).await;
        assert_eq!(
            received.len(),
            N,
            "shutdown 后订阅者应收到全部已入片事件（漏发率 = 0）"
        );
    }

    /// P1-1 并发排空：`shutdown_sharding()` 与持续 `publish(Unordered)` 重叠时，
    /// **不得**出现「入片成功但永不汇入」的静默丢事件。
    ///
    /// 判定口径选「订阅者实收数 == 发布成功数」而非计数器：
    /// shutdown 后 `shadow_stats()` 随 `store(None)` 归零，计数器不可读；
    /// 而丢事件必然表现为订阅者少收（未汇入 broadcast）。
    ///
    /// 改前失效链：worker 见 `draining && is_empty()` 即 break，而 `try_push`
    /// 无 draining 判定、槽位仍在（`store(None)` 在 await 之后）—— break 后入片的
    /// 事件返回 Ok 却永不汇入。
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_publish_and_shutdown_loses_nothing() {
        const ROUNDS: usize = 10;
        const PRODUCERS: usize = 8;
        const N: usize = 400;

        for round in 0..ROUNDS {
            // 容量远大于 N：避免慢消费者 Lagged 造成假丢
            let bus = EventBus::with_capacity(16_384);
            bus.enable_sharding(8).expect("启用分片应成功");
            let mut rx = bus.subscribe();

            let mut handles = Vec::new();
            for _ in 0..PRODUCERS {
                let b = bus.clone();
                handles.push(tokio::spawn(async move {
                    for _ in 0..(N / PRODUCERS) {
                        b.publish(make_test_event()).await.expect("publish 应成功");
                    }
                }));
            }

            // 让部分发布先入片，再与排空并发（关键竞态窗口）
            tokio::task::yield_now().await;
            bus.shutdown_sharding().await;

            for h in handles {
                h.await.expect("producer 不应 panic");
            }

            let received = drain_events(&mut rx, N, 3000).await;
            assert_eq!(
                received.len(),
                N,
                "round {round}: shutdown 与发布并发时不得丢事件（实收 {} / {N}）",
                received.len()
            );
        }
    }

    /// P2-5：`CHIMERA_BUS_SHARDS` 严格解析——非法值等同未设置，
    /// 不得绕过 `without_sharding()` 宿主角色的 Off 策略。
    #[test]
    fn env_shard_override_parsing_is_strict() {
        assert_eq!(EventBus::resolve_shard_override(Some("1")), Some(1));
        assert_eq!(EventBus::resolve_shard_override(Some("8")), Some(8));
        assert_eq!(EventBus::resolve_shard_override(Some("256")), Some(256));
        assert_eq!(EventBus::resolve_shard_override(Some("0")), None);
        assert_eq!(EventBus::resolve_shard_override(Some("257")), None);
        assert_eq!(EventBus::resolve_shard_override(Some("abc")), None);
        assert_eq!(EventBus::resolve_shard_override(Some("")), None);
        assert_eq!(EventBus::resolve_shard_override(None), None);
    }

    /// P2-4：`shutdown_sharding` 后 `shard_bus=None` 但 guard 已 set（无法重启），
    /// `try_enable_sharding` 必须诚实返回 false，不得谎报「分片可用」。
    #[tokio::test]
    async fn try_enable_after_shutdown_reports_false() {
        let bus = EventBus::new();
        assert!(bus.try_enable_sharding("test"), "首次启用应成功");
        bus.shutdown_sharding().await;
        assert!(!bus.sharding_enabled(), "shutdown 后分片应为关闭态");
        assert!(
            !bus.try_enable_sharding("test"),
            "shutdown 后不得谎报分片可用（guard 已 set，无法重启用）"
        );
    }

    #[tokio::test]
    async fn test_zero_subscriber_merged_total_counts_entry_not_delivery() {
        // 第三轮 T1.4/F-A2 显式锁测:零订阅者时 worker `send` 恒返 Err(Closed),
        // 但 `merged_total` 仍 == `sharded_total`——该等式的口径是「入汇入通道」
        // 而非「送达」;「入通道即丢弃」全部归因到 T1.1 新增的 `merge_closed_total`。
        // 本测同锁两件事:① merged==sharded 等式不因零订阅而破;② 零订阅下
        // merge_closed_total==merged_total(逐条都丢)。
        let bus = EventBus::new();
        bus.enable_sharding(64).unwrap();
        // 故意不 subscribe:broadcast 无订阅者 → worker send 恒 Err(Closed)
        const N: usize = 50;
        for _ in 0..N {
            bus.publish(make_test_event()).await.unwrap();
        }
        let stats = wait_merged_equals_sharded(&bus, 2000).await;
        assert_eq!(stats.sharded_total, N as u64, "零订阅不影响入分片");
        assert_eq!(
            stats.merged_total, stats.sharded_total,
            "零订阅下 merged==sharded 等式仍成立(证「入通道」非「送达」)"
        );
        assert_eq!(
            stats.merge_closed_total, stats.merged_total,
            "T1.1:零订阅者时全部汇入丢弃,merge_closed_total==merged_total"
        );
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
        // 不用 loom 的理由是**取舍**而非能力限制（原注「Windows GNU 下 loom 编译失败」
        // 已被 R2 C-102 实测推翻：loom 0.7.2 在本工具链编过且模型检验有牙，
        // 证据见 `tmp/loom_probe` 与 ADR-226 E1）。
        // 本测试锁吞吐/守恒：8 线程 × 100 次并发 publish_blocking，
        // 验证 Mutex/Atomic/ArrayQueue/CAS 竞争安全 + 漏发率 0。
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

    /// T17(a) 顺序红线守护:is_critical_mpsc_event() 必须**恰好命中 17 个** mpsc 旁路
    /// 变体(规模由 `CRITICAL_MPSC_VARIANTS` 锁定,广播专属 Critical 计数由
    /// `CRITICAL_TOTAL` 锁定)。防「全 Critical 一律进旁路」旧口径回潮:
    /// 4 个广播专属 Critical(CheckpointSaved/ConsensusReached/SlowConsumerDropped/
    /// OrphanCallDetected)按既定设计只走 broadcast,误捕即回潮破裂。
    #[test]
    // 函数名 exact_13_not_17 为历史标签(T17 立测时的旧规模),现值以
    // CRITICAL_MPSC_VARIANTS=18 为准(C-64b 后;改名留 T3)
    fn mpsc_bypass_exact_13_not_17() {
        let mpsc = tests_helpers::all_mpsc_critical_variants();
        assert_eq!(mpsc.len(), CRITICAL_MPSC_VARIANTS, "18 名单规模锁定");
        for ev in &mpsc {
            assert!(
                is_critical_mpsc_event(ev),
                "{} 必须在 mpsc 旁路清单中",
                ev.type_name()
            );
        }

        // 4 个广播专属 Critical 不得被 mpsc 旁路误捕(17 ⊆ 21,差集恒为 4)
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
                "{} 属广播专属 Critical,不得被 mpsc 误捕(全 Critical 进旁路的旧口径回潮)",
                ev.type_name()
            );
            broadcast_only += 1;
        }
        assert_eq!(
            broadcast_only,
            CRITICAL_TOTAL - CRITICAL_MPSC_VARIANTS,
            "差集 21-17 = {CRITICAL_TOTAL}-{CRITICAL_MPSC_VARIANTS} = 4 个广播专属 Critical"
        );
    }

    /// T17(b) 分片红线:18 个 mpsc 旁路变体全部判定为 Critical 车道(永不进分片,
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
