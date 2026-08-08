//! P4-W16.1.1: 轨迹捕获 hook trait — 在 Router::route() 边界注入观测逻辑
//!
//! 对应架构层:L1 Core(model-router)
//! 对应 spec.md §Scenario "model-router 轨迹捕获"
//!
//! # 设计原则
//!
//! ## 1. 依赖倒置原则(DIP)
//! model-router 位于 L1,不能依赖上层(L9 quest-engine / L10 chimera-cli)。
//! 通过 trait 注入:本模块定义 `RouteHook` 契约,上层实现具体捕获逻辑
//! (如写入经验回放池、发送到追踪系统)。与 P1-W3.1 `EscalationHandler` 同模式。
//!
//! ## 2. 同步 trait(非 async)
//! 参考 seccore `EscalationHandler`,hook 为同步 trait。原因:
//! - route() 内部已 async,hook 调用是同步开销极低(μs 级)
//! - hook 内部如需异步操作可自行 `tokio::spawn`
//! - 避免 `async-trait` 依赖引入 Box<dyn Future> 堆分配
//! - 与 EscalationHandler 模式一致,降低认知负担
//!
//! ## 3. 不可变借用契约
//! hook 仅观察 `TrajectoryEvent`(包含请求快照与决策快照),不能修改路由决策。
//! 这避免了"hook 副作用污染路由"的安全风险。
//!
//! ## 4. 向后兼容
//! `ModelRouter::new()` 不配置 hook,行为与既有完全一致。
//! 通过 `with_hook()` / `with_hooks()` / `with_cacr_and_hook()` builder 注入。
//!
//! # 事件流
//! ```text
//! route() 入口 ──> 计时开始 ──> 策略分发 ──> CACR 拦截
//!                                              │
//!                                              ▼
//!             ┌─── 成功路径 ────> 发布 ModelRouteSelected ────> 计时结束
//!             │                                                 │
//!             └─── 错误路径 ────> 发布 BudgetExceeded ──────────┤
//!                                                              ▼
//!                                         构造 TrajectoryEvent ──> 调用 hook.on_route_completed()
//! ```

use crate::error::RouterError;
use crate::types::{RoutingDecision, RoutingStrategy};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

/// 路由 hook trait — 上层实现以观察路由轨迹
///
/// # 实现契约
/// - 必须 `Send + Sync`(ModelRouter 可在 async 任务间共享)
/// - `on_route_completed` 为同步方法,内部如需异步请用 `tokio::spawn`
/// - 不可修改 `TrajectoryEvent` 内容(仅观察,不干预)
/// - 实现不应 panic(可能导致 ModelRouter 不可用)
///
/// # 默认实现
/// trait 提供空默认实现,允许实现者按需覆盖,无强制实现负担
pub trait RouteHook: Send + Sync {
    /// 路由完成后调用 — 传入完整的 TrajectoryEvent
    ///
    /// # 调用时机
    /// - 成功路径:发布 `ModelRouteSelected` 事件之后
    /// - 错误路径:发布 `BudgetExceeded` 事件之后或返回错误之前
    ///
    /// # 副作用建议
    /// - 写入经验回放池(P4-W16.2)
    /// - 推送到追踪系统(Jaeger/OpenTelemetry)
    /// - 累计 metrics(prometheus counter)
    /// - 异步落盘(通过 `tokio::spawn` 避免阻塞 route())
    fn on_route_completed(&self, _event: TrajectoryEvent) {}
}

/// 路由结果快照 — 成功或失败的不可变记录
///
/// # 设计决策
/// 使用 enum 而非 `Result<RoutingDecision, RouterError>`:
/// - `Result` 的 `Err` 携带 `RouterError`,无法 `Clone`(RouterError 未派生 Clone)
/// - enum 变体只保留必要字段,便于序列化到回放池
/// - `Error` 变体的 `error_kind: String` 提供错误类型标签(如 "BudgetExceeded")
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum TrajectoryOutcome {
    /// 成功路径 — 路由决策已发布
    Success {
        /// 选中的模型 ID
        model_id: String,
        /// 路由原因(人类可读)
        route_reason: String,
        /// 预估成本(美分,1 美元 = 100 美分)
        estimated_cost: u64,
        /// 候选模型列表(按策略优先级降序排序)
        candidates: Vec<String>,
    },

    /// 错误路径 — 路由失败
    Error {
        /// 错误类型标签(对应 RouterError 变体名,如 "NoModelsRegistered")
        error_kind: String,
    },
}

impl TrajectoryOutcome {
    /// 从 `Result<RoutingDecision, RouterError>` 构造 outcome 快照
    ///
    /// # 设计决策
    /// 此转换在 route() 末尾调用,将 RouterError 转为可序列化的 String 标签。
    /// 保留 RouterError 的 Debug 字符串作为兜底,便于人工排查;
    /// 标签前缀匹配变体名,便于程序化过滤(如 `error_kind.contains("BudgetExceeded")`)
    pub fn from_result(result: &Result<RoutingDecision, RouterError>) -> Self {
        match result {
            Ok(decision) => Self::Success {
                model_id: decision.model_id.clone(),
                route_reason: decision.route_reason.clone(),
                estimated_cost: decision.estimated_cost,
                candidates: decision.candidates.clone(),
            },
            Err(err) => {
                // 提取错误变体名作为标签 — 使用 Debug 字符串前缀
                // 例:RouterError::BudgetExceeded{..} 的 Debug 为 "BudgetExceeded { cost: ..., limit: ... }"
                let debug_str = format!("{:?}", err);
                // 提取第一个标识符(变体名),截至空格或 `{`
                let error_kind = debug_str
                    .split_whitespace()
                    .next()
                    .unwrap_or("UnknownError")
                    .trim_end_matches('{')
                    .to_string();
                Self::Error { error_kind }
            }
        }
    }
}

/// 轨迹事件 — 单次路由调用的完整观测记录
///
/// # 字段设计
/// - `quest_id`:关联 quest-engine 的 Quest ID,便于跨模块追踪
/// - `strategy`:路由策略(Lite/Efficient/Auto),便于按策略分组统计
/// - `estimated_tokens`:预估 token 数,用于成本核算
/// - `latency`:端到端延迟(从入口到出口),用于性能分析
/// - `outcome`:成功或失败的快照
///
/// # 序列化支持
/// 派生 `Serialize + Deserialize`,支持序列化到经验回放池(P4-W16.2)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrajectoryEvent {
    /// 所属 Quest ID(关联 quest-engine 的 Quest)
    pub quest_id: String,
    /// 路由策略
    pub strategy: RoutingStrategy,
    /// 预估 token 数(输入 + 输出)
    pub estimated_tokens: u32,
    /// 端到端延迟(从 route() 入口到出口)
    pub latency: Duration,
    /// 路由结果快照(成功或失败)
    pub outcome: TrajectoryOutcome,
}

// ============================================================
// P4-W16.1.2: 生产级 RecordingHook 实现
// ============================================================

/// 生产级轨迹捕获 hook — 将 `TrajectoryEvent` 缓冲到有界内存队列
///
/// 对应架构层:L1 Core(model-router)
/// 对应 spec.md §Scenario "model-router 轨迹捕获" 捕获点 1
///
/// # 设计目标
/// - **捕获点 1**:请求/响应/延迟/token 成本/路由决策
/// - **有界缓冲**:容量超限 FIFO 淘汰,避免内存爆炸(§6.1 红线 6)
/// - **统计可观测**:原子计数器提供 success/error/策略分布,无锁热路径
/// - **回放池对接**:`drain()` 方法供 P4-W16.2 经验回放池消费
///
/// # 容量管理
/// 默认容量 10_000(与 P4-W16.2 回放池 ≥10K 轨迹目标对齐)。
/// 超出时淘汰最旧条目并 `tracing::warn!` 记录(便于运维感知)。
///
/// # 线程安全
/// - `buffer` 用 `Mutex<VecDeque>` 保护,Push/Drain 互斥
/// - `stats` 用 `AtomicU64` 计数,热路径(`on_route_completed`)无锁读取
/// - 整体 `Send + Sync`,满足 `RouteHook` trait 约束
///
/// # 使用示例
/// ```rust,no_run
/// use model_router::{ModelRouter, ModelRegistry, RouterConfig, trajectory::RecordingHook};
/// use std::sync::Arc;
///
/// let bus = event_bus::EventBus::new();
/// let registry = ModelRegistry::from_config(&RouterConfig::default());
/// let hook = Arc::new(RecordingHook::new());
/// let router = ModelRouter::with_hook(registry, bus, hook.clone());
///
/// // ... 路由调用 ...
///
/// // 消费轨迹(供回放池)
/// let events: Vec<_> = hook.drain();
/// assert!(!events.is_empty());
/// ```
pub struct RecordingHook {
    /// 有界缓冲区 — FIFO 顺序保存 TrajectoryEvent
    buffer: Mutex<VecDeque<TrajectoryEvent>>,
    /// 缓冲区容量 — 超出则淘汰最旧条目
    capacity: usize,
    /// 总事件数(含已淘汰)— AtomicU64 无锁热路径
    total_events: AtomicU64,
    /// 成功事件数 — 仅 outcome=Success 时递增
    success_count: AtomicU64,
    /// 错误事件数 — 仅 outcome=Error 时递增
    error_count: AtomicU64,
    /// 因容量超限被淘汰的条目数 — 用于运维观测
    evicted_count: AtomicU64,
}

impl RecordingHook {
    /// 创建默认容量(10_000)的 RecordingHook
    ///
    /// # 容量选择
    /// 10_000 与 P4-W16.2 经验回放池 ≥10K 轨迹目标对齐。
    /// 生产环境可通过 [`with_capacity`] 自定义。
    pub fn new() -> Self {
        Self::with_capacity(10_000)
    }

    /// 创建指定容量的 RecordingHook
    ///
    /// # 参数
    /// - `capacity`:缓冲区最大条目数,超出时 FIFO 淘汰最旧条目
    ///
    /// # 约束
    /// - `capacity = 0` 视为 1(避免 push 时除零)
    /// - 容量过小会增加淘汰频率,建议 ≥1_000
    pub fn with_capacity(capacity: usize) -> Self {
        let normalized = if capacity == 0 { 1 } else { capacity };
        Self {
            buffer: Mutex::new(VecDeque::with_capacity(normalized)),
            capacity: normalized,
            total_events: AtomicU64::new(0),
            success_count: AtomicU64::new(0),
            error_count: AtomicU64::new(0),
            evicted_count: AtomicU64::new(0),
        }
    }

    /// 获取当前缓冲区中的事件数(不含已淘汰)
    ///
    /// # 注意
    /// 此方法获取 Mutex,不应在路由热路径调用。
    /// 仅供运维查询或测试断言使用。
    pub fn len(&self) -> usize {
        self.buffer.lock().map(|buf| buf.len()).unwrap_or(0)
    }

    /// 判断缓冲区是否为空
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 获取缓冲区容量上限
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// 获取累计事件总数(含已淘汰)— 原子读,无锁
    pub fn total_events(&self) -> u64 {
        self.total_events.load(Ordering::Relaxed)
    }

    /// 获取成功事件数 — 原子读,无锁
    pub fn success_count(&self) -> u64 {
        self.success_count.load(Ordering::Relaxed)
    }

    /// 获取错误事件数 — 原子读,无锁
    pub fn error_count(&self) -> u64 {
        self.error_count.load(Ordering::Relaxed)
    }

    /// 获取因容量超限被淘汰的条目数 — 原子读,无锁
    pub fn evicted_count(&self) -> u64 {
        self.evicted_count.load(Ordering::Relaxed)
    }

    /// 获取轨迹统计快照 — 一次性返回所有计数器
    ///
    /// # 返回
    /// `TrajectoryStats` 包含 total/success/error/evicted/buffer_len 字段,
    /// 便于上层(L9 efficiency-monitor / L10 TUI)统一观测。
    ///
    /// # 性能
    /// 6 次 AtomicU64::load + 1 次 Mutex lock,~100ns,适合定期采样。
    pub fn stats(&self) -> TrajectoryStats {
        TrajectoryStats {
            total_events: self.total_events.load(Ordering::Relaxed),
            success_count: self.success_count.load(Ordering::Relaxed),
            error_count: self.error_count.load(Ordering::Relaxed),
            evicted_count: self.evicted_count.load(Ordering::Relaxed),
            buffer_len: self.len(),
            capacity: self.capacity,
        }
    }

    /// 取出所有缓冲的轨迹事件并清空缓冲区
    ///
    /// # 用途
    /// 供 P4-W16.2 经验回放池批量消费。Drain 后缓冲区为空,
    /// 新事件继续写入。此方法获取 Mutex,不应在路由热路径调用。
    ///
    /// # 错误处理
    /// Mutex poison 时返回空 Vec(不 panic,保证 route() 不被拖垮)
    pub fn drain(&self) -> Vec<TrajectoryEvent> {
        self.buffer
            .lock()
            .map(|mut buf| buf.drain(..).collect())
            .unwrap_or_default()
    }

    /// 获取当前缓冲区的快照(不影响缓冲区)
    ///
    /// # 用途
    /// 测试断言与调试观测。生产消费请用 [`drain`] 避免内存增长。
    pub fn snapshot(&self) -> Vec<TrajectoryEvent> {
        self.buffer
            .lock()
            .map(|buf| buf.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// 清空缓冲区(不重置计数器)
    ///
    /// # 用途
    /// 测试场景重置缓冲区但保留累计统计。
    /// 生产场景一般用 `drain()` 替代。
    pub fn clear(&self) {
        if let Ok(mut buf) = self.buffer.lock() {
            buf.clear();
        }
    }

    /// 内部 push 实现 — 容量超限时 FIFO 淘汰
    ///
    /// # 设计决策
    /// - 不在 Mutex 内做 stats 更新(原子操作在锁外)
    /// - 容量超限时先 pop_front 再 push_back,保证 FIFO 淘汰
    /// - 淘汰事件计数 atomic increment,便于运维观测
    fn push_event(&self, event: TrajectoryEvent) {
        // 先更新计数器(原子操作,无锁,~5ns)
        self.total_events.fetch_add(1, Ordering::Relaxed);
        match &event.outcome {
            TrajectoryOutcome::Success { .. } => {
                self.success_count.fetch_add(1, Ordering::Relaxed);
            }
            TrajectoryOutcome::Error { .. } => {
                self.error_count.fetch_add(1, Ordering::Relaxed);
            }
        }

        // 再写入缓冲区(Mutex,~50ns)
        if let Ok(mut buf) = self.buffer.lock() {
            if buf.len() >= self.capacity {
                // FIFO 淘汰最旧条目
                buf.pop_front();
                self.evicted_count.fetch_add(1, Ordering::Relaxed);
                // WHY tracing::debug 而非 warn:容量淘汰是设计预期行为,
                // 高频 warn 会污染日志;生产环境可通过 stats().evicted_count 观测
                tracing::debug!(
                    capacity = self.capacity,
                    evicted_total = self.evicted_count.load(Ordering::Relaxed),
                    "RecordingHook 容量超限,FIFO 淘汰最旧条目"
                );
            }
            buf.push_back(event);
        }
        // Mutex poison 时静默丢弃事件(避免 panic 拖垮 route())
        // 运维可通过 stats().total_events > buffer_len 感知丢失
    }
}

impl Default for RecordingHook {
    fn default() -> Self {
        Self::new()
    }
}

impl RouteHook for RecordingHook {
    /// 路由完成后调用 — 将事件缓冲到内存队列
    ///
    /// # 性能预算
    /// - 原子计数器更新:~10ns(2-3 次 fetch_add)
    /// - Mutex lock + push_back:~50-100ns
    /// - 总开销:<200ns,对 route() 边际延迟影响可忽略
    ///
    /// # 错误处理
    /// - Mutex poison 时静默丢弃(避免 panic)
    /// - 不返回错误(hook 契约:仅观察,不干预路由)
    fn on_route_completed(&self, event: TrajectoryEvent) {
        self.push_event(event);
    }
}

/// 轨迹统计快照 — `RecordingHook` 的可观测视图
///
/// # 设计原则
/// - 值类型(Snapshot),可跨线程传递
/// - 一次性快照(非实时视图),避免锁持有
/// - 包含 buffer_len/capacity 便于容量监控
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrajectoryStats {
    /// 累计事件总数(含已淘汰)
    pub total_events: u64,
    /// 成功事件数
    pub success_count: u64,
    /// 错误事件数
    pub error_count: u64,
    /// 因容量超限被淘汰的条目数
    pub evicted_count: u64,
    /// 当前缓冲区中的事件数(不含已淘汰)
    pub buffer_len: usize,
    /// 缓冲区容量上限
    pub capacity: usize,
}

impl TrajectoryStats {
    /// 计算错误率(0.0-1.0)
    ///
    /// # 返回
    /// - `total_events = 0` 时返回 0.0(避免除零)
    /// - 否则返回 `error_count / total_events` 的浮点比值
    pub fn error_rate(&self) -> f64 {
        if self.total_events == 0 {
            0.0
        } else {
            self.error_count as f64 / self.total_events as f64
        }
    }

    /// 计算缓冲区使用率(0.0-1.0)
    ///
    /// # 返回
    /// - `capacity = 0` 时返回 0.0(避免除零,虽然构造时已规范化)
    /// - 否则返回 `buffer_len / capacity` 的浮点比值
    pub fn buffer_usage(&self) -> f64 {
        if self.capacity == 0 {
            0.0
        } else {
            self.buffer_len as f64 / self.capacity as f64
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::RoutingDecision;
    use std::sync::Arc;

    // ============================================================
    // TrajectoryOutcome 单元测试
    // ============================================================

    #[test]
    fn test_outcome_from_success_result() {
        let decision = RoutingDecision {
            model_id: "test-model".into(),
            route_reason: "test reason".into(),
            estimated_cost: 500,
            candidates: vec!["alt-1".into(), "alt-2".into()],
        };
        let result = Ok(decision);
        let outcome = TrajectoryOutcome::from_result(&result);

        match outcome {
            TrajectoryOutcome::Success {
                model_id,
                route_reason,
                estimated_cost,
                candidates,
            } => {
                assert_eq!(model_id, "test-model");
                assert_eq!(route_reason, "test reason");
                assert_eq!(estimated_cost, 500);
                assert_eq!(candidates, vec!["alt-1".to_string(), "alt-2".into()]);
            }
            _ => panic!("应构造 Success 变体"),
        }
    }

    #[test]
    fn test_outcome_from_budget_exceeded_error() {
        let err = RouterError::BudgetExceeded {
            cost: 200,
            limit: 100,
        };
        let result: Result<RoutingDecision, RouterError> = Err(err);
        let outcome = TrajectoryOutcome::from_result(&result);

        match outcome {
            TrajectoryOutcome::Error { error_kind } => {
                assert!(
                    error_kind.contains("BudgetExceeded"),
                    "error_kind 应包含 BudgetExceeded: {}",
                    error_kind
                );
            }
            _ => panic!("应构造 Error 变体"),
        }
    }

    #[test]
    fn test_outcome_from_no_models_registered_error() {
        let err = RouterError::NoModelsRegistered;
        let result: Result<RoutingDecision, RouterError> = Err(err);
        let outcome = TrajectoryOutcome::from_result(&result);

        match outcome {
            TrajectoryOutcome::Error { error_kind } => {
                assert!(
                    error_kind.contains("NoModelsRegistered"),
                    "error_kind 应包含 NoModelsRegistered: {}",
                    error_kind
                );
            }
            _ => panic!("应构造 Error 变体"),
        }
    }

    #[test]
    fn test_trajectory_event_serde_roundtrip() {
        let event = TrajectoryEvent {
            quest_id: "q-1".into(),
            strategy: RoutingStrategy::Auto,
            estimated_tokens: 1000,
            latency: Duration::from_millis(42),
            outcome: TrajectoryOutcome::Success {
                model_id: "auto-model".into(),
                route_reason: "best fit".into(),
                estimated_cost: 100,
                candidates: vec!["alt-1".into()],
            },
        };

        let json = serde_json::to_string(&event).expect("序列化必须成功");
        let de: TrajectoryEvent = serde_json::from_str(&json).expect("反序列化必须成功");
        assert_eq!(de, event);
    }

    #[test]
    fn test_trajectory_event_error_outcome_serde() {
        let event = TrajectoryEvent {
            quest_id: "q-err".into(),
            strategy: RoutingStrategy::Lite,
            estimated_tokens: 0,
            latency: Duration::from_micros(100),
            outcome: TrajectoryOutcome::Error {
                error_kind: "NoModelsRegistered".into(),
            },
        };

        let json = serde_json::to_string(&event).expect("序列化必须成功");
        let de: TrajectoryEvent = serde_json::from_str(&json).expect("反序列化必须成功");
        assert_eq!(de, event);
    }

    // ============================================================
    // RouteHook trait 默认实现测试
    // ============================================================

    /// 测试用空实现 hook
    #[derive(Debug, Default)]
    struct NoopHook;

    impl RouteHook for NoopHook {}

    #[test]
    fn test_noop_hook_can_be_constructed() {
        let _hook = NoopHook;
    }

    #[test]
    fn test_hook_send_sync_static_assert() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<NoopHook>();
        assert_send_sync::<TrajectoryEvent>();
        assert_send_sync::<TrajectoryOutcome>();
    }

    // ============================================================
    // P4-W16.1.2: RecordingHook 生产级实现测试
    // ============================================================

    /// 构造测试用 TrajectoryEvent 成功事件
    fn make_success_event(quest_id: &str, strategy: RoutingStrategy) -> TrajectoryEvent {
        TrajectoryEvent {
            quest_id: quest_id.into(),
            strategy,
            estimated_tokens: 1000,
            latency: Duration::from_millis(42),
            outcome: TrajectoryOutcome::Success {
                model_id: "test-model".into(),
                route_reason: "test".into(),
                estimated_cost: 100,
                candidates: vec!["alt-1".into()],
            },
        }
    }

    /// 构造测试用 TrajectoryEvent 错误事件
    fn make_error_event(quest_id: &str, strategy: RoutingStrategy) -> TrajectoryEvent {
        TrajectoryEvent {
            quest_id: quest_id.into(),
            strategy,
            estimated_tokens: 1000,
            latency: Duration::from_millis(10),
            outcome: TrajectoryOutcome::Error {
                error_kind: "BudgetExceeded".into(),
            },
        }
    }

    #[test]
    fn test_recording_hook_default_capacity() {
        let hook = RecordingHook::new();
        assert_eq!(hook.capacity(), 10_000, "默认容量必须为 10_000");
        assert_eq!(hook.total_events(), 0);
        assert_eq!(hook.success_count(), 0);
        assert_eq!(hook.error_count(), 0);
        assert_eq!(hook.evicted_count(), 0);
        assert!(hook.is_empty());
    }

    #[test]
    fn test_recording_hook_with_custom_capacity() {
        let hook = RecordingHook::with_capacity(500);
        assert_eq!(hook.capacity(), 500);
    }

    #[test]
    fn test_recording_hook_zero_capacity_normalizes_to_one() {
        // 容量 0 视为 1(避免 push 时除零)
        let hook = RecordingHook::with_capacity(0);
        assert_eq!(hook.capacity(), 1, "容量 0 必须规范化为 1");
    }

    #[test]
    fn test_recording_hook_push_success_event() {
        let hook = RecordingHook::new();
        hook.on_route_completed(make_success_event("q-1", RoutingStrategy::Lite));

        assert_eq!(hook.len(), 1, "缓冲区应有 1 个事件");
        assert_eq!(hook.total_events(), 1);
        assert_eq!(hook.success_count(), 1);
        assert_eq!(hook.error_count(), 0);
        assert_eq!(hook.evicted_count(), 0);
    }

    #[test]
    fn test_recording_hook_push_error_event() {
        let hook = RecordingHook::new();
        hook.on_route_completed(make_error_event("q-err", RoutingStrategy::Auto));

        assert_eq!(hook.len(), 1);
        assert_eq!(hook.total_events(), 1);
        assert_eq!(hook.success_count(), 0);
        assert_eq!(hook.error_count(), 1);
    }

    #[test]
    fn test_recording_hook_mixed_events_stats() {
        let hook = RecordingHook::new();
        // 3 成功 + 1 错误
        hook.on_route_completed(make_success_event("q-1", RoutingStrategy::Lite));
        hook.on_route_completed(make_success_event("q-2", RoutingStrategy::Auto));
        hook.on_route_completed(make_success_event("q-3", RoutingStrategy::Efficient));
        hook.on_route_completed(make_error_event("q-err", RoutingStrategy::Auto));

        let stats = hook.stats();
        assert_eq!(stats.total_events, 4);
        assert_eq!(stats.success_count, 3);
        assert_eq!(stats.error_count, 1);
        assert_eq!(stats.buffer_len, 4);
        assert_eq!(stats.capacity, 10_000);
        assert_eq!(stats.evicted_count, 0);

        // 错误率 = 1/4 = 0.25
        let err_rate = stats.error_rate();
        assert!(
            (err_rate - 0.25).abs() < f64::EPSILON,
            "错误率应为 0.25,实际: {}",
            err_rate
        );

        // 缓冲区使用率 = 4/10000 = 0.0004
        let usage = stats.buffer_usage();
        assert!(
            (usage - 0.0004).abs() < 1e-9,
            "使用率应为 0.0004,实际: {}",
            usage
        );
    }

    #[test]
    fn test_recording_hook_fifo_eviction() {
        let hook = RecordingHook::with_capacity(3);

        // 写入 3 个事件(满)
        hook.on_route_completed(make_success_event("q-1", RoutingStrategy::Lite));
        hook.on_route_completed(make_success_event("q-2", RoutingStrategy::Lite));
        hook.on_route_completed(make_success_event("q-3", RoutingStrategy::Lite));
        assert_eq!(hook.len(), 3);
        assert_eq!(hook.evicted_count(), 0);

        // 写入第 4 个事件 — 淘汰 q-1
        hook.on_route_completed(make_success_event("q-4", RoutingStrategy::Lite));
        assert_eq!(hook.len(), 3, "容量满后 push 仍保持 3 条");
        assert_eq!(hook.total_events(), 4, "total_events 累计 4");
        assert_eq!(hook.evicted_count(), 1, "应淘汰 1 条");

        // 验证 FIFO — q-1 已被淘汰,buffer 中是 q-2/q-3/q-4
        let snapshot = hook.snapshot();
        assert_eq!(snapshot.len(), 3);
        assert_eq!(snapshot[0].quest_id, "q-2", "q-1 应被淘汰,最早的是 q-2");
        assert_eq!(snapshot[1].quest_id, "q-3");
        assert_eq!(snapshot[2].quest_id, "q-4");
    }

    #[test]
    fn test_recording_hook_drain() {
        let hook = RecordingHook::new();
        hook.on_route_completed(make_success_event("q-1", RoutingStrategy::Lite));
        hook.on_route_completed(make_success_event("q-2", RoutingStrategy::Auto));
        hook.on_route_completed(make_error_event("q-3", RoutingStrategy::Lite));

        let events = hook.drain();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].quest_id, "q-1");
        assert_eq!(events[1].quest_id, "q-2");
        assert_eq!(events[2].quest_id, "q-3");

        // drain 后缓冲区清空,但计数器保留
        assert!(hook.is_empty());
        assert_eq!(hook.total_events(), 3, "计数器不重置");
        assert_eq!(hook.success_count(), 2);
        assert_eq!(hook.error_count(), 1);
    }

    #[test]
    fn test_recording_hook_drain_empty() {
        let hook = RecordingHook::new();
        let events = hook.drain();
        assert!(events.is_empty(), "空缓冲区 drain 应返回空 Vec");
    }

    #[test]
    fn test_recording_hook_snapshot_does_not_mutate() {
        let hook = RecordingHook::new();
        hook.on_route_completed(make_success_event("q-1", RoutingStrategy::Lite));

        let snap1 = hook.snapshot();
        assert_eq!(snap1.len(), 1);

        // snapshot 不影响缓冲区
        assert_eq!(hook.len(), 1, "snapshot 不应清空缓冲区");

        let snap2 = hook.snapshot();
        assert_eq!(snap2.len(), 1, "第二次 snapshot 仍应有 1 条");
    }

    #[test]
    fn test_recording_hook_clear() {
        let hook = RecordingHook::new();
        hook.on_route_completed(make_success_event("q-1", RoutingStrategy::Lite));
        hook.on_route_completed(make_success_event("q-2", RoutingStrategy::Auto));

        hook.clear();
        assert!(hook.is_empty(), "clear 后缓冲区应为空");
        // 计数器不重置
        assert_eq!(hook.total_events(), 2, "计数器不应被 clear 影响");
    }

    #[test]
    fn test_recording_hook_concurrent_push() {
        use std::sync::Arc;
        use std::thread;

        let hook = Arc::new(RecordingHook::with_capacity(1000));
        let mut handles = vec![];

        // 10 个线程,每个写入 100 个事件
        for t in 0..10 {
            let hook_clone = Arc::clone(&hook);
            handles.push(thread::spawn(move || {
                for i in 0..100 {
                    let quest_id = format!("q-{}-{}", t, i);
                    hook_clone
                        .on_route_completed(make_success_event(&quest_id, RoutingStrategy::Auto));
                }
            }));
        }

        for handle in handles {
            handle.join().expect("线程不应 panic");
        }

        // 总事件数 = 10 * 100 = 1000
        assert_eq!(hook.total_events(), 1000, "应累计 1000 个事件");
        assert_eq!(hook.success_count(), 1000);
        assert_eq!(hook.len(), 1000, "缓冲区应容纳全部 1000 个事件(未超容量)");
        assert_eq!(hook.evicted_count(), 0, "未超容量,不应淘汰");
    }

    #[test]
    fn test_recording_hook_concurrent_push_with_eviction() {
        use std::sync::Arc;
        use std::thread;

        // 容量 100,但写入 1000 个事件,应淘汰 900 个
        let hook = Arc::new(RecordingHook::with_capacity(100));
        let mut handles = vec![];

        for t in 0..10 {
            let hook_clone = Arc::clone(&hook);
            handles.push(thread::spawn(move || {
                for i in 0..100 {
                    let quest_id = format!("q-{}-{}", t, i);
                    hook_clone
                        .on_route_completed(make_success_event(&quest_id, RoutingStrategy::Auto));
                }
            }));
        }

        for handle in handles {
            handle.join().expect("线程不应 panic");
        }

        assert_eq!(hook.total_events(), 1000, "累计 1000 个事件");
        assert_eq!(hook.len(), 100, "缓冲区保持在容量上限 100");
        assert_eq!(hook.evicted_count(), 900, "应淘汰 900 个事件");
    }

    #[test]
    fn test_recording_hook_send_sync_static_assert() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<RecordingHook>();
        assert_send_sync::<Arc<RecordingHook>>();
        assert_send_sync::<TrajectoryStats>();
    }

    #[test]
    fn test_trajectory_stats_error_rate_zero_total() {
        let stats = TrajectoryStats {
            total_events: 0,
            success_count: 0,
            error_count: 0,
            evicted_count: 0,
            buffer_len: 0,
            capacity: 100,
        };
        assert_eq!(stats.error_rate(), 0.0, "total=0 时错误率应为 0.0");
    }

    #[test]
    fn test_trajectory_stats_buffer_usage_zero_capacity() {
        // 构造时已规范化 capacity=0 → 1,但 TrajectoryStats 可直接构造,
        // 测试 capacity=0 边界(避免除零)
        let stats = TrajectoryStats {
            total_events: 0,
            success_count: 0,
            error_count: 0,
            evicted_count: 0,
            buffer_len: 0,
            capacity: 0,
        };
        assert_eq!(stats.buffer_usage(), 0.0, "capacity=0 时使用率应为 0.0");
    }

    #[test]
    fn test_recording_hook_trait_object_compatible() {
        // 验证 RecordingHook 可作为 Arc<dyn RouteHook> 使用
        let hook: Arc<dyn RouteHook> = Arc::new(RecordingHook::new());
        hook.on_route_completed(make_success_event("q-1", RoutingStrategy::Lite));
        // 由于通过 trait object 调用,只能验证不 panic
    }
}
