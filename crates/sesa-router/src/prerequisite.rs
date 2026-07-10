//! 前置事件校验器 — 确保 SESA 激活前上游路由已完成
//!
//! 对应架构层:L6 Router
//! 设计决策(2026-07-09):默认启用,安全优先,强制五层路由顺序
//!
//! # 设计动机
//! SESA 是五层路由末端(OSA → KVBSR → FaaE → GEA → SESA),若上游
//! (OSA/KVBSR/FaaE)未完成就激活 SESA,会导致稀疏激活基于不完整的路由结果,
//! 违反 Ω-Sparse 原则。`PrerequisiteChecker` 在 `activate()` 入口校验三个
//! 必需上游事件:
//! - `OmniSparseMasksComputed`(OSA 完成)
//! - `ToolsRouted`(KVBSR/FaaE 完成)
//! - `ExpertRouted`(FaaE 路由完成)
//!
//! # 订阅模式(§4.4 反模式 #3)
//! 构造时同步调用 `bus.subscribe_filtered()`,确保不错过后续事件。
//! 内部用 `FilteredSubscriber` 仅接收 `EventTopic::Routing` 事件,减少无关事件占用。
//!
//! # 并发安全
//! 内部状态用单个 `Mutex<PrerequisiteInner>` 保护,`check()` 是 `&self` 方法,
//! 可在 `SesaRouter::activate(&self)` 中直接调用。锁不跨 `.await`(check 是同步方法)。

use std::collections::HashSet;
use std::sync::Mutex;

use event_bus::{EventBus, EventTopic, FilteredSubscriber, NexusEvent};
use tracing::warn;

use crate::error::SesaError;

/// 前置事件校验状态 — 跟踪三个上游路由事件是否已收到
#[derive(Debug, Clone, Default)]
struct PrerequisiteState {
    /// 是否已收到 OmniSparseMasksComputed(OSA 完成)
    osa_done: bool,
    /// 是否已收到 ToolsRouted(KVBSR/FaaE 完成)
    tools_routed: bool,
    /// 是否已收到 ExpertRouted(FaaE 路由完成)
    expert_routed: bool,
}

/// PrerequisiteChecker 内部可变状态(Mutex 保护)
///
/// WHY 单 Mutex:state 与 subscriber 放在同一锁内,避免双锁死锁风险,
/// 且 drain + check 在同一临界区内完成,语义更清晰。
struct PrerequisiteInner {
    /// 校验状态
    state: PrerequisiteState,
    /// FilteredSubscriber 用于接收 Routing 事件(None 表示禁用)
    subscriber: Option<FilteredSubscriber>,
}

/// 前置事件校验器 — 确保 SESA 激活前五层路由顺序已完成
///
/// # 默认启用(安全优先)
/// SESA 是五层路由末端,若上游(OSA/KVBSR/FaaE)未完成就激活 SESA,
/// 会导致稀疏激活基于不完整的路由结果,违反 Ω-Sparse 原则。
/// 默认启用强制五层路由顺序,仅在测试或降级场景下禁用。
///
/// # 订阅模式(§4.4 反模式 #3)
/// 构造时同步调用 `bus.subscribe_filtered()`,确保不错过后续事件。
/// `check()` 内部用 `try_recv` 非阻塞 drain 已缓冲事件,不阻塞 async runtime。
///
/// # 示例
/// ```no_run
/// use sesa_router::PrerequisiteChecker;
/// use event_bus::EventBus;
///
/// let bus = EventBus::new();
/// let checker = PrerequisiteChecker::new(&bus);
/// // 在 activate() 入口调用 check()
/// match checker.check() {
///     Ok(()) => { /* 前置条件满足,可以激活 */ }
///     Err(e) => { /* 缺少上游事件,等待后重试 */ }
/// }
/// ```
pub struct PrerequisiteChecker {
    /// 内部状态(Mutex 保护,async 安全:不跨 await 持锁)
    inner: Mutex<PrerequisiteInner>,
    /// 是否启用(false 时跳过校验,仅用于测试或降级场景)
    enabled: bool,
}

impl PrerequisiteChecker {
    /// 创建启用状态的 PrerequisiteChecker 并订阅 Routing 事件
    ///
    /// # 订阅时机(§4.4 反模式 #3)
    /// 必须在 `tokio::spawn` 之前同步调用此构造函数,确保不错过后续事件。
    /// 内部调用 `bus.subscribe_filtered()` 仅接收 `EventTopic::Routing` 事件。
    pub fn new(event_bus: &EventBus) -> Self {
        let mut topics = HashSet::new();
        topics.insert(EventTopic::Routing);
        // §4.4 反模式 #3:subscribe 必须在 spawn 之前同步调用
        let subscriber = event_bus.subscribe_filtered(topics);
        Self {
            inner: Mutex::new(PrerequisiteInner {
                state: PrerequisiteState::default(),
                subscriber: Some(subscriber),
            }),
            enabled: true,
        }
    }

    /// 创建禁用状态的 PrerequisiteChecker(降级场景或测试用)
    ///
    /// WHY 禁用模式:既有测试或降级场景不需要前置校验,
    /// 禁用后 `check()` 直接返回 Ok(()),行为与未引入 PrerequisiteChecker 前一致。
    pub fn disabled() -> Self {
        Self {
            inner: Mutex::new(PrerequisiteInner {
                state: PrerequisiteState::default(),
                subscriber: None,
            }),
            enabled: false,
        }
    }

    /// 是否启用
    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// 校验前置条件是否满足
    ///
    /// 内部先 drain 已缓冲的 Routing 事件(更新状态),再检查三个上游事件是否齐备。
    ///
    /// # 返回值
    /// - `Ok(())`:所有上游路由已完成,可以激活 SESA
    /// - `Err(PrerequisiteNotMet)`:缺少上游事件,`activate()` 应拒绝执行
    ///
    /// # 并发安全
    /// 使用 `Mutex` 保护内部状态,锁在方法返回前释放,不跨 `.await`。
    /// WHY `&self`:使 `SesaRouter::activate(&self)` 可直接调用,无需额外 `Mutex` 包装。
    pub fn check(&self) -> Result<(), SesaError> {
        if !self.enabled {
            return Ok(());
        }

        // 获取锁(中毒锁降级,与 EventBus 一致策略)
        let mut inner = match self.inner.lock() {
            Ok(guard) => guard,
            Err(e) => e.into_inner(),
        };

        // 非阻塞 drain 已缓冲的 Routing 事件
        self.drain_pending_events(&mut inner);

        // 检查三个上游事件是否齐备
        let mut missing = Vec::new();
        if !inner.state.osa_done {
            missing.push("OmniSparseMasksComputed");
        }
        if !inner.state.tools_routed {
            missing.push("ToolsRouted");
        }
        if !inner.state.expert_routed {
            missing.push("ExpertRouted");
        }

        if missing.is_empty() {
            Ok(())
        } else {
            Err(SesaError::PrerequisiteNotMet {
                missing_events: missing,
            })
        }
    }

    /// 非阻塞地 drain 已缓冲的 Routing 事件,更新内部状态
    ///
    /// WHY 非阻塞:使用 `try_recv` 而非 `recv().await`,避免阻塞 async runtime。
    /// `try_recv` 返回 `None` 时表示无新事件,正常退出循环。
    ///
    /// WHY 在持锁状态下 drain:state 与 subscriber 在同一锁内,
    /// drain 期间保持锁可避免并发 check 之间的状态不一致。
    fn drain_pending_events(&self, inner: &mut PrerequisiteInner) {
        let Some(subscriber) = &mut inner.subscriber else {
            return;
        };
        // 循环 drain 直到缓冲区为空或出错
        loop {
            match subscriber.try_recv() {
                Ok(Some(event)) => {
                    Self::update_state(&mut inner.state, &event);
                }
                Ok(None) => break, // 缓冲区为空
                Err(e) => {
                    // WHY 仅记日志不传播:try_recv 出错(通道关闭/lag)不应阻塞激活流程,
                    // 降级为使用当前已积累的状态判断(可能误判为缺少事件,但安全优先)
                    warn!(error = %e, "PrerequisiteChecker drain 事件失败,降级使用当前状态");
                    break;
                }
            }
        }
    }

    /// 根据收到的事件更新校验状态(纯函数,易于单元测试)
    fn update_state(state: &mut PrerequisiteState, event: &NexusEvent) {
        match event {
            NexusEvent::OmniSparseMasksComputed { .. } => state.osa_done = true,
            NexusEvent::ToolsRouted { .. } => state.tools_routed = true,
            NexusEvent::ExpertRouted { .. } => state.expert_routed = true,
            // 其他 Routing 事件(SesaActivationCompleted/ExpertRegistered 等)不影响前置条件
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use event_bus::EventMetadata;

    /// 构造 OmniSparseMasksComputed 事件
    fn make_osa_event() -> NexusEvent {
        NexusEvent::OmniSparseMasksComputed {
            metadata: EventMetadata::new("osa-coordinator"),
            mask_hash: "mask-001".into(),
            sparsity: 0.6,
            context_mask: vec!["file-1".into()],
        }
    }

    /// 构造 ToolsRouted 事件
    fn make_tools_routed_event() -> NexusEvent {
        NexusEvent::ToolsRouted {
            metadata: EventMetadata::new("kvbsr-router"),
            routed_count: 8,
            top_tool: "tool-1".into(),
            routed_tools: vec!["tool-1".into()],
        }
    }

    /// 构造 ExpertRouted 事件
    fn make_expert_routed_event() -> NexusEvent {
        NexusEvent::ExpertRouted {
            metadata: EventMetadata::new("faae-router"),
            routed_tool: "tool-1".into(),
            confidence: 0.92,
        }
    }

    #[test]
    fn test_disabled_checker_always_ok() {
        let checker = PrerequisiteChecker::disabled();
        assert!(!checker.is_enabled());
        // 禁用状态下 check 应直接返回 Ok
        assert!(checker.check().is_ok());
    }

    #[test]
    fn test_enabled_checker_is_enabled() {
        let bus = EventBus::new();
        let checker = PrerequisiteChecker::new(&bus);
        assert!(checker.is_enabled());
    }

    #[test]
    fn test_check_blocks_without_upstream_events() {
        let bus = EventBus::new();
        let checker = PrerequisiteChecker::new(&bus);
        // 未发布任何上游事件
        let result = checker.check();
        assert!(result.is_err());
        match result {
            Err(SesaError::PrerequisiteNotMet { missing_events }) => {
                assert_eq!(missing_events.len(), 3, "应缺少全部 3 个事件");
                assert!(missing_events.contains(&"OmniSparseMasksComputed"));
                assert!(missing_events.contains(&"ToolsRouted"));
                assert!(missing_events.contains(&"ExpertRouted"));
            }
            _ => panic!("期望 PrerequisiteNotMet 错误"),
        }
    }

    #[test]
    fn test_check_passes_with_all_upstream_events() {
        let bus = EventBus::new();
        let checker = PrerequisiteChecker::new(&bus);

        // 发布三个上游事件
        bus.publish_blocking(make_osa_event())
            .expect("发布 OSA 失败");
        bus.publish_blocking(make_tools_routed_event())
            .expect("发布 ToolsRouted 失败");
        bus.publish_blocking(make_expert_routed_event())
            .expect("发布 ExpertRouted 失败");

        // drain + check 应通过
        let result = checker.check();
        assert!(
            result.is_ok(),
            "三事件齐备应通过校验, 实际: {:?}",
            result.err()
        );
    }

    #[test]
    fn test_check_partial_events_reports_missing() {
        let bus = EventBus::new();
        let checker = PrerequisiteChecker::new(&bus);

        // 只发布 OSA 事件
        bus.publish_blocking(make_osa_event()).expect("发布失败");

        let result = checker.check();
        assert!(result.is_err());
        match result {
            Err(SesaError::PrerequisiteNotMet { missing_events }) => {
                assert_eq!(missing_events.len(), 2, "应缺少 2 个事件");
                assert!(!missing_events.contains(&"OmniSparseMasksComputed"));
                assert!(missing_events.contains(&"ToolsRouted"));
                assert!(missing_events.contains(&"ExpertRouted"));
            }
            _ => panic!("期望 PrerequisiteNotMet 错误"),
        }
    }

    #[test]
    fn test_update_state_ignores_irrelevant_events() {
        let mut state = PrerequisiteState::default();
        // 不相关的事件不应更新状态
        let irrelevant = NexusEvent::SesaActivationCompleted {
            metadata: EventMetadata::new("sesa"),
            total_experts: 10,
            active_experts: 4,
            sparsity_ratio: 0.4,
            latency_us: 100,
        };
        PrerequisiteChecker::update_state(&mut state, &irrelevant);
        assert!(!state.osa_done);
        assert!(!state.tools_routed);
        assert!(!state.expert_routed);

        // 三个上游事件应分别更新对应字段
        PrerequisiteChecker::update_state(&mut state, &make_osa_event());
        assert!(state.osa_done);
        assert!(!state.tools_routed);
        assert!(!state.expert_routed);

        PrerequisiteChecker::update_state(&mut state, &make_tools_routed_event());
        assert!(state.osa_done);
        assert!(state.tools_routed);
        assert!(!state.expert_routed);

        PrerequisiteChecker::update_state(&mut state, &make_expert_routed_event());
        assert!(state.osa_done);
        assert!(state.tools_routed);
        assert!(state.expert_routed);
    }

    #[test]
    fn test_check_idempotent_multiple_calls() {
        let bus = EventBus::new();
        let checker = PrerequisiteChecker::new(&bus);

        // 发布全部事件
        bus.publish_blocking(make_osa_event()).expect("发布失败");
        bus.publish_blocking(make_tools_routed_event())
            .expect("发布失败");
        bus.publish_blocking(make_expert_routed_event())
            .expect("发布失败");

        // 多次 check 应都返回 Ok(状态不回退)
        assert!(checker.check().is_ok());
        assert!(checker.check().is_ok());
        assert!(checker.check().is_ok());
    }

    #[test]
    fn test_filtered_subscriber_ignores_non_routing_events() {
        let bus = EventBus::new();
        let checker = PrerequisiteChecker::new(&bus);

        // 发布非 Routing topic 事件(Quest 生命周期)
        bus.publish_blocking(NexusEvent::QuestCreated {
            metadata: EventMetadata::new("quest-engine"),
            quest_id: "q-1".into(),
            title: "测试".into(),
            task_count: 1,
        })
        .expect("发布失败");

        // 非 Routing 事件不应影响前置校验状态
        let result = checker.check();
        assert!(result.is_err(), "非 Routing 事件不应满足前置条件");
        match result {
            Err(SesaError::PrerequisiteNotMet { missing_events }) => {
                assert_eq!(missing_events.len(), 3, "三个上游事件都应缺失");
            }
            _ => panic!("期望 PrerequisiteNotMet"),
        }
    }
}
