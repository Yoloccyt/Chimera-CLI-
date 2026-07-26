//! 订阅构建器 — 强制 "subscribe-then-spawn" 原子化(P1-W4.2)
//!
//! 对应架构:L1 Core(event-bus 内部模块)
//!
//! # 设计目标
//! 将 Week 6 SSRA 教训("bus.subscribe() 必须在 tokio::spawn() 之前同步调用,
//! 否则事件静默丢失")从人为纪律升级为 API 结构保证,通过 TypeState 模式
//! 在编译期强制 subscribe → spawn 顺序。
//!
//! # TypeState 状态机
//! ```text
//! EventBus::subscriber()  →  SubscriberBuilder<Unsubscribed>
//!                                      │
//!                                      ▼ .subscribe()(同步调用 bus.subscribe())
//!                             SubscriberBuilder<Subscribed>
//!                                      │
//!                                      ▼ .spawn()(tokio::spawn)
//!                                  JoinHandle<...>
//! ```
//!
//! `Unsubscribed` 状态没有 `spawn` 方法(编译期拒绝),
//! `Subscribed` 状态才有 `spawn` 方法。
//!
//! # 零运行时开销
//! TypeState 通过类型参数实现,状态标记类型在运行时无额外开销。
//! `Unsubscribed` 为 ZST;`Subscribed` 持有 EventReceiver(必要的运行时状态)。
//!
//! # 与任务描述的偏差说明
//! 任务描述建议 `subscribe()` 返回 `(builder, Receiver)` tuple。但此设计下
//! 调用方拿到 Receiver 后可自行 `tokio::spawn`,绕过 `builder.spawn()`,
//! 破坏原子化保证。本实现将 Receiver 存储在 `Subscribed` 状态内部,
//! `spawn()` 取出 Receiver 传给闭包,确保调用方无法绕过。
//! 同时提供 `into_receiver()` 方法供需要直接处理 Receiver 的场景使用。

use crate::bus::{EventBus, EventReceiver};
use crate::types::NexusEvent;
use std::future::Future;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

// ============================================================
// TypeState 标记类型
// ============================================================

/// 未订阅状态 — TypeState 初始标记
///
/// ZST(零大小类型),仅作为编译期类型标记。此状态下仅可调用 `subscribe()`,
/// **没有** `spawn()` 方法 — 编译期保证无法在订阅前 spawn。
pub struct Unsubscribed;

/// 已订阅状态(普通事件)— 持有 EventReceiver
///
/// 非 ZST:持有 `EventReceiver` 是必要的运行时状态,`spawn()` 时传给闭包。
/// 字段私有,外部无法直接构造,只能通过 `SubscriberBuilder::subscribe()` 转换。
pub struct Subscribed {
    /// 订阅得到的 EventReceiver,spawn 时传给闭包
    receiver: EventReceiver,
}

/// 已订阅状态(Critical 事件)— 持有 mpsc::Receiver
///
/// 与 [`Subscribed`] 区分:Critical 事件走 mpsc 旁路通道(§6.2 红线),
/// 返回 `mpsc::Receiver` 而非 `EventReceiver`。
pub struct CriticalSubscribed {
    /// 订阅 Critical 事件得到的 mpsc::Receiver
    receiver: mpsc::Receiver<NexusEvent>,
}

// ============================================================
// 普通事件订阅构建器
// ============================================================

/// 普通事件订阅构建器 — 强制 subscribe-then-spawn 顺序(P1-W4.2)
///
/// WHY: Week 6 SSRA 教训 — `bus.subscribe()` 必须在 `tokio::spawn()` 之前
/// 同步调用,否则事件静默丢失。此构建器通过 TypeState 模式在编译期强制顺序:
///
/// 1. `EventBus::subscriber()` 返回 `SubscriberBuilder<Unsubscribed>`
/// 2. `.subscribe()` 消费 builder 返回 `SubscriberBuilder<Subscribed>`
/// 3. `.spawn()` 仅在 `Subscribed` 状态下可用
///
/// # 编译期拒绝:未 subscribe 直接 spawn
///
/// `Unsubscribed` 状态没有 `spawn` 方法,以下代码无法编译:
/// ```compile_fail
/// use event_bus::EventBus;
///
/// let bus = EventBus::new();
/// let builder = bus.subscriber();
/// // 编译失败:SubscriberBuilder<Unsubscribed> 没有 spawn 方法
/// builder.spawn(|rx| async { let _ = rx; });
/// ```
///
/// # 正确用法
///
/// ```
/// use event_bus::EventBus;
/// use event_bus::NexusEvent;
///
/// # async fn demo() -> Result<(), Box<dyn std::error::Error>> {
/// let bus = EventBus::new();
/// // 1. 创建构建器(Unsubscribed 状态)
/// let builder = bus.subscriber();
/// // 2. 订阅(同步调用 bus.subscribe(),返回 Subscribed 状态)
/// let builder = builder.subscribe();
/// // 3. spawn 任务(仅 Subscribed 状态可调用)
/// let handle = builder.spawn(|mut rx| async move {
///     if let Ok(event) = rx.recv().await {
///         println!("收到事件: {:?}", event.type_name());
///     }
/// });
/// // bus 在 spawn 之前已订阅,不会丢失事件
/// # handle.await?;
/// # Ok(())
/// # }
/// ```
pub struct SubscriberBuilder<'bus, State> {
    /// EventBus 引用,`subscribe()` 时用于调用 `bus.subscribe()`
    bus: &'bus EventBus,
    /// 状态:Unsubscribed(ZST) 或 Subscribed(持有 EventReceiver)
    state: State,
}

impl<'bus> SubscriberBuilder<'bus, Unsubscribed> {
    /// crate 内构造函数 — 仅 `EventBus::subscriber()` 可调用
    pub(crate) fn new(bus: &'bus EventBus) -> Self {
        Self {
            bus,
            state: Unsubscribed,
        }
    }

    /// 订阅事件流 — 消费 Unsubscribed 状态,返回 Subscribed 状态
    ///
    /// 此方法**同步**调用 `bus.subscribe()`,确保订阅在后续 `spawn()` 之前完成。
    /// 返回的 `SubscriberBuilder<Subscribed>` 持有 EventReceiver,供 `spawn()` 使用。
    ///
    /// # 调用时机(§4.4 反模式 3)
    /// 必须在 `tokio::spawn()` **之前同步调用**,确保不错过后续事件。
    /// 此 API 通过 TypeState 强制此顺序:`spawn()` 仅在 `Subscribed` 状态下可用。
    #[must_use = "subscribe() 返回新的 builder,丢弃将丢失订阅"]
    pub fn subscribe(self) -> SubscriberBuilder<'bus, Subscribed> {
        let receiver = self.bus.subscribe();
        SubscriberBuilder {
            bus: self.bus,
            state: Subscribed { receiver },
        }
    }
}

impl<'bus> SubscriberBuilder<'bus, Subscribed> {
    /// spawn 任务 — 仅在已订阅状态下可用
    ///
    /// 消费 self,将内部持有的 EventReceiver 传给闭包,然后 spawn 闭包返回的 Future。
    /// 此方法确保 spawn 时订阅已完成(由 TypeState 保证)。
    ///
    /// # 参数
    /// `f`: 闭包,接收 `EventReceiver`,返回 `Future`。闭包与 Future 都需 `Send + 'static`。
    ///
    /// # 返回
    /// `JoinHandle<Fut::Output>`,可 await 获取任务结果。
    ///
    /// # 生命周期说明
    /// 虽然构建器持有 `&'bus EventBus` 引用,但 `spawn` 仅提取 `EventReceiver`
    /// (owned,满足 `'static`)传给闭包,不将 bus 引用泄漏到 spawned future。
    pub fn spawn<F, Fut>(self, f: F) -> JoinHandle<Fut::Output>
    where
        F: FnOnce(EventReceiver) -> Fut + Send + 'static,
        Fut: Future + Send + 'static,
        Fut::Output: Send + 'static,
    {
        let receiver = self.state.receiver;
        tokio::spawn(f(receiver))
    }

    /// 提取 EventReceiver — 如果调用方想自己处理(不通过 spawn)
    ///
    /// 此方法等效于旧的 `bus.subscribe()` API,但调用方**明确选择**退出原子化。
    /// 调用方需自行确保订阅后立即 spawn,不要在订阅和 spawn 之间做耗时操作。
    ///
    /// WHY 提供此方法:某些场景需要对接收者做特殊处理(如设置 timeout、
    /// 过滤、与其他 future join 等),不适合用 spawn 闭包。提供逃生舱口
    /// 避免过度限制,但调用方需自行承担顺序保证责任。
    #[must_use = "into_receiver() 返回 EventReceiver,丢弃将丢失订阅"]
    pub fn into_receiver(self) -> EventReceiver {
        self.state.receiver
    }
}

// ============================================================
// Critical 事件订阅构建器
// ============================================================

/// Critical 事件订阅构建器 — 强制 subscribe-then-spawn 顺序(P1-W4.2)
///
/// 与 [`SubscriberBuilder`] 类似,但针对 Critical 事件(§6.2 红线):
/// SkepticVeto/RedTeamAudit/AsaIntervention/BudgetExceeded 必须用 mpsc channel
/// 确保送达。此构建器返回 `mpsc::Receiver` 而非 `EventReceiver`。
///
/// # 编译期拒绝:未 subscribe 直接 spawn
///
/// `Unsubscribed` 状态没有 `spawn` 方法,以下代码无法编译:
/// ```compile_fail
/// use event_bus::EventBus;
///
/// let bus = EventBus::new();
/// let builder = bus.critical_subscriber();
/// // 编译失败:CriticalSubscriberBuilder<Unsubscribed> 没有 spawn 方法
/// builder.spawn(|rx| async { let _ = rx; });
/// ```
///
/// # 正确用法
///
/// ```
/// use event_bus::EventBus;
///
/// # async fn demo() -> Result<(), Box<dyn std::error::Error>> {
/// let bus = EventBus::new();
/// let builder = bus.critical_subscriber();
/// let builder = builder.subscribe();
/// let handle = builder.spawn(|mut rx| async move {
///     if let Some(event) = rx.recv().await {
///         println!("收到 Critical 事件");
///     }
/// });
/// # handle.await?;
/// # Ok(())
/// # }
/// ```
pub struct CriticalSubscriberBuilder<'bus, State> {
    /// EventBus 引用,`subscribe()` 时用于调用 `bus.subscribe_critical_events()`
    bus: &'bus EventBus,
    /// 状态:Unsubscribed(ZST) 或 CriticalSubscribed(持有 mpsc::Receiver)
    state: State,
}

impl<'bus> CriticalSubscriberBuilder<'bus, Unsubscribed> {
    /// crate 内构造函数 — 仅 `EventBus::critical_subscriber()` 可调用
    pub(crate) fn new(bus: &'bus EventBus) -> Self {
        Self {
            bus,
            state: Unsubscribed,
        }
    }

    /// 订阅 Critical 事件 mpsc 旁路通道 — 消费 Unsubscribed 状态
    ///
    /// 同步调用 `bus.subscribe_critical_events()`,返回 `mpsc::Receiver<NexusEvent>`。
    /// 即使在 broadcast Lagged 场景下也能收到 Critical 事件(§6.2 红线)。
    ///
    /// # 调用时机(§4.4 反模式 3)
    /// 必须在 `tokio::spawn()` **之前同步调用**,确保不错过后续 Critical 事件。
    #[must_use = "subscribe() 返回新的 builder,丢弃将丢失订阅"]
    pub fn subscribe(self) -> CriticalSubscriberBuilder<'bus, CriticalSubscribed> {
        let receiver = self.bus.subscribe_critical_events();
        CriticalSubscriberBuilder {
            bus: self.bus,
            state: CriticalSubscribed { receiver },
        }
    }
}

impl<'bus> CriticalSubscriberBuilder<'bus, CriticalSubscribed> {
    /// spawn 任务 — 仅在已订阅状态下可用
    ///
    /// 消费 self,将内部持有的 `mpsc::Receiver` 传给闭包,然后 spawn。
    pub fn spawn<F, Fut>(self, f: F) -> JoinHandle<Fut::Output>
    where
        F: FnOnce(mpsc::Receiver<NexusEvent>) -> Fut + Send + 'static,
        Fut: Future + Send + 'static,
        Fut::Output: Send + 'static,
    {
        let receiver = self.state.receiver;
        tokio::spawn(f(receiver))
    }

    /// 提取 mpsc::Receiver — 如果调用方想自己处理
    ///
    /// 与 [`SubscriberBuilder::into_receiver`] 类似,提供逃生舱口。
    /// 调用方需自行确保订阅后立即 spawn。
    #[must_use = "into_receiver() 返回 mpsc::Receiver,丢弃将丢失订阅"]
    pub fn into_receiver(self) -> mpsc::Receiver<NexusEvent> {
        self.state.receiver
    }
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::EventMetadata;

    /// 辅助:构造普通测试事件
    fn make_test_event() -> NexusEvent {
        NexusEvent::QuestCreated {
            metadata: EventMetadata::new("test-source"),
            quest_id: "q-001".into(),
            title: "测试任务".into(),
            task_count: 1,
        }
    }

    /// 辅助:构造 Critical 测试事件(SkepticVeto,走 mpsc 旁路)
    fn make_critical_event() -> NexusEvent {
        NexusEvent::SkepticVeto {
            metadata: EventMetadata::new("test-source"),
            quest_id: "q-001".into(),
            veto_reason: "unsafe op".into(),
            frozen_capabilities: vec!["cap-1".into()],
        }
    }

    #[tokio::test]
    async fn test_subscriber_builder_subscribe_then_spawn() {
        let bus = EventBus::new();
        let builder = bus.subscriber().subscribe();
        let handle = builder.spawn(|mut rx| async move { rx.recv().await });

        bus.publish(make_test_event()).await.unwrap();

        let result = tokio::time::timeout(std::time::Duration::from_secs(1), handle)
            .await
            .expect("spawn 任务应在 1s 内完成")
            .expect("spawn 任务不应 panic");
        assert!(result.is_ok(), "应成功收到事件");
    }

    #[tokio::test]
    async fn test_critical_subscriber_builder_subscribe_then_spawn() {
        let bus = EventBus::new();
        let builder = bus.critical_subscriber().subscribe();
        let handle = builder.spawn(|mut rx| async move { rx.recv().await });

        bus.publish_critical(make_critical_event()).await.unwrap();

        let result = tokio::time::timeout(std::time::Duration::from_secs(1), handle)
            .await
            .expect("spawn 任务应在 1s 内完成")
            .expect("spawn 任务不应 panic");
        assert!(result.is_some(), "应通过 mpsc 旁路收到 Critical 事件");
    }

    #[tokio::test]
    async fn test_into_receiver_extraction() {
        let bus = EventBus::new();
        let builder = bus.subscriber().subscribe();
        let mut rx = builder.into_receiver();

        // WHY 使用同一事件实例:make_test_event() 每次生成新 UUID/timestamp,
        // 必须用 clone 比较同一实例,否则 assert_eq 永远失败
        let event = make_test_event();
        bus.publish(event.clone()).await.unwrap();
        let received = rx.recv().await.unwrap();
        assert_eq!(received, event);
    }

    #[tokio::test]
    async fn test_critical_into_receiver_extraction() {
        let bus = EventBus::new();
        let builder = bus.critical_subscriber().subscribe();
        let mut rx = builder.into_receiver();

        bus.publish_critical(make_critical_event()).await.unwrap();
        let event = rx.recv().await;
        assert!(event.is_some(), "应通过 mpsc 旁路收到 Critical 事件");
    }
}
