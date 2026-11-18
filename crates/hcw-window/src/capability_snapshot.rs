//! 能力档案快照 — `ContextCapabilityUpdated` → L2 只读消费（本 Spec A1）
//!
//! # 职责
//! 把 L10 mca-gateway 探针广播的能力档案（标称/有效窗口 + 置信度）在 L2 侧
//! 维护为可按路由键查询的快照，供 [`crate::governor::AdaptiveGovernor`] 的
//! `PlanContext.effective_window` 读取。
//!
//! # 依赖方向（蓝图 §5.2 铁律）
//! 事件由 L10 发布，本模块只订阅 event-bus；hcw-window（L2）不 import
//! mca-gateway（L10），也不持有其句柄。数据经 L0 契约 + 事件中转。
//!
//! # 失败语义
//! - 事件载荷非法（`effective_window == 0` 或 `> advertised_window`）⇒ 保留旧值；
//! - 未知路由键 ⇒ 只入库，不改变 active route；
//! - 无 active route ⇒ `plan_context` 返回 `None`，调用方走静态回退。
//!
//! # 锁纪律
//! 快照只有短临界区（HashMap 读/写），不跨 `.await`；订阅任务在锁外 await。

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use event_bus::EventBus;
use nexus_contracts::mca::context_capability::CalibrationTier;
use nexus_events::NexusEvent;

use crate::governor::PlanContext;

/// 单路由的窗口能力快照（事件可携带的最小决策集）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CapabilityWindow {
    /// 标称窗口（官方）
    pub advertised_window: u32,
    /// 有效窗口（探针标定或先验）
    pub effective_window: u32,
    /// 标定置信度
    pub confidence: CalibrationTier,
}

#[derive(Debug, Default)]
struct SnapshotInner {
    windows: HashMap<String, CapabilityWindow>,
    active_route: Option<String>,
}

/// 能力档案快照（`Clone` 共享同一份状态）
#[derive(Debug, Clone, Default)]
pub struct CapabilitySnapshot {
    inner: Arc<RwLock<SnapshotInner>>,
}

impl CapabilitySnapshot {
    /// 创建空快照
    pub fn new() -> Self {
        Self::default()
    }

    /// 写入/更新一条路由的能力窗口
    ///
    /// 非法载荷（有效窗口为 0 或超过标称窗口）返回 `false` 且不改变旧值。
    pub fn upsert(
        &self,
        route_key: impl Into<String>,
        advertised_window: u32,
        effective_window: u32,
        confidence: CalibrationTier,
    ) -> bool {
        if effective_window == 0 || effective_window > advertised_window {
            return false;
        }
        let mut guard = match self.inner.write() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.windows.insert(
            route_key.into(),
            CapabilityWindow {
                advertised_window,
                effective_window,
                confidence,
            },
        );
        true
    }

    /// 设置当前请求使用的路由键（无对应档案时仍可设置，读取时返回 None）
    pub fn set_active_route(&self, route_key: impl Into<String>) {
        let mut guard = match self.inner.write() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.active_route = Some(route_key.into());
    }

    /// 读取指定路由的能力窗口
    pub fn get(&self, route_key: &str) -> Option<CapabilityWindow> {
        let guard = match self.inner.read() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.windows.get(route_key).copied()
    }

    /// 读取 active route 的能力窗口
    pub fn active(&self) -> Option<CapabilityWindow> {
        let guard = match self.inner.read() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard
            .active_route
            .as_deref()
            .and_then(|key| guard.windows.get(key))
            .copied()
    }

    /// 生成 governor 输入（无 active 档案时返回 None ⇒ 静态回退）
    pub fn plan_context(
        &self,
        budget_utilization: f32,
        thinking_mode: impl Into<String>,
    ) -> Option<PlanContext> {
        let window = self.active()?;
        Some(PlanContext {
            effective_window: window.effective_window,
            budget_utilization,
            thinking_mode: thinking_mode.into(),
        })
    }

    /// 应用一条事件（纯函数，测试可直接调用）；返回是否更新
    pub fn apply_event(&self, event: &NexusEvent) -> bool {
        let NexusEvent::ContextCapabilityUpdated {
            route_key,
            advertised_window,
            effective_window,
            confidence,
            ..
        } = event
        else {
            return false;
        };
        let tier = parse_confidence(confidence);
        self.upsert(
            route_key.clone(),
            *advertised_window,
            *effective_window,
            tier,
        )
    }

    /// 订阅 `ContextCapabilityUpdated` 并后台刷新快照
    ///
    /// 调用方负责保留/终止返回的 JoinHandle（关键路径禁 fire-and-forget）。
    pub fn spawn_refresh(self: &Arc<Self>, bus: EventBus) -> tokio::task::JoinHandle<()> {
        // 铁律：先 subscribe 再 spawn，否则事件静默丢失
        let mut rx = bus.subscribe();
        let snapshot = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        snapshot.apply_event(&event);
                    }
                    Err(event_bus::EventBusError::SlowConsumerDropped { lag, .. }) => {
                        tracing::warn!(lag, "能力档案快照订阅落后，继续消费最新事件");
                    }
                    Err(event_bus::EventBusError::ChannelClosed) => break,
                    Err(other) => {
                        tracing::warn!(error = %other, "能力档案快照订阅异常，继续消费");
                    }
                }
            }
        })
    }
}

/// 置信度字符串 → 枚举（未知值按保守推断处理）
fn parse_confidence(raw: &str) -> CalibrationTier {
    match raw {
        "own_probe" => CalibrationTier::OwnProbe,
        "external_prior" => CalibrationTier::ExternalPrior,
        _ => CalibrationTier::Inferred,
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use nexus_events::EventMetadata;

    fn event(route: &str, advertised: u32, effective: u32, confidence: &str) -> NexusEvent {
        NexusEvent::ContextCapabilityUpdated {
            metadata: EventMetadata::new("mca-gateway"),
            route_key: route.into(),
            advertised_window: advertised,
            effective_window: effective,
            confidence: confidence.into(),
        }
    }

    #[test]
    fn valid_event_updates_snapshot() {
        let snap = CapabilitySnapshot::new();
        snap.set_active_route("deepseek/v4");
        assert!(snap.apply_event(&event("deepseek/v4", 1_000_000, 600_000, "own_probe")));
        let w = snap.active().expect("active 档案应存在");
        assert_eq!(w.effective_window, 600_000);
        assert_eq!(w.confidence, CalibrationTier::OwnProbe);
        let ctx = snap
            .plan_context(0.0, "Standard")
            .expect("应生成 PlanContext");
        assert_eq!(ctx.effective_window, 600_000);
    }

    #[test]
    fn invalid_event_keeps_old_value() {
        let snap = CapabilitySnapshot::new();
        snap.set_active_route("r");
        assert!(snap.apply_event(&event("r", 1_000_000, 500_000, "external_prior")));
        // effective > advertised：非法，保留旧值
        assert!(!snap.apply_event(&event("r", 1_000_000, 1_200_000, "own_probe")));
        assert_eq!(snap.active().expect("旧值仍在").effective_window, 500_000);
        // effective == 0：非法
        assert!(!snap.apply_event(&event("r", 1_000_000, 0, "inferred")));
        assert_eq!(snap.active().expect("旧值仍在").effective_window, 500_000);
    }

    #[test]
    fn unknown_route_does_not_replace_active() {
        let snap = CapabilitySnapshot::new();
        snap.set_active_route("known");
        assert!(snap.apply_event(&event("known", 100, 80, "external_prior")));
        assert!(snap.apply_event(&event("other", 200, 120, "external_prior")));
        assert_eq!(snap.active().expect("active 不变").effective_window, 80);
    }

    #[test]
    fn no_active_route_yields_none() {
        let snap = CapabilitySnapshot::new();
        assert!(snap.plan_context(0.0, "Fast").is_none());
    }
}
