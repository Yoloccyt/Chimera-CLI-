//! 事件总线实现 — 基于 tokio::broadcast 的 typed broadcast bus
//!
//! 对应架构:L1 Core,所有跨层通信的唯一通道(§2.2 依赖铁律)
//!
//! # 设计要点
//! - 封装 `tokio::broadcast::channel`,提供类型安全的发布订阅
//! - 序列化采用 MessagePack(ADR-004),提供跨进程投递能力
//! - 关键事件标注 Critical,背压策略据此保护(见 backpressure 模块)
//! - 所有 async fn 满足 Send 约束,可被 tokio::spawn

use crate::error::EventBusError;
use crate::logging::BusLogger;
use crate::types::{EventSeverity, NexusEvent};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};
use tracing::warn;

/// 默认广播容量
///
/// WHY:1024 平衡内存占用与突发流量。每个 NexusEvent 约 200-500 字节,
/// 1024 容量约占 0.5MB,可吸收短时突发;持续高吞吐应增大容量或加背压策略。
pub const DEFAULT_CAPACITY: usize = 1024;

/// 判断事件是否走 mpsc 旁路通道(Critical 安全告警事件)
///
/// §6.2 红线要求:Critical 安全事件(SkepticVeto/RedTeamAudit/AsaIntervention/
/// BudgetExceeded)必须用 mpsc channel 确保送达,避免 broadcast 在 Lagged 场景下
/// 丢失。这与 `NexusEvent::severity()` 部分重叠但语义不同:
/// - `severity()` 是事件总线背压级别(同步函数,AsaIntervention 即使 Block 级
///   也返回 Normal,因为不依赖运行时值)
/// - `is_critical_mpsc_event` 是 mpsc 旁路通道判定,4 类安全告警事件强制走 mpsc
///
/// WHY 单独定义:AsaIntervention 的 severity() 返回 Normal,但 Block 级别在语义上
/// 等价于 Critical(见 types.rs:807-810 注释),必须通过 mpsc 旁路确保投递。
fn is_critical_mpsc_event(event: &NexusEvent) -> bool {
    matches!(
        event,
        NexusEvent::SkepticVeto { .. }
            | NexusEvent::RedTeamAudit { .. }
            | NexusEvent::AsaIntervention { .. }
            | NexusEvent::BudgetExceeded { .. }
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
/// 4 类 Critical 安全告警事件(SkepticVeto/RedTeamAudit/AsaIntervention/
/// BudgetExceeded)额外走 mpsc 旁路通道,确保在 broadcast Lagged 场景下
/// 仍能被订阅者接收。订阅者通过 [`subscribe_critical_events`](Self::subscribe_critical_events)
/// 获取 mpsc Receiver。旁路通道按需初始化(首次订阅时创建),无订阅者时
/// `publish` 仅走 broadcast 不报错。
#[derive(Clone)]
pub struct EventBus {
    sender: broadcast::Sender<NexusEvent>,
    /// 通道容量(创建时固定,broadcast::Sender 不暴露 capacity())
    capacity: usize,
    /// 可选日志记录器(Arc 共享,跨 Clone 共享同一计数器)
    logger: Option<Arc<BusLogger>>,
    /// Critical 事件 mpsc 旁路通道(§6.2 红线双通道化)
    ///
    /// WHY Arc<Mutex<Vec<UnboundedSender>>>:
    /// - `Vec<UnboundedSender>` fan-out 模式:每个 subscribe_critical_events
    ///   调用创建独立 mpsc channel,Sender 入 Vec,Receiver 返回给订阅者
    /// - `Mutex` 同步 Vec 修改(订阅/发送互斥)
    /// - `Arc` 使 EventBus 保留 Clone 派生(所有 Clone 副本共享同一 Vec)
    /// - `UnboundedSender` 实现 Clone,EventBus Clone 副本可向同一 Vec 投递
    /// - receiver drop 时 send 返回 Err,send_critical_mpsc 静默忽略并定期
    ///   清理失效 sender(避免 Vec 无限增长)
    critical_tx: Arc<Mutex<Vec<mpsc::UnboundedSender<NexusEvent>>>>,
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
    /// 4 类 Critical 安全告警事件(SkepticVeto/RedTeamAudit/AsaIntervention/
    /// BudgetExceeded)额外走 mpsc 旁路通道,确保在 broadcast Lagged 场景下
    /// 仍能被 `subscribe_critical_events` 订阅者接收。旁路通道未初始化时
    /// (无 Critical 订阅者)仅走 broadcast,不报错。
    #[allow(clippy::unused_async)]
    pub async fn publish(&self, event: NexusEvent) -> Result<(), EventBusError> {
        let subscriber_count = self.sender.receiver_count();

        // 记录发布日志(若已启用日志埋点)
        if let Some(logger) = &self.logger {
            logger.log_publish(&event, subscriber_count);
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

        // §6.2 红线双通道:4 类 Critical 安全告警事件额外走 mpsc 旁路
        // WHY 先 mpsc 后 broadcast:mpsc UnboundedSender::send 不会阻塞,
        // 先投递 mpsc 确保 Critical 订阅者必收;broadcast 仍走以保证向后兼容
        if is_critical_mpsc_event(&event) {
            self.send_critical_mpsc(&event);
        }

        // broadcast::Sender::send 仅在无活跃接收者时返回 Err;
        // 按设计无订阅者时事件被静默丢弃,不视为错误。
        let _ = self.sender.send(event);
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
    /// 与 `publish` 一致:4 类 Critical 安全告警事件额外走 mpsc 旁路通道。
    pub fn publish_blocking(&self, event: NexusEvent) -> Result<(), EventBusError> {
        let subscriber_count = self.sender.receiver_count();

        // 记录发布日志(若已启用日志埋点)
        if let Some(logger) = &self.logger {
            logger.log_publish(&event, subscriber_count);
        }

        // SubTask 17.2:Critical 事件无订阅者告警(与 publish 保持一致)
        if subscriber_count == 0 && event.severity() == EventSeverity::Critical {
            warn!(
                event_type = event.type_name(),
                "Critical 事件无订阅者,事件将被丢弃(同步发布)"
            );
        }

        // §6.2 红线双通道:4 类 Critical 安全告警事件额外走 mpsc 旁路
        if is_critical_mpsc_event(&event) {
            self.send_critical_mpsc(&event);
        }

        // 与 publish 保持一致:无订阅者时静默丢弃事件。
        let _ = self.sender.send(event);
        Ok(())
    }

    /// 显式发布 Critical 事件到双通道(broadcast + mpsc 旁路)
    ///
    /// 调用方明确知道事件为 Critical 时使用此方法,语义清晰。
    /// 内部行为与 [`publish`](Self::publish) 对 4 类 Critical 事件的处理一致,
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
    /// 内部行为与 [`publish_blocking`](Self::publish_blocking) 对 4 类 Critical
    /// 事件的处理一致,但不依赖 `is_critical_mpsc_event` 判定。
    pub fn publish_critical_blocking(&self, event: NexusEvent) -> Result<(), EventBusError> {
        self.send_critical_mpsc(&event);
        let _ = self.sender.send(event);
        Ok(())
    }

    /// 订阅 Critical 事件 mpsc 旁路通道
    ///
    /// §6.2 红线:Critical 安全事件(SkepticVeto/RedTeamAudit/AsaIntervention/
    /// BudgetExceeded)必须用 mpsc channel 确保送达。此方法返回 mpsc Receiver,
    /// 订阅者通过它接收 Critical 事件,即使在 broadcast Lagged 场景下也不会丢失。
    ///
    /// # fan-out 多订阅者
    /// 每次调用创建独立 mpsc channel,Sender 入 `Vec` 内部状态,Receiver 返回。
    /// 后续发布的 Critical 事件会向 `Vec` 中所有 Sender 投递(fan-out 广播)。
    /// receiver drop 后,对应 Sender 的 `send` 返回 Err,下次发送时被清理。
    ///
    /// # 调用时机(§4.4 反模式 3)
    /// 必须在 `tokio::spawn()` **之前同步调用**此方法,确保不会错过后续发布的
    /// Critical 事件。在 spawn 的 async block 内调用可能导致事件静默丢失。
    pub fn subscribe_critical_events(&self) -> mpsc::UnboundedReceiver<NexusEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        // WHY unwrap_or_else: 中毒锁降级访问内部数据而非 panic。
        // EventBus 是核心组件,前任持有者 panic 导致 poison 后,
        // 继续抛 panic 会中断所有事件发布,降级为访问中毒数据更稳健(§4.1 红线)。
        // 与 csn-substitutor/substitutor.rs 的 register_lock 处理方式保持一致。
        let mut guard = self.critical_tx.lock().unwrap_or_else(|e| e.into_inner());
        guard.push(tx);
        rx
    }

    /// 向 mpsc 旁路通道投递 Critical 事件(内部辅助方法)
    ///
    /// fan-out 投递:遍历 `Vec<UnboundedSender>`,向每个 Sender 投递事件 clone。
    /// `send` 返回 Err 的 Sender(receiver 已 drop)被从 Vec 中移除,避免无限增长。
    /// Vec 为空(无 Critical 订阅者)时静默跳过,不报错。
    fn send_critical_mpsc(&self, event: &NexusEvent) {
        // WHY unwrap_or_else: 中毒锁降级访问而非 panic(见 subscribe_critical_events 注释)。
        let mut guard = self.critical_tx.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_empty() {
            return;
        }
        // 保留 send 成功的 sender,移除 send 失败的(receiver 已 drop)
        // WHY retain:O(n) 一次遍历完成发送 + 清理,避免两次遍历
        guard.retain(|tx| tx.send(event.clone()).is_ok());
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
        EventReceiver::from_broadcast(self.sender.subscribe(), subscriber_id, self.logger.clone())
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
        );
        crate::topic::FilteredSubscriber::new(receiver, topics)
    }

    /// 获取当前订阅者数量
    pub fn subscriber_count(&self) -> usize {
        self.sender.receiver_count()
    }

    /// 获取通道容量
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
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
    ) -> Self {
        EventReceiver {
            inner,
            subscriber_id,
            logger,
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

#[cfg(test)]
mod tests {
    use super::*;
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
}
