//! 事件总线 — 基于 tokio::broadcast + MessagePack 的跨层通信通道
//!
//! 对应架构层:L1 Core
//! 对应创新点:无(基础设施,所有跨层通信唯一通道)
//!
//! # 核心职责
//! - 提供类型安全的发布订阅(typed broadcast bus)
//! - 定义 144 个 NexusEvent 跨层事件变体(v2.27.0-omega 实测枚举),
//!   修正 4 处依赖方向违规(Part A 分析)
//! - 背压处理与慢消费者隔离,避免孤儿调用(架构红线)
//! - MessagePack 序列化(ADR-004),支持跨进程投递
//!
//! # 快速示例
//! ```no_run
//! use event_bus::{EventBus, NexusEvent, EventMetadata};
//!
//! # async fn run() {
//! let bus = EventBus::new();
//! let mut rx = bus.subscribe();
//! bus.publish(NexusEvent::QuestCreated {
//!     metadata: EventMetadata::new("quest-engine"),
//!     quest_id: "q-1".into(),
//!     title: "示例".into(),
//!     task_count: 1,
//! }).await.unwrap();
//! let event = rx.recv().await.unwrap();
//! # }
//! ```

#![forbid(unsafe_code)]
#![warn(missing_docs, clippy::all)]

/// P4-T6: CausalGraph 归因台账（ADR-132,diff 5s 窗口因果链回溯）
pub mod attribution;
pub mod backpressure;
pub mod bus;
/// 因果一致性模块(P2-W7.2.1,spec.md L251-254 因果一致性 Scenario)
///
/// 向量时钟跟踪跨膜事件因果关系(Lamport 算法),判定事件偏序:
/// - `happens_before`:a → b(a 先于 b 发生)
/// - `concurrent_with`:a 与 b 无因果关系
///
/// 详见 [`causal::VectorClock`]。
pub mod causal;
/// 事件分类实现(P1-3 拆分,types.rs 上帝文件治理)
///
/// 承接 `NexusEvent` 的 severity()/type_name() 两个巨型 match,
/// types.rs 保留 enum 定义与 metadata()。同 crate 内 `impl NexusEvent`
/// 跨文件分块,调用路径零变化。severity() 判定逻辑留在 event-bus
/// (架构红线:Critical 事件 mpsc 保障),与 bus.rs `is_critical_mpsc_event`
/// 双清单同步红线由守护测试兜底。
pub mod classification;
/// CBF 信用流原语(P1-T11,手册 §8.5 / T-06 / v4.0 WI-08)
///
/// 订阅者按消费速率获信用、发布者无信用挂起:分片启用后 Unordered 事件先
/// `try_acquire(1)`(方案 A:信用流仅作分片背压载体,未启用分片/OrderSensitive
/// 直投不扣减 —— 无归还方的路径不扣),信用耗尽不丢弃(回退 broadcast),
/// 高优事件异步等待 ≤100ms 窗口(Notify),Critical 事件豁免背压(红线:
/// Critical 背压 = 死锁源,推演 9)。
/// 详见 [`credit_flow::CreditFlow`]。
pub mod credit_flow;
/// P3-T10: 事件双轨注册表（v4.0 WI-21 落地,ADR-149:命名空间配额 ≤64/空间 + 审计）
pub mod dynamic_registry;
pub mod error;
/// 分层子枚举 — NexusEvent 按架构层拆分的分类实现
///
/// 将 134 变体的 NexusEvent 按架构层拆分为 8 个子枚举:
/// CoreEvent/MemoryEvent/StorageEvent/SecurityEvent/RouterEvent/
/// ExecutionEvent/QuestEvent/InterfaceEvent。每个子枚举实现
/// `EventClassification` trait(metadata/severity/type_name)。
///
/// # 渐进式方案
/// NexusEvent 变体结构保持不变(消费方零改动),子枚举作为分类
/// 参考与未来迁移目标。
pub mod event_types;
/// 经验卡片总线 — OpenMLE 经验卡片双通道 + 四索引（v3.4.0 §6.1）
///
/// 承载 ExperienceCardBus（broadcast + mpsc 分级投递 + task/node/factor/error
/// 四索引 + 全局统计），使经验卡片成为 Event Bus 一级公民。
pub mod experience_card_bus;
/// FormalVerifier M1 — 事件因果一致性形式化验证(P7-T4,ADR-047 Property #4)
///
/// 验证 EventMetadata 序列(event_id 时序/同 source 时间戳/唯一性),
/// 与 107+ 事件变体载荷解耦;类型复用 `nexus_contracts::formal_props`(L0)。
pub mod formal;
pub mod logging;
/// 膜渗透过滤器(P2-W6.1,ADR-033 后续膜深化)
///
/// 按"是否影响内环状态"分类事件语义类别(EventCategory),结合内环负载档位
/// (InnerLoad)与膜厚度(MembraneThickness)三维度决策事件是否穿膜入内环。
/// 详见 [`membrane::MembraneFilter`]。
pub mod membrane;
/// P2-T5: PatternIndex 事件订阅精确索引(v4.0 WI-15 阶段一)
///
/// 命名空间前缀树 + 字面量哈希精确匹配,语义与广播等价(结构性漏发率 = 0);
/// Critical 强制广播(红线 1:永不近似)。阶段二 HNSW 近似路由仅在订阅者 > 500
/// 且精确索引 P99 > 1ms 时评估,本模块不实现。详见 [`pattern_index::PatternIndex`]。
pub mod pattern_index;
/// 事件载荷与辅助类型 — NexusEvent 枚举依赖的结构化数据类型
///
/// 将 `EventMetadata`、`BudgetMetricsPayload`、`ClvSummary` 等辅助类型
/// 独立为 payloads 模块,减少 types.rs 膨胀。通过 types 模块的 `pub use`
/// 重导出保持向后兼容。
pub mod payloads;
/// RCU 单调读状态容器(P2-W7.2.3,§9.1 arc-swap)
///
/// 因果一致性三层之二:内环共享状态的最终一致 + 单调读。
/// 无锁读(~5ns)+ 原子写(~50ns),旧快照在新写入后仍有效(RCU 回收语义)。
/// 详见 [`rcu::MonotonicState`]。
pub mod rcu;
/// Segment-aware PER — 轨迹分段优先级经验回放（v3.4.0 §6.2）
///
/// 承载 SegmentAwarePER + PerBuffer（铁律9 分段身份共享 + prompt-equal
/// denominator + TD 误差权重采样）。
pub mod segment_per;
/// P4-T4: 影子双跑 diff 采集（串行影子 vs 分片实投逐事件比对 + ≥7 天台账）
pub mod shadow_diff;
/// ShardedBus 分片核心 — 非 Critical 事件的分片扇出(P1-T12,手册 §8.5)
///
/// Lane 三车道判定(Critical/OrderSensitive/Unordered)+ 64 片无锁队列扇出
/// + shard_worker 攒批汇入既有 broadcast。灰度开关:`EventBus::enable_sharding`
/// (默认不启用,零回归)。详见 [`shard::ShardedEventBus`]。
pub mod shard;
pub mod subscriber;
/// P2-T11: OTel 风格轻量遥测（v4.0 WI-28 落地,ADR-143:event-bus 增强）
///
/// Turn Span 生命周期 + 延迟直方图（AtomicU64 无锁热路径,开销 <5% CPU）;
/// 完整 OTLP 导出器留 Phase 3。与 L9 efficiency-monitor 分工:本模块=基础设施追踪。
pub mod telemetry;
/// Token 账本 — Dressage Token 级证据的 append-only 存储（v3.4.0 §6.1 + §5.3）
///
/// 承载 TokenLedger（有序账本 + 会话/实例双索引 + 完整性校验 + 导出通道），
/// 保证"Token Ledger 不可丢失（训练证据完整性）"绝对红线。
pub mod token_ledger;
pub mod topic;
pub mod types;

// === 关键类型重导出,简化外部导入 ===
pub use backpressure::{is_critical_event, BackpressurePolicy, SlowConsumerDetector};
// FormalVerifier M1:事件因果一致性验证器重导出(P7-T4)
pub use bus::{
    deserialize_json, deserialize_msgpack, serialize_json, serialize_msgpack, EventBus,
    EventReceiver, CRITICAL_MPSC_VARIANTS, CRITICAL_TOTAL, DEFAULT_CAPACITY, LANE_FORBIDDEN_SHARD,
};
// P3-T10: 事件双轨注册表公开 API（WI-21/ADR-149）
// P4-T4: 双跑采集公开 API
pub use attribution::{
    AttributionNode, AttributionResult, CausalAttributionLedger, ATTRIBUTION_WINDOW_MS,
};
pub use dynamic_registry::{
    DynamicEventRegistry, RegisterOutcome, RegistryAuditEntry, NAMESPACE_QUOTA,
};
pub use shadow_diff::{
    canonical_fingerprint, compare_cross_instance, event_fingerprint, DiffEntry,
    ShadowDiffRecorder, ShadowDiffReport, ShadowSojournLedger,
};
// §16.5 L1 吞吐量周期报告器(Phase 10 Wave 6)
pub use crate::bus::spawn_throughput_reporter;
pub use error::EventBusError;
// P1-T11:CBF 信用流原语(CreditFlow/Priority/CreditError/CreditStats)
pub use credit_flow::{
    CreditError, CreditFlow, CreditStats, Priority, DEFAULT_CREDITS, HIGH_PRIORITY_WAIT_WINDOW,
};
pub use formal::CausalConsistencyChecker;
pub use logging::BusLogger;
// P2-W7.2.3: RCU 单调读状态容器(内环最终一致 + 单调读)
pub use rcu::MonotonicState;
// P1-T12:ShardedBus 分片核心(Lane 三车道 + 64 片扇出 + 前哨统计)
pub use shard::{
    event_lane, fnv1a, Lane, SessionKey, ShadowStats, ShardedEventBus, DEFAULT_SHARD_COUNT,
    MAX_EVENT_PAYLOAD_BYTES, SHARD_CAPACITY, SHARD_WORKER_BATCH,
};
// P1-W4.2: TypeState 订阅构建器(强制 subscribe-then-spawn 原子化)
pub use subscriber::{
    CriticalSubscribed, CriticalSubscriberBuilder, Subscribed, SubscriberBuilder, Unsubscribed,
};
// P2-W6.1: 膜渗透过滤器(内环/外环选择性渗透决策)
pub use membrane::{
    EventCategory, InnerLoad, MembraneFilter, MembraneThickness, PermeationDecision,
};
// P2-W7.2.1: 因果一致性(向量时钟,跨膜事件因果关系跟踪)
pub use causal::{CausalRelation, VectorClock};
pub use topic::{EventTopic, FilteredSubscriber};
pub use types::{
    ActionSource, AgentStatus, BudgetMetricsPayload, ChatStatus, ClvSummary, CompatLevel,
    ConsultUrgency, CriticalEventDropped, EventMetadata, EventSeverity, NexusEvent, QuestStatus,
    RouterStatsPayload, TaskPriority, VoteValue,
};
// v3.4.0 §6.1: 经验卡片总线（OpenMLE 双通道 + 四索引）
pub use experience_card_bus::{ExperienceCardBus, GlobalCardStats};
// v3.4.0 §6.1 + §5.3: Token 账本（Dressage 证据完整性）
pub use token_ledger::{LedgerError, LedgerIntegrityReport, TokenLedger};
// v3.4.0 §6.2: Segment-aware PER（铁律9 分段身份）
pub use segment_per::{PerBuffer, PerEntry, PerStats, SegmentAwarePER};
// 分层子枚举与分类 trait(渐进式拆分,消费方可按需导入)
pub use event_types::{
    CoreEvent, EventClassification, ExecutionEvent, InterfaceEvent, MemoryEvent, QuestEvent,
    RouterEvent, SecurityEvent, StorageEvent,
};

/// 预导入模块 — 提供最常用类型
pub mod prelude {
    pub use crate::backpressure::BackpressurePolicy;
    pub use crate::bus::{EventBus, EventReceiver, DEFAULT_CAPACITY};
    pub use crate::error::EventBusError;
    pub use crate::logging::BusLogger;
    pub use crate::topic::{EventTopic, FilteredSubscriber};
    pub use crate::types::{
        ActionSource, AgentStatus, BudgetMetricsPayload, ChatStatus, ConsultUrgency, EventMetadata,
        EventSeverity, NexusEvent, QuestStatus, RouterStatsPayload, TaskPriority,
    };
}
