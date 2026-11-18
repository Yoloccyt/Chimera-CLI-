//! 构造器子模块 — [`EventBusBuilder`] 与全字段初始化唯一落点。
//!
//! 从 `bus/mod.rs` 按职责拆出:构造路径集中于此,
//! 旧构造函数(wrapper)仍留在父模块 [`super`]。

use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex, OnceLock};

use arc_swap::ArcSwapOption;
use tokio::sync::broadcast;

use super::{DsRuntime, EventBus};
use crate::backpressure::BackpressurePolicy;
use crate::credit_flow::CreditFlow;
use crate::critical_sink::{CriticalSink, LogCriticalSink};
use crate::logging::BusLogger;

/// EventBus 构造器(R4.1)—— 全字段初始化的唯一落点
///
/// 字段默认值与 `EventBus::new()` 语义完全一致;链式方法只覆盖既有构造路径
/// 可表达的维度(容量/日志/背压策略/保底 sink/分片开关),不新增能力。
/// `build()` 之外不存在其它字段初始化代码 —— 新增 EventBus 字段时只需改
/// `build()` 一处(旧构造函数均为 wrapper,自动继承)。
///
/// 新代码优先用 `EventBus::builder()` 替代 `new`/`with_capacity`/`with_logger`/`with_policy`。
pub struct EventBusBuilder {
    /// 容量默认值 — 供父模块 [`super::EventBus::builder`] 构造初值(拆分后跨模块初始化)
    pub(super) capacity: usize,
    pub(super) logger: Option<BusLogger>,
    pub(super) policy: Option<BackpressurePolicy>,
    pub(super) critical_fallback: Option<Arc<dyn CriticalSink>>,
    pub(super) sharding_allowed: bool,
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
