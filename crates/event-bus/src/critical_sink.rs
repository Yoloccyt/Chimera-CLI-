//! Critical 事件保底送达 sink(B-a,M0;`arch-refactor-directions-v2-2026-09-12` 方向 B-a)
//!
//! # 背景(痛点证据)
//!
//! §6.2 红线要求 Critical 安全/治理告警事件"确保送达"。既有实现的双通道:
//! broadcast(主通道)+ Critical mpsc 旁路(保障通道)。但旁路消费者由**组合根
//! 显式接线**(`spawn_critical_subscriber`),实测全库 9 处生产 `EventBus::new()`
//! 中仅 `composition.rs::build_app_server` 一处接线 —— 其余 8 条路径
//! (chat/run/exec/quest/parliament/agent/tui/doctor)的 Critical 事件在
//! 无订阅者分支被 **warn + 放弃**,旁路空转,"确保送达"承诺落空。
//!
//! # 本模块的修复
//!
//! 把"必须有对端"从组合根义务下沉为总线自身保证:发布路径的空订阅者分支
//! 改为投递到可插拔的 [`CriticalSink`](CriticalSink)。默认实现
//! [`LogCriticalSink`](LogCriticalSink) 将事件以结构化 `error!` 级日志落盘
//! (携带 event_type/severity/event_id/source 四个定位字段),任何
//! `EventBus::new()` 路径**零接线即获得保底落点**。
//!
//! # 语义边界(设计决策记录)
//!
//! - **不回放**:sink 是"发布时刻的保底落点",不缓存事件、不向后续订阅者补发
//!   —— `subscribe_critical_events` "从订阅时刻开始接收,不回放历史"的既有
//!   语义保持不变(见 `EventBus::subscribe_critical_events` 文档)。
//! - **可能重复**:并发交错下同一事件可能同时进入 mpsc 订阅者与 sink
//!   (检查订阅者与投递非原子)。保底通道的重复落盘无害;丢失才有害。
//!   因此 sink 语义是 **at-least-once 观测**,不是恰好一次投递。
//! - **不替代真实消费者**:组合根的显式订阅者仍是"结构化消费"的推荐路径
//!   (可升级为 TUI 面板/告警管道);sink 只是不可绕过的最低保障。

use crate::types::{NexusEvent, EventSeverity};

/// Critical 事件保底送达 sink(发布路径空订阅者分支的投递目标)
///
/// # 实现约束
///
/// - `Send + Sync + 'static`:`EventBus` 派生 `Clone`(内部 `Arc<dyn CriticalSink>`
///   跨线程共享),sink 必须可安全跨线程调用;发布路径为同步上下文,实现体
///   **不得阻塞、不得 panic**(§4.4 红线:热路径禁止阻塞调用)。
/// - **at-least-once 语义**:见模块文档"可能重复"条目,实现方不得假设恰好一次。
///
/// # 示例
///
/// ```no_run
/// use std::sync::Arc;
/// use event_bus::{EventBus, CriticalSink, NexusEvent};
///
/// struct CollectingSink;
/// impl CriticalSink for CollectingSink {
///     fn on_critical(&self, event: &NexusEvent) {
///         // 生产环境可转发到告警管道/持久化存储;此处仅演示
///         let _ = event.type_name();
///     }
/// }
///
/// // 无 mpsc 订阅者时,Critical 事件投递到 CollectingSink 而非被放弃
/// let bus = EventBus::new()
///     .with_critical_fallback(Arc::new(CollectingSink));
/// ```
pub trait CriticalSink: Send + Sync + 'static {
    /// 接收一条无 mpsc 订阅者的 Critical 事件(保底落点)
    ///
    /// # 参数
    /// - `event`:发布路径判入 Critical 清单(`is_critical_mpsc_event`)的事件
    ///   借用引用;实现方如需持有请自行 clone(发布方不保证生命周期)。
    ///
    /// # 返回
    /// 无返回值:保底通道不允许失败路径(实现方内部降级,不得上抛)。
    fn on_critical(&self, event: &NexusEvent);
}

/// 默认保底 sink — 结构化 `error!` 级日志
///
/// WHY `error!` 而非既有 `warn!`:warn 语义是"告警但事件已放弃";保底送达
/// 之后事件已有落点,error 级更准确表达"该事件未到达任何真实消费者,请运维
/// 关注"。字段与既有 C3 观测一致(event_type/severity/event_id),另补
/// `source`(事件来源 crate,定位发布方)。
pub struct LogCriticalSink;

impl CriticalSink for LogCriticalSink {
    fn on_critical(&self, event: &NexusEvent) {
        tracing::error!(
            event_type = %event.type_name(),
            severity = ?event.severity(),
            event_id = %event.metadata().event_id,
            source = %event.metadata().source,
            is_critical = event.severity() == EventSeverity::Critical,
            "Critical 事件保底送达(无 mpsc 订阅者,结构化日志落点)"
        );
    }
}
