//! 成本熔断守卫 — 累计实际成本 + 超限熔断（ADR-069 Task 6.2）
//!
//! 对应架构层：L10 Interface（与 VendorAdapter 同层，原子无锁，不跨 await）
//!
//! # 熔断状态机
//!
//! ```text
//! CLOSED(累计成本 < 上限) ──record() 跨线──▶ 首次 check() 触发:
//!   ├─ 发布 BudgetExceeded(Critical,防重放只发一次)
//!   └─ 打开熔断 30s(circuit_open_until = now + 30)
//! OPEN(熔断中,check → Err) ──30s 后──▶ HALF_OPEN(半开窗口):
//!   ├─ 放行一个探测请求(同时重开熔断 30s)
//!   └─ 探测后仍超限 → 重新熔断(下一 check 拒绝)
//! ```
//!
//! # 设计决策(WHY)
//! - **check() 内做全部状态机**: `record()` 仅原子累计(fetch_add),
//!   `check()` 是唯一入口点(每次 invoke 传输前必查),跨线检测延迟到
//!   下一次 check,事件语义不变(携带的 current/limit 为发布时刻真实值)。
//! - **Atomic 无锁**: 四个原子字段无 Mutex,天然不跨 await(§4.4 红线 1)。
//! - **CAS 防重放**: `budget_exceeded_reported` 用 compare_exchange 抢占
//!   "发布 + 熔断"职责,并发下仅一个线程发布 BudgetExceeded。
//! - **同步发布**: check() 是同步方法(§4.4 红线 8),事件用
//!   `bus.publish_blocking`——broadcast send 非阻塞,mpsc try_send 非阻塞,
//!   Critical 事件由 bus 内部 mpsc 旁路保证送达,无需 await。
//! - **成本只增不减**: 熔断周期内成本单调累计,半开探测后仍超限恒成立,
//!   故"半开放行即重开熔断",每 30s 窗口恰好放行一个探测请求。

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};

use event_bus::{EventBus, EventMetadata, NexusEvent};

/// 事件源标识(与 adapters.rs EVENT_SOURCE 一致,同源链路可归并)
const EVENT_SOURCE: &str = "mca-gateway";

/// BudgetExceeded 事件的预算类型标识(成本熔断面)
///
/// WHY 字符串契约: BudgetExceeded.budget_type 为自由字符串,消费者
/// (efficiency-monitor / acb-governor)按此值分流成本面告警。
pub const BUDGET_TYPE: &str = "token_efficiency_cost";

/// 熔断开启时长(秒)— 超限后 30s 内拒绝全部请求,之后进入半开窗口
///
/// WHY 30s: 与 mca-gateway 通道熔断器(CircuitBreaker)对齐,
/// 足够短以快速恢复试错,足够长以吸收一次突发成本冲击。
pub const CIRCUIT_OPEN_DURATION_SECS: i64 = 30;

/// 成本熔断错误 — check() 拒绝超限通道的返回面
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CostGuardError {
    /// 熔断中(或首次跨线的当前 check)— 拒绝本次请求
    #[error(
        "cost circuit open: spent {spent} micro >= limit {limit} micro, reopen at unix {reopen_at}"
    )]
    CircuitOpen {
        /// 累计实际成本(微元)
        spent: u64,
        /// 成本上限(微元)
        limit: u64,
        /// 下次半开窗口开启时间(Unix 秒)
        reopen_at: i64,
    },
}

/// 成本熔断守卫 — 累计实际成本 + 超限熔断（ADR-069 Task 6.2）
///
/// 挂在 `VendorAdapter.cost_guard` 上,invoke() 传输前 `check(now)`、
/// 调用成功后 `record(cost.total_micro)`。全部字段为原子量,
/// 无 Mutex、无跨 await(§4.4 红线 1),Arc 共享跨 Clone 一致。
pub struct CostGuard {
    /// 成本上限(微元);None = 不设上限(恒放行)
    budget_limit_micro: Option<u64>,
    /// 累计实际成本(微元,只增不减)
    spent_micro: AtomicU64,
    /// 熔断开启截止时间(Unix 秒;i64::MIN = 从未熔断)
    ///
    /// WHY i64::MIN 哨兵: 首次跨线前 circuit_open_until 无意义,
    /// 用最小哨兵保证 now >= open_until 恒真(不会误判为熔断中)。
    circuit_open_until: AtomicI64,
    /// 是否已发布 BudgetExceeded(防重放,compare_exchange 抢占)
    budget_exceeded_reported: AtomicBool,
    /// 事件总线(None = 静默模式,单测/录播回放用)
    bus: Option<EventBus>,
}

impl CostGuard {
    /// 创建成本熔断守卫(静默模式,bus = None)
    pub fn new(budget_limit_micro: Option<u64>) -> Self {
        Self {
            budget_limit_micro,
            spent_micro: AtomicU64::new(0),
            circuit_open_until: AtomicI64::new(i64::MIN),
            budget_exceeded_reported: AtomicBool::new(false),
            bus: None,
        }
    }

    /// 创建成本熔断守卫并挂接事件总线(超限时发布 BudgetExceeded)
    pub fn with_bus(budget_limit_micro: Option<u64>, bus: Option<EventBus>) -> Self {
        Self {
            bus,
            ..Self::new(budget_limit_micro)
        }
    }

    /// 成本熔断前置检查 — invoke() 传输前必查
    ///
    /// # 状态机(单入口,原子实现)
    /// 1. 未设上限 → 恒 Ok
    /// 2. 累计成本 < 上限 → Ok(未超限)
    /// 3. 熔断中(now < circuit_open_until)→ Err(CircuitOpen)
    /// 4. 不在熔断期且已超限:
    ///    - 首次跨线(CAS 抢占成功)→ 发布 BudgetExceeded + 打开熔断 30s → Err
    ///    - 已发布过(半开窗口)→ 放行一个探测请求 + 重开熔断 30s → Ok
    ///
    /// # 并发语义
    /// CAS 保证 BudgetExceeded 全局只发布一次;半开窗口并发下可能
    /// 多个线程同时落入探测分支(无互斥),最坏多放行一个请求,
    /// 成本守卫属低风险背压面,可接受(注释明示,不做过度防护)。
    pub fn check(&self, now_secs: i64) -> Result<(), CostGuardError> {
        let Some(limit) = self.budget_limit_micro else {
            return Ok(());
        };
        let spent = self.spent_micro.load(Ordering::Relaxed);
        if spent < limit {
            return Ok(());
        }
        let open_until = self.circuit_open_until.load(Ordering::Relaxed);
        if now_secs < open_until {
            return Err(CostGuardError::CircuitOpen {
                spent,
                limit,
                reopen_at: open_until,
            });
        }
        // 抢占"发布 + 熔断"职责:只有第一个跨线的线程能成功
        if self
            .budget_exceeded_reported
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            let reopen_at = now_secs + CIRCUIT_OPEN_DURATION_SECS;
            self.circuit_open_until.store(reopen_at, Ordering::Relaxed);
            self.publish_exceeded(spent, limit);
            return Err(CostGuardError::CircuitOpen {
                spent,
                limit,
                reopen_at,
            });
        }
        // 已发布过 → 半开窗口:放行一个探测请求,同时重开熔断(探测后仍超限)
        self.circuit_open_until
            .store(now_secs + CIRCUIT_OPEN_DURATION_SECS, Ordering::Relaxed);
        Ok(())
    }

    /// 累计实际成本(微元)— invoke() 解码回算成功后调用
    ///
    /// 仅原子累计,跨线检测与事件发布延迟到下一次 check()(唯一入口),
    /// 事件 payload 的 current/limit 为发布时刻真实值,语义不受延迟影响。
    pub fn record(&self, cost_micro: u64) {
        self.spent_micro.fetch_add(cost_micro, Ordering::Relaxed);
    }

    /// 当前累计成本(微元,观测/测试用)
    pub fn spent_micro(&self) -> u64 {
        self.spent_micro.load(Ordering::Relaxed)
    }

    /// 成本上限(微元,观测/测试用)
    pub fn budget_limit_micro(&self) -> Option<u64> {
        self.budget_limit_micro
    }

    /// 发布 BudgetExceeded(同步,publish_blocking 与 async publish 等价旁路)
    ///
    /// WHY 忽略发布错误: 事件是观测面,发布失败不改变熔断决策;
    /// Critical 事件由 bus 内部 mpsc 旁路保证送达(§6.2 红线双通道)。
    fn publish_exceeded(&self, spent: u64, limit: u64) {
        if let Some(bus) = &self.bus {
            let _ = bus.publish_blocking(NexusEvent::BudgetExceeded {
                metadata: EventMetadata::new(EVENT_SOURCE),
                budget_type: BUDGET_TYPE.to_string(),
                current: spent,
                limit,
            });
        }
    }
}

impl std::fmt::Debug for CostGuard {
    /// 手写 Debug: EventBus 未实现 Debug,仅暴露预算/熔断状态快照
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CostGuard")
            .field("budget_limit_micro", &self.budget_limit_micro)
            .field("spent_micro", &self.spent_micro())
            .field(
                "circuit_open_until",
                &self.circuit_open_until.load(Ordering::Relaxed),
            )
            .field(
                "budget_exceeded_reported",
                &self.budget_exceeded_reported.load(Ordering::Relaxed),
            )
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use event_bus::{EventBus, NexusEvent};

    use super::{CostGuard, CostGuardError, CIRCUIT_OPEN_DURATION_SECS};

    // ============================================================
    // 6.4 CostGuard 单元测试 — 熔断/半开/防重放/事件字段
    // ============================================================

    #[test]
    fn no_limit_always_allows() {
        let guard = CostGuard::new(None);
        guard.record(1_000_000);
        assert!(guard.check(1000).is_ok(), "未设上限必须恒 Ok");
        assert!(guard.check(2000).is_ok());
    }

    #[test]
    fn crossing_limit_rejects_with_circuit_open() {
        let guard = CostGuard::new(Some(50));
        assert!(guard.check(1000).is_ok(), "未超限必须放行");
        guard.record(100);
        let err = guard.check(1000).unwrap_err();
        match err {
            CostGuardError::CircuitOpen {
                spent,
                limit,
                reopen_at,
            } => {
                assert_eq!(spent, 100);
                assert_eq!(limit, 50);
                assert_eq!(reopen_at, 1000 + CIRCUIT_OPEN_DURATION_SECS);
            }
        }
        // 熔断期内持续拒绝
        assert!(guard.check(1001).is_err());
    }

    #[test]
    fn crossing_exact_limit_also_rejects() {
        // spent == limit 即视为超限(≥ 语义)
        let guard = CostGuard::new(Some(50));
        guard.record(50);
        assert!(guard.check(1000).is_err());
    }

    #[test]
    fn record_accumulates_spent() {
        let guard = CostGuard::new(None);
        guard.record(10);
        guard.record(20);
        assert_eq!(guard.spent_micro(), 30);
    }

    #[test]
    fn budget_limit_getter_roundtrip() {
        let guard = CostGuard::new(Some(1234));
        assert_eq!(guard.budget_limit_micro(), Some(1234));
        let unlimited = CostGuard::new(None);
        assert_eq!(unlimited.budget_limit_micro(), None);
    }

    #[tokio::test]
    async fn budget_exceeded_published_once_with_fields() {
        // broadcast 纪律:subscribe 必须在 check(publish) 之前同步调用
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let guard = CostGuard::with_bus(Some(50), Some(bus));
        guard.record(100);
        assert!(guard.check(1000).is_err());

        let mut events = 0;
        while let Ok(ev) = rx.recv_timeout(Duration::from_millis(100)).await {
            if let NexusEvent::BudgetExceeded {
                budget_type,
                current,
                limit,
                ..
            } = ev
            {
                events += 1;
                assert_eq!(budget_type, "token_efficiency_cost");
                assert_eq!(current, 100);
                assert_eq!(limit, 50);
            }
        }
        assert_eq!(events, 1, "多次超限只发布一次 BudgetExceeded");

        // 熔断期内重复 check 不重发
        assert!(guard.check(1001).is_err());
        assert!(guard.check(1002).is_err());
        let mut extra = 0;
        while let Ok(ev) = rx.recv_timeout(Duration::from_millis(100)).await {
            if let NexusEvent::BudgetExceeded { .. } = ev {
                extra += 1;
            }
        }
        assert_eq!(extra, 0, "防重放:不得重复发布");
    }

    #[tokio::test]
    async fn no_crossing_publishes_nothing() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let guard = CostGuard::with_bus(Some(50), Some(bus));
        guard.record(30); // 未达上限
        assert!(guard.check(1000).is_ok());
        let mut events = 0;
        while let Ok(ev) = rx.recv_timeout(Duration::from_millis(100)).await {
            if let NexusEvent::BudgetExceeded { .. } = ev {
                events += 1;
            }
        }
        assert_eq!(events, 0, "未超限不得发布 BudgetExceeded");
    }

    #[test]
    fn half_open_allows_single_probe_then_remelts() {
        let guard = CostGuard::new(Some(50));
        guard.record(100);
        // t0 跨线 → 首次 check 熔断
        assert!(guard.check(1000).is_err());
        // 熔断期内拒绝(29s 处仍熔断)
        assert!(guard.check(1029).is_err());
        // 30s 后半开:放行一个探测请求,同时重开熔断
        assert!(guard.check(1030).is_ok(), "半开窗口必须放行探测请求");
        // 探测后仍超限 → 重新熔断
        assert!(guard.check(1031).is_err(), "探测后仍超限必须重新熔断");
        // 下一个半开窗口(1030 + 30 = 1060)
        assert!(guard.check(1060).is_ok());
        assert!(guard.check(1061).is_err());
    }
}
