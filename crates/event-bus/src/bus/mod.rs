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
use crate::credit_flow::CreditFlow;
use crate::critical_sink::CriticalSink;
use crate::logging::BusLogger;
use crate::shard::ShardedEventBus;
use crate::types::{EventSeverity, NexusEvent};
// F-a(M0):ArcSwapOption 承载分片总线单一真值源(取代 Mutex<Option<Arc<_>>> +
// AtomicBool 双表达 —— 双源真相靠手工 Release/Relaxed 配对维持,分叉即灰度时灵时不灵)
use arc_swap::ArcSwapOption;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

/// 构造器子模块（全字段初始化唯一落点）— 与 W6 拆分同批归位
mod builder;
/// 序列化工具子模块（纯编解码，无总线状态）— ADR-314 W6 / BC-01 按职责拆分
mod codec;
/// 观测面子模块（只读指标与吞吐量周期报告器）— 与 W6 拆分同批归位
mod observe;
/// 发布面子模块（事件投递路径）— ADR-314 W6 / BC-01 按职责拆分
mod publish;
/// 接收者子模块（EventReceiver 与其 Drop 语义）— 与 W6 拆分同批归位
mod receiver;
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
}

pub use builder::EventBusBuilder;
pub use observe::spawn_throughput_reporter;
pub use receiver::EventReceiver;

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
