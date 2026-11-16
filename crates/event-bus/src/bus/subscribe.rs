//! 订阅面 — 事件的接收路径（ADR-314 W6 / BC-01 按职责拆分）
//!
//! 拆分自原 `bus.rs`（生产面 2060 行 → 五职责子模块）。
//!
//! WHY 独立成模块：订阅面与发布面是**两套独立的语义**——发布决定事件
//! 投到哪儿，订阅决定谁能看到它们（普通广播 / Critical mpsc 旁路 /
//! 按 topic 过滤 / TypeState 构建器）。混在一个文件里时，两套语义互相
//! 干扰，且"订阅必须在 spawn 之前同步建立"这类时序红线难以一眼看出。

use std::sync::atomic::Ordering;

use tokio::sync::mpsc;

use crate::types::NexusEvent;

use super::{EventBus, EventReceiver, CRITICAL_CHANNEL_CAPACITY};

impl EventBus {
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
    ///
    /// # 与保底 sink 的关系(B-a,M0)
    /// 本订阅建立**之前**发布的 Critical 事件投递到保底 sink(默认结构化
    /// `error!` 日志,见 [`critical_sink`](crate::critical_sink) 模块文档)——
    /// sink 不缓存、不回放,"从订阅时刻开始接收"语义保持不变;建立**之后**
    /// 发布的事件仅走 mpsc(sink 不再收到,避免双消费噪声)。组合根自检以
    /// [`has_critical_subscribers`](Self::has_critical_subscribers)==true 为目标态。
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

    /// 获取 Critical 空订阅者保底投递累计次数(B-a,M0 新增)
    ///
    /// 返回发布时因 mpsc 旁路无任何订阅者而改投保底 sink
    /// ([`CriticalSink`](crate::critical_sink::CriticalSink),默认结构化
    /// `error!` 日志)的 Critical 事件总数。单调递增,不重置。
    ///
    /// # 运维语义(与 [`critical_dropped_count`](Self::critical_dropped_count) 的分工)
    /// - 本计数持续增长 = 组合根**未接线真实消费者**,Critical 事件仅落保底日志
    ///   (接线覆盖问题;处置:组合根调用 `subscribe_critical_events` + spawn 消费者)
    /// - `critical_dropped_count` 增长 = 消费者已接线但通道满(容量问题)
    ///
    /// WHY Relaxed 内存序:同 `critical_dropped_count`,统计指标非控制流信号。
    pub fn critical_no_subscriber_total(&self) -> u64 {
        self.critical_no_subscriber_count.load(Ordering::Relaxed)
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
    pub(super) fn send_critical_mpsc(&self, event: &NexusEvent) {
        // WHY unwrap_or_else: 中毒锁降级访问而非 panic(见 subscribe_critical_events 注释)。
        let mut guard = self.critical_tx.lock().unwrap_or_else(|e| e.into_inner());
        if guard.is_empty() {
            // B-a(M0): 保底送达 — 空订阅者不再是"warn + 放弃"。
            // C3(2026-09-04) 原实现仅 warn 告警后 return,事件在旁路维度上
            // 无任何落点(修复前 9 条生产路径中 8 条处于此状态)。
            // 现投递到可插拔 fallback sink(默认 LogCriticalSink 结构化 error!),
            // 使任何 EventBus::new() 路径零接线即获得 Critical 保底落点。
            // 语义边界(见 critical_sink 模块文档):
            // - 不回放:subscribe_critical_events "订阅时刻起接收"语义不变
            // - at-least-once:并发交错下可能与后续 mpsc 投递重复,重复无害
            // 计数独立于 critical_dropped_count:后者=订阅者存在但通道满;
            // 本计数=完全无订阅者走了保底(接线覆盖问题,运维处置不同)。
            self.critical_no_subscriber_count
                .fetch_add(1, Ordering::Relaxed);
            self.critical_fallback.on_critical(event);
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
        // 遍历所有 Sender,try_send 失败时按错误类型处理（无优先级选择:容量满即丢,
        // 丢弃数经 critical_dropped_count 计数可见）
        // - Full:递增丢弃计数,保留 Sender(临时满载)
        // - Closed:移除 Sender(receiver 已 drop)
        // delivered:本次成功投递到的活跃订阅者数(B-a 用于 stale 检测)
        let mut delivered = 0usize;
        guard.retain(|tx| match tx.try_send(event.clone()) {
            Ok(()) => {
                delivered += 1;
                true // 投递成功,保留
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                // 容量满:丢弃本条（无优先级判定）,递增计数,保留 Sender 待后续
                // WHY fetch_add 而非 store(load + 1):原子操作避免读改写竞态
                self.critical_dropped_count.fetch_add(1, Ordering::Relaxed);
                true // 保留 Sender(临时满载,下次可能成功)
            }
            Err(mpsc::error::TrySendError::Closed(_)) => false, // receiver 已 drop,移除
        });
        // B-a(M0): stale 订阅者兜底 — 订阅者全部 drop 后的首个 Critical 事件,
        // is_empty 检查时 Vec 尚含失效 Sender(惰性清理发生在本次 retain),
        // 事件 try_send 全部 Closed → 既没进 mpsc 也不该无痕消失。此处投递
        // fallback(与空订阅者分支同一落点),修复"订阅后 drop → 首事件静默丢失"缺口。
        // 边界:guard 仍非空(存在 Full 满载活跃订阅者)时不投 fallback ——
        // 那是 P1-W2.1 既定的优先级采样丢弃(有 critical_dropped_count 指标与告警),
        // 向 fallback 转投会破坏背压设计(满载刷屏),保持原语义。
        if delivered == 0 && guard.is_empty() {
            self.critical_no_subscriber_count
                .fetch_add(1, Ordering::Relaxed);
            self.critical_fallback.on_critical(event);
        }
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
    /// # 语义(B-a 后更新)
    /// `false` 不再意味着事件"无落点"——此时 Critical 事件投递到保底 sink
    /// (默认结构化 error! 日志,投递次数见
    /// [`critical_no_subscriber_total`](Self::critical_no_subscriber_total));
    /// 但"仅保底日志"仍低于"真实消费者"的保障级别,组合根自检建议维持
    /// `has_critical_subscribers()==true` 为目标态(显式订阅者可升级为
    /// 面板/告警管道消费)。
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
    /// 清单,当前 18 类;第三轮 T1.3 纠漂)的 mpsc 旁路通道。
    ///
    /// 与 [`subscriber`](Self::subscriber) 区别:返回 `mpsc::Receiver` 而非 `EventReceiver`,
    /// 确保在 broadcast Lagged 场景下仍能收到 Critical 事件。
    #[must_use = "critical_subscriber() 返回构建器,需调用 subscribe() 完成订阅"]
    pub fn critical_subscriber(
        &self,
    ) -> crate::subscriber::CriticalSubscriberBuilder<'_, crate::subscriber::Unsubscribed> {
        crate::subscriber::CriticalSubscriberBuilder::new(self)
    }
}
