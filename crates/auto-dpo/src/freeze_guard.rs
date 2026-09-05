//! AutoDPO 冻结违反处置 — 回滚守卫(freeze guard)
//!
//! 对应架构层:L5 Knowledge
//!
//! # 核心职责
//!
//! 当 R2 冻结条件被触发时,遗留在「冻结窗口」内的 pending 更新需要被尽力回滚。
//! [`attempt_rollback_with_guard`] 先尽力回滚 pending 更新:
//! - **回滚成功**:静默返回,不发布任何事件;
//! - **回滚失败**:经 `EventBus` 的 **Critical mpsc 旁路** 发布
//!   [`NexusEvent::R2FreezeRollbackFailed`],使该 Critical 事件具备**真实的
//!   生产发布方**,保证治理链对该违规可见、可追踪(Ω-Event 定律 + §6.2 架构红线)。
//!
//! # 依赖方向(§2.2 依赖铁律)
//!
//! auto-dpo 属 L5 Knowledge,向下依赖 L1 的 event-bus(唯一跨层事件通道)。
//! 本模块仅使用 `event-bus` 的公共 API,不向上依赖任何 L8/L9 crate。
//!
//! # 发布语义
//!
//! 使用 `EventBus::publish_critical_blocking`(Critical 事件 mpsc 旁路),
//! 该路径同时投递到:
//! - **Critical 有界 mpsc 旁路**(`subscribe_critical_events` 可接收,防丢),
//! - broadcast 主通道(常规订阅者可见)。
//!
//! metadata.source 为 `"auto-dpo:freeze_guard"`,用于审计与依赖方向校验。

use std::sync::Arc;
use thiserror::Error;

use event_bus::{EventBus, NexusEvent};
use nexus_contracts::EventMetadata;

/// 本次发布方的事件元数据 source 标识(用于审计与依赖方向校验)
const PUBLISHER_SOURCE: &str = "auto-dpo:freeze_guard";

/// pending 更新的状态标志
///
/// WHY 三态枚举:刻画「冻结窗口」内一个更新的生命周期,
/// 使 rollback 具备可判定的成功/失败语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PendingState {
    /// 尚待应用的自由更新,可安全回滚(复位标志并丢弃)
    Pending,
    /// 更新已进入「应用中」阶段,中途状态无法安全回滚
    Applying,
    /// 已回滚(已丢弃),无需重复处置
    RolledBack,
}

/// 一个仍在「冻结窗口」内的 pending 更新
#[derive(Debug, Clone)]
pub struct PendingUpdate {
    /// 更新的唯一标识(用于审计日志)
    pub update_id: String,
    /// pending 状态标志(回滚操作的目标)
    pub state: PendingState,
}

impl PendingUpdate {
    /// 创建处于 `Pending`(可回滚)状态的 pending 更新
    pub fn new(update_id: impl Into<String>) -> Self {
        Self {
            update_id: update_id.into(),
            state: PendingState::Pending,
        }
    }

    /// 尽力回滚：把 pending 状态标志复位/丢弃(`Pending` → `RolledBack`)
    ///
    /// # 返回
    /// - `Ok(())`:已成功复位并丢弃;
    /// - `Err(..)`:处于无法安全回滚的状态(应用中 / 已回滚),需要上游处置。
    pub fn rollback(&mut self) -> Result<(), RollbackFailure> {
        match self.state {
            PendingState::Pending => {
                self.state = PendingState::RolledBack;
                Ok(())
            }
            PendingState::Applying => Err(RollbackFailure(format!(
                "update '{}' is mid-apply; cannot safely rollback",
                self.update_id
            ))),
            PendingState::RolledBack => Err(RollbackFailure(format!(
                "update '{}' already rolled back",
                self.update_id
            ))),
        }
    }
}

/// 回滚失败携带的人类可读原因
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RollbackFailure(pub String);

/// 冻结回滚守卫错误
///
/// WHY thiserror 库层错误(§4.1):本模块属库层(library),提供结构化错误
/// 供调用方按严重级别处置。
#[derive(Debug, Error)]
pub enum FreezeGuardError {
    /// 回滚失败 — 已尽力但无法复位/discard pending 更新
    #[error("rollback failed: {reason}")]
    RollbackFailed {
        /// 回滚失败的人类可读原因
        reason: String,
    },
    /// 发布 `R2FreezeRollbackFailed` 事件失败(Critical 旁路投递失败)
    #[error("failed to publish R2FreezeRollbackFailed via critical channel: {reason}")]
    PublishFailed {
        /// 事件总线发布错误的人类可读描述
        reason: String,
    },
}

/// 冻结违反处置回滚守卫
///
/// 先尽力回滚 `pending` 更新:
/// - **成功**:静默返回 `Ok(())`,不发布任何事件;
/// - **失败**:经 Critical mpsc 旁路发布 [`NexusEvent::R2FreezeRollbackFailed`]
///   (metadata.source = `"auto-dpo:freeze_guard"`),并返回 [`FreezeGuardError::RollbackFailed`]。
///
/// # 设计决策(WHY)
/// - 传入共享 `Arc<EventBus>` 而非持有总线:本守卫是无状态工具函数,
///   总线由调用方持有,避免与既有 `PreferencePairGenerator` 的总线命名冲突。
/// - `publish_critical_blocking` 而非 `publish_blocking`:前者显式走
///   Critical mpsc 旁路(§4.2 反模式 6 说明 `publish_blocking` 仅为同步形式;
///   Critical 安全事件需旁路投递保证),确保即使 broadcast 出现 Lagged 也不丢事件。
pub fn attempt_rollback_with_guard(
    bus: &Arc<EventBus>,
    pending: &mut PendingUpdate,
) -> Result<(), FreezeGuardError> {
    match pending.rollback() {
        // 回滚成功:静默,不发布任何事件
        Ok(()) => Ok(()),
        // 回滚失败:尽力发布 Critical 告警以形成处置链,再把失败上抛给调用方
        Err(RollbackFailure(reason)) => {
            let event = NexusEvent::R2FreezeRollbackFailed {
                metadata: EventMetadata::new(PUBLISHER_SOURCE),
                reason: reason.clone(),
            };
            bus.publish_critical_blocking(event).map_err(|err| {
                FreezeGuardError::PublishFailed {
                    reason: err.to_string(),
                }
            })?;
            Err(FreezeGuardError::RollbackFailed { reason })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造独立的共享内存总线(避免测试间互相污染)
    fn make_bus() -> Arc<EventBus> {
        Arc::new(EventBus::new())
    }

    /// 构造一个处于「应用中」状态、必然回滚失败的 pending 更新
    fn refused_pending(id: &str) -> PendingUpdate {
        PendingUpdate {
            update_id: id.to_string(),
            state: PendingState::Applying,
        }
    }

    // ============================================================
    // 单测 1:回滚成功 -> 静默
    // ============================================================

    #[test]
    fn rollback_success_is_silent() {
        let bus = make_bus();
        // 必须在发布前订阅 Critical 旁路(§4.4 反模式 3)
        let mut rx = bus.subscribe_critical_events();
        let mut pending = PendingUpdate::new("upd-1");

        let result = attempt_rollback_with_guard(&bus, &mut pending);

        assert!(result.is_ok(), "pending 更新应被成功回滚");
        assert_eq!(
            pending.state,
            PendingState::RolledBack,
            "回滚后状态标志应被复位/丢弃"
        );
        // 成功回滚 => 不发布任何 Critical 事件
        assert!(rx.try_recv().is_err(), "回滚成功不应发布任何事件(静默)");
    }

    // ============================================================
    // 单测 2:回滚失败 -> 发布 R2FreezeRollbackFailed
    // ============================================================

    #[test]
    fn rollback_failure_publishes_r2_freeze_rollback_failed() {
        let bus = make_bus();
        let mut rx = bus.subscribe_critical_events();
        let mut pending = refused_pending("upd-2");

        let result = attempt_rollback_with_guard(&bus, &mut pending);

        assert!(matches!(
            result,
            Err(FreezeGuardError::RollbackFailed { .. })
        ));
        let event = rx.try_recv().expect("回滚失败必须发布 Critical 事件");
        assert!(
            matches!(event, NexusEvent::R2FreezeRollbackFailed { .. }),
            "应发布 R2FreezeRollbackFailed 变体"
        );
    }

    // ============================================================
    // 单测 3:旁路可达 + severity == Critical + metadata.source 识别
    // ============================================================

    #[test]
    fn critical_bypass_reachable_and_severity_critical() {
        use event_bus::EventSeverity;

        let bus = make_bus();
        let mut rx = bus.subscribe_critical_events();
        let mut pending = refused_pending("upd-3");

        let _ = attempt_rollback_with_guard(&bus, &mut pending);

        let event = rx.try_recv().expect("Critical 旁路必须可到达");
        assert!(matches!(event, NexusEvent::R2FreezeRollbackFailed { .. }));
        // §6.2 红线:BudgetExceeded 家族/冻结处置告警必须为 Critical
        assert_eq!(
            event.severity(),
            EventSeverity::Critical,
            "R2FreezeRollbackFailed 必须为 Critical 级"
        );
        // 发布方标识正确,便于依赖方向校验与审计
        assert_eq!(event.metadata().source, PUBLISHER_SOURCE);
    }

    // ============================================================
    // 单测 4:PendingUpdate::rollback 直接语义
    // ============================================================

    #[test]
    fn rollback_direct_state_transitions() {
        let mut p = PendingUpdate::new("upd-4");
        assert!(p.rollback().is_ok());
        assert_eq!(p.state, PendingState::RolledBack);
        // 已回滚再次回滚 => 失败
        assert!(p.rollback().is_err());
        // 应用中 => 失败
        let mut applying = refused_pending("upd-4b");
        assert!(applying.rollback().is_err());
        assert_eq!(applying.state, PendingState::Applying);
    }
}
