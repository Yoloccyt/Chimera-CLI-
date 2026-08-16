//! NexusStateChanged EventBus 适配器 — P1-1 观察者接线(L1 Core 深度优化)
//!
//! 对应架构层: **L9 Quest**(适配层;观察点 trait 在 L1 nexus-core)
//!
//! # 背景(WHY 本模块存在)
//!
//! 架构红线"所有状态变更必须通过事件广播"要求 `NexusState` 变更发布
//! `NexusStateChanged` 事件,但 L1 nexus-core 不能依赖 event-bus
//! (会形成 L1 内部环)。nexus-core 采用**依赖倒置**定义最小
//! [`nexus_core::StateChangeListener`] trait,本模块提供生产适配器:
//! 将状态变更通知转换为 `NexusEvent::NexusStateChanged` 事件发布。
//!
//! # 装配方式
//!
//! ```
//! use event_bus::EventBus;
//! use nexus_core::NexusState;
//! use quest_engine::BusStateChangeListener;
//! use std::sync::Arc;
//!
//! let bus = EventBus::new();
//! let mut rx = bus.subscribe(); // 先订阅再发布(红线)
//! let state = NexusState::with_listener(
//!     Arc::new(BusStateChangeListener::new(bus)),
//! );
//! ```
//!
//! # 链式哈希(prev_hash)
//!
//! `NexusStateChanged` 载荷含 `state_hash` + `prev_hash` 链式校验字段。
//! 适配器内部维护上一次哈希(Mutex 保护,锁内仅做 swap,不跨 await),
//! 首个事件 `prev_hash` 为空字符串(创世块语义)。

use std::sync::Mutex;

use event_bus::{EventBus, EventMetadata, NexusEvent};
use nexus_core::{StateChangeKind, StateChangeListener};
use tracing::{debug, warn};

/// 默认事件源标识(EventMetadata.source,依赖方向审计用)
pub const STATE_LISTENER_SOURCE: &str = "quest-engine";

/// EventBus 状态变更适配器 — 把 NexusState 变更通知转为 NexusStateChanged 事件
///
/// # 线程安全
/// `EventBus` Clone 廉价(Arc),`prev_hash` 用 Mutex 保护,
/// 整体满足 Send + Sync,可作为 `Arc<dyn StateChangeListener>` 跨线程共享。
pub struct BusStateChangeListener {
    /// 事件总线(跨层通信唯一通道,§2.2)
    bus: EventBus,
    /// 事件源标识(EventMetadata.source)
    source: String,
    /// 前一状态哈希(链式校验);Mutex 内仅做 swap,持锁时间为 O(1)
    prev_hash: Mutex<String>,
}

impl BusStateChangeListener {
    /// 创建适配器(事件源 = "quest-engine")
    pub fn new(bus: EventBus) -> Self {
        Self {
            bus,
            source: STATE_LISTENER_SOURCE.to_string(),
            prev_hash: Mutex::new(String::new()),
        }
    }

    /// 创建自定义事件源的适配器(测试/多实例场景区分来源)
    pub fn with_source(bus: EventBus, source: impl Into<String>) -> Self {
        Self {
            bus,
            source: source.into(),
            prev_hash: Mutex::new(String::new()),
        }
    }
}

impl StateChangeListener for BusStateChangeListener {
    /// 发布 `NexusStateChanged` 事件
    ///
    /// # 发布模式(红线)
    /// `on_state_changed` 是同步回调(来自 NexusState 写路径),
    /// 使用 `publish_blocking`(sync 方法正确发布模式,§4.4 #8)。
    ///
    /// # 失败处理
    /// 发布失败(如无订阅者)仅 warn 日志,不影响状态变更本身
    /// (回调发生在变更完成之后,监听器异常不得反向影响状态)。
    fn on_state_changed(&self, kind: StateChangeKind, state_hash: String) {
        // 1. 取 prev_hash 并更新链(锁内仅 swap,持锁 O(1),不跨 await)
        let prev_hash = {
            let mut guard = self.prev_hash.lock().unwrap_or_else(|e| e.into_inner());
            std::mem::replace(&mut *guard, state_hash.clone())
        };

        // 2. 构造并发布事件
        let event = NexusEvent::NexusStateChanged {
            metadata: EventMetadata::new(self.source.clone()),
            state_hash,
            prev_hash,
        };
        if let Err(e) = self.bus.publish_blocking(event) {
            warn!(
                kind = ?kind,
                error = %e,
                "NexusStateChanged 发布失败(状态变更本身已完成)"
            );
        } else {
            debug!(kind = ?kind, "NexusStateChanged 已发布");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nexus_core::NexusState;

    /// 端到端:适配器接线后,NexusState 变更事件实际到达订阅者
    #[test]
    fn test_state_change_event_reaches_subscriber() {
        let bus = EventBus::new();
        // 红线:先 subscribe 再发布(broadcast 不缓存历史消息)
        let mut rx = bus.subscribe();
        let state =
            NexusState::with_listener(std::sync::Arc::new(BusStateChangeListener::new(bus)));

        state
            .register_quest(nexus_core::Quest {
                quest_id: "q-1".into(),
                title: "测试".into(),
                tasks: vec![],
                thinking_mode: nexus_core::ThinkingMode::Standard,
                checkpoint_id: None,
                priority: 128,
            })
            .unwrap();

        // 同步路径:publish_blocking 完成后事件已入队
        // WHY 双层解包:try_recv 返回 Result<Option<NexusEvent>>,None = 队列空
        let event = rx
            .try_recv()
            .expect("接收不应出错")
            .expect("应收到 NexusStateChanged 事件");
        match event {
            NexusEvent::NexusStateChanged {
                metadata,
                state_hash,
                prev_hash,
            } => {
                assert_eq!(metadata.source, STATE_LISTENER_SOURCE);
                assert_eq!(state_hash.len(), 64, "state_hash 应为 sha256 hex");
                assert!(prev_hash.is_empty(), "首个事件 prev_hash 为空(创世块)");
            }
            other => panic!("期望 NexusStateChanged,实际 {other:?}"),
        }
    }

    /// 链式哈希:第二次变更的 prev_hash = 第一次的 state_hash
    #[test]
    fn test_prev_hash_chains_across_changes() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let state =
            NexusState::with_listener(std::sync::Arc::new(BusStateChangeListener::new(bus)));

        state.set_global_budget(100);
        state.set_global_budget(200);

        let first = rx
            .try_recv()
            .expect("接收不应出错")
            .expect("第一次变更事件");
        let second = rx
            .try_recv()
            .expect("接收不应出错")
            .expect("第二次变更事件");
        let (h1, h2) = match (first, second) {
            (
                NexusEvent::NexusStateChanged { state_hash: h1, .. },
                NexusEvent::NexusStateChanged {
                    state_hash: h2,
                    prev_hash,
                    ..
                },
            ) => {
                assert_eq!(prev_hash, h1, "第二次 prev_hash 应等于第一次 state_hash");
                (h1, h2)
            }
            _ => panic!("期望两个 NexusStateChanged 事件"),
        };
        assert_ne!(h1, h2, "状态变更后哈希应变化");
    }

    /// 自定义事件源标识
    #[test]
    fn test_with_source_custom_label() {
        let bus = EventBus::new();
        let mut rx = bus.subscribe();
        let state = NexusState::with_listener(std::sync::Arc::new(
            BusStateChangeListener::with_source(bus, "test-source"),
        ));

        state.set_global_budget(1);

        let event = rx.try_recv().expect("接收不应出错").expect("应收到事件");
        match event {
            NexusEvent::NexusStateChanged { metadata, .. } => {
                assert_eq!(metadata.source, "test-source");
            }
            other => panic!("期望 NexusStateChanged,实际 {other:?}"),
        }
    }
}
